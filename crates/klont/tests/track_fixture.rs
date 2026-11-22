//! Cross-language check: `web/src/m1/track.test.ts` encodes a small track and
//! compares it with `tests/fixtures/track_small.bin`; this test loads the
//! same file and checks every field, then runs the transport over it.

use klont::seq::*;

const BLOB: &[u8] = include_bytes!("fixtures/track_small.bin");
const NAN: f32 = f32::NAN;

fn load() -> Track {
    let mut t = Track::new();
    t.load(BLOB).expect("fixture loads");
    t
}

#[test]
fn every_field() {
    let t = load();
    assert!(t.looping());
    assert_eq!(t.n_sections(), 2);
    assert_eq!(*t.section(0), Section { feel: FEEL_GRID, length: 3840.0, bpm: 120.0, repeats: 2, ramp_ms: 0.0, uid: 0 });
    assert_eq!(*t.section(1), Section { feel: FEEL_FREE, length: 2.5, bpm: 0.0, repeats: 1, ramp_ms: 0.0, uid: 0 });
    assert_eq!((t.n_clips(), t.n_events()), (3, 9));

    let want_clips = [
        Clip { section: 0, layer: 0, loop_len: 3840.0, first_event: 0, n_events: 3, curve: 3, first_warp: 0, n_warp: 3, first_morph: 0, n_morph: 0, audio: -1, first_lane: 0, n_lanes: 0, first_audio_warp: 0, n_audio_warp: 0 },
        Clip { section: 0, layer: 1, loop_len: 1920.0, first_event: 3, n_events: 4, curve: -1, first_warp: 3, n_warp: 0, first_morph: 0, n_morph: 0, audio: -1, first_lane: 0, n_lanes: 0, first_audio_warp: 0, n_audio_warp: 0 },
        Clip { section: 1, layer: 7, loop_len: 2.5, first_event: 7, n_events: 2, curve: -1, first_warp: 3, n_warp: 0, first_morph: 0, n_morph: 0, audio: -1, first_lane: 0, n_lanes: 0, first_audio_warp: 0, n_audio_warp: 0 },
    ];
    for (i, w) in want_clips.iter().enumerate() {
        assert_eq!(t.clip(i), w, "clip {i}");
    }
    assert_eq!(t.clip_index_for(0, 0), Some(0));
    assert_eq!(t.clip_index_for(0, 1), Some(1));
    assert_eq!(t.clip_index_for(1, 7), Some(2));
    assert_eq!(t.clip_index_for(1, 0), None);
    assert_eq!(
        t.warp(t.clip(0)),
        &[
            WarpAnchor { take_sec: 0.25, clip_t: 0.0 },
            WarpAnchor { take_sec: 1.0, clip_t: 960.0 },
            WarpAnchor { take_sec: 2.0, clip_t: 2000.0 },
        ]
    );
    assert!(t.warp(t.clip(1)).is_empty());

    #[rustfmt::skip]
    let want = [
        // t, dur, cents, hard, slot, strength, attack, burst, bend, hold, vowel, tone, end_stop, end_click
        (0.0, 480.0, 4800.0, 0.5, 0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        (960.0, 480.0, NAN, 0.25, 0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        (1920.0, 960.0, 5100.0, 0.75, 0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        (0.0, 0.0, 3600.0, 0.0, 0, 0.75, 0.5, 0.25, -0.5, 0.125, 0.75, 0.375, 0.5, 0.0625),
        (480.0, 0.0, NAN, 0.0, 1, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        (960.0, 0.0, NAN, 0.0, 2, 1.0, 0.25, 0.5, 0.5, 0.25, 0.5, 0.625, 0.25, 0.125),
        (1440.0, 0.0, NAN, 0.5, 3, 0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        (0.0, 0.5, 6000.0, 1.0, 0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        (1.25, 0.75, 6200.0, 0.5, 0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
    ];
    for (i, w) in want.iter().enumerate() {
        let e = t.event(i);
        let same = |a: f32, b: f32| a == b || (a.is_nan() && b.is_nan());
        assert_eq!((e.t, e.dur, e.slot), (w.0, w.1, w.4), "event {i}");
        assert!(same(e.cents, w.2), "event {i} cents {}", e.cents);
        let got = [e.hard, e.strength, e.attack, e.burst, e.bend, e.hold, e.vowel, e.tone, e.end_stop, e.end_click];
        let exp = [w.3, w.5, w.6, w.7, w.8, w.9, w.10, w.11, w.12, w.13];
        assert_eq!(got, exp, "event {i}");
    }
}

fn run(tr: &mut Transport, t: &Track, total: usize, span: usize) -> Vec<(usize, u32, u32, u32)> {
    let mut out = EventList::new();
    let mut got = vec![];
    let mut done = 0;
    while done < total {
        let n = span.min(total - done);
        out.clear();
        tr.advance(t, n, &mut out);
        for e in out.as_slice() {
            got.push((done + e.offset as usize, e.layer, e.event, e.dur_samples));
        }
        done += n;
    }
    got
}

#[test]
fn transport_over_fixture() {
    let t = load();
    let mut tr = Transport::new(48000.0);
    tr.play();
    // 0.04 ticks per sample: beat n is sample 24000 n.
    let got = run(&mut tr, &t, 60001, 1000);
    assert_eq!(
        got,
        vec![
            (0, 0, 0, 12000),
            (0, 1, 3, 1),
            (12000, 1, 4, 1),
            (24000, 0, 1, 12000),
            (24000, 1, 5, 1),
            (36000, 1, 6, 1),
            (48000, 0, 2, 24000),
            (48000, 1, 3, 1), // the 2-beat hit loop wraps
            (60000, 1, 4, 1),
        ]
    );
}

#[test]
fn transport_through_both_sections() {
    let t = load();
    let mut tr = Transport::new(48000.0);
    tr.play();
    // Two passes of 96000 samples, then the free section from sample 192000.
    let got = run(&mut tr, &t, 192000 + 70000, 4096);
    let l7: Vec<_> = got.iter().filter(|g| g.1 == 7).collect();
    assert_eq!(l7, vec![&(192000, 7, 7, 24000), &(252000, 7, 8, 36000)]);
    // Second pass restarts the layer 0 clip.
    assert!(got.contains(&(96000, 0, 0, 12000)));
    assert_eq!(tr.section(), 1);
    assert!((tr.pos() - 70000.0 / 48000.0).abs() < 1e-9);
    assert_eq!(tr.bpm(), 0.0);
}
