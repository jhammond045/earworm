//! Metronome and count-in: sample-accurate clicks, accented downbeat, kept out
//! of offline renders and out of the mic buffer.

use klont::Engine;
use klont::protocol::*;
use klont::seq::{FEEL_GRID, PPQ, TRACK_MAGIC};
use params as P;

const SR: f32 = 48_000.0;

fn track(beats: f32, bpm: f32, looping: bool) -> Vec<u8> {
    let mut w: Vec<u32> = Vec::new();
    w.extend([TRACK_MAGIC, 1, 0, 0, 0, looping as u32, 0, 0]);
    w.extend([
        FEEL_GRID,
        (beats * PPQ as f32).to_bits(),
        bpm.to_bits(),
        1,
        0f32.to_bits(),
        0,
        0,
        0,
    ]);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn note_track(beats: f32, bpm: f32) -> Vec<u8> {
    let mut w: Vec<u32> = Vec::new();
    w.extend([TRACK_MAGIC, 1, 1, 1, 0, 0, 0, 0]);
    w.extend([
        FEEL_GRID,
        (beats * PPQ as f32).to_bits(),
        bpm.to_bits(),
        1,
        0f32.to_bits(),
        0,
        0,
        0,
    ]);
    w.extend([
        0,
        0,
        (beats * PPQ as f32).to_bits(),
        0,
        1,
        (-1i32) as u32,
        0,
        0,
    ]);
    let mut r = [0u32; 16];
    r[0] = 0f32.to_bits();
    r[1] = (beats * PPQ as f32).to_bits();
    r[2] = 5700f32.to_bits();
    r[5] = 1f32.to_bits();
    w.extend(r);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn enable(e: &mut Engine, bar: f32) {
    e.push(MSG_SET_METRO, 0, 0, METRO_ON, 1.0, 0);
    e.push(MSG_SET_METRO, 0, 0, METRO_LEVEL, 0.8, 0);
    e.push(MSG_SET_METRO, 0, 0, METRO_VOICE, 0.0, 0);
    e.push(MSG_SET_METRO, 0, 0, METRO_BAR, bar, 0);
}

fn process_all(e: &mut Engine, n: usize, chunk: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let k = chunk.min(n - out.len());
        e.process(k);
        out.extend_from_slice(&e.output().0[..k]);
    }
    out
}

fn render_all(e: &mut Engine, n: usize, chunk: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let k = e.render(chunk.min(n - out.len()));
        out.extend_from_slice(&e.output().0[..k]);
    }
    out
}

/// First sample of each click: a rise above the quiet floor.
fn click_samples(x: &[f32]) -> Vec<usize> {
    let mut hits = Vec::new();
    let mut i = 0;
    while i < x.len() {
        if x[i].abs() > 0.02 {
            hits.push(i);
            i += 8_000; // one click dies well inside a beat at these tempos
        } else {
            i += 1;
        }
    }
    hits
}

#[test]
fn clicks_land_on_the_beat_and_the_downbeat_is_louder() {
    let mut e = Engine::new(SR, 4096);
    assert_eq!(
        e.load_bytes(0, LOAD_TRACK, &track(8.0, 120.0, true)),
        LOAD_OK
    );
    enable(&mut e, 4.0);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let l = process_all(&mut e, 48_000, 128); // one second, two beats at 120
    let hits = click_samples(&l);
    assert_eq!(hits.len(), 2, "clicks at {hits:?}");
    assert!(hits[0] <= 2, "downbeat at {}", hits[0]);
    assert!((24_000..24_003).contains(&hits[1]), "beat 2 at {}", hits[1]);
    let peak = |a: usize| l[a..a + 400].iter().map(|v| v.abs()).fold(0.0f32, f32::max);
    assert!(
        peak(hits[0]) > peak(hits[1]) * 1.3,
        "accent {} vs {}",
        peak(hits[0]),
        peak(hits[1])
    );
    assert_eq!(e.meter()[M_METRO], 1.0);
}

#[test]
fn click_positions_do_not_depend_on_block_size() {
    let run = |chunk: usize| {
        let mut e = Engine::new(SR, 4096);
        e.load_bytes(0, LOAD_TRACK, &track(8.0, 120.0, true));
        enable(&mut e, 4.0);
        e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
        process_all(&mut e, 48_000, chunk)
    };
    let a = run(128);
    for chunk in [32usize, 96, 100, 333] {
        let b = run(chunk);
        let first = a.iter().zip(&b).position(|(x, y)| x != y);
        assert!(a == b, "chunk {chunk} differs at {first:?}");
    }
}

#[test]
fn render_and_the_mic_do_not_contain_the_click() {
    let mut live = Engine::new(SR, 256);
    let mut off = Engine::new(SR, 256);
    live.load_bytes(0, LOAD_TRACK, &track(4.0, 120.0, false));
    off.load_bytes(0, LOAD_TRACK, &track(4.0, 120.0, false));
    enable(&mut live, 4.0);
    live.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    off.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    for s in live.input_mut().iter_mut().take(256) {
        *s = 0.2;
    }
    for s in off.input_mut().iter_mut().take(256) {
        *s = 0.2;
    }
    live.process(256);
    off.process(256);
    assert_eq!(
        &live.input()[..256],
        &off.input()[..256],
        "the click must not enter the mic buffer"
    );
    assert_eq!(
        live.frame_records(),
        off.frame_records(),
        "analysis frames come from the mic, not the click"
    );
    assert!(live.output().0.iter().any(|v| v.abs() > 0.02));
    assert!(off.output().0.iter().all(|v| v.abs() < 1e-6));

    let mut exp = Engine::new(SR, 256);
    exp.load_bytes(0, LOAD_TRACK, &track(4.0, 120.0, false));
    enable(&mut exp, 4.0);
    exp.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let rendered = render_all(&mut exp, 48_000, 256);
    assert!(
        rendered.iter().all(|v| v.abs() < 1e-6),
        "an export of an empty track stays silent with the metronome on"
    );
}

#[test]
fn count_in_is_one_bar_and_the_note_starts_on_the_downbeat() {
    let mut e = Engine::new(SR, 4096);
    e.load_bytes(0, LOAD_TRACK, &note_track(4.0, 120.0));
    e.push(
        MSG_SET_PARAM,
        0,
        SET_BASE,
        P::SRC_HARM_MIX as u16,
        P::to_norm(P::SRC_HARM_MIX, 1.0),
        0,
    );
    e.push(MSG_SET_METRO, 0, 0, METRO_BAR, 4.0, 0);
    e.push(MSG_SET_METRO, 0, 0, METRO_LEVEL, 0.8, 0);
    // Metronome itself is off: the count-in still clicks, and the note is the sustained sound.
    e.push(MSG_COUNT_IN, 0, 0, 0, 120.0, 0);
    let l = process_all(&mut e, 120_000, 128);
    let clicks = click_samples(&l);
    assert!(clicks.len() >= 4, "count-in clicks: {clicks:?}");
    assert!(clicks[0] <= 2, "{}", clicks[0]);
    assert!((24_000..24_003).contains(&clicks[1]), "{}", clicks[1]);
    assert!((48_000..48_003).contains(&clicks[2]), "{}", clicks[2]);
    assert!((72_000..72_003).contains(&clicks[3]), "{}", clicks[3]);
    let quiet = l[8_000..20_000]
        .iter()
        .map(|v| v.abs())
        .fold(0.0f32, f32::max);
    let body = l[104_000..116_000]
        .iter()
        .map(|v| v.abs())
        .fold(0.0f32, f32::max);
    assert!(quiet < 0.01, "count-in gap should be quiet, got {quiet}");
    assert!(
        body > 0.02,
        "the note should be playing after the bar, got {body}"
    );
    assert_eq!(e.meter()[M_PLAYING], 1.0);
    assert_eq!(e.meter()[M_COUNT_SEQ], 1.0);
    assert_eq!(e.meter()[M_COUNTIN], 0.0);
}

#[test]
fn kit_click_uses_a_voice_and_a_tick_does_not() {
    let mut tick = Engine::new(SR, 128);
    tick.load_bytes(0, LOAD_TRACK, &track(4.0, 120.0, true));
    enable(&mut tick, 4.0);
    tick.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    tick.process(128);
    assert_eq!(tick.meter()[M_ACTIVE_VOICES], 0.0);

    let mut kit = Engine::new(SR, 128);
    kit.load_bytes(0, LOAD_TRACK, &track(4.0, 120.0, true));
    kit.push(MSG_SET_LAYER, 0, 0, LAYER_KIND, KIND_BEAT as f32, 0);
    enable(&mut kit, 4.0);
    kit.push(MSG_SET_METRO, 0, 0, METRO_VOICE, 1.0, 0);
    kit.push(MSG_SET_METRO, 0, 0, METRO_LAYER, 0.0, 0);
    kit.push(MSG_SET_METRO, 0, 0, METRO_SLOT, 0.0, 0);
    kit.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    kit.process(128);
    assert!(
        kit.meter()[M_ACTIVE_VOICES] >= 1.0,
        "a kit click starts a voice"
    );
}
