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
//! Scene "demo": the 8-layer demo track from web/public/m1/demo (written by
//! `pnpm gen:demo`), rendered at the automatic budget's ceiling.

use klont::Engine;
use klont::protocol::*;
use params as P;
use std::time::Instant;

const SR: f32 = 48_000.0;
const BLOCK: usize = 128;

fn set(e: &mut Engine, layer: u8, p: usize, real: f32) {
    e.push(MSG_SET_PARAM, layer, SET_BASE, p as u16, P::to_norm(p, real), 0);
}

fn worst_case(budget: u32) -> Box<Engine> {
    let mut e = Engine::new(SR, BLOCK);
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let seconds: f64 = arg("--seconds").and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let json = args.iter().any(|a| a == "--json");
    let demo = arg("--demo").unwrap_or_else(|| "web/public/m1/demo".into());

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

    let mut demo_json = String::from("null");
    let read = |f: &str| std::fs::read(format!("{demo}/{f}"));
    match read("track.bin") {
        Ok(track) => {
            let mut e = Engine::new(SR, 4096);
            for l in 0..MAX_LAYERS as u32 {
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
            "{{\"worst_case\":[{}],\"demo\":{demo_json},\"live_block_us\":{{\"disarmed\":{:.2},\"armed\":{:.2}}}}}",
            rows.join(","),
            live_us[0],
            live_us[1]
        );
    }
}
