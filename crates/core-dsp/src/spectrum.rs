//! Spectrum analyzer: drains the audio thread's tap rings and keeps smoothed
//! magnitude spectra (dBFS) for the pre- and post-chain signals.

use crate::ring::Ring;
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const FFT_N: usize = 2048;
pub const BINS: usize = FFT_N / 2 + 1;
const FLOOR_DB: f32 = -120.0;

struct Channel {
    ring: Arc<Ring>,
    window_buf: Vec<f32>, // newest FFT_N samples (oldest first)
    pub_db: Vec<f32>,
}

pub struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    norm: f32,
    input: Channel,
    output: Channel,
    scratch_in: Vec<f32>,
    scratch_out: Vec<Complex<f32>>,
    fft_scratch: Vec<Complex<f32>>,
    tmp: Vec<f32>,
}

impl Analyzer {
    pub fn new(spec_in: Arc<Ring>, spec_out: Arc<Ring>) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_N);
        let window: Vec<f32> = (0..FFT_N)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_N as f32 - 1.0)).cos()
            })
            .collect();
        let sum: f32 = window.iter().sum();
        let mk = |r: &Arc<Ring>| Channel {
            ring: r.clone(),
            window_buf: vec![0.0; FFT_N],
            pub_db: vec![FLOOR_DB; BINS],
        };
        Self {
            scratch_out: fft.make_output_vec(),
            fft_scratch: fft.make_scratch_vec(),
            fft,
            window,
            norm: 2.0 / sum,
            input: mk(&spec_in),
            output: mk(&spec_out),
            scratch_in: vec![0.0; FFT_N],
            tmp: vec![0.0; 8192],
        }
    }

    /// Pull new samples and refresh both spectra. `dt` is seconds since the last call.
    pub fn update(&mut self, dt: f32) {
        let rel = (-dt / 0.25).exp(); // ~250 ms fall
        for which in 0..2 {
            let ch = if which == 0 {
                &mut self.input
            } else {
                &mut self.output
            };
            let mut got_any = false;
            loop {
                let n = ch.ring.pop(&mut self.tmp);
                if n == 0 {
                    break;
                }
                got_any = true;
                if n >= FFT_N {
                    ch.window_buf.copy_from_slice(&self.tmp[n - FFT_N..n]);
                } else {
                    ch.window_buf.copy_within(n.., 0);
                    ch.window_buf[FFT_N - n..].copy_from_slice(&self.tmp[..n]);
                }
                if n < self.tmp.len() {
                    break;
                }
            }
            if !got_any {
                // No fresh audio: let the display decay.
                for v in ch.pub_db.iter_mut() {
                    *v = (*v - 40.0 * dt).max(FLOOR_DB);
                }
                continue;
            }
            for ((d, s), w) in self
                .scratch_in
                .iter_mut()
                .zip(ch.window_buf.iter())
                .zip(self.window.iter())
            {
                *d = s * w;
            }
            if self
                .fft
                .process_with_scratch(
                    &mut self.scratch_in,
                    &mut self.scratch_out,
                    &mut self.fft_scratch,
                )
                .is_err()
            {
                continue;
            }
            for (db, c) in ch.pub_db.iter_mut().zip(self.scratch_out.iter()) {
                let mag = c.norm() * self.norm;
                let new = 20.0 * mag.max(1e-9).log10();
                *db = if new > *db {
                    new
                } else {
                    new + (*db - new) * rel
                };
            }
        }
    }

    pub fn input_db(&self) -> &[f32] {
        &self.input.pub_db
    }
    pub fn output_db(&self) -> &[f32] {
        &self.output.pub_db
    }
}

/// Bin index to frequency (Hz) at 48 kHz.
pub fn bin_hz(i: usize) -> f32 {
    i as f32 * 48_000.0 / FFT_N as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peak_lands_in_the_right_bin_at_the_right_level() {
        let ring_in = Arc::new(Ring::new(1 << 14));
        let ring_out = Arc::new(Ring::new(1 << 14));
        let mut a = Analyzer::new(ring_in, ring_out.clone());
        let f = 3000.0f32;
        let amp = 0.25f32; // -12 dBFS
        let x: Vec<f32> = (0..FFT_N * 2)
            .map(|i| amp * (2.0 * std::f32::consts::PI * f * i as f32 / 48_000.0).sin())
            .collect();
        ring_out.push(&x[..FFT_N]);
        ring_out.push(&x[FFT_N..]);
        a.update(0.033);
        let db = a.output_db();
        let (peak_i, peak_v) =
            db.iter().enumerate().fold(
                (0, f32::MIN),
                |m, (i, v)| if *v > m.1 { (i, *v) } else { m },
            );
        assert!((bin_hz(peak_i) - f).abs() < 30.0, "bin {peak_i}");
        assert!((peak_v - (-12.0)).abs() < 1.5, "level {peak_v}");
    }
}
