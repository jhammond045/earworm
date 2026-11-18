//! Offline renders through the Klont engine, shared by the Influence tests:
//! the round trip (known parameters in, measured parameters out) and the
//! blind-match proxy. The layer blob is built here the same way the web
//! compiler builds it (web/src/m1/blobs.ts, crates/klont/src/protocol.rs).
#![allow(dead_code)]

use klont::Engine;
use klont::protocol::*;
use params as P;

pub const SR: f32 = 48_000.0;
/// Bit set of all five aspect groups.
pub const MASK_ALL: u32 = 0x1f;

/// A parameter set built from real-unit overrides on top of the neutral set.
pub fn real_set(over: &[(usize, f32)]) -> P::ParamSet {
    let mut s = P::neutral_set();
    for &(i, v) in over {
        s[i] = P::to_norm(i, v);
    }
    s
}

pub struct LayerSpec {
    pub base: P::ParamSet,
    /// Influence target, amount and mask; None leaves the influence off.
    pub influence: Option<(P::ParamSet, f32, u32)>,
}

impl LayerSpec {
    pub fn base(base: P::ParamSet) -> Self {
        LayerSpec { base, influence: None }
    }
}

pub fn layer_bytes(spec: &LayerSpec) -> Vec<u8> {
    let mut w = vec![0u32; LAYER_WORDS];
    w[0] = LAYER_MAGIC;
    let f = |x: f32| x.to_bits();
    let (amount, mask) = spec.influence.as_ref().map_or((0.0, 0), |i| (i.1, i.2));
    w[LAYER_OFF_FIELDS + LAYER_LEVEL as usize] = f(0.0);
    w[LAYER_OFF_FIELDS + LAYER_KIND as usize] = f(KIND_MELODY as f32);
    w[LAYER_OFF_FIELDS + LAYER_INFLUENCE_AMOUNT as usize] = f(amount);
    w[LAYER_OFF_FIELDS + LAYER_INFLUENCE_MASK as usize] = f(mask as f32);
    for i in 0..P::COUNT {
        w[LAYER_OFF_BASE + i] = f(spec.base[i]);
        let t = spec.influence.as_ref().map_or(spec.base[i], |x| x.0[i]);
        w[LAYER_OFF_INFLUENCE + i] = f(t);
        for k in 0..KIT_SLOTS {
            w[LAYER_OFF_KIT + k * P::COUNT + i] = f(P::neutral_set()[i]);
        }
    }
    for k in 0..N_MACROS {
        w[LAYER_OFF_MACROS + k] = f(0.5);
    }
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A grain blob (LOAD_GRAIN = 4): magic, sample rate, ref cents (NaN none),
/// sample count, samples.
pub fn grain_bytes(rate: f32, ref_cents: Option<f32>, data: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(16 + 4 * data.len());
    b.extend_from_slice(&0x3147_5745u32.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&ref_cents.unwrap_or(f32::NAN).to_le_bytes());
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    for v in data {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

/// True when this build of the Klont has the grain cloud (LOAD_GRAIN accepted).
pub fn engine_has_grain() -> bool {
    let mut e = Engine::new(SR, 256);
    e.load_bytes(0, 4, &grain_bytes(SR, None, &[0.0; 64])) == LOAD_OK
}

/// Render one note on layer 0: note on at 0, note off after `hold_s`, `total_s`
/// of output in all (left channel). A short silent lead-in lets the layer's
/// parameter smoothing settle and is not part of the result.
pub fn render_note(spec: &LayerSpec, grain: Option<(&[u8], u32)>, cents: f32, hold_s: f32, total_s: f32) -> Vec<f32> {
    let mut e = Engine::new(SR, 4096);
    assert_eq!(e.load_bytes(0, LOAD_LAYER, &layer_bytes(spec)), LOAD_OK);
    if let Some((bytes, kind)) = grain {
        let r = e.load_bytes(0, kind, bytes);
        assert_eq!(r, LOAD_OK, "grain blob rejected: {r}");
    }
    let mut lead = 0;
    while lead < (0.15 * SR) as usize {
        lead += e.render(4096.min((0.15 * SR) as usize - lead));
    }
    e.push(MSG_NOTE_ON, 0, 0, 0, cents, 0);
    let total = (total_s * SR) as usize;
    let off_at = ((hold_s * SR) as usize).min(total);
    let mut out = Vec::with_capacity(total);
    let mut off_sent = false;
    while out.len() < total {
        if !off_sent && out.len() >= off_at {
            e.push(MSG_NOTE_OFF, 0, 0, 0, cents, 0);
            off_sent = true;
        }
        let want = if off_sent { total - out.len() } else { off_at - out.len() };
        let k = e.render(want.min(4096));
        out.extend_from_slice(&e.output().0[..k]);
    }
    out
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}
