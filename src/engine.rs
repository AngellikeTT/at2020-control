//! WASAPI (through cpal) capture -> processing chain -> render engine.
//!
//! Input callback  : any rate/channels/format -> mono f32 -> 48 kHz -> chain (480-sample frames)
//!                   -> SPSC ring(s).
//! Output callback : ring -> cubic resampler with drift compensation -> device rate/channels.
//! Windows cannot create a virtual microphone without a kernel driver, so the
//! processed signal is rendered to a *virtual cable* playback device (e.g. VB-CABLE's
//! "CABLE Input"); other apps then pick the matching recording device ("CABLE Output").

use crate::dsp::{Biquad, Chain, Coeffs};
use crate::params::*;
use crate::resample::Resampler;
use crate::ring::Ring;
use crate::shared::Shared;
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

const RING_CAP: usize = 1 << 16; // ~1.36 s @ 48 kHz
/// Target ring fill used for drift compensation / underrun protection (~30 ms).
const TARGET_FILL: usize = 1440;

#[derive(Clone, Debug)]
pub struct EngineInfo {
    pub in_rate: u32,
    pub in_channels: u16,
    pub out_rate: u32,
    pub out_channels: u16,
}

pub struct Engine {
    _input: Stream,
    _output: Stream,
    _monitor: Option<Stream>,
    pub info: EngineInfo,
    shared: Arc<Shared>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.monitor_active.store(false, Relaxed);
    }
}

fn find_device(host: &cpal::Host, name: &str, input: bool) -> Result<cpal::Device> {
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

impl Engine {
    pub fn start(
        input_name: &str,
        output_name: &str,
        monitor_name: Option<&str>,
        shared: Arc<Shared>,
    ) -> Result<Engine> {
        let host = cpal::default_host();
        let in_dev = find_device(&host, input_name, true)?;
        let out_dev = find_device(&host, output_name, false)?;

        let main_ring = Arc::new(Ring::new(RING_CAP));
        let mon_ring = Arc::new(Ring::new(RING_CAP));

        let (output, out_rate, out_ch) =
            build_output(&out_dev, main_ring.clone(), shared.clone(), false)
                .context("opening output device")?;
        let monitor = match monitor_name {
            Some(n) => {
                let d = find_device(&host, n, false)?;
                let (s, _, _) = build_output(&d, mon_ring.clone(), shared.clone(), true)
                    .context("opening monitor device")?;
                Some(s)
            }
            None => None,
        };
        shared.monitor_active.store(monitor.is_some(), Relaxed);
        let (input, in_rate, in_ch) = build_input(&in_dev, main_ring, mon_ring, shared.clone())
            .context("opening input device")?;

        output.play().context("starting output stream")?;
        if let Some(m) = &monitor {
            m.play().context("starting monitor stream")?;
        }
        input.play().context("starting input stream")?;

        Ok(Engine {
            _input: input,
            _output: output,
            info: EngineInfo {
                in_rate,
                in_channels: in_ch,
                out_rate,
                out_channels: out_ch,
            },
            _monitor: monitor,
            shared,
        })
    }

    /// Rough end-to-end latency in ms (processing + queue; excludes WASAPI device buffers).
    pub fn latency_ms(&self) -> f32 {
        (Chain::LATENCY_SAMPLES + FRAME / 2 + self.shared.out_fill.load(Relaxed)) as f32 / 48.0
    }
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
    local: ChainParams,
    ver: u64,
    shared: Arc<Shared>,
    main: Arc<Ring>,
    mon: Arc<Ring>,
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
            mono: Vec::with_capacity(32_768),
            acc: Vec::with_capacity(32_768),
            chain: Chain::new(),
            local: ChainParams::default(),
            ver: 0,
            shared,
            main,
            mon,
        }
    }

    fn process<T: Sample>(&mut self, data: &[T])
    where
        f32: FromSample<T>,
    {
        // Pick up parameter changes without ever blocking.
        let v = self.shared.params.version.load(Relaxed);
        if v != self.ver {
            if let Ok(g) = self.shared.params.data.try_lock() {
                self.local = *g;
                self.ver = v;
            }
        }

        let ch = self.ch;
        let mode = self.local.input.channel;
        self.mono.clear();
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
            self.acc.extend_from_slice(&self.mono);
        } else {
            let mut it = self.mono.iter().copied();
            let acc = &mut self.acc;
            self.rs.run(self.step, &mut || it.next(), &mut |y| {
                acc.push(y);
                true
            });
        }

        let mon_on = self.shared.monitor_active.load(Relaxed);
        while self.acc.len() >= FRAME {
            let st = self.chain.process(&mut self.acc[..FRAME], &self.local);
            let m = &self.shared.meters;
            m.in_peak.fetch_max(st.in_peak);
            m.out_peak.fetch_max(st.out_peak);
            m.gr_db.fetch_max(st.gr_db);
            m.vad.store(st.vad);
            m.gate_open.store(st.gate_open, Relaxed);
            self.main.push(&self.acc[..FRAME]);
            if mon_on {
                self.mon.push(&self.acc[..FRAME]);
            }
            self.acc.drain(..FRAME);
        }
    }
}

fn build_input(
    dev: &cpal::Device,
    main: Arc<Ring>,
    mon: Arc<Ring>,
    shared: Arc<Shared>,
) -> Result<(Stream, u32, u16)> {
    let sc = dev.default_input_config()?;
    let cfg: StreamConfig = sc.config();
    let (rate, ch) = (cfg.sample_rate.0, cfg.channels);
    let proc = InputProcessor::new(rate, ch as usize, main, mon, shared.clone());
    let es = shared.clone();
    let err = move |e: cpal::StreamError| es.set_fault(format!("Input stream error: {e}"));
    let stream = match sc.sample_format() {
        SampleFormat::F32 => build_in::<f32>(dev, &cfg, proc, err)?,
        SampleFormat::I16 => build_in::<i16>(dev, &cfg, proc, err)?,
        SampleFormat::I32 => build_in::<i32>(dev, &cfg, proc, err)?,
        SampleFormat::U16 => build_in::<u16>(dev, &cfg, proc, err)?,
        f => bail!("unsupported input sample format {f:?}"),
    };
    Ok((stream, rate, ch))
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
    is_monitor: bool,
    shared: Arc<Shared>,
}

impl OutputProcessor {
    fn fill<T>(&mut self, data: &mut [T])
    where
        T: Sample + FromSample<f32>,
    {
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
            } else {
                data.fill(silence);
                return;
            }
        }
        // Clock-drift compensation: nudge the resampling ratio toward the target fill.
        let err = (fill as f64 - TARGET_FILL as f64) / SAMPLE_RATE as f64;
        let want = (err * 0.5).clamp(-0.005, 0.005);
        self.adj += (want - self.adj) * 0.05;
        let step = (SAMPLE_RATE as f64 / self.out_rate) * (1.0 + self.adj);

        let vol = if self.is_monitor {
            self.shared.monitor_vol.load()
        } else {
            1.0
        };
        let ring = self.ring.clone();
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
) -> Result<(Stream, u32, u16)> {
    let sc = dev.default_output_config()?;
    let cfg: StreamConfig = sc.config();
    let (rate, ch) = (cfg.sample_rate.0, cfg.channels);
    let proc = OutputProcessor {
        ring,
        rs: Resampler::new(),
        ch: ch as usize,
        out_rate: rate as f64,
        primed: false,
        adj: 0.0,
        is_monitor,
        shared: shared.clone(),
    };
    let es = shared.clone();
    let err = move |e: cpal::StreamError| {
        es.set_fault(format!(
            "{} stream error: {e}",
            if is_monitor { "Monitor" } else { "Output" }
        ))
    };
    let stream = match sc.sample_format() {
        SampleFormat::F32 => build_out::<f32>(dev, &cfg, proc, err)?,
        SampleFormat::I16 => build_out::<i16>(dev, &cfg, proc, err)?,
        SampleFormat::I32 => build_out::<i32>(dev, &cfg, proc, err)?,
        SampleFormat::U16 => build_out::<u16>(dev, &cfg, proc, err)?,
        f => bail!("unsupported output sample format {f:?}"),
    };
    Ok((stream, rate, ch))
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
