# AT2020USB-X Control Panel

Native Windows (Rust + egui/eframe) control panel and real-time voice chain for the
Audio-Technica AT2020USB-X. No Chromium / WebView / Electron.

```
 AT2020USB-X ──WASAPI capture──► mono ► 48 kHz ► [ gain → low-cut → RNNoise → gate → 5-band EQ
                                                    → compressor → out gain → limiter ]
        ──► WASAPI render to a virtual cable  ("CABLE Input")   ──►  Discord/OBS pick "CABLE Output"
        └─► optional monitor output (your headphones)
```

## Build & run

```powershell
rustup default stable-x86_64-pc-windows-msvc    # needs the MSVC build tools
cargo run --release
```
The very first build downloads crates; nothing else is required.
Settings and presets live in `%APPDATA%\AT2020 Control Panel\`.

## Using it with Discord / OBS / games

Windows cannot create a microphone device without a kernel driver, so this app renders the
processed audio into a **virtual audio cable** (free: VB-CABLE, https://vb-audio.com/Cable/).

1. Install the cable and reboot.
2. In this app: **Mic** = your AT2020USB-X, **Output** = `CABLE Input` (auto-selected if found).
3. In Discord / OBS / your game choose **`CABLE Output`** as the microphone.
4. Turn on **Monitor** and pick your headphones if you want to hear yourself.

## What is controlled where

| Feature | How |
|---|---|
| Device list, hot-plug, default-device changes | `IMMDeviceEnumerator` + `IMMNotificationClient` |
| Mic volume (scalar + dB) and mute | `IAudioEndpointVolume` |
| Windows-side peak meter | `IAudioMeterInformation` |
| Friendly name, "audio enhancements" flag | `IPropertyStore` (writing the flag may need admin) |
| Capture / render streams | `cpal` (WASAPI shared mode) |
| Noise suppression | RNNoise via the pure-Rust `nnnoiseless` |
| Gate (hysteresis, hold, 4 ms lookahead), EQ (RBJ biquads), compressor (soft knee), limiter | `src/dsp.rs` |

Controls that exist only physically on the microphone are not exposed through Windows
audio APIs and therefore can't be driven from software.

## Design notes

* All COM lives on one MTA thread (`devctl.rs`); the UI talks to it over a channel.
* Audio callbacks never block or allocate in steady state: parameters are `Copy` and handed
  over with a version counter + `try_lock`; audio moves between callbacks via a lock-free ring.
* Input and output devices run on independent clocks. The output side uses a cubic resampler whose
  ratio is nudged by ring-buffer fill level, so there is no drift-induced glitching.
* Processing latency ≈ 10 ms (RNNoise) + 4 ms (gate lookahead) + ≈5 ms framing + ~30 ms queue cushion
  (`TARGET_FILL` in `engine.rs` — lower it for less latency, raise it if you hear underruns),
  plus WASAPI shared-mode buffers (~10–20 ms each side).

## Tests

`dsp.rs`, `resample.rs` and `ring.rs` carry unit tests (filter responses vs. theory, gate
hysteresis/hold, compressor static curve, limiter ceiling, resampler fidelity, ring-buffer
threading). Run `cargo test`.

## Known limitations / ideas

* Shared-mode WASAPI only (no exclusive mode).
* No global mute hotkey or tray icon yet (`RegisterHotKey` / `Shell_NotifyIcon` would be the route).
