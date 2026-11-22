//! Basic Pitch's network (Bittner et al., "A Lightweight Instrument-Agnostic Model for Polyphonic
//! Note Transcription and Multipitch Estimation", ICASSP 2022; Spotify, Apache 2.0), ported from the
//! ONNX graph the `basic-pitch` Python package ships, with its weights (`basic_pitch.bin`, written
//! by tools/basic-pitch/export.py). The license is in `basic_pitch.LICENSE` beside the weights.
//!
//! The model reads 22050 Hz audio in windows of `WIN` samples and gives, per window, `FRAMES`
//! frames (256 samples apart) of three outputs: note activations (88 semitones, A0 to C8), onset
//! activations (88) and pitch contours (264 bins, a third of a semitone apart). `Stream` cuts a take
//! into windows as the Python code does (`inference.py`: 7680 samples of zeros first, windows
//! `WIN_HOP` apart, 15 frames dropped at each end of every window) and keeps the outputs.
//!
//! Stages, as in the graph: a constant-Q transform (CQT2010v2 from nnAudio: one octave's 36 complex
//! 256-tap kernels, the signal low-passed and halved for each lower octave, 9 octaves, 309 bins from
//! 27.5 Hz kept), a log scale normalized to 0..1 over the window, a batch norm, harmonic stacking
//! (the spectrum shifted to put harmonics 0.5, 1, 2 .. 7 of each bin on top of it, 8 channels of 264
//! bins), then six small convolutions with ReLUs and sigmoids. Everything is f32, computed in a fixed
//! order, so the same input gives the same output bit for bit on the same build.

/// Sample rate the model reads.
pub const SR: u32 = 22050;
/// Samples between frames.
pub const HOP: usize = 256;
/// Samples per window (2 s minus one hop).
pub const WIN: usize = 2 * SR as usize - HOP;
/// Frames per window.
pub const FRAMES: usize = WIN / HOP + 1;
/// Frames shared by neighbouring windows; half are dropped at each end.
pub const OVERLAP_FRAMES: usize = 30;
/// Samples shared by neighbouring windows.
pub const OVERLAP: usize = OVERLAP_FRAMES * HOP;
/// Samples between window starts.
pub const WIN_HOP: usize = WIN - OVERLAP;
/// Frames each window adds.
pub const KEEP: usize = FRAMES - OVERLAP_FRAMES;
pub const N_NOTES: usize = 88;
pub const N_CONTOUR: usize = 264;
/// MIDI note of semitone 0.
pub const MIDI_OFFSET: i32 = 21;

const BPO: usize = 36;
const N_OCT: usize = 9;
const N_CQT: usize = 309;
const KLEN: usize = 256;
const SHIFTS: [isize; 8] = [-36, 0, 36, 57, 72, 84, 93, 101];
const N_HARM: usize = SHIFTS.len();

/// One convolution: weights [out][in][kt][kf], bias [out]; "same" padding as the graph has it.
struct Conv {
    cout: usize,
    cin: usize,
    kt: usize,
    kf: usize,
    /// Weights reordered [in][kt][kf][out] so a block of outputs reads them together.
    w: Vec<f32>,
    b: Vec<f32>,
}

impl Conv {
    fn new(r: &mut Reader, cout: usize, cin: usize, kt: usize, kf: usize) -> Self {
        let raw = r.take(cout * cin * kt * kf);
        let mut w = vec![0.0; raw.len()];
        for o in 0..cout {
            for c in 0..cin {
                for i in 0..kt {
                    for j in 0..kf {
                        w[((c * kt + i) * kf + j) * cout + o] = raw[((o * cin + c) * kt + i) * kf + j];
                    }
                }
            }
        }
        Conv { cout, cin, kt, kf, w, b: r.take(cout) }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Vec<f32> {
        let s = &self.bytes[self.at..self.at + 4 * n];
        self.at += 4 * n;
        s.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }
}

static WEIGHTS: &[u8] = include_bytes!("basic_pitch.bin");

/// The network's weights and its scratch buffers (about 6 MB), made once per take.
pub struct Model {
    kre: Vec<f32>,
    kim: Vec<f32>,
    lowpass: Vec<f32>,
    scale: Vec<f32>,
    bn: [f32; 2],
    onset1: Conv,
    contour1: Conv,
    contour2: Conv,
    note1: Conv,
    note2: Conv,
    onset2: Conv,
    // Scratch.
    cqt: Vec<f32>,
    /// The last window's CQT power in dB, frames x bins, before it was normalized.
    db: Vec<f32>,
    sig: Vec<f32>,
    sig2: Vec<f32>,
    pad: Vec<f32>,
    hs: Vec<f32>,
    a: Vec<f32>,
    b: Vec<f32>,
    c: Vec<f32>,
    d: Vec<f32>,
}

/// One window's outputs, frames x bins, row-major.
pub struct Outputs {
    pub note: Vec<f32>,
    pub onset: Vec<f32>,
    pub contour: Vec<f32>,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    pub fn new() -> Self {
        assert_eq!(&WEIGHTS[..4], b"BPW1", "basic_pitch.bin: bad magic");
        let mut r = Reader { bytes: WEIGHTS, at: 4 };
        let kre = r.take(BPO * KLEN);
        let kim = r.take(BPO * KLEN);
        let lowpass = r.take(KLEN);
        let scale = r.take(N_CQT);
        let bn = r.take(2);
        let onset1 = Conv::new(&mut r, 32, 8, 5, 5);
        let contour1 = Conv::new(&mut r, 8, 8, 3, 39);
        let contour2 = Conv::new(&mut r, 1, 8, 5, 5);
        let note1 = Conv::new(&mut r, 32, 1, 7, 7);
        let note2 = Conv::new(&mut r, 1, 32, 7, 3);
        let onset2 = Conv::new(&mut r, 1, 33, 3, 3);
        assert_eq!(r.at, WEIGHTS.len(), "basic_pitch.bin: wrong size");
        Model {
            kre,
            kim,
            lowpass,
            scale,
            bn: [bn[0], bn[1]],
            onset1,
            contour1,
            contour2,
            note1,
            note2,
            onset2,
            cqt: vec![0.0; FRAMES * N_CQT],
            db: vec![0.0; FRAMES * N_CQT],
            sig: Vec::with_capacity(WIN + 2 * KLEN),
            sig2: Vec::with_capacity(WIN + 2 * KLEN),
            pad: Vec::with_capacity(WIN + 2 * KLEN),
            hs: Vec::new(),
            a: Vec::new(),
            b: Vec::new(),
            c: Vec::new(),
            d: Vec::new(),
        }
    }

    /// Run one window of `WIN` samples. The outputs are written into `out` (resized as needed).
    pub fn run(&mut self, x: &[f32], out: &mut Outputs) {
        assert_eq!(x.len(), WIN);
        self.cqt_window(x);
        self.normalize();
        self.stack();
        let (t, f) = (FRAMES, N_CONTOUR);
        // Contours: 8 -> 8 (3 x 39), ReLU; 8 -> 1 (5 x 5), sigmoid.
        conv2d(&self.contour1, &self.hs, t, f, 1, 1, 19, &mut self.a, &mut self.pad);
        relu(&mut self.a);
        conv2d(&self.contour2, &self.a, t, f, 1, 2, 2, &mut self.b, &mut self.pad);
        sigmoid(&mut self.b);
        out.contour.clear();
        out.contour.extend_from_slice(&self.b);
        // Notes: 1 -> 32 (7 x 7, every third bin), ReLU; 32 -> 1 (7 x 3), sigmoid.
        conv2d(&self.note1, &self.b, t, f, 3, 3, 2, &mut self.c, &mut self.pad);
        relu(&mut self.c);
        let nf = N_NOTES;
        conv2d(&self.note2, &self.c, t, nf, 1, 3, 1, &mut self.d, &mut self.pad);
        sigmoid(&mut self.d);
        out.note.clear();
        out.note.extend_from_slice(&self.d);
        // Onsets: 8 -> 32 (5 x 5, every third bin), ReLU; [notes, those 32] -> 1 (3 x 3), sigmoid.
        conv2d(&self.onset1, &self.hs, t, f, 3, 2, 1, &mut self.a, &mut self.pad);
        relu(&mut self.a);
        self.c.clear();
        self.c.extend_from_slice(&self.d);
        self.c.extend_from_slice(&self.a);
        conv2d(&self.onset2, &self.c, t, nf, 1, 1, 1, &mut self.b, &mut self.pad);
        sigmoid(&mut self.b);
        out.onset.clear();
        out.onset.extend_from_slice(&self.b);
    }

    /// The CQT power (dB) of frame t of the last window, 309 bins from 27.5 Hz, a third of a
    /// semitone apart.
    pub fn cqt_db(&self, t: usize) -> &[f32] {
        &self.db[t * N_CQT..(t + 1) * N_CQT]
    }

    /// The CQT magnitudes of the window, frames x 309 bins (lowest first), into `self.cqt`, already
    /// scaled as the graph does (each bin by the square root of its kernel length).
    fn cqt_window(&mut self, x: &[f32]) {
        self.sig.clear();
        self.sig.extend_from_slice(x);
        let mut re = vec![0.0f32; N_OCT * BPO * FRAMES];
        let mut im = vec![0.0f32; N_OCT * BPO * FRAMES];
        for o in 0..N_OCT {
            if o > 0 {
                downsample(&self.sig, &self.lowpass, &mut self.sig2);
                std::mem::swap(&mut self.sig, &mut self.sig2);
            }
            let hop = HOP >> o;
            let n = self.sig.len();
            assert_eq!(n / hop + 1, FRAMES, "octave {o}: {n} samples");
            // Reflect padding of 128 at both ends.
            self.pad.clear();
            self.pad.extend((1..=KLEN / 2).rev().map(|i| self.sig[i]));
            self.pad.extend_from_slice(&self.sig);
            self.pad.extend((n - 1 - KLEN / 2..n - 1).rev().map(|i| self.sig[i]));
            // Bins of octave o (0 = top) sit at (N_OCT - 1 - o) * 36 in the stack, lowest first.
            let base = (N_OCT - 1 - o) * BPO;
            for t in 0..FRAMES {
                let seg = &self.pad[t * hop..t * hop + KLEN];
                for k in 0..BPO {
                    re[(base + k) * FRAMES + t] = dot(&self.kre[k * KLEN..(k + 1) * KLEN], seg);
                    im[(base + k) * FRAMES + t] = dot(&self.kim[k * KLEN..(k + 1) * KLEN], seg);
                }
            }
        }
        let skip = N_OCT * BPO - N_CQT;
        for b in 0..N_CQT {
            let s = self.scale[b];
            for t in 0..FRAMES {
                let r = re[(b + skip) * FRAMES + t] * s;
                let i = im[(b + skip) * FRAMES + t] * s;
                self.cqt[t * N_CQT + b] = (r * r + i * i).sqrt();
            }
        }
    }

    /// Log power, normalized to 0..1 over the window, then the batch norm.
    fn normalize(&mut self) {
        const LOG10_E: f32 = 0.434_294_46;
        let mut lo = f32::INFINITY;
        for v in self.cqt.iter_mut() {
            *v = ((*v * *v + 1e-10).ln() * LOG10_E) * 10.0;
            lo = lo.min(*v);
        }
        self.db.copy_from_slice(&self.cqt);
        let mut hi = f32::NEG_INFINITY;
        for v in self.cqt.iter_mut() {
            *v -= lo;
            hi = hi.max(*v);
        }
        for v in self.cqt.iter_mut() {
            let x = if hi == 0.0 { 0.0 } else { *v / hi };
            *v = x * self.bn[0] + self.bn[1];
        }
    }

    /// Harmonic stacking: channel h, bin f reads CQT bin f + SHIFTS[h] (zero outside), [h][t][f].
    fn stack(&mut self) {
        self.hs.clear();
        self.hs.resize(N_HARM * FRAMES * N_CONTOUR, 0.0);
        for (h, &s) in SHIFTS.iter().enumerate() {
            for t in 0..FRAMES {
                let row = &mut self.hs[(h * FRAMES + t) * N_CONTOUR..(h * FRAMES + t + 1) * N_CONTOUR];
                for (f, v) in row.iter_mut().enumerate() {
                    let src = f as isize + s;
                    if (0..N_CQT as isize).contains(&src) {
                        *v = self.cqt[t * N_CQT + src as usize];
                    }
                }
            }
        }
    }
}

/// Four f32 lanes: wasm SIMD in the browser, plain arrays natively (which the compiler vectorizes
/// itself). Each lane adds its own products in the same order either way, with no fused
/// multiply-add, so the wasm build and the native one compute the same numbers.
#[cfg(target_arch = "wasm32")]
mod f4 {
    use core::arch::wasm32::{f32x4_add, f32x4_extract_lane, f32x4_mul, f32x4_splat, v128};

    #[derive(Clone, Copy)]
    pub struct F4(v128);

    impl F4 {
        #[inline(always)]
        pub fn splat(x: f32) -> Self {
            F4(f32x4_splat(x))
        }
        #[inline(always)]
        pub fn load(s: &[f32]) -> Self {
            let s = &s[..4];
            // SAFETY: four readable f32s; wasm loads need no alignment.
            F4(unsafe { core::ptr::read_unaligned(s.as_ptr() as *const v128) })
        }
        /// self + a * b
        #[inline(always)]
        pub fn mul_add(self, a: F4, b: F4) -> Self {
            F4(f32x4_add(self.0, f32x4_mul(a.0, b.0)))
        }
        #[inline(always)]
        pub fn store(self, s: &mut [f32]) {
            let s = &mut s[..4];
            // SAFETY: four writable f32s.
            unsafe { core::ptr::write_unaligned(s.as_mut_ptr() as *mut v128, self.0) }
        }
        #[inline(always)]
        pub fn lanes(self) -> [f32; 4] {
            [f32x4_extract_lane::<0>(self.0), f32x4_extract_lane::<1>(self.0), f32x4_extract_lane::<2>(self.0), f32x4_extract_lane::<3>(self.0)]
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod f4 {
    #[derive(Clone, Copy)]
    pub struct F4([f32; 4]);

    impl F4 {
        #[inline(always)]
        pub fn splat(x: f32) -> Self {
            F4([x; 4])
        }
        #[inline(always)]
        pub fn load(s: &[f32]) -> Self {
            F4([s[0], s[1], s[2], s[3]])
        }
        /// self + a * b
        #[inline(always)]
        pub fn mul_add(self, a: F4, b: F4) -> Self {
            F4(std::array::from_fn(|l| self.0[l] + a.0[l] * b.0[l]))
        }
        #[inline(always)]
        pub fn store(self, s: &mut [f32]) {
            s[..4].copy_from_slice(&self.0);
        }
        #[inline(always)]
        pub fn lanes(self) -> [f32; 4] {
            self.0
        }
    }
}

use f4::F4;

/// Dot product of two equal slices (a multiple of 8 long), in eight running sums.
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let (mut s0, mut s1) = (F4::splat(0.0), F4::splat(0.0));
    for (x, y) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        s0 = s0.mul_add(F4::load(&x[..4]), F4::load(&y[..4]));
        s1 = s1.mul_add(F4::load(&x[4..]), F4::load(&y[4..]));
    }
    let (p, q) = (s0.lanes(), s1.lanes());
    ((p[0] + p[1]) + (p[2] + p[3])) + ((q[0] + q[1]) + (q[2] + q[3]))
}

/// Low-pass and keep every second sample (nnAudio `downsampling_by_n`: 127 zeros either side).
fn downsample(x: &[f32], h: &[f32], out: &mut Vec<f32>) {
    let pad = (KLEN - 1) / 2;
    let mut p = vec![0.0f32; x.len() + 2 * pad + KLEN];
    p[pad..pad + x.len()].copy_from_slice(x);
    let n = (x.len() + 2 * pad - KLEN) / 2 + 1;
    out.clear();
    out.extend((0..n).map(|i| dot(h, &p[2 * i..2 * i + KLEN])));
}

fn relu(x: &mut [f32]) {
    for v in x {
        *v = v.max(0.0);
    }
}

fn sigmoid(x: &mut [f32]) {
    for v in x {
        *v = 1.0 / (1.0 + (-*v).exp());
    }
}

/// Output positions per block, and output channels per block.
const XB: usize = 8;
const OB: usize = 8;

/// A 2-D convolution over [cin][t][fin] into `out` [cout][t][fout], fout = ceil(fin / stride), with
/// `pt` frames of zeros before and after in time and `pf` bins of zeros before in frequency (the
/// graph's explicit pads; whatever the kernel reaches past the end is zero too). For a stride of 3,
/// each input row is split into its three phases first, so every kernel tap reads a run of
/// consecutive values. Each output adds its bias, then its taps in (in, kt, kf) order.
#[allow(clippy::too_many_arguments)]
fn conv2d(cv: &Conv, input: &[f32], t: usize, fin: usize, stride: usize, pt: usize, pf: usize, out: &mut Vec<f32>, pad: &mut Vec<f32>) {
    assert_eq!(input.len(), cv.cin * t * fin);
    let fout = fin.div_ceil(stride);
    // Padded input: per channel, (t + kt - 1) rows; per row, `stride` phases of `pw` values.
    let rows = t + cv.kt - 1;
    let pw = fout + cv.kf.div_ceil(stride) + XB;
    let plane = rows * stride * pw;
    pad.clear();
    pad.resize(cv.cin * plane, 0.0);
    for c in 0..cv.cin {
        for y in 0..t {
            let src = &input[(c * t + y) * fin..(c * t + y + 1) * fin];
            let row = &mut pad[c * plane + (y + pt) * stride * pw..][..stride * pw];
            for (f, &v) in src.iter().enumerate() {
                let p = f + pf; // position in the zero-padded row
                row[(p % stride) * pw + p / stride] = v;
            }
        }
    }
    out.clear();
    out.resize(cv.cout * t * fout, 0.0);
    let g = Geometry { plane, stride, pw, t, fout };
    if cv.cout % OB == 0 {
        for ob in (0..cv.cout).step_by(OB) {
            conv_block::<OB>(cv, pad, &g, ob, out);
        }
    } else {
        assert_eq!(cv.cout, 1);
        conv_block::<1>(cv, pad, &g, 0, out);
    }
}

struct Geometry {
    plane: usize,
    stride: usize,
    pw: usize,
    t: usize,
    fout: usize,
}

/// Output channels ob..ob + ON of a convolution, XB positions at a time.
#[inline(always)]
fn conv_block<const ON: usize>(cv: &Conv, pad: &[f32], g: &Geometry, ob: usize, out: &mut [f32]) {
    let k = cv.kt * cv.kf;
    let rw = g.stride * g.pw;
    for y in 0..g.t {
        for xb in (0..g.fout).step_by(XB) {
            let mut a0 = [F4::splat(0.0); ON];
            let mut a1 = [F4::splat(0.0); ON];
            for o in 0..ON {
                a0[o] = F4::splat(cv.b[ob + o]);
                a1[o] = a0[o];
            }
            for c in 0..cv.cin {
                for i in 0..cv.kt {
                    let row = &pad[c * g.plane + (y + i) * rw..][..rw];
                    let wrow = &cv.w[(c * k + i * cv.kf) * cv.cout..][..cv.kf * cv.cout];
                    for j in 0..cv.kf {
                        let src = &row[(j % g.stride) * g.pw + xb + j / g.stride..][..XB];
                        let (s0, s1) = (F4::load(&src[..4]), F4::load(&src[4..]));
                        let w = &wrow[j * cv.cout + ob..][..ON];
                        for o in 0..ON {
                            let wv = F4::splat(w[o]);
                            a0[o] = a0[o].mul_add(wv, s0);
                            a1[o] = a1[o].mul_add(wv, s1);
                        }
                    }
                }
            }
            let n = XB.min(g.fout - xb);
            for o in 0..ON {
                let mut v = [0.0f32; XB];
                a0[o].store(&mut v[..4]);
                a1[o].store(&mut v[4..]);
                out[((ob + o) * g.t + y) * g.fout + xb..][..n].copy_from_slice(&v[..n]);
            }
        }
    }
}
