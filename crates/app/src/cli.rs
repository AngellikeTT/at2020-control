//! Offline command line: `--process in.wav out.wav --preset <name|file.toml>`.

use crate::settings;
use anyhow::{bail, Context, Result};
use core_dsp::offline::{process_samples, read_wav_mono, write_wav_mono};
use core_dsp::params::ChainParams;
use std::path::Path;

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
