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

/// Layer blob for the classic melody patch every engine layer starts with.
pub fn layer_bytes(spec: &LayerSpec) -> Vec<u8> {
    let mut fields = klont::classic::default_fields(KIND_MELODY);
    let (amount, mask) = spec.influence.as_ref().map_or((0.0, 0), |i| (i.1, i.2));
    fields[LAYER_INFLUENCE_AMOUNT as usize] = amount;
    fields[LAYER_INFLUENCE_MASK as usize] = mask as f32;
    let target = spec.influence.as_ref().map_or(spec.base, |x| x.0);
    klont::classic::layer_blob(KIND_MELODY, &fields, &spec.base, &[P::neutral_set(); KIT_SLOTS], &[0.5; N_MACROS], &target)
}

/// A sample blob (LOAD_SAMPLE = 4) for layer 0's grains: magic, sample rate,
/// ref cents (NaN none), sample count, samples.
pub fn grain_bytes(rate: f32, ref_cents: Option<f32>, data: &[f32]) -> Vec<u8> {
    klont::classic::sample_blob(rate, ref_cents.unwrap_or(f32::NAN), data)
}

/// True when this build of the Klont has the grain cloud (a sample accepted).
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
