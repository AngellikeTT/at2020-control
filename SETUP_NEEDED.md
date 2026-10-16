# Setup status

Everything needed to build is installed. Nothing further is **required**.

| Tool | Status |
|---|---|
| git | 2.55 (was present) |
| MSVC build tools | VS 2022 Community, VC 14.44 (was present) |
| Rust | **installed this session** per-user via rustup: stable 1.99.1 `x86_64-pc-windows-msvc`, with clippy and rustfmt (`%USERPROFILE%\.cargo`, `%USERPROFILE%\.rustup`) |
| Inno Setup | not installed (optional): `scripts\package.ps1` falls back to an IExpress installer; CI installs Inno Setup itself |

## Worth knowing / optional

1. **Smart App Control is now enforcing on this PC** (it was in evaluation mode earlier in the
   day and switched during this session). It blocks *newly compiled unsigned executables* it has no
   reputation for: debug-profile test binaries and new Cargo build scripts failed with
   `os error 4551`. I did not turn it off (a security downgrade that is irreversible without an OS
   reset, so it is yours to decide). What works: `cargo build --release`, `cargo test --release`,
   `cargo clippy`, and running freshly built release executables, although the verdict is per file hash and not
   always the same: when a new exe was blocked, rebuilding (the build id changes the hash) was enough. If you want normal `cargo test`
   (debug) and arbitrary new build scripts here, either disable Smart App Control in
   *Windows Security -> App & browser control -> Smart App Control*, or sign the binaries with a
   certificate Windows trusts.
2. **The microphone has been delivering digital silence since about 14:35** (both the raw
   WASAPI capture and Windows' own peak meter read exact zeros; endpoint mute is *off* and
   volume is 100 %). The AT2020USB-X has a touch-sensitive mute: check its LED / tap it, and check
   *Windows Settings -> Privacy -> Microphone*. The automated tests that need real mic content
   (RNNoise reduction on live noise) ran earlier in the day when the mic was live
   (raw -52.3 dBFS, 13.7 dB suppression); later runs only prove the plumbing.
3. You have Voicemeeter installed next to VB-CABLE. The app now prefers `CABLE Input` when
   choosing an output; Voicemeeter's devices are still listed and flagged as virtual.
