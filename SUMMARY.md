# SUMMARY

State at the end of the session. Details of every choice are in `DECISIONS.md`; things that need
a human are in `HUMAN_TESTS.md`; machine-level notes are in `SETUP_NEEDED.md`.

## Result by phase

| Phase | Status |
|---|---|
| 1. Builds on Windows | **Done.** `cargo build --release`, `cargo test --release --workspace` (40 tests), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check` all clean. No test was weakened. devctl.rs compiled once `windows-core` + the `implement` feature were added. |
| 2. `--selftest` | **Done.** Writes `SELFTEST_REPORT.md`, restores volume/mute (guard), compares enhancement flag and default devices before/after. First full run (incl. the 60-minute soak) passed 56/56 and exposed two real defects that were fixed (pitch-modulating drift controller, see DECISIONS #11; test-method issues #12-14). The final run's numbers are in `SELFTEST_REPORT.md`. |
| 3. Features | **Done** (list below). Exclusive mode is real (own WASAPI code), verified on this hardware: 144-frame (3 ms) buffers on both the mic and the cable, 0 underruns. |
| 4. Themes, fonts, polish, icon | **Done.** 63 screenshots (9 themes x 7 tabs) in `screenshots\`. |
| 5. Workspace, CI, installer, docs | **Done**, with the caveats below (CI never ran; installer built but not executed). |

## What was built

- **DSP** (`core-dsp`): split-band de-esser, RNNoise-driven VAD gate, noise gate, 5-band EQ, compressor, limiter.
  Every switch is folded into a neutral value and all parameters glide, so nothing clicks (unit-tested, and a
  click found by the self-test in the compressor make-up gain was fixed).
- **Real-time rules**: no allocation / locks / logging in the callbacks; parameters via `triple_buffer`;
  analyzer and recorder taps are SPSC rings; MMCSS "Pro Audio" registered once per callback thread (the one
  syscall, documented).
- **App**: tabs *Chain, Spectrum, Audio, Cable setup, Presets, Appearance, System*; gain-reduction meters,
  VAD indicator, 3 meter styles; spectrum before/after the chain; sample rate / buffer / exclusive mode from
  device-reported capabilities; latency + xrun readouts; virtual-cable detection and a guided checklist (it
  correctly flagged that `CABLE Input` is this PC's default playback device); presets, A/B, TOML import/export;
  tray, start with Windows, global hotkeys; test recording; nine themes + Follow Windows + hot-reloaded user
  themes + editor; Inter / JetBrains Mono bundled with OFL texts; original icon; dark title bar.
- **CLI**: `--process in.wav out.wav --preset <name|file.toml>` (verified end to end), `--list-presets`,
  hidden `--selftest`, `--screenshots`, `--write-icon`, `--embed-icon`.
- **Packaging** (`dist\`, git-ignored): `at2020-control.exe` (portable, icon embedded),
  `AT2020-Control-0.2.0-portable.zip`, `AT2020-Control-0.2.0-Setup.exe` (IExpress, per-user).
  `scripts\package.ps1` rebuilds them; `.github\workflows\ci.yml` does fmt/clippy/test/build/package.

## Things I skipped or could not do (read these)

1. **Smart App Control started enforcing mid-session** and blocks newly compiled unsigned executables. I did
   not disable it (irreversible security downgrade; your call). Effects: debug-profile `cargo test` and new build
   scripts fail with `os error 4551` on this PC, so tests are run with `--release`, and the workspace has no build
   script (the icon is embedded post-build instead). Details: DECISIONS #36, SETUP_NEEDED.md.
2. **The microphone has been delivering exact digital silence since ~15:30** (raw capture and Windows' peak meter;
   endpoint mute is off, volume 100 %). Probably the AT2020USB-X's touch-mute or a privacy setting. The live
   RNNoise check passed earlier with a live mic (raw -52.3 dBFS -> processed -66.0 dBFS, 13.7 dB suppression); the
   screenshots show flat meters for the same reason.
3. **Installer not executed** (it would add a Start-menu entry and registry keys). Inno Setup is not installed and
   I did not install it; the IExpress fallback was used. CI uses Inno Setup.
4. **CI workflow has never run** (no GitHub access); it mirrors the local commands.
5. **Physical hot-plug and default-device changes** were exercised through the handler code paths (burst
   coalescing, vanished endpoint, re-bind, engine restart), not by unplugging hardware. See HUMAN_TESTS 9-10.
6. **Global hotkeys, tray menu and start-with-Windows** are implemented and compile/log cleanly but need a human
   (HUMAN_TESTS 16-18); the screenshot mode deliberately does not register hotkeys.
7. **Sample format is displayed, not selectable**: cpal picks the device's best and everything is converted to
   f32. Sample rate, buffer size and exclusive mode are selectable.
8. **No project licence is declared** (yours to choose); fonts ship with their OFL texts.
9. **Project folder was not a git repo**; I ran `git init` and committed after every phase (8+ commits, each
   rollback-able). Line endings warnings (LF/CRLF) are harmless.

## Verification snapshot

- Unit tests: 40 (30 core-dsp, 8 app: themes/settings, 2 device).
- Self-test (full, earlier build): 56 passed, 0 failed, 60.0 min soak: 0 xruns, ring fill 804/1437/1924 samples,
  CPU avg 0.23 % (of 20 logical CPUs), memory 20.3 -> 20.5 MiB, system state restored.
- Self-test (final build): see `SELFTEST_REPORT.md` (the soak line is the long pole).
- Cable round trip: flat to 0.00 dB (100 Hz-16 kHz), THD+N -110 dB, device-path latency about 110-120 ms in
  shared mode; exclusive mode estimate about 50 ms end to end.
