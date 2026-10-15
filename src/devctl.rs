//! Windows Core Audio device control via windows-rs.
//!
//! One dedicated COM (MTA) thread owns every COM object:
//!   * IMMDeviceEnumerator            - list capture/render endpoints
//!   * IMMNotificationClient          - hot-plug / default-device / property notifications
//!   * IAudioEndpointVolume           - the mic's Windows-level volume (scalar + dB) and mute
//!   * IAudioMeterInformation         - Windows-side peak meter
//!   * IPropertyStore                 - friendly name, "audio enhancements" (SysFx) flag
//! The UI talks to it through a command channel and reads a shared `DevState` snapshot.

use eframe::egui;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{implement, Result as WinResult, GUID, PCWSTR, PROPVARIANT};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ, STGM_READWRITE,
};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;

/// PKEY_AudioEndpoint_Disable_SysFx {E4870E26-3CC5-4CD2-BA46-CA0A9A70ED04},5 (1 = enhancements disabled)
const KEY_DISABLE_SYSFX: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xE4870E26_3CC5_4CD2_BA46_CA0A9A70ED04),
    pid: 5,
};

#[derive(Clone, Debug, Default)]
pub struct EndpointInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub virtual_cable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DevState {
    pub captures: Vec<EndpointInfo>,
    pub renders: Vec<EndpointInfo>,
    pub selected_id: Option<String>,
    pub selected_name: Option<String>,
    pub has_endpoint: bool,
    pub volume: f32,
    pub volume_db: f32,
    pub db_min: f32,
    pub db_max: f32,
    pub mute: bool,
    pub peak: f32,
    pub sysfx_disabled: Option<bool>,
    pub message: Option<String>,
    /// Bumped whenever the device lists or selection change.
    pub revision: u64,
}

pub enum Cmd {
    Refresh,
    Select(String),
    SetVolume(f32),
    SetMute(bool),
    SetSysFxDisabled(bool),
    Shutdown,
}

pub struct DevCtl {
    tx: Sender<Cmd>,
    pub state: Arc<Mutex<DevState>>,
    join: Option<JoinHandle<()>>,
}

impl DevCtl {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(DevState::default()));
        let (st, tx2) = (state.clone(), tx.clone());
        let join = std::thread::Builder::new()
            .name("devctl-com".into())
            .spawn(move || run(rx, tx2, st, ctx))
            .ok();
        Self { tx, state, join }
    }
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
    pub fn snapshot(&self) -> DevState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for DevCtl {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

// ------------------------------------------------------ notifications -----

#[implement(IMMNotificationClient)]
struct Notifier {
    tx: Mutex<Sender<Cmd>>,
}
impl Notifier {
    fn poke(&self) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(Cmd::Refresh);
        }
    }
}
#[allow(non_snake_case)]
impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> WinResult<()> {
        self.poke();
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> WinResult<()> {
        self.poke();
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> WinResult<()> {
        self.poke();
        Ok(())
    }
    fn OnDefaultDeviceChanged(
        &self,
        _flow: EDataFlow,
        _role: ERole,
        _id: &PCWSTR,
    ) -> WinResult<()> {
        self.poke();
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> WinResult<()> {
        // Fires for many unrelated keys; volume/mute are polled, so ignore.
        Ok(())
    }
}

// --------------------------------------------------------------- helpers ---

unsafe fn pwstr_to_string(p: windows::core::PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
    s
}

unsafe fn device_id(d: &IMMDevice) -> WinResult<String> {
    Ok(pwstr_to_string(d.GetId()?))
}

unsafe fn friendly_name(d: &IMMDevice) -> WinResult<String> {
    let store = d.OpenPropertyStore(STGM_READ)?;
    let pv = store.GetValue(&PKEY_Device_FriendlyName)?;
    Ok(pwstr_to_string(PropVariantToStringAlloc(&pv)?))
}

unsafe fn read_sysfx_disabled(d: &IMMDevice) -> Option<bool> {
    let store = d.OpenPropertyStore(STGM_READ).ok()?;
    let pv = store.GetValue(&KEY_DISABLE_SYSFX).ok()?;
    // Fails (None) if the stored value is not a VT_UI4.
    u32::try_from(&pv).ok().map(|v| v == 1)
}

unsafe fn write_sysfx_disabled(d: &IMMDevice, disabled: bool) -> WinResult<()> {
    let store = d.OpenPropertyStore(STGM_READWRITE)?;
    let pv = PROPVARIANT::from(u32::from(disabled));
    store.SetValue(&KEY_DISABLE_SYSFX, &pv)?;
    store.Commit()
}

fn is_virtual_cable(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "cable input",
        "voicemeeter",
        "virtual audio",
        "vb-audio",
        "hi-fi cable",
    ]
    .iter()
    .any(|k| n.contains(k))
}

type Listed = Vec<(EndpointInfo, IMMDevice)>;

unsafe fn enum_flow(en: &IMMDeviceEnumerator, flow: EDataFlow) -> WinResult<Listed> {
    let default_id = en
        .GetDefaultAudioEndpoint(flow, eConsole)
        .ok()
        .and_then(|d| device_id(&d).ok());
    let col = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
    let mut out = Vec::new();
    for i in 0..col.GetCount()? {
        let d = col.Item(i)?;
        let id = device_id(&d)?;
        let name = friendly_name(&d).unwrap_or_else(|_| id.clone());
        out.push((
            EndpointInfo {
                is_default: default_id.as_deref() == Some(id.as_str()),
                virtual_cable: is_virtual_cable(&name),
                id,
                name,
            },
            d,
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------- worker ---

struct Worker {
    en: IMMDeviceEnumerator,
    client: IMMNotificationClient,
    captures: Listed,
    want_id: Option<String>,
    device: Option<IMMDevice>,
    vol: Option<IAudioEndpointVolume>,
    meter: Option<IAudioMeterInformation>,
    state: Arc<Mutex<DevState>>,
    ctx: egui::Context,
    refresh_at: Option<Instant>,
    revision: u64,
}

impl Worker {
    fn new(tx: Sender<Cmd>, state: Arc<Mutex<DevState>>, ctx: egui::Context) -> WinResult<Self> {
        unsafe {
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let client: IMMNotificationClient = Notifier { tx: Mutex::new(tx) }.into();
            en.RegisterEndpointNotificationCallback(&client)?;
            Ok(Self {
                en,
                client,
                captures: Vec::new(),
                want_id: None,
                device: None,
                vol: None,
                meter: None,
                state,
                ctx,
                refresh_at: Some(Instant::now()),
                revision: 0,
            })
        }
    }

    fn with_state(&self, f: impl FnOnce(&mut DevState)) {
        if let Ok(mut s) = self.state.lock() {
            f(&mut s);
        }
    }

    fn message(&self, m: Option<String>) {
        self.with_state(|s| s.message = m);
    }

    fn refresh(&mut self) {
        unsafe {
            let caps = match enum_flow(&self.en, eCapture) {
                Ok(c) => c,
                Err(e) => {
                    return self.message(Some(format!("Could not enumerate capture devices: {e}")))
                }
            };
            let rends: Vec<EndpointInfo> = enum_flow(&self.en, eRender)
                .map(|v| v.into_iter().map(|(i, _)| i).collect())
                .unwrap_or_default();
            self.captures = caps;

            // Keep the user's choice if it still exists; otherwise auto-pick only when nothing was ever chosen.
            let idx = match &self.want_id {
                Some(id) => self.captures.iter().position(|(i, _)| &i.id == id),
                None => {
                    let p = self
                        .captures
                        .iter()
                        .position(|(i, _)| i.name.to_lowercase().contains("at2020"))
                        .or_else(|| self.captures.iter().position(|(i, _)| i.is_default))
                        .or(if self.captures.is_empty() {
                            None
                        } else {
                            Some(0)
                        });
                    if let Some(p) = p {
                        self.want_id = Some(self.captures[p].0.id.clone());
                    }
                    p
                }
            };
            self.bind(idx);
            let infos: Vec<EndpointInfo> = self.captures.iter().map(|(i, _)| i.clone()).collect();
            self.revision += 1;
            let rev = self.revision;
            let msg = if idx.is_none() && self.want_id.is_some() {
                Some("Selected microphone is not connected.".to_string())
            } else {
                None
            };
            self.with_state(|s| {
                s.captures = infos;
                s.renders = rends;
                s.revision = rev;
                s.message = msg;
            });
        }
        self.poll(true);
        self.ctx.request_repaint();
    }

    unsafe fn bind(&mut self, idx: Option<usize>) {
        self.device = None;
        self.vol = None;
        self.meter = None;
        let Some(i) = idx else {
            return self.with_state(|s| {
                s.selected_id = None;
                s.selected_name = None;
                s.has_endpoint = false;
                s.sysfx_disabled = None;
                s.peak = 0.0;
            });
        };
        let (info, dev) = (&self.captures[i].0, self.captures[i].1.clone());
        self.vol = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok();
        self.meter = dev
            .Activate::<IAudioMeterInformation>(CLSCTX_ALL, None)
            .ok();
        let (mut mn, mut mx, mut inc) = (0.0f32, 0.0f32, 0.0f32);
        if let Some(v) = &self.vol {
            let _ = v.GetVolumeRange(&mut mn, &mut mx, &mut inc);
        }
        let sysfx = read_sysfx_disabled(&dev);
        let (id, name, ok) = (info.id.clone(), info.name.clone(), self.vol.is_some());
        self.device = Some(dev);
        self.with_state(|s| {
            s.selected_id = Some(id);
            s.selected_name = Some(name);
            s.has_endpoint = ok;
            s.db_min = mn;
            s.db_max = mx;
            s.sysfx_disabled = sysfx;
        });
    }

    fn handle(&mut self, c: Cmd) {
        unsafe {
            match c {
                Cmd::Refresh => self.refresh_at = Some(Instant::now() + Duration::from_millis(250)), // debounce bursts
                Cmd::Select(id) => {
                    self.want_id = Some(id);
                    self.refresh_at = Some(Instant::now());
                }
                Cmd::SetVolume(v) => {
                    if let Some(ep) = &self.vol {
                        if let Err(e) =
                            ep.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), core::ptr::null())
                        {
                            self.message(Some(format!("Set volume failed: {e}")));
                        }
                    }
                }
                Cmd::SetMute(m) => {
                    if let Some(ep) = &self.vol {
                        if let Err(e) = ep.SetMute(BOOL::from(m), core::ptr::null()) {
                            self.message(Some(format!("Set mute failed: {e}")));
                        }
                    }
                }
                Cmd::SetSysFxDisabled(d) => {
                    if let Some(dev) = &self.device {
                        match write_sysfx_disabled(dev, d) {
                            Ok(()) => {
                                let now = read_sysfx_disabled(dev);
                                self.with_state(|s| s.sysfx_disabled = now);
                                self.message(None);
                            }
                            Err(e) => self.message(Some(format!("Changing audio enhancements needs administrator rights (or is blocked by the driver): {e}"))),
                        }
                    }
                }
                Cmd::Shutdown => {}
            }
        }
    }

    fn poll(&mut self, slow: bool) {
        unsafe {
            let peak = self
                .meter
                .as_ref()
                .and_then(|m| m.GetPeakValue().ok())
                .unwrap_or(0.0);
            let vs = if slow {
                self.vol.as_ref().map(|v| {
                    (
                        v.GetMasterVolumeLevelScalar().unwrap_or(0.0),
                        v.GetMasterVolumeLevel().unwrap_or(0.0),
                        v.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                    )
                })
            } else {
                None
            };
            self.with_state(|s| {
                s.peak = peak;
                if let Some((v, db, m)) = vs {
                    s.volume = v;
                    s.volume_db = db;
                    s.mute = m;
                }
            });
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        unsafe {
            let _ = self.en.UnregisterEndpointNotificationCallback(&self.client);
        }
    }
}

fn run(rx: Receiver<Cmd>, tx: Sender<Cmd>, state: Arc<Mutex<DevState>>, ctx: egui::Context) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    match Worker::new(tx, state.clone(), ctx) {
        Ok(mut w) => {
            let mut last_slow = Instant::now();
            loop {
                match rx.recv_timeout(Duration::from_millis(40)) {
                    Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                    Ok(c) => w.handle(c),
                    Err(RecvTimeoutError::Timeout) => {}
                }
                if w.refresh_at.is_some_and(|t| Instant::now() >= t) {
                    w.refresh_at = None;
                    w.refresh();
                }
                let slow = last_slow.elapsed() >= Duration::from_millis(200);
                if slow {
                    last_slow = Instant::now();
                }
                w.poll(slow);
            }
            drop(w); // release COM objects before CoUninitialize
        }
        Err(e) => {
            if let Ok(mut s) = state.lock() {
                s.message = Some(format!("Core Audio initialisation failed: {e}"));
            }
        }
    }
    unsafe { CoUninitialize() };
}
