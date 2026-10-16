# HUMAN_TESTS

Things that need ears, hands or other apps. Everything measurable was automated in
`--selftest` (see SELFTEST_REPORT.md). Tick each when done.

## Listening
1. **Voice preset sounds right.** Start the app, speak normally with the *Voice (Discord / calls)*
   preset. Expected: clear, not boomy, no pumping, no clicks when you move sliders.
2. **A/B compare.** Press **B**, change the EQ or compressor, flip A/B while talking. Expected: instant,
   click-free switching; each slot keeps its own settings (also after restarting the app).
3. **De-esser.** Say "sixty-six sassy sisters" with the de-esser off then on; use the Spectrum tab
   (marker at the de-ess frequency). Expected: sibilance softer, the rest unchanged; no lisp at default
   settings. Tune frequency (5-9 kHz differs between voices).
4. **VAD gate vs noise gate.** Play fan/keyboard noise while silent. Expected: the VAD indicator stays
   below the marker and the gate shuts even when the noise is loud. Check word starts are not clipped
   (raise *Hold* if they are).
5. **Test recording.** Audio tab -> Record 10 s "After chain", then "Raw". Expected: both WAVs play in the
   folder opened by *Open folder*; the processed one is quieter in background noise.

## Other apps
6. **Discord / OBS / Teams.** Select `CABLE Output` as the microphone. Expected: others hear the processed
   voice; Discord's own noise suppression can be set to *off* to avoid double processing.
7. **Do other apps get the raw mic?** Select the AT2020USB-X directly in another app while this app runs
   (shared mode). Expected: works. With *Exclusive mode* on, the other app is expected to fail; that is the
   documented trade-off.
8. **System sound leakage.** With `CABLE Input` as the Windows default playback device the Cable setup
   tab warns. Set your headphones as default playback and confirm only the voice reaches the cable.

## Hardware / Windows behaviour
9. **Unplug the USB mic while running**, wait 5 s, replug. Expected: status shows the mic is gone, the
   engine restarts by itself, no crash, volume slider re-attaches.
10. **Change the default recording/playback device in Windows Sound settings while running.** Expected: lists
    update, selection is kept, the engine keeps running.
11. **Mute from Windows** (taskbar, Sound settings) and from the app. Expected: both stay in sync within ~0.3 s.
12. **Audio enhancements flag.** Toggle in the left panel. Expected: either it changes, or the message says
    administrator rights are needed (driver dependent). Note what your driver reports.
13. **Mic hardware controls** (gain knob, headphone volume, mute touch) are not reachable from software;
    confirm nothing in the app pretends otherwise.
14. **Exclusive mode.** Audio tab -> tick *Use WASAPI exclusive mode*. Expected: engine restarts; the
    "Mode" line shows exclusive for each side that accepted, or a plain-language reason for a fallback.
    Compare the latency readout.
15. **Fixed buffer sizes.** Try the smallest buffer the slider offers; watch the xrun counter while moving the
    window around. Expected: raise the buffer if xruns appear.

## Desktop integration
16. **Tray.** Close the window with the X. Expected: app keeps running in the tray; left click on the icon
    restores it; right-click menu works; *Quit* exits.
17. **Start with Windows.** System tab -> tick it, sign out and in. Expected: app starts hidden in the tray
    (`--minimized`), engine running. Untick and confirm the Run entry is gone.
18. **Global hotkeys** (defaults Ctrl+Alt+M output mute, Ctrl+Alt+B bypass, Ctrl+Alt+Shift+M mic mute) while
    another app has focus, including a borderless-window game. Expected: toggles; a conflicting hotkey
    shows a message on the System tab.
19. **Installer.** Run `dist\AT2020-Control-<version>-Setup.exe` (per-user). Expected: Start-menu entry,
    entry in "Installed apps", uninstall removes both. (Not executed during development because it
    changes the machine.)
20. **Smart App Control.** Unsigned builds may be blocked on machines with Smart App Control enforcing. If
    the installer or exe is blocked, that is an OS reputation decision (see DECISIONS.md #36), not a bug.

## Look and feel
21. **Themes.** Appearance tab: cycle all nine themes and "Follow Windows" (switch Windows between light and
    dark while the app runs). Expected: text readable everywhere, title bar follows.
22. **User theme.** Save a custom theme, edit its TOML in a text editor, save the file. Expected: the app
    picks the change up within about a second.
23. **High DPI / second monitor.** Drag the window between monitors with different scaling; check the graphs
    and meters stay crisp and the layout does not clip at 1000 px wide.
24. **Meter styles.** Bar, Segments, Needle; check readability at a glance while speaking at normal level.

## Silent-mic follow-ups (added with FIX_REPORT.md)
26. **Touch mute is invisible to Windows.** Tap the AT2020USB-X mute pad while running
    `target\release\mic-probe.exe` (or watch the app's input meter). Expected: LED changes, the probe
    reports "DIGITAL SILENCE" while Windows still shows the mic unmuted at 100 %; tap again and the
    signal returns. This confirms the root cause in FIX_REPORT.md.
27. **Exclusive mode next to Voicemeeter.** With Voicemeeter running, turn on exclusive mode in the Audio
    tab. Expected: the note names `voicemeeter_x64.exe`; Voicemeeter's mic strip goes silent; turning
    exclusive mode off and using Voicemeeter's *Restart Audio Engine* brings it back.
28. **Crash recovery.** Start `at2020-control.exe --selftest --quick` and end it in Task Manager while the mic
    volume is below 100 % (first ~2 s). Expected: the mic stays changed until you start the app again, which
    restores it (the log says "restored ... left changed by an interrupted run").

## Long run
25. **Overnight.** Leave the app running overnight with a call or music through it. Expected: 0 xruns, memory
    flat (the automated 60-minute soak measured about 20 MiB working set with no growth).
