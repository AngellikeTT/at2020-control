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
2. **The microphone works again** (since ~20:20 on 2026-10-15). From ~14:30 to ~20:20 it
   delivered exact digital silence; `FIX_REPORT.md` shows that the zeros came from the device
   itself, most likely its touch mute, which Windows cannot see. If it happens again while
   Windows shows the mic unmuted at 100 %, check the mute LED / tap the pad, then run
   `target\release\mic-probe.exe` (prints "DIGITAL SILENCE" in that case). The self-test now
   fails on digital silence instead of passing.
3. You have Voicemeeter installed next to VB-CABLE. The app now prefers `CABLE Input` when
   choosing an output; Voicemeeter's devices are still listed and flagged as virtual.
   Voicemeeter captures the AT2020 continuously, so keep the app's **exclusive mode off** while
   it runs. Exclusive mode cuts Voicemeeter's mic stream, and Voicemeeter stays silent until
   you use its *Restart Audio Engine* menu item. The app warns about this, and the self-test skips that part.
