//! RNNoise (via the pure-Rust `nnnoiseless` port) with a dry/wet mix.
//! RNNoise's overlap-add output lags its input by exactly one frame, so the
//! dry path is delayed by one frame to stay phase-aligned when mixing.
//! The network runs even when "off" so toggling never changes latency.

use crate::params::FRAME;
use nnnoiseless::DenoiseState;

pub struct Denoiser {
    st: Box<DenoiseState<'static>>,
    inp: [f32; FRAME],
    out: [f32; FRAME],
    dry: [f32; FRAME],
    first: bool,
}

impl Denoiser {
    pub fn new() -> Self {
        Self {
            st: DenoiseState::new(),
            inp: [0.0; FRAME],
            out: [0.0; FRAME],
            dry: [0.0; FRAME],
            first: true,
        }
    }

    /// `buf` must be exactly `FRAME` samples in [-1, 1]. Returns voice-activity probability (0..1).
    pub fn process(&mut self, buf: &mut [f32], strength: f32) -> f32 {
        for (d, s) in self.inp.iter_mut().zip(buf.iter()) {
            *d = *s * 32768.0;
        }
        let vad = self.st.process_frame(&mut self.out, &self.inp);
        if self.first {
            // First output frame contains start-up artefacts; discard it.
            self.out = [0.0; FRAME];
            self.first = false;
        }
        let s = strength.clamp(0.0, 1.0);
        #[allow(clippy::needless_range_loop)] // indexes three parallel buffers
        for i in 0..FRAME {
            let wet = self.out[i] * (1.0 / 32768.0);
            let y = self.dry[i] * (1.0 - s) + wet * s;
            self.dry[i] = buf[i];
            buf[i] = y;
        }
        vad
    }
}
