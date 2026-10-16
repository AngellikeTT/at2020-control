//! Real-time stress harness for the engine.
//!
//! Runs the engine's real capture-side ([`InputProcessor`]: resampler, chain, ring) and
//! render-side ([`OutputProcessor`]: drift-compensated resampler) on two threads paced
//! by the wall clock, exactly as WASAPI would call them, including the MMCSS
//! "Pro Audio" registration. The two audio threads are pinned to one core of the
//! process's affinity mask; optionally every *other* core in the mask gets a busy-loop
//! load thread at normal priority, and another thread keeps changing parameters.
//! The render clock can run fast or slow (ppm) to exercise the drift controller.

use crate::engine::{promote_thread, InputProcessor, OutputProcessor, RING_CAP};
use crate::shared::Shared;
use core_dsp::params::ChainParams;
use core_dsp::ring::Ring;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Barrier, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct SimConfig {
    pub secs: f64,
    pub in_rate: u32,
    pub out_rate: u32,
    /// Callback period of both simulated devices.
    pub period_ms: f64,
    /// Render clock error relative to the capture clock, in parts per million.
    pub out_ppm: f64,
    /// Saturate every other allowed core with a busy loop.
    pub load: bool,
    /// Keep changing the output gain and EQ from another thread.
    pub toggle_params: bool,
    pub tone_hz: f64,
    pub tone_amp: f32,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            secs: 6.0,
            in_rate: 48_000,
            out_rate: 48_000,
            period_ms: 10.0,
            out_ppm: 0.0,
            load: true,
            toggle_params: false,
            tone_hz: 1000.0,
            tone_amp: 0.25,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SimReport {
    pub audio_core: usize,
    pub load_threads: usize,
    pub mmcss_threads: u32,
    pub underruns: u64,
    /// Largest sample-to-sample step after start-up, and the most a clean tone can make.
    pub max_jump: f32,
    pub jump_limit: f32,
    pub max_jump_at_s: f64,
    /// Tone frequency measured over the last 40 % (output-sample time base).
    pub freq_hz: f64,
    pub expected_hz: f64,
    pub fill_min: usize,
    pub fill_max: usize,
    pub final_adj_ppm: f64,
    /// Worst lateness of a simulated callback against its deadline.
    pub worst_late_ms: f64,
    pub param_changes: usize,
    /// (seconds, ring fill, drift adjustment in ppm) every 0.25 s.
    pub trace: Vec<(f64, usize, f64)>,
    /// Longest single capture / render callback, and the longest interval between two
    /// completed capture callbacks (a stall shows up here even if wake-ups are on time).
    pub capture_busy_ms: f64,
    pub render_busy_ms: f64,
    pub capture_gap_ms: f64,
    /// Late wake-ups (> half a period) and underruns, with their callback index.
    pub events: Vec<String>,
}

impl SimReport {
    /// Everything a glitch-free, drift-corrected run must satisfy.
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.underruns != 0 {
            p.push(format!("{} underrun(s)", self.underruns));
        }
        if self.max_jump > self.jump_limit {
            p.push(format!(
                "glitch: step {:.4} > {:.4} at {:.3} s",
                self.max_jump, self.jump_limit, self.max_jump_at_s
            ));
        }
        if (self.freq_hz - self.expected_hz).abs() > 0.2 {
            p.push(format!(
                "pitch {:.3} Hz, expected {:.3} Hz (drift controller)",
                self.freq_hz, self.expected_hz
            ));
        }
        if self.fill_min == 0 || self.fill_max > RING_CAP / 4 {
            p.push(format!(
                "ring fill out of bounds: {}..{}",
                self.fill_min, self.fill_max
            ));
        }
        p
    }
}

/// Logical processors this process may run on (its affinity mask), lowest first.
pub fn allowed_cores() -> Vec<usize> {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};
    let (mut proc_mask, mut sys_mask) = (0usize, 0usize);
    // SAFETY: valid out-pointers; the pseudo-handle needs no closing.
    let ok = unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut proc_mask, &mut sys_mask) }
        .is_ok();
    if !ok || proc_mask == 0 {
        return vec![0];
    }
    (0..usize::BITS as usize)
        .filter(|i| proc_mask & (1usize << i) != 0)
        .collect()
}

fn pin_current_thread(core: usize) {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadAffinityMask};
    // SAFETY: pseudo-handle of the calling thread; the core is inside the process mask.
    unsafe {
        SetThreadAffinityMask(GetCurrentThread(), 1usize << core);
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Sleeps until `due` (absolute), returning how late the wake-up was.
fn sleep_until(due: Instant) -> Duration {
    let now = Instant::now();
    if let Some(d) = due.checked_duration_since(now) {
        std::thread::sleep(d);
    }
    Instant::now().saturating_duration_since(due)
}

pub fn run(cfg: &SimConfig) -> SimReport {
    let cores = allowed_cores();
    let audio_core = cores[0];
    let stop = Arc::new(AtomicBool::new(false));

    let load: Vec<_> = if cfg.load {
        cores[1..]
            .iter()
            .map(|&c| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    pin_current_thread(c);
                    let mut x = 0x9E37_79B9_7F4A_7C15u64;
                    while !stop.load(Relaxed) {
                        for _ in 0..10_000 {
                            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                        }
                        std::hint::black_box(x);
                    }
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    std::thread::sleep(Duration::from_millis(150)); // let the load ramp up

    let base = ChainParams::flat();
    let shared = Arc::new(Shared::new(base));
    let main = Arc::new(Ring::new(RING_CAP));
    let mon = Arc::new(Ring::new(RING_CAP));
    let period = Duration::from_secs_f64(cfg.period_ms / 1000.0);
    let worst_late_us = Arc::new(AtomicUsize::new(0));
    // Both audio threads get ready (pinned and registered with MMCSS, as a WASAPI
    // callback thread is before its first period) before the clock starts: a thread that
    // is still at normal priority when its first deadline passes would start late and
    // then run its missed periods back to back, which no real device asks for.
    let ready = Arc::new(Barrier::new(3));
    let start: Arc<OnceLock<Instant>> = Arc::new(OnceLock::new());

    let capture = {
        let (shared, main, mon, cfg) = (shared.clone(), main.clone(), mon.clone(), cfg.clone());
        let late = worst_late_us.clone();
        let (ready, start) = (ready.clone(), start.clone());
        std::thread::spawn(move || {
            pin_current_thread(audio_core);
            let mut inp = InputProcessor::new(cfg.in_rate, 1, main, mon, shared.clone());
            promote_thread(&mut inp.mmcss, &shared);
            ready.wait();
            ready.wait();
            let t0 = *start.get().unwrap_or(&Instant::now());
            let per_tick = (cfg.in_rate as f64 * cfg.period_ms / 1000.0).round() as usize;
            let mut buf = vec![0.0f32; per_tick];
            let w = std::f64::consts::TAU * cfg.tone_hz / cfg.in_rate as f64;
            let mut n = 0u64;
            let mut k = 0u32;
            let (mut busy, mut gap, mut last_done) = (Duration::ZERO, Duration::ZERO, None);
            let mut events = Vec::new();
            while (k as f64) * cfg.period_ms / 1000.0 < cfg.secs {
                k += 1;
                let l = sleep_until(t0 + period * k);
                late.fetch_max(l.as_micros() as usize, Relaxed);
                if l > period / 2 {
                    events.push(format!("capture tick {k}: woke {:.1} ms late", ms(l)));
                }
                for s in buf.iter_mut() {
                    *s = cfg.tone_amp * (w * n as f64).sin() as f32;
                    n += 1;
                }
                let s = Instant::now();
                inp.process(&buf);
                let done = Instant::now();
                busy = busy.max(done - s);
                if let Some(p) = last_done {
                    gap = gap.max(done - p);
                }
                last_done = Some(done);
            }
            (busy, gap, events)
        })
    };

    let render = {
        let (shared, main, cfg) = (shared.clone(), main.clone(), cfg.clone());
        let late = worst_late_us.clone();
        let (ready, start) = (ready.clone(), start.clone());
        std::thread::spawn(move || {
            pin_current_thread(audio_core);
            let mut out =
                OutputProcessor::new(main.clone(), 1, cfg.out_rate, false, shared.clone());
            promote_thread(&mut out.mmcss, &shared);
            ready.wait();
            ready.wait();
            let t0 = *start.get().unwrap_or(&Instant::now());
            let mut result: Vec<f32> =
                Vec::with_capacity((cfg.out_rate as f64 * (cfg.secs + 1.0)) as usize);
            let mut buf = vec![0.0f32; cfg.out_rate as usize];
            let per_tick =
                cfg.out_rate as f64 * cfg.period_ms / 1000.0 * (1.0 + cfg.out_ppm * 1e-6);
            let (mut owed, mut k) = (0.0f64, 0u32);
            let (mut fmin, mut fmax) = (usize::MAX, 0usize);
            let mut trace = Vec::with_capacity(64);
            let settle = 1.0; // s: priming and controller start-up
            let mut busy = Duration::ZERO;
            let mut events = Vec::new();
            while (k as f64) * cfg.period_ms / 1000.0 < cfg.secs {
                k += 1;
                let l = sleep_until(t0 + period * k);
                late.fetch_max(l.as_micros() as usize, Relaxed);
                if l > period / 2 {
                    events.push(format!("render tick {k}: woke {:.1} ms late", ms(l)));
                }
                owed += per_tick;
                let n = owed as usize;
                owed -= n as f64;
                let before = main.len();
                let under = shared.underruns.load(Relaxed);
                let s = Instant::now();
                out.fill(&mut buf[..n]);
                busy = busy.max(s.elapsed());
                if shared.underruns.load(Relaxed) != under {
                    events.push(format!(
                        "render tick {k}: underrun, needed {n}, ring had {before}"
                    ));
                }
                result.extend_from_slice(&buf[..n]);
                let t = (k as f64) * cfg.period_ms / 1000.0;
                if t > settle {
                    let f = main.len();
                    fmin = fmin.min(f);
                    fmax = fmax.max(f);
                }
                if (t * 4.0).fract() < cfg.period_ms / 1000.0 * 4.0 {
                    trace.push((t, main.len(), out.adj * 1e6));
                }
            }
            (result, fmin, fmax, out.adj, trace, busy, events)
        })
    };

    ready.wait(); // both audio threads pinned and promoted
    let _ = start.set(Instant::now() + Duration::from_millis(20));
    ready.wait(); // go

    let changes = if cfg.toggle_params {
        let shared = shared.clone();
        let secs = cfg.secs;
        std::thread::spawn(move || {
            let start = Instant::now();
            let mut i = 0usize;
            while start.elapsed().as_secs_f64() < secs {
                let mut p = base;
                p.output.gain_db = if i % 2 == 0 { -6.0 } else { 0.0 };
                p.eq.enabled = true;
                p.eq.bands[2].gain_db = if i % 3 == 0 { 3.0 } else { -3.0 };
                shared.params.set(p);
                i += 1;
                std::thread::sleep(Duration::from_millis(37));
            }
            i
        })
    } else {
        std::thread::spawn(|| 0)
    };

    let (cap_busy, cap_gap, mut events) = capture.join().unwrap_or_default();
    let (out, fill_min, fill_max, adj, trace, ren_busy, ren_events) =
        render.join().unwrap_or_default();
    events.extend(ren_events);
    let param_changes = changes.join().unwrap_or(0);
    stop.store(true, Relaxed);
    let load_threads = load.len();
    for t in load {
        let _ = t.join();
    }

    let mut r = analyse(
        cfg,
        &out,
        &shared,
        audio_core,
        load_threads,
        fill_min,
        fill_max,
        adj,
    );
    r.worst_late_ms = worst_late_us.load(Relaxed) as f64 / 1000.0;
    r.param_changes = param_changes;
    r.trace = trace;
    r.capture_busy_ms = cap_busy.as_secs_f64() * 1000.0;
    r.render_busy_ms = ren_busy.as_secs_f64() * 1000.0;
    r.capture_gap_ms = cap_gap.as_secs_f64() * 1000.0;
    r.events = events;
    r
}

#[allow(clippy::too_many_arguments)]
fn analyse(
    cfg: &SimConfig,
    out: &[f32],
    shared: &Shared,
    audio_core: usize,
    load_threads: usize,
    fill_min: usize,
    fill_max: usize,
    adj: f64,
) -> SimReport {
    let fs = cfg.out_rate as f64;
    // Skip start-up: the output is silent until the ring is primed, then the tone
    // starts mid-waveform (a legitimate step) and the drift controller settles.
    let start = ((1.2 * fs) as usize).min(out.len());
    let tail = &out[start..];
    let (mut max_jump, mut at) = (0.0f32, 0usize);
    for (i, w) in tail.windows(2).enumerate() {
        let j = (w[1] - w[0]).abs();
        if j > max_jump {
            max_jump = j;
            at = i;
        }
    }
    // A tone of amplitude A changes by at most A*2*pi*f/fs per sample. Parameter
    // changes may raise A by the largest gain they apply (EQ +3 dB at the tone) but,
    // being smoothed, never step it.
    let gain = if cfg.toggle_params {
        10f64.powf(3.0 / 20.0)
    } else {
        1.0
    };
    let jump_limit = (cfg.tone_amp as f64 * gain * std::f64::consts::TAU * cfg.tone_hz / fs * 1.25)
        as f32
        + 1e-3;

    // Frequency from rising zero crossings (linear interpolation) over the last 40 %.
    let a = out.len() * 6 / 10;
    let seg = &out[a..];
    let mut first = None;
    let mut last = 0.0f64;
    let mut count = 0usize;
    for (i, w) in seg.windows(2).enumerate() {
        if w[0] < 0.0 && w[1] >= 0.0 {
            let t = i as f64 + (-w[0] / (w[1] - w[0])) as f64;
            if first.is_none() {
                first = Some(t);
            } else {
                count += 1;
            }
            last = t;
        }
    }
    let freq_hz = match first {
        Some(f) if count > 0 && last > f => count as f64 * fs / (last - f),
        _ => 0.0,
    };

    SimReport {
        audio_core,
        load_threads,
        mmcss_threads: shared.mmcss_threads.load(Relaxed),
        underruns: shared.underruns.load(Relaxed),
        max_jump,
        jump_limit,
        max_jump_at_s: (start + at) as f64 / fs,
        freq_hz,
        // The render device consumes (1 + ppm) faster than its nominal rate, so the
        // drift controller must deliver the tone at f / (1 + ppm) in output samples.
        expected_hz: cfg.tone_hz / (1.0 + cfg.out_ppm * 1e-6),
        fill_min: if fill_min == usize::MAX { 0 } else { fill_min },
        fill_max,
        final_adj_ppm: adj * 1e6,
        worst_late_ms: 0.0,
        param_changes: 0,
        trace: Vec::new(),
        capture_busy_ms: 0.0,
        render_busy_ms: 0.0,
        capture_gap_ms: 0.0,
        events: Vec::new(),
    }
}
