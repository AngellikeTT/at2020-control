# DECISIONS

Session constraints: no shell on the Windows PC; cloud container is Linux; static.rust-lang.org blocked (no Windows std target); no ALSA dev package.

1. **Verification harnesses instead of a Windows build.** Compiled the platform-neutral modules (params, denoise, dsp, resample, ring, shared, settings, widgets, app with stubbed devctl/engine) on Linux against the pinned eframe 0.29 / nnnoiseless 0.5. Compiled engine.rs against cpal 0.15 with a fake alsa.pc (type-check only; cpal's API surface is the same on WASAPI). Alternative (upgrading crates) rejected per instructions.
2. **devctl.rs changed from windows 0.58 source, NOT compiled.** `PROPVARIANT` is `windows::core::PROPVARIANT` (opaque newtype); raw field access removed in favour of `PROPVARIANT::from(u32)` / `u32::try_from(&pv)`. `PropVariantToStringAlloc` import moved to `Win32::System::Com::StructuredStorage`. Removed the unused `Win32_System_Variant` feature. IMMNotificationClient_Impl signatures were checked against windows-0.58.0 impl.rs and already matched.
3. **Clippy fixes:** 0.7071 -> `FRAC_1_SQRT_2` (changes the Q by <0.01%, tests unchanged); derived `Default` for `ChainParams` (identical to the hand-written impl); `Stroke::new(1.0_f32, ..)` to silence the float_literal_f32_fallback future-incompat warning; two `needless_range_loop` allows (indexing parallel buffers / index used as widget id); `map_or(false, ..)` -> `is_some_and`; fixed a borrow error in app.rs (mute colour read before the mutable toggle borrow).
4. No test was changed or weakened.
5. Git: no git on the PC side; no commit was made in D:\Mic App - Claude.
6. Phases 2-5 not started: they need a Windows toolchain/hardware, and writing thousands of lines blind on never-compiled code was judged worse than stopping.

## Session 2 (Windows, real toolchain)
7. **Installed Rust via rustup (per-user, ~/.cargo, stable 1.99.1 msvc)** after finding none on C:. User said "do all you need to do". Alternative: stop (original instruction); superseded by the user's later message. No drivers or system-wide software installed. git was already present; `git init` done.
8. **Cargo.toml: added `windows` feature `implement` and direct dep `windows-core = "0.58"`.** Needed for `#[implement(IMMNotificationClient)]` (generates `_Impl` trait and references `::windows_core`). Same major version as pinned; no crate upgraded.
9. **Clippy (real Windows run):** removed unused `EngineInfo::monitor`; `Ring::discard_all` made `#[cfg(test)]` (a test uses it; it will be re-exposed if xrun recovery needs it); blank `//!` line before doc paragraph in devctl.rs. No test changed.
10. devctl.rs compiled first time after #8; the earlier PROPVARIANT fixes were correct.

## Phase 2
11. **Engine drift controller smoothed** (found by the selftest): the ratio was driven by the instantaneous ring fill, so bursty callbacks modulated pitch by up to +/-0.5%. Fill is now low-passed (alpha 0.02 per callback), gain halved and clamp reduced to +/-0.2%. Verified: offline pipeline gain 0.00 dB, THD+N -55..-153 dB at 48/44.1/96 kHz in, 48/44.1 out.
12. **Selftest loopback design:** one VB-CABLE pair can't loop the live engine back without a feedback path, so (a) the real engine runs mic -> CABLE Input while a recorder taps CABLE Output (30 s live + soak), (b) synthetic stimuli are played into CABLE Input and captured from CABLE Output for latency/response/THD+N, and (c) each DSP stage's output is generated offline by the same Chain, rendered through the cable and compared. Alternative (WASAPI loopback of a speaker endpoint) rejected: audible and system-dependent.
13. **Latency measurement:** render-callback hand-over to capture-callback delivery using Instant timestamps (start-up cost excluded). Measured ~110 ms for VB-CABLE through cpal default buffers; this is device-path latency, separate from the ~49 ms processing estimate.
14. **THD+N is measured over 0.2 s windows (worst window) with a +/-0.3 Hz frequency refinement**, because clock-drift compensation legitimately makes ppm-level pitch adjustments that a long fixed-frequency fit reads as noise. Threshold -55 dB for the resampled paths, -80 dB for the flat chain.
15. **Hot-plug paths are exercised in software** (60-notification burst, selecting a vanished endpoint id, re-select, engine start on a missing device, fault flag round trip, 3x engine restart). Physically unplugging / `pnputil /disable-device` needs admin or hands; listed in HUMAN_TESTS.md.
16. Selftest only *reads* the enhancement flag and default devices (compared before/after); volume and mute are changed and restored by a Drop guard.
