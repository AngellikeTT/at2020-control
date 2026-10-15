//! Offline command line: `--process in.wav out.wav --preset <name|file.toml>`.

use crate::dsp::Chain;
use crate::params::{ChainParams, FRAME, SAMPLE_RATE};
use crate::resample::Resampler;
use crate::settings;
use anyhow::{bail, Context, Result};
use std::path::Path;

const RATE: u32 = SAMPLE_RATE as u32;

fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return x.to_vec();
    }
    let mut it = x.iter().copied();
    let mut out = Vec::with_capacity((x.len() as f64 * to as f64 / from as f64) as usize + 8);
    let mut rs = Resampler::new();
    // No anti-alias filter when downsampling: inputs are voice recordings and the chain's
    // own filtering dominates (see DECISIONS.md).
    rs.run(from as f64 / to as f64, &mut || it.next(), &mut |y| {
        out.push(y);
        true
    });
    out
}

/// Runs `mono` (any rate) through the chain with parameters `p`; the output has the
/// same rate and length as the input and is aligned with it (algorithmic delay removed).
pub fn process_samples(mono: &[f32], rate: u32, p: &ChainParams) -> Vec<f32> {
    let mut x = resample(mono, rate, RATE);
    let want = x.len();
    // Flush the chain's latency with silence, then drop the delay at the front.
    x.resize(want + Chain::LATENCY_SAMPLES + FRAME, 0.0);
    let n = x.len() / FRAME * FRAME;
    x.truncate(n);
    let mut chain = Chain::new();
    for f in x.chunks_mut(FRAME) {
        chain.process(f, p);
    }
    let aligned: Vec<f32> = x
        .into_iter()
        .skip(Chain::LATENCY_SAMPLES)
        .take(want)
        .collect();
    let mut y = resample(&aligned, RATE, rate);
    y.resize(mono.len(), 0.0);
    y
}

pub fn read_wav_mono(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut r =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    let mono = samples
        .chunks_exact(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect();
    Ok((mono, spec.sample_rate))
}

pub fn write_wav_mono(path: &Path, x: &[f32], rate: u32, bits: u16) -> Result<()> {
    let spec = if bits == 32 {
        hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        }
    } else {
        hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        }
    };
    let mut w = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creating {}", path.display()))?;
    match bits {
        32 => {
            for v in x {
                w.write_sample(*v)?;
            }
        }
        16 | 24 => {
            let max = ((1i64 << (bits - 1)) - 1) as f32;
            for v in x {
                w.write_sample((v.clamp(-1.0, 1.0) * max).round() as i32)?;
            }
        }
        b => bail!("unsupported bit depth {b} (use 16, 24 or 32)"),
    }
    w.finalize()?;
    Ok(())
}

const USAGE: &str = "usage: at2020-control --process <in.wav> <out.wav> [--preset <name|file.toml>] [--bits 16|24|32]\n       at2020-control --list-presets";

/// Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e:#}");
            2
        }
    }
}

fn run_inner(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "--list-presets") {
        for (n, _) in ChainParams::builtin() {
            println!("{n}");
        }
        for n in settings::list_user_presets() {
            println!("{n}");
        }
        return Ok(());
    }
    let pos = args.iter().position(|a| a == "--process").context(USAGE)?;
    let (Some(inp), Some(out)) = (args.get(pos + 1), args.get(pos + 2)) else {
        bail!("{USAGE}");
    };
    let opt = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let params = match opt("--preset") {
        Some(n) => settings::resolve_preset(&n).map_err(anyhow::Error::msg)?,
        None => ChainParams::voice(),
    };
    let bits: u16 = opt("--bits").and_then(|b| b.parse().ok()).unwrap_or(24);
    let (mono, rate) = read_wav_mono(Path::new(inp))?;
    let y = process_samples(&mono, rate, &params);
    write_wav_mono(Path::new(out), &y, rate, bits)?;
    println!(
        "{} -> {}  ({:.1} s, {} Hz, {}-bit)",
        inp,
        out,
        y.len() as f32 / rate as f32,
        rate,
        bits
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f64, db: f32, n: usize, fs: f64) -> Vec<f32> {
        let a = 10f64.powf(db as f64 / 20.0);
        (0..n)
            .map(|i| (a * (2.0 * std::f64::consts::PI * f * i as f64 / fs).sin()) as f32)
            .collect()
    }

    #[test]
    fn output_matches_input_length_and_level() {
        let x = tone(1000.0, -12.0, 48_000, 48_000.0);
        let y = process_samples(&x, 48_000, &ChainParams::flat());
        assert_eq!(y.len(), x.len());
        let rms = |v: &[f32]| (v.iter().map(|a| a * a).sum::<f32>() / v.len() as f32).sqrt();
        let (a, b) = (rms(&x[20_000..]), rms(&y[20_000..]));
        assert!((20.0 * (b / a).log10()).abs() < 0.2);
    }

    #[test]
    fn output_is_time_aligned_with_input() {
        let mut x = vec![0.0f32; 24_000];
        x[12_000] = 0.5;
        let y = process_samples(&x, 48_000, &ChainParams::flat());
        let peak =
            y.iter().enumerate().fold(
                (0, 0.0f32),
                |m, (i, v)| {
                    if v.abs() > m.1 {
                        (i, v.abs())
                    } else {
                        m
                    }
                },
            );
        assert!((peak.0 as i64 - 12_000).abs() <= 2, "peak at {}", peak.0);
    }

    #[test]
    fn works_at_other_sample_rates() {
        let x = tone(500.0, -10.0, 44_100, 44_100.0);
        let y = process_samples(&x, 44_100, &ChainParams::voice());
        assert_eq!(y.len(), x.len());
        assert!(y.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn wav_round_trip_24_bit() {
        let dir = std::env::temp_dir().join("at2020_cli_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("rt.wav");
        let x = tone(440.0, -6.0, 4800, 48_000.0);
        write_wav_mono(&p, &x, 48_000, 24).unwrap();
        let (y, r) = read_wav_mono(&p).unwrap();
        assert_eq!(r, 48_000);
        assert_eq!(y.len(), x.len());
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-5));
    }
}
