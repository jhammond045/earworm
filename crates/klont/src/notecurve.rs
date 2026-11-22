//! Note curves: the curve of each note of a poly take (docs/poly.md "Recording a poly take",
//! stage P3), read by the voice that plays the note. A take's mono curve (`curve.rs`) holds what
//! the whole input did; a note curve holds what one note did: its pitch offset from the note,
//! volume, intensity, hardness and onset, 100 records a second over the note's life.
//!
//! # Note-curve blob (`ew_load` kind LOAD_NOTE_CURVES, slot = note-curve id)
//!
//! ```text
//! u32 magic NOTE_CURVE_MAGIC, u32 n_notes, u32 n_records,
//! n_notes x 3 words: f32 start (take seconds), u32 first record, u32 record count (>= 1)
//! n_records x NOTE_REC f32: pitch offset (cents from the note's own pitch), volume (0..1),
//!   intensity (0..1), hardness (0..1), onset (0 or 1)
//! ```
//! Record r of a note is at take time start + r / 100 s.

use crate::protocol::{MAX_CURVES, NOTE_CURVE_POOL_FLOATS};

pub const NOTE_CURVE_MAGIC: u32 = 0x314e_5745; // "EWN1"
/// Floats per record.
pub const NOTE_REC: usize = 5;
pub const PITCH: usize = 0;
pub const VOLUME: usize = 1;
pub const INTENSITY: usize = 2;
pub const HARDNESS: usize = 3;
pub const ONSET: usize = 4;
const RATE: f64 = 100.0;
/// Floats per note in the pool: start, first record, record count.
const NOTE_WORDS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteCurveError {
    BadSlot,
    BadMagic,
    TooBig,
    Malformed,
}

#[derive(Clone, Copy, Default)]
struct Slot {
    off: usize,
    notes: usize,
    /// Floats in use (notes and records), and reserved at `off`.
    len: usize,
    cap: usize,
}

/// All note curves in one pool allocated in `new`, managed as `CurvePool` does: a load reuses its
/// slot's space when it fits, else goes after the used part, else the live curves are moved to the
/// front first. A failed load changes nothing.
pub struct NoteCurves {
    data: Vec<f32>,
    slots: [Slot; MAX_CURVES],
    used: usize,
}

impl Default for NoteCurves {
    fn default() -> Self {
        Self::new()
    }
}

fn word(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]])
}

impl NoteCurves {
    pub fn new() -> Self {
        NoteCurves { data: vec![0.0; NOTE_CURVE_POOL_FLOATS], slots: [Slot::default(); MAX_CURVES], used: 0 }
    }

    fn compact(&mut self, skip: usize) {
        self.slots[skip] = Slot::default();
        let mut buf = [0u8; MAX_CURVES];
        let mut n = 0;
        for s in 0..MAX_CURVES {
            if self.slots[s].len > 0 {
                let mut k = n;
                while k > 0 && self.slots[buf[k - 1] as usize].off > self.slots[s].off {
                    buf[k] = buf[k - 1];
                    k -= 1;
                }
                buf[k] = s as u8;
                n += 1;
            }
        }
        let mut at = 0;
        for &s in &buf[..n] {
            let slot = &mut self.slots[s as usize];
            if slot.off != at {
                self.data.copy_within(slot.off..slot.off + slot.len, at);
                slot.off = at;
            }
            slot.cap = slot.len;
            at += slot.len;
        }
        self.used = at;
    }

    pub fn load(&mut self, slot: usize, b: &[u8]) -> Result<(), NoteCurveError> {
        if slot >= MAX_CURVES {
            return Err(NoteCurveError::BadSlot);
        }
        if b.len() < 12 || b.len() % 4 != 0 {
            return Err(NoteCurveError::Malformed);
        }
        if word(b, 0) != NOTE_CURVE_MAGIC {
            return Err(NoteCurveError::BadMagic);
        }
        let (notes, recs) = (word(b, 1) as usize, word(b, 2) as usize);
        let need = notes
            .checked_mul(NOTE_WORDS)
            .and_then(|a| recs.checked_mul(NOTE_REC).and_then(|r| a.checked_add(r)))
            .ok_or(NoteCurveError::TooBig)?;
        if need > NOTE_CURVE_POOL_FLOATS {
            return Err(NoteCurveError::TooBig);
        }
        if b.len() != 12 + need * 4 {
            return Err(NoteCurveError::Malformed);
        }
        for i in 0..notes {
            let start = f32::from_bits(word(b, 3 + i * NOTE_WORDS));
            let (first, n) = (word(b, 4 + i * NOTE_WORDS) as usize, word(b, 5 + i * NOTE_WORDS) as usize);
            if !start.is_finite() || n == 0 || first.checked_add(n).is_none_or(|e| e > recs) {
                return Err(NoteCurveError::Malformed);
            }
        }
        let old = self.slots[slot];
        let off = if need <= old.cap {
            old.off
        } else if old.cap > 0 && old.off + old.cap == self.used && old.off + need <= NOTE_CURVE_POOL_FLOATS {
            self.used = old.off + need;
            old.off
        } else if self.used + need <= NOTE_CURVE_POOL_FLOATS {
            let off = self.used;
            self.used += need;
            off
        } else {
            let live: usize = (0..MAX_CURVES).filter(|&s| s != slot).map(|s| self.slots[s].len).sum();
            if live + need > NOTE_CURVE_POOL_FLOATS {
                return Err(NoteCurveError::TooBig);
            }
            self.compact(slot);
            let off = self.used;
            self.used += need;
            off
        };
        for (i, d) in self.data[off..off + need].iter_mut().enumerate() {
            let w = word(b, 3 + i);
            // The note table's first record and count are integers; records are f32.
            *d = if i < notes * NOTE_WORDS && i % NOTE_WORDS != 0 { w as f32 } else { f32::from_bits(w) };
        }
        let cap = if off == old.off { old.cap.max(need) } else { need };
        self.slots[slot] = Slot { off, notes, len: need, cap };
        Ok(())
    }

    /// Notes in a slot (0 when empty or out of range).
    pub fn notes(&self, slot: usize) -> usize {
        self.slots.get(slot).map_or(0, |s| s.notes)
    }

    /// Note `index` of `slot` at take time `sec`, interpolated between the two nearest records:
    /// pitch, volume and intensity linearly, hardness and onset from the nearer record. Before
    /// the first record or after the last, that record. None for a note the slot does not have.
    pub fn read(&self, slot: usize, index: usize, sec: f64) -> Option<[f32; NOTE_REC]> {
        let s = self.slots.get(slot)?;
        if index >= s.notes {
            return None;
        }
        let nt = &self.data[s.off + index * NOTE_WORDS..][..NOTE_WORDS];
        let (start, first, n) = (nt[0] as f64, nt[1] as usize, nt[2] as usize);
        let recs = &self.data[s.off + s.notes * NOTE_WORDS + first * NOTE_REC..][..n * NOTE_REC];
        let rec = |i: usize| -> [f32; NOTE_REC] { recs[i * NOTE_REC..(i + 1) * NOTE_REC].try_into().unwrap() };
        let x = (sec - start) * RATE;
        if !(x > 0.0) {
            return Some(rec(0));
        }
        if x >= (n - 1) as f64 {
            return Some(rec(n - 1));
        }
        let i = x.floor() as usize;
        let f = (x - i as f64) as f32;
        let (a, b) = (rec(i), rec(i + 1));
        let near = if f < 0.5 { a } else { b };
        Some(std::array::from_fn(|k| match k {
            HARDNESS | ONSET => near[k],
            _ => a[k] + (b[k] - a[k]) * f,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two notes: the first starts at 1 s with 3 records, the second at 2 s with 1.
    fn blob(tag: f32) -> Vec<u8> {
        let mut b = Vec::new();
        for w in [NOTE_CURVE_MAGIC, 2, 4] {
            b.extend(w.to_le_bytes());
        }
        for (start, first, n) in [(1.0f32, 0u32, 3u32), (2.0, 3, 1)] {
            b.extend(start.to_le_bytes());
            b.extend(first.to_le_bytes());
            b.extend(n.to_le_bytes());
        }
        for r in 0..4 {
            for k in 0..NOTE_REC {
                b.extend((tag + 10.0 * r as f32 + k as f32).to_le_bytes());
            }
        }
        b
    }

    #[test]
    fn reads_interpolate_and_hold() {
        let mut p = NoteCurves::new();
        p.load(3, &blob(0.0)).unwrap();
        assert_eq!(p.notes(3), 2);
        assert_eq!(p.read(3, 0, 0.5).unwrap(), [0.0, 1.0, 2.0, 3.0, 4.0], "before the note: its first record");
        // 1.25 records in: a quarter of the way from record 1 to record 2.
        let r = p.read(3, 0, 1.0125).unwrap();
        assert!((r[PITCH] - 12.5).abs() < 1e-4 && (r[VOLUME] - 13.5).abs() < 1e-4, "{r:?}");
        assert_eq!((r[HARDNESS], r[ONSET]), (13.0, 14.0), "nearer record");
        assert_eq!(p.read(3, 0, 1.017).unwrap()[ONSET], 24.0);
        assert_eq!(p.read(3, 0, 9.0).unwrap()[PITCH], 20.0, "after the note: its last record");
        assert_eq!(p.read(3, 1, 2.5).unwrap()[PITCH], 30.0);
        assert!(p.read(3, 2, 1.0).is_none() && p.read(4, 0, 1.0).is_none());
    }

    #[test]
    fn bad_blobs_change_nothing() {
        let mut p = NoteCurves::new();
        p.load(0, &blob(0.0)).unwrap();
        let mut bad = blob(5.0);
        bad[4 * 7..4 * 8].copy_from_slice(&9u32.to_le_bytes()); // first record of note 0 past the end
        assert_eq!(p.load(0, &bad), Err(NoteCurveError::Malformed));
        assert_eq!(p.load(0, &bad[..bad.len() - 4]), Err(NoteCurveError::Malformed));
        assert_eq!(p.load(MAX_CURVES, &blob(0.0)), Err(NoteCurveError::BadSlot));
        assert_eq!(p.read(0, 0, 1.0).unwrap()[PITCH], 0.0);
        p.load(0, &blob(5.0)).unwrap();
        assert_eq!(p.read(0, 0, 1.0).unwrap()[PITCH], 5.0);
    }
}
