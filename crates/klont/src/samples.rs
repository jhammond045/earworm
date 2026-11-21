//! The sample pool (`LOAD_SAMPLE`, docs/synth.md "Engine changes"): up to
//! MAX_SAMPLES mono buffers in one block of SAMPLE_POOL_FLOATS floats,
//! allocated at start-up. A sample is an Influence source, a take's audio or
//! an imported file; grains read them.
//!
//! Each loaded buffer is a region. A region stays while anything uses it:
//! the sample table, or a grain cloud still fading out of it after the sample
//! was replaced, so a replacement crossfades instead of cutting. When a new
//! sample does not fit at the end of the pool, the live regions are moved
//! down to close the gaps (as the curve pool does), and region handles stay
//! valid because readers hold a handle, not an offset.

use crate::protocol::{MAX_SAMPLES, SAMPLE_POOL_FLOATS};

/// Regions: one per sample plus one being faded out per layer.
pub const MAX_REGIONS: usize = MAX_SAMPLES + 2 * crate::protocol::MAX_LAYERS;

#[derive(Clone, Copy, Debug, Default)]
struct Region {
    off: usize,
    len: usize,
    refs: u32,
}

/// What a reader needs to play a sample. Equal when it is the same region
/// with the same header (bit for bit, so an unpitched NaN equals itself).
#[derive(Clone, Copy, Debug)]
pub struct SampleRef {
    pub region: u16,
    pub n: usize,
    pub rate: f32,
    /// Cents above C0 of the sample's pitch; NaN unpitched.
    pub ref_cents: f32,
}

impl PartialEq for SampleRef {
    fn eq(&self, o: &Self) -> bool {
        self.region == o.region && self.n == o.n && self.rate.to_bits() == o.rate.to_bits() && self.ref_cents.to_bits() == o.ref_cents.to_bits()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SampleError {
    TooBig,
}

pub struct SamplePool {
    data: Vec<f32>,
    regions: [Region; MAX_REGIONS],
    samples: [Option<SampleRef>; MAX_SAMPLES],
    end: usize,
}

impl Default for SamplePool {
    fn default() -> Self {
        Self::new()
    }
}

impl SamplePool {
    pub fn new() -> Self {
        SamplePool {
            data: vec![0.0; SAMPLE_POOL_FLOATS],
            regions: [Region::default(); MAX_REGIONS],
            samples: [None; MAX_SAMPLES],
            end: 0,
        }
    }

    pub fn get(&self, id: usize) -> Option<SampleRef> {
        self.samples.get(id).copied().flatten()
    }

    pub fn data(&self, region: u16) -> &[f32] {
        let r = &self.regions[region as usize];
        &self.data[r.off..r.off + r.len]
    }

    pub fn acquire(&mut self, region: u16) {
        self.regions[region as usize].refs += 1;
    }

    pub fn release(&mut self, region: u16) {
        let r = &mut self.regions[region as usize];
        r.refs = r.refs.saturating_sub(1);
        if r.refs == 0 {
            r.len = 0;
        }
    }

    fn live_floats(&self) -> usize {
        self.regions.iter().filter(|r| r.refs > 0).map(|r| r.len).sum()
    }

    /// Move the live regions down so they sit end to end from 0.
    fn compact(&mut self) {
        let mut order = [0u16; MAX_REGIONS];
        let mut n = 0;
        for (i, r) in self.regions.iter().enumerate() {
            if r.refs > 0 {
                order[n] = i as u16;
                n += 1;
            }
        }
        // Insertion sort by offset: never allocates.
        for i in 1..n {
            let mut j = i;
            while j > 0 && self.regions[order[j - 1] as usize].off > self.regions[order[j] as usize].off {
                order.swap(j - 1, j);
                j -= 1;
            }
        }
        let mut at = 0;
        for &i in &order[..n] {
            let r = &mut self.regions[i as usize];
            if r.off != at {
                self.data.copy_within(r.off..r.off + r.len, at);
                r.off = at;
            }
            at += r.len;
        }
        self.end = at;
    }

    /// Replace sample `id` with `n` little-endian f32 samples from `bytes`
    /// (n = 0 clears it). The old data stays for any reader that holds it.
    pub fn load(&mut self, id: usize, rate: f32, ref_cents: f32, n: usize, bytes: &[u8]) -> Result<Option<SampleRef>, SampleError> {
        let old = self.samples[id];
        let freed = old.map_or(0, |o| if self.regions[o.region as usize].refs == 1 { o.n } else { 0 });
        if n > 0 && self.live_floats() - freed + n > SAMPLE_POOL_FLOATS {
            return Err(SampleError::TooBig);
        }
        if let Some(o) = old {
            self.release(o.region);
        }
        self.samples[id] = None;
        if n == 0 {
            return Ok(None);
        }
        let Some(slot) = self.regions.iter().position(|r| r.refs == 0) else { return Err(SampleError::TooBig) };
        if self.end + n > SAMPLE_POOL_FLOATS {
            self.compact();
        }
        let off = self.end;
        for (d, c) in self.data[off..off + n].iter_mut().zip(bytes.chunks_exact(4)) {
            let x = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            *d = if x.is_finite() { x } else { 0.0 };
        }
        self.end = off + n;
        self.regions[slot] = Region { off, len: n, refs: 1 };
        let s = SampleRef { region: slot as u16, n, rate, ref_cents };
        self.samples[id] = Some(s);
        Ok(Some(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn load_replace_and_keep_old_data_for_a_reader() {
        let mut p = SamplePool::new();
        let a = p.load(3, 48000.0, f32::NAN, 4, &bytes(&[1.0, 2.0, 3.0, 4.0])).unwrap().unwrap();
        assert_eq!(p.data(a.region), &[1.0, 2.0, 3.0, 4.0]);
        p.acquire(a.region); // a grain cloud reads it
        let b = p.load(3, 48000.0, f32::NAN, 2, &bytes(&[5.0, 6.0])).unwrap().unwrap();
        assert_ne!(a.region, b.region);
        assert_eq!(p.data(a.region), &[1.0, 2.0, 3.0, 4.0], "still there while the reader fades");
        p.release(a.region);
        assert_eq!(p.get(3), Some(b));
        assert_eq!(p.load(3, 48000.0, f32::NAN, 0, &[]).unwrap(), None);
        assert_eq!(p.get(3), None);
    }

    #[test]
    fn compaction_makes_room_and_too_big_is_refused() {
        let mut p = SamplePool::new();
        let half = SAMPLE_POOL_FLOATS / 2;
        let big = vec![0u8; half * 4];
        p.load(0, 48000.0, f32::NAN, half, &big).unwrap();
        p.load(1, 48000.0, f32::NAN, half - 10, &big[..(half - 10) * 4]).unwrap();
        // Freeing 0 leaves a hole at the start; a new half only fits after compaction.
        p.load(0, 48000.0, f32::NAN, 0, &[]).unwrap();
        let mut tagged = big.clone();
        tagged[..4].copy_from_slice(&7.0f32.to_le_bytes());
        let s = p.load(2, 48000.0, f32::NAN, half, &tagged).unwrap().unwrap();
        assert_eq!(p.data(s.region)[0], 7.0);
        assert_eq!(p.load(3, 48000.0, f32::NAN, 20, &[0u8; 80]), Err(SampleError::TooBig));
    }
}
