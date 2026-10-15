# DECISIONS

Session constraints: no shell on the Windows PC; cloud container is Linux; static.rust-lang.org blocked (no Windows std target); no ALSA dev package.

1. **Verification harnesses instead of a Windows build.** Compiled the platform-neutral modules (params, denoise, dsp, resample, ring, shared, settings, widgets, app with stubbed devctl/engine) on Linux against the pinned eframe 0.29 / nnnoiseless 0.5. Compiled engine.rs against cpal 0.15 with a fake alsa.pc (type-check only; cpal's API surface is the same on WASAPI). Alternative (upgrading crates) rejected per instructions.
2. **devctl.rs changed from windows 0.58 source, NOT compiled.** `PROPVARIANT` is `windows::core::PROPVARIANT` (opaque newtype); raw field access removed in favour of `PROPVARIANT::from(u32)` / `u32::try_from(&pv)`. `PropVariantToStringAlloc` import moved to `Win32::System::Com::StructuredStorage`. Removed the unused `Win32_System_Variant` feature. IMMNotificationClient_Impl signatures were checked against windows-0.58.0 impl.rs and already matched.
3. **Clippy fixes:** 0.7071 -> `FRAC_1_SQRT_2` (changes the Q by <0.01%, tests unchanged); derived `Default` for `ChainParams` (identical to the hand-written impl); `Stroke::new(1.0_f32, ..)` to silence the float_literal_f32_fallback future-incompat warning; two `needless_range_loop` allows (indexing parallel buffers / index used as widget id); `map_or(false, ..)` -> `is_some_and`; fixed a borrow error in app.rs (mute colour read before the mutable toggle borrow).
4. No test was changed or weakened.
5. Git: no git on the PC side; no commit was made in D:\Mic App - Claude.
6. Phases 2-5 not started: they need a Windows toolchain/hardware, and writing thousands of lines blind on never-compiled code was judged worse than stopping.
