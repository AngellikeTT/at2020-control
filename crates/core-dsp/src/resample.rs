//! Streaming cubic (Catmull-Rom) resampler usable in both push and pull modes,
//! with a variable step so the output side can compensate clock drift.

pub struct Resampler {
    s: [f32; 4],
    mu: f64,
}

impl Resampler {
    pub fn new() -> Self {
        Self {
            s: [0.0; 4],
            mu: 1.0,
        }
    }

    #[inline]
    fn interp(&self) -> f32 {
        let [s0, s1, s2, s3] = self.s;
        let t = self.mu as f32;
        let c1 = 0.5 * (s2 - s0);
        let c2 = s0 - 2.5 * s1 + 2.0 * s2 - 0.5 * s3;
        let c3 = 0.5 * (s3 - s0) + 1.5 * (s1 - s2);
        ((c3 * t + c2) * t + c1) * t + s1
    }

    /// `step` = input samples consumed per output sample.
    /// Runs until `next` runs dry or `sink` returns false.
    pub fn run(
        &mut self,
        step: f64,
        next: &mut dyn FnMut() -> Option<f32>,
        sink: &mut dyn FnMut(f32) -> bool,
    ) {
        loop {
            while self.mu >= 1.0 {
                match next() {
                    Some(x) => {
                        self.s = [self.s[1], self.s[2], self.s[3], x];
                        self.mu -= 1.0;
                    }
                    None => return,
                }
            }
            let y = self.interp();
            self.mu += step;
            if !sink(y) {
                return;
            }
        }
    }
}

impl Default for Resampler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, f: f64, fs: f64) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / fs).sin() as f32)
            .collect()
    }

    #[test]
    fn upsample_44k1_to_48k_keeps_pitch_and_level() {
        let input = sine(44_100, 1000.0, 44_100.0);
        let mut rs = Resampler::new();
        let mut it = input.iter().copied();
        let mut out = Vec::new();
        rs.run(44_100.0 / 48_000.0, &mut || it.next(), &mut |y| {
            out.push(y);
            true
        });
        assert!((out.len() as i64 - 48_000).abs() < 8, "len {}", out.len());
        // Least-squares fit of a 1 kHz sinusoid (phase-agnostic): amplitude must be ~1, residual tiny.
        let seg = &out[200..4000];
        let w = 2.0 * std::f64::consts::PI * 1000.0 / 48_000.0;
        let (mut a, mut b) = (0.0f64, 0.0f64);
        for (i, v) in seg.iter().enumerate() {
            let t = (i + 200) as f64 * w;
            a += *v as f64 * t.sin();
            b += *v as f64 * t.cos();
        }
        a *= 2.0 / seg.len() as f64;
        b *= 2.0 / seg.len() as f64;
        let amp = (a * a + b * b).sqrt();
        assert!((amp - 1.0).abs() < 0.01, "amp {amp}");
        let resid: f64 = seg
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let t = (i + 200) as f64 * w;
                (*v as f64 - (a * t.sin() + b * t.cos())).powi(2)
            })
            .sum::<f64>()
            / seg.len() as f64;
        assert!(resid.sqrt() < 0.01, "resid {}", resid.sqrt());
    }

    #[test]
    fn identity_ratio_is_pure_delay() {
        let input = sine(2000, 440.0, 48_000.0);
        let mut rs = Resampler::new();
        let mut it = input.iter().copied();
        let mut out = Vec::new();
        rs.run(1.0, &mut || it.next(), &mut |y| {
            out.push(y);
            true
        });
        let e: f32 = (10..1900)
            .map(|i| {
                (0..4)
                    .map(|d| (out[i] - input[i - d.min(i)]).abs())
                    .fold(f32::MAX, f32::min)
            })
            .fold(0.0, f32::max);
        assert!(e < 1e-5, "{e}");
    }

    #[test]
    fn pull_mode_stops_when_full() {
        let mut rs = Resampler::new();
        let mut c = 0.0f32;
        let mut out = [0.0f32; 16];
        let mut n = 0;
        rs.run(
            1.0,
            &mut || {
                c += 1.0;
                Some(c)
            },
            &mut |y| {
                out[n] = y;
                n += 1;
                n < 16
            },
        );
        assert_eq!(n, 16);
    }
}
