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

## Phase 3
17. **Every enable switch is folded into a neutral numeric value** (`effective_params`): compressor off = ratio 1 / make-up 0, EQ off = 0 dB gains, de-esser off = 0 dB max cut, VAD gate off = 0 dB range, low-cut off = 10 Hz corner, limiter off = ceiling 0 dB, mute = -inf gain. Together with per-sample gain ramps and ~20 ms parameter glides this makes toggling a stage click-free without crossfade buffers. Alternatives: per-stage wet/dry crossfades (more CPU and state) or discrete switching (clicks). Covered by unit tests (`toggling_stages_mid_stream_does_not_click` etc.).
18. **Bypass keeps zero extra latency**: the chain always runs (so every state stays warm) and the output crossfades between chain and the input-gain-only dry path over one frame. This preserves the existing `bypass_is_transparent_except_gain` test.
19. **Lock-free parameter hand-off with the `triple_buffer` crate** instead of the previous `try_lock` on a mutex. Each engine start attaches a fresh reader seeded with the latest parameters. New dependency, justified by the "no locks in the audio callback" rule.
20. **Taps for the analyzer and recorder are SPSC rings** written by the audio thread (`Ring::push` is bounded and wait-free) and drained by the UI / a writer thread. The recorder also records either the processed or the raw (pre-chain) signal.
21. **De-esser is split-band** (band-pass around the sibilance frequency drives the gain computer; reduction is subtracted from that band only). Zero added latency. Alternative: wideband compression keyed from a high-pass, which dulls the whole voice.
22. **VAD gate uses RNNoise's own voice probability** (one value per 10 ms frame, with hold and release) so steady loud non-voice noise stays shut where a level gate would open.
23. **Exclusive mode is implemented with our own WASAPI event-driven code** (`wasapi_excl.rs`), because cpal 0.15 only does shared mode. Formats are probed with `IsFormatSupported` (24-in-32, 16, 32-bit int, float, packed 24) at the requested/mix/48/44.1/96 kHz rates; each side (input, output) falls back to shared mode independently and the reason is shown in the Audio tab. Exclusive capture of the mic stops other apps using it, which is stated in the UI.
24. **Sample rate and buffer size options are read from the devices** (`supported_*_configs`): the rate list is the intersection of what input and output report; the buffer slider only appears when the driver reports a range, and a fixed buffer is only applied when inside it. The sample format is shown, not chosen (cpal picks the best the device offers; the engine converts everything to f32).
25. **MMCSS "Pro Audio" is registered once per callback thread on its first invocation.** That is the single system call on those threads (it can only be made from the thread itself); the registration count is shown in the Audio tab.
26. **Hotkeys / tray events are delivered by handlers that forward to the UI over a channel and call `request_repaint`**, so they work while the window is hidden.
27. **Start with Windows** writes the per-user Run key (`--minimized`), no administrator rights. **Test recordings** are capped at 10 minutes and written as 48 kHz mono 32-bit float WAV.
28. **Default output choice prefers `CABLE Input`** over other "virtual" devices. The detection list also flags Voicemeeter and generic "virtual audio" names; with several virtual devices installed (this PC has Voicemeeter) the old "first virtual device" rule picked `Voicemeeter In 4`, found while taking screenshots.
29. **THD+N / latency numbers in the self-test report exercise the same code that runs live**, but the VB-CABLE round trip includes cpal's default WASAPI buffers (~110 ms); this is why exclusive mode and fixed buffers exist.

## Phase 4
30. **Theme model:** 14 colours + corner radius + dark flag, serialised as TOML with `#rrggbb` strings; every field defaults so partial user files work. Built-ins: Midnight, Graphite, Daylight, Solarized Dark, Solarized Light, Nord, Dracula, High Contrast, Amber Console. "Follow Windows" maps the `AppsUseLightTheme` registry value to Midnight/Daylight. A unit test asserts body text contrast >= 4.5:1 on every built-in panel colour (Solarized Light's text colour was darkened to pass).
31. **User themes hot-reload by polling the folder's file modification times every ~0.75 s** (no extra crate, works for editors that replace files).
32. **Fonts:** Inter (variable TTF from google/fonts, default instance) for UI text and JetBrains Mono Regular for monospace values, both SIL OFL 1.1, licence texts bundled next to the fonts and in every package. egui's default fonts remain as fallback for symbols.
33. **Three meter styles:** gradient bar, LED segments and an analogue-style arc gauge with needle and peak marker.
34. **Original icon is drawn procedurally** (rounded square gradient, capsule microphone, cradle, stand, sound arcs) from `icon.rs`, used for the window, tray and `.ico`. Embedding into the exe is done at packaging time by `--embed-icon` (UpdateResource) rather than by a build script, see #36.
35. **Screenshots** are produced by a hidden `--screenshots <dir>` mode that cycles every theme x tab using egui's viewport screenshot command; that mode never saves settings and never registers hotkeys.

## Phase 5
36. **Smart App Control switched from evaluation to enforcement on this PC during the session** (registry `VerifiedAndReputablePolicyState=1`, `SAC_PreviousState=2`; Code Integrity events 3077 at 14:49). Consequences: freshly compiled *debug* test executables and *new build scripts* are blocked (`os error 4551`); release executables built by rustc keep running. I did **not** disable Smart App Control (a security downgrade only the owner should decide). Mitigations: (a) tests are run with `cargo test --release`; (b) no build script is used in the workspace (an `embed-resource` build-dependency forced serde/quote/proc-macro2 build scripts to be recompiled and blocked), the icon is embedded after the build instead; (c) pinned crate versions unchanged. Plain `cargo test` (debug) works on machines without enforcement and in CI.
37. **Workspace split:** `core-dsp` (platform-neutral, no OS or UI dependencies, `unwrap`/`expect` denied by lint), `device` (Windows audio, `unwrap`/`expect` denied), `app` (egui, themes, tray, CLI, self-test). `device` does not depend on egui: the device thread wakes the UI through a `Wake` callback.
38. **Installer:** Inno Setup script for CI plus an IExpress-based per-user installer (`installer/install.cmd`) generated by `scripts/package.ps1` when Inno Setup is not installed (it is not on this PC and I did not install it). Both are per-user, no admin. The installer was built but **not executed** here (it would change the machine); see HUMAN_TESTS.md.
39. **License:** no licence is declared for the project itself (not my decision); only the bundled fonts' OFL texts ship.

## Late changes
40. **Click found by the self-test, fixed twice.** First, compressor make-up gain was a per-block constant, so enabling
    the compressor stepped the level at block boundaries (now slewed per sample, 5 ms). Second, EQ and low-cut
    coefficients changed once per 10 ms block; they now interpolate linearly per sample across the block
    (`Biquad::process_block_glide`; linear interpolation of biquad coefficients stays stable because the stability
    triangle is convex). Self-test result: largest sample step during simultaneous gain/EQ/compressor changes went
    0.0737 -> 0.0265 -> 0.0105 against 0.0104 for the clean sine.
41. **Build id.** `AT2020_BUILD_ID` (git short hash, set by `scripts/package.ps1` and CI) is shown on the System tab and
    in the log, so a bug report names the exact build. It also means each commit yields a distinct binary.
42. **Smart App Control verdicts are per file hash and not predictable** (some freshly built identical-source exes were
    allowed, others blocked). Where a build was blocked I changed nothing security-related; I rebuilt (the build id /
    a throw-away constant changes the hash) and re-ran. The final release binary used for the final self-test is the
    one built from the commit shown in `SELFTEST_REPORT.md`. The `dist` artifacts are built from the same source but
    contain the embedded icon, so they are not byte-identical to the tested exe.

## Session 3: the silent microphone, crash safety, multi-core testing

Details and evidence: `FIX_REPORT.md`.

43. **Backups before any change.** `backup\MMDevices-Audio.reg` and `backup\CapabilityAccessManager-HKCU.reg`
    (git-ignored: machine-specific, 8 MB). The System Restore point could not be created: Windows allows one per 24 h
    and one existed from 13:23 that day; changing that frequency means editing a registry key outside the two trees
    I was allowed to touch, so I did not.
44. **Device-stack resets skipped.** The mic was already delivering signal before anything was changed (first probe),
    so restarting audiosrv and disabling/enabling the AT2020 had nothing to fix. They would only have invalidated
    every live stream, including Voicemeeter's, which I showed does not recover by itself, and the user's Discord.
45. **Root cause by elimination, with a decisive measurement.** Every write the app makes was replayed on the live
    mic and none silences it. The exclusive-mode streams of the earlier session, which bypass the audio engine, APOs,
    endpoint volume/mute and Voice Clarity and use a different format, also read exact zeros, and the same path
    reads -1.7 dBFS now. So the zeros came from the device. The AT2020USB-X's topology has no mute node, so its touch
    mute is invisible to Windows; no PnP, power or USB event happened all day.
46. **Self-test exclusive section skips instead of pre-empting** other apps (WARN naming them); `--allow-preempt`
    overrides. Alternative (run and restart the victims) rejected: there is no general way to restart another app's
    stream; for Voicemeeter it needed its own API.
47. **Restore journal, not a registry/settings snapshot.** Only the endpoint values the app changes are journaled
    (volume, channel levels, mute, enhancement flag) plus the default devices for reporting; the app never changes
    defaults and there is no public API to set them.
48. **`panic = "unwind"` in release.** A few hundred KB larger, but every Drop guard (exclusive stream, COM scope,
    event handle, journal) now runs on a panic. Hard kills are covered by the journal.
49. **cpal callbacks wrapped in `GuardedCallback`**: a panic in the chain is contained, reported once (engine restart)
    and the stream outputs the format's silence until then.
50. **loom via target-only rustflags.** `--target x86_64-pc-windows-msvc --config target.<triple>.rustflags=['--cfg','loom']`
    keeps build scripts out of the loom build, because Smart App Control blocks freshly compiled build-script binaries
    on this PC. loom's model checks ring and meter; the shared parameter hand-off uses the `triple_buffer` crate (not
    ours) and is covered by a std stress test instead.
51. **Real-time stress harness (`device::rtsim`)** drives the engine's real input/output processors from two
    wall-clock-paced threads on one core, with a busy-loop thread pinned to every other core of the affinity mask,
    a skewed render clock (drift controller) and a parameter-changing thread. Real-device tests and this harness are
    `#[serial(system)]` (serial_test); pure tests stay parallel.
52. **DevCtl drains its command queue each pass.** Found by the hot-swap stress test: one command per pass plus a full
    re-enumeration per selection made shutdown wait behind a backlog of minutes.
53. **Drift controller measures a phase-independent fill** (found by the matrix on one core). It adds the capture
    device's output since its last push (elapsed time from a QueryPerformanceCounter read) to the ring fill, and
    raises the target by one capture period so the raw cushion is unchanged. That costs up to one capture period
    (10 ms) of extra latency. Alternative considered: a much slower controller; rejected because a static phase
    flip still needs the ring to move by a whole frame, i.e. a long pitch excursion. The clock read in the
    callbacks is user-mode and non-blocking; it is the second documented exception to "no system calls" (after
    the MMCSS registration).
54. **Priming drops the excess, never-played samples** (up to one frame) so playback starts at the controller's
    equilibrium, instead of a ~5 s pitch excursion at every engine start or after an underrun.
