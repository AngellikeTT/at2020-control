//! Concurrency, real-time and real-device tests.
//!
//! Anything that touches a real audio device or system state, or saturates the CPU,
//! is `#[serial(system)]` so parallel test runs never fight over the mic, the cable or
//! the restore journal. Tests needing hardware that is absent (CI) return early.

use crate::rtsim::{self, SimConfig};
use crate::shared::Shared;
use core_dsp::params::ChainParams;
use serial_test::serial;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

// --------------------------------------------------------------- real time --

fn assert_clean(name: &str, r: &rtsim::SimReport) {
    println!("{name}: {r:#?}");
    let p = r.problems();
    assert!(p.is_empty(), "{name}: {p:?}\n{r:#?}");
}

/// The engine's audio path at 48 kHz with every other core saturated, the render
/// clock 1000 ppm fast and parameters changing every 37 ms: no xrun, no glitch, and the
/// drift controller locks the pitch.
#[test]
#[serial(system)]
fn rt_no_xruns_or_glitches_with_cpu_saturated() {
    let r = rtsim::run(&SimConfig {
        out_ppm: 1000.0,
        toggle_params: true,
        ..SimConfig::default()
    });
    assert!(
        r.param_changes > 50,
        "parameter thread ran: {}",
        r.param_changes
    );
    assert_clean("48k->48k +1000ppm, load, params", &r);
}

/// The capture and render callbacks keep swapping order within the period (their
/// phases cross): the drift controller must not mistake that frame-sized jump in the
/// measured ring fill for clock drift, or it bends the pitch.
#[test]
#[serial(system)]
fn rt_drift_controller_ignores_callback_order() {
    let r = rtsim::run(&SimConfig {
        out_ppm: 300.0,
        capture_jitter_ms: 2.0,
        secs: 8.0,
        ..SimConfig::default()
    });
    assert_clean("48k->48k +300ppm, callback order flipping", &r);
}

/// Same with sample-rate conversion on both sides and a slow render clock.
#[test]
#[serial(system)]
fn rt_resampling_path_under_load_with_slow_clock() {
    let r = rtsim::run(&SimConfig {
        in_rate: 44_100,
        out_rate: 48_000,
        out_ppm: -500.0,
        ..SimConfig::default()
    });
    assert_clean("44.1k->48k -500ppm, load", &r);
}

// -------------------------------------------------------- shared state ---

/// UI threads publish parameter sets while the engine is restarted over and over
/// (every restart attaches a new reader = device hot-swap): every set a reader sees
/// must be one that was published whole (no torn reads), and nothing may deadlock.
#[test]
fn params_survive_concurrent_writers_and_hot_swaps() {
    // Every published set is a function of one value, so a reader can tell a whole
    // set from a torn one by rebuilding it.
    fn set_for(v: f32) -> ChainParams {
        let mut p = ChainParams::default();
        if v != 0.0 {
            p.input.gain_db = v;
            p.output.gain_db = -v;
            p.comp.threshold_db = -v / 2.0;
            p.eq.bands[1].gain_db = v / 4.0;
        }
        p
    }
    let s = Arc::new(Shared::new(set_for(0.0)));
    let done = Arc::new(AtomicBool::new(false));
    let writers: Vec<_> = (0..2)
        .map(|w| {
            let s = s.clone();
            std::thread::spawn(move || {
                for k in 1..=30_000u32 {
                    s.params.set(set_for((k * 2 + w) as f32));
                }
            })
        })
        .collect();
    let engine = {
        let (s, done) = (s.clone(), done.clone());
        std::thread::spawn(move || {
            let mut reads = 0u64;
            while !done.load(Relaxed) {
                let mut r = s.params.attach();
                for _ in 0..200 {
                    let p = *r.read();
                    assert_eq!(p, set_for(p.input.gain_db), "torn parameter set");
                    reads += 1;
                }
            }
            reads
        })
    };
    let ui_reader = {
        let (s, done) = (s.clone(), done.clone());
        std::thread::spawn(move || {
            while !done.load(Relaxed) {
                let p = s.params.current();
                assert_eq!(p, set_for(p.input.gain_db), "torn parameter set");
            }
        })
    };
    for w in writers {
        w.join().unwrap();
    }
    done.store(true, Relaxed);
    let reads = engine.join().unwrap();
    ui_reader.join().unwrap();
    assert!(reads > 0);
    // After the writers stop, a fresh reader sees the final published set.
    let last = *s.params.attach().read();
    assert_eq!(last, s.params.current());
}

/// Audio threads report faults while the UI drains them: no deadlock, every message
/// delivered is one that was sent, and a fault raised after the storm is delivered.
#[test]
fn faults_are_delivered_under_contention() {
    let s = Arc::new(Shared::new(ChainParams::default()));
    let setters: Vec<_> = (0..4)
        .map(|t| {
            let s = s.clone();
            std::thread::spawn(move || {
                for k in 0..20_000 {
                    s.set_fault(format!("t{t}-{k}"));
                }
            })
        })
        .collect();
    let mut taken = 0u32;
    while setters.iter().any(|h| !h.is_finished()) {
        if let Some(m) = s.take_fault() {
            assert!(m.starts_with('t') && m.contains('-'), "{m}");
            taken += 1;
        }
    }
    for h in setters {
        h.join().unwrap();
    }
    let _ = s.take_fault();
    s.set_fault("device unplugged".into());
    assert_eq!(s.take_fault().as_deref(), Some("device unplugged"));
    assert_eq!(s.take_fault(), None);
    // `taken` may legitimately be 0: on a single core the setters can all finish before
    // the draining thread runs once. Delivery is what the lines above check.
    let _ = taken;
}

// ------------------------------------------------------------ real devices --

/// Device control under a storm of concurrent hot-plug notifications, selection
/// changes and snapshot reads (read-only: nothing on the device is changed).
#[test]
#[serial(system)]
fn devctl_survives_concurrent_hot_swap_commands() {
    use crate::devctl::{Cmd, DevCtl};
    let dev = Arc::new(DevCtl::spawn(Arc::new(|| {})));
    let t0 = Instant::now();
    while dev.snapshot().captures.is_empty() && t0.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let Some(id) = dev.snapshot().captures.first().map(|c| c.id.clone()) else {
        println!("no capture device; skipped");
        return;
    };
    let stop = Arc::new(AtomicBool::new(false));
    let threads: Vec<_> = (0..4)
        .map(|t| {
            let (dev, stop, id) = (dev.clone(), stop.clone(), id.clone());
            std::thread::spawn(move || {
                let mut i = 0u32;
                while !stop.load(Relaxed) {
                    match (t + i) % 4 {
                        0 => dev.send(Cmd::Refresh),
                        1 => dev.send(Cmd::Select(id.clone())),
                        2 => dev.send(Cmd::Select("{0.0.1.00000000}.{bogus}".into())),
                        _ => {
                            let s = dev.snapshot();
                            assert!(s.captures.len() < 10_000);
                        }
                    }
                    i += 1;
                    std::thread::sleep(Duration::from_micros(300));
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(1500));
    stop.store(true, Relaxed);
    for t in threads {
        t.join().unwrap();
    }
    dev.send(Cmd::Select(id.clone()));
    let t0 = Instant::now();
    loop {
        let s = dev.snapshot();
        if s.has_endpoint && s.selected_id.as_deref() == Some(id.as_str()) {
            break;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "device not re-bound after the storm: {:?}",
            s.message
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let dev = Arc::try_unwrap(dev).ok().expect("all threads joined");
    let t0 = Instant::now();
    drop(dev); // joins the COM thread
    assert!(t0.elapsed() < Duration::from_secs(3), "shutdown hung");
}

fn cable_input() -> Option<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    cpal::default_host()
        .output_devices()
        .ok()?
        .filter_map(|d| d.name().ok())
        .find(|n| n.starts_with("CABLE Input"))
}

/// Exclusive streams are released when dropped and after their callback panics: the
/// endpoint can be reopened exclusively right away, and then in shared mode.
/// Uses VB-CABLE's playback side (renders silence); skipped if another app is
/// streaming to it (exclusive mode would pre-empt that app) or the cable is absent.
#[test]
#[serial(system)]
fn exclusive_stream_is_released_on_drop_and_on_panic() {
    use crate::wasapi_excl::{open_render, Fault};
    let Some(name) = cable_input() else {
        println!("no VB-CABLE; skipped");
        return;
    };
    let me = std::process::id();
    let others = crate::sessions::preempted_by_exclusive(
        &crate::sessions::endpoint_sessions(&name, false).unwrap_or_default(),
        me,
    );
    if !others.is_empty() {
        println!(
            "skipped: {} streaming to {name}",
            crate::sessions::describe(&others)
        );
        return;
    }
    let faults = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let f2 = faults.clone();
    let fault: Fault = Arc::new(move |m| f2.lock().unwrap().push(m));
    let silent = |_: &crate::wasapi_excl::Negotiated| |b: &mut [f32]| b.fill(0.0);

    for round in 0..3 {
        let (s, n) = open_render(&name, None, silent, fault.clone())
            .unwrap_or_else(|e| panic!("exclusive open #{round} failed (not released?): {e:#}"));
        assert!(n.buffer_frames > 0);
        std::thread::sleep(Duration::from_millis(150));
        drop(s);
    }

    // Panic inside the callback: the stream thread must stop, release and report.
    let (s, _) = open_render(
        &name,
        None,
        |_| {
            let mut calls = 0u32;
            move |b: &mut [f32]| {
                calls += 1;
                b.fill(0.0);
                if calls == 5 {
                    panic!("synthetic callback failure");
                }
            }
        },
        fault.clone(),
    )
    .expect("exclusive open for the panic test");
    let t0 = Instant::now();
    while !faults
        .lock()
        .unwrap()
        .iter()
        .any(|m| m.contains("panicked"))
    {
        assert!(t0.elapsed() < Duration::from_secs(3), "panic not reported");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(s);
    let (s, _) = open_render(&name, None, silent, fault.clone())
        .expect("exclusive reopen after a callback panic (stream not released?)");
    drop(s);

    // And shared mode works again afterwards.
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let dev = cpal::default_host()
        .output_devices()
        .unwrap()
        .find(|d| d.name().ok().as_deref() == Some(name.as_str()))
        .unwrap();
    let cfg = dev.default_output_config().unwrap().config();
    let st = dev
        .build_output_stream(
            &cfg,
            |b: &mut [f32], _: &cpal::OutputCallbackInfo| b.fill(0.0),
            |e| panic!("{e}"),
            None,
        )
        .expect("shared open after exclusive use");
    st.play().unwrap();
    std::thread::sleep(Duration::from_millis(100));
}

/// The journal against the real default microphone: record, change the volume
/// slightly (never mute: other apps may be using the mic), "crash", recover.
#[test]
#[serial(system)]
fn journal_restores_the_real_default_microphone() {
    use crate::restore::{
        default_endpoint_id, recover_pending, CoreAudio, EndpointControl, Journal,
    };
    let Ok(id) = default_endpoint_id(true) else {
        println!("no capture endpoint; skipped");
        return;
    };
    let path = std::env::temp_dir().join(format!(
        "at2020-journal-real-{}/restore-journal.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let j = Journal::begin(&path, CoreAudio, &id).expect("journal begin");
    let orig = j.snapshot().clone();
    // Whatever happens below, put the original back.
    let o2 = orig.clone();
    let _safety = crate::guard::OnDrop::new(move || {
        let _ = CoreAudio.apply(&o2);
    });
    let mut changed = orig.clone();
    let delta = if orig.volume_scalar > 0.5 {
        -0.02
    } else {
        0.02
    };
    changed.volume_scalar = (orig.volume_scalar + delta).clamp(0.0, 1.0);
    changed.channel_scalars = orig
        .channel_scalars
        .iter()
        .map(|v| (v + delta).clamp(0.0, 1.0))
        .collect();
    CoreAudio.apply(&changed).expect("change volume");
    let now = CoreAudio.read(&id).unwrap();
    assert!(
        (now.volume_scalar - changed.volume_scalar).abs() < 0.01,
        "change took"
    );
    std::mem::forget(j); // simulated crash
    let rec = recover_pending(&path, &mut CoreAudio)
        .expect("recover")
        .expect("pending journal");
    assert!((rec.was.volume_scalar - changed.volume_scalar).abs() < 0.01);
    let after = CoreAudio.read(&id).unwrap();
    assert!((after.volume_scalar - orig.volume_scalar).abs() < 0.006);
    assert_eq!(after.mute, orig.mute);
    assert!(!path.exists());
}
