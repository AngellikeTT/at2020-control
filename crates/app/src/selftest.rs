//! Hidden `--selftest` mode: exercises devices, the DSP chain, the engine and the
//! virtual-cable loopback, then writes SELFTEST_REPORT.md.
//!
//! Every Windows setting touched (mic volume, mute) is captured first and restored by a
//! guard, including on early return. Default devices and the enhancement flag are only
//! read, then compared before/after.

use anyhow::{anyhow, bail, Context, Result};
use core_dsp::dsp::{comp_curve_db, db_to_lin, lin_to_db, Chain};
use core_dsp::params::*;
use core_dsp::resample::Resampler;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use device::devctl::{Cmd, DevCtl, DevState};
use device::engine::{pump_offline, Engine, EngineConfig};
use device::shared::Shared;
use std::fmt::Write as _;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FS: f64 = SAMPLE_RATE as f64;

pub struct Options {
    pub live_secs: u64,
    pub soak_secs: u64,
    pub out_path: String,
}

impl Options {
    pub fn from_args(args: &[String]) -> Self {
        let get = |k: &str| {
            args.iter()
                .position(|a| a == k)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let quick = args.iter().any(|a| a == "--quick");
        let soak_min: f64 = get("--soak-minutes")
            .and_then(|v| v.parse().ok())
            .unwrap_or(if quick { 0.5 } else { 60.0 });
        Self {
            live_secs: get("--live-seconds")
                .and_then(|v| v.parse().ok())
                .unwrap_or(if quick { 8 } else { 30 }),
            soak_secs: (soak_min * 60.0) as u64,
            out_path: get("--out").unwrap_or_else(|| "SELFTEST_REPORT.md".into()),
        }
    }
}

// ---------------------------------------------------------------- report --

#[derive(Default)]
struct Report {
    body: String,
    pass: u32,
    fail: u32,
    warn: u32,
}

impl Report {
    fn h(&mut self, t: &str) {
        let _ = writeln!(self.body, "\n## {t}\n");
        println!("== {t}");
    }
    fn info(&mut self, t: impl AsRef<str>) {
        let _ = writeln!(self.body, "- {}", t.as_ref());
        println!("   {}", t.as_ref());
    }
    fn warn(&mut self, t: impl AsRef<str>) {
        self.warn += 1;
        let _ = writeln!(self.body, "- WARN: {}", t.as_ref());
        println!("   WARN {}", t.as_ref());
    }
    fn check(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
        }
        let tag = if ok { "PASS" } else { "FAIL" };
        let _ = writeln!(self.body, "- **{tag}** {name}: {}", detail.as_ref());
        println!("   {tag} {name}: {}", detail.as_ref());
    }
}

fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

// ---------------------------------------------------------- signal checks --

/// Verdict on audio captured from the microphone: (pass, detail). Exact digital
/// silence fails: it means the device is sending zeros (hardware/touch mute), which
/// earlier versions of this test reported as a quiet but passing signal.
fn mic_signal_check(x: &[f32]) -> (bool, String) {
    let c = core_dsp::level::classify(x);
    (c.is_signal(), c.describe())
}

/// Verdict on the input peak measured while an exclusive-mode stream ran.
fn exclusive_signal_check(peak: f32) -> (bool, String) {
    let ok = core_dsp::level::peak_is_signal(peak);
    let mut d = format!("in peak {:.1} dBFS", lin_to_db(peak));
    if !ok {
        d.push_str(" (exact digital silence from the device: hardware/touch mute?)");
    }
    (ok, d)
}

// --------------------------------------------------------------- signals --

fn tone(f: f64, db: f32, secs: f64, fs: f64) -> Vec<f32> {
    let a = db_to_lin(db) as f64;
    (0..(secs * fs) as usize)
        .map(|i| (a * (2.0 * std::f64::consts::PI * f * i as f64 / fs).sin()) as f32)
        .collect()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
    }
}

fn noise(db_rms: f32, secs: f64, fs: f64, seed: u64) -> Vec<f32> {
    let mut r = Rng(seed | 1);
    // uniform [-1,1] has rms 1/sqrt(3)
    let a = db_to_lin(db_rms) * 3.0f32.sqrt();
    (0..(secs * fs) as usize).map(|_| r.next() * a).collect()
}

fn rms_db(x: &[f32]) -> f32 {
    if x.is_empty() {
        return -200.0;
    }
    let p = x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64;
    (10.0 * (p + 1e-20).log10()) as f32
}

/// Least-squares sine fit at `f`; returns (peak amplitude, residual rms).
fn fit_sine(x: &[f32], fs: f64, f: f64) -> (f64, f64) {
    let n = x.len() as f64;
    let mean = x.iter().map(|v| *v as f64).sum::<f64>() / n;
    let w = 2.0 * std::f64::consts::PI * f / fs;
    let (mut ss, mut cc, mut sc, mut sx, mut cx) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let (s, c) = (w * i as f64).sin_cos();
        let v = *v as f64 - mean;
        ss += s * s;
        cc += c * c;
        sc += s * c;
        sx += s * v;
        cx += c * v;
    }
    let det = ss * cc - sc * sc;
    let a = (sx * cc - cx * sc) / det;
    let b = (cx * ss - sx * sc) / det;
    let mut r = 0.0;
    for (i, v) in x.iter().enumerate() {
        let (s, c) = (w * i as f64).sin_cos();
        let e = *v as f64 - mean - a * s - b * c;
        r += e * e;
    }
    ((a * a + b * b).sqrt(), (r / n).sqrt())
}

fn amp_db(x: &[f32], fs: f64, f: f64) -> f32 {
    (20.0 * (fit_sine(x, fs, f).0 + 1e-12).log10()) as f32
}

fn thdn_db(x: &[f32], fs: f64, f: f64) -> f32 {
    let (a, r) = fit_sine(x, fs, f);
    (20.0 * ((r + 1e-12) / (a / std::f64::consts::SQRT_2 + 1e-12)).log10()) as f32
}

/// Delay (samples) of `y` relative to `x` by brute-force cross-correlation of a probe.
fn find_delay(x: &[f32], y: &[f32], max_lag: usize) -> Option<usize> {
    let n = 8192.min(x.len());
    let probe = &x[..n];
    let (mut best, mut best_v) = (0usize, f64::MIN);
    for lag in 0..max_lag.min(y.len().saturating_sub(n)) {
        let v: f64 = probe
            .iter()
            .zip(&y[lag..lag + n])
            .map(|(a, b)| *a as f64 * *b as f64)
            .sum();
        if v > best_v {
            best_v = v;
            best = lag;
        }
    }
    (best_v > 0.0).then_some(best)
}

fn run_chain(p: &ChainParams, x: &[f32]) -> Vec<f32> {
    let mut c = Chain::new();
    let mut y = x.to_vec();
    let n = y.len() / FRAME * FRAME;
    for ch in y[..n].chunks_mut(FRAME) {
        c.process(ch, p);
    }
    y.truncate(n);
    y
}

fn tail(y: &[f32], from_frac: f32) -> &[f32] {
    &y[(y.len() as f32 * from_frac) as usize..]
}

fn resample(x: &[f32], from: f64, to: f64) -> Vec<f32> {
    if (from - to).abs() < 1e-6 {
        return x.to_vec();
    }
    let mut it = x.iter().copied();
    let mut out = Vec::with_capacity((x.len() as f64 * to / from) as usize + 8);
    let mut rs = Resampler::new();
    rs.run(from / to, &mut || it.next(), &mut |y| {
        out.push(y);
        true
    });
    out
}

// ---------------------------------------------------------- audio streams --

type Chunks = Arc<Mutex<Vec<(usize, usize, Instant)>>>;

struct Recorder {
    _stream: Stream,
    buf: Arc<Mutex<Vec<f32>>>,
    chunks: Chunks,
    rate: f64,
}

fn find_dev(name: &str, input: bool) -> Result<cpal::Device> {
    let host = cpal::default_host();
    let mut it: Box<dyn Iterator<Item = cpal::Device>> = if input {
        Box::new(host.input_devices()?)
    } else {
        Box::new(host.output_devices()?)
    };
    it.find(|d| d.name().map(|n| n == name).unwrap_or(false))
        .ok_or_else(|| anyhow!("device \"{name}\" not found"))
}

fn rec_stream<T>(
    dev: &cpal::Device,
    cfg: &StreamConfig,
    buf: Arc<Mutex<Vec<f32>>>,
    chunks: Chunks,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let ch = cfg.channels as usize;
    Ok(dev.build_input_stream(
        cfg,
        move |d: &[T], _: &cpal::InputCallbackInfo| {
            if let (Ok(mut b), Ok(mut c)) = (buf.lock(), chunks.lock()) {
                c.push((b.len(), d.len() / ch, Instant::now()));
                for fr in d.chunks_exact(ch) {
                    b.push(fr[0].to_sample::<f32>());
                }
            }
        },
        |_| {},
        None,
    )?)
}

impl Recorder {
    fn start(name: &str) -> Result<Self> {
        let dev = find_dev(name, true)?;
        let sc = dev.default_input_config()?;
        let cfg: StreamConfig = sc.config();
        let buf = Arc::new(Mutex::new(Vec::with_capacity(1 << 20)));
        let chunks: Chunks = Arc::new(Mutex::new(Vec::new()));
        let stream = match sc.sample_format() {
            SampleFormat::F32 => rec_stream::<f32>(&dev, &cfg, buf.clone(), chunks.clone())?,
            SampleFormat::I16 => rec_stream::<i16>(&dev, &cfg, buf.clone(), chunks.clone())?,
            SampleFormat::I32 => rec_stream::<i32>(&dev, &cfg, buf.clone(), chunks.clone())?,
            f => bail!("unsupported sample format {f:?}"),
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            buf,
            chunks,
            rate: cfg.sample_rate.0 as f64,
        })
    }
    /// Captured mono samples resampled to 48 kHz, plus the per-callback timestamps
    /// (start index in device-rate samples, length, callback instant).
    fn take_48k_with_chunks(&self) -> (Vec<f32>, Vec<(usize, usize, Instant)>) {
        let v = self
            .buf
            .lock()
            .map(|mut b| std::mem::take(&mut *b))
            .unwrap_or_default();
        let c = self
            .chunks
            .lock()
            .map(|mut c| std::mem::take(&mut *c))
            .unwrap_or_default();
        (resample(&v, self.rate, FS), c)
    }
    fn take_48k(&self) -> Vec<f32> {
        self.take_48k_with_chunks().0
    }
}

struct Player {
    _stream: Stream,
    done: Arc<std::sync::atomic::AtomicBool>,
    marker: Arc<Mutex<Option<Instant>>>,
}

fn play_stream<T>(
    dev: &cpal::Device,
    cfg: &StreamConfig,
    data: Vec<f32>,
    marker_idx: usize,
    done: Arc<std::sync::atomic::AtomicBool>,
    marker: Arc<Mutex<Option<Instant>>>,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let ch = cfg.channels as usize;
    let rate = cfg.sample_rate.0 as f64;
    let mut pos = 0usize;
    Ok(dev.build_output_stream(
        cfg,
        move |d: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = d.len() / ch;
            if (pos..pos + frames).contains(&marker_idx) {
                let ahead = Duration::from_secs_f64((marker_idx - pos) as f64 / rate);
                if let Ok(mut m) = marker.lock() {
                    *m = Some(Instant::now() + ahead);
                }
            }
            for fr in d.chunks_exact_mut(ch) {
                let v = data.get(pos).copied().unwrap_or(0.0);
                pos += 1;
                for s in fr.iter_mut() {
                    *s = T::from_sample(v);
                }
            }
            if pos >= data.len() {
                done.store(true, Relaxed);
            }
        },
        |_| {},
        None,
    )?)
}

impl Player {
    /// `samples48` is mono at 48 kHz; `marker48` is the sample index whose submit time is recorded.
    fn start(name: &str, samples48: &[f32], marker48: usize) -> Result<Self> {
        let dev = find_dev(name, false)?;
        let sc = dev.default_output_config()?;
        let cfg: StreamConfig = sc.config();
        let rate = cfg.sample_rate.0 as f64;
        let data = resample(samples48, FS, rate);
        let m = (marker48 as f64 * rate / FS) as usize;
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marker = Arc::new(Mutex::new(None));
        let (d, mk) = (done.clone(), marker.clone());
        let stream = match sc.sample_format() {
            SampleFormat::F32 => play_stream::<f32>(&dev, &cfg, data, m, d, mk)?,
            SampleFormat::I16 => play_stream::<i16>(&dev, &cfg, data, m, d, mk)?,
            SampleFormat::I32 => play_stream::<i32>(&dev, &cfg, data, m, d, mk)?,
            f => bail!("unsupported sample format {f:?}"),
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            done,
            marker,
        })
    }
}

/// Plays `stim` (48 kHz mono) into `render` while recording `capture`.
/// Returns the capture aligned to stimulus index 0 (truncated to the stimulus length) and the
/// submit-to-delivery latency in ms (render callback hand-over to capture callback).
fn loopback(render: &str, capture: &str, stim: &[f32]) -> Result<(Vec<f32>, f64)> {
    let rec = Recorder::start(capture)?;
    std::thread::sleep(Duration::from_millis(400));
    let _ = rec.take_48k_with_chunks();
    let lead = 4800usize;
    let mut padded = vec![0.0f32; lead];
    padded.extend_from_slice(stim);
    padded.resize(padded.len() + 24_000, 0.0);
    let p = Player::start(render, &padded, lead)?;
    let t0 = Instant::now();
    while !p.done.load(Relaxed) && t0.elapsed().as_secs_f64() < padded.len() as f64 / FS + 3.0 {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(400));
    let marker = p.marker.lock().ok().and_then(|m| *m);
    drop(p);
    let (cap, chunks) = rec.take_48k_with_chunks();
    let rate = rec.rate;
    drop(rec);
    let delay = find_delay(&padded, &cap, 48_000)
        .ok_or_else(|| anyhow!("no correlation between stimulus and capture (silent cable?)"))?;
    let start = delay + lead;
    if start >= cap.len() {
        bail!("capture shorter than expected");
    }
    // Delivery time of the stimulus start: the callback that held that sample.
    let idx_dev = (start as f64 * rate / FS) as usize;
    let latency_ms = chunks
        .iter()
        .find(|(s, n, _)| (*s..*s + *n).contains(&idx_dev))
        .zip(marker)
        .map_or(f64::NAN, |((s, n, t), m)| {
            let behind = (s + n - idx_dev) as f64 / rate;
            let t_cap = t.checked_sub(Duration::from_secs_f64(behind)).unwrap_or(*t);
            t_cap.saturating_duration_since(m).as_secs_f64() * 1000.0
        });
    let mut out = cap[start..].to_vec();
    out.truncate(stim.len());
    Ok((out, latency_ms))
}

// ------------------------------------------------------------ process info --

mod procinfo {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    fn ft(f: FILETIME) -> f64 {
        ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) as f64 * 1e-7
    }
    /// Total CPU seconds (user + kernel) used by this process.
    pub fn cpu_seconds() -> f64 {
        unsafe {
            let (mut c, mut e, mut k, mut u) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_ok() {
                ft(k) + ft(u)
            } else {
                0.0
            }
        }
    }
    /// Working set in MiB.
    pub fn working_set_mib() -> f64 {
        unsafe {
            let mut m = PROCESS_MEMORY_COUNTERS {
                cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                ..Default::default()
            };
            if GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb).is_ok() {
                m.WorkingSetSize as f64 / (1024.0 * 1024.0)
            } else {
                0.0
            }
        }
    }
}

// ----------------------------------------------------------- restore guard --

struct Restore<'a> {
    dev: &'a DevCtl,
    id: Option<String>,
    volume: f32,
    mute: bool,
    active: bool,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Some(id) = &self.id {
            self.dev.send(Cmd::Select(id.clone()));
            std::thread::sleep(Duration::from_millis(700));
        }
        self.dev.send(Cmd::SetVolume(self.volume));
        self.dev.send(Cmd::SetMute(self.mute));
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn wait_for(dev: &DevCtl, timeout_ms: u64, f: impl Fn(&DevState) -> bool) -> Option<DevState> {
    let t0 = Instant::now();
    loop {
        let s = dev.snapshot();
        if f(&s) {
            return Some(s);
        }
        if t0.elapsed().as_millis() as u64 > timeout_ms {
            return None;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

// ------------------------------------------------------------------ tests --

fn sec_dsp(r: &mut Report) {
    r.h("DSP stages (offline, 48 kHz)");
    let mut flat = ChainParams::flat();
    flat.limiter.ceiling_db = -0.5;

    // Transparent chain: gain, flatness and THD+N.
    let mut worst: f32 = 0.0;
    let mut line = String::new();
    for f in [50.0, 100.0, 250.0, 1000.0, 4000.0, 10_000.0, 16_000.0] {
        let y = run_chain(&flat, &tone(f, -12.0, 1.5, FS));
        let g = amp_db(tail(&y, 0.4), FS, f) + 12.0;
        worst = worst.max(g.abs());
        let _ = write!(line, "{f:.0} Hz {g:+.2} dB; ");
    }
    r.check(
        "flat chain frequency response",
        worst < 0.3,
        format!("worst deviation {worst:.2} dB ({line})"),
    );
    let y = run_chain(&flat, &tone(1000.0, -6.0, 2.0, FS));
    let t = thdn_db(tail(&y, 0.3), FS, 1000.0);
    r.check(
        "flat chain THD+N @1 kHz -6 dBFS",
        t < -80.0,
        format!("{t:.1} dB"),
    );

    let mut bp = flat;
    bp.bypass = true;
    bp.input.gain_db = 6.0;
    let y = run_chain(&bp, &tone(1000.0, -20.0, 1.0, FS));
    let g = amp_db(tail(&y, 0.3), FS, 1000.0) + 20.0;
    r.check(
        "bypass applies input gain only (+6 dB)",
        (g - 6.0).abs() < 0.1,
        format!("{g:+.2} dB"),
    );

    // Low cut.
    let mut p = flat;
    p.input.lowcut_on = true;
    p.input.lowcut_hz = 80.0;
    let at80 = amp_db(
        tail(&run_chain(&p, &tone(80.0, -12.0, 1.5, FS)), 0.4),
        FS,
        80.0,
    ) + 12.0;
    let at1k = amp_db(
        tail(&run_chain(&p, &tone(1000.0, -12.0, 1.5, FS)), 0.4),
        FS,
        1000.0,
    ) + 12.0;
    r.check(
        "low cut -3 dB at corner",
        (at80 + 3.0).abs() < 1.0,
        format!("{at80:+.2} dB @80 Hz"),
    );
    r.check(
        "low cut passes 1 kHz",
        at1k.abs() < 0.2,
        format!("{at1k:+.2} dB"),
    );

    // RNNoise.
    let n = noise(-30.0, 4.0, FS, 7);
    let mut p = flat;
    p.denoise.enabled = true;
    p.denoise.strength = 1.0;
    let on = rms_db(tail(&run_chain(&p, &n), 0.5));
    let mut off = flat;
    off.denoise.enabled = false;
    let off_db = rms_db(tail(&run_chain(&off, &n), 0.5));
    r.check(
        "RNNoise suppresses stationary noise",
        off_db - on > 10.0,
        format!("{off_db:.1} dBFS -> {on:.1} dBFS ({:.1} dB)", off_db - on),
    );

    // Gate.
    let mut p = flat;
    p.gate.enabled = true;
    p.gate.threshold_db = -40.0;
    p.gate.range_db = -60.0;
    let loud = amp_db(
        tail(&run_chain(&p, &tone(1000.0, -20.0, 2.0, FS)), 0.5),
        FS,
        1000.0,
    ) + 20.0;
    let quiet = amp_db(
        tail(&run_chain(&p, &tone(1000.0, -60.0, 2.0, FS)), 0.5),
        FS,
        1000.0,
    ) + 60.0;
    r.check(
        "gate open passes signal",
        loud.abs() < 0.5,
        format!("{loud:+.2} dB"),
    );
    r.check(
        "gate closed attenuates",
        quiet < -20.0,
        format!("{quiet:+.1} dB"),
    );

    // EQ.
    let mut p = flat;
    p.eq.enabled = true;
    p.eq.bands[2].gain_db = 6.0;
    p.eq.bands[2].freq = 1000.0;
    let g = amp_db(
        tail(&run_chain(&p, &tone(1000.0, -20.0, 1.5, FS)), 0.4),
        FS,
        1000.0,
    ) + 20.0;
    r.check(
        "EQ +6 dB peak at 1 kHz",
        (g - 6.0).abs() < 0.5,
        format!("{g:+.2} dB"),
    );

    // Compressor.
    let mut p = flat;
    p.comp = CompParams {
        enabled: true,
        threshold_db: -20.0,
        ratio: 4.0,
        knee_db: 0.0,
        makeup_db: 0.0,
        auto_makeup: false,
        attack_ms: 1.0,
        release_ms: 50.0,
    };
    let y = run_chain(&p, &tone(1000.0, -8.0, 2.0, FS));
    let pk = lin_to_db(tail(&y, 0.5).iter().fold(0.0f32, |a, v| a.max(v.abs())));
    let want = comp_curve_db(-8.0, &p.comp);
    r.check(
        "compressor matches static curve",
        (pk - want).abs() < 1.0,
        format!("peak {pk:.2} dBFS, expected {want:.2}"),
    );

    // Limiter.
    let mut p = flat;
    p.limiter.ceiling_db = -3.0;
    p.input.gain_db = 12.0;
    let y = run_chain(&p, &tone(1000.0, -6.0, 1.5, FS));
    let pk = lin_to_db(tail(&y, 0.2).iter().fold(0.0f32, |a, v| a.max(v.abs())));
    r.check(
        "limiter holds ceiling",
        pk <= -3.0 + 0.1,
        format!("peak {pk:.2} dBFS (ceiling -3)"),
    );

    // Output gain & mute.
    let mut p = flat;
    p.output.gain_db = 6.0;
    let g = amp_db(
        tail(&run_chain(&p, &tone(1000.0, -30.0, 1.0, FS)), 0.3),
        FS,
        1000.0,
    ) + 30.0;
    r.check(
        "output gain +6 dB",
        (g - 6.0).abs() < 0.2,
        format!("{g:+.2} dB"),
    );
    p.output.mute = true;
    let m = rms_db(tail(&run_chain(&p, &tone(1000.0, -6.0, 1.0, FS)), 0.3));
    r.check("output mute silences", m < -100.0, format!("{m:.0} dBFS"));

    // De-esser: sibilance band reduced, speech band untouched.
    let mut p = flat;
    p.deess.enabled = true;
    p.deess.threshold_db = -30.0;
    p.deess.ratio = 6.0;
    p.deess.max_reduction_db = 12.0;
    let hi = amp_db(
        tail(&run_chain(&p, &tone(6500.0, -12.0, 1.5, FS)), 0.5),
        FS,
        6500.0,
    ) + 12.0;
    let mid = amp_db(
        tail(&run_chain(&p, &tone(500.0, -12.0, 1.5, FS)), 0.5),
        FS,
        500.0,
    ) + 12.0;
    r.check(
        "de-esser reduces 6.5 kHz",
        hi < -5.0,
        format!("{hi:+.1} dB"),
    );
    r.check(
        "de-esser leaves 500 Hz alone",
        mid.abs() < 0.5,
        format!("{mid:+.2} dB"),
    );

    // VAD gate: stationary non-voice noise is shut out even though it is loud.
    let mut p = flat;
    p.vad_gate.enabled = true;
    p.vad_gate.threshold = 0.5;
    p.vad_gate.range_db = -40.0;
    p.denoise.enabled = true;
    p.denoise.strength = 0.0; // keep the noise itself; only the VAD decision is used
    let n = noise(-25.0, 4.0, FS, 11);
    let gated = rms_db(tail(&run_chain(&p, &n), 0.5));
    let mut off = p;
    off.vad_gate.enabled = false;
    let open = rms_db(tail(&run_chain(&off, &n), 0.5));
    r.check(
        "VAD gate shuts on non-voice noise",
        open - gated > 20.0,
        format!("{open:.1} -> {gated:.1} dBFS ({:.1} dB)", open - gated),
    );

    // Smoothing: abrupt parameter changes must not produce clicks.
    let x = tone(200.0, -8.0, 1.2, FS);
    let mut c = Chain::new();
    let mut pp = ChainParams::flat();
    let mut y = Vec::new();
    for (i, ch) in x.chunks(FRAME).enumerate() {
        if i == 60 {
            pp.output.gain_db = -30.0;
            pp.eq.enabled = true;
            pp.eq.bands[2].gain_db = 12.0;
            pp.comp.enabled = true;
        }
        let mut b = ch.to_vec();
        c.process(&mut b, &pp);
        y.extend(b);
    }
    let max_step = y[FRAME * 5..]
        .windows(2)
        .fold(0.0f32, |a, w| a.max((w[1] - w[0]).abs()));
    let clean = db_to_lin(-8.0) * 2.0 * std::f32::consts::PI * 200.0 / 48_000.0;
    r.check(
        "parameter changes are click-free",
        max_step < clean * 1.3,
        format!("max step {max_step:.4} vs clean sine {clean:.4}"),
    );
}

fn sec_pipeline(r: &mut Report) {
    r.h("Engine pipeline (offline: resampler, ring, drift control)");
    let flat = ChainParams::flat();
    for (ri, ro) in [
        (48_000u32, 48_000u32),
        (44_100, 48_000),
        (96_000, 48_000),
        (48_000, 44_100),
    ] {
        let n = noise(-20.0, 1.0, ri as f64, 99);
        let y = pump_offline(&n, ri, ro, &flat);
        let delay_ms = find_delay(&resample(&n, ri as f64, ro as f64), &y, ro as usize / 2)
            .map(|d| d as f64 * 1000.0 / ro as f64);
        let t = tone(1000.0, -12.0, 3.0, ri as f64);
        let y = pump_offline(&t, ri, ro, &flat);
        let seg = tail(&y, 0.5);
        let g = amp_db(seg, ro as f64, 1000.0) + 12.0;
        let (thd, f_meas) = thdn_refined(seg, ro as f64, 1000.0);
        r.check(
            &format!("{ri} Hz in -> {ro} Hz out"),
            g.abs() < 0.3
                && thd < -55.0
                && (f_meas - 1000.0).abs() < 0.5
                && delay_ms.is_some_and(|d| d < 120.0),
            format!(
                "gain {g:+.2} dB, THD+N {thd:.1} dB, pitch error {:+.3} Hz, latency {} ms",
                f_meas - 1000.0,
                delay_ms.map_or("n/a".into(), |d| format!("{d:.1}"))
            ),
        );
    }
}

struct Devices {
    mic: Option<(String, String)>, // (id, name)
    cable_in: Option<String>,      // render
    cable_out: Option<String>,     // capture
}

fn sec_devices(r: &mut Report, dev: &DevCtl) -> (Devices, Option<DevState>) {
    r.h("Device discovery and capabilities");
    let st = match wait_for(dev, 8000, |s| s.revision > 0) {
        Some(s) => s,
        None => {
            r.check(
                "device enumeration completes",
                false,
                "no result within 8 s",
            );
            return (
                Devices {
                    mic: None,
                    cable_in: None,
                    cable_out: None,
                },
                None,
            );
        }
    };
    r.info(format!(
        "{} capture endpoints, {} render endpoints",
        st.captures.len(),
        st.renders.len()
    ));
    for c in &st.captures {
        r.info(format!(
            "capture: `{}`{}{}",
            c.name,
            if c.is_default { " (default)" } else { "" },
            if c.virtual_cable { " [virtual]" } else { "" }
        ));
    }
    for c in &st.renders {
        r.info(format!(
            "render: `{}`{}{}",
            c.name,
            if c.is_default { " (default)" } else { "" },
            if c.virtual_cable { " [virtual]" } else { "" }
        ));
    }
    let mic = st
        .captures
        .iter()
        .find(|c| c.name.to_lowercase().contains("at2020"))
        .map(|c| (c.id.clone(), c.name.clone()));
    r.check(
        "AT2020USB-X found",
        mic.is_some(),
        mic.as_ref().map_or("not present".into(), |m| m.1.clone()),
    );
    let cable_in = st
        .renders
        .iter()
        .find(|c| c.name.to_lowercase().contains("cable input"))
        .map(|c| c.name.clone());
    let cable_out = st
        .captures
        .iter()
        .find(|c| c.name.to_lowercase().contains("cable output"))
        .map(|c| c.name.clone());
    r.check(
        "virtual cable present",
        cable_in.is_some() && cable_out.is_some(),
        format!("input={cable_in:?} output={cable_out:?}"),
    );
    if let Some((id, name)) = &mic {
        if st.selected_id.as_deref() != Some(id.as_str()) {
            dev.send(Cmd::Select(id.clone()));
        }
        let s2 = wait_for(dev, 4000, |s| {
            s.selected_id.as_deref() == Some(id.as_str()) && s.has_endpoint
        })
        .unwrap_or(st.clone());
        r.info(format!(
            "endpoint volume: {:.0}% ({:.1} dB), range {:.1}..{:.1} dB, mute={}, enhancements disabled={:?}",
            s2.volume * 100.0,
            s2.volume_db,
            s2.db_min,
            s2.db_max,
            s2.mute,
            s2.sysfx_disabled
        ));
        r.check(
            "endpoint volume interface available",
            s2.has_endpoint,
            name.clone(),
        );
        r.check(
            "dB range reported",
            s2.db_max > s2.db_min,
            format!("{:.1}..{:.1}", s2.db_min, s2.db_max),
        );
        if let Ok(d) = find_dev(name, true) {
            if let Ok(it) = d.supported_input_configs() {
                for c in it {
                    r.info(format!(
                        "WASAPI input format: {} ch, {}-{} Hz, {:?}",
                        c.channels(),
                        c.min_sample_rate().0,
                        c.max_sample_rate().0,
                        c.sample_format()
                    ));
                }
            }
        }
        return (
            Devices {
                mic: mic.clone(),
                cable_in,
                cable_out,
            },
            Some(s2),
        );
    }
    (
        Devices {
            mic,
            cable_in,
            cable_out,
        },
        Some(st),
    )
}

fn sec_volume(r: &mut Report, dev: &DevCtl, st: &DevState) {
    r.h("Volume and mute round trip");
    if !st.has_endpoint {
        r.warn("no endpoint volume interface; skipped");
        return;
    }
    let (v0, m0) = (st.volume, st.mute);
    let mut last_db = f32::MIN;
    let mut monotonic = true;
    for target in [0.25f32, 0.5, 0.75] {
        dev.send(Cmd::SetVolume(target));
        let got = wait_for(dev, 2500, |s| (s.volume - target).abs() < 0.03);
        match got {
            Some(s) => {
                monotonic &= s.volume_db > last_db;
                last_db = s.volume_db;
                r.check(
                    &format!("set volume {:.0}%", target * 100.0),
                    true,
                    format!("read back {:.1}% / {:.1} dB", s.volume * 100.0, s.volume_db),
                );
            }
            None => r.check(
                &format!("set volume {:.0}%", target * 100.0),
                false,
                "did not read back within 2.5 s",
            ),
        }
    }
    r.check(
        "dB increases with scalar volume",
        monotonic,
        "monotonic over 25/50/75%",
    );
    for m in [!m0, m0] {
        dev.send(Cmd::SetMute(m));
        let ok = wait_for(dev, 2500, |s| s.mute == m).is_some();
        r.check(&format!("set mute={m}"), ok, "read back");
    }
    dev.send(Cmd::SetVolume(v0));
    let ok = wait_for(dev, 2500, |s| (s.volume - v0).abs() < 0.03 && s.mute == m0).is_some();
    r.check(
        "original volume and mute restored",
        ok,
        format!("{:.0}%, mute={m0}", v0 * 100.0),
    );
}

fn sec_live(r: &mut Report, d: &Devices, secs: u64) {
    r.h(&format!("Live capture with RNNoise ({secs} s)"));
    let (Some((_, mic)), Some(cin), Some(cout)) = (&d.mic, &d.cable_in, &d.cable_out) else {
        r.warn("mic or virtual cable missing; skipped");
        return;
    };
    let shared = Arc::new(Shared::new(ChainParams::voice()));
    let raw = Recorder::start(mic);
    let cap = Recorder::start(cout);
    let eng = Engine::start(mic, cin, None, &EngineConfig::default(), shared.clone());
    let eng = match eng {
        Ok(e) => e,
        Err(e) => {
            r.check("engine starts", false, format!("{e:#}"));
            return;
        }
    };
    r.info(format!(
        "engine: in {} Hz x{}, out {} Hz x{}, algorithmic latency {:.1} ms",
        eng.info.in_rate,
        eng.info.in_channels,
        eng.info.out_rate,
        eng.info.out_channels,
        eng.latency_ms()
    ));
    let t0 = Instant::now();
    let (mut vad_max, mut vad_sum, mut n) = (0.0f32, 0.0f32, 0u32);
    let mut gr_max = 0.0f32;
    while t0.elapsed().as_secs() < secs {
        std::thread::sleep(Duration::from_millis(100));
        let v = shared.meters.vad.load();
        vad_max = vad_max.max(v);
        vad_sum += v;
        gr_max = gr_max.max(shared.meters.gr_db.take());
        n += 1;
    }
    let in_pk = shared.meters.in_peak.take();
    let out_pk = shared.meters.out_peak.take();
    let fault = shared.take_fault();
    let unders = shared.underruns.load(Relaxed);
    let lat = eng.latency_ms();
    drop(eng);
    let raw_v = raw.map(|x| x.take_48k()).unwrap_or_default();
    let cap_v = cap.map(|x| x.take_48k()).unwrap_or_default();
    r.info(format!(
        "meters: in peak {:.1} dBFS, out peak {:.1} dBFS, max GR {gr_max:.1} dB, VAD mean {:.2} max {vad_max:.2}",
        lin_to_db(in_pk),
        lin_to_db(out_pk),
        vad_sum / n.max(1) as f32
    ));
    r.check("no stream fault", fault.is_none(), format!("{fault:?}"));
    r.check(
        "underruns after start-up <= 2",
        unders <= 2,
        format!("{unders}"),
    );
    r.check(
        "live latency estimate < 120 ms",
        lat < 120.0,
        format!("{lat:.1} ms"),
    );
    let (rr, cr) = (rms_db(&raw_v), rms_db(&cap_v));
    r.info(format!(
        "raw mic RMS {rr:.1} dBFS; cable capture RMS {cr:.1} dBFS ({:+.1} dB)",
        cr - rr
    ));
    let (live, detail) = mic_signal_check(&raw_v);
    r.check(
        "microphone delivers a signal (not digital silence)",
        live,
        detail,
    );
    r.check(
        "processed audio arrives at CABLE Output",
        cap_v.len() > 48_000 * (secs as usize / 2) && cr.is_finite(),
        format!("{} samples captured", cap_v.len()),
    );
    r.check(
        "capture has no NaN/inf",
        cap_v.iter().all(|v| v.is_finite()),
        "all finite",
    );
}

fn sec_loopback(r: &mut Report, d: &Devices) {
    r.h("VB-CABLE loopback");
    let (Some(cin), Some(cout)) = (&d.cable_in, &d.cable_out) else {
        r.warn("virtual cable missing; skipped");
        return;
    };
    // Latency with a noise burst.
    let burst = noise(-18.0, 0.6, FS, 1234);
    match loopback(cin, cout, &burst) {
        Ok((_, ms)) => {
            r.check(
                "cable render-to-capture latency < 150 ms",
                ms < 150.0,
                format!("{ms:.1} ms"),
            );
        }
        Err(e) => {
            r.check("loopback runs", false, format!("{e:#}"));
            return;
        }
    }
    // Frequency response with DSP bypassed.
    let freqs = [
        100.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 12_000.0, 16_000.0,
    ];
    let seg = (0.5 * FS) as usize;
    let mut stim = noise(-18.0, 0.3, FS, 5);
    for &f in &freqs {
        stim.extend(tone(f, -12.0, 0.5, FS));
    }
    match loopback(cin, cout, &stim) {
        Ok((cap, _)) => {
            let mut levels = Vec::new();
            for (i, &f) in freqs.iter().enumerate() {
                let a = (0.3 * FS) as usize + i * seg;
                let s = cap.get(a + seg / 5..a + seg - seg / 5);
                levels.push(s.map_or(-200.0, |s| amp_db(s, FS, f)));
            }
            let reff = levels[3];
            let dev: Vec<f32> = levels.iter().map(|l| l - reff).collect();
            let worst = dev.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            let line = freqs
                .iter()
                .zip(&dev)
                .map(|(f, v)| format!("{f:.0}:{v:+.2}"))
                .collect::<Vec<_>>()
                .join(" ");
            r.check(
                "flat response (DSP bypassed)",
                worst < 1.0,
                format!("worst {worst:.2} dB; {line}"),
            );
            r.check(
                "cable gain near unity",
                (reff + 12.0).abs() < 1.0,
                format!("1 kHz reads {:+.2} dB re -12 dBFS", reff + 12.0),
            );
        }
        Err(e) => r.check("response sweep", false, format!("{e:#}")),
    }
    // THD+N.
    match loopback(cin, cout, &tone(1000.0, -6.0, 3.0, FS)) {
        Ok((cap, _)) => {
            let s = &cap[(0.4 * FS) as usize..(2.6 * FS).min(cap.len() as f64) as usize];
            let t = thdn_db(s, FS, 1000.0);
            r.check(
                "cable THD+N @1 kHz -6 dBFS",
                t < -60.0,
                format!("{t:.1} dB"),
            );
        }
        Err(e) => r.check("THD+N", false, format!("{e:#}")),
    }
    // Each DSP stage's output survives the real device path.
    r.info("stage output generated offline, rendered through the cable, compared per tone");
    let tones = [100.0, 500.0, 1000.0, 3000.0, 8000.0];
    let mut base = Vec::new();
    for &f in &tones {
        let t = tone(f, -26.0, 4.0, FS);
        if base.is_empty() {
            base = t;
        } else {
            for (b, s) in base.iter_mut().zip(t) {
                *b += s;
            }
        }
    }
    let flat = ChainParams::flat();
    let mut stages: Vec<(&str, ChainParams)> = Vec::new();
    let mut p = flat;
    p.input.lowcut_on = true;
    p.input.lowcut_hz = 150.0;
    stages.push(("low cut 150 Hz", p));
    let mut p = flat;
    p.denoise.enabled = true;
    stages.push(("RNNoise", p));
    let mut p = flat;
    p.gate.enabled = true;
    p.gate.threshold_db = -40.0;
    stages.push(("gate", p));
    let mut p = flat;
    p.eq.enabled = true;
    p.eq.bands[3].gain_db = 6.0;
    p.eq.bands[3].freq = 3000.0;
    stages.push(("EQ +6 dB @3 kHz", p));
    let mut p = flat;
    p.comp.enabled = true;
    p.comp.threshold_db = -30.0;
    p.comp.ratio = 4.0;
    stages.push(("compressor", p));
    let mut p = flat;
    p.input.gain_db = 18.0;
    p.limiter.ceiling_db = -6.0;
    stages.push(("limiter", p));
    let mut p = flat;
    p.deess.enabled = true;
    p.deess.freq_hz = 8000.0;
    p.deess.threshold_db = -40.0;
    p.deess.max_reduction_db = 12.0;
    stages.push(("de-esser @8 kHz", p));
    let mut p = flat;
    p.vad_gate.enabled = true;
    p.vad_gate.threshold = 0.3;
    stages.push(("VAD gate", p));
    for (name, p) in stages {
        let sw = run_chain(&p, &base);
        let sw_levels: Vec<f32> = tones
            .iter()
            .map(|&f| amp_db(tail(&sw, 0.4), FS, f))
            .collect();
        match loopback(cin, cout, &sw) {
            Ok((cap, _)) => {
                let n = cap.len();
                let cap_levels: Vec<f32> = tones
                    .iter()
                    .map(|&f| amp_db(&cap[(n as f32 * 0.4) as usize..], FS, f))
                    .collect();
                let worst = sw_levels
                    .iter()
                    .zip(&cap_levels)
                    .filter(|(a, _)| **a > -75.0)
                    .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
                let eff = sw_levels
                    .iter()
                    .map(|l| format!("{l:.1}"))
                    .collect::<Vec<_>>()
                    .join("/");
                r.check(
                    &format!("stage via cable: {name}"),
                    worst < 1.0,
                    format!("tone levels dBFS {eff}; device-path error {worst:.2} dB"),
                );
            }
            Err(e) => r.check(&format!("stage via cable: {name}"), false, format!("{e:#}")),
        }
    }
}

fn sec_exclusive(r: &mut Report, d: &Devices) {
    r.h("WASAPI exclusive mode");
    let (Some((_, mic)), Some(cin)) = (&d.mic, &d.cable_in) else {
        r.warn("mic or virtual cable missing; skipped");
        return;
    };
    let shared = Arc::new(Shared::new(ChainParams::voice()));
    let cfg = EngineConfig {
        exclusive: true,
        ..EngineConfig::default()
    };
    match Engine::start(mic, cin, None, &cfg, shared.clone()) {
        Ok(e) => {
            let i = e.info.clone();
            r.info(format!(
                "input: {} Hz x{} {} {} frames ({}); output: {} Hz x{} {} {} frames ({})",
                i.in_rate,
                i.in_channels,
                i.in_format,
                i.in_buffer.unwrap_or(0),
                if i.in_exclusive {
                    "exclusive"
                } else {
                    "shared"
                },
                i.out_rate,
                i.out_channels,
                i.out_format,
                i.out_buffer.unwrap_or(0),
                if i.out_exclusive {
                    "exclusive"
                } else {
                    "shared"
                },
            ));
            for n in &i.notes {
                r.info(format!("note: {n}"));
            }
            std::thread::sleep(Duration::from_secs(5));
            let unders = shared.underruns.load(Relaxed);
            let fault = shared.take_fault();
            let inp = shared.meters.in_peak.take();
            let out = shared.meters.out_peak.take();
            let lat = e.latency_ms();
            drop(e);
            r.check(
                "exclusive engine starts (or falls back)",
                true,
                "running 5 s",
            );
            r.check(
                "no stream fault in exclusive mode",
                fault.is_none(),
                format!("{fault:?}"),
            );
            r.check("exclusive underruns <= 2", unders <= 2, format!("{unders}"));
            let (flows, detail) = exclusive_signal_check(inp);
            r.check(
                "signal flows (input not digital silence)",
                flows,
                format!("{detail}; out peak {:.1} dBFS", lin_to_db(out)),
            );
            r.info(format!("estimated latency {lat:.1} ms"));
        }
        Err(e) => r.check(
            "exclusive engine starts (or falls back)",
            false,
            format!("{e:#}"),
        ),
    }
}

fn sec_soak(r: &mut Report, d: &Devices, secs: u64) {
    r.h(&format!("Soak ({} min)", secs as f64 / 60.0));
    let (Some((_, mic)), Some(cin)) = (&d.mic, &d.cable_in) else {
        r.warn("mic or virtual cable missing; skipped");
        return;
    };
    if secs == 0 {
        r.warn("soak disabled");
        return;
    }
    let shared = Arc::new(Shared::new(ChainParams::voice()));
    let eng = match Engine::start(mic, cin, None, &EngineConfig::default(), shared.clone()) {
        Ok(e) => e,
        Err(e) => {
            r.check("engine starts", false, format!("{e:#}"));
            return;
        }
    };
    std::thread::sleep(Duration::from_secs(3)); // let start-up settle
    let base_unders = shared.underruns.load(Relaxed);
    let (cpu0, t0) = (procinfo::cpu_seconds(), Instant::now());
    let mem0 = procinfo::working_set_mib();
    let (mut fill_min, mut fill_max, mut fill_sum, mut samples) =
        (usize::MAX, 0usize, 0usize, 0usize);
    let mut cpu_max = 0.0f64;
    let (mut last_cpu, mut last_t) = (cpu0, t0);
    let ncpu = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let mut fault = None;
    let mut next_report = 600u64;
    while t0.elapsed().as_secs() < secs {
        std::thread::sleep(Duration::from_secs(2));
        let f = shared.out_fill.load(Relaxed);
        fill_min = fill_min.min(f);
        fill_max = fill_max.max(f);
        fill_sum += f;
        samples += 1;
        let (c, t) = (procinfo::cpu_seconds(), Instant::now());
        let dt = (t - last_t).as_secs_f64().max(1e-3);
        cpu_max = cpu_max.max((c - last_cpu) / dt / ncpu * 100.0);
        (last_cpu, last_t) = (c, t);
        shared.meters.gr_db.take();
        shared.meters.in_peak.take();
        shared.meters.out_peak.take();
        if let Some(e) = shared.take_fault() {
            fault = Some(e);
            break;
        }
        if t0.elapsed().as_secs() >= next_report {
            next_report += 600;
            println!(
                "   soak {:.0} min: underruns {}, ws {:.1} MiB",
                t0.elapsed().as_secs_f64() / 60.0,
                shared.underruns.load(Relaxed) - base_unders,
                procinfo::working_set_mib()
            );
        }
    }
    let wall = t0.elapsed().as_secs_f64();
    let cpu_avg = (procinfo::cpu_seconds() - cpu0) / wall / ncpu * 100.0;
    let mem1 = procinfo::working_set_mib();
    let unders = shared.underruns.load(Relaxed) - base_unders;
    drop(eng);
    r.info(format!(
        "ran {:.1} min; ring fill min/avg/max {}/{}/{} samples; CPU avg {cpu_avg:.2}% max {cpu_max:.2}% (of {ncpu:.0} logical CPUs)",
        wall / 60.0,
        if fill_min == usize::MAX { 0 } else { fill_min },
        fill_sum / samples.max(1),
        fill_max
    ));
    r.check("no stream fault", fault.is_none(), format!("{fault:?}"));
    r.check("xruns (output underruns)", unders == 0, format!("{unders}"));
    r.check(
        "CPU average < 10%",
        cpu_avg < 10.0,
        format!("{cpu_avg:.2}%"),
    );
    r.check(
        "memory growth < 25 MiB",
        mem1 - mem0 < 25.0,
        format!("{mem0:.1} -> {mem1:.1} MiB"),
    );
}

fn sec_hotplug(r: &mut Report, dev: &DevCtl, d: &Devices) {
    r.h("Hot-plug and failure handler paths");
    let Some((id, name)) = &d.mic else {
        r.warn("mic missing; skipped");
        return;
    };
    // 1. Burst of change notifications is debounced and harmless.
    let rev0 = dev.snapshot().revision;
    for _ in 0..60 {
        dev.send(Cmd::Refresh);
    }
    let after = wait_for(dev, 3000, |s| s.revision > rev0);
    let rev1 = after.as_ref().map_or(rev0, |s| s.revision);
    std::thread::sleep(Duration::from_millis(800));
    let rev2 = dev.snapshot().revision;
    r.check(
        "refresh burst coalesced",
        after.is_some() && rev2 - rev0 <= 5,
        format!("60 notifications -> {} refreshes", rev2 - rev0),
    );
    let _ = rev1;
    // 2. Selected device disappears.
    dev.send(Cmd::Select(
        "{0.0.1.00000000}.{00000000-0000-0000-0000-000000000000}".into(),
    ));
    let gone = wait_for(dev, 3000, |s| !s.has_endpoint && s.message.is_some());
    r.check(
        "missing device reported, no endpoint bound",
        gone.is_some(),
        gone.as_ref()
            .and_then(|s| s.message.clone())
            .unwrap_or_else(|| "no message".into()),
    );
    // 3. Device comes back.
    dev.send(Cmd::Select(id.clone()));
    let back = wait_for(dev, 3000, |s| {
        s.has_endpoint && s.selected_id.as_deref() == Some(id)
    });
    r.check("device re-bound after return", back.is_some(), name.clone());
    // 4. Engine start on a missing device fails cleanly.
    let shared = Arc::new(Shared::new(ChainParams::default()));
    let e = Engine::start(
        "No Such Microphone",
        d.cable_in.as_deref().unwrap_or("x"),
        None,
        &EngineConfig::default(),
        shared.clone(),
    );
    r.check(
        "engine start on missing input -> Err",
        e.is_err(),
        e.err().map_or(String::new(), |e| format!("{e:#}")),
    );
    // 5. Fault flag round trip and engine restart.
    shared.set_fault("synthetic fault".into());
    r.check(
        "fault flag take/clear",
        shared.take_fault().as_deref() == Some("synthetic fault") && shared.take_fault().is_none(),
        "set -> take -> empty",
    );
    if let Some(cin) = &d.cable_in {
        let mut ok = true;
        for _ in 0..3 {
            match Engine::start(name, cin, None, &EngineConfig::default(), shared.clone()) {
                Ok(e) => {
                    std::thread::sleep(Duration::from_millis(400));
                    drop(e);
                }
                Err(_) => ok = false,
            }
        }
        r.check(
            "engine stop/start x3 (device-change restart path)",
            ok,
            "no failure",
        );
    }
}

// ------------------------------------------------------------------ main ---

pub fn run(opts: &Options) -> i32 {
    let started = Instant::now();
    let mut r = Report::default();
    let dev = DevCtl::spawn(Arc::new(|| {}));

    sec_dsp(&mut r);
    sec_pipeline(&mut r);

    let (devices, st) = sec_devices(&mut r, &dev);
    let before = st.clone();
    let guard = Restore {
        dev: &dev,
        id: st.as_ref().and_then(|s| s.selected_id.clone()),
        volume: st.as_ref().map_or(1.0, |s| s.volume),
        mute: st.as_ref().is_some_and(|s| s.mute),
        active: st.as_ref().is_some_and(|s| s.has_endpoint),
    };
    if let Some(s) = &st {
        sec_volume(&mut r, &dev, s);
    }
    sec_hotplug(&mut r, &dev, &devices);
    sec_live(&mut r, &devices, opts.live_secs);
    sec_loopback(&mut r, &devices);
    sec_exclusive(&mut r, &devices);
    sec_soak(&mut r, &devices, opts.soak_secs);

    // Final verification that nothing was left changed.
    r.h("System state restored");
    let (id, vol, mute) = (guard.id.clone(), guard.volume, guard.mute);
    drop(guard);
    if let Some(b) = before {
        let a = wait_for(&dev, 4000, |s| {
            s.selected_id == id && (s.volume - vol).abs() < 0.03 && s.mute == mute
        })
        .unwrap_or_else(|| dev.snapshot());
        r.check(
            "mic volume and mute",
            (a.volume - b.volume).abs() < 0.03 && a.mute == b.mute,
            format!(
                "{:.0}% mute={} (was {:.0}% mute={})",
                a.volume * 100.0,
                a.mute,
                b.volume * 100.0,
                b.mute
            ),
        );
        r.check(
            "enhancement flag unchanged",
            a.sysfx_disabled == b.sysfx_disabled,
            format!("{:?} (was {:?})", a.sysfx_disabled, b.sysfx_disabled),
        );
        let defs = |s: &DevState| -> (Vec<String>, Vec<String>) {
            (
                s.captures
                    .iter()
                    .filter(|c| c.is_default)
                    .map(|c| c.id.clone())
                    .collect(),
                s.renders
                    .iter()
                    .filter(|c| c.is_default)
                    .map(|c| c.id.clone())
                    .collect(),
            )
        };
        r.check(
            "default devices unchanged",
            defs(&a) == defs(&b),
            "capture and render defaults identical",
        );
    }

    let mut out = String::new();
    let _ = writeln!(out, "# SELFTEST REPORT\n");
    let _ = writeln!(
        out,
        "Generated {} - run time {:.1} min - {} passed, {} failed, {} warnings\n",
        utc_now(),
        started.elapsed().as_secs_f64() / 60.0,
        r.pass,
        r.fail,
        r.warn
    );
    out.push_str(&r.body);
    let wrote =
        std::fs::write(&opts.out_path, out).with_context(|| format!("writing {}", opts.out_path));
    if let Err(e) = wrote {
        eprintln!("{e:#}");
        return 2;
    }
    println!(
        "\n{} passed, {} failed, {} warnings -> {}",
        r.pass, r.fail, r.warn, opts.out_path
    );
    i32::from(r.fail > 0)
}

/// Worst THD+N over 0.2 s windows, with the fit frequency refined within +/-0.3 Hz.
/// The engine's clock-drift compensation makes ppm-level pitch changes; over a long fixed
/// fit those read as noise, in short windows they do not. Returns (THD+N dB, mean freq).
fn thdn_refined(x: &[f32], fs: f64, f0: f64) -> (f32, f64) {
    let win = (0.2 * fs) as usize;
    let (mut worst, mut fsum, mut n) = (f64::MIN, 0.0, 0.0);
    for w in x.chunks_exact(win) {
        let mut best = (f64::MAX, f0);
        let mut df = -0.3;
        while df <= 0.3001 {
            let (a, r) = fit_sine(w, fs, f0 + df);
            let ratio = r / (a / std::f64::consts::SQRT_2 + 1e-12);
            if ratio < best.0 {
                best = (ratio, f0 + df);
            }
            df += 0.02;
        }
        worst = worst.max(20.0 * (best.0 + 1e-12).log10());
        fsum += best.1;
        n += 1.0;
    }
    (worst as f32, fsum / f64::max(n, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digital_silence_fails_the_mic_check() {
        let (ok, detail) = mic_signal_check(&vec![0.0; 48_000]);
        assert!(!ok, "exact zeros must not pass: {detail}");
    }

    #[test]
    fn room_noise_passes_the_mic_check() {
        let (ok, detail) = mic_signal_check(&noise(-55.0, 1.0, FS, 7));
        assert!(ok, "{detail}");
    }

    #[test]
    fn exclusive_check_fails_on_silence() {
        assert!(!exclusive_signal_check(0.0).0);
        assert!(exclusive_signal_check(db_to_lin(-40.0)).0);
    }
}
