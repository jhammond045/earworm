//! Note-level scoring for the "hums kept" measure (TDD "Testing", melody analysis):
//! precision and recall of found notes against labelled ones.

/// A found note matches a true note when its onset is within this many seconds ...
pub const ONSET_TOL_S: f32 = 0.05;
/// ... and its pitch within this many cents.
pub const PITCH_TOL_CENTS: f32 = 50.0;
/// A hum is kept when its F1 reaches this (about one mistake in ten notes).
pub const KEPT_F1: f32 = 0.9;
/// The share of hums that should be kept.
pub const KEPT_TARGET: f32 = 0.8;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NoteScore {
    /// Matched notes.
    pub tp: usize,
    pub found: usize,
    pub truth: usize,
}

impl NoteScore {
    pub fn precision(&self) -> f32 {
        if self.found == 0 { if self.truth == 0 { 1.0 } else { 0.0 } } else { self.tp as f32 / self.found as f32 }
    }
    pub fn recall(&self) -> f32 {
        if self.truth == 0 { 1.0 } else { self.tp as f32 / self.truth as f32 }
    }
    pub fn f1(&self) -> f32 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
    }
    pub fn kept(&self) -> bool {
        self.f1() >= KEPT_F1
    }
    pub fn add(&mut self, o: &NoteScore) {
        self.tp += o.tp;
        self.found += o.found;
        self.truth += o.truth;
    }
}

/// One-to-one matching of `found` against `truth`, each `(onset seconds, cents above C0)`.
/// Candidate pairs within both tolerances are taken closest onset first. Returns the score
/// and, for each found note, the index of the true note it matched.
pub fn match_notes(found: &[(f32, f32)], truth: &[(f32, f32)], onset_tol: f32, cents_tol: f32) -> (NoteScore, Vec<Option<usize>>) {
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (i, f) in found.iter().enumerate() {
        for (j, t) in truth.iter().enumerate() {
            let dt = (f.0 - t.0).abs();
            if dt <= onset_tol && (f.1 - t.1).abs() <= cents_tol {
                pairs.push((dt, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut of_found = vec![None; found.len()];
    let mut taken = vec![false; truth.len()];
    let mut tp = 0;
    for (_, i, j) in pairs {
        if of_found[i].is_none() && !taken[j] {
            of_found[i] = Some(j);
            taken[j] = true;
            tp += 1;
        }
    }
    (NoteScore { tp, found: found.len(), truth: truth.len() }, of_found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_within_tolerance_one_to_one() {
        let truth = [(1.0, 5000.0), (2.0, 5200.0), (3.0, 5200.0)];
        let found = [(1.04, 5030.0), (2.06, 5200.0), (3.01, 5260.0), (3.02, 5200.0)];
        let (s, of) = match_notes(&found, &truth, ONSET_TOL_S, PITCH_TOL_CENTS);
        // 2.06 is 60 ms late; 3.01 is 60 cents off; 3.02 matches note 3; 1.04 matches note 1.
        assert_eq!((s.tp, s.found, s.truth), (2, 4, 3));
        assert_eq!(of, vec![Some(0), None, None, Some(2)]);
        assert!((s.precision() - 0.5).abs() < 1e-6 && (s.recall() - 2.0 / 3.0).abs() < 1e-6);
        // Two found notes cannot claim one true note.
        let (s, _) = match_notes(&[(1.0, 5000.0), (1.01, 5000.0)], &[(1.0, 5000.0)], ONSET_TOL_S, PITCH_TOL_CENTS);
        assert_eq!(s.tp, 1);
    }

    #[test]
    fn kept_means_f1_of_point_nine() {
        let s = NoteScore { tp: 9, found: 10, truth: 10 };
        assert!(s.kept());
        let s = NoteScore { tp: 8, found: 10, truth: 10 };
        assert!(!s.kept());
        assert_eq!(NoteScore::default().f1(), 1.0);
        assert_eq!(NoteScore { tp: 0, found: 3, truth: 0 }.f1(), 0.0);
    }
}
