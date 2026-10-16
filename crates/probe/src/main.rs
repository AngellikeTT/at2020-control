//! `mic-probe [name-substring] [--secs N] [--raw]`
//!
//! Standalone diagnostic, independent of the app's engine: prints every capture endpoint
//! (state, default roles), the chosen endpoint's volume / mute / per-channel levels /
//! enhancement flag / mix format, then captures N seconds (default 5) in WASAPI *shared*
//! mode and reports peak, RMS, exact-zero count and how many packets carried
//! AUDCLNT_BUFFERFLAGS_SILENT. `--raw` requests AUDCLNT_STREAMOPTIONS_RAW (bypasses APOs).
//! Exit code: 0 = nonzero signal captured, 1 = digital silence, 2 = error.

use std::time::{Duration, Instant};
use windows::core::{Interface, GUID};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, AudioCategory_Other, AudioClientProperties,
    IAudioCaptureClient, IAudioClient, IAudioClient2, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMOPTIONS_RAW, DEVICE_STATE, DEVICE_STATEMASK_ALL,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;

const KEY_DISABLE_SYSFX: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xE4870E26_3CC5_4CD2_BA46_CA0A9A70ED04),
    pid: 5,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut want = "at2020".to_string();
    let mut secs = 5.0f64;
    let mut raw = false;
    let mut roundtrip = false;
    let mut hold: Option<f64> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--secs" => {
                i += 1;
                secs = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(5.0);
            }
            "--raw" => raw = true,
            "--roundtrip" => roundtrip = true,
            "--render-sessions" => {
                // Who is playing into render endpoints whose name matches `want`.
                // SAFETY: COM calls on this thread after CoInitializeEx.
                unsafe { render_sessions(&want) };
                std::process::exit(0);
            }
            "--hold-exclusive" => {
                i += 1;
                hold = args.get(i).and_then(|s| s.parse().ok());
            }
            s => want = s.to_lowercase(),
        }
        i += 1;
    }
    if let Some(h) = hold {
        // SAFETY: as below.
        let code = match unsafe { hold_exclusive(&want, h) } {
            Ok(()) => 0,
            Err(e) => {
                println!("ERROR: {e}");
                2
            }
        };
        std::process::exit(code);
    }
    // SAFETY: plain COM calls on the main thread after CoInitializeEx.
    let code = match unsafe { run(&want, secs, raw, roundtrip) } {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            println!("ERROR: {e}");
            2
        }
    };
    std::process::exit(code);
}

unsafe fn text(p: windows::core::PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
    s
}

unsafe fn name_of(d: &IMMDevice) -> String {
    d.OpenPropertyStore(STGM_READ)
        .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
        .and_then(|pv| PropVariantToStringAlloc(&pv))
        .map(|p| text(p))
        .unwrap_or_else(|_| "?".into())
}

unsafe fn find_active(en: &IMMDeviceEnumerator, want: &str) -> windows::core::Result<IMMDevice> {
    let col = en.EnumAudioEndpoints(eCapture, DEVICE_STATE(1))?;
    for i in 0..col.GetCount()? {
        let d = col.Item(i)?;
        if name_of(&d).to_lowercase().contains(want) {
            return Ok(d);
        }
    }
    Err(windows::core::Error::from_hresult(windows::core::HRESULT(
        0x8889_0004u32 as i32,
    )))
}

/// Opens the endpoint in WASAPI *exclusive* mode (timer-driven) and keeps the stream
/// running for `secs`, draining packets: simulates a process that holds the mic exclusively.
unsafe fn hold_exclusive(want: &str, secs: f64) -> windows::core::Result<()> {
    use windows::Win32::Media::Audio::{AUDCLNT_SHAREMODE_EXCLUSIVE, WAVEFORMATEXTENSIBLE_0};
    use windows::Win32::Media::KernelStreaming::KSDATAFORMAT_SUBTYPE_PCM;
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = find_active(&en, want)?;
    let mut chosen = None;
    'f: for (ch, bits, valid) in [(1u16, 16u16, 16u16), (1, 32, 24), (2, 16, 16), (2, 32, 24)] {
        let block = ch * bits / 8;
        let w = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                nChannels: ch,
                nSamplesPerSec: 48_000,
                nAvgBytesPerSec: 48_000 * block as u32,
                nBlockAlign: block,
                wBitsPerSample: bits,
                cbSize: 22,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: valid,
            },
            dwChannelMask: if ch == 1 { 4 } else { 3 },
            SubFormat: KSDATAFORMAT_SUBTYPE_PCM,
        };
        let c: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let mut def = 0i64;
        c.GetDevicePeriod(Some(&mut def), None)?;
        if c.Initialize(AUDCLNT_SHAREMODE_EXCLUSIVE, 0, def, def, &w.Format, None)
            .is_ok()
        {
            println!("exclusive: holding {ch} ch {valid}-bit 48 kHz, period {def} hns");
            chosen = Some(c);
            break 'f;
        }
    }
    let Some(c) = chosen else {
        println!("exclusive: no format accepted (device in use?)");
        return Ok(());
    };
    let cap: IAudioCaptureClient = c.GetService()?;
    c.Start()?;
    let t0 = Instant::now();
    let mut frames_total = 0u64;
    while t0.elapsed().as_secs_f64() < secs {
        std::thread::sleep(Duration::from_millis(3));
        while cap.GetNextPacketSize()? > 0 {
            let mut data: *mut u8 = std::ptr::null_mut();
            let (mut frames, mut flags) = (0u32, 0u32);
            cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            frames_total += frames as u64;
            cap.ReleaseBuffer(frames)?;
        }
    }
    c.Stop()?;
    println!("exclusive: released after {secs} s ({frames_total} frames drained)");
    Ok(())
}

unsafe fn render_sessions(want: &str) {
    use windows::Win32::Media::Audio::eRender;
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
    else {
        return;
    };
    let Ok(col) = en.EnumAudioEndpoints(eRender, DEVICE_STATE(1)) else {
        return;
    };
    for i in 0..col.GetCount().unwrap_or(0) {
        let Ok(d) = col.Item(i) else { continue };
        let n = name_of(&d);
        if n.to_lowercase().contains(want) {
            println!("render endpoint: {n}");
            list_sessions(&d);
        }
    }
}

/// Walks the endpoint's KS device topology (the driver's view of the hardware) and prints
/// every part with a mute, volume or automatic-gain-control (boost/AGC) interface.
unsafe fn walk_topology(dev: &IMMDevice) {
    use windows::Win32::Media::Audio::{
        IAudioAutoGainControl, IAudioMute, IAudioVolumeLevel, IDeviceTopology, IPart,
    };
    let Ok(topo) = dev.Activate::<IDeviceTopology>(CLSCTX_ALL, None) else {
        println!("topology: unavailable");
        return;
    };
    let Ok(conn) = topo.GetConnector(0) else {
        return;
    };
    let Ok(other) = conn.GetConnectedTo() else {
        println!("topology: connector not connected");
        return;
    };
    let Ok(start) = other.cast::<IPart>() else {
        return;
    };
    println!("hardware topology (driver view):");
    let mut stack = vec![start];
    let mut seen = std::collections::HashSet::new();
    while let Some(part) = stack.pop() {
        let gid = part.GetGlobalId().map(|p| text(p)).unwrap_or_default();
        if !gid.is_empty() && !seen.insert(gid.clone()) {
            continue;
        }
        if seen.len() > 64 {
            break;
        }
        let name = part.GetName().map(|p| text(p)).unwrap_or_default();
        let kind = part.GetPartType().map(|t| t.0).unwrap_or(-1);
        let sub = part
            .GetSubType()
            .map(|g| format!("{g:?}"))
            .unwrap_or_default();
        println!(
            "  part \"{name}\" ({}) {sub}",
            if kind == 0 { "connector" } else { "subunit" }
        );
        let act = |iid: &GUID| -> Option<*mut core::ffi::c_void> {
            let mut p = std::ptr::null_mut();
            part.Activate(CLSCTX_ALL.0, iid, Some(&mut p)).ok()?;
            (!p.is_null()).then_some(p)
        };
        if let Some(p) = act(&IAudioMute::IID) {
            let m = IAudioMute::from_raw(p);
            println!(
                "  [{name}] hardware mute = {:?}",
                m.GetMute().map(|b| b.as_bool())
            );
        }
        if let Some(p) = act(&IAudioVolumeLevel::IID) {
            let v = IAudioVolumeLevel::from_raw(p);
            let n = v.GetChannelCount().unwrap_or(0);
            for c in 0..n {
                let (mut lo, mut hi, mut st) = (0f32, 0f32, 0f32);
                let _ = v.GetLevelRange(c, &mut lo, &mut hi, &mut st);
                println!(
                    "  [{name}] hardware level ch{c} = {:?} dB (range {lo}..{hi})",
                    v.GetLevel(c)
                );
            }
        }
        if let Some(p) = act(&IAudioAutoGainControl::IID) {
            let a = IAudioAutoGainControl::from_raw(p);
            println!(
                "  [{name}] AGC/boost enabled = {:?}",
                a.GetEnabled().map(|b| b.as_bool())
            );
        }
        for parts in [part.EnumPartsIncoming(), part.EnumPartsOutgoing()]
            .into_iter()
            .flatten()
        {
            for i in 0..parts.GetCount().unwrap_or(0) {
                if let Ok(p) = parts.GetPart(i) {
                    stack.push(p);
                }
            }
        }
    }
}

/// Prints every audio session on the endpoint (owning PID, state), i.e. who has it open.
unsafe fn list_sessions(dev: &IMMDevice) {
    use windows::Win32::Media::Audio::{IAudioSessionControl2, IAudioSessionManager2};
    let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
        println!("sessions: IAudioSessionManager2 unavailable");
        return;
    };
    let Ok(en) = mgr.GetSessionEnumerator() else {
        return;
    };
    let n = en.GetCount().unwrap_or(0);
    println!("sessions on this endpoint: {n}");
    for i in 0..n {
        let Ok(c) = en.GetSession(i) else { continue };
        let Ok(c2) = c.cast::<IAudioSessionControl2>() else {
            continue;
        };
        let pid = c2.GetProcessId().unwrap_or(0);
        let st = match c.GetState().map(|s| s.0).unwrap_or(-1) {
            0 => "inactive",
            1 => "ACTIVE",
            2 => "expired",
            _ => "?",
        };
        let exe = std::process::Command::new("cmd")
            .args(["/C", &format!("tasklist /FI \"PID eq {pid}\" /FO CSV /NH")])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let exe = exe.split(',').next().unwrap_or("").trim_matches('"');
        println!("  pid {pid:>6} {st:<8} {exe}");
    }
}

/// Replays the self-test's volume/mute round trip (25/50/75 %, mute toggled twice, volume
/// restored) on the endpoint, reading every value back.
unsafe fn roundtrip_volume(vol: &IAudioEndpointVolume) -> windows::core::Result<()> {
    let (v0, m0) = (vol.GetMasterVolumeLevelScalar()?, vol.GetMute()?.as_bool());
    for t in [0.25f32, 0.5, 0.75] {
        vol.SetMasterVolumeLevelScalar(t, std::ptr::null())?;
        std::thread::sleep(Duration::from_millis(250));
        println!(
            "  roundtrip: set {t:.2} -> read {:.3}",
            vol.GetMasterVolumeLevelScalar()?
        );
    }
    for m in [!m0, m0] {
        vol.SetMute(m, std::ptr::null())?;
        std::thread::sleep(Duration::from_millis(250));
        println!("  roundtrip: mute {m} -> read {}", vol.GetMute()?.as_bool());
    }
    vol.SetMasterVolumeLevelScalar(v0, std::ptr::null())?;
    std::thread::sleep(Duration::from_millis(250));
    println!(
        "  roundtrip: restored volume {:.3}, mute {}",
        vol.GetMasterVolumeLevelScalar()?,
        vol.GetMute()?.as_bool()
    );
    Ok(())
}

unsafe fn run(want: &str, secs: f64, raw: bool, roundtrip: bool) -> windows::core::Result<bool> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let def = |role| {
        en.GetDefaultAudioEndpoint(eCapture, role)
            .ok()
            .and_then(|d| d.GetId().ok().map(|p| text(p)))
            .unwrap_or_default()
    };
    let (d_con, d_mm, d_com) = (def(eConsole), def(eMultimedia), def(eCommunications));
    println!("probe at {:?}", std::time::SystemTime::now());
    println!("capture endpoints (all states):");
    let col = en.EnumAudioEndpoints(eCapture, DEVICE_STATE(DEVICE_STATEMASK_ALL))?;
    let mut chosen: Option<IMMDevice> = None;
    for i in 0..col.GetCount()? {
        let d = col.Item(i)?;
        let id = d.GetId().map(|p| text(p)).unwrap_or_default();
        let n = name_of(&d);
        let st = d.GetState().map(|s| s.0).unwrap_or(0);
        let state = match st {
            1 => "ACTIVE",
            2 => "DISABLED",
            4 => "NOTPRESENT",
            8 => "UNPLUGGED",
            _ => "?",
        };
        let mut roles = String::new();
        if id == d_con {
            roles += " default-console";
        }
        if id == d_mm {
            roles += " default-multimedia";
        }
        if id == d_com {
            roles += " default-communications";
        }
        println!("  [{state:<10}] {n}  {id}{roles}");
        if st == 1 && chosen.is_none() && n.to_lowercase().contains(want) {
            chosen = Some(d);
        }
    }
    let Some(dev) = chosen else {
        println!("no ACTIVE capture endpoint matching \"{want}\"");
        return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x8889_0004u32 as i32,
        )));
    };
    println!("chosen: {}", name_of(&dev));

    let vol: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None)?;
    let (mut mn, mut mx, mut inc) = (0f32, 0f32, 0f32);
    vol.GetVolumeRange(&mut mn, &mut mx, &mut inc)?;
    println!(
        "endpoint volume: scalar {:.3}, {:.2} dB (range {mn:.1}..{mx:.1} step {inc:.3}), mute {}",
        vol.GetMasterVolumeLevelScalar()?,
        vol.GetMasterVolumeLevel()?,
        vol.GetMute()?.as_bool()
    );
    list_sessions(&dev);
    walk_topology(&dev);
    if roundtrip {
        roundtrip_volume(&vol)?;
    }
    let nch = vol.GetChannelCount()?;
    for c in 0..nch {
        println!(
            "  channel {c}: scalar {:.3}, {:.2} dB",
            vol.GetChannelVolumeLevelScalar(c)?,
            vol.GetChannelVolumeLevel(c)?
        );
    }
    let sysfx = dev
        .OpenPropertyStore(STGM_READ)
        .and_then(|s| s.GetValue(&KEY_DISABLE_SYSFX))
        .ok()
        .and_then(|pv| u32::try_from(&pv).ok());
    println!("Disable_SysFx: {sysfx:?} (1 = enhancements off, None = not set)");

    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    if raw {
        let c2: IAudioClient2 = client.cast()?;
        let props = AudioClientProperties {
            cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
            bIsOffload: false.into(),
            eCategory: AudioCategory_Other,
            Options: AUDCLNT_STREAMOPTIONS_RAW,
        };
        match c2.SetClientProperties(&props) {
            Ok(()) => println!("RAW stream option set (APOs bypassed)"),
            Err(e) => println!("RAW stream option rejected: {e}"),
        }
    }
    let mix = client.GetMixFormat()?;
    let f: WAVEFORMATEX = *mix;
    let float = if f.wFormatTag == WAVE_FORMAT_EXTENSIBLE as u16 {
        let x = *(mix as *const WAVEFORMATEXTENSIBLE);
        let sub = x.SubFormat;
        sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    } else {
        f.wFormatTag == WAVE_FORMAT_IEEE_FLOAT as u16
    };
    let (rate, ch, bits) = (f.nSamplesPerSec, f.nChannels as usize, f.wBitsPerSample);
    println!(
        "shared mix format: {rate} Hz, {ch} ch, {bits} bit {}",
        if float { "float" } else { "int" }
    );
    client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, mix, None)?;
    CoTaskMemFree(Some(mix as *const core::ffi::c_void));
    let cap: IAudioCaptureClient = client.GetService()?;
    let meter: Option<IAudioMeterInformation> = dev.Activate(CLSCTX_ALL, None).ok();
    client.Start()?;

    let (mut peak, mut sumsq, mut n, mut zeros) = (0f64, 0f64, 0u64, 0u64);
    let (mut packets, mut silent, mut discont) = (0u64, 0u64, 0u64);
    let mut meter_peak = 0f32;
    let mut first_nonzero_ms: Option<u128> = None;
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        std::thread::sleep(Duration::from_millis(10));
        if let Some(m) = &meter {
            meter_peak = meter_peak.max(m.GetPeakValue().unwrap_or(0.0));
        }
        loop {
            if cap.GetNextPacketSize()? == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let (mut frames, mut flags) = (0u32, 0u32);
            cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            packets += 1;
            if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                discont += 1;
            }
            let samples = frames as usize * ch;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                silent += 1;
                zeros += samples as u64;
                n += samples as u64;
            } else {
                for k in 0..samples {
                    let v: f64 = match (float, bits) {
                        (true, 32) => *(data as *const f32).add(k) as f64,
                        (false, 16) => *(data as *const i16).add(k) as f64 / 32768.0,
                        (false, 32) => *(data as *const i32).add(k) as f64 / 2147483648.0,
                        _ => 0.0,
                    };
                    if v == 0.0 {
                        zeros += 1;
                    } else if first_nonzero_ms.is_none() {
                        first_nonzero_ms = Some(t0.elapsed().as_millis());
                    }
                    peak = peak.max(v.abs());
                    sumsq += v * v;
                    n += 1;
                }
            }
            cap.ReleaseBuffer(frames)?;
        }
    }
    client.Stop()?;
    let rms = if n > 0 {
        (sumsq / n as f64).sqrt()
    } else {
        0.0
    };
    let db = |x: f64| {
        if x > 0.0 {
            format!("{:.1} dBFS", 20.0 * x.log10())
        } else {
            "-inf dBFS".into()
        }
    };
    println!(
        "captured {:.1} s: {n} samples, {packets} packets, {silent} with AUDCLNT_BUFFERFLAGS_SILENT, {discont} discontinuities",
        t0.elapsed().as_secs_f64()
    );
    println!(
        "peak {peak:.6} ({}), RMS {rms:.6} ({}), exact zeros {zeros}/{n} ({:.1} %), first nonzero at {first_nonzero_ms:?} ms",
        db(peak),
        db(rms),
        if n > 0 { 100.0 * zeros as f64 / n as f64 } else { 0.0 }
    );
    println!("Windows meter peak during capture: {meter_peak:.6}");
    let live = peak > 0.0;
    println!(
        "RESULT: {}",
        if live {
            "SIGNAL (nonzero samples)"
        } else {
            "DIGITAL SILENCE"
        }
    );
    Ok(live)
}
