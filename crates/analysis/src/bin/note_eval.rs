//! Score the second pass's notes against hand-labelled hums: the "hums kept"
//! measure (TDD "Testing", melody analysis; 80% kept target).
//!
//! Usage: cargo run -p analysis --release --bin note_eval -- [--melody] <dir or wav files...>
//!
//! For each WAV (first channel, any rate) with a label file `<name>.notes.csv` next to
//! it, the tool runs the live tier over the file (`frames_from_signal`, 16 bands), then
//! `second_pass`, and scores the found notes against the labels. Files without labels
//! are skipped. `--melody` forces the clip kind to melody; otherwise it is detected
//! (the kind is printed, and a hum read as a beat or texture has no notes).
//!
//! Label file: one note per line, `onset_s,offset_s,pitch`. A header line is optional
//! (lines that do not start with a number are skipped); if its third column is named
//! `hz` the pitch column is frequency in Hz, otherwise it is a MIDI note number (60 =
//! middle C, fractions allowed). Without a header, values above 127 are read as Hz. Time
//! is seconds from the start of the WAV.
//!
//! A found note matches a labelled note when its onset is within 50 ms and its pitch
//! within 50 cents, one to one. Per hum it prints precision, recall and F1; a hum is
//! "kept" when F1 >= 0.9. Then the totals and the kept share against the 80% target,
//! and the unmatched notes of every hum that was not kept.
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
    use analysis::score::{KEPT_F1, KEPT_TARGET, NoteScore, ONSET_TOL_S, PITCH_TOL_CENTS, match_notes};
    use analysis::second::{Kind, Options, STRIDE, second_pass};
    use dsp::capture::frames_from_signal;
    use dsp::frame::{DEFAULT_BANDS, frame_len};
    use dsp::hz_to_cents;
    use std::path::{Path, PathBuf};

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

    /// Parse `onset_s,offset_s,pitch` lines into (onset s, offset s, cents above C0).
    pub fn parse_labels(text: &str) -> Vec<(f32, f32, f32)> {
        let mut hz_column = false;
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split(',').map(|c| c.trim()).collect();
            if cols.len() < 3 {
                continue;
            }
            let nums: Vec<Option<f32>> = cols.iter().take(3).map(|c| c.parse::<f32>().ok()).collect();
            if nums.iter().any(|n| n.is_none()) {
                // Header: the third column says what the pitch is.
                if cols[2].to_ascii_lowercase().contains("hz") {
                    hz_column = true;
                }
                continue;
            }
            let (on, off, p) = (nums[0].unwrap(), nums[1].unwrap(), nums[2].unwrap());
            let cents = if hz_column || p > 127.0 { hz_to_cents(p) } else { (p - 12.0) * 100.0 };
            if p > 0.0 && on.is_finite() && off >= on {
                out.push((on, off, cents));
            }
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    fn collect(args: &[String]) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for a in args {
            let p = PathBuf::from(a);
            if p.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav"))).collect();
                    v.sort();
                    files.extend(v);
                }
            } else {
                files.push(p);
            }
        }
        files
    }

    pub fn run() {
        let mut args: Vec<String> = std::env::args().skip(1).collect();
        let force_melody = args.iter().any(|a| a == "--melody");
        args.retain(|a| a != "--melody");
        if args.is_empty() {
            eprintln!("usage: note_eval [--melody] <dir or wav files...>");
            std::process::exit(2);
        }
        let opts = Options { kind: if force_melody { Some(Kind::Melody) } else { None }, ..Options::default() };
        let (mut total, mut kept, mut hums) = (NoteScore::default(), 0usize, 0usize);
        for path in collect(&args) {
            let labels = path.with_extension("notes.csv");
            let Ok(text) = std::fs::read_to_string(&labels) else {
                println!("{}: no {} (skipped)", path.display(), labels.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                continue;
            };
            let truth = parse_labels(&text);
            let (audio, sr) = match read_wav(&path) {
                Ok(v) => v,
                Err(e) => {
                    println!("{}: {e}", path.display());
                    continue;
                }
            };
            let frames = frames_from_signal(&audio, sr, DEFAULT_BANDS);
            let mut rec = Vec::with_capacity(frames.len() * STRIDE);
            for f in &frames {
                rec.extend_from_slice(&f[..frame_len(DEFAULT_BANDS)]);
                rec.push(f32::NAN);
            }
            let r = second_pass(&audio, sr, &rec, &opts);
            let found: Vec<(f32, f32)> = r.notes.iter().map(|n| (n.t, n.cents)).collect();
            let want: Vec<(f32, f32)> = truth.iter().map(|t| (t.0, t.2)).collect();
            let (score, of_found) = match_notes(&found, &want, ONSET_TOL_S, PITCH_TOL_CENTS);
            hums += 1;
            kept += score.kept() as usize;
            total.add(&score);
            println!(
                "{} {}: P {:.2} R {:.2} F1 {:.2} ({} of {} labelled, {} found) kind {:?}",
                if score.kept() { "kept  " } else { "FAILED" },
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                score.precision(),
                score.recall(),
                score.f1(),
                score.tp,
                score.truth,
                score.found,
                r.kind
            );
            if !score.kept() {
                let mut hit = vec![false; want.len()];
                for j in of_found.iter().flatten() {
                    hit[*j] = true;
                }
                for (j, t) in truth.iter().enumerate().filter(|(j, _)| !hit[*j]) {
                    println!("        missed: labelled #{j} {:.2}s..{:.2}s {:.0} cents", t.0, t.1, t.2);
                }
                for (i, f) in found.iter().enumerate().filter(|(i, _)| of_found[*i].is_none()) {
                    println!("        extra:  found #{i} {:.2}s {:.0} cents", f.0, f.1);
                }
            }
        }
        if hums == 0 {
            println!("no labelled WAVs found (each needs a <name>.notes.csv next to it)");
            return;
        }
        let share = kept as f32 / hums as f32;
        println!(
            "\n{hums} hums, {} labelled notes, {} found. precision {:.3} recall {:.3} F1 {:.3}",
            total.truth,
            total.found,
            total.precision(),
            total.recall(),
            total.f1()
        );
        println!(
            "kept {kept} of {hums} = {:.1}% (hum kept at F1 >= {KEPT_F1}; target {:.0}%): {}",
            100.0 * share,
            100.0 * KEPT_TARGET,
            if share >= KEPT_TARGET { "meets the target" } else { "below the target" }
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn labels_midi_hz_and_headers() {
            let l = parse_labels("onset_s,offset_s,midi\n0.5,1.0,60\n1.0,1.5,61.5\n# c\n\n");
            assert_eq!(l.len(), 2);
            assert!((l[0].2 - 4800.0).abs() < 1e-3 && (l[1].2 - 4950.0).abs() < 1e-3);
            let l = parse_labels("onset_s,offset_s,hz\n0.5,1.0,110\n");
            assert!((l[0].2 - hz_to_cents(110.0)).abs() < 1e-3);
            // No header: 262 is above 127 so it is Hz; 60 is MIDI.
            let l = parse_labels("0.0,0.5,262\n0.5,1.0,60\n");
            assert!((l[0].2 - hz_to_cents(262.0)).abs() < 1e-3 && (l[1].2 - 4800.0).abs() < 1e-3);
            // Bad lines are skipped.
            assert!(parse_labels("a,b\n1,2\nx,y,z\n").is_empty());
        }
    }
}
