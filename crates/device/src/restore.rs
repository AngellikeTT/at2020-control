//! Crash-safe save/restore of endpoint state the app changes for testing (volume,
//! per-channel levels, mute, enhancement flag), plus a record of the default devices.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EndpointSnapshot {
    pub endpoint_id: String,
    pub name: String,
    pub volume_scalar: f32,
    pub channel_scalars: Vec<f32>,
    pub mute: bool,
    /// PKEY_AudioEndpoint_Disable_SysFx as stored (None = not set).
    pub sysfx_disabled: Option<u32>,
    /// Default capture endpoint ids for eConsole, eMultimedia, eCommunications.
    pub default_capture: Vec<Option<String>>,
    /// Default render endpoint ids for eConsole, eMultimedia, eCommunications.
    pub default_render: Vec<Option<String>>,
}

/// Reads and writes the state of one endpoint. The real implementation talks to Core
/// Audio; tests use a fake.
pub trait EndpointControl {
    fn read(&mut self, endpoint_id: &str) -> Result<EndpointSnapshot>;
    fn apply(&mut self, snap: &EndpointSnapshot) -> Result<()>;
}

/// What a recovery did.
#[derive(Clone, Debug, PartialEq)]
pub struct Recovery {
    pub restored: EndpointSnapshot,
    pub was: EndpointSnapshot,
    /// Things that differ but cannot be restored through a public API (default devices).
    pub warnings: Vec<String>,
}

pub fn default_journal_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| {
        PathBuf::from(d)
            .join("AT2020 Control Panel")
            .join("restore-journal.json")
    })
}

/// Writes the journal so that a crash at any point leaves either the old file or the
/// complete new one: temp file, flushed to disk, then renamed over the target.
fn write_atomic(path: &Path, snap: &EndpointSnapshot) -> Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f =
        std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    f.write_all(serde_json::to_string_pretty(snap)?.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn read_journal(path: &Path) -> Result<Option<EndpointSnapshot>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(
            serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Applies `snap`, reads the endpoint back and checks it took. Default-device changes
/// cannot be undone through a public Windows API (this app never makes any), so they
/// are only reported.
fn restore_to<C: EndpointControl>(ctl: &mut C, snap: &EndpointSnapshot) -> Result<Recovery> {
    let was = ctl.read(&snap.endpoint_id)?;
    ctl.apply(snap)?;
    let now = ctl.read(&snap.endpoint_id)?;
    let close = |a: f32, b: f32| (a - b).abs() <= 0.005;
    if !close(now.volume_scalar, snap.volume_scalar)
        || now.mute != snap.mute
        || now.channel_scalars.len() != snap.channel_scalars.len()
        || !now
            .channel_scalars
            .iter()
            .zip(&snap.channel_scalars)
            .all(|(a, b)| close(*a, *b))
    {
        bail!(
            "restore did not take: volume {:.3} (want {:.3}), mute {} (want {})",
            now.volume_scalar,
            snap.volume_scalar,
            now.mute,
            snap.mute
        );
    }
    let mut warnings = Vec::new();
    if now.sysfx_disabled != snap.sysfx_disabled {
        warnings.push(format!(
            "enhancement flag is {:?}, was {:?} (writing it needs administrator rights)",
            now.sysfx_disabled, snap.sysfx_disabled
        ));
    }
    if now.default_capture != snap.default_capture {
        warnings.push(format!(
            "default capture devices changed: {:?} -> {:?}",
            snap.default_capture, now.default_capture
        ));
    }
    if now.default_render != snap.default_render {
        warnings.push(format!(
            "default render devices changed: {:?} -> {:?}",
            snap.default_render, now.default_render
        ));
    }
    Ok(Recovery {
        restored: snap.clone(),
        was,
        warnings,
    })
}

/// Restores the state recorded by a journal left behind by a run that never finished
/// (killed, crashed, aborted) and removes it. `Ok(None)`: nothing was pending. On error
/// the journal is kept so a later launch can try again.
pub fn recover_pending<C: EndpointControl>(path: &Path, ctl: &mut C) -> Result<Option<Recovery>> {
    let Some(snap) = read_journal(path)? else {
        return Ok(None);
    };
    let rec = restore_to(ctl, &snap)
        .with_context(|| format!("restoring \"{}\" from {}", snap.name, path.display()))?;
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    Ok(Some(rec))
}

/// Records an endpoint's state on disk *before* the caller changes anything, and puts
/// it back on `restore()`, on drop (including unwinding from a panic) or, if the
/// process dies first, on the next launch through [`recover_pending`].
pub struct Journal<C: EndpointControl> {
    path: PathBuf,
    snap: EndpointSnapshot,
    ctl: C,
    done: bool,
}

impl<C: EndpointControl> Journal<C> {
    pub fn begin(path: &Path, mut ctl: C, endpoint_id: &str) -> Result<Self> {
        // A journal from a run that never finished holds the true original state:
        // restore it first, or the state that run left behind would be recorded instead.
        recover_pending(path, &mut ctl)?;
        let snap = ctl.read(endpoint_id)?;
        write_atomic(path, &snap)?;
        Ok(Self {
            path: path.to_path_buf(),
            snap,
            ctl,
            done: false,
        })
    }

    pub fn snapshot(&self) -> &EndpointSnapshot {
        &self.snap
    }

    /// Puts the recorded state back, verifies it and removes the journal.
    pub fn restore(&mut self) -> Result<Recovery> {
        let rec = restore_to(&mut self.ctl, &self.snap)?;
        self.done = true;
        std::fs::remove_file(&self.path)
            .with_context(|| format!("removing {}", self.path.display()))?;
        Ok(rec)
    }
}

impl<C: EndpointControl> Drop for Journal<C> {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.restore();
        }
    }
}

// ------------------------------------------------------------ Core Audio --

/// [`EndpointControl`] over Windows Core Audio. Each call runs on a short-lived MTA
/// thread of its own, so it works from any thread regardless of its COM apartment.
#[derive(Clone, Copy, Debug, Default)]
pub struct CoreAudio;

/// PKEY_AudioEndpoint_Disable_SysFx {E4870E26-3CC5-4CD2-BA46-CA0A9A70ED04},5
const KEY_DISABLE_SYSFX: windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY =
    windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0xE4870E26_3CC5_4CD2_BA46_CA0A9A70ED04),
        pid: 5,
    };

fn on_com_thread<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            // SAFETY: paired with CoInitializeEx on this thread.
            unsafe { CoUninitialize() }
        }
    }
    std::thread::spawn(move || {
        // SAFETY: initialises COM for this thread only; undone by `Com` on exit/unwind.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let _com = Com;
        f()
    })
    .join()
    .map_err(|_| anyhow::anyhow!("Core Audio worker thread panicked"))?
}

mod core_audio {
    use super::*;
    use windows::core::{HSTRING, PROPVARIANT};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, IMMDevice,
        IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ, STGM_READWRITE,
    };

    unsafe fn text(p: windows::core::PWSTR) -> String {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        s
    }

    pub(super) unsafe fn device(id: &str) -> Result<(IMMDeviceEnumerator, IMMDevice)> {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let d = en
            .GetDevice(&HSTRING::from(id))
            .with_context(|| format!("endpoint {id} not found"))?;
        Ok((en, d))
    }

    unsafe fn defaults(en: &IMMDeviceEnumerator, flow: EDataFlow) -> Vec<Option<String>> {
        [eConsole, eMultimedia, eCommunications]
            .into_iter()
            .map(|role| {
                en.GetDefaultAudioEndpoint(flow, role)
                    .ok()
                    .and_then(|d| d.GetId().ok().map(|p| text(p)))
            })
            .collect()
    }

    pub(super) unsafe fn read(id: &str) -> Result<EndpointSnapshot> {
        let (en, d) = device(id)?;
        let vol: IAudioEndpointVolume = d.Activate(CLSCTX_ALL, None)?;
        let n = vol.GetChannelCount()?;
        let mut ch = Vec::with_capacity(n as usize);
        for c in 0..n {
            ch.push(vol.GetChannelVolumeLevelScalar(c)?);
        }
        let store = d.OpenPropertyStore(STGM_READ)?;
        let name = store
            .GetValue(&PKEY_Device_FriendlyName)
            .and_then(|pv| PropVariantToStringAlloc(&pv))
            .map(|p| text(p))
            .unwrap_or_default();
        let sysfx = store
            .GetValue(&KEY_DISABLE_SYSFX)
            .ok()
            .and_then(|pv| u32::try_from(&pv).ok());
        Ok(EndpointSnapshot {
            endpoint_id: id.to_string(),
            name,
            volume_scalar: vol.GetMasterVolumeLevelScalar()?,
            channel_scalars: ch,
            mute: vol.GetMute()?.as_bool(),
            sysfx_disabled: sysfx,
            default_capture: defaults(&en, eCapture),
            default_render: defaults(&en, eRender),
        })
    }

    pub(super) unsafe fn apply(s: &EndpointSnapshot) -> Result<()> {
        let (_en, d) = device(&s.endpoint_id)?;
        let vol: IAudioEndpointVolume = d.Activate(CLSCTX_ALL, None)?;
        // Master first: it rescales every channel; the channel values then restore balance.
        vol.SetMasterVolumeLevelScalar(s.volume_scalar, std::ptr::null())?;
        let n = vol.GetChannelCount()?;
        for (c, v) in s.channel_scalars.iter().enumerate().take(n as usize) {
            vol.SetChannelVolumeLevelScalar(c as u32, *v, std::ptr::null())?;
        }
        vol.SetMute(s.mute, std::ptr::null())?;
        let now = d
            .OpenPropertyStore(STGM_READ)?
            .GetValue(&KEY_DISABLE_SYSFX)
            .ok()
            .and_then(|pv| u32::try_from(&pv).ok());
        if let (Some(want), true) = (s.sysfx_disabled, now != s.sysfx_disabled) {
            // Needs administrator rights; a failure is reported by the read-back check.
            if let Ok(store) = d.OpenPropertyStore(STGM_READWRITE) {
                let _ = store.SetValue(&KEY_DISABLE_SYSFX, &PROPVARIANT::from(want));
                let _ = store.Commit();
            }
        }
        Ok(())
    }
}

impl EndpointControl for CoreAudio {
    fn read(&mut self, endpoint_id: &str) -> Result<EndpointSnapshot> {
        let id = endpoint_id.to_string();
        // SAFETY: COM calls on a thread initialised by `on_com_thread`.
        on_com_thread(move || unsafe { core_audio::read(&id) })
    }
    fn apply(&mut self, snap: &EndpointSnapshot) -> Result<()> {
        let s = snap.clone();
        // SAFETY: as above.
        on_com_thread(move || unsafe { core_audio::apply(&s) })
    }
}

/// Id of the default endpoint (console role) for capture or render.
pub fn default_endpoint_id(capture: bool) -> Result<String> {
    on_com_thread(move || {
        use windows::Win32::Media::Audio::{
            eCapture, eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
        // SAFETY: COM calls on a thread initialised by `on_com_thread`.
        unsafe {
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let d =
                en.GetDefaultAudioEndpoint(if capture { eCapture } else { eRender }, eConsole)?;
            let p = d.GetId()?;
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
            Ok(s)
        }
    })
}

/// Startup hook for the app: restores whatever a crashed or killed earlier run left
/// changed. Returns a human-readable summary for the log, or None if nothing was pending.
pub fn recover_at_startup() -> Option<std::result::Result<String, String>> {
    let path = default_journal_path()?;
    match recover_pending(&path, &mut CoreAudio) {
        Ok(None) => None,
        Ok(Some(r)) => Some(Ok(format!(
            "restored \"{}\" left changed by an interrupted run: volume {:.0}% -> {:.0}%, mute {} -> {}{}",
            r.restored.name,
            r.was.volume_scalar * 100.0,
            r.restored.volume_scalar * 100.0,
            r.was.mute,
            r.restored.mute,
            if r.warnings.is_empty() {
                String::new()
            } else {
                format!(" ({})", r.warnings.join("; "))
            }
        ))),
        Err(e) => Some(Err(format!("{e:#}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Fake {
        state: Arc<Mutex<EndpointSnapshot>>,
        fail_apply: bool,
    }

    impl Fake {
        fn new(volume: f32, mute: bool) -> Self {
            Self {
                state: Arc::new(Mutex::new(EndpointSnapshot {
                    endpoint_id: "{mic}".into(),
                    name: "Microphone (Fake)".into(),
                    volume_scalar: volume,
                    channel_scalars: vec![volume],
                    mute,
                    sysfx_disabled: Some(0),
                    default_capture: vec![Some("{mic}".into()); 3],
                    default_render: vec![Some("{spk}".into()); 3],
                })),
                fail_apply: false,
            }
        }
        fn set(&self, volume: f32, mute: bool) {
            let mut s = self.state.lock().unwrap();
            s.volume_scalar = volume;
            s.channel_scalars = vec![volume];
            s.mute = mute;
        }
        fn get(&self) -> (f32, bool) {
            let s = self.state.lock().unwrap();
            (s.volume_scalar, s.mute)
        }
    }

    impl EndpointControl for Fake {
        fn read(&mut self, _id: &str) -> Result<EndpointSnapshot> {
            Ok(self.state.lock().unwrap().clone())
        }
        fn apply(&mut self, snap: &EndpointSnapshot) -> Result<()> {
            if self.fail_apply {
                anyhow::bail!("device gone");
            }
            let mut s = self.state.lock().unwrap();
            s.volume_scalar = snap.volume_scalar;
            s.channel_scalars = snap.channel_scalars.clone();
            s.mute = snap.mute;
            s.sysfx_disabled = snap.sysfx_disabled;
            Ok(())
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("at2020-journal-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("restore-journal.json")
    }

    #[test]
    fn journal_is_on_disk_before_the_first_change() {
        let p = tmp("on-disk");
        let fake = Fake::new(1.0, false);
        let j = Journal::begin(&p, fake.clone(), "{mic}").unwrap();
        assert!(
            p.exists(),
            "the saved state must be persisted before anything changes"
        );
        let on_disk: EndpointSnapshot =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(&on_disk, j.snapshot());
        drop(j);
        assert!(!p.exists(), "a completed restore removes the journal");
    }

    #[test]
    fn a_killed_run_is_restored_on_the_next_launch() {
        let p = tmp("killed");
        let fake = Fake::new(1.0, false);
        let j = Journal::begin(&p, fake.clone(), "{mic}").unwrap();
        fake.set(0.25, true);
        std::mem::forget(j); // process killed / panic=abort: no destructor runs
        assert_eq!(fake.get(), (0.25, true));

        let mut next_launch = fake.clone();
        let rec = recover_pending(&p, &mut next_launch).unwrap();
        let rec = rec.expect("a pending journal must be recovered");
        assert_eq!(rec.was.volume_scalar, 0.25);
        assert_eq!(fake.get(), (1.0, false));
        assert!(!p.exists());
        assert_eq!(recover_pending(&p, &mut next_launch).unwrap(), None);
    }

    #[test]
    fn begin_recovers_a_stale_journal_before_snapshotting() {
        let p = tmp("stale");
        let fake = Fake::new(0.8, false);
        let crashed = Journal::begin(&p, fake.clone(), "{mic}").unwrap();
        fake.set(0.25, true);
        std::mem::forget(crashed);
        // The next run must not record 25 % / muted as the "original" state.
        let j = Journal::begin(&p, fake.clone(), "{mic}").unwrap();
        assert_eq!(j.snapshot().volume_scalar, 0.8);
        assert!(!j.snapshot().mute);
        drop(j);
        assert_eq!(fake.get(), (0.8, false));
    }

    #[test]
    fn a_panicking_run_restores_on_unwind() {
        let p = tmp("panic");
        let fake = Fake::new(0.6, false);
        let f2 = fake.clone();
        let p2 = p.clone();
        let r = std::panic::catch_unwind(move || {
            let _j = Journal::begin(&p2, f2.clone(), "{mic}").unwrap();
            f2.set(0.1, true);
            panic!("test body failed");
        });
        assert!(r.is_err());
        assert_eq!(fake.get(), (0.6, false));
        assert!(!p.exists());
    }

    #[test]
    fn a_failed_restore_keeps_the_journal_for_later() {
        let p = tmp("fail");
        let fake = Fake::new(0.9, false);
        let j = Journal::begin(&p, fake.clone(), "{mic}").unwrap();
        fake.set(0.2, false);
        std::mem::forget(j);
        let mut broken = fake.clone();
        broken.fail_apply = true;
        assert!(recover_pending(&p, &mut broken).is_err());
        assert!(
            p.exists(),
            "keep the journal so a later launch can still restore"
        );
        let mut ok = fake.clone();
        assert!(recover_pending(&p, &mut ok).unwrap().is_some());
        assert_eq!(fake.get(), (0.9, false));
    }

    #[test]
    fn changed_default_devices_are_reported() {
        let p = tmp("defaults");
        let fake = Fake::new(1.0, false);
        let j = Journal::begin(&p, fake.clone(), "{mic}").unwrap();
        fake.state.lock().unwrap().default_render = vec![Some("{cable}".into()); 3];
        std::mem::forget(j);
        let rec = recover_pending(&p, &mut fake.clone()).unwrap().unwrap();
        assert!(
            rec.warnings.iter().any(|w| w.contains("render")),
            "{:?}",
            rec.warnings
        );
    }
}
