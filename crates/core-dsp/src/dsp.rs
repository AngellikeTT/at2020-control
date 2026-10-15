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
    /// Constant-0-dB-peak band-pass.
    pub fn bandpass(fs: f64, f0: f64, q: f64) -> Self {
        let (c, s) = Self::w(fs, f0);
        let al = s / (2.0 * q);
        Self::norm(al, 0.0, -al, 1.0 + al, -2.0 * c, 1.0 - al)
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
    /// Make-up gain actually applied; it slews toward the target per sample so that
    /// parameter changes never step at block boundaries.
    makeup_db: f32,
    makeup_init: bool,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            det: 0.0,
            gr_db: 0.0,
            makeup_db: 0.0,
            makeup_init: false,
        }
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
        if !self.makeup_init {
            self.makeup_db = makeup;
            self.makeup_init = true;
        }
        let mk = 1.0 - tc_coef(5.0);
        let mut max_gr = 0.0f32;
        for x in buf.iter_mut() {
            self.makeup_db += (makeup - self.makeup_db) * mk;
            self.det = x.abs().max(self.det * det_rel);
            let xdb = lin_to_db(self.det.max(1e-6));
            let target = comp_curve_db(xdb, p) - xdb; // <= 0
            let c = if target < self.gr_db { att } else { rel };
            self.gr_db = c * self.gr_db + (1.0 - c) * target;
            *x *= db_to_lin(self.gr_db + self.makeup_db);
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

// ------------------------------------------------------------- de-esser ----

/// Split-band de-esser: a band-pass around the sibilance frequency drives a
/// compressor-style gain computer, and the reduction is applied to that band only
/// (`y = x - band * (1 - g)`), so the rest of the spectrum is untouched.
pub struct DeEsser {
    bp: Biquad,
    cached_hz: f32,
    env: f32,
    gr_db: f32,
}

impl DeEsser {
    pub fn new() -> Self {
        Self {
            bp: Biquad::new(Coeffs::identity()),
            cached_hz: 0.0,
            env: 0.0,
            gr_db: 0.0,
        }
    }

    /// Returns the largest gain reduction (positive dB) applied during the block.
    pub fn process(&mut self, buf: &mut [f32], p: &DeEssParams) -> f32 {
        if (self.cached_hz - p.freq_hz).abs() > 0.5 {
            self.bp
                .set(Coeffs::bandpass(SAMPLE_RATE as f64, p.freq_hz as f64, 1.2));
            self.cached_hz = p.freq_hz;
        }
        let det_rel = tc_coef(4.0);
        let att = tc_coef(0.5);
        let rel = tc_coef(25.0);
        let inv = 1.0 / p.ratio.max(1.0);
        let max_red = p.max_reduction_db.max(0.0);
        let mut max_gr = 0.0f32;
        for x in buf.iter_mut() {
            let band = self.bp.process(*x as f64) as f32;
            self.env = band.abs().max(self.env * det_rel);
            let over = lin_to_db(self.env.max(1e-6)) - p.threshold_db;
            let target = if over > 0.0 {
                (over * (1.0 - inv)).min(max_red)
            } else {
                0.0
            };
            let c = if target > self.gr_db { att } else { rel };
            self.gr_db = c * self.gr_db + (1.0 - c) * target;
            *x -= band * (1.0 - db_to_lin(-self.gr_db));
            max_gr = max_gr.max(self.gr_db);
        }
        max_gr
    }
}

// -------------------------------------------------------------- vad gate ----

/// Gate opened by RNNoise's voice probability rather than by level, so steady
/// non-voice sounds (fans, keyboard) stay shut even when loud.
pub struct VadGate {
    gain: f32,
    open: bool,
    hold: u32,
}

impl VadGate {
    pub fn new() -> Self {
        Self {
            gain: 1.0,
            open: true,
            hold: 0,
        }
    }

    /// `vad` is the probability for this block. Returns whether the gate is open.
    pub fn process(&mut self, buf: &mut [f32], vad: f32, p: &VadGateParams) -> bool {
        let frames_hold = (p.hold_ms.max(0.0) * 0.001 * SAMPLE_RATE / FRAME as f32).ceil() as u32;
        if vad >= p.threshold {
            self.open = true;
            self.hold = frames_hold;
        } else if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.open = false;
        }
        let floor = db_to_lin(p.range_db.min(0.0));
        let target = if self.open { 1.0 } else { floor };
        let a_att = 1.0 - tc_coef(3.0);
        let a_rel = 1.0 - tc_coef(p.release_ms.max(1.0) / 6.9);
        for x in buf.iter_mut() {
            self.gain += (target - self.gain) * if target > self.gain { a_att } else { a_rel };
            *x *= self.gain;
        }
        self.open
    }
}

// ---------------------------------------------------------------- chain -----

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub in_peak: f32,
    pub out_peak: f32,
    pub gr_db: f32,
    pub deess_gr_db: f32,
    pub gate_open: bool,
    pub vad_gate_open: bool,
    pub vad: f32,
}

/// Parameter smoothing coefficient per 10 ms frame (time constant ~ 20 ms).
const SMOOTH: f32 = 0.4;

#[inline]
fn sm(cur: &mut f32, tgt: f32) {
    *cur += (tgt - *cur) * SMOOTH;
    if (tgt - *cur).abs() < 1e-5 * tgt.abs().max(1.0) {
        *cur = tgt;
    }
}
/// Smooth a frequency on a log axis.
#[inline]
fn sm_hz(cur: &mut f32, tgt: f32) {
    let mut l = cur.max(1.0).ln();
    sm(&mut l, tgt.max(1.0).ln());
    *cur = l.exp();
    if (tgt - *cur).abs() < 1e-3 * tgt {
        *cur = tgt;
    }
}

/// Folds every enable switch into a neutral numeric value so that toggling a stage is
/// just another smoothed parameter move (no discrete switching in the signal path).
pub fn effective_params(p: &ChainParams) -> ChainParams {
    let mut e = *p;
    if !p.input.lowcut_on {
        e.input.lowcut_hz = 10.0;
    }
    if !p.denoise.enabled {
        e.denoise.strength = 0.0;
    }
    if !p.vad_gate.enabled {
        e.vad_gate.range_db = 0.0;
    }
    for b in e.eq.bands.iter_mut() {
        if !p.eq.enabled || !b.enabled {
            b.gain_db = 0.0;
        }
        b.enabled = true;
    }
    e.eq.enabled = true;
    if !p.deess.enabled {
        e.deess.max_reduction_db = 0.0;
    }
    if !p.comp.enabled {
        e.comp.ratio = 1.0;
        e.comp.makeup_db = 0.0;
        e.comp.auto_makeup = false;
    }
    if !p.limiter.enabled {
        e.limiter.ceiling_db = 0.0;
    }
    e.limiter.enabled = true;
    e.output.gain_db = if p.output.mute {
        -120.0
    } else {
        p.output.gain_db
    };
    e
}

fn smooth_params(c: &mut ChainParams, t: &ChainParams) {
    sm(&mut c.input.gain_db, t.input.gain_db);
    sm_hz(&mut c.input.lowcut_hz, t.input.lowcut_hz);
    sm(&mut c.denoise.strength, t.denoise.strength);
    sm(&mut c.gate.threshold_db, t.gate.threshold_db);
    sm(&mut c.gate.hysteresis_db, t.gate.hysteresis_db);
    sm(&mut c.gate.attack_ms, t.gate.attack_ms);
    sm(&mut c.gate.hold_ms, t.gate.hold_ms);
    sm(&mut c.gate.release_ms, t.gate.release_ms);
    sm(&mut c.gate.range_db, t.gate.range_db);
    sm(&mut c.vad_gate.threshold, t.vad_gate.threshold);
    sm(&mut c.vad_gate.hold_ms, t.vad_gate.hold_ms);
    sm(&mut c.vad_gate.release_ms, t.vad_gate.release_ms);
    sm(&mut c.vad_gate.range_db, t.vad_gate.range_db);
    for (cb, tb) in c.eq.bands.iter_mut().zip(t.eq.bands.iter()) {
        cb.kind = tb.kind;
        sm_hz(&mut cb.freq, tb.freq);
        sm(&mut cb.gain_db, tb.gain_db);
        sm(&mut cb.q, tb.q);
    }
    sm_hz(&mut c.deess.freq_hz, t.deess.freq_hz);
    sm(&mut c.deess.threshold_db, t.deess.threshold_db);
    sm(&mut c.deess.ratio, t.deess.ratio);
    sm(&mut c.deess.max_reduction_db, t.deess.max_reduction_db);
    sm(&mut c.comp.threshold_db, t.comp.threshold_db);
    sm(&mut c.comp.ratio, t.comp.ratio);
    sm(&mut c.comp.attack_ms, t.comp.attack_ms);
    sm(&mut c.comp.release_ms, t.comp.release_ms);
    sm(&mut c.comp.knee_db, t.comp.knee_db);
    sm(&mut c.comp.makeup_db, t.comp.makeup_db);
    sm(&mut c.limiter.ceiling_db, t.limiter.ceiling_db);
    sm(&mut c.limiter.release_ms, t.limiter.release_ms);
    sm(&mut c.output.gain_db, t.output.gain_db);
    // Discrete values follow immediately.
    c.input.channel = t.input.channel;
    c.input.phase_invert = t.input.phase_invert;
    c.comp.auto_makeup = t.comp.auto_makeup;
}

pub struct Chain {
    hpf: Biquad,
    hpf_freq: f32,
    denoise: Denoiser,
    vad_gate: VadGate,
    gate: Gate,
    eq: Eq,
    deess: DeEsser,
    comp: Compressor,
    lim: Limiter,
    cur: ChainParams,
    started: bool,
    g_in: f32,
    g_out: f32,
    bypass_mix: f32,
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
            vad_gate: VadGate::new(),
            gate: Gate::new(),
            eq: Eq::new(),
            deess: DeEsser::new(),
            comp: Compressor::new(),
            lim: Limiter::new(),
            cur: ChainParams::default(),
            started: false,
            g_in: 1.0,
            g_out: 1.0,
            bypass_mix: 0.0,
        }
    }

    /// Fixed algorithmic latency of the chain in samples (denoise frame + gate lookahead).
    pub const LATENCY_SAMPLES: usize = FRAME + GATE_LOOKAHEAD;

    pub fn process(&mut self, buf: &mut [f32], target: &ChainParams) -> FrameStats {
        debug_assert_eq!(buf.len(), FRAME);
        let tgt = effective_params(target);
        let sign = if tgt.input.phase_invert { -1.0 } else { 1.0 };
        let tgt_in = db_to_lin(tgt.input.gain_db) * sign;
        let tgt_out = if tgt.output.gain_db <= -119.0 {
            0.0
        } else {
            db_to_lin(tgt.output.gain_db)
        };
        let tgt_bypass = if target.bypass { 1.0 } else { 0.0 };
        if !self.started {
            // First block snaps to the target so there is nothing to glide from.
            self.cur = tgt;
            self.g_in = tgt_in;
            self.g_out = tgt_out;
            self.bypass_mix = tgt_bypass;
            self.started = true;
        } else {
            smooth_params(&mut self.cur, &tgt);
        }
        let p = self.cur;

        let mut st = FrameStats::default();
        // Input gain with a per-sample linear ramp from the previous block's value.
        let step_in = (tgt_in - self.g_in) / FRAME as f32;
        let mut g = self.g_in;
        for x in buf.iter_mut() {
            g += step_in;
            *x *= g;
            st.in_peak = st.in_peak.max(x.abs());
        }
        self.g_in = tgt_in;

        // Dry path for bypass (zero extra latency; crossfaded below).
        let mut dry = [0.0f32; FRAME];
        dry.copy_from_slice(buf);

        if (p.input.lowcut_hz - self.hpf_freq).abs() > 1e-3 {
            self.hpf.set(Coeffs::highpass(
                SAMPLE_RATE as f64,
                p.input.lowcut_hz as f64,
                std::f64::consts::FRAC_1_SQRT_2,
            ));
            self.hpf_freq = p.input.lowcut_hz;
        }
        for x in buf.iter_mut() {
            *x = self.hpf.process(*x as f64) as f32;
        }

        st.vad = self.denoise.process(buf, p.denoise.strength);
        st.vad_gate_open =
            self.vad_gate.process(buf, st.vad, &p.vad_gate) || !target.vad_gate.enabled;
        let gate_p = GateParams {
            enabled: target.gate.enabled,
            ..p.gate
        };
        st.gate_open = self.gate.process(buf, &gate_p);
        self.eq.process(buf, &p.eq);
        st.deess_gr_db = self.deess.process(buf, &p.deess);
        st.gr_db = self.comp.process(buf, &p.comp);

        // Output gain ramp, then limiter.
        let step_out = (tgt_out - self.g_out) / FRAME as f32;
        let mut g = self.g_out;
        for x in buf.iter_mut() {
            g += step_out;
            *x *= g;
        }
        self.g_out = tgt_out;
        self.lim.process(buf, &p.limiter);

        // Bypass crossfade (dry also gets output gain and a hard clamp).
        let step_b = (tgt_bypass - self.bypass_mix) / FRAME as f32;
        let mut m = self.bypass_mix;
        for (x, d) in buf.iter_mut().zip(dry.iter()) {
            m += step_b;
            if m > 0.0 {
                let dy = (*d * tgt_out).clamp(-1.0, 1.0);
                *x = *x * (1.0 - m) + dy * m;
            }
        }
        self.bypass_mix = tgt_bypass;
        if target.bypass {
            st.gate_open = true;
            st.vad_gate_open = true;
            st.gr_db = 0.0;
            st.deess_gr_db = 0.0;
        }
        for x in buf.iter() {
            st.out_peak = st.out_peak.max(x.abs());
        }
        st
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for Eq {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for DeEsser {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for VadGate {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for Chain {
    fn default() -> Self {
        Self::new()
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
    // ---- de-esser, VAD gate and smoothing ----

    fn peak_db(x: &[f32]) -> f32 {
        lin_to_db(x.iter().fold(0.0f32, |a, v| a.max(v.abs())))
    }

    #[test]
    fn deesser_cuts_sibilance_band_and_leaves_the_rest() {
        let p = DeEssParams {
            enabled: true,
            freq_hz: 6500.0,
            threshold_db: -30.0,
            ratio: 6.0,
            max_reduction_db: 12.0,
        };
        let mut hi = DeEsser::new();
        let mut x = sine(48_000, 6500.0, db_to_lin(-12.0));
        let before = peak_db(&x[40_000..]);
        let mut gr = 0.0;
        for ch in x.chunks_mut(FRAME) {
            gr = hi.process(ch, &p);
        }
        let after = peak_db(&x[40_000..]);
        assert!(gr > 6.0, "gr {gr}");
        assert!(before - after > 5.0, "cut {:.1} dB", before - after);

        let mut lo = DeEsser::new();
        let mut v = sine(48_000, 500.0, db_to_lin(-12.0));
        let b = peak_db(&v[40_000..]);
        for ch in v.chunks_mut(FRAME) {
            lo.process(ch, &p);
        }
        assert!((b - peak_db(&v[40_000..])).abs() < 0.5, "500 Hz must pass");
    }

    #[test]
    fn deesser_is_transparent_below_threshold() {
        let p = DeEssParams::default();
        let mut d = DeEsser::new();
        let mut x = sine(24_000, 6500.0, db_to_lin(-60.0));
        let b = peak_db(&x[12_000..]);
        for ch in x.chunks_mut(FRAME) {
            assert_eq!(d.process(ch, &p), 0.0);
        }
        assert!((b - peak_db(&x[12_000..])).abs() < 0.2);
    }

    #[test]
    fn vad_gate_follows_probability_with_hold() {
        let p = VadGateParams {
            enabled: true,
            threshold: 0.5,
            hold_ms: 100.0,
            release_ms: 50.0,
            range_db: -40.0,
        };
        let mut g = VadGate::new();
        let mut buf = [0.5f32; FRAME];
        assert!(g.process(&mut buf, 0.9, &p));
        // 5 quiet blocks (50 ms) are inside the 100 ms hold: still open.
        for _ in 0..5 {
            let mut b = [0.5f32; FRAME];
            assert!(g.process(&mut b, 0.1, &p));
        }
        // After the hold expires it closes and attenuates by about the range.
        let mut last = [0.0f32; FRAME];
        let mut open = true;
        for _ in 0..40 {
            last = [0.5f32; FRAME];
            open = g.process(&mut last, 0.1, &p);
        }
        assert!(!open);
        assert!(lin_to_db(last[FRAME - 1] / 0.5) < -30.0);
        // Voice returns: opens again.
        let mut b = [0.5f32; FRAME];
        assert!(g.process(&mut b, 0.8, &p));
    }

    /// Largest sample-to-sample step in a signal.
    fn max_step(x: &[f32]) -> f32 {
        x.windows(2).fold(0.0f32, |a, w| a.max((w[1] - w[0]).abs()))
    }

    #[test]
    fn output_gain_change_is_click_free() {
        let x = sine(FRAME * 60, 200.0, 0.4);
        let mut p = ChainParams::flat();
        let mut c = Chain::new();
        let mut y = Vec::new();
        for (i, ch) in x.chunks(FRAME).enumerate() {
            if i == 30 {
                p.output.gain_db = -30.0; // abrupt UI change
            }
            let mut b = ch.to_vec();
            c.process(&mut b, &p);
            y.extend(b);
        }
        // A 200 Hz sine at 0.4 changes by at most 0.4*2*pi*200/48000 = 0.0105 per sample.
        let step = max_step(&y[FRAME * 5..]);
        assert!(step < 0.0125, "step {step}");
        assert!(peak_db(&y[FRAME * 50..]) < -35.0, "gain must settle");
    }

    #[test]
    fn toggling_stages_mid_stream_does_not_click() {
        let x = sine(FRAME * 100, 300.0, 0.3);
        let mut c = Chain::new();
        let mut p = ChainParams::voice();
        let mut y = Vec::new();
        for (i, ch) in x.chunks(FRAME).enumerate() {
            match i {
                20 => p.eq.enabled = false,
                30 => p.comp.enabled = false,
                40 => p.limiter.enabled = false,
                50 => p.deess.enabled = false,
                60 => p.bypass = true,
                70 => p.bypass = false,
                72 => p.comp.enabled = true,
                74 => p.eq.enabled = true,
                76 => p.deess.enabled = true,
                78 => p.limiter.enabled = true,
                _ => {}
            }
            let mut b = ch.to_vec();
            c.process(&mut b, &p);
            y.extend(b);
        }
        // Skip the start-up; no step may exceed ~2x a clean 300 Hz sine's maximum step.
        let clean = 0.3 * 2.0 * std::f32::consts::PI * 300.0 / 48_000.0;
        let step = max_step(&y[FRAME * 10..]);
        assert!(step < clean * 2.0, "step {step} vs clean {clean}");
    }

    #[test]
    fn eq_gain_sweep_is_click_free() {
        let x = sine(FRAME * 60, 1000.0, 0.2);
        let mut c = Chain::new();
        let mut p = ChainParams::flat();
        p.eq.enabled = true;
        let mut y = Vec::new();
        for (i, ch) in x.chunks(FRAME).enumerate() {
            p.eq.bands[2].gain_db = if i < 20 { 0.0 } else { 12.0 };
            let mut b = ch.to_vec();
            c.process(&mut b, &p);
            y.extend(b);
        }
        let clean = 0.2 * 4.0 * 2.0 * std::f32::consts::PI * 1000.0 / 48_000.0; // up to +12 dB
        assert!(max_step(&y[FRAME * 5..]) < clean * 1.2);
    }

    #[test]
    fn effective_params_neutralise_disabled_stages() {
        let mut p = ChainParams::voice();
        p.comp.enabled = false;
        p.deess.enabled = false;
        p.eq.enabled = false;
        let e = effective_params(&p);
        assert_eq!(e.comp.ratio, 1.0);
        assert_eq!(e.deess.max_reduction_db, 0.0);
        assert!(e.eq.bands.iter().all(|b| b.gain_db == 0.0));
    }
}
