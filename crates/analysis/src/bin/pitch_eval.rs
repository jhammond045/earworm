//! Compare the live YIN tracker with the pYIN second pass on recorded takes.
//! See docs/M0.md, "Second-pass pitch tracker".
//!
//! Usage: cargo run -p analysis --release --bin pitch_eval -- <dir or wav files...>
//!
//! For each WAV (first channel, any rate) it runs the live tier over the file
//! and takes pitch slot 0 of the stored frames, runs pyin, and compares them.
//! If `<name>.f0.csv` sits next to the WAV (lines `time_s,hz`, 0 hz for
//! unvoiced, any frame rate) it also scores live, pyin and the repaired curve
//! against it.
//!
//! The whole tool is native only (hound is not built for wasm).

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::run();
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use analysis::pyin::{pyin, repair};
    use dsp::capture::frames_from_signal;
    use dsp::frame::{DEFAULT_BANDS, PITCH};
    use dsp::hz_to_cents;
    use std::path::{Path, PathBuf};

    const AGREE_CENTS: f32 = 50.0;

    #[derive(Default, Clone, Copy)]
    struct Score {
        /// Label-voiced frames, and those where the curve is voiced and within 50 cents.
        ref_voiced: usize,
        pitch_ok: usize,
        /// Frames where voiced/unvoiced matches the label.
        voicing_ok: usize,
        frames: usize,
    }

    impl Score {
        fn add(&mut self, o: &Score) {
            self.ref_voiced += o.ref_voiced;
            self.pitch_ok += o.pitch_ok;
            self.voicing_ok += o.voicing_ok;
            self.frames += o.frames;
        }
    }

    #[derive(Default, Clone, Copy)]
    struct Row {
        frames: usize,
        live_voiced: usize,
        pyin_voiced: usize,
        both_voiced: usize,
        agree: usize,
        octave: usize,
        repaired: usize,
        labelled: bool,
        live: Score,
        second: Score,
        fixed: Score,
    }

    impl Row {
        fn add(&mut self, o: &Row) {
            self.frames += o.frames;
            self.live_voiced += o.live_voiced;
            self.pyin_voiced += o.pyin_voiced;
            self.both_voiced += o.both_voiced;
            self.agree += o.agree;
            self.octave += o.octave;
            self.repaired += o.repaired;
            if o.labelled {
                self.labelled = true;
                self.live.add(&o.live);
                self.second.add(&o.second);
                self.fixed.add(&o.fixed);
            }
        }
    }

    /// Read the first channel of a WAV as f32 plus its sample rate.
    fn read_wav(path: &Path) -> Result<(Vec<f32>, f32), String> {
        let mut r = hound::WavReader::open(path).map_err(|e| e.to_string())?;
        let spec = r.spec();
        let ch = spec.channels.max(1) as usize;
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => r.samples::<f32>().step_by(ch).collect::<Result<_, _>>().map_err(|e| e.to_string())?,
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                r.samples::<i32>().step_by(ch).map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>().map_err(|e| e.to_string())?
            }
        };
        Ok((samples, spec.sample_rate as f32))
    }

    /// Parse `time_s,hz` lines; lines that do not parse (headers, comments) are skipped.
    fn parse_labels(text: &str) -> Vec<(f32, f32)> {
        let mut v: Vec<(f32, f32)> = text
            .lines()
            .filter_map(|l| {
                let mut it = l.split(',');
                let t = it.next()?.trim().parse::<f32>().ok()?;
                let hz = it.next()?.trim().parse::<f32>().ok()?;
                (t.is_finite() && hz.is_finite()).then_some((t, hz))
            })
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    }

    /// Labels resampled to n frames at 10 ms, as cents (NaN unvoiced). Between two
    /// voiced points the pitch is interpolated; otherwise the nearer point decides.
    fn labels_to_frames(labels: &[(f32, f32)], n: usize) -> Vec<f32> {
        let mut out = vec![f32::NAN; n];
        if labels.is_empty() {
            return out;
        }
        let mut j = 0;
        for (i, o) in out.iter_mut().enumerate() {
            let t = i as f32 * 0.01;
            while j + 1 < labels.len() && labels[j + 1].0 <= t {
                j += 1;
            }
            let (a, b) = (labels[j], labels[(j + 1).min(labels.len() - 1)]);
            if t < labels[0].0 || t > labels[labels.len() - 1].0 {
                continue;
            }
            let hz = if a.1 > 0.0 && b.1 > 0.0 && b.0 > a.0 {
                a.1 + (b.1 - a.1) * ((t - a.0) / (b.0 - a.0)).clamp(0.0, 1.0)
            } else if (t - a.0).abs() <= (b.0 - t).abs() {
                a.1
            } else {
                b.1
            };
            if hz > 0.0 {
                *o = hz_to_cents(hz);
            }
        }
        out
    }

    fn score(curve: &[f32], truth: &[f32]) -> Score {
        let mut s = Score::default();
        for (&c, &t) in curve.iter().zip(truth) {
            s.frames += 1;
            if c.is_finite() == t.is_finite() {
                s.voicing_ok += 1;
            }
            if t.is_finite() {
                s.ref_voiced += 1;
                if c.is_finite() && (c - t).abs() <= AGREE_CENTS {
                    s.pitch_ok += 1;
                }
            }
        }
        s
    }

    /// Compare two curves (and optional labels) frame by frame.
    fn compare(live: &[f32], second: &[f32], truth: Option<&[f32]>) -> Row {
        let n = live.len().min(second.len());
        let (live, second) = (&live[..n], &second[..n]);
        let mut r = Row { frames: n, ..Default::default() };
        for i in 0..n {
            let (a, b) = (live[i], second[i]);
            r.live_voiced += a.is_finite() as usize;
            r.pyin_voiced += b.is_finite() as usize;
            if a.is_finite() && b.is_finite() {
                r.both_voiced += 1;
                let d = (a - b).abs();
                if d <= AGREE_CENTS {
                    r.agree += 1;
                }
                if (d - 1200.0).abs() <= 100.0 {
                    r.octave += 1;
                }
            }
        }
        let (fixed, n_rep) = repair(live, second);
        r.repaired = n_rep;
        if let Some(t) = truth {
            let t = &t[..n.min(t.len())];
            r.labelled = true;
            r.live = score(live, t);
            r.second = score(second, t);
            r.fixed = score(&fixed, t);
        }
        r
    }

    fn eval_file(path: &Path) -> Result<Row, String> {
        let (sig, sr) = read_wav(path)?;
        let frames = frames_from_signal(&sig, sr, DEFAULT_BANDS);
        let live: Vec<f32> = frames.iter().map(|f| f[PITCH]).collect();
        let second = pyin(&sig, sr);
        let label_path = path.with_extension("f0.csv");
        let truth = match std::fs::read_to_string(&label_path) {
            Ok(text) => Some(labels_to_frames(&parse_labels(&text), live.len().min(second.len()))),
            Err(_) => None,
        };
        Ok(compare(&live, &second, truth.as_deref()))
    }

    fn pct(a: usize, b: usize) -> String {
        if b == 0 { "   -".to_string() } else { format!("{:4.0}", 100.0 * a as f32 / b as f32) }
    }

    fn print_row(name: &str, r: &Row) {
        let lab = |s: &Score| format!("{}/{}", pct(s.pitch_ok, s.ref_voiced), pct(s.voicing_ok, s.frames));
        let (l, p, f) = if r.labelled { (lab(&r.live), lab(&r.second), lab(&r.fixed)) } else { ("-".into(), "-".into(), "-".into()) };
        println!(
            "{:<28} {:>6} {:>6} {:>6} {:>7} {:>5} {:>8} | {:>9} {:>9} {:>9}",
            name,
            r.frames,
            r.live_voiced,
            r.pyin_voiced,
            pct(r.agree, r.both_voiced),
            r.octave,
            r.repaired,
            l,
            p,
            f
        );
    }

    fn collect_inputs(args: &[String]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for a in args {
            let p = PathBuf::from(a);
            if p.is_dir() {
                let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                    .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
                    .unwrap_or_default();
                v.retain(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")));
                v.sort();
                out.extend(v);
            } else {
                out.push(p);
            }
        }
        out
    }

    pub fn run() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let files = collect_inputs(&args);
        if files.is_empty() {
            eprintln!("usage: pitch_eval <dir or wav files...>");
            std::process::exit(2);
        }
        println!(
            "{:<28} {:>6} {:>6} {:>6} {:>7} {:>5} {:>8} | {:>9} {:>9} {:>9}",
            "file", "frames", "live_v", "pyin_v", "agree%", "oct", "repaired", "live", "pyin", "repaired"
        );
        println!("{:<28} {:>6} {:>6} {:>6} {:>7} {:>5} {:>8} | (pitch%/voicing% vs labels)", "", "", "", "", "", "", "");
        let mut total = Row::default();
        for f in &files {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match eval_file(f) {
                Ok(r) => {
                    print_row(&name, &r);
                    total.add(&r);
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        }
        print_row("TOTAL", &total);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn temp_dir(tag: &str) -> PathBuf {
            let d = std::env::temp_dir().join(format!("pitch_eval_{tag}_{}", std::process::id()));
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        #[test]
        fn parses_labels_and_resamples() {
            let l = parse_labels("time,hz\n0.0,100\n0.05,0\n0.1,200\n# x\n");
            assert_eq!(l.len(), 3);
            let f = labels_to_frames(&[(0.0, 100.0), (0.1, 200.0)], 12);
            assert!((f[5] - hz_to_cents(150.0)).abs() < 1.0);
            assert!(f[11].is_nan()); // past the last label
            let f = labels_to_frames(&l, 11);
            assert!(f[5].is_nan()); // 0 hz label at 0.05
            assert!((f[10] - hz_to_cents(200.0)).abs() < 1.0);
        }

        #[test]
        fn compare_counts_octave_errors_and_repairs() {
            let second: Vec<f32> = vec![6000.0; 40];
            let mut live = second.clone();
            for v in &mut live[10..20] {
                *v += 1200.0;
            }
            live[30] = f32::NAN;
            let truth = second.clone();
            let r = compare(&live, &second, Some(&truth));
            assert_eq!(r.frames, 40);
            assert_eq!(r.live_voiced, 39);
            assert_eq!(r.both_voiced, 39);
            assert_eq!(r.agree, 29);
            assert_eq!(r.octave, 10);
            assert_eq!(r.repaired, 10);
            assert_eq!(r.live.pitch_ok, 29);
            assert_eq!(r.second.pitch_ok, 40);
            assert_eq!(r.fixed.pitch_ok, 39);
            assert_eq!(r.fixed.voicing_ok, 39);
        }

        #[test]
        fn wav_round_trip_and_eval_on_synth_tone() {
            let dir = temp_dir("tone");
            let sr = 48_000u32;
            let n = sr as usize * 2;
            let tone: Vec<f32> = (0..n)
                .map(|i| {
                    let t = i as f32 / sr as f32;
                    0.4 * (2.0 * std::f32::consts::PI * 220.0 * t).sin() + 0.1 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                })
                .collect();
            let path = dir.join("tone.wav");
            let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for &s in &tone {
                let v = (s * 32767.0) as i16;
                w.write_sample(v).unwrap();
                w.write_sample(0i16).unwrap();
            }
            w.finalize().unwrap();
            std::fs::write(dir.join("tone.f0.csv"), "0,220\n2,220\n").unwrap();

            let (back, rate) = read_wav(&path).unwrap();
            assert_eq!(rate, sr as f32);
            assert_eq!(back.len(), n);
            assert!((back[1000] - tone[1000]).abs() < 1e-3);

            let r = eval_file(&path).unwrap();
            assert!(r.labelled);
            assert!(r.pyin_voiced as f32 > 0.8 * r.frames as f32);
            assert!(r.second.pitch_ok as f32 > 0.9 * r.second.ref_voiced as f32, "{} of {}", r.second.pitch_ok, r.second.ref_voiced);
            assert!(r.agree as f32 > 0.8 * r.both_voiced as f32);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
