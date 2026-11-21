//! Native benchmark for the M1 gate: partial budget and offline render speed.
//!
//! cargo run -p klont --release --bin bench [-- --seconds 10] [--demo <dir>] [--json]
//!
//! Scene "worst case": 8 layers x 8 held voices, every sound bright enough
//! that all 64 partials are audible, a vibrato LFO on every voice (so the
//! oscillators retune every tick), delay and reverb on every layer. It is
//! rendered once per budget; each line gives the real-time factor (seconds
//! of audio per second of CPU) and the cost per active partial per sample.
//!
//! Scene "routes" (stage S2): the worst case with 48 voice routes on every
//! voice patch, all evaluated every tick (envelopes, LFOs and note fields into
//! the partial bank's spectrum, the filter, the amp, the click and pitch; some
//! bent, some with points, some smoothed; `classic::bench_routes`), at budget
//! 4096, beside the classic worst case run alternately in the same session.
//! "routes, heavy" also moves stretch, jitter and damp in every voice.
//!
//! Scene "demo": the 8-layer demo track from web/public/m1/demo (written by
//! `pnpm gen:demo`), rendered at the automatic budget's ceiling.
//!
//! Scene "costs" (stage S3, `--costs`): what each module type, an envelope, an LFO, a voice
//! route and a partial bank that rebuilds every tick cost per sample, measured as the difference
//! between two 8 x 8 scenes that differ only in that part (best of 3). These are the `cost`
//! figures of crates/params/modules.toml, which the engine and the app use to cap a layer's
//! voices.
//!
//! Scene "worst allowed" (stage S3): 8 layers, each with the heaviest patch the caps allow (every
//! voice module at its cap, all on, 48 routes moving the partial bank, a full channel), playing
//! as many voices as its cost leaves room for, beside the classic worst case.
//!
//! Scene "listening" (stage S4, also `--listen` alone): the live tier with a sung input and
//! three melody layers holding a note, each with a layer route from its sung volume to its
//! filter cutoff, while none, one or all three listen. The live tier runs once whatever the
//! number, so the three-listener figure should exceed the none figure by about what the
//! listeners' own routes cost, not by three tiers.

use klont::build::{self, FxSpec, VoiceSpec, fullest_channel, heaviest_voice, voice};
use klont::Engine;
use klont::protocol::*;
use params as P;
use std::time::Instant;

const SR: f32 = 48_000.0;
const BLOCK: usize = 128;

fn set(e: &mut Engine, layer: u8, p: usize, real: f32) {
    klont::classic::set_param(e, layer, KIND_MELODY, p, P::to_norm(p, real));
}

fn worst_case(budget: u32) -> Box<Engine> {
    worst_case_with(budget, None)
}

/// The worst case; with `routes`, every voice patch carries the 48 bench routes (heavy or not)
/// instead of the classic ones.
fn worst_case_with(budget: u32, routes: Option<bool>) -> Box<Engine> {
    let mut e = Engine::new(SR, BLOCK);
    if let Some(heavy) = routes {
        let blob = klont::classic::patch_blob_ext(KIND_MELODY, 0, &klont::classic::bench_routes(heavy), &[], 0, false);
        for l in 0..MAX_LAYERS as u32 {
            assert_eq!(e.load_bytes(l, LOAD_PATCH, &blob), LOAD_OK, "bench routes patch");
        }
    }
    for l in 0..MAX_LAYERS as u8 {
        set(&mut e, l, P::SRC_TILT, -1.5);
        set(&mut e, l, P::SRC_NOISE_MIX, 0.1);
        set(&mut e, l, P::ENV_SUSTAIN, 1.0);
        set(&mut e, l, P::LFO1_RATE, 5.0 + l as f32 * 0.3);
        set(&mut e, l, P::LFO1_TO_PITCH, 20.0);
        set(&mut e, l, P::DELAY_MIX, 0.2);
        set(&mut e, l, P::VERB_MIX, 0.2);
        e.push(MSG_SET_LAYER, l, 0, LAYER_LEVEL, -24.0, 0);
    }
    e.push(MSG_SET_BUDGET, 0, 0, 0, budget as f32, 0);
    e.render(BLOCK);
    for l in 0..MAX_LAYERS as u8 {
        for v in 0..VOICES_PER_LAYER {
            // 55 Hz to 223 Hz: every voice keeps all 64 harmonics under
            // 0.45 fs, so the scene asks for the full 4096 partials.
            let cents = 3300.0 + 25.0 * (l as f32 * 2.0 + v as f32 * 5.0);
            e.push(MSG_NOTE_ON, l, 0, 0, cents, 0);
        }
    }
    e
}

struct Run {
    rt: f64,
    ns_per_partial: f64,
    partials: f64,
}

fn time_render(e: &mut Engine, seconds: f64) -> Run {
    let blocks = (seconds * SR as f64 / BLOCK as f64) as usize;
    // Warm up past the attacks so the budget has settled.
    for _ in 0..200 {
        e.render(BLOCK);
    }
    let mut partial_samples = 0.0f64;
    let t = Instant::now();
    for _ in 0..blocks {
        e.render(BLOCK);
        partial_samples += e.meter()[M_ACTIVE_PARTIALS] as f64 * BLOCK as f64;
    }
    let el = t.elapsed().as_secs_f64();
    let audio = (blocks * BLOCK) as f64 / SR as f64;
    Run { rt: audio / el, ns_per_partial: el * 1e9 / partial_samples.max(1.0), partials: partial_samples / (blocks * BLOCK) as f64 }
}

/// A second of a sine as sample 0 (for samplers and grains).
fn load_sample(e: &mut Engine) {
    let n = SR as usize;
    let mut b: Vec<u8> = Vec::new();
    for w in [SAMPLE_MAGIC, SR.to_bits(), 4800.0f32.to_bits(), n as u32] {
        b.extend_from_slice(&w.to_le_bytes());
    }
    for i in 0..n {
        let x = (std::f32::consts::TAU * 261.63 * i as f32 / SR).sin() * 0.5;
        b.extend_from_slice(&x.to_le_bytes());
    }
    assert_eq!(e.load_bytes(0, LOAD_SAMPLE, &b), LOAD_OK);
}

/// 8 layers x 8 held voices of one patch (budget 4096, all partials), ready to time.
fn scene(blob: &[u8]) -> Box<Engine> {
    let mut e = Engine::new(SR, BLOCK);
    load_sample(&mut e);
    for l in 0..MAX_LAYERS as u8 {
        assert_eq!(e.load_bytes(l as u32, LOAD_PATCH, blob), LOAD_OK);
        e.push(MSG_SET_LAYER, l, 0, LAYER_LEVEL, -24.0, 0);
    }
    e.push(MSG_SET_BUDGET, 0, 0, 0, 4096.0, 0);
    e.render(BLOCK);
    for l in 0..MAX_LAYERS as u8 {
        for v in 0..VOICES_PER_LAYER {
            let cents = 3300.0 + 25.0 * (l as f32 * 2.0 + v as f32 * 5.0);
            e.push(MSG_NOTE_ON, l, 0, 0, cents, 0);
        }
    }
    e
}

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}
unsafe extern "C" {
    fn clock_gettime(clock: u32, t: *mut Timespec) -> i32;
}
#[cfg(target_os = "macos")]
const THREAD_CPU: u32 = 16;
#[cfg(not(target_os = "macos"))]
const THREAD_CPU: u32 = 3;

/// This thread's CPU time in seconds: unlike wall time it leaves out the time other work on a
/// loaded machine takes the core away.
fn cpu_s() -> f64 {
    let mut t = Timespec { sec: 0, nsec: 0 };
    // SAFETY: clock_gettime writes one Timespec; the clock id is the platform's thread CPU clock.
    unsafe { clock_gettime(THREAD_CPU, &mut t) };
    t.sec as f64 + t.nsec as f64 * 1e-9
}

/// CPU nanoseconds per sample of the whole 8 x 8 scene for this patch, best of 5.
fn scene_ns(blob: &[u8], seconds: f64) -> f64 {
    let blocks = (seconds * SR as f64 / BLOCK as f64) as usize;
    (0..5)
        .map(|_| {
            let mut e = scene(blob);
            for _ in 0..200 {
                e.render(BLOCK);
            }
            let t = cpu_s();
            for _ in 0..blocks {
                e.render(BLOCK);
            }
            (cpu_s() - t) * 1e9 / (blocks * BLOCK) as f64
        })
        .fold(f64::INFINITY, f64::min)
}

fn costs(seconds: f64) {
    let blob = |v: &VoiceSpec, ch: &[FxSpec], cs: &[(usize, usize, f32)]| build::patch(std::slice::from_ref(v), ch, cs);
    let per_voice = |a: f64, b: f64| (a - b) / (MAX_LAYERS * VOICES_PER_LAYER) as f64;
    let per_layer = |a: f64, b: f64| (a - b) / MAX_LAYERS as f64;
    let osc = params::T_OSC;
    let t = |v: &VoiceSpec| scene_ns(&blob(v, &[], &[]), seconds);
    let base = t(&voice(&[], &[], 0, 0, 0, 0));
    let with_osc = t(&voice(&[osc], &[], 0, 0, 0, P::osc::FINE));
    println!("costs, ns per sample (voice parts per sounding voice, layer parts per layer); 8 x 8 scenes, best of 3:");
    let mut rows: Vec<(&str, f64)> = Vec::new();
    // As many copies as the caps allow, so the part measured is most of the difference.
    for (name, code, n) in [("partials", params::T_PARTIALS, 1), ("osc", osc, 4), ("noise", params::T_NOISE, 2), ("sampler", params::T_SAMPLER, 1), ("click", params::T_CLICK, 1)] {
        let c = per_voice(t(&voice(&vec![code; n], &[], 0, 0, 0, 0)), base) / n as f64;
        rows.push((name, c));
    }
    for (name, code, n) in [("filter", params::T_FILTER, 2), ("amp", params::T_AMP, 3), ("fold", params::T_FOLD, 1), ("clang", params::T_CLANG, 1), ("comb", params::T_COMB, 1)] {
        rows.push((name, per_voice(t(&voice(&[osc], &vec![code; n], 0, 0, 0, P::osc::FINE)), with_osc) / n as f64));
    }
    rows.push(("env", per_voice(t(&voice(&[osc], &[], 3, 0, 0, P::osc::FINE)), with_osc) / 3.0));
    rows.push(("lfo", per_voice(t(&voice(&[osc], &[], 0, 4, 0, P::osc::FINE)), with_osc) / 3.0));
    let routes = per_voice(t(&voice(&[osc], &[], 0, 0, 47, P::osc::FINE)), with_osc) / 47.0;
    let part = |p: usize| t(&voice(&[params::T_PARTIALS], &[], 0, 0, 1, p));
    let rebuild = per_voice(part(P::partials::TILT), part(P::partials::LEVEL));
    // Layer parts.
    let ov = voice(&[osc], &[], 0, 0, 0, P::osc::FINE);
    let lt = |ch: &[FxSpec], cs: &[(usize, usize, f32)]| scene_ns(&blob(&ov, ch, cs), seconds);
    let none = lt(&[], &[]);
    let rev = lt(&[(params::T_REVERB, false)], &[(0, P::reverb::MIX, 0.3)]);
    rows.push(("crush", per_layer(lt(&[(params::T_CRUSH, false)], &[(0, P::crush::BITS, 8.0)]), none)));
    rows.push(("tape", per_layer(lt(&[(params::T_TAPE, false)], &[(0, P::tape::AMOUNT, 0.5)]), none)));
    rows.push(("delay", per_layer(lt(&[(params::T_DELAY, false)], &[(0, P::delay::MIX, 0.3), (0, P::delay::FEEDBACK, 0.5)]), none)));
    rows.push(("reverb", per_layer(rev, none)));
    rows.push(("cfilter", per_layer(lt(&[(params::T_REVERB, false), (params::T_CFILTER, false)], &[(0, P::reverb::MIX, 0.3), (1, P::cfilter::CUTOFF, 2000.0)]), rev)));
    let with_noise = VoiceSpec { noise: Some((0, 0.5)), ..ov.clone() };
    rows.push(("shaped_noise", per_layer(scene_ns(&blob(&with_noise, &[], &[]), seconds), with_osc)));
    let with_grains = VoiceSpec { grains: Some((0, 0, 0.5)), ..ov.clone() };
    rows.push(("grains", per_layer(scene_ns(&blob(&with_grains, &[], &[]), seconds), with_osc)));
    for (name, c) in &rows {
        println!("  {name:>13} {c:7.2}");
    }
    println!("  {:>13} {routes:7.2}\n  {:>13} {rebuild:7.2}", "route", "rebuild");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let seconds: f64 = arg("--seconds").and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let json = args.iter().any(|a| a == "--json");
    let demo = arg("--demo").unwrap_or_else(|| "web/public/m1/demo".into());

    if args.iter().any(|a| a == "--costs") {
        costs(seconds.min(2.0));
        return;
    }
    if args.iter().any(|a| a == "--listen") {
        listen_scene();
        return;
    }
    println!("Earworm M1 native benchmark, {SR} Hz, {BLOCK}-frame blocks, {seconds} s of audio per run");
    println!("worst case: 8 layers x 8 voices x 64 partials, vibrato, delay and reverb on every layer");
    println!("{:>8} {:>10} {:>10} {:>14} {:>12}", "budget", "partials", "x realtime", "ns/partial/s", "core load");
    let mut rows = Vec::new();
    for budget in [4096u32, 3072, 2048, 1536, 1024, 768, 512] {
        let mut e = worst_case(budget);
        let r = time_render(&mut e, seconds);
        println!("{:>8} {:>10.0} {:>10.2} {:>14.3} {:>11.1}%", budget, r.partials, r.rt, r.ns_per_partial, 100.0 / r.rt);
        rows.push(format!(
            "{{\"budget\":{budget},\"partials\":{:.0},\"realtime\":{:.3},\"ns_per_partial_sample\":{:.4}}}",
            r.partials, r.rt, r.ns_per_partial
        ));
    }

    // Stage S2: 48 voice routes per voice patch, against the classic worst case, alternately.
    let (mut best_routes, mut best_heavy, mut best_classic) = (0.0f64, 0.0f64, 0.0f64);
    for _ in 0..3 {
        let mut e = worst_case_with(4096, Some(false));
        best_routes = best_routes.max(time_render(&mut e, seconds).rt);
        let mut e = worst_case_with(4096, Some(true));
        best_heavy = best_heavy.max(time_render(&mut e, seconds).rt);
        let mut e = worst_case(4096);
        best_classic = best_classic.max(time_render(&mut e, seconds).rt);
    }
    println!(
        "routes: 8 layers x 8 voices x 48 voice routes, budget 4096: {best_routes:.2}x realtime; heavy (also stretch, jitter, damp): {best_heavy:.2}x; classic worst case beside them: {best_classic:.2}x; best of 3"
    );

    // Stage S3: each layer's heaviest allowed patch at the voice count its cost allows.
    let (ch, cs) = fullest_channel();
    let worst = build::patch(&[heaviest_voice()], &ch, &cs);
    let voices = {
        let mut p = klont::patch::Patch::new_boxed();
        p.parse(&worst).expect("the heaviest patch is valid");
        p
    };
    let (mut best_worst, mut best_classic3) = (0.0f64, 0.0f64);
    let mut allowed = 0.0;
    for _ in 0..3 {
        let mut e = scene(&worst);
        let r = time_render(&mut e, seconds);
        allowed = e.meter()[M_ACTIVE_VOICES];
        best_worst = best_worst.max(r.rt);
        let mut e = worst_case(4096);
        best_classic3 = best_classic3.max(time_render(&mut e, seconds).rt);
    }
    println!(
        "worst allowed: 8 layers x {} voices of the heaviest patch ({:.0} ns a voice, {:.0} ns a layer): {best_worst:.2}x realtime; classic worst case beside it: {best_classic3:.2}x; best of 3",
        allowed as usize / MAX_LAYERS,
        voices.max_voice_cost(),
        voices.layer_cost()
    );

    // Live tier: cost of `process` per block with a voiced input, disarmed
    // (tier only) and with a melody layer armed (tier plus a live voice).
    let hum: Vec<f32> = (0..(SR as usize)).map(|i| {
        let t = i as f32 / SR;
        (1..=12).map(|k| (std::f32::consts::TAU * 220.0 * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.15
    }).collect();
    let mut live_us = [0.0f64; 2];
    for (n, armed) in [false, true].into_iter().enumerate() {
        let mut e = Engine::new(SR, BLOCK);
        if armed {
            e.push(MSG_ARM, 0, 0, 0, 1.0, 0);
        }
        let blocks = 20_000usize;
        let run = |e: &mut Engine, count: usize| {
            for b in 0..count {
                let o = (b * BLOCK) % (hum.len() - BLOCK);
                e.input_mut()[..BLOCK].copy_from_slice(&hum[o..o + BLOCK]);
                e.process(BLOCK);
            }
        };
        run(&mut e, 2000);
        let t = Instant::now();
        run(&mut e, blocks);
        live_us[n] = t.elapsed().as_secs_f64() * 1e6 / blocks as f64;
    }
    let budget_us = BLOCK as f64 / SR as f64 * 1e6;
    println!(
        "live tier per {BLOCK}-sample block: {:.1} us disarmed, {:.1} us with a melody layer armed (block is {:.0} us)",
        live_us[0], live_us[1], budget_us
    );

    let listen_us = listen_scene();

    let mut demo_json = String::from("null");
    let read = |f: &str| std::fs::read(format!("{demo}/{f}"));
    match read("track.bin") {
        Ok(track) => {
            let mut e = Engine::new(SR, 4096);
            for l in 0..MAX_LAYERS as u32 {
                if let Ok(b) = read(&format!("patch{l}.bin")) {
                    assert_eq!(e.load_bytes(l, LOAD_PATCH, &b), LOAD_OK, "patch{l}.bin");
                }
                if let Ok(b) = read(&format!("layer{l}.bin")) {
                    assert_eq!(e.load_bytes(l, LOAD_LAYER, &b), LOAD_OK, "layer{l}.bin");
                }
            }
            assert_eq!(e.load_bytes(0, LOAD_TRACK, &track), LOAD_OK, "track.bin");
            e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
            let total = (seconds * SR as f64) as usize;
            let t = Instant::now();
            let (mut done, mut peak, mut max_parts) = (0usize, 0.0f32, 0.0f32);
            while done < total {
                done += e.render(4096.min(total - done));
                peak = peak.max(e.meter()[M_PEAK_L]).max(e.meter()[M_PEAK_R]);
                max_parts = max_parts.max(e.meter()[M_ACTIVE_PARTIALS]);
            }
            let rt = done as f64 / SR as f64 / t.elapsed().as_secs_f64();
            println!("demo track ({demo}): {rt:.2}x realtime in 4096-frame chunks, peak {peak:.2}, up to {max_parts} partials");
            demo_json = format!("{{\"realtime\":{rt:.3},\"peak\":{peak:.3},\"max_partials\":{max_parts}}}");
        }
        Err(_) => println!("demo track: {demo}/track.bin not found (run `pnpm gen:demo`), skipped"),
    }
    if json {
        println!(
            "{{\"worst_case\":[{}],\"routes48\":{{\"realtime\":{best_routes:.3},\"heavy\":{best_heavy:.3},\"classic\":{best_classic:.3}}},\"worst_allowed\":{{\"realtime\":{best_worst:.3},\"voices\":{allowed},\"classic\":{best_classic3:.3}}},\"demo\":{demo_json},\"live_block_us\":{{\"disarmed\":{:.2},\"armed\":{:.2}}},\"listen_block_us\":[{:.2},{:.2},{:.2},{:.2}]}}",
            rows.join(","),
            live_us[0],
            live_us[1],
            listen_us[0],
            listen_us[1],
            listen_us[2],
            listen_us[3]
        );
    }
}

/// A sung "ah" at 220 Hz with a slow swell, one second, looped by the caller.
fn sung() -> Vec<f32> {
    (0..(SR as usize))
        .map(|i| {
            let t = i as f32 / SR;
            let swell = 0.6 + 0.4 * (std::f32::consts::TAU * 2.0 * t).sin();
            (1..=12).map(|k| (std::f32::consts::TAU * 220.0 * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.15 * swell
        })
        .collect()
}

/// Stage S4: microseconds per block of `process` with a sung input, three melody layers
/// holding a note with a route from their sung volume, and 0, 1 or 3 of them listening; and
/// the live tier alone (no layer sounding). Best of 3 runs of 20,000 blocks each.
fn listen_scene() -> [f64; 4] {
    use klont::classic::{self, Extra, ExtraTarget};
    let hum = sung();
    let make = |listeners: usize, sounding: bool| {
        let mut e = Engine::new(SR, BLOCK);
        let route = Extra::new(klont::patch::SRC_LAYER_VOLUME, 0, ExtraTarget::V("filter1", P::filter::CUTOFF), 0.6);
        for l in 0..3u8 {
            let blob = classic::patch_blob_ext(KIND_MELODY, l as u32, &[], &[route], 0, true);
            assert_eq!(e.load_bytes(l as u32, LOAD_PATCH, &blob), LOAD_OK);
            if sounding {
                e.push(MSG_NOTE_ON, l, 0, 0, 4800.0 + 400.0 * l as f32, 0);
            }
            if (l as usize) < listeners {
                e.push(MSG_LISTEN, l, 0, 0, 1.0, 0);
            }
        }
        e
    };
    let run = |e: &mut Engine, count: usize| {
        for b in 0..count {
            let o = (b * BLOCK) % (hum.len() - BLOCK);
            e.input_mut()[..BLOCK].copy_from_slice(&hum[o..o + BLOCK]);
            e.process(BLOCK);
        }
    };
    let cases = [(0usize, false), (0, true), (1, true), (3, true)];
    let mut best = [f64::INFINITY; 4];
    for _ in 0..3 {
        for (i, &(n, sounding)) in cases.iter().enumerate() {
            let mut e = make(n, sounding);
            run(&mut e, 2000);
            let t = Instant::now();
            run(&mut e, 20_000);
            best[i] = best[i].min(t.elapsed().as_secs_f64() * 1e6 / 20_000.0);
        }
    }
    let block_us = BLOCK as f64 / SR as f64 * 1e6;
    println!(
        "listening, us per {BLOCK}-sample block with a sung input (block is {block_us:.0} us), best of 3: live tier alone {:.1}; three layers holding a note, none listening {:.1}, one {:.1}, three {:.1}",
        best[0], best[1], best[2], best[3]
    );
    println!(
        "  three listeners cost {:.1} us a block more than none; the live tier alone is {:.1} us (three tiers would add {:.1})",
        best[3] - best[1],
        best[0],
        2.0 * best[0]
    );
    best
}
