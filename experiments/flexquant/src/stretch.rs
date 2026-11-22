//! Playing a take's audio through a warp map, four ways:
//!
//! - `Varispeed`: what the engine does today (`take_audio` in klont's engine.rs): read the take
//!   at the mapped position with linear interpolation, so a stretch changes the pitch.
//! - `Slice`: cut at the anchors, play each slice at its own speed from its new start, fade it
//!   out where the next one starts (or where its own audio ends). Drums.
//! - `Wsola`: waveform-similarity overlap-add between anchors. Voice and other single sounds.
//! - `Pv`: phase vocoder with identity phase locking between anchors. Chords and pads.
//!
//! The last three are anchor-locked: each segment between two anchors is rendered on its own,
//! starting 5 ms before its anchor and copying the first 20 ms after it verbatim, then
//! stretching, then running at the take's own speed again for its last half frame so it reads
//! exactly the samples the next segment's pre-roll reads. Segments crossfade over the 5 ms
//! before each anchor, between two identical copies. A segment's output depends only on the take and the map, never on what played
//! before it, so output is the same however the calls are sliced and wherever play starts,
//! from the next anchor on.
//!
//! Streaming with fixed buffers, as the engine would run it: everything is allocated in `new`.

use dsp::fft::{Fft, hann};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Method {
    Varispeed,
    Slice,
    Wsola,
    Pv,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Varispeed => "varispeed (today)",
            Method::Slice => "slice",
            Method::Wsola => "wsola",
            Method::Pv => "phase vocoder",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Params {
    /// Pre-roll and crossfade before each anchor, samples.
    pub pre: usize,
    /// Verbatim copy after each anchor, samples.
    pub hold: usize,
    /// WSOLA hop (frame = 2 hops) and search range, samples.
    pub wsola_hop: usize,
    pub wsola_search: usize,
    /// Phase vocoder hop (frame = 4 hops), samples.
    pub pv_hop: usize,
}

impl Default for Params {
    fn default() -> Self {
        // At 48 kHz: 5 ms, 20 ms, 10.7 ms hop / 21 ms frame, 8 ms search, 10.7 ms hop / 43 ms frame.
        Params { pre: 240, hold: 960, wsola_hop: 512, wsola_search: 384, pv_hop: 512 }
    }
}

/// One stretch of output between two anchors, in output samples.
#[derive(Clone, Copy, Debug)]
struct Seg {
    /// Output sample where the segment starts (its anchor minus the pre-roll).
    o: i64,
    /// Output sample where it ends (the next anchor).
    e: i64,
    pre: i64,
    /// Take sample at the anchor, and at the next anchor.
    s: f64,
    s_next: f64,
    /// Verbatim samples after the anchor; samples at the end that run at the take's speed into
    /// the next anchor. Between them the path runs through the soft anchors `knots[k0..k1]`.
    hold: f64,
    tail: f64,
    k0: usize,
    k1: usize,
    fade_in: bool,
    fade_out: bool,
}

impl Seg {
    /// Take position for segment-local output sample `j`.
    fn sigma(&self, j: f64, knots: &[(f64, f64)]) -> f64 {
        let l = self.len() as f64;
        if j >= l - self.tail {
            return self.s_next - (l - j);
        }
        let x = j - self.pre as f64;
        if x < self.hold {
            return self.s + x;
        }
        let (mut ja, mut sa) = (self.pre as f64 + self.hold, self.s + self.hold);
        let (jb, sb) = (l - self.tail, self.s_next - self.tail);
        for &(jk, sk) in &knots[self.k0..self.k1] {
            if jk <= ja || sk <= sa || sk >= sb {
                continue;
            }
            if jk >= jb {
                break;
            }
            if j < jk {
                return sa + (j - ja) * (sk - sa) / (jk - ja);
            }
            ja = jk;
            sa = sk;
        }
        sa + (j - ja) * (sb - sa) / (jb - ja)
    }
    /// A frame centred at `c` sits in a part that runs at the take's speed: it is placed exactly.
    fn exact(&self, c: i64) -> bool {
        (c as f64) <= self.pre as f64 + self.hold || (c as f64) >= self.len() as f64 - self.tail
    }
    fn len(&self) -> i64 {
        self.e - self.o
    }
}

/// Segments between consecutive hard points; the soft points between them go to `knots`
/// (segment-local output sample, take sample). `half`: half the frame length; the tail must
/// cover every frame that reaches the crossfade.
fn segments(map: &[(f64, f64, bool)], p: &Params, min_frame: usize, half: usize, knots: &mut Vec<(f64, f64)>) -> Vec<Seg> {
    let hard: Vec<usize> = (0..map.len()).filter(|&i| map[i].2 || i == 0 || i == map.len() - 1).collect();
    let m = hard.len();
    (0..m - 1)
        .map(|i| {
            let (ia, ib) = (hard[i], hard[i + 1]);
            let (u, s) = (map[ia].0.round() as i64, map[ia].1);
            let (u2, s2) = (map[ib].0.round() as i64, map[ib].1);
            let pre = if i == 0 { 0 } else { p.pre as i64 };
            let fade_out = i + 2 < m;
            let out_len = (u2 - u) as f64;
            let src_len = s2 - s;
            let k0 = knots.len();
            knots.extend(map[ia + 1..ib].iter().map(|q| (q.0 - (u - pre) as f64, q.1)));
            let mut seg = Seg { o: u - pre, e: u2, pre, s, s_next: s2, hold: 0.0, tail: 0.0, k0, k1: knots.len(), fade_in: i > 0, fade_out };
            // The middle has to keep room to stretch, at a sane rate; otherwise drop the exact zones.
            let full = (p.hold as f64, if fade_out { (p.pre + half) as f64 } else { 0.0 });
            for (hold, tail) in [full, (0.0, 0.0)] {
                let mid_out = out_len - hold - (tail - pre as f64).max(0.0);
                let mid_src = src_len - hold - (tail - pre as f64).max(0.0);
                let rate = mid_src / mid_out.max(1.0);
                if (mid_out >= 2.0 * min_frame as f64 && (0.5..=2.0).contains(&rate)) || hold == 0.0 {
                    seg.hold = hold;
                    seg.tail = tail;
                    break;
                }
            }
            seg
        })
        .collect()
}

#[inline]
fn at_below(src: &[f32], i: i64, limit: i64) -> f32 {
    if i < limit { at(src, i) } else { 0.0 }
}

#[inline]
fn at(src: &[f32], i: i64) -> f32 {
    if i >= 0 && (i as usize) < src.len() { src[i as usize] } else { 0.0 }
}

fn read_lin(src: &[f32], pos: f64) -> f32 {
    let i = pos.floor();
    let f = (pos - i) as f32;
    let i = i as i64;
    at(src, i) * (1.0 - f) + at(src, i + 1) * f
}

fn princarg(x: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    x - tau * (x / tau).round()
}

/// One segment being rendered: a slice read, or an overlap-add stream of frames.
struct Rend {
    seg: Seg,
    /// Segment-local output sample of the next sample this renderer returns.
    j: i64,
    // Overlap-add state.
    n: usize,
    hop: usize,
    k: i64,
    j0: i64,
    base: i64,
    acc: Vec<f32>,
    fifo: Vec<f32>,
    fifo_pos: usize,
    fifo_len: usize,
    /// Take samples from here on belong to the next segment and read as silence.
    limit: i64,
    prev_a: i64,
    first: bool,
    win: Vec<f32>,
    // Phase vocoder state.
    fft: Option<Fft>,
    re: Vec<f32>,
    im: Vec<f32>,
    mag: Vec<f32>,
    pha: Vec<f32>,
    pha_prev: Vec<f32>,
    phs: Vec<f32>,
    peaks: Vec<usize>,
}

impl Rend {
    fn new(method: Method, p: &Params) -> Self {
        let (n, hop) = match method {
            Method::Pv => (4 * p.pv_hop, p.pv_hop),
            _ => (2 * p.wsola_hop, p.wsola_hop),
        };
        let mut win = vec![0.0f32; n];
        hann(&mut win);
        let bins = n / 2 + 1;
        Rend {
            seg: Seg { o: 0, e: 0, pre: 0, s: 0.0, s_next: 0.0, hold: 0.0, tail: 0.0, k0: 0, k1: 0, fade_in: false, fade_out: false },
            j: 0,
            n,
            hop,
            k: 0,
            j0: 0,
            base: 0,
            acc: vec![0.0; n],
            fifo: vec![0.0; hop],
            fifo_pos: 0,
            fifo_len: 0,
            limit: i64::MAX,
            prev_a: 0,
            first: true,
            win,
            fft: (method == Method::Pv).then(|| Fft::new(n)),
            re: vec![0.0; n],
            im: vec![0.0; n],
            mag: vec![0.0; bins],
            pha: vec![0.0; bins],
            pha_prev: vec![0.0; bins],
            phs: vec![0.0; bins],
            peaks: Vec::with_capacity(bins),
        }
    }

    /// Start rendering `seg` at segment-local sample `j0` (0 normally, later after a seek).
    /// Frames sit on the segment's own grid (centres at multiples of the hop from its start),
    /// so a seek adds the same frames, in the same order, as a render from the start would.
    fn start(&mut self, seg: Seg, j0: i64) {
        self.seg = seg;
        self.j = j0;
        self.j0 = j0;
        let (hop, n) = (self.hop as i64, self.n as i64);
        // The frame that completes the hop block holding j0, and the first frame that overlaps it.
        let block = j0.div_euclid(hop) * hop;
        let kb = (block + n / 2) / hop;
        self.k = kb - n / hop + 1;
        self.base = self.k * hop - n / 2;
        self.limit = if seg.fade_out { seg.s_next.floor() as i64 } else { i64::MAX };
        self.acc.iter_mut().for_each(|x| *x = 0.0);
        self.fifo_pos = 0;
        self.fifo_len = 0;
        self.first = true;
    }

    fn sample(&mut self, method: Method, src: &[f32], p: &Params, knots: &[(f64, f64)]) -> f32 {
        let seg = self.seg;
        let j = self.j;
        self.j += 1;
        match method {
            Method::Slice => {
                // Play at the take's own speed; stop where the take reaches the next anchor.
                let end = (seg.pre + (seg.s_next - seg.s).floor() as i64).min(seg.len());
                if j >= end {
                    return 0.0;
                }
                let mut g = 1.0;
                let fade = p.pre as i64;
                if end < seg.len() && j >= end - fade {
                    let x = (j - (end - fade)) as f32 + 0.5;
                    g = (std::f32::consts::FRAC_PI_2 * x / fade as f32).cos().powi(2);
                }
                g * at(src, seg.s.round() as i64 - seg.pre + j)
            }
            _ => {
                while self.fifo_pos >= self.fifo_len {
                    self.frame(method, src, p, knots);
                }
                let v = self.fifo[self.fifo_pos];
                self.fifo_pos += 1;
                v
            }
        }
    }

    /// Add the next frame, then hand out the hop it completes.
    fn frame(&mut self, method: Method, src: &[f32], p: &Params, knots: &[(f64, f64)]) {
        let c = self.k * self.hop as i64; // segment-local frame centre
        let ideal = self.seg.sigma(c as f64, knots).round() as i64;
        let half = (self.n / 2) as i64;
        let a = match method {
            Method::Wsola => self.wsola_pos(c, ideal, src, p, knots),
            _ => ideal,
        };
        match method {
            Method::Wsola => {
                for i in 0..self.n {
                    self.acc[i] += self.win[i] * at_below(src, a - half + i as i64, self.limit);
                }
            }
            _ => self.pv_frame(c, a, src),
        }
        self.prev_a = a;
        self.first = false;
        // The first hop of the accumulator is complete.
        let h = self.hop;
        if self.base + h as i64 > self.j0 {
            self.fifo[..h].copy_from_slice(&self.acc[..h]);
            // Only the part at or after the start counts; earlier samples are before j0.
            let skip = (self.j0 - self.base).max(0) as usize;
            self.fifo_pos = skip.min(h);
            self.fifo_len = h;
        }
        self.acc.copy_within(h.., 0);
        let n = self.n;
        self.acc[n - h..].iter_mut().for_each(|x| *x = 0.0);
        self.base += h as i64;
        self.k += 1;
    }

    /// WSOLA: the take position near `ideal` whose start best continues the previous frame.
    fn wsola_pos(&self, c: i64, ideal: i64, src: &[f32], p: &Params, knots: &[(f64, f64)]) -> i64 {
        let hop = self.hop as i64;
        // Where the path runs at the take's speed (pre-roll and hold, and the tail into the next
        // anchor) frames sit exactly where it says, so consecutive frames copy the take verbatim.
        if self.first || self.seg.exact(c) {
            return ideal;
        }
        // The frame before the tail must also join the exactly placed frame after it.
        let next = self.seg.exact(c + hop).then(|| self.seg.sigma((c + hop) as f64, knots).round() as i64);
        let d = p.wsola_search as i64;
        let (mut lo, mut hi) = (-d, d);
        // Never read past the next anchor for output before it: that would play its attack early.
        if self.limit != i64::MAX {
            let cap = self.limit - hop.min(self.seg.len() - c) - ideal;
            hi = hi.min(cap);
            lo = lo.min(hi);
        }
        let score = |cand: i64, step: usize| -> f32 {
            let (mut xy, mut yy) = (0.0f32, 1e-9f32);
            let (mut xz, mut zz) = (0.0f32, 1e-9f32);
            let mut m = 0;
            while m < self.hop {
                let x = at(src, self.prev_a + m as i64);
                let y = at(src, cand - hop + m as i64);
                xy += x * y;
                yy += y * y;
                if let Some(nx) = next {
                    let w = at(src, nx - hop + m as i64);
                    let z = at(src, cand + m as i64);
                    xz += w * z;
                    zz += z * z;
                }
                m += step;
            }
            xy / yy.sqrt() + if next.is_some() { xz / zz.sqrt() } else { 0.0 }
        };
        // Coarse search every 2 samples on every 4th sample, then refine around the best.
        let mut best = (f32::NEG_INFINITY, 0i64);
        let mut dlt = lo;
        while dlt <= hi {
            let s = score(ideal + dlt, 4);
            if s > best.0 {
                best = (s, dlt);
            }
            dlt += 2;
        }
        let centre = best.1;
        best = (f32::NEG_INFINITY, centre);
        for dlt in (centre - 2).max(lo)..=(centre + 2).min(hi) {
            let s = score(ideal + dlt, 1);
            if s > best.0 {
                best = (s, dlt);
            }
        }
        ideal + best.1
    }

    /// Phase vocoder frame at take position `a`, identity phase locking (Laroche and Dolson).
    fn pv_frame(&mut self, c: i64, a: i64, src: &[f32]) {
        let n = self.n;
        let half = (n / 2) as i64;
        let bins = n / 2 + 1;
        for i in 0..n {
            self.re[i] = self.win[i] * at_below(src, a - half + i as i64, self.limit);
            self.im[i] = 0.0;
        }
        let fft = self.fft.as_ref().unwrap();
        fft.forward(&mut self.re, &mut self.im);
        for b in 0..bins {
            self.mag[b] = (self.re[b] * self.re[b] + self.im[b] * self.im[b]).sqrt();
            self.pha[b] = self.im[b].atan2(self.re[b]);
        }
        if self.first || self.seg.exact(c) {
            // Start, pre-roll and hold, and the tail: the synthesis phase is the analysis phase,
            // so these parts reconstruct the take exactly and a seek into them matches a render
            // from the start.
            self.phs[..bins].copy_from_slice(&self.pha[..bins]);
        } else {
            let ha = (a - self.prev_a) as f32;
            let hs = self.hop as f32;
            self.peaks.clear();
            for b in 1..bins - 1 {
                if self.mag[b] > self.mag[b - 1] && self.mag[b] >= self.mag[b + 1] {
                    self.peaks.push(b);
                }
            }
            if self.peaks.is_empty() {
                self.peaks.push(0);
            }
            let w = std::f32::consts::TAU / n as f32;
            for &b in &self.peaks {
                let omega = w * b as f32;
                let inst = if ha > 0.0 { omega + princarg(self.pha[b] - self.pha_prev[b] - omega * ha) / ha } else { omega };
                self.phs[b] = princarg(self.phs[b] + inst * hs);
            }
            // Every other bin keeps its phase relation to the peak it belongs to.
            let mut pi = 0;
            for b in 0..bins {
                while pi + 1 < self.peaks.len() && (self.peaks[pi + 1] as i64 - b as i64).abs() < (self.peaks[pi] as i64 - b as i64).abs() {
                    pi += 1;
                }
                let pk = self.peaks[pi];
                if b != pk {
                    self.phs[b] = princarg(self.phs[pk] + self.pha[b] - self.pha[pk]);
                }
            }
        }
        self.pha_prev[..bins].copy_from_slice(&self.pha[..bins]);
        for b in 0..bins {
            let (s, c) = self.phs[b].sin_cos();
            self.re[b] = self.mag[b] * c;
            self.im[b] = self.mag[b] * s;
        }
        for b in bins..n {
            self.re[b] = self.re[n - b];
            self.im[b] = -self.im[n - b];
        }
        fft.inverse(&mut self.re, &mut self.im);
        // Hann analysis and synthesis at a quarter-frame hop sum to 1.5.
        for i in 0..n {
            self.acc[i] += self.re[i] * self.win[i] / 1.5;
        }
    }
}

pub struct Stretcher<'a> {
    method: Method,
    p: Params,
    src: &'a [f32],
    map: Vec<(f64, f64, bool)>,
    segs: Vec<Seg>,
    knots: Vec<(f64, f64)>,
    pos: i64,
    rends: [Rend; 2],
    /// (segment index, renderer slot) of the current segment and the next one fading in.
    cur: Option<(usize, usize)>,
    nxt: Option<(usize, usize)>,
}

impl<'a> Stretcher<'a> {
    pub fn new(method: Method, p: Params, src: &'a [f32], map: &[(f64, f64, bool)]) -> Self {
        let min_frame = match method {
            Method::Pv => 2 * p.pv_hop,
            _ => p.wsola_hop,
        };
        let half = match method {
            Method::Pv => 2 * p.pv_hop,
            _ => p.wsola_hop,
        };
        let mut knots = Vec::new();
        let segs = segments(map, &p, min_frame, half, &mut knots);
        let rends = [Rend::new(method, &p), Rend::new(method, &p)];
        let mut s = Stretcher { method, p, src, map: map.to_vec(), segs, knots, pos: 0, rends, cur: None, nxt: None };
        s.seek(0);
        s
    }

    /// Jump to output sample `n`, as Play from a stopped transport does.
    pub fn seek(&mut self, n: i64) {
        self.pos = n;
        self.cur = None;
        self.nxt = None;
        if let Some(i) = self.segs.iter().position(|s| s.o <= n && n < s.e) {
            let seg = self.segs[i];
            self.rends[0].start(seg, n - seg.o);
            self.cur = Some((i, 0));
            if let Some(s2) = self.segs.get(i + 1).copied() {
                if s2.o <= n {
                    self.rends[1].start(s2, n - s2.o);
                    self.nxt = Some((i + 1, 1));
                }
            }
        }
    }

    fn varispeed_pos(&self, n: f64) -> f64 {
        let m = &self.map;
        let i = m.partition_point(|q| q.0 <= n).clamp(1, m.len() - 1);
        let (a, b) = (m[i - 1], m[i]);
        a.1 + (b.1 - a.1) * (n - a.0) / (b.0 - a.0)
    }

    pub fn process(&mut self, out: &mut [f32]) {
        if self.method == Method::Varispeed {
            for o in out.iter_mut() {
                *o = read_lin(self.src, self.varispeed_pos(self.pos as f64));
                self.pos += 1;
            }
            return;
        }
        let fade = self.p.pre as f32;
        for o in out.iter_mut() {
            // Hand over to the next segment at the current one's end.
            if let Some((ci, _)) = self.cur {
                if self.pos >= self.segs[ci].e {
                    self.cur = self.nxt.take();
                    if self.cur.is_none() {
                        if let Some(s2) = self.segs.get(ci + 1).copied() {
                            let slot = 0;
                            self.rends[slot].start(s2, self.pos - s2.o);
                            self.cur = Some((ci + 1, slot));
                        }
                    }
                }
            }
            let Some((ci, cs)) = self.cur else {
                *o = 0.0;
                self.pos += 1;
                continue;
            };
            if self.nxt.is_none() {
                if let Some(s2) = self.segs.get(ci + 1).copied() {
                    if s2.o <= self.pos {
                        let slot = 1 - cs;
                        self.rends[slot].start(s2, self.pos - s2.o);
                        self.nxt = Some((ci + 1, slot));
                    }
                }
            }
            let seg = self.segs[ci];
            let mut v = self.rends[cs].sample(self.method, self.src, &self.p, &self.knots);
            if seg.fade_out && self.pos >= seg.e - self.p.pre as i64 {
                let x = (self.pos - (seg.e - self.p.pre as i64)) as f32 + 0.5;
                v *= (std::f32::consts::FRAC_PI_2 * x / fade).cos().powi(2);
            }
            if let Some((_, ns)) = self.nxt {
                let s2 = self.rends[ns].seg;
                let mut w = self.rends[ns].sample(self.method, self.src, &self.p, &self.knots);
                if s2.fade_in {
                    let x = (self.pos - s2.o) as f32 + 0.5;
                    w *= (std::f32::consts::FRAC_PI_2 * x / fade).sin().powi(2);
                }
                v += w;
            }
            *o = v;
            self.pos += 1;
        }
    }
}

/// Render `len` output samples from the start in one call.
pub fn render(method: Method, p: Params, src: &[f32], map: &[(f64, f64, bool)], len: usize) -> Vec<f32> {
    let mut s = Stretcher::new(method, p, src, map);
    let mut out = vec![0.0f32; len];
    s.process(&mut out);
    out
}
