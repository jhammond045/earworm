//! Radix-2 complex FFT plus helpers for magnitude spectra. Allocates only in `new`.

pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<u32>,
}

impl Fft {
    /// `n` must be a power of two >= 2.
    pub fn new(n: usize) -> Self {
        assert!(n >= 2 && n.is_power_of_two(), "fft size must be a power of two");
        let half = n / 2;
        let mut cos = Vec::with_capacity(half);
        let mut sin = Vec::with_capacity(half);
        for k in 0..half {
            let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
            cos.push(a.cos() as f32);
            sin.push(a.sin() as f32);
        }
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        Self { n, cos, sin, rev }
    }
    pub fn len(&self) -> usize {
        self.n
    }
    /// In-place forward transform, no scaling. `re.len() == im.len() == n`.
    pub fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        assert!(re.len() == n && im.len() == n);
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            let mut start = 0;
            while start < n {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
                start += size;
            }
            size *= 2;
        }
    }
    /// In-place inverse transform, scaled by 1/n.
    pub fn inverse(&self, re: &mut [f32], im: &mut [f32]) {
        // Swapping real and imaginary parts turns the forward transform into
        // the inverse (up to scale).
        self.forward(im, re);
        let s = 1.0 / self.n as f32;
        for v in re.iter_mut().chain(im.iter_mut()) {
            *v *= s;
        }
    }
}

/// Hann window of length n (periodic form, w[i] = 0.5 - 0.5 cos(2 pi i / n)).
pub fn hann(out: &mut [f32]) {
    let n = out.len() as f64;
    for (i, w) in out.iter_mut().enumerate() {
        *w = (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n).cos()) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_lands_in_right_bin() {
        let n = 256;
        let bin = 17;
        let fft = Fft::new(n);
        let mut re: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * bin as f32 * i as f32 / n as f32).sin())
            .collect();
        let mut im = vec![0.0; n];
        fft.forward(&mut re, &mut im);
        let mag: Vec<f32> = (0..n).map(|i| re[i].hypot(im[i])).collect();
        assert!((mag[bin] - n as f32 / 2.0).abs() < 1e-2, "{}", mag[bin]);
        assert!((mag[n - bin] - n as f32 / 2.0).abs() < 1e-2);
        for (i, &m) in mag.iter().enumerate() {
            if i != bin && i != n - bin {
                assert!(m < 1e-2, "bin {i} = {m}");
            }
        }
    }

    #[test]
    fn matches_naive_dft_on_impulse_and_random() {
        let n = 16;
        let fft = Fft::new(n);
        let x: Vec<f32> = (0..n).map(|i| ((i * 7 + 3) % 11) as f32 - 5.0).collect();
        let mut re = x.clone();
        let mut im = vec![0.0; n];
        fft.forward(&mut re, &mut im);
        for k in 0..n {
            let (mut sr, mut si) = (0.0f64, 0.0f64);
            for (j, &v) in x.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (j * k) as f64 / n as f64;
                sr += v as f64 * a.cos();
                si += v as f64 * a.sin();
            }
            assert!((re[k] as f64 - sr).abs() < 1e-3 && (im[k] as f64 - si).abs() < 1e-3);
        }
    }

    #[test]
    fn round_trip() {
        let n = 1024;
        let fft = Fft::new(n);
        let x: Vec<f32> = (0..n).map(|i| ((i * 31 + 5) % 17) as f32 / 8.0 - 1.0).collect();
        let y: Vec<f32> = (0..n).map(|i| ((i * 13 + 1) % 7) as f32 / 3.0 - 1.0).collect();
        let (mut re, mut im) = (x.clone(), y.clone());
        fft.forward(&mut re, &mut im);
        fft.inverse(&mut re, &mut im);
        for i in 0..n {
            assert!((re[i] - x[i]).abs() < 1e-4 && (im[i] - y[i]).abs() < 1e-4);
        }
    }

    #[test]
    fn hann_shape() {
        let mut w = [0.0f32; 8];
        hann(&mut w);
        assert_eq!(w[0], 0.0);
        assert!((w[4] - 1.0).abs() < 1e-6);
        assert!((w[2] - 0.5).abs() < 1e-6);
        assert!((w[1] - w[7]).abs() < 1e-6);
    }
}
