//! Poly takes in the engine (docs/poly.md, stage P3): the note-curve blob (LOAD_NOTE_CURVES), the
//! v4 track blob whose notes name their note curve, and voices that read their own note's curve
//! for their `voice.*` sources. The recorded-take check (record, commit, play back, export) is in
//! web/src/record/poly.test.ts.

use klont::build::{self, RouteSpec, VoiceSpec};
use klont::notecurve::{NOTE_CURVE_MAGIC, NOTE_REC};
use klont::patch::*;
use klont::protocol::*;
use klont::seq::{FEEL_FREE, TRACK_MAGIC3, TRACK_MAGIC4};
use klont::Engine;
use params as P;

const SR: f32 = 48_000.0;

struct W(Vec<u8>);
impl W {
    fn u(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn f(&mut self, x: f32) {
        self.u(x.to_bits());
    }
}

/// A note: t, dur, cents, note-curve slot (-1 none), index.
type N = (f32, f32, f32, i32, u32);

/// One free section `len` seconds long, one clip on layer 0 with these notes, no take curve, a
/// warp map reading take seconds at clip seconds. `magic` TRACK_MAGIC4 or 3.
fn track(magic: u32, len: f32, notes: &[N]) -> Vec<u8> {
    let mut w = W(Vec::new());
    for x in [magic, 1, 1, notes.len() as u32, 2, 0, 0, 0] {
        w.u(x);
    }
    w.u(FEEL_FREE);
    w.f(len);
    w.f(0.0);
    w.u(1);
    w.f(0.0);
    w.u(1);
    w.u(0);
    w.u(0);
    for x in [0, 0, len.to_bits(), 0, notes.len() as u32, u32::MAX, 0, 2, 0, 0, u32::MAX, 0, 0, 0, 0, 0] {
        w.u(x);
    }
    for &(t, dur, cents, slot, index) in notes {
        w.f(t);
        w.f(dur);
        w.f(cents);
        w.f(0.0);
        w.u(0);
        w.f(1.0);
        (6..14).for_each(|_| w.f(0.0));
        w.u(slot as u32);
        w.u(index);
    }
    for (take, clip) in [(0.0f32, 0.0f32), (len, len)] {
        w.f(take);
        w.f(clip);
    }
    w.u(0);
    w.0
}

/// Note curves: per note, its start (take seconds) and its records.
fn curves(notes: &[(f32, Vec<[f32; NOTE_REC]>)]) -> Vec<u8> {
    let mut w = W(Vec::new());
    let n_rec: usize = notes.iter().map(|n| n.1.len()).sum();
    w.u(NOTE_CURVE_MAGIC);
    w.u(notes.len() as u32);
    w.u(n_rec as u32);
    let mut first = 0;
    for (start, recs) in notes {
        w.f(*start);
        w.u(first);
        w.u(recs.len() as u32);
        first += recs.len() as u32;
    }
    for (_, recs) in notes {
        for r in recs {
            r.iter().for_each(|&x| w.f(x));
        }
    }
    w.0
}

/// A sine into an amp the first envelope opens, its gain scaled by the voice's volume.
fn voice() -> VoiceSpec {
    let mut v = build::simple_voice(&[P::T_OSC]);
    v.set.push((0, P::osc::WAVE, 0.0));
    v.set.push((1, P::amp::GAIN, 1.0));
    v.routes.push(RouteSpec { mode: MODE_SCALE, polarity: POL_DOWN, ..RouteSpec::new(SRC_VOICE_VOLUME, 0, 1, P::amp::GAIN, 1.0) });
    v
}

fn render(track: &[u8], curves: &[u8], secs: f32) -> Vec<f32> {
    let mut e = Engine::new(SR, 4096);
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &build::patch(&[voice()], &[], &[])), LOAD_OK);
    assert_eq!(e.load_bytes(5, LOAD_NOTE_CURVES, curves), LOAD_OK);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, track), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let n = (secs * SR) as usize;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let k = 4096.min(n - out.len());
        e.render(k);
        out.extend_from_slice(&e.output().0[..k]);
    }
    out
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

/// Frequency from upward zero crossings.
fn freq(x: &[f32]) -> f32 {
    let ups: Vec<usize> = (1..x.len()).filter(|&i| x[i - 1] < 0.0 && x[i] >= 0.0).collect();
    (ups.len() - 1) as f32 * SR / (ups[ups.len() - 1] - ups[0]) as f32
}

/// `n` records at volume `v`, pitch offset `p(i)`.
fn recs(n: usize, v: impl Fn(usize) -> f32, p: impl Fn(usize) -> f32) -> Vec<[f32; NOTE_REC]> {
    (0..n).map(|i| [p(i), v(i), 0.5, 0.3, if i == 0 { 1.0 } else { 0.0 }]).collect()
}

#[test]
fn a_voice_follows_its_notes_pitch_offset() {
    // A3 bent up an octave over the first second, then held.
    let c = curves(&[(0.5, recs(150, |_| 1.0, |i| 1200.0 * (i as f32 / 100.0).min(1.0)))]);
    let out = render(&track(TRACK_MAGIC4, 3.0, &[(0.5, 1.5, 4500.0, 5, 0)]), &c, 2.0);
    let at = |a: f32, b: f32| freq(&out[(a * SR) as usize..(b * SR) as usize]);
    let (start, end) = (at(0.52, 0.56), at(1.6, 1.9));
    println!("bend: {start:.1} Hz at the start, {end:.1} Hz after it");
    assert!((start - 220.0).abs() < 8.0, "{start}");
    assert!((end - 440.0).abs() < 4.0, "{end}");
}

#[test]
fn a_voice_follows_its_notes_volume_and_the_others_do_not_touch_it() {
    // Note 0 (A3) at full volume throughout; note 1 (E4), same time, silent after 0.3 s.
    let c = curves(&[
        (0.0, recs(200, |_| 1.0, |_| 0.0)),
        (0.0, recs(200, |i| if i < 30 { 1.0 } else { 0.0 }, |_| 0.0)),
    ]);
    let both = render(&track(TRACK_MAGIC4, 3.0, &[(0.0, 2.0, 4500.0, 5, 0), (0.0, 2.0, 5200.0, 5, 1)]), &c, 2.0);
    let alone = render(&track(TRACK_MAGIC4, 3.0, &[(0.0, 2.0, 4500.0, 5, 0)]), &c, 2.0);
    let s = |a: f32, b: f32| ((a * SR) as usize, (b * SR) as usize);
    let (a, b) = s(0.6, 1.9);
    assert!(rms(&alone[a..b]) > 0.05, "the first note sounds");
    assert_eq!(&both[a..b], &alone[a..b], "a silent second note leaves the first one's output bit for bit as it was");
    let (a, b) = s(0.05, 0.25);
    assert!(rms(&both[a..b]) > 1.2 * rms(&alone[a..b]), "both sound before the second note's volume drops");
}

#[test]
fn track_blobs_without_note_curves_still_load_and_bad_slots_do_not() {
    let mut e = Engine::new(SR, 128);
    let v3 = track(TRACK_MAGIC3, 2.0, &[(0.0, 1.0, 4500.0, 0, 0)]);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &v3), LOAD_OK, "v3: words 14 and 15 are not read");
    let bad = track(TRACK_MAGIC4, 2.0, &[(0.0, 1.0, 4500.0, MAX_CURVES as i32, 0)]);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &bad), LOAD_MALFORMED);
    let none = track(TRACK_MAGIC4, 2.0, &[(0.0, 1.0, 4500.0, -1, 0)]);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &none), LOAD_OK);
    assert_eq!(e.load_bytes(MAX_CURVES as u32, LOAD_NOTE_CURVES, &curves(&[(0.0, recs(3, |_| 1.0, |_| 0.0))])), LOAD_BAD_SLOT);
}

#[test]
fn a_note_whose_curve_is_missing_plays_at_its_own_pitch() {
    // The note names index 3 of a slot that holds one note: it plays as a note with no curve.
    let c = curves(&[(0.0, recs(100, |_| 0.0, |_| 1200.0))]);
    let out = render(&track(TRACK_MAGIC4, 2.0, &[(0.0, 1.5, 4500.0, 5, 3)]), &c, 1.0);
    let f = freq(&out[(0.3 * SR) as usize..(0.9 * SR) as usize]);
    assert!((f - 220.0).abs() < 3.0, "{f}");
}
