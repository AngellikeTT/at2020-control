# FIX_REPORT: the silent AT2020USB-X, crash safety, multi-core testing

Session of 2026-10-15 (evening, local time UTC-7). Everything below was done on this PC,
from an elevated shell, without stopping to ask. Raw evidence is in `probe-logs\`,
`fix-logs\` and `test-matrix\`.

## TL;DR

* **The microphone works.** The very first probe, before anything was changed, already
  captured a real signal (peak -5.1 dBFS, RMS -26.3 dBFS, 0 silent packets). Nothing on
  the system had to be repaired to get sound back.
* **Root cause of the silence (14:15-~20:20): the microphone itself sent zeros.** The
  most likely cause is its touch mute. That is not proven, because Windows cannot see that
  mute; but every software cause was tested and excluded. It was **not** the app's
  volume/mute/enhancement/default-device writes, **not** a stale or exclusive stream, and **not** the
  driver stack. The decisive evidence: during the silence, the self-test's *exclusive-mode*
  capture also read exact zeros. That path bypasses the Windows audio engine, every APO,
  the endpoint volume and mute, and Voice Clarity, and uses a different sample format, so
  only the USB device itself (or its driver) can have produced those zeros. The same path
  reads -1.7 dBFS today. See "Root cause".
* **The app did have real bugs** (all fixed, each with a test that failed first, each in
  its own commit):
  1. The self-test passed (66/66) on a dead microphone.
  2. The self-test's exclusive-mode phase silently cut Voicemeeter off the microphone. I reproduced
     this tonight and repaired it.
  3. Restoring the mic state was not crash-safe (in-memory guard + `panic = "abort"`).
  4. A panic in an audio callback left a dead stream that looked alive, leaked an event
     handle and skipped COM cleanup.
  5. `Ring::len()` read from a third thread could return 18446744073709551615.
  6. A `-0.0` sample pinned a peak meter and hid every later peak.
  7. Device control hung on shutdown after a burst of commands (found by the new stress test).
  8. The drift controller bent the pitch by up to 0.2 % whenever the capture and render
     callbacks swapped order (found by the single-core test matrix).
* **Physical action for you:** none is required now. If the mic ever goes silent again
  while Windows shows it unmuted at 100 %, tap the touch-mute pad on top of the AT2020USB-X
  and check its LED first (see "What needs you").

## 1. Backups (before any change)

| What | Result |
|---|---|
| `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio` | `backup\MMDevices-Audio.reg` (8.2 MB) |
| `HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager` | `backup\CapabilityAccessManager-HKCU.reg` (73 KB) |
| System Restore point | **Not created.** Windows allows one per 24 h and one already existed ("Windows Software Development Kit", 13:23 today). Lifting that limit needs a registry value outside the two allowed trees, so I skipped it. |

`backup\` is git-ignored (machine-specific).

## 2. Processes holding the mic or the cable

Found with the probe's session list (`IAudioSessionManager2`) and the per-app microphone
usage records:

| Process | State | Action |
|---|---|---|
| `at2020-control.exe` (dist build, PID 10200, started by you from Explorer at 20:22) | shared-mode session on the AT2020 | stopped (`Stop-Process`, our own app) |
| `voicemeeter_x64.exe` (PID 30236, running since 13:17) | shared-mode session on the AT2020, ACTIVE | left alone (third-party, yours) |
| any process in exclusive mode | none | — |

No stale self-test or probe process existed.

## 3. Device settings audit

All read back and compared with what the self-test had recorded (volume 100 %, mute off,
enhancements enabled, AT2020 default for all capture roles). **Nothing differed; nothing was changed.**

| Setting | Value | OK |
|---|---|---|
| Endpoint state | ACTIVE | yes |
| Endpoint volume | 100 % = +30.0 dB (range -13.0..+30.0, step 0.375) | yes (as recorded) |
| Channel levels | 1 channel, 100 % / +30.0 dB | yes |
| Endpoint mute | off | yes |
| Hardware topology | Microphone -> ADC -> **Volume** -> Device Specific -> SRC -> Capture: no mute node, **no AGC/boost node** | (informative) |
| Enhancements (`Disable_SysFx`) | 0 (enhancements on; FX pack: Windows *VocaEffectPack* / Voice Clarity) | as recorded |
| Shared-mode format | 48 kHz, 1 ch, 24-bit PCM | plausible device default |
| "Allow exclusive control" / "exclusive priority" | default (allowed) / on | default |
| Default capture device | AT2020USB-X for console, multimedia and communications | yes |
| Microphone privacy | global Allow (HKCU and HKLM), desktop apps Allow | yes |
| Registry key last written | AT2020 `Properties`: 14:14:06 (during the first self-test), nothing since | — |

## 4. Device-stack reset

**Skipped, on purpose.** The goal of the reset was to get a nonzero signal from the mic,
and that was already the case before I changed anything. Restarting audiosrv or doing
`pnputil /disable-device` + `/enable-device` would have done nothing for the mic but
invalidated every running stream: Voicemeeter's (which, as shown below, does not recover
by itself) and your Discord's. No driver or device was removed; nothing was rebooted.

## 5. Probe output before and after

`mic-probe.exe` (new, `crates/probe`): WASAPI shared mode, 5 s, independent of the engine.

Before anything was changed (20:28, `probe-logs\00-before-app-running.txt`):

```
chosen: Microphone (AT2020USB-X)
endpoint volume: scalar 1.000, 30.00 dB (range -13.0..30.0 step 0.375), mute false
Disable_SysFx: Some(0)
shared mix format: 48000 Hz, 2 ch, 32 bit float
captured 5.0 s: 480000 samples, 500 packets, 0 with AUDCLNT_BUFFERFLAGS_SILENT, 1 discontinuities
peak 0.557141 (-5.1 dBFS), RMS 0.048390 (-26.3 dBFS), exact zeros 0/480000 (0.0 %)
Windows meter peak during capture: 1.000000
RESULT: SIGNAL (nonzero samples)
```

After everything (final build, `probe-logs\99-after-final-build.txt`):

```
chosen: Microphone (AT2020USB-X)
endpoint volume: scalar 1.000, 30.00 dB (range -13.0..30.0 step 0.375), mute false
Disable_SysFx: Some(0)
shared mix format: 48000 Hz, 2 ch, 32 bit float
captured 5.0 s: 480960 samples, 501 packets, 0 with AUDCLNT_BUFFERFLAGS_SILENT, 1 discontinuities
peak 0.764968 (-2.3 dBFS), RMS 0.065020 (-23.7 dBFS), exact zeros 0/480960 (0.0 %)
Windows meter peak during capture: 0.833084
RESULT: SIGNAL (nonzero samples)
```

Same settings as before; signal throughout. All intermediate probes are in `probe-logs\`.

## 6. Root cause

### Timeline (local time)

| Time | Evidence |
|---|---|
| 13:15-13:17 | Reboot; Voicemeeter (x64) starts and stays running all day. |
| 14:14:06 | Last write to the AT2020 endpoint's registry `Properties` key, during the first self-test run (which value is not recorded; nothing wrote it afterwards). |
| 14:15 | That self-test reports a **live** mic (raw -52.3 dBFS, input peak -29.8 dBFS). |
| 14:15-14:32 | Only offline DSP work and `--selftest --quick` runs. |
| ~14:32 | Full self-test: raw mic **-200 dBFS (exact zeros)**, no stream error. |
| 15:35, 16:53, 17:59 | Self-tests: shared capture **and exclusive capture** read exact zeros. |
| all day | **No** PnP, USB, power, sleep or audio-endpoint event for the AT2020 in any event log. |
| 20:21 | Voicemeeter opens a new mic session; 20:22 you start the app; 20:23 Discord. |
| 20:28 | My first probe: **signal**. |

### What was excluded, and how

| Hypothesis | Test | Result |
|---|---|---|
| App left volume/mute changed | read back; replayed the self-test's exact volume/mute round trip on the live mic (`--roundtrip`, E1) | state correct; mic stayed live |
| The whole self-test does it | ran the real `--selftest --quick` (E2) on the live mic | 66/66, mic stayed live (-44.6 dBFS) |
| A leftover exclusive-mode stream | held the mic exclusively from a second process and probed (E3) | shared open **fails** with `AUDCLNT_E_DEVICE_IN_USE` (0x8889000A); it does not return zeros |
| Enhancements / APO / Voice Clarity, endpoint mute, privacy | all audited (section 3); and the exclusive path bypasses all of them yet read zeros | excluded |
| Driver / USB stall | a fresh exclusive stream (new pin, new format) read zeros at 15:35, 16:53, 17:59 | the samples came from the device |
| Device re-enumerated / reset | event logs | no event at all; recovery without any reset |

The AT2020USB-X's endpoint has no hardware mute node, so the mic's touch mute is
invisible to Windows: Windows shows "unmuted, 100 %" while the device sends exact zeros.
That matches every observation, and is consistent with the mic coming back exactly when
someone was at the PC again (20:21). It cannot be proven from software after the fact.

### What the app did get wrong

The app did not cause the silence, but its self-test hid it (PASS on -200 dBFS), and its
exclusive-mode phase *does* silence other apps: I reproduced that tonight (E2) on
Voicemeeter. With "Give exclusive mode applications priority" on (the Windows default),
the exclusive open invalidated Voicemeeter's stream (`AUDCLNT_E_DEVICE_INVALIDATED`,
0x88890004, E4). Voicemeeter's mic session ended at 20:36:57, the same second as Windows'
"Released exclusive mode resource", and its mic strip stayed at level 0. I restarted
Voicemeeter's audio engine through its official Remote API
(`Command.Restart`, = its "Restart Audio Engine" menu item); its strip went back to 0.18-0.24.

## 7. Changes

### To the system

| Change | Why | Undo |
|---|---|---|
| Stopped our own `at2020-control.exe` (PID 10200) | step 2 | start it again |
| Voicemeeter: `Command.Restart` via its Remote API (20:42) | repair the stream my E2 test had killed | none needed (same as its menu item) |
| Mic volume briefly 25/50/75 % and mute toggled (E1, E2), restored and verified | experiments | already restored (journal verified) |
| Crash test: a self-test killed at 25 %, restored by the next launch | verify the journal | already restored |
| Registry | **no value changed** in either tree | — |

Nothing else was touched: no driver, device, service, Smart App Control, Defender or UAC
change, no reboot.

### To the code (one commit per fix; failing test first, logs in `fix-logs\`)

| # | Commit | Bug | Test that failed first |
|---|---|---|---|
| 1 | `Self-test: fail on digital silence` | 66/66 PASS on a dead mic: "signal flows" was `out_peak.is_finite()` | `digital_silence_fails_the_mic_check`, `exclusive_check_fails_on_silence` |
| 2 | `Exclusive mode: never silently pre-empt` | exclusive phase killed Voicemeeter's mic stream | `exclusive_section_skips_when_it_would_preempt_another_app` |
| 3 | `Crash-safe restore: journal` | in-memory guard, `panic=abort`: a kill left the mic at 25 % | 5 of 6 journal tests (`a_killed_run_is_restored_on_the_next_launch`, ...) |
| 4 | `Streams fail safe` | callback panic: no Stop/CloseHandle/CoUninitialize, no fault, dead stream "Running" | `a_panicking_stream_body_is_stopped_and_reported`, `a_panicking_callback_is_contained_and_silenced` |
| 5 | `Ring: len() ... can no longer wrap` | observer saw `len = 18446744073709551615` | `len_seen_by_an_observer_is_bounded`, `loom_observer_len_never_exceeds_capacity` |
| 6 | `Meters: -0.0 no longer hides later peaks` | `fetch_max(-0.0)` pinned the meter | `negative_zero_does_not_hide_later_peaks`, `concurrent_peaks_are_never_lost`, `loom_meter_peak_survives_concurrent_take` |
| 7 | `Device control: drain the command queue` | shutdown hung behind a backlog after a command burst | `devctl_survives_concurrent_hot_swap_commands` ("shutdown hung") |
| 8 | `Drift controller: no pitch bend when callback order flips` | pitch bent up to 0.2 % when capture/render callback order flips; ~5 s pitch excursion after every start | `rt_drift_controller_ignores_callback_order` (999.407 vs 999.700 Hz) |

Audit of every path that writes device state:

| Path | Before | Now |
|---|---|---|
| Self-test volume/mute round trip | in-memory guard (never ran on panic/kill) | journal on disk before the first change; restored at end/unwind/next launch; verified by read-back |
| Exclusive open/close | cleanup only on the happy path | RAII (`ComScope`, `Event`, Stop guard), panics contained and reported; release on drop verified on real hardware (`exclusive_stream_is_released_on_drop_and_on_panic`) |
| Enhancement toggle (UI) | deliberate user action, persists by design | unchanged; journaled and restored if a test changes it |
| Default-device switching | the app has none (no `IPolicyConfig`) | defaults are journaled and any change is reported |
| UI volume / mute / mic-mute hotkey | deliberate user actions | unchanged |

## 8. Multi-core testing

`scripts\test-matrix.ps1`: the full suite (`cargo test --release --workspace`, 79 tests:
14 app, 40 core-dsp, 25 device) under `start /affinity <mask>` with
`--test-threads=N`, 5 runs per cell. CPU: Core Ultra 7 265K, 20 logical CPUs = 8
P-cores (LP 0,1,6-9,18,19) + 12 E-cores (LP 2-5,10-17, efficiency class 0 from
`GetSystemCpuSetInformation`). Per-run logs: `test-matrix\*.txt`, data `test-matrix\matrix.csv`.

**Final matrix: 100 / 100 runs PASS** (P = pass):

| affinity | threads=1 | threads=2 | threads=4 | threads=20 |
|---|---|---|---|---|
| 0x1 (one P-core) | PPPPP | PPPPP | PPPPP | PPPPP |
| 0x3 (two P-cores) | PPPPP | PPPPP | PPPPP | PPPPP |
| 0xF (2 P + 2 E) | PPPPP | PPPPP | PPPPP | PPPPP |
| all (0xFFFFF) | PPPPP | PPPPP | PPPPP | PPPPP |
| E-cores only (0x3FC3C) | PPPPP | PPPPP | PPPPP | PPPPP |

loom (model checking, `fix-logs\loom-final.txt`): 3 / 3 pass. The models cover the ring's
observer `len()`, SPSC order and integrity, and meter peak vs. a concurrent take.

What the stress tests cover:

- **Real time:** the engine's real processors run on one core, with a busy-loop
  thread pinned to every other core of the mask and MMCSS like the real callbacks.
  Scenarios: 48 kHz with a +1000 ppm clock and parameters changing every 37 ms;
  44.1 → 48 kHz at −500 ppm; callback order flipping. Each requires 0 underruns, no
  sample step above what a clean tone allows, pitch within 0.2 Hz (the drift
  controller) and a bounded ring.
- **Shared state:** concurrent parameter writers with engine hot-swaps (no torn
  sets), fault storms, ring and meter stress, DevCtl command storms.
- **Real devices:** run one at a time with `serial_test`: exclusive release on drop and
  on panic, journal restore on the real mic.

Earlier iterations of the matrix and stress runs found real bugs, all fixed with the
cause, without retries or added sleeps:

| Found | Cause | Fix |
|---|---|---|
| DevCtl shutdown hung (first stress run) | one command per pass + full re-enumeration per Select | drain queue each pass (commit 7) |
| rare underrun at start (~1/20, all cores) | harness: sim threads promoted to MMCSS only in their first callback, started 60 ms late behind normal-priority work, then ran missed periods back to back | threads pinned + promoted before the clock starts |
| 0x1 / 1 thread: pitch 998.3 Hz instead of 999.0 (3/3) | **engine**: the drift controller read the callback order as clock drift | phase-independent fill estimate, priming at equilibrium (commit `Drift controller: ...`) |
| 0x1 / 1 thread: `taken > 0` | test assumed parallelism | assertion removed; delivery still checked |

## 9. Final reports

**Live noise suppression:** `LIVE_NOISE_REPORT.md`, 30 s of real microphone audio, 10/10 PASS.

| | |
|---|---|
| mic signal | RMS -15.3 dBFS, peak +1.1 dBFS (live, not digital silence) |
| frames | 3000: 1544 noise-only (VAD < 0.1), 753 speech (VAD > 0.9) |
| room noise, Voice preset | -32.0 dBFS in -> -41.1 dBFS out = **9.1 dB reduction** (criterion >= 6 dB) |
| speech | -10.5 dBFS in -> -20.0 dBFS out (-9.5 dB, criterion -15..+10 dB) |
| system state | volume/mute/enhancements/defaults unchanged |

The peak of +1.1 dBFS means loud speech already clips with the endpoint at its maximum
(+30 dB). Lowering the Windows mic level to about 75 % (+10.6 dB) would leave headroom.
That is a suggestion only; I did not change it.

**Full self-test** (`SELFTEST_REPORT.md`, 62.2 min, 60-minute soak): **62 passed, 5 failed, 1 warning**.

- **Passed:**
  - soak: **0 xruns**, CPU 0.09 %, memory 17.6 -> 14.4 MiB, ring 1058/1647/2324
  - mic live; live noise 13.7 dB, speech preserved
  - journal restore verified; volume, mute, enhancements and defaults unchanged
- **Warning:** exclusive-mode section skipped because Voicemeeter is on the mic (by design).
- **The 5 failures, and what happened to them:**

  | Failure | Cause | Status |
  |---|---|---|
  | offline pipeline THD+N -50 dB at 4 rate pairs | **regression from my drift-controller fix**: the new estimate read the wall clock in the offline pump too | fixed (timing only on real-time streams); test `offline_pump_is_deterministic` failed first; values back to -150.9 / -89.5 / -147.2 / -55.5 dB |
  | cable THD+N -11.7 dB | other audio mixed into `CABLE Input`, the Windows default playback device; Steam, Wallpaper Engine and Discord all play into it (`mic-probe "cable input" --render-sessions`) | environmental; -110.2 dB in every re-test |

- **Re-test of the failed items only** (quick self-test without the soak, final build):
  - **62/62 PASS** (`fix-logs\retest-failed-4.md`).
  - A cable check that showed up in the re-tests ("stage via cable: low cut", 6.27 / 2.76 dB) was
    a pre-existing measurement flaw: multitone alignment is ambiguous by 20 ms periods, and the
    window included the padding. Fixed by reading the middle of the tone; 0.00 dB since.
  - The live noise check is content-dependent. Over 5 runs it gave 9.1, 13.7 and 35.6 dB (pass),
    -1.6 dB (fail, 10 s capture) and 0.2 dB (fail, only 2 s analysed: my too-short setting).
    When the room's non-speech sound is something RNNoise keeps (music or other audio from
    speakers), the Voice preset's make-up gain and EQ make it slightly louder. That is a
    property of the preset, reported here; the check was not loosened.
  - The full 60-minute soak was not repeated after these last fixes, which do not touch the
    real-time path (the offline-timing gate and the self-test's measurement window).

**Deliverable:** `dist\at2020-control.exe` (portable, icon embedded, built from this commit),
`dist\AT2020-Control-0.2.0-portable.zip` and `dist\AT2020-Control-0.2.0-Setup.exe`.
Smoke-tested (`--list-presets`); Smart App Control allowed it.

## 10. Rolling back

* **Registry** (only if something audio-related ever looks wrong; nothing was changed):
  double-click `backup\MMDevices-Audio.reg` (needs admin) and
  `backup\CapabilityAccessManager-HKCU.reg`, or `reg import <file>`, then restart the
  "Windows Audio" service or reboot. Importing re-adds and overwrites values; it does not
  delete values created later.
* **Code:** every fix is its own commit; `git revert <hash>` undoes one.

## 11. What needs you

1. **If the mic goes silent again while Windows shows it unmuted at 100 %:** look at
   the AT2020USB-X's mute LED and tap the touch pad; that is the only thing Windows
   cannot see. Run `target\release\mic-probe.exe`: "DIGITAL SILENCE" with a healthy
   endpoint means the device is sending zeros. If tapping does not help, try another
   USB port (a power or USB-controller reset was outside what I was allowed to do).
2. **Voicemeeter** captures the AT2020 all the time. Keep the app's exclusive mode off
   while Voicemeeter runs (the app now says so when you turn it on), or Voicemeeter's mic strip goes silent
   until you use its "Restart Audio Engine" menu.
3. The System Restore point could not be created today (24 h limit).
