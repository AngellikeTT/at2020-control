//! WASAPI exclusive-mode streams (event-driven), which cpal's shared-mode backend
//! cannot provide. Each stream runs on its own thread that owns every COM object, and
//! hands/receives interleaved `f32` frames through a closure.
//!
//! Formats are never assumed: candidates are tried with `IsFormatSupported` and the
//! first the endpoint accepts is used (16/24/32-bit integer or 32-bit float PCM).

use anyhow::{anyhow, bail, Result};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::{GUID, HRESULT};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eCapture, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_SHAREMODE_EXCLUSIVE,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{
    AvSetMmThreadCharacteristicsW, CreateEventW, WaitForSingleObject,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Enc {
    I16,
    I24in32,
    I24Packed,
    I32,
    F32,
}

impl Enc {
    fn container_bits(self) -> u16 {
        match self {
            Enc::I16 => 16,
            Enc::I24in32 | Enc::I32 | Enc::F32 => 32,
            Enc::I24Packed => 24,
        }
    }
    fn valid_bits(self) -> u16 {
        match self {
            Enc::I16 => 16,
            Enc::I24in32 | Enc::I24Packed => 24,
            Enc::I32 | Enc::F32 => 32,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Enc::I16 => "PCM16",
            Enc::I24in32 => "PCM24(32)",
            Enc::I24Packed => "PCM24",
            Enc::I32 => "PCM32",
            Enc::F32 => "Float32",
        }
    }
}

/// Negotiated stream format, reported back to the engine.
#[derive(Clone, Debug)]
pub struct Negotiated {
    pub rate: u32,
    pub channels: u16,
    pub format: String,
    pub buffer_frames: u32,
}

/// Handle to a running exclusive stream; dropping it stops the stream.
pub struct ExclusiveStream {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for ExclusiveStream {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub type Fault = Arc<dyn Fn(String) + Send + Sync>;

/// Opens an exclusive capture stream. `make` runs on the stream thread once the format
/// is known and returns the per-buffer callback (interleaved f32 frames).
pub fn open_capture<F>(
    name: &str,
    want_rate: Option<u32>,
    make: impl FnOnce(&Negotiated) -> F + Send + 'static,
    fault: Fault,
) -> Result<(ExclusiveStream, Negotiated)>
where
    F: FnMut(&[f32]) + Send + 'static,
{
    spawn(name.to_string(), true, want_rate, make, fault)
}

/// Opens an exclusive render stream. The callback fills interleaved f32 frames.
pub fn open_render<F>(
    name: &str,
    want_rate: Option<u32>,
    make: impl FnOnce(&Negotiated) -> F + Send + 'static,
    fault: Fault,
) -> Result<(ExclusiveStream, Negotiated)>
where
    F: FnMut(&mut [f32]) + Send + 'static,
{
    spawn_render(name.to_string(), want_rate, make, fault)
}

fn spawn<F>(
    name: String,
    _capture: bool,
    want_rate: Option<u32>,
    make: impl FnOnce(&Negotiated) -> F + Send + 'static,
    fault: Fault,
) -> Result<(ExclusiveStream, Negotiated)>
where
    F: FnMut(&[f32]) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<std::result::Result<Negotiated, String>>();
    let st = stop.clone();
    let join = std::thread::Builder::new()
        .name("wasapi-excl-capture".into())
        .spawn(move || {
            // SAFETY: all COM use is confined to this thread; `ComScope` undoes the
            // initialisation after every COM object (declared later) has been released.
            unsafe {
                let _com = ComScope::enter();
                promote();
                if let Err(e) = capture_main(&name, want_rate, make, &st, &tx, &fault) {
                    let _ = tx.send(Err(format!("{e:#}")));
                    fault(format!("Exclusive capture stopped: {e:#}"));
                }
            }
        })?;
    finish_open(stop, join, rx)
}

fn spawn_render<F>(
    name: String,
    want_rate: Option<u32>,
    make: impl FnOnce(&Negotiated) -> F + Send + 'static,
    fault: Fault,
) -> Result<(ExclusiveStream, Negotiated)>
where
    F: FnMut(&mut [f32]) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<std::result::Result<Negotiated, String>>();
    let st = stop.clone();
    let join = std::thread::Builder::new()
        .name("wasapi-excl-render".into())
        .spawn(move || {
            // SAFETY: as above.
            unsafe {
                let _com = ComScope::enter();
                promote();
                if let Err(e) = render_main(&name, want_rate, make, &st, &tx, &fault) {
                    let _ = tx.send(Err(format!("{e:#}")));
                    fault(format!("Exclusive render stopped: {e:#}"));
                }
            }
        })?;
    finish_open(stop, join, rx)
}

fn finish_open(
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
    rx: mpsc::Receiver<std::result::Result<Negotiated, String>>,
) -> Result<(ExclusiveStream, Negotiated)> {
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(n)) => Ok((
            ExclusiveStream {
                stop,
                join: Some(join),
            },
            n,
        )),
        Ok(Err(e)) => {
            stop.store(true, Relaxed);
            let _ = join.join();
            Err(anyhow!(e))
        }
        Err(_) => {
            stop.store(true, Relaxed);
            let _ = join.join();
            Err(anyhow!("timed out opening the exclusive stream"))
        }
    }
}

/// COM initialisation for the stream thread, undone on drop (also while unwinding).
struct ComScope;

impl ComScope {
    unsafe fn enter() -> Self {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        ComScope
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        // SAFETY: paired with CoInitializeEx in `enter` on this same thread.
        unsafe { CoUninitialize() }
    }
}

/// A Win32 event handle, closed on drop.
struct Event(HANDLE);

impl Event {
    unsafe fn new() -> Result<Self> {
        Ok(Event(CreateEventW(None, false, false, None)?))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateEventW and is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

unsafe fn promote() {
    let mut idx = 0u32;
    let _ = AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut idx);
}

// -------------------------------------------------------------- endpoint ---

unsafe fn find_endpoint(en: &IMMDeviceEnumerator, name: &str, capture: bool) -> Result<IMMDevice> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    let col = en.EnumAudioEndpoints(
        if capture { eCapture } else { eRender },
        DEVICE_STATE_ACTIVE,
    )?;
    for i in 0..col.GetCount()? {
        let d = col.Item(i)?;
        let store = d.OpenPropertyStore(STGM_READ)?;
        let pv = store.GetValue(&PKEY_Device_FriendlyName)?;
        let p = PropVariantToStringAlloc(&pv)?;
        let n = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        if n == name {
            return Ok(d);
        }
    }
    bail!("endpoint \"{name}\" not found")
}

fn make_wfx(rate: u32, ch: u16, enc: Enc) -> WAVEFORMATEXTENSIBLE {
    let cb = enc.container_bits();
    let block = ch * (cb / 8);
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: ch,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * block as u32,
            nBlockAlign: block,
            wBitsPerSample: cb,
            cbSize: 22,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: enc.valid_bits(),
        },
        dwChannelMask: match ch {
            1 => 0x4,
            2 => 0x3,
            _ => 0,
        },
        SubFormat: if enc == Enc::F32 {
            KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            KSDATAFORMAT_SUBTYPE_PCM
        },
    }
}

struct Opened {
    client: IAudioClient,
    enc: Enc,
    rate: u32,
    channels: u16,
    frames: u32,
}

/// Finds a format the endpoint accepts in exclusive mode and initialises the client.
unsafe fn open_client(dev: &IMMDevice, want_rate: Option<u32>) -> Result<Opened> {
    let probe: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let mix = probe.GetMixFormat()?;
    let (mix_rate, mix_ch) = ((*mix).nSamplesPerSec, (*mix).nChannels);
    CoTaskMemFree(Some(mix as *const core::ffi::c_void));

    let mut rates: Vec<u32> = Vec::new();
    for r in [
        want_rate,
        Some(mix_rate),
        Some(48_000),
        Some(44_100),
        Some(96_000),
    ]
    .into_iter()
    .flatten()
    {
        if !rates.contains(&r) {
            rates.push(r);
        }
    }
    let mut chans = vec![mix_ch];
    for c in [2u16, 1] {
        if !chans.contains(&c) {
            chans.push(c);
        }
    }
    let encs = [Enc::I24in32, Enc::I16, Enc::I32, Enc::F32, Enc::I24Packed];
    let mut chosen = None;
    'search: for &r in &rates {
        for &c in &chans {
            for &e in &encs {
                let w = make_wfx(r, c, e);
                let hr: HRESULT = probe.IsFormatSupported(
                    AUDCLNT_SHAREMODE_EXCLUSIVE,
                    &w.Format as *const WAVEFORMATEX,
                    None,
                );
                if hr.is_ok() && hr == HRESULT(0) {
                    chosen = Some((r, c, e));
                    break 'search;
                }
            }
        }
    }
    let Some((rate, channels, enc)) = chosen else {
        bail!("the device accepts none of the tried exclusive-mode formats (it may be in use by another app)");
    };

    let mut def = 0i64;
    let mut min = 0i64;
    probe.GetDevicePeriod(Some(&mut def), Some(&mut min))?;
    let mut period = min.max(30_000); // never below 3 ms
    let wfx = make_wfx(rate, channels, enc);
    let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
    let mut client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let mut r = client.Initialize(
        AUDCLNT_SHAREMODE_EXCLUSIVE,
        flags,
        period,
        period,
        &wfx.Format as *const WAVEFORMATEX,
        None,
    );
    if let Err(e) = &r {
        if e.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
            // The driver wants a frame-aligned buffer: derive the period from it and retry.
            let frames = client.GetBufferSize()?;
            period = (10_000_000.0 * frames as f64 / rate as f64 + 0.5) as i64;
            client = dev.Activate(CLSCTX_ALL, None)?;
            r = client.Initialize(
                AUDCLNT_SHAREMODE_EXCLUSIVE,
                flags,
                period,
                period,
                &wfx.Format as *const WAVEFORMATEX,
                None,
            );
        }
    }
    r.map_err(|e| anyhow!("exclusive Initialize failed: {e}"))?;
    let frames = client.GetBufferSize()?;
    Ok(Opened {
        client,
        enc,
        rate,
        channels,
        frames,
    })
}

// ------------------------------------------------------------ conversion ---

fn bytes_to_f32(enc: Enc, src: &[u8], dst: &mut [f32]) {
    match enc {
        Enc::I16 => {
            for (d, b) in dst.iter_mut().zip(src.chunks_exact(2)) {
                *d = i16::from_le_bytes([b[0], b[1]]) as f32 * (1.0 / 32_768.0);
            }
        }
        Enc::I24in32 | Enc::I32 => {
            for (d, b) in dst.iter_mut().zip(src.chunks_exact(4)) {
                *d = i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 * (1.0 / 2_147_483_648.0);
            }
        }
        Enc::I24Packed => {
            for (d, b) in dst.iter_mut().zip(src.chunks_exact(3)) {
                let v = i32::from_le_bytes([0, b[0], b[1], b[2]]);
                *d = v as f32 * (1.0 / 2_147_483_648.0);
            }
        }
        Enc::F32 => {
            for (d, b) in dst.iter_mut().zip(src.chunks_exact(4)) {
                *d = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
        }
    }
}

fn f32_to_bytes(enc: Enc, src: &[f32], dst: &mut [u8]) {
    match enc {
        Enc::I16 => {
            for (s, b) in src.iter().zip(dst.chunks_exact_mut(2)) {
                let v = (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
                b.copy_from_slice(&v.to_le_bytes());
            }
        }
        Enc::I24in32 | Enc::I32 => {
            for (s, b) in src.iter().zip(dst.chunks_exact_mut(4)) {
                let v = (s.clamp(-1.0, 1.0) as f64 * 2_147_483_647.0).round() as i32;
                // 24-in-32 formats keep the significant bits in the top three bytes.
                b.copy_from_slice(&v.to_le_bytes());
            }
        }
        Enc::I24Packed => {
            for (s, b) in src.iter().zip(dst.chunks_exact_mut(3)) {
                let v = (s.clamp(-1.0, 1.0) as f64 * 8_388_607.0).round() as i32;
                let le = v.to_le_bytes();
                b.copy_from_slice(&le[..3]);
            }
        }
        Enc::F32 => {
            for (s, b) in src.iter().zip(dst.chunks_exact_mut(4)) {
                b.copy_from_slice(&s.to_le_bytes());
            }
        }
    }
}

// ------------------------------------------------------------- the loops ---

unsafe fn capture_main<F>(
    name: &str,
    want_rate: Option<u32>,
    make: impl FnOnce(&Negotiated) -> F,
    stop: &AtomicBool,
    tx: &mpsc::Sender<std::result::Result<Negotiated, String>>,
    fault: &Fault,
) -> Result<()>
where
    F: FnMut(&[f32]),
{
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = find_endpoint(&en, name, true)?;
    let o = open_client(&dev, want_rate)?;
    let neg = Negotiated {
        rate: o.rate,
        channels: o.channels,
        format: o.enc.name().into(),
        buffer_frames: o.frames,
    };
    let mut cb = make(&neg);
    let ev = Event::new()?;
    o.client.SetEventHandle(ev.0)?;
    let cap: IAudioCaptureClient = o.client.GetService()?;
    o.client.Start()?;
    let _ = tx.send(Ok(neg));

    let ch = o.channels as usize;
    let mut scratch = vec![0.0f32; (o.frames as usize * ch).max(1) * 4];
    // Stop() runs exactly once whatever ends the loop (stop request, device error, a
    // panic in the processing callback); the client, the capture service and the event
    // are then released by their destructors, in reverse order of creation.
    let client = &o.client;
    crate::guard::supervise_stream(
        || {
            let _ = client.Stop();
        },
        || {
            if stop.load(Relaxed) {
                return Ok(false);
            }
            if WaitForSingleObject(ev.0, 250) != WAIT_OBJECT_0 {
                return Ok(true);
            }
            loop {
                let n = cap.GetNextPacketSize()?;
                if n == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let (mut frames, mut flags) = (0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let samples = frames as usize * ch;
                if scratch.len() < samples {
                    scratch.resize(samples, 0.0);
                }
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    scratch[..samples].fill(0.0);
                } else {
                    let bytes = std::slice::from_raw_parts(
                        data,
                        samples * (o.enc.container_bits() as usize / 8),
                    );
                    bytes_to_f32(o.enc, bytes, &mut scratch[..samples]);
                }
                // Release the packet even if the callback panics, so the client is
                // left in a consistent state for Stop().
                let _release = crate::guard::OnDrop::new(|| {
                    let _ = cap.ReleaseBuffer(frames);
                });
                cb(&scratch[..samples]);
            }
            Ok(true)
        },
        &|m| fault(format!("Exclusive capture error: {m}")),
    );
    Ok(())
}

unsafe fn render_main<F>(
    name: &str,
    want_rate: Option<u32>,
    make: impl FnOnce(&Negotiated) -> F,
    stop: &AtomicBool,
    tx: &mpsc::Sender<std::result::Result<Negotiated, String>>,
    fault: &Fault,
) -> Result<()>
where
    F: FnMut(&mut [f32]),
{
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = find_endpoint(&en, name, false)?;
    let o = open_client(&dev, want_rate)?;
    let neg = Negotiated {
        rate: o.rate,
        channels: o.channels,
        format: o.enc.name().into(),
        buffer_frames: o.frames,
    };
    let mut cb = make(&neg);
    let ev = Event::new()?;
    o.client.SetEventHandle(ev.0)?;
    let rend: IAudioRenderClient = o.client.GetService()?;
    // Start with a buffer of silence so the first event has something valid to play.
    rend.GetBuffer(o.frames)?;
    rend.ReleaseBuffer(o.frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
    o.client.Start()?;
    let _ = tx.send(Ok(neg));

    let ch = o.channels as usize;
    let bytes_per = o.enc.container_bits() as usize / 8;
    let mut scratch = vec![0.0f32; o.frames as usize * ch];
    let client = &o.client;
    crate::guard::supervise_stream(
        || {
            let _ = client.Stop();
        },
        || {
            if stop.load(Relaxed) {
                return Ok(false);
            }
            if WaitForSingleObject(ev.0, 250) != WAIT_OBJECT_0 {
                return Ok(true);
            }
            // Fill first, then hand the device a complete buffer: a panic in the
            // callback leaves no device buffer half written or unreleased.
            cb(&mut scratch);
            let p = rend.GetBuffer(o.frames)?;
            let bytes = std::slice::from_raw_parts_mut(p, scratch.len() * bytes_per);
            f32_to_bytes(o.enc, &scratch, bytes);
            rend.ReleaseBuffer(o.frames, 0)?;
            Ok(true)
        },
        &|m| fault(format!("Exclusive render error: {m}")),
    );
    Ok(())
}

/// Used by the tests: is `g` the IEEE-float sub-format GUID?
#[allow(dead_code)]
fn is_float(g: &GUID) -> bool {
    *g == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_conversions_round_trip() {
        let src = [0.0f32, 0.5, -0.5, 0.999, -1.0];
        for enc in [Enc::I16, Enc::I24in32, Enc::I24Packed, Enc::I32, Enc::F32] {
            let mut bytes = vec![0u8; src.len() * (enc.container_bits() as usize / 8)];
            f32_to_bytes(enc, &src, &mut bytes);
            let mut back = vec![0.0f32; src.len()];
            bytes_to_f32(enc, &bytes, &mut back);
            let tol = match enc {
                Enc::I16 => 1.0 / 16_000.0,
                _ => 1e-5,
            };
            for (a, b) in src.iter().zip(&back) {
                assert!((a - b).abs() <= tol, "{enc:?}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn wave_format_fields_are_consistent() {
        let w = make_wfx(48_000, 2, Enc::I24in32);
        let f = w.Format;
        assert_eq!({ f.nBlockAlign }, 8);
        assert_eq!({ f.nAvgBytesPerSec }, 48_000 * 8);
        assert_eq!({ f.cbSize }, 22);
        let v = unsafe { w.Samples.wValidBitsPerSample };
        assert_eq!(v, 24);
    }
}
