//! Checks the Basic Pitch port (crates/analysis/src/poly) against the Python package's own notes
//! and against labelled recordings (docs/poly.md, P3 checks 1 and 2; docs/P3.md).
//!
//!   cargo run --release -p analysis --bin poly_eval -- model <dir> <name>
//!     The network alone, on the 22050 Hz audio tools/basic-pitch/reference.py dumped with
//!     `--dump <name>`: largest difference per output, then the decoding run on the reference's
//!     own outputs and on the port's, each against the reference's notes.
//!   cargo run --release -p analysis --bin poly_eval -- run <dir> [--profile guitar|piano|generic]
//!     Every `<name>.wav` in <dir> through `poly_pass` (no range or channel limit unless a profile is
//!     given), its notes written to `<name>.rs.tsv`, then compared with `<name>.ref.tsv` (the
//!     reference, tools/basic-pitch/reference.py) and scored, with the reference and P0's
//!     `<name>.bp.tsv` (if present), against `<name>.notes.tsv` (the labels) as docs/poly.md
//!     scores Basic Pitch: same semitone, onset within 50 ms either way, one to one.
//!
//! Environment: POLY_FILES keeps only names containing one of its comma-separated parts;
//! POLY_THREADS (default: all cores) runs files in parallel (timings are only clean at 1).

//! The whole tool is native only (hound is not built for wasm).

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::main();
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use analysis::poly::{self, model, Options};
    use std::time::Instant;

    fn read_f32(path: &str) -> Vec<f32> {
        let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }

    fn read_wav(path: &str) -> (Vec<f32>, f32) {
        let mut r = hound::WavReader::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let spec = r.spec();
        let ch = spec.channels as usize;
        let raw: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => r.samples::<f32>().map(|s| s.unwrap()).collect(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                r.samples::<i32>().map(|s| s.unwrap() as f32 * scale).collect()
            }
        };
        let mono = raw.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect();
        (mono, spec.sample_rate as f32)
    }

    /// Rows of a TSV of numbers: onset s, offset s, MIDI pitch, ...
    fn read_tsv(path: &str) -> Option<Vec<Vec<f64>>> {
        let s = std::fs::read_to_string(path).ok()?;
        Some(s.lines().filter(|l| !l.trim().is_empty()).map(|l| l.split('\t').map(|x| x.trim().parse().unwrap_or(f64::NAN)).collect()).collect())
    }

    #[derive(Clone, Copy)]
    struct N {
        /// Pitch in cents (MIDI x 100; fractional where the labels or the curves have it).
        cents: f64,
        on: f64,
        off: f64,
        midi: i32,
    }

    fn notes_of(rows: &[Vec<f64>], len: f64) -> Vec<N> {
        let mut v: Vec<N> = rows.iter().filter(|r| r[0] < len).map(|r| N { on: r[0], off: r[1].min(len), midi: r[2].round() as i32, cents: 100.0 * r[2] }).collect();
        v.sort_by(|a, b| a.on.total_cmp(&b.on));
        v
    }

    /// One-to-one matching as docs/poly.md scores: each `truth` note, in onset order, takes the unused
    /// `est` note of its semitone whose onset is nearest within `win` either way. Per truth note: the
    /// index matched.
    fn matching(truth: &[N], est: &[N], win: f64) -> Vec<Option<usize>> {
        let mut used = vec![false; est.len()];
        truth
            .iter()
            .map(|t| {
                let mut best: Option<usize> = None;
                for (i, e) in est.iter().enumerate() {
                    let d = e.on - t.on;
                    if used[i] || e.midi != t.midi || d.abs() > win {
                        continue;
                    }
                    if best.is_none_or(|b| d.abs() < (est[b].on - t.on).abs()) {
                        best = Some(i);
                    }
                }
                if let Some(i) = best {
                    used[i] = true;
                }
                best
            })
            .collect()
    }

    /// Is truth note i re-struck: the same pitch sounding until just before it (docs/poly.md P0).
    fn restruck(truth: &[N], i: usize) -> bool {
        let t = truth[i];
        truth.iter().any(|u| u.midi == t.midi && u.on < t.on - 0.02 && u.off > t.on - 0.1)
    }

    #[derive(Default, Clone, Copy)]
    struct Score {
        n_ref: usize,
        n_est: usize,
        tp: usize,
        re_n: usize,
        re_tp: usize,
    }

    impl Score {
        fn add(&mut self, o: Score) {
            self.n_ref += o.n_ref;
            self.n_est += o.n_est;
            self.tp += o.tp;
            self.re_n += o.re_n;
            self.re_tp += o.re_tp;
        }
        fn p(&self) -> f64 {
            self.tp as f64 / self.n_est.max(1) as f64
        }
        fn r(&self) -> f64 {
            self.tp as f64 / self.n_ref.max(1) as f64
        }
        fn f1(&self) -> f64 {
            2.0 * self.p() * self.r() / (self.p() + self.r()).max(1e-12)
        }
        fn restruck(&self) -> f64 {
            self.re_tp as f64 / self.re_n.max(1) as f64
        }
    }

    fn score(truth: &[N], est: &[N]) -> Score {
        let m = matching(truth, est, 0.05);
        let mut s = Score { n_ref: truth.len(), n_est: est.len(), tp: m.iter().filter(|x| x.is_some()).count(), ..Score::default() };
        for (i, hit) in m.iter().enumerate() {
            if restruck(truth, i) {
                s.re_n += 1;
                s.re_tp += hit.is_some() as usize;
            }
        }
        s
    }

    /// The port's notes against the reference's: (reference notes, of them matched by a port note of
    /// the same pitch with its onset within 12 ms, port notes).
    fn identical(reference: &[N], port: &[N]) -> (usize, usize, usize) {
        let m = matching(reference, port, 0.012);
        (reference.len(), m.iter().filter(|x| x.is_some()).count(), port.len())
    }

    fn model_check(dir: &str, name: &str) {
        let audio = read_f32(&format!("{dir}/{name}.ref22k.f32"));
        let t0 = Instant::now();
        let mut s = poly::PolyStream::new(22050.0, Options::unlimited());
        s.feed(&audio);
        let res_port = {
            let acts = s.activations_until_end();
            let refs = |k: &str| read_f32(&format!("{dir}/{name}.ref.{k}.f32"));
            let (rn, ro, rc) = (refs("note"), refs("onset"), refs("contour"));
            let diff = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
            println!(
                "{name}: {} frames (reference {}); largest difference: notes {:.2e}, onsets {:.2e}, contours {:.2e} (after rounding to bytes); {:.2} s",
                acts.frames,
                rn.len() / model::N_NOTES,
                diff(&acts.note, &rn),
                diff(&acts.onset, &ro),
                diff(&acts.contour.iter().map(|&v| v as f32 / 255.0).collect::<Vec<_>>(), &rc),
                t0.elapsed().as_secs_f64()
            );
            let len = audio.len() as f64 / 22050.0;
            let reference = notes_of(&read_tsv(&format!("{dir}/{name}.ref.tsv")).unwrap(), f64::INFINITY);
            let to_n = |v: &[poly::decode::RawNote]| -> Vec<N> {
                let mut o: Vec<N> = v.iter().map(|n| N { on: poly::decode::frame_time(n.start), off: poly::decode::frame_time(n.end), midi: n.midi, cents: 100.0 * n.midi as f64 }).collect();
                o.sort_by(|a, b| a.on.total_cmp(&b.on));
                o
            };
            let mut from_ref = poly::Activations { frames: rn.len() / model::N_NOTES, note: rn, onset: ro, ..Default::default() };
            from_ref.contour.clear();
            let dec_ref = to_n(&poly::decode::notes(&from_ref, &poly::decode::Settings::default()));
            let (a, b, c) = identical(&reference, &dec_ref);
            println!("  decoding the reference's outputs: {b} of {a} reference notes, {c} notes ({:.1} s)", len);
            let exact = dec_ref.iter().zip(&reference).all(|(x, y)| x.midi == y.midi && (x.on - y.on).abs() < 1e-5 && (x.off - y.off).abs() < 1e-5);
            println!("  ... exactly the same notes, times to 10 us: {exact}");
            to_n(&poly::decode::notes(acts, &poly::decode::Settings::default()))
        };
        let reference = notes_of(&read_tsv(&format!("{dir}/{name}.ref.tsv")).unwrap(), f64::INFINITY);
        let (a, b, c) = identical(&reference, &res_port);
        println!("  decoding the port's outputs: {b} of {a} reference notes identical, {c} port notes");
    }

    fn run(dir: &str, profile: Option<&str>) {
        let filt: Option<Vec<String>> = std::env::var("POLY_FILES").ok().map(|f| f.split(',').map(String::from).collect());
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|f| f.strip_suffix(".wav").map(String::from))
            .filter(|n| filt.as_ref().is_none_or(|f| f.iter().any(|p| n.contains(p.as_str()))))
            .collect();
        names.sort();
        let opts = match profile {
            Some("guitar") => Options::profile(poly::GUITAR),
            Some("piano") => Options::profile(poly::PIANO),
            Some("generic") => Options::profile(poly::GENERIC),
            Some(p) => panic!("profile {p}"),
            None => Options::unlimited(),
        };
        let threads: usize = std::env::var("POLY_THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(name) = names.get(i) else { break };
                        let (audio, sr) = read_wav(&format!("{dir}/{name}.wav"));
                        let t0 = Instant::now();
                        let res = poly::poly_pass(&audio, sr, opts);
                        let secs = t0.elapsed().as_secs_f64();
                        let len = audio.len() as f64 / sr as f64;
                        let mut tsv = String::new();
                        for n in &res.notes {
                            tsv += &format!("{:.6}\t{:.6}\t{}\t{:.6}\n", n.t, n.t + n.dur, n.midi, n.amp);
                        }
                        if profile.is_none() {
                            std::fs::write(format!("{dir}/{name}.rs.tsv"), &tsv).unwrap();
                        }
                        // A note's pitch: its semitone plus the median of its curve's pitch offsets.
                        let pitch = |n: &poly::PolyNote| {
                            let mut v: Vec<f32> = (n.first..n.first + n.n).map(|i| res.records[i * poly::REC_LEN]).collect();
                            v.sort_by(f32::total_cmp);
                            n.midi as f64 + v[v.len() / 2] as f64 / 100.0
                        };
                        // The semitone is the note's own; the curve only refines `cents`.
                        let mut port: Vec<N> =
                            res.notes.iter().filter(|n| n.t < len).map(|n| N { on: n.t, off: (n.t + n.dur).min(len), midi: n.midi, cents: 100.0 * pitch(n) }).collect();
                        port.sort_by(|a, b| a.on.total_cmp(&b.on));
                        let all = |suffix: &str| read_tsv(&format!("{dir}/{name}.{suffix}")).map(|r| notes_of(&r, len));
                        let reference = all("ref.tsv");
                        let truth = all("notes.tsv").unwrap_or_default();
                        let ident = reference.as_ref().map(|r| identical(r, &port));
                        let s_port = score(&truth, &port);
                        let s_ref = reference.as_ref().map(|r| score(&truth, r));
                        let s_bp = all("bp.tsv").map(|b| score(&truth, &b));
                        let cents: Vec<f64> = matching(&truth, &port, 0.05).iter().zip(&truth).filter_map(|(m, t)| m.map(|i| (port[i].cents - t.cents).abs())).collect();
                        results.lock().unwrap().push((name.clone(), len, secs, ident, s_port, s_ref, s_bp, cents));
                    }
                });
            }
        });
        let mut rows = results.into_inner().unwrap();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let (mut len, mut secs) = (0.0, 0.0);
        let (mut ia, mut ib, mut ic) = (0, 0, 0);
        let (mut sp, mut sr, mut sb) = (Score::default(), Score::default(), Score::default());
        let mut worst: Vec<(f64, String)> = Vec::new();
        let mut cents: Vec<f64> = rows.iter().flat_map(|r| r.7.iter().copied()).collect();
        cents.sort_by(f64::total_cmp);
        for (name, l, s, ident, p, r, b, _) in &rows {
            len += l;
            secs += s;
            if let Some((a, bb, c)) = ident {
                ia += a;
                ib += bb;
                ic += c;
                worst.push((*bb as f64 / (*a).max(1) as f64, name.clone()));
            }
            sp.add(*p);
            if let Some(r) = r {
                sr.add(*r);
            }
            if let Some(b) = b {
                sb.add(*b);
            }
        }
        worst.sort_by(|a, b| a.0.total_cmp(&b.0));
        println!("{} files, {:.1} min; port {:.1} s in all ({:.0}x real time per thread, {threads} threads)", rows.len(), len / 60.0, secs, len / secs);
        println!("Port against reference: {ib} of {ia} reference notes identical (same pitch, onset within 12 ms): {:.2}%; port notes {ic}", 100.0 * ib as f64 / ia.max(1) as f64);
        for (f, n) in worst.iter().take(3) {
            println!("  lowest: {n} {:.1}%", 100.0 * f);
        }
        if !cents.is_empty() {
            let q = |p: f64| cents[((cents.len() - 1) as f64 * p) as usize];
            println!("Pitch of found notes against the labels (semitone plus the curve's median offset): median {:.1} cents, p90 {:.1}", q(0.5), q(0.9));
        }
        let line = |what: &str, s: &Score| println!("| {what} | {:.3} | {:.3} | {:.3} | {:.2} ({} of {}) | {} |", s.p(), s.r(), s.f1(), s.restruck(), s.re_tp, s.re_n, s.n_est);
        println!("\n| Notes | Precision | Recall | F1 | Re-struck recall | Notes found |\n| --- | --- | --- | --- | --- | --- |");
        line(&format!("Port{}", profile.map_or(String::new(), |p| format!(", {p} profile"))), &sp);
        if sr.n_ref > 0 {
            line("Reference (Python, ONNX)", &sr);
        }
        if sb.n_ref > 0 {
            line("P0's run (Python, Core ML)", &sb);
        }
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        match args.get(1).map(String::as_str) {
            Some("model") => model_check(&args[2], &args[3]),
            Some("run") => {
                let profile = args.iter().position(|a| a == "--profile").map(|i| args[i + 1].as_str());
                run(&args[2], profile)
            }
            _ => eprintln!("usage: poly_eval model <dir> <name> | run <dir> [--profile guitar|piano|generic]"),
        }
    }
}
