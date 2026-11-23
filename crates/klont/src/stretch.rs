//! Stretching a flexed clip's take audio through its time map without changing its pitch
//! (docs/quant.md "Stretching the audio", stage Q2). Moved from the prototype in
//! experiments/flexquant (`stretch.rs`) as it was measured, less WSOLA, and made to loop.
//!
//! The compiler sends the map as points (clip time, take second, flags) folded into the clip's
//! loop: runs of points, each run a stretch of the take that plays inside one pass of the loop
//! (a take placed across the loop end gives two runs). The audio is cut at hard points only, and
//! each stretch between two hard points is rendered on its own:
//!
//! - it starts `PRE` (5 ms) before its point and copies the `HOLD` (20 ms) after it verbatim, so
//!   the attack is untouched;
//! - its last half frame runs at the take's own speed, so it reads exactly the samples the next
//!   stretch's pre-roll reads, and the 5 ms crossfade adds two copies of the same audio;
//! - it never reads past its end point, so no stretch plays the next attack early;
//! - soft points between hard ones only bend it.
//!
//! Two modes: **hits** plays each stretch at the take's own speed from its new start and fades it
//! out where the next one starts or its own audio runs out (beat layers); **smooth** is a phase
//! vocoder (2,048-sample frames, a quarter-frame hop, identity phase locking) reset at every hard
//! point (melody and texture).
//!
//! Determinism: a stretch's frames sit on its own grid, so its output depends on the take, the
//! map and the later of its start and the point play started, never on how calls are sliced.
//! Every buffer is allocated in `new`; `sync` and `process` never allocate.

use dsp::fft::{Fft, hann};

/// Clip audio modes (track blob v5, clip word 15).
pub const MODE_READ: u32 = 0;
pub const MODE_HITS: u32 = 1;
pub const MODE_SMOOTH: u32 = 2;
/// Flags of a map point (track blob v5, warp anchor word 2): a hard point cuts the audio and is
/// copied verbatim; a run point starts a new run (the point before it ends the last one).
pub const POINT_HARD: u32 = 1;
pub const POINT_RUN: u32 = 2;
/// Most points one clip's map can hold here (web/src/m1/track.ts MAX_STRETCH_POINTS); the
/// compiler sends no stretch map with more. A 59 s take of 16ths at 180 bpm has about 700.
pub const MAX_POINTS: usize = 1024;

const PRE_S: f64 = 0.005;
const HOLD_S: f64 = 0.020;
const HOP: usize = 512;
const N: usize = 4 * HOP;
const BINS: usize = N / 2 + 1;

/// A map point in samples: `u` inside the pass (output), `s` in the take.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub u: f64,
    pub s: f64,
    pub flags: u32,
}

/// One stretch as rendered: output samples `o..e` (counted by the stretcher), take samples from
/// `s` at its point to `s_next` at the next.
#[derive(Clone, Copy, Debug, Default)]
struct Seg {
    o: i64,
    e: i64,
    pre: i64,
    s: f64,
    s_next: f64,
    /// Verbatim samples after the point; samples at the end that run at the take's speed.
    hold: f64,
    tail: f64,
    /// Soft points between: `pts[k0..k1]`, at stretch-local output sample `u + koff`.
    k0: usize,
    k1: usize,
    koff: f64,
}

impl Seg {
    fn len(&self) -> i64 {
        self.e - self.o
    }

    /// Take position for stretch-local output sample `j`.
    fn sigma(&self, j: f64, pts: &[Point]) -> f64 {
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
        for p in &pts[self.k0..self.k1] {
            let (jk, sk) = (p.u + self.koff, p.s);
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

    /// A frame centred at `c` sits where the path runs at the take's speed.
    fn exact(&self, c: i64) -> bool {
        (c as f64) <= self.pre as f64 + self.hold || (c as f64) >= self.len() as f64 - self.tail
    }
}

#[inline]
fn at(src: &[f32], i: i64) -> f32 {
    if i >= 0 && (i as usize) < src.len() { src[i as usize] } else { 0.0 }
}

#[inline]
fn at_below(src: &[f32], i: i64, limit: i64) -> f32 {
    if i < limit { at(src, i) } else { 0.0 }
}

fn princarg(x: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    x - tau * (x / tau).round()
}

/// Raised-cosine gains over `fade` samples: `x` samples in, 0.5 offset so the two halves of a
/// crossfade sum to exactly 1.
#[inline]
fn fade_in(x: i64, fade: i64) -> f32 {
    let a = std::f32::consts::FRAC_PI_2 * (x as f32 + 0.5) / fade as f32;
    a.sin().powi(2)
}

#[inline]
fn fade_out(x: i64, fade: i64) -> f32 {
    let a = std::f32::consts::FRAC_PI_2 * (x as f32 + 0.5) / fade as f32;
    a.cos().powi(2)
}

/// One stretch being rendered: a slice read, or a phase-vocoder stream of frames.
struct Rend {
    seg: Seg,
    /// Stretch-local output sample of the next sample this renderer returns.
    j: i64,
    k: i64,
    j0: i64,
    base: i64,
    acc: Vec<f32>,
    fifo: Vec<f32>,
    fifo_pos: usize,
    fifo_len: usize,
    /// Take samples from here on belong to the next stretch and read as silence.
    limit: i64,
    prev_a: i64,
    first: bool,
    win: Vec<f32>,
    fft: Fft,
    re: Vec<f32>,
    im: Vec<f32>,
    mag: Vec<f32>,
    pha: Vec<f32>,
    pha_prev: Vec<f32>,
    phs: Vec<f32>,
    peaks: Vec<usize>,
}

impl Rend {
    fn new() -> Self {
        let mut win = vec![0.0f32; N];
        hann(&mut win);
        Rend {
            seg: Seg::default(),
            j: 0,
            k: 0,
            j0: 0,
            base: 0,
            acc: vec![0.0; N],
            fifo: vec![0.0; HOP],
            fifo_pos: 0,
            fifo_len: 0,
            limit: i64::MAX,
            prev_a: 0,
            first: true,
            win,
            fft: Fft::new(N),
            re: vec![0.0; N],
            im: vec![0.0; N],
            mag: vec![0.0; BINS],
            pha: vec![0.0; BINS],
            pha_prev: vec![0.0; BINS],
            phs: vec![0.0; BINS],
            peaks: Vec::with_capacity(BINS),
        }
    }

    /// Start rendering `seg` at stretch-local sample `j0` (0 normally, later after a seek).
    /// Frames sit on the stretch's own grid (centres at multiples of the hop from its start), so
    /// a seek adds the same frames, in the same order, as a render from the start would.
    fn start(&mut self, seg: Seg, j0: i64) {
        self.seg = seg;
        self.j = j0;
        self.j0 = j0;
        let (hop, n) = (HOP as i64, N as i64);
        let block = j0.div_euclid(hop) * hop;
        let kb = (block + n / 2) / hop;
        self.k = kb - n / hop + 1;
        self.base = self.k * hop - n / 2;
        self.limit = seg.s_next.floor() as i64;
        self.acc.fill(0.0);
        self.fifo_pos = 0;
        self.fifo_len = 0;
        self.first = true;
    }

    fn sample(&mut self, mode: u32, src: &[f32], pts: &[Point]) -> f32 {
        let seg = self.seg;
        let j = self.j;
        self.j += 1;
        if mode == MODE_HITS {
            // The take's own speed from the new start; stop where the take reaches the next point.
            let end = (seg.pre + (seg.s_next - seg.s).floor() as i64).min(seg.len());
            if j >= end {
                return 0.0;
            }
            let fade = seg.pre.max(1);
            let g = if end < seg.len() && j >= end - fade { fade_out(j - (end - fade), fade) } else { 1.0 };
            return g * at(src, seg.s.round() as i64 - seg.pre + j);
        }
        while self.fifo_pos >= self.fifo_len {
            self.frame(src, pts);
        }
        let v = self.fifo[self.fifo_pos];
        self.fifo_pos += 1;
        v
    }

    /// Add the next frame, then hand out the hop it completes.
    fn frame(&mut self, src: &[f32], pts: &[Point]) {
        let c = self.k * HOP as i64;
        let a = self.seg.sigma(c as f64, pts).round() as i64;
        self.pv_frame(c, a, src);
        self.prev_a = a;
        self.first = false;
        if self.base + HOP as i64 > self.j0 {
            self.fifo.copy_from_slice(&self.acc[..HOP]);
            let skip = (self.j0 - self.base).max(0) as usize;
            self.fifo_pos = skip.min(HOP);
            self.fifo_len = HOP;
        }
        self.acc.copy_within(HOP.., 0);
        self.acc[N - HOP..].fill(0.0);
        self.base += HOP as i64;
        self.k += 1;
    }

    /// Phase vocoder frame at take position `a`, identity phase locking (Laroche and Dolson).
    fn pv_frame(&mut self, c: i64, a: i64, src: &[f32]) {
        let half = (N / 2) as i64;
        for i in 0..N {
            self.re[i] = self.win[i] * at_below(src, a - half + i as i64, self.limit);
            self.im[i] = 0.0;
        }
        self.fft.forward(&mut self.re, &mut self.im);
        for b in 0..BINS {
            self.mag[b] = (self.re[b] * self.re[b] + self.im[b] * self.im[b]).sqrt();
            self.pha[b] = self.im[b].atan2(self.re[b]);
        }
        if self.first || self.seg.exact(c) {
            // Start, pre-roll and hold, and the tail: the synthesis phase is the analysis phase,
            // so these parts give back the take exactly and a seek into them matches a render
            // from the start.
            self.phs.copy_from_slice(&self.pha);
        } else {
            let ha = (a - self.prev_a) as f32;
            let hs = HOP as f32;
            self.peaks.clear();
            for b in 1..BINS - 1 {
                if self.mag[b] > self.mag[b - 1] && self.mag[b] >= self.mag[b + 1] {
                    self.peaks.push(b);
                }
            }
            if self.peaks.is_empty() {
                self.peaks.push(0);
            }
            let w = std::f32::consts::TAU / N as f32;
            for &b in &self.peaks {
                let omega = w * b as f32;
                let inst = if ha > 0.0 { omega + princarg(self.pha[b] - self.pha_prev[b] - omega * ha) / ha } else { omega };
                self.phs[b] = princarg(self.phs[b] + inst * hs);
            }
            // Every other bin keeps its phase relation to the peak it belongs to.
            let mut pi = 0;
            for b in 0..BINS {
                while pi + 1 < self.peaks.len() && (self.peaks[pi + 1] as i64 - b as i64).abs() < (self.peaks[pi] as i64 - b as i64).abs() {
                    pi += 1;
                }
                let pk = self.peaks[pi];
                if b != pk {
                    self.phs[b] = princarg(self.phs[pk] + self.pha[b] - self.pha[pk]);
                }
            }
        }
        self.pha_prev.copy_from_slice(&self.pha);
        for b in 0..BINS {
            let (s, c) = self.phs[b].sin_cos();
            self.re[b] = self.mag[b] * c;
            self.im[b] = self.mag[b] * s;
        }
        for b in BINS..N {
            self.re[b] = self.re[N - b];
            self.im[b] = -self.im[N - b];
        }
        self.fft.inverse(&mut self.re, &mut self.im);
        // Hann analysis and synthesis at a quarter-frame hop sum to 1.5.
        for i in 0..N {
            self.acc[i] += self.re[i] * self.win[i] / 1.5;
        }
    }
}

/// A stretch playing in one of the two renderers: its global index (pass * stretches + i) and
/// whether its pre-roll fades in (not when play started inside it).
#[derive(Clone, Copy, Debug)]
struct Playing {
    g: i64,
    r: usize,
    fade_in: bool,
}

/// One clip's map, played from a start point on. Output samples are counted from that start.
pub struct Stretcher {
    mode: u32,
    pts: Vec<Point>,
    /// (first point, last point) of each stretch, in order through the pass.
    segs: Vec<(u32, u32)>,
    /// Pass length, samples.
    len: f64,
    pre: i64,
    hold: f64,
    /// Output samples rendered since the start.
    pos: i64,
    /// Where pass 0 starts, in output samples (negative: play started inside it).
    ps: f64,
    rends: [Rend; 2],
    cur: Option<Playing>,
    nxt: Option<Playing>,
    /// The stretch to start next when none plays (a gap between runs).
    next_g: i64,
}

impl Stretcher {
    pub fn new(sr: f32) -> Self {
        Stretcher {
            mode: MODE_READ,
            pts: Vec::with_capacity(MAX_POINTS),
            segs: Vec::with_capacity(MAX_POINTS),
            len: 0.0,
            pre: (PRE_S * sr as f64).round() as i64,
            hold: (HOLD_S * sr as f64).round(),
            pos: 0,
            ps: 0.0,
            rends: [Rend::new(), Rend::new()],
            cur: None,
            nxt: None,
            next_g: 0,
        }
    }

    pub fn pre(&self) -> i64 {
        self.pre
    }

    /// Whether `pts` (in samples, as `load` takes them) and `mode`, `len` are what this plays.
    pub fn same(&self, mode: u32, len: f64, pts: impl Iterator<Item = Point>) -> bool {
        if mode != self.mode || len != self.len {
            return false;
        }
        let mut n = 0;
        for p in pts {
            if n >= self.pts.len() || self.pts[n] != p {
                return false;
            }
            n += 1;
        }
        n == self.pts.len()
    }

    /// Take a new map: `pts` in samples, sorted by `u`, `len` the pass length in samples. Points
    /// past MAX_POINTS are dropped. False when there is nothing to play.
    pub fn load(&mut self, mode: u32, len: f64, pts: impl Iterator<Item = Point>) -> bool {
        self.mode = mode;
        self.len = len;
        self.pts.clear();
        for p in pts.take(MAX_POINTS) {
            self.pts.push(p);
        }
        self.segs.clear();
        let n = self.pts.len();
        let run_end = |i: usize, pts: &[Point]| i + 1 == n || pts[i + 1].flags & POINT_RUN != 0;
        let mut i = 0;
        while i < n {
            if run_end(i, &self.pts) {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while !run_end(j, &self.pts) && self.pts[j].flags & POINT_HARD == 0 {
                j += 1;
            }
            if self.pts[j].u > self.pts[i].u && self.pts[j].s > self.pts[i].s {
                self.segs.push((i as u32, j as u32));
            }
            i = j;
        }
        self.cur = None;
        self.nxt = None;
        !self.segs.is_empty() && len > 0.0
    }

    /// Stretch `g` (pass `g / stretches`), as rendered from the current start.
    fn seg(&self, g: i64) -> Seg {
        let m = self.segs.len() as i64;
        let (pass, i) = (g.div_euclid(m), g.rem_euclid(m) as usize);
        let (a, b) = self.segs[i];
        let (pa, pb) = (self.pts[a as usize], self.pts[b as usize]);
        let p0 = self.ps + pass as f64 * self.len;
        let u = (p0 + pa.u).round() as i64;
        let u2 = (p0 + pb.u).round() as i64;
        let pre = self.pre;
        let mut seg = Seg {
            o: u - pre,
            e: u2,
            pre,
            s: pa.s,
            s_next: pb.s,
            hold: 0.0,
            tail: 0.0,
            k0: a as usize + 1,
            k1: b as usize,
            koff: p0 - (u - pre) as f64,
        };
        if self.mode == MODE_SMOOTH {
            // The middle has to keep room to stretch, at a sane rate; otherwise drop the exact zones.
            let (out_len, src_len) = ((u2 - u) as f64, pb.s - pa.s);
            let (min_frame, half) = (2 * HOP, N / 2);
            for (hold, tail) in [(self.hold, (pre as usize + half) as f64), (0.0, 0.0)] {
                let over = (tail - pre as f64).max(0.0);
                let mid_out = out_len - hold - over;
                let mid_src = src_len - hold - over;
                let rate = mid_src / mid_out.max(1.0);
                if (mid_out >= 2.0 * min_frame as f64 && (0.5..=2.0).contains(&rate)) || hold == 0.0 {
                    seg.hold = hold;
                    seg.tail = tail;
                    break;
                }
            }
        }
        seg
    }

    /// Play from here: the current output sample is at `u` samples into a pass. Nothing from
    /// before is kept.
    pub fn seek(&mut self, u: f64) {
        self.pos = 0;
        self.ps = -u;
        self.cur = None;
        self.nxt = None;
        let m = self.segs.len() as i64;
        // Pass 0 holds the start point; the first stretch of pass 1 may already be in its pre-roll.
        let mut g = 0;
        while g <= m && self.seg(g).e <= 0 {
            g += 1;
        }
        self.next_g = g;
        let s = self.seg(g);
        if s.o <= 0 {
            self.rends[0].start(s, -s.o);
            self.cur = Some(Playing { g, r: 0, fade_in: false });
            let s2 = self.seg(g + 1);
            if s2.o <= 0 {
                self.rends[1].start(s2, -s2.o);
                self.nxt = Some(Playing { g: g + 1, r: 1, fade_in: true });
            }
        }
    }

    /// Where pass 0 started, and the pass length, in output samples since the start.
    pub fn phase(&self) -> (i64, f64, f64) {
        (self.pos, self.ps, self.len)
    }

    /// Add the next `out.len()` samples into `out`, times `gain(i)`.
    pub fn process(&mut self, src: &[f32], out: &mut [f32], mut gain: impl FnMut(usize) -> f32) {
        let pre = self.pre;
        for (i, o) in out.iter_mut().enumerate() {
            let pos = self.pos;
            self.pos += 1;
            if let Some(c) = self.cur {
                if pos >= self.rends[c.r].seg.e {
                    self.cur = self.nxt.take();
                    self.next_g = c.g + 1;
                }
            }
            if self.cur.is_none() {
                let s = self.seg(self.next_g);
                if s.o > pos {
                    continue;
                }
                self.rends[0].start(s, pos - s.o);
                self.cur = Some(Playing { g: self.next_g, r: 0, fade_in: true });
            }
            let c = self.cur.unwrap();
            if self.nxt.is_none() {
                let s2 = self.seg(c.g + 1);
                if s2.o <= pos {
                    let r = 1 - c.r;
                    self.rends[r].start(s2, pos - s2.o);
                    self.nxt = Some(Playing { g: c.g + 1, r, fade_in: true });
                }
            }
            let mut v = 0.0;
            for p in [Some(c), self.nxt].into_iter().flatten() {
                let rd = &mut self.rends[p.r];
                let seg = rd.seg;
                let mut w = rd.sample(self.mode, src, &self.pts);
                let j = pos - seg.o;
                if p.fade_in && j < pre {
                    w *= fade_in(j, pre);
                }
                if pos >= seg.e - pre {
                    w *= fade_out(pos - (seg.e - pre), pre);
                }
                v += w;
            }
            *o += v * gain(i);
        }
    }
}

/// A layer's stretched take audio: the stretcher playing the clip's map, and a second one that
/// fades out over 5 ms when the map changes, the clip changes, or the transport moves where the
/// stretcher did not expect (docs/quant.md "Engine changes").
pub struct LayerStretch {
    st: [Stretcher; 2],
    /// Which stretcher plays the current map; the other may be fading out.
    main: usize,
    on: [bool; 2],
    sample: [u16; 2],
    /// Samples into the old one's fade out, and into the new one's fade in.
    fade_old: i64,
    fade_new: i64,
    /// What `main` plays: the clip index and the track it came from.
    clip: usize,
    track_gen: u64,
}

/// Where the stretched audio of the clip playing now should be.
pub struct StretchAt<'a> {
    pub clip: usize,
    pub track_gen: u64,
    pub mode: u32,
    /// The take's audio (a region of the sample pool).
    pub region: u16,
    /// Pass length in output samples, and the clip position at the tick start in output samples.
    pub len: f64,
    pub u: f64,
    /// Output samples per section unit, and take samples per take second.
    pub spu: f64,
    pub rate: f64,
    /// The clip's map: (take second, clip time in section units, flags).
    pub map: &'a [crate::seq::WarpAnchor],
    /// The transport is ramping to the section's tempo: keep playing without checking where.
    pub ramping: bool,
}

impl LayerStretch {
    pub fn new(sr: f32) -> Self {
        LayerStretch {
            st: [Stretcher::new(sr), Stretcher::new(sr)],
            main: 0,
            on: [false; 2],
            sample: [0; 2],
            fade_old: i64::MAX,
            fade_new: i64::MAX,
            clip: usize::MAX,
            track_gen: 0,
        }
    }

    /// Nothing plays; the next `sync` starts fresh, as Play from a stop does.
    pub fn stop(&mut self) {
        self.on = [false; 2];
        self.clip = usize::MAX;
    }

    pub fn playing(&self) -> bool {
        self.on[self.main]
    }

    /// Once per control tick while the clip plays: keep playing, or start the stretcher on the
    /// clip's map at the transport's position.
    pub fn sync(&mut self, at: &StretchAt) {
        let pts = || {
            at.map.iter().map(|a| Point { u: a.clip_t * at.spu, s: a.take_sec * at.rate, flags: a.flags })
        };
        let m = self.main;
        if self.on[m] && self.clip == at.clip {
            let same = self.track_gen == at.track_gen || self.st[m].same(at.mode, at.len, pts());
            self.track_gen = at.track_gen;
            if same && self.sample[m] == at.region {
                if at.ramping {
                    return;
                }
                // Same map: where the transport says the pass started against where the
                // stretcher has it.
                let (pos, ps, len) = self.st[m].phase();
                let want = pos as f64 - at.u;
                let k = ((want - ps) / len).round();
                if (want - (ps + k * len)).abs() < 0.5 {
                    return;
                }
            }
        }
        // Start the other stretcher here, and fade this one out if it plays.
        let fresh = !self.on[m];
        let same_clip = self.clip == at.clip;
        let n = 1 - m;
        if !self.st[n].load(at.mode, at.len, pts()) {
            self.on[n] = false;
            if self.on[m] {
                self.main = n;
                self.fade_old = 0;
            }
            self.clip = at.clip;
            self.track_gen = at.track_gen;
            return;
        }
        self.st[n].seek(at.u.rem_euclid(at.len));
        self.on[n] = true;
        self.sample[n] = at.region;
        self.main = n;
        self.fade_old = if fresh { i64::MAX } else { 0 };
        // A map change or a jump in the same clip crossfades; a new clip starts at full level.
        self.fade_new = if !fresh && same_clip { 0 } else { i64::MAX };
        self.clip = at.clip;
        self.track_gen = at.track_gen;
    }

    /// Write the next `out.len()` samples into `out`.
    pub fn process(&mut self, out: &mut [f32], pool: &crate::samples::SamplePool) {
        out.fill(0.0);
        let (m, o) = (self.main, 1 - self.main);
        let fade = self.st[m].pre().max(1);
        if self.on[o] {
            let start = self.fade_old;
            let src = pool.data(self.sample[o]);
            self.st[o].process(src, out, |i| {
                let x = start + i as i64;
                if x < fade { fade_out(x, fade) } else { 0.0 }
            });
            self.fade_old = start + out.len() as i64;
            if self.fade_old >= fade {
                self.on[o] = false;
            }
        }
        if self.on[m] {
            let start = self.fade_new;
            let src = pool.data(self.sample[m]);
            self.st[m].process(src, out, |i| {
                let x = start.saturating_add(i as i64);
                if x < fade { fade_in(x, fade) } else { 1.0 }
            });
            self.fade_new = start.saturating_add(out.len() as i64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize) -> Vec<f32> {
        let mut x = 12345u32;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    fn run(mode: u32, src: &[f32], pts: &[Point], len: f64, u0: f64, n: usize) -> Vec<f32> {
        let mut s = Stretcher::new(48_000.0);
        assert!(s.load(mode, len, pts.iter().copied()));
        s.seek(u0);
        let mut out = vec![0.0; n];
        s.process(src, &mut out, |_| 1.0);
        out
    }

    #[test]
    fn an_identity_map_gives_back_the_take() {
        // One run over the whole pass, a hard point every 0.25 s, no stretching.
        let src = noise(48_000);
        let mut pts: Vec<Point> = (0..=4).map(|k| Point { u: k as f64 * 12_000.0, s: k as f64 * 12_000.0, flags: POINT_HARD }).collect();
        pts[0].flags |= POINT_RUN;
        for mode in [MODE_HITS, MODE_SMOOTH] {
            let out = run(mode, &src, &pts, 48_000.0, 0.0, 47_000);
            let err = out.iter().zip(&src).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(err < 1e-3, "mode {mode}: {err}");
        }
    }

    #[test]
    fn attacks_land_where_the_map_puts_them() {
        // Clicks every 0.25 s in the take, moved 10 ms later or earlier by the map.
        let mut src = vec![0.0f32; 48_000];
        for k in 0..4 {
            src[k * 12_000 + 600] = 1.0;
        }
        let moves = [0.0, 480.0, -480.0, 240.0, 0.0];
        let mut pts: Vec<Point> = (0..=4).map(|k| Point { u: k as f64 * 12_000.0 + 600.0 + moves[k], s: k as f64 * 12_000.0 + 600.0, flags: POINT_HARD }).collect();
        pts[0] = Point { u: 0.0, s: 0.0, flags: POINT_HARD | POINT_RUN };
        pts[4] = Point { u: 48_000.0, s: 48_000.0, flags: POINT_HARD };
        for mode in [MODE_HITS, MODE_SMOOTH] {
            let out = run(mode, &src, &pts, 48_000.0, 0.0, 48_000);
            for k in 1..4 {
                let want = (k as f64 * 12_000.0 + 600.0 + moves[k]) as usize;
                assert!((out[want] - 1.0).abs() < 1e-3, "mode {mode} click {k}: {}", out[want]);
            }
        }
    }

    #[test]
    fn the_loop_wraps_and_calls_do_not_matter() {
        let src = noise(30_000);
        let pts = [
            Point { u: 0.0, s: 10_000.0, flags: POINT_HARD | POINT_RUN },
            Point { u: 9_000.0, s: 20_000.0, flags: POINT_HARD },
            Point { u: 12_000.0, s: 0.0, flags: POINT_HARD | POINT_RUN },
            Point { u: 15_000.0, s: 4_000.0, flags: 0 },
            Point { u: 24_000.0, s: 10_000.0, flags: POINT_HARD },
        ];
        for mode in [MODE_HITS, MODE_SMOOTH] {
            let whole = run(mode, &src, &pts, 24_000.0, 0.0, 100_000);
            let mut s = Stretcher::new(48_000.0);
            s.load(mode, 24_000.0, pts.iter().copied());
            s.seek(0.0);
            let mut out = vec![0.0; 100_000];
            let mut i = 0;
            let mut k = 1;
            while i < out.len() {
                let n = (k * 37 % 333 + 1).min(out.len() - i);
                s.process(&src, &mut out[i..i + n], |_| 1.0);
                i += n;
                k += 1;
            }
            assert!(whole.iter().zip(&out).all(|(a, b)| a.to_bits() == b.to_bits()), "mode {mode}");
            // The second pass repeats the first, from where the first stretch has its pre-roll.
            let p = 24_000;
            assert!((300..20_000).all(|i| whole[i].to_bits() == whole[i + p].to_bits() || mode == MODE_SMOOTH && (whole[i] - whole[i + p]).abs() < 1e-5));
            let peak = whole.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(whole.iter().all(|v| v.is_finite()) && peak < 3.0, "mode {mode}: peak {peak}");
        }
    }
}
