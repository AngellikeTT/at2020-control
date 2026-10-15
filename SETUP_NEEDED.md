# Setup needed before the build phases can start

Checked on 2026-10-15:

| Tool | Status |
|---|---|
| git | OK (2.55.0.windows.5) |
| MSVC build tools | OK (VS 2022 Community, VC tools 14.44.35207, C++ workload present) |
| rustup | **MISSING** (not on PATH, no `%USERPROFILE%\.rustup`) |
| cargo / rustc | **MISSING** (no `%USERPROFILE%\.cargo\bin`) |

## What to install

1. Install Rust via rustup (this is the only missing piece):
   - Download and run `rustup-init.exe` from https://rustup.rs
   - Choose the default: toolchain `stable`, host `x86_64-pc-windows-msvc`.
2. Open a **new** terminal so `%USERPROFILE%\.cargo\bin` is on PATH, then confirm:
   ```
   rustup --version
   cargo --version
   rustc --version
   ```
3. Add the components the phases need:
   ```
   rustup component add clippy rustfmt
   ```

No Visual Studio changes are needed; the "Desktop development with C++" workload is already installed.

## Not done (blocked)

Phases 1-5 were not started, since nothing can be compiled without cargo. Also note
`D:\Mic App - Claude` is not yet a git repository; run `git init` there (or let the
next session do it) before the first phase commit.

Once Rust is installed, re-run the same session prompt and it will begin at Phase 1.
