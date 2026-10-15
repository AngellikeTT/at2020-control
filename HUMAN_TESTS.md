# HUMAN_TESTS
1. `cargo build --release` on Windows succeeds. Expected: no errors (report devctl.rs errors if any).
2. App launches, finds AT2020USB-X, volume slider moves Windows' mic volume; mute toggles Windows mute.
3. Output = CABLE Input; Discord/OBS input = CABLE Output; voice is audible, no crackle.
