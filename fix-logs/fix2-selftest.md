# SELFTEST REPORT

Generated 2026-10-16 03:53:07 UTC - run time 1.2 min - 59 passed, 0 failed, 2 warnings


## DSP stages (offline, 48 kHz)

- **PASS** flat chain frequency response: worst deviation 0.01 dB (50 Hz -0.01 dB; 100 Hz -0.00 dB; 250 Hz -0.00 dB; 1000 Hz +0.00 dB; 4000 Hz +0.00 dB; 10000 Hz +0.00 dB; 16000 Hz +0.00 dB; )
- **PASS** flat chain THD+N @1 kHz -6 dBFS: -147.9 dB
- **PASS** bypass applies input gain only (+6 dB): +6.00 dB
- **PASS** low cut -3 dB at corner: -3.01 dB @80 Hz
- **PASS** low cut passes 1 kHz: -0.00 dB
- **PASS** RNNoise suppresses stationary noise: -30.0 dBFS -> -91.5 dBFS (61.5 dB)
- **PASS** gate open passes signal: -0.00 dB
- **PASS** gate closed attenuates: -60.0 dB
- **PASS** EQ +6 dB peak at 1 kHz: +6.00 dB
- **PASS** compressor matches static curve: peak -16.88 dBFS, expected -17.00
- **PASS** limiter holds ceiling: peak -3.00 dBFS (ceiling -3)
- **PASS** output gain +6 dB: +6.00 dB
- **PASS** output mute silences: -200 dBFS
- **PASS** de-esser reduces 6.5 kHz: -12.0 dB
- **PASS** de-esser leaves 500 Hz alone: -0.00 dB
- **PASS** VAD gate shuts on non-voice noise: -25.0 -> -65.0 dBFS (40.0 dB)
- **PASS** parameter changes are click-free: max step 0.0105 vs clean sine 0.0104

## Engine pipeline (offline: resampler, ring, drift control)

- **PASS** 48000 Hz in -> 48000 Hz out: gain +0.00 dB, THD+N -150.9 dB, pitch error +0.000 Hz, latency 34.0 ms
- **PASS** 44100 Hz in -> 48000 Hz out: gain -0.00 dB, THD+N -89.5 dB, pitch error +0.000 Hz, latency 34.0 ms
- **PASS** 96000 Hz in -> 48000 Hz out: gain -0.00 dB, THD+N -147.2 dB, pitch error +0.000 Hz, latency 34.1 ms
- **PASS** 48000 Hz in -> 44100 Hz out: gain -0.00 dB, THD+N -55.5 dB, pitch error +0.000 Hz, latency 34.0 ms

## Device discovery and capabilities

- 12 capture endpoints, 15 render endpoints
- capture: `CABLE Output (VB-Audio Virtual Cable)` [virtual]
- capture: `Microphone (C505 HD Webcam)`
- capture: `Microphone (AT2020USB-X)` (default)
- capture: `Voicemeeter Out B3 (VB-Audio Voicemeeter VAIO)` [virtual]
- capture: `Voicemeeter Out A3 (VB-Audio Voicemeeter VAIO)` [virtual]
- capture: `Voicemeeter Out B1 (VB-Audio Voicemeeter VAIO)` [virtual]
- capture: `Voicemeeter Out A4 (VB-Audio Voicemeeter VAIO)` [virtual]
- capture: `Voicemeeter Out A5 (VB-Audio Voicemeeter VAIO)` [virtual]
- capture: `Headset Microphone (DualSense Wireless Controller)`
- capture: `Voicemeeter Out A2 (VB-Audio Voicemeeter VAIO)` [virtual]
- capture: `Voicemeeter Out B2 (VB-Audio Voicemeeter VAIO)` [virtual]
- capture: `Voicemeeter Out A1 (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `Voicemeeter In 4 (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `Voicemeeter In 2 (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `Realtek Digital Output (Realtek(R) Audio)`
- render: `Voicemeeter In 1 (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `Voicemeeter Input (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `CS24FK (NVIDIA High Definition Audio)`
- render: `Voicemeeter VAIO3 Input (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `Voicemeeter In 3 (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `CABLE In 16ch (VB-Audio Virtual Cable)` [virtual]
- render: `Voicemeeter In 5 (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `CABLE Input (VB-Audio Virtual Cable)` (default) [virtual]
- render: `Speakers (DualSense Wireless Controller)`
- render: `MSI MAG270CR (NVIDIA High Definition Audio)`
- render: `Voicemeeter AUX Input (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `Headphones (AT2020USB-X)`
- **PASS** AT2020USB-X found: Microphone (AT2020USB-X)
- **PASS** virtual cable present: input=Some("CABLE Input (VB-Audio Virtual Cable)") output=Some("CABLE Output (VB-Audio Virtual Cable)")
- endpoint volume: 100% (30.0 dB), range -13.0..30.0 dB, mute=false, enhancements disabled=Some(false)
- **PASS** endpoint volume interface available: Microphone (AT2020USB-X)
- **PASS** dB range reported: -13.0..30.0
- WASAPI input format: 2 ch, 48000-48000 Hz, U8
- WASAPI input format: 2 ch, 48000-48000 Hz, I16
- WASAPI input format: 2 ch, 48000-48000 Hz, I32
- WASAPI input format: 2 ch, 48000-48000 Hz, F32

## Volume and mute round trip

- **PASS** set volume 25%: read back 25.0% / -3.7 dB
- **PASS** set volume 50%: read back 50.0% / 2.3 dB
- **PASS** set volume 75%: read back 75.0% / 10.6 dB
- **PASS** dB increases with scalar volume: monotonic over 25/50/75%
- **PASS** set mute=true: read back
- **PASS** set mute=false: read back
- **PASS** original volume and mute restored: 100%, mute=false

## Hot-plug and failure handler paths

- **PASS** refresh burst coalesced: 60 notifications -> 1 refreshes
- **PASS** missing device reported, no endpoint bound: Selected microphone is not connected.
- **PASS** device re-bound after return: Microphone (AT2020USB-X)
- **PASS** engine start on missing input -> Err: Input device "No Such Microphone" not found (is it plugged in / enabled?)
- **PASS** fault flag take/clear: set -> take -> empty
- **PASS** engine stop/start x3 (device-change restart path): no failure

## Live capture with RNNoise (3 s)

- engine: in 48000 Hz x2, out 48000 Hz x2, algorithmic latency 19.0 ms
- meters: in peak -38.5 dBFS, out peak -45.0 dBFS, max GR -0.0 dB, VAD mean 0.02 max 0.29
- **PASS** no stream fault: None
- **PASS** underruns after start-up <= 2: 0
- **PASS** live latency estimate < 120 ms: 42.8 ms
- raw mic RMS -49.6 dBFS; cable capture RMS -71.8 dBFS (-22.2 dB)
- **PASS** microphone delivers a signal (not digital silence): RMS -49.6 dBFS, peak -38.5 dBFS
- **PASS** processed audio arrives at CABLE Output: 160320 samples captured
- **PASS** capture has no NaN/inf: all finite

## VB-CABLE loopback

- **PASS** cable render-to-capture latency < 150 ms: 101.7 ms
- **PASS** flat response (DSP bypassed): worst 0.00 dB; 100:+0.00 250:+0.00 500:+0.00 1000:+0.00 2000:-0.00 4000:-0.00 8000:-0.00 12000:-0.00 16000:-0.00
- **PASS** cable gain near unity: 1 kHz reads -0.00 dB re -12 dBFS
- **PASS** cable THD+N @1 kHz -6 dBFS: -110.2 dB
- stage output generated offline, rendered through the cable, compared per tone
- **PASS** stage via cable: low cut 150 Hz: tone levels dBFS -33.8/-26.0/-26.0/-26.0/-26.0; device-path error 0.04 dB
- **PASS** stage via cable: RNNoise: tone levels dBFS -32.2/-26.0/-26.0/-26.3/-27.6; device-path error 0.00 dB
- **PASS** stage via cable: gate: tone levels dBFS -26.0/-26.0/-26.0/-26.0/-26.0; device-path error 0.00 dB
- **PASS** stage via cable: EQ +6 dB @3 kHz: tone levels dBFS -26.0/-25.8/-25.2/-20.0/-25.2; device-path error 0.00 dB
- **PASS** stage via cable: compressor: tone levels dBFS -34.0/-34.0/-34.0/-34.0/-34.0; device-path error 0.00 dB
- **PASS** stage via cable: limiter: tone levels dBFS -17.9/-18.0/-17.9/-18.0/-18.0; device-path error 0.00 dB
- **PASS** stage via cable: de-esser @8 kHz: tone levels dBFS -26.0/-26.0/-26.0/-26.4/-37.9; device-path error 0.00 dB
- **PASS** stage via cable: VAD gate: tone levels dBFS -26.0/-26.0/-26.0/-26.0/-26.0; device-path error 0.00 dB

## WASAPI exclusive mode

- WARN: skipped: exclusive mode would pre-empt the running stream(s) of voicemeeter_x64.exe (pid 30236) on `Microphone (AT2020USB-X)` (they would stay silent until restarted); close them or pass --allow-preempt

## Soak (0 min)

- WARN: soak disabled

## System state restored

- **PASS** mic volume and mute: 100% mute=false (was 100% mute=false)
- **PASS** enhancement flag unchanged: Some(false) (was Some(false))
- **PASS** default devices unchanged: capture and render defaults identical
