# AT2020USB-X Control Panel

A native Windows control panel and real-time voice-processing chain for the
**Audio-Technica AT2020USB-X**, written in Rust (no WebView, no Chromium).

It does two jobs:

1. **Controls the microphone through Windows** – endpoint volume (scalar and dB, with the
   range Windows reports), mute, and the "audio enhancements" flag, plus device
   enumeration and hot-plug handling.
2. **Processes your voice in real time** – capture → input gain → low-cut → RNNoise
   noise suppression → voice gate (VAD) → noise gate → 5-band EQ → de-esser →
   compressor → output gain → limiter, rendered into a virtual cable so Discord, OBS,
   games and calls can use the processed signal as their microphone.

> Windows cannot create a microphone without a kernel driver. This app therefore plays
> the processed audio into the playback side of a **virtual cable** (VB-CABLE is
> tested); other apps pick the cable's *recording* side (`CABLE Output`) as their mic.

Hardware-only controls on the microphone (knobs, switches, touch mute) are not exposed by
the Windows audio APIs, so they can neither be read nor changed from software.

## Quick start

1. Install a virtual cable (free): <https://vb-audio.com/Cable/>.
2. Run `at2020-control.exe` (portable) or the installer from the releases / `dist` folder.
3. Pick your mic and set **Output** to the cable's playback device (`CABLE Input`).
   The app picks `CABLE Input` automatically when it finds it.
4. In Discord / OBS / your game choose **`CABLE Output`** as the microphone.
5. Keep your real speakers or headphones as the Windows default *playback* device
   (otherwise all system sound goes into the cable). The **Cable setup** tab checks this.

## Features

| Area | What you get |
|---|---|
| Windows control | Volume (scalar + dB), mute, enhancements flag, device list, hot-plug / default-change handling |
| DSP chain | Low-cut, RNNoise (strength), VAD gate, noise gate (hysteresis/hold), 5-band EQ with graph, split-band de-esser, compressor (soft knee, auto make-up), limiter, output gain/mute, bypass |
| Metering | Input/output level (3 meter styles), compressor and de-esser gain reduction, voice-activity indicator, Windows-side raw peak |
| Analyzer | 2048-point spectrum of the signal before and after the chain (`realfft`) |
| Audio settings | Sample rate and buffer size from what the device reports, **WASAPI exclusive mode** with per-side fallback, latency readout, xrun counter, MMCSS "Pro Audio" |
| Presets | Built-ins, user presets, A/B compare, TOML import/export |
| Desktop | System tray, start with Windows, global hotkeys, test recording to WAV |
| Look | Nine themes + "Follow Windows", hot-reloaded user TOML themes with an editor, bundled Inter and JetBrains Mono fonts, dark title bar |
| Offline | `--process in.wav out.wav --preset <name>` |

### Real-time rules

The audio callbacks never allocate, lock, log or make system calls. Parameters arrive
through a lock-free triple buffer and every parameter change is smoothed (gains ramp per
sample, filter coefficients and thresholds glide over ~20 ms, stage switches are folded
into neutral values), so moving a slider or toggling a stage does not click. The only
system call on a callback thread is a single MMCSS registration on its first invocation.

## Command line

```text
at2020-control.exe --process in.wav out.wav [--preset <name|file.toml>] [--bits 16|24|32]
at2020-control.exe --list-presets
at2020-control.exe --selftest [--quick] [--out report.md]    (hidden; see below)
at2020-control.exe --minimized                                (start hidden in the tray)
```

The release exe uses the Windows GUI subsystem, so a terminal does not wait for it by default;
use `start /wait at2020-control.exe --process ...` in cmd or `Start-Process -Wait` in PowerShell to
get the output in order and a usable exit code (0 = ok, 2 = error).

`--preset` accepts a built-in name (`voice`, `noisy`, `flat` or the full name), the name of
a saved preset, or a path to a preset `.toml`. Output has the input's sample rate and
length and is time-aligned with it.

## Presets and themes (TOML)

Presets are plain TOML mirroring the controls; any missing key keeps its default:

```toml
bypass = false

[comp]
enabled = true
threshold_db = -22.0
ratio = 3.5

[deess]
enabled = true
freq_hz = 6500.0
```

Themes live in `%APPDATA%\AT2020 Control Panel\themes\*.toml` and reload automatically when
a file changes. Colours are `"#rrggbb"`:

```toml
name = "My theme"
dark = true
accent = "#ff8800"
bg = "#101014"
```

## Self-test

`--selftest` writes `SELFTEST_REPORT.md` and restores every setting it touches (mic volume
and mute are restored by a guard; default devices and the enhancement flag are only read
and compared before/after). It covers device discovery and capabilities, the volume/mute
round trip, DSP stages, the engine pipeline at several sample-rate pairs, a 30 s live
capture with RNNoise, VB-CABLE loopback (latency, flat response with DSP bypassed, THD+N,
each stage through the real device path), exclusive mode, a 60-minute soak (xruns, ring
fill, CPU, memory) and the hot-plug handler paths. `--quick` shortens the long parts.

## Building

Requirements: Rust (stable, MSVC toolchain) and the Visual Studio Build Tools with the
"Desktop development with C++" workload.

```powershell
cargo build --release
cargo test --release --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
powershell -ExecutionPolicy Bypass -File scripts\package.ps1   # dist\: exe, zip, installer
```

Crates:

| Crate | Contents |
|---|---|
| `crates/core-dsp` | Platform-neutral DSP: parameters, biquads, gate, EQ, de-esser, compressor, limiter, RNNoise wrapper, resampler, ring buffer, spectrum, offline processing |
| `crates/device` | Core Audio device control, shared (cpal) and exclusive WASAPI engines, recorder, Windows helpers |
| `crates/app` | egui application, themes, tray and hotkeys, self-test, CLI |

> **Note on Smart App Control:** on a machine with Windows *Smart App Control* enforcing,
> freshly compiled unsigned executables (debug test binaries, new build scripts) can be
> blocked (`os error 4551`). `cargo test --release` worked on the development machine;
> see `DECISIONS.md`.

## Files

Settings, presets, themes, logs and test recordings are stored under
`%APPDATA%\AT2020 Control Panel\`. Logs roll daily and keep the last seven files.

## Licences

- Inter (UI font) and JetBrains Mono (numbers) are bundled under the SIL Open Font
  License 1.1; the licence texts are in `crates/app/assets/fonts/` and ship with every
  package.
- RNNoise is used through the pure-Rust port `nnnoiseless` (BSD-3-Clause).
