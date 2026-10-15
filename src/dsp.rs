//! Real-time processing chain at 48 kHz mono, processed in 480-sample frames.
//! Order: input gain -> low-cut -> RNNoise -> gate -> EQ -> compressor
//!        -> output gain -> limiter.
//! Nothing in here allocates or locks after construction.

use crate::denoise::Denoiser;
use crate::params::*;
use std::f64::consts::PI;

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db * (1.0 / 20.0))
}
#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}
#[inline]
fn tc_coef(ms: f32) -> f32 {
    (-1.0 / (ms.max(0.01) * 0.001 * SAMPLE_RATE)).exp()
}

// ---------------------------------------------------------------- biquad ----

#[derive(Clone, Copy, Debug)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coeffs {
    pub fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
    fn norm(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
    fn w(fs: f64, f0: f64) -> (f64, f64) {
        let w0 = 2.0 * PI * (f0.clamp(10.0, fs * 0.49)) / fs;
        (w0.cos(), w0.sin())
    }
    pub fn highpass(fs: f64, f0: f64, q: f64) -> Self {
        let (c, s) = Self::w(fs, f0);
        let al = s / (2.0 * q);
        Self::norm(
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + al,
            -2.0 * c,
            1.0 - al,
        )
    }
    pub fn lowpass(fs: f64, f0: f64, q: f64) -> Self {
        let (c, s) = Self::w(fs, f0);
        let al = s / (2.0 * q);
        Self::norm(
            (1.0 - c) / 2.0,
            1.0 - c,
            (1.0 - c) / 2.0,
            1.0 + al,
            -2.0 * c,
            1.0 - al,
        )
    }
    pub fn peaking(fs: f64, f0: f64, gain_db: f64, q: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (c, s) = Self::w(fs, f0);
        let al = s / (2.0 * q.max(0.05));
        Self::norm(
            1.0 + al * a,
            -2.0 * c,
            1.0 - al * a,
            1.0 + al / a,
            -2.0 * c,
            1.0 - al / a,
        )
    }
    pub fn low_shelf(fs: f64, f0: f64, gain_db: f64, q: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (c, s) = Self::w(fs, f0);
        let al = s / (2.0 * q.max(0.05));
        let sa = 2.0 * a.sqrt() * al;
        Self::norm(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        )
    }
    pub fn high_shelf(fs: f64, f0: f64, gain_db: f64, q: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (c, s) = Self::w(fs, f0);
        let al = s / (2.0 * q.max(0.05));
        let sa = 2.0 * a.sqrt() * al;
        Self::norm(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }

    /// Magnitude response in dB at frequency `f` (Hz).
    pub fn magnitude_db(&self, fs: f64, f: f64) -> f32 {
        let w = 2.0 * PI * f / fs;
        let (c1, s1, c2, s2) = (w.cos(), -w.sin(), (2.0 * w).cos(), -(2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = self.b1 * s1 + self.b2 * s2;
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = self.a1 * s1 + self.a2 * s2;
        let m2 = (nr * nr + ni * ni) / (dr * dr + di * di).max(1e-30);
        (10.0 * m2.max(1e-30).log10()) as f32
    }
}

#[derive(Clone, Copy)]
pub struct Biquad {
    c: Coeffs,
    z1: f64,
    z2: f64,
}
impl Biquad {
    pub fn new(c: Coeffs) -> Self {
        Self {
            c,
            z1: 0.0,
            z2: 0.0,
        }
    }
    pub fn set(&mut self, c: Coeffs) {
        self.c = c;
    }
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.c.b0 * x + self.z1;
        self.z1 = self.c.b1 * x - self.c.a1 * y + self.z2;
        self.z2 = self.c.b2 * x - self.c.a2 * y;
        y
    }
}

pub fn band_coeffs(b: &EqBand, fs: f64) -> Coeffs {
    let (f, g, q) = (b.freq as f64, b.gain_db as f64, b.q as f64);
    match b.kind {
        BandKind::LowShelf => Coeffs::low_shelf(fs, f, g, q),
        BandKind::Peak => Coeffs::peaking(fs, f, g, q),
        BandKind::HighShelf => Coeffs::high_shelf(fs, f, g, q),
    }
}

/// Combined EQ response in dB, for drawing the curve in the UI.
pub fn eq_response_db(bands: &[EqBand; EQ_BANDS], f: f64) -> f32 {
    bands
        .iter()
        .filter(|b| b.enabled)
        .map(|b| band_coeffs(b, SAMPLE_RATE as f64).magnitude_db(SAMPLE_RATE as f64, f))
        .sum()
}

// ------------------------------------------------------------------ gate ----

/// Lookahead so the gate opens *before* the first syllable (constant, always applied).
pub const GATE_LOOKAHEAD: usize = 192; // 4 ms

pub struct Gate {
    delay: [f32; GATE_LOOKAHEAD],
    pos: usize,
    env: f32,
    gain: f32,
    open: bool,
    hold: u32,
}

impl Gate {
    pub fn new() -> Self {
        Self {
            delay: [0.0; GATE_LOOKAHEAD],
            pos: 0,
            env: 0.0,
            gain: 1.0,
            open: true,
            hold: 0,
        }
    }

    /// Returns whether the gate is open at the end of the block.
    pub fn process(&mut self, buf: &mut [f32], p: &GateParams) -> bool {
        let open_thr = db_to_lin(p.threshold_db);
        let close_thr = db_to_lin(p.threshold_db - p.hysteresis_db.max(0.0));
        let floor = db_to_lin(p.range_db.min(0.0));
        let env_rel = tc_coef(5.0);
        let a_att = 1.0 - tc_coef(p.attack_ms.max(0.1));
        // "Release" = time to fall through the whole 60 dB range (ln(1000) time constants).
        let a_rel = 1.0 - tc_coef(p.release_ms.max(1.0) / 6.9);
        let hold_n = (p.hold_ms.max(0.0) * 0.001 * SAMPLE_RATE) as u32;

        for x in buf.iter_mut() {
            let inp = *x;
            let lvl = inp.abs();
            self.env = lvl.max(self.env * env_rel);
            if !p.enabled {
                self.open = true;
            } else if !self.open {
                if self.env >= open_thr {
                    self.open = true;
                    self.hold = hold_n;
                }
            } else if self.env >= close_thr {
                self.hold = hold_n;
            } else if self.hold > 0 {
                self.hold -= 1;
            } else {
                self.open = false;
            }
            let target = if self.open { 1.0 } else { floor };
            self.gain += (target - self.gain) * if target > self.gain { a_att } else { a_rel };
            let delayed = self.delay[self.pos];
            self.delay[self.pos] = inp;
            self.pos = (self.pos + 1) % GATE_LOOKAHEAD;
            *x = delayed * self.gain;
        }
        self.open
    }
}

// ------------------------------------------------------------------- eq -----

pub struct Eq {
    filters: [Biquad; EQ_BANDS],
    cached: [Option<EqBand>; EQ_BANDS],
}

impl Eq {
    pub fn new() -> Self {
        Self {
            filters: [Biquad::new(Coeffs::identity()); EQ_BANDS],
            cached: [None; EQ_BANDS],
        }
    }
    pub fn process(&mut self, buf: &mut [f32], p: &EqParams) {
        for i in 0..EQ_BANDS {
            let b = p.bands[i];
            if self.cached[i] != Some(b) {
                self.filters[i].set(band_coeffs(&b, SAMPLE_RATE as f64));
                self.cached[i] = Some(b);
            }
            if b.enabled {
                let f = &mut self.filters[i];
                for x in buf.iter_mut() {
                    *x = f.process(*x as f64) as f32;
                }
            }
        }
    }
}

// ----------------------------------------------------------- compressor -----

/// Static gain curve: output level (dB) for an input level (dB), soft knee.
pub fn comp_curve_db(x_db: f32, p: &CompParams) -> f32 {
    let over = x_db - p.threshold_db;
    let k = p.knee_db.max(0.0);
    let inv = 1.0 / p.ratio.max(1.0);
    if 2.0 * over < -k {
        x_db
    } else if 2.0 * over.abs() <= k {
        x_db + (inv - 1.0) * (over + k / 2.0).powi(2) / (2.0 * k.max(1e-6))
    } else {
        p.threshold_db + over * inv
    }
}

pub fn comp_auto_makeup_db(p: &CompParams) -> f32 {
    -p.threshold_db * (1.0 - 1.0 / p.ratio.max(1.0)) * 0.5
}

pub struct Compressor {
    det: f32,
    gr_db: f32,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            det: 0.0,
            gr_db: 0.0,
        }
    }
    pub fn reset(&mut self) {
        self.det = 0.0;
        self.gr_db = 0.0;
    }
    /// Returns the maximum gain reduction (positive dB) applied during the block.
    pub fn process(&mut self, buf: &mut [f32], p: &CompParams) -> f32 {
        let att = tc_coef(p.attack_ms.max(0.1));
        let rel = tc_coef(p.release_ms.max(1.0));
        let det_rel = tc_coef(2.0);
        let makeup = if p.auto_makeup {
            comp_auto_makeup_db(p)
        } else {
            p.makeup_db
        };
        let mut max_gr = 0.0f32;
        for x in buf.iter_mut() {
            self.det = x.abs().max(self.det * det_rel);
            let xdb = lin_to_db(self.det.max(1e-6));
            let target = comp_curve_db(xdb, p) - xdb; // <= 0
            let c = if target < self.gr_db { att } else { rel };
            self.gr_db = c * self.gr_db + (1.0 - c) * target;
            *x *= db_to_lin(self.gr_db + makeup);
            max_gr = max_gr.max(-self.gr_db);
        }
        max_gr
    }
}

// -------------------------------------------------------------- limiter -----

pub struct Limiter {
    g: f32,
}
impl Limiter {
    pub fn new() -> Self {
        Self { g: 1.0 }
    }
    pub fn process(&mut self, buf: &mut [f32], p: &LimiterParams) {
        if !p.enabled {
            for x in buf.iter_mut() {
                *x = x.clamp(-1.0, 1.0);
            }
            self.g = 1.0;
            return;
        }
        let ceil = db_to_lin(p.ceiling_db.min(0.0));
        let rel = 1.0 - tc_coef(p.release_ms.max(1.0));
        for x in buf.iter_mut() {
            let a = x.abs();
            let released = self.g + (1.0 - self.g) * rel;
            self.g = if a * released > ceil {
                ceil / a
            } else {
                released
            };
            *x *= self.g;
        }
    }
}

// ---------------------------------------------------------------- chain -----

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub in_peak: f32,
    pub out_peak: f32,
    pub gr_db: f32,
    pub gate_open: bool,
    pub vad: f32,
}

pub struct Chain {
    hpf: Biquad,
    hpf_freq: f32,
    denoise: Denoiser,
    gate: Gate,
    eq: Eq,
    comp: Compressor,
    lim: Limiter,
}

impl Chain {
    pub fn new() -> Self {
        Self {
            hpf: Biquad::new(Coeffs::highpass(
                SAMPLE_RATE as f64,
                80.0,
                std::f64::consts::FRAC_1_SQRT_2,
            )),
            hpf_freq: 80.0,
            denoise: Denoiser::new(),
            gate: Gate::new(),
            eq: Eq::new(),
            comp: Compressor::new(),
            lim: Limiter::new(),
        }
    }

    /// Fixed algorithmic latency of the chain in samples (denoise frame + gate lookahead).
    pub const LATENCY_SAMPLES: usize = FRAME + GATE_LOOKAHEAD;

    pub fn process(&mut self, buf: &mut [f32], p: &ChainParams) -> FrameStats {
        debug_assert_eq!(buf.len(), FRAME);
        let g_in = db_to_lin(p.input.gain_db) * if p.input.phase_invert { -1.0 } else { 1.0 };
        let g_out = if p.output.mute {
            0.0
        } else {
            db_to_lin(p.output.gain_db)
        };
        let mut st = FrameStats::default();
        for x in buf.iter_mut() {
            *x *= g_in;
            st.in_peak = st.in_peak.max(x.abs());
        }

        if p.bypass {
            for x in buf.iter_mut() {
                *x = (*x * g_out).clamp(-1.0, 1.0);
                st.out_peak = st.out_peak.max(x.abs());
            }
            st.gate_open = true;
            return st;
        }

        if (p.input.lowcut_hz - self.hpf_freq).abs() > f32::EPSILON {
            self.hpf.set(Coeffs::highpass(
                SAMPLE_RATE as f64,
                p.input.lowcut_hz as f64,
                std::f64::consts::FRAC_1_SQRT_2,
            ));
            self.hpf_freq = p.input.lowcut_hz;
        }
        for x in buf.iter_mut() {
            let y = self.hpf.process(*x as f64) as f32; // keep state warm even when off
            if p.input.lowcut_on {
                *x = y;
            }
        }

        st.vad = self.denoise.process(
            buf,
            if p.denoise.enabled {
                p.denoise.strength
            } else {
                0.0
            },
        );
        st.gate_open = self.gate.process(buf, &p.gate);
        if p.eq.enabled {
            self.eq.process(buf, &p.eq);
        }
        if p.comp.enabled {
            st.gr_db = self.comp.process(buf, &p.comp);
        } else {
            self.comp.reset();
        }
        for x in buf.iter_mut() {
            *x *= g_out;
        }
        self.lim.process(buf, &p.limiter);
        for x in buf.iter() {
            st.out_peak = st.out_peak.max(x.abs());
        }
        st
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48_000.0;

    fn sine(n: usize, f: f64, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * f * i as f64 / FS).sin() as f32)
            .collect()
    }
    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn peaking_filter_hits_requested_gain() {
        let c = Coeffs::peaking(FS, 1000.0, 6.0, 1.0);
        assert!((c.magnitude_db(FS, 1000.0) - 6.0).abs() < 0.01);
        assert!(c.magnitude_db(FS, 100.0).abs() < 0.5);
    }

    #[test]
    fn shelves_reach_their_gain() {
        let lo = Coeffs::low_shelf(FS, 200.0, 5.0, 0.7);
        assert!((lo.magnitude_db(FS, 20.0) - 5.0).abs() < 0.3);
        assert!(lo.magnitude_db(FS, 10_000.0).abs() < 0.3);
        let hi = Coeffs::high_shelf(FS, 5000.0, -4.0, 0.7);
        assert!((hi.magnitude_db(FS, 20_000.0) + 4.0).abs() < 0.4);
        assert!(hi.magnitude_db(FS, 100.0).abs() < 0.3);
    }

    #[test]
    fn highpass_is_3db_down_at_cutoff() {
        let c = Coeffs::highpass(FS, 100.0, std::f64::consts::FRAC_1_SQRT_2);
        assert!((c.magnitude_db(FS, 100.0) + 3.01).abs() < 0.1);
        assert!(c.magnitude_db(FS, 1000.0).abs() < 0.1);
    }

    #[test]
    fn measured_response_matches_formula() {
        let c = Coeffs::peaking(FS, 2000.0, 9.0, 2.0);
        let mut bq = Biquad::new(c);
        let x = sine(48_000, 2000.0, 0.1);
        let y: Vec<f32> = x.iter().map(|v| bq.process(*v as f64) as f32).collect();
        let g = 20.0 * (rms(&y[4800..]) / rms(&x[4800..])).log10();
        assert!((g - 9.0).abs() < 0.1, "{g}");
    }

    #[test]
    fn gate_closes_on_quiet_and_opens_on_loud() {
        let mut g = Gate::new();
        let p = GateParams::default(); // -50 dBFS
        let quiet = sine(48_000, 300.0, db_to_lin(-70.0));
        let mut out = quiet.clone();
        for ch in out.chunks_mut(FRAME) {
            g.process(ch, &p);
        }
        assert!(
            rms(&out[24_000..]) < rms(&quiet) * 0.01,
            "quiet not attenuated"
        );
        let loud = sine(24_000, 300.0, 0.3);
        let mut out = loud.clone();
        for ch in out.chunks_mut(FRAME) {
            g.process(ch, &p);
        }
        assert!(
            rms(&out[12_000..]) > rms(&loud[12_000..]) * 0.98,
            "loud not passed"
        );
    }

    #[test]
    fn gate_holds_then_releases() {
        let mut g = Gate::new();
        let p = GateParams {
            hold_ms: 100.0,
            ..GateParams::default()
        };
        let mut loud = sine(4800, 300.0, 0.3);
        g.process(&mut loud, &p);
        let mut quiet = vec![0.0f32; 2400]; // 50 ms of silence < hold
        let open = g.process(&mut quiet, &p);
        assert!(open, "should still be in hold");
        let mut quiet = vec![0.0f32; 24_000];
        assert!(!g.process(&mut quiet, &p), "should have closed");
    }

    #[test]
    fn compressor_matches_static_curve() {
        let p = CompParams {
            threshold_db: -20.0,
            ratio: 4.0,
            knee_db: 0.0,
            makeup_db: 0.0,
            auto_makeup: false,
            attack_ms: 1.0,
            release_ms: 50.0,
            enabled: true,
        };
        // -8 dBFS peak sine: 12 dB over threshold -> 9 dB reduction at 4:1
        let x = sine(48_000, 1000.0, db_to_lin(-8.0));
        let mut y = x.clone();
        let mut c = Compressor::new();
        let mut max_gr = 0.0;
        for ch in y.chunks_mut(FRAME) {
            max_gr = c.process(ch, &p);
        }
        assert!((max_gr - 9.0).abs() < 0.7, "gr {max_gr}");
        let pk = y[40_000..].iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(
            (lin_to_db(pk) - (-17.0)).abs() < 0.8,
            "out {}",
            lin_to_db(pk)
        );
    }

    #[test]
    fn comp_curve_is_continuous_across_knee() {
        let p = CompParams {
            threshold_db: -20.0,
            ratio: 3.0,
            knee_db: 8.0,
            ..CompParams::default()
        };
        let mut prev = comp_curve_db(-60.0, &p);
        let mut x = -60.0;
        while x < 0.0 {
            x += 0.1;
            let y = comp_curve_db(x, &p);
            assert!((y - prev).abs() < 0.2, "jump at {x}");
            prev = y;
        }
    }

    #[test]
    fn limiter_never_exceeds_ceiling() {
        let p = LimiterParams {
            enabled: true,
            ceiling_db: -3.0,
            release_ms: 50.0,
        };
        let mut l = Limiter::new();
        let mut x = sine(48_000, 220.0, 2.5);
        for ch in x.chunks_mut(FRAME) {
            l.process(ch, &p);
        }
        let pk = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(pk <= db_to_lin(-3.0) + 1e-4, "{pk}");
    }

    #[test]
    fn full_chain_is_finite_and_bounded() {
        let mut c = Chain::new();
        let p = ChainParams::voice();
        let mut seed = 12345u32;
        for _ in 0..200 {
            let mut f = [0.0f32; FRAME];
            for v in f.iter_mut() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                *v = ((seed >> 8) as f32 / 8_388_608.0 - 1.0) * 0.9;
            }
            let st = c.process(&mut f, &p);
            assert!(f.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
            assert!(st.out_peak <= 1.0);
        }
    }

    #[test]
    fn bypass_is_transparent_except_gain() {
        let mut c = Chain::new();
        let mut p = ChainParams {
            bypass: true,
            ..Default::default()
        };
        p.input.gain_db = 6.0206;
        let mut f = [0.1f32; FRAME];
        c.process(&mut f, &p);
        assert!(f.iter().all(|v| (v - 0.2).abs() < 1e-3));
    }

    #[test]
    fn mute_silences_output() {
        let mut c = Chain::new();
        let mut p = ChainParams::flat();
        p.output.mute = true;
        let mut f = [0.5f32; FRAME];
        let st = c.process(&mut f, &p);
        assert_eq!(st.out_peak, 0.0);
    }
}
