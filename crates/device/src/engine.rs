//! WASAPI (through cpal) capture -> processing chain -> render engine.
//!
//! Input callback  : any rate/channels/format -> mono f32 -> 48 kHz -> chain (480-sample frames)
//!                   -> SPSC ring(s).
//! Output callback : ring -> cubic resampler with drift compensation -> device rate/channels.
//! Windows cannot create a virtual microphone without a kernel driver, so the
//! processed signal is rendered to a *virtual cable* playback device (e.g. VB-CABLE's
//! "CABLE Input"); other apps then pick the matching recording device ("CABLE Output").
//!
//! Real-time rules for the callbacks: no allocation (all buffers are sized up front),
//! no locks (parameters arrive through a triple buffer, meters are atomics), no logging.
//! The single exception is a one-time MMCSS "Pro Audio" registration on each callback
//! thread's first invocation, which is the only point where that thread is available.

use crate::shared::{Shared, REC_RAW};
use anyhow::{anyhow, bail, Context, Result};
use core_dsp::dsp::{Biquad, Chain, Coeffs};
use core_dsp::params::*;
use core_dsp::resample::Resampler;
use core_dsp::ring::Ring;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig,
    SupportedBufferSize,
};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use triple_buffer::Output;

use crate::wasapi_excl::{self, ExclusiveStream, Fault};

const RING_CAP: usize = 1 << 16; // ~1.36 s @ 48 kHz
/// Target ring fill used for drift compensation / underrun protection (~30 ms).
const TARGET_FILL: usize = 1440;
/// Scratch capacity: covers any realistic device period (3 s of 192 kHz audio).
const SCRATCH: usize = 1 << 19;

/// User-selectable stream parameters. `None` = whatever the device reports as default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineConfig {
    pub sample_rate: Option<u32>,
    pub buffer_frames: Option<u32>,
    /// Try WASAPI exclusive mode (falls back to shared mode per side, with a note).
    pub exclusive: bool,
}

#[derive(Clone, Debug)]
pub struct EngineInfo {
    pub in_rate: u32,
    pub in_channels: u16,
    pub in_format: String,
    pub in_buffer: Option<u32>,
    pub out_rate: u32,
    pub out_channels: u16,
    pub out_format: String,
    pub out_buffer: Option<u32>,
    pub in_exclusive: bool,
    pub out_exclusive: bool,
    /// Human-readable remarks, e.g. why exclusive mode fell back to shared.
    pub notes: Vec<String>,
}

/// A running stream: cpal (shared mode) or our own exclusive-mode thread.
pub enum Handle {
    Cpal(Stream),
    Excl(#[allow(dead_code)] ExclusiveStream),
}

impl Handle {
    fn play(&self) -> Result<()> {
        if let Handle::Cpal(s) = self {
            s.play()?;
        }
        Ok(())
    }
}

pub struct Engine {
    _input: Handle,
    _output: Handle,
    _monitor: Option<Stream>,
    pub info: EngineInfo,
    shared: Arc<Shared>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.monitor_active.store(false, Relaxed);
    }
}

/// What a device reports it can do (read from WASAPI, never assumed).
#[derive(Clone, Debug, Default)]
pub struct DeviceCaps {
    pub default_rate: u32,
    pub default_channels: u16,
    pub rates: Vec<u32>,
    pub formats: Vec<String>,
    pub buffer_range: Option<(u32, u32)>,
}

const COMMON_RATES: [u32; 9] = [
    8_000, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 192_000,
];

pub fn query_caps(name: &str, input: bool) -> Option<DeviceCaps> {
    let host = cpal::default_host();
    let dev = find_device_in(&host, name, input).ok()?;
    let def = if input {
        dev.default_input_config().ok()?
    } else {
        dev.default_output_config().ok()?
    };
    let ranges: Vec<cpal::SupportedStreamConfigRange> = if input {
        dev.supported_input_configs().ok()?.collect()
    } else {
        dev.supported_output_configs().ok()?.collect()
    };
    let mut caps = DeviceCaps {
        default_rate: def.sample_rate().0,
        default_channels: def.channels(),
        ..Default::default()
    };
    for r in &ranges {
        for hz in COMMON_RATES {
            if hz >= r.min_sample_rate().0
                && hz <= r.max_sample_rate().0
                && !caps.rates.contains(&hz)
            {
                caps.rates.push(hz);
            }
        }
        let f = format!("{:?}", r.sample_format());
        if !caps.formats.contains(&f) {
            caps.formats.push(f);
        }
        if let SupportedBufferSize::Range { min, max } = r.buffer_size() {
            let (lo, hi) = caps.buffer_range.unwrap_or((*min, *max));
            caps.buffer_range = Some((lo.min(*min), hi.max(*max)));
        }
    }
    caps.rates.sort_unstable();
    if !caps.rates.contains(&caps.default_rate) {
        caps.rates.push(caps.default_rate);
        caps.rates.sort_unstable();
    }
    Some(caps)
}

fn find_device_in(host: &cpal::Host, name: &str, input: bool) -> Result<cpal::Device> {
    let devices: Vec<cpal::Device> = if input {
        host.input_devices()?.collect()
    } else {
        host.output_devices()?.collect()
    };
    devices
        .into_iter()
        .find(|d| d.name().map(|n| n == name).unwrap_or(false))
        .ok_or_else(|| {
            anyhow!(
                "{} device \"{}\" not found (is it plugged in / enabled?)",
                if input { "Input" } else { "Output" },
                name
            )
        })
}

/// Chooses the stream configuration: the device default, overridden by the requested
/// rate / buffer size where the device reports support for them.
fn pick_config(
    dev: &cpal::Device,
    input: bool,
    cfg: &EngineConfig,
) -> Result<(StreamConfig, SampleFormat, Option<u32>)> {
    let def = if input {
        dev.default_input_config()?
    } else {
        dev.default_output_config()?
    };
    let mut chosen = def.clone();
    if let Some(want) = cfg.sample_rate {
        if want != def.sample_rate().0 {
            let ranges: Vec<cpal::SupportedStreamConfigRange> = if input {
                dev.supported_input_configs()?.collect()
            } else {
                dev.supported_output_configs()?.collect()
            };
            let hit = ranges
                .into_iter()
                .filter(|r| {
                    r.min_sample_rate().0 <= want
                        && want <= r.max_sample_rate().0
                        && r.channels() == def.channels()
                })
                .max_by_key(|r| match r.sample_format() {
                    SampleFormat::F32 => 3,
                    SampleFormat::I32 => 2,
                    SampleFormat::I16 => 1,
                    _ => 0,
                });
            if let Some(r) = hit {
                chosen = r.with_sample_rate(cpal::SampleRate(want));
            }
        }
    }
    let mut sc: StreamConfig = chosen.config();
    let mut buf = None;
    if let (Some(n), SupportedBufferSize::Range { min, max }) =
        (cfg.buffer_frames, chosen.buffer_size())
    {
        if n >= *min && n <= *max {
            sc.buffer_size = BufferSize::Fixed(n);
            buf = Some(n);
        }
    }
    Ok((sc, chosen.sample_format(), buf))
}

impl Engine {
    pub fn start(
        input_name: &str,
        output_name: &str,
        monitor_name: Option<&str>,
        cfg: &EngineConfig,
        shared: Arc<Shared>,
    ) -> Result<Engine> {
        let host = cpal::default_host();
        let in_dev = find_device_in(&host, input_name, true)?;
        let out_dev = find_device_in(&host, output_name, false)?;

        let main_ring = Arc::new(Ring::new(RING_CAP));
        let mon_ring = Arc::new(Ring::new(RING_CAP));

        let mut notes = Vec::new();
        if cfg.exclusive {
            // Exclusive mode pre-empts other apps' shared streams on the same endpoint
            // (Windows default "give exclusive mode applications priority"); say who.
            let me = std::process::id();
            for (name, capture) in [(input_name, true), (output_name, false)] {
                let s = crate::sessions::endpoint_sessions(name, capture).unwrap_or_default();
                let p = crate::sessions::preempted_by_exclusive(&s, me);
                if !p.is_empty() {
                    notes.push(format!(
                        "Exclusive mode on \"{name}\" stops the audio of {} until they restart their stream",
                        crate::sessions::describe(&p)
                    ));
                }
            }
        }
        let exclusive_out = if cfg.exclusive {
            match build_output_exclusive(output_name, main_ring.clone(), shared.clone(), cfg) {
                Ok(v) => Some(v),
                Err(e) => {
                    notes.push(format!(
                        "Output: exclusive mode unavailable, using shared ({e:#})"
                    ));
                    None
                }
            }
        } else {
            None
        };
        let (output, out_info) = match exclusive_out {
            Some(v) => v,
            None => {
                let (s, i) = build_output(&out_dev, main_ring.clone(), shared.clone(), false, cfg)
                    .context("opening output device")?;
                (Handle::Cpal(s), i)
            }
        };
        let monitor = match monitor_name {
            Some(n) => {
                let d = find_device_in(&host, n, false)?;
                let (s, _) = build_output(&d, mon_ring.clone(), shared.clone(), true, cfg)
                    .context("opening monitor device")?;
                Some(s)
            }
            None => None,
        };
        shared.monitor_active.store(monitor.is_some(), Relaxed);
        let exclusive_in = if cfg.exclusive {
            match build_input_exclusive(
                input_name,
                main_ring.clone(),
                mon_ring.clone(),
                shared.clone(),
                cfg,
            ) {
                Ok(v) => Some(v),
                Err(e) => {
                    notes.push(format!(
                        "Input: exclusive mode unavailable, using shared ({e:#})"
                    ));
                    None
                }
            }
        } else {
            None
        };
        let (input, in_info) = match exclusive_in {
            Some(v) => v,
            None => {
                let (s, i) = build_input(&in_dev, main_ring, mon_ring, shared.clone(), cfg)
                    .context("opening input device")?;
                (Handle::Cpal(s), i)
            }
        };

        output.play().context("starting output stream")?;
        if let Some(m) = &monitor {
            m.play().context("starting monitor stream")?;
        }
        input.play().context("starting input stream")?;

        Ok(Engine {
            _input: input,
            _output: output,
            info: EngineInfo {
                in_rate: in_info.rate,
                in_channels: in_info.channels,
                in_format: in_info.format,
                in_buffer: in_info.buffer,
                out_rate: out_info.rate,
                out_channels: out_info.channels,
                out_format: out_info.format,
                out_buffer: out_info.buffer,
                in_exclusive: in_info.exclusive,
                out_exclusive: out_info.exclusive,
                notes,
            },
            _monitor: monitor,
            shared,
        })
    }

    /// Estimated end-to-end latency in ms: chain + queue + (when fixed) device buffers.
    pub fn latency_ms(&self) -> f32 {
        let proc =
            (Chain::LATENCY_SAMPLES + FRAME / 2 + self.shared.out_fill.load(Relaxed)) as f32 / 48.0;
        let dev = |b: Option<u32>, r: u32| b.map_or(0.0, |b| b as f32 * 1000.0 / r.max(1) as f32);
        proc + dev(self.info.in_buffer, self.info.in_rate)
            + dev(self.info.out_buffer, self.info.out_rate)
    }
}

struct StreamInfo {
    rate: u32,
    channels: u16,
    format: String,
    buffer: Option<u32>,
    exclusive: bool,
}

// ------------------------------------------------------------------ mmcss --

/// Registers the calling thread with MMCSS ("Pro Audio") once. Failure is harmless.
fn promote_thread(done: &mut bool, shared: &Shared) {
    if *done {
        return;
    }
    *done = true;
    let mut idx = 0u32;
    // SAFETY: plain FFI call with a valid out-pointer; the task handle is intentionally
    // kept for the lifetime of the callback thread.
    let ok = unsafe {
        windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW(
            windows::core::w!("Pro Audio"),
            &mut idx,
        )
    }
    .is_ok();
    if ok {
        shared.mmcss_threads.fetch_add(1, Relaxed);
    }
}

// ------------------------------------------------------- exclusive mode --

fn fault_cb(shared: &Arc<Shared>) -> Fault {
    let sh = shared.clone();
    Arc::new(move |m: String| sh.set_fault(m))
}

fn build_input_exclusive(
    name: &str,
    main: Arc<Ring>,
    mon: Arc<Ring>,
    shared: Arc<Shared>,
    cfg: &EngineConfig,
) -> Result<(Handle, StreamInfo)> {
    let fault = fault_cb(&shared);
    let (stream, neg) = wasapi_excl::open_capture(
        name,
        cfg.sample_rate,
        move |n| {
            let mut proc = InputProcessor::new(n.rate, n.channels as usize, main, mon, shared);
            move |d: &[f32]| proc.process(d)
        },
        fault,
    )?;
    Ok((
        Handle::Excl(stream),
        StreamInfo {
            rate: neg.rate,
            channels: neg.channels,
            format: neg.format,
            buffer: Some(neg.buffer_frames),
            exclusive: true,
        },
    ))
}

fn build_output_exclusive(
    name: &str,
    ring: Arc<Ring>,
    shared: Arc<Shared>,
    cfg: &EngineConfig,
) -> Result<(Handle, StreamInfo)> {
    let fault = fault_cb(&shared);
    let (stream, neg) = wasapi_excl::open_render(
        name,
        cfg.sample_rate,
        move |n| {
            let mut proc = OutputProcessor {
                ring,
                rs: Resampler::new(),
                ch: n.channels as usize,
                out_rate: n.rate as f64,
                primed: false,
                adj: 0.0,
                fill_avg: TARGET_FILL as f64,
                is_monitor: false,
                shared,
                mmcss: true,
            };
            move |d: &mut [f32]| proc.fill(d)
        },
        fault,
    )?;
    Ok((
        Handle::Excl(stream),
        StreamInfo {
            rate: neg.rate,
            channels: neg.channels,
            format: neg.format,
            buffer: Some(neg.buffer_frames),
            exclusive: true,
        },
    ))
}

// ------------------------------------------------------------------ input --

struct InputProcessor {
    ch: usize,
    step: f64,
    lpf: Option<Biquad>,
    rs: Resampler,
    mono: Vec<f32>,
    acc: Vec<f32>,
    chain: Chain,
    params: Output<ChainParams>,
    shared: Arc<Shared>,
    main: Arc<Ring>,
    mon: Arc<Ring>,
    mmcss: bool,
}

impl InputProcessor {
    fn new(rate: u32, ch: usize, main: Arc<Ring>, mon: Arc<Ring>, shared: Arc<Shared>) -> Self {
        let lpf = if rate > 48_000 {
            Some(Biquad::new(Coeffs::lowpass(
                rate as f64,
                21_000.0,
                std::f64::consts::FRAC_1_SQRT_2,
            )))
        } else {
            None
        };
        Self {
            ch: ch.max(1),
            step: rate as f64 / SAMPLE_RATE as f64,
            lpf,
            rs: Resampler::new(),
            mono: Vec::with_capacity(SCRATCH),
            acc: Vec::with_capacity(SCRATCH),
            chain: Chain::new(),
            params: shared.params.attach(),
            shared,
            main,
            mon,
            mmcss: false,
        }
    }

    fn process<T: Sample>(&mut self, data: &[T])
    where
        f32: FromSample<T>,
    {
        promote_thread(&mut self.mmcss, &self.shared);
        // Newest published parameters; never blocks.
        let local: ChainParams = *self.params.read();

        let ch = self.ch;
        let mode = local.input.channel;
        self.mono.clear();
        if self.mono.capacity() < data.len() / ch {
            return; // absurdly large buffer: drop rather than allocate
        }
        for fr in data.chunks_exact(ch) {
            let s = match mode {
                ChannelMode::Left => fr[0].to_sample::<f32>(),
                ChannelMode::Right => fr[ch.min(2) - 1].to_sample::<f32>(),
                ChannelMode::Mix => {
                    let mut a = 0.0f32;
                    for x in fr {
                        a += x.to_sample::<f32>();
                    }
                    a / ch as f32
                }
            };
            self.mono.push(s);
        }
        if let Some(l) = &mut self.lpf {
            for x in self.mono.iter_mut() {
                *x = l.process(*x as f64) as f32;
            }
        }

        if (self.step - 1.0).abs() < 1e-9 {
            let room = self.acc.capacity() - self.acc.len();
            let n = self.mono.len().min(room);
            self.acc.extend_from_slice(&self.mono[..n]);
        } else {
            let mut it = self.mono.iter().copied();
            let acc = &mut self.acc;
            self.rs.run(self.step, &mut || it.next(), &mut |y| {
                if acc.len() < acc.capacity() {
                    acc.push(y);
                }
                true
            });
        }

        let mon_on = self.shared.monitor_active.load(Relaxed);
        let rec_on = self.shared.rec_active.load(Relaxed);
        let rec_raw = self.shared.rec_source.load(Relaxed) == REC_RAW;
        let mut consumed = 0usize;
        while self.acc.len() - consumed >= FRAME {
            let frame = &mut self.acc[consumed..consumed + FRAME];
            self.shared.spec_in.push(frame);
            if rec_on && rec_raw {
                self.shared.rec_ring.push(frame);
            }
            let st = self.chain.process(frame, &local);
            let m = &self.shared.meters;
            m.in_peak.fetch_max(st.in_peak);
            m.out_peak.fetch_max(st.out_peak);
            m.gr_db.fetch_max(st.gr_db);
            m.deess_gr_db.fetch_max(st.deess_gr_db);
            m.vad.store(st.vad);
            m.gate_open.store(st.gate_open, Relaxed);
            m.vad_gate_open.store(st.vad_gate_open, Relaxed);
            self.main.push(frame);
            self.shared.spec_out.push(frame);
            if rec_on && !rec_raw {
                self.shared.rec_ring.push(frame);
            }
            if mon_on {
                self.mon.push(frame);
            }
            consumed += FRAME;
        }
        if consumed > 0 {
            self.acc.drain(..consumed);
        }
    }
}

fn build_input(
    dev: &cpal::Device,
    main: Arc<Ring>,
    mon: Arc<Ring>,
    shared: Arc<Shared>,
    cfg: &EngineConfig,
) -> Result<(Stream, StreamInfo)> {
    let (sc, fmt, buf) = pick_config(dev, true, cfg)?;
    let (rate, ch) = (sc.sample_rate.0, sc.channels);
    let proc = InputProcessor::new(rate, ch as usize, main, mon, shared.clone());
    let es = shared.clone();
    let err = move |e: cpal::StreamError| es.set_fault(format!("Input stream error: {e}"));
    let stream = match fmt {
        SampleFormat::F32 => build_in::<f32>(dev, &sc, proc, err)?,
        SampleFormat::I16 => build_in::<i16>(dev, &sc, proc, err)?,
        SampleFormat::I32 => build_in::<i32>(dev, &sc, proc, err)?,
        SampleFormat::U16 => build_in::<u16>(dev, &sc, proc, err)?,
        f => bail!("unsupported input sample format {f:?}"),
    };
    Ok((
        stream,
        StreamInfo {
            rate,
            channels: ch,
            format: format!("{fmt:?}"),
            buffer: buf,
            exclusive: false,
        },
    ))
}

fn build_in<T>(
    dev: &cpal::Device,
    cfg: &StreamConfig,
    mut proc: InputProcessor,
    err: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    Ok(dev.build_input_stream(
        cfg,
        move |d: &[T], _: &cpal::InputCallbackInfo| proc.process(d),
        err,
        None,
    )?)
}

// ----------------------------------------------------------------- output --

struct OutputProcessor {
    ring: Arc<Ring>,
    rs: Resampler,
    ch: usize,
    out_rate: f64,
    primed: bool,
    adj: f64,
    fill_avg: f64,
    is_monitor: bool,
    shared: Arc<Shared>,
    mmcss: bool,
}

impl OutputProcessor {
    fn fill<T>(&mut self, data: &mut [T])
    where
        T: Sample + FromSample<f32>,
    {
        promote_thread(&mut self.mmcss, &self.shared);
        let ch = self.ch;
        let frames = data.len() / ch;
        let fill = self.ring.len();
        if !self.is_monitor {
            self.shared.out_fill.store(fill, Relaxed);
        }
        let silence = T::from_sample(0.0f32);
        if !self.primed {
            if fill >= TARGET_FILL {
                self.primed = true;
                self.fill_avg = fill as f64;
            } else {
                data.fill(silence);
                return;
            }
        }
        // Clock-drift compensation: nudge the resampling ratio toward the target fill.
        // The measured fill is smoothed first: callbacks deliver bursts, and feeding that
        // straight into the ratio would modulate pitch.
        self.fill_avg += (fill as f64 - self.fill_avg) * 0.02;
        let err = (self.fill_avg - TARGET_FILL as f64) / SAMPLE_RATE as f64;
        let want = (err * 0.5).clamp(-0.002, 0.002);
        self.adj += (want - self.adj) * 0.01;
        let step = (SAMPLE_RATE as f64 / self.out_rate) * (1.0 + self.adj);

        let vol = if self.is_monitor {
            self.shared.monitor_vol.load()
        } else {
            1.0
        };
        let ring = &self.ring;
        let mut next = || {
            let mut b = [0.0f32; 1];
            if ring.pop(&mut b) == 1 {
                Some(b[0])
            } else {
                None
            }
        };
        let mut n = 0usize;
        self.rs.run(step, &mut next, &mut |y| {
            let v = T::from_sample((y * vol).clamp(-1.0, 1.0));
            for c in 0..ch {
                data[n * ch + c] = v;
            }
            n += 1;
            n < frames
        });
        if n < frames {
            for s in data[n * ch..].iter_mut() {
                *s = silence;
            }
            self.primed = false; // underrun: rebuild the cushion before resuming
            if !self.is_monitor {
                self.shared.underruns.fetch_add(1, Relaxed);
            }
        }
    }
}

fn build_output(
    dev: &cpal::Device,
    ring: Arc<Ring>,
    shared: Arc<Shared>,
    is_monitor: bool,
    cfg: &EngineConfig,
) -> Result<(Stream, StreamInfo)> {
    let (sc, fmt, buf) = pick_config(dev, false, cfg)?;
    let (rate, ch) = (sc.sample_rate.0, sc.channels);
    let proc = OutputProcessor {
        ring,
        rs: Resampler::new(),
        ch: ch as usize,
        out_rate: rate as f64,
        primed: false,
        adj: 0.0,
        fill_avg: TARGET_FILL as f64,
        is_monitor,
        shared: shared.clone(),
        mmcss: false,
    };
    let es = shared.clone();
    let err = move |e: cpal::StreamError| {
        es.set_fault(format!(
            "{} stream error: {e}",
            if is_monitor { "Monitor" } else { "Output" }
        ))
    };
    let stream = match fmt {
        SampleFormat::F32 => build_out::<f32>(dev, &sc, proc, err)?,
        SampleFormat::I16 => build_out::<i16>(dev, &sc, proc, err)?,
        SampleFormat::I32 => build_out::<i32>(dev, &sc, proc, err)?,
        SampleFormat::U16 => build_out::<u16>(dev, &sc, proc, err)?,
        f => bail!("unsupported output sample format {f:?}"),
    };
    Ok((
        stream,
        StreamInfo {
            rate,
            channels: ch,
            format: format!("{fmt:?}"),
            buffer: buf,
            exclusive: false,
        },
    ))
}

fn build_out<T>(
    dev: &cpal::Device,
    cfg: &StreamConfig,
    mut proc: OutputProcessor,
    err: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    Ok(dev.build_output_stream(
        cfg,
        move |d: &mut [T], _: &cpal::OutputCallbackInfo| proc.fill(d),
        err,
        None,
    )?)
}

/// Offline harness for the self-test and the CLI: runs mono samples at `in_rate` through
/// the real input processor (resample, chain, ring) and output processor
/// (drift-compensated resampler) without any audio device. Returns mono samples at
/// `out_rate`.
pub fn pump_offline(input: &[f32], in_rate: u32, out_rate: u32, params: &ChainParams) -> Vec<f32> {
    let shared = Arc::new(Shared::new(*params));
    let main = Arc::new(Ring::new(RING_CAP));
    let mon = Arc::new(Ring::new(RING_CAP));
    let mut inp = InputProcessor::new(in_rate, 1, main.clone(), mon, shared.clone());
    let mut out = OutputProcessor {
        ring: main,
        rs: Resampler::new(),
        ch: 1,
        out_rate: out_rate as f64,
        primed: false,
        adj: 0.0,
        fill_avg: TARGET_FILL as f64,
        is_monitor: false,
        shared,
        mmcss: true,
    };
    inp.mmcss = true;
    let chunk_in = (in_rate as usize / 100).max(1);
    let mut result = Vec::with_capacity(input.len() * out_rate as usize / in_rate as usize + 1024);
    let mut owed = 0.0f64;
    let mut buf = vec![0.0f32; 4096];
    for c in input.chunks(chunk_in) {
        inp.process(c);
        owed += c.len() as f64 * out_rate as f64 / in_rate as f64;
        let n = owed as usize;
        owed -= n as f64;
        let mut left = n;
        while left > 0 {
            let k = left.min(buf.len());
            out.fill(&mut buf[..k]);
            result.extend_from_slice(&buf[..k]);
            left -= k;
        }
    }
    result
}
