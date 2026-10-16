# LIVE NOISE SUPPRESSION REPORT

Generated 2026-10-16 05:40:57 UTC - run time 0.5 min - 10 passed, 0 failed, 0 warnings


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
- render: `CABLE Input (VB-Audio Virtual Cable)` [virtual]
- render: `Speakers (DualSense Wireless Controller)`
- render: `MSI MAG270CR (NVIDIA High Definition Audio)`
- render: `Voicemeeter AUX Input (VB-Audio Voicemeeter VAIO)` [virtual]
- render: `Headphones (AT2020USB-X)` (default)
- **PASS** AT2020USB-X found: Microphone (AT2020USB-X)
- **PASS** virtual cable present: input=Some("CABLE Input (VB-Audio Virtual Cable)") output=Some("CABLE Output (VB-Audio Virtual Cable)")
- endpoint volume: 100% (30.0 dB), range -13.0..30.0 dB, mute=false, enhancements disabled=Some(false)
- **PASS** endpoint volume interface available: Microphone (AT2020USB-X)
- **PASS** dB range reported: -13.0..30.0
- WASAPI input format: 2 ch, 48000-48000 Hz, U8
- WASAPI input format: 2 ch, 48000-48000 Hz, I16
- WASAPI input format: 2 ch, 48000-48000 Hz, I32
- WASAPI input format: 2 ch, 48000-48000 Hz, F32

## Live noise suppression (30 s of real microphone audio)

- **PASS** microphone delivers a signal (not digital silence): RMS -15.3 dBFS, peak 1.1 dBFS
- preset Voice, chain run offline on the capture: 3000 frames, 1544 noise-only (VAD < 0.1), 753 speech (VAD > 0.9)
- room noise -32.0 dBFS in -> -41.1 dBFS out
- **PASS** noise floor reduced by >= 6 dB: 9.1 dB
- **PASS** speech preserved (level change within -15..+10 dB): speech -10.5 dBFS in -> -20.0 dBFS out (-9.5 dB)

## System state restored

- **PASS** mic volume and mute: 100% mute=false (was 100% mute=false)
- **PASS** enhancement flag unchanged: Some(false) (was Some(false))
- **PASS** default devices unchanged: capture and render defaults identical
