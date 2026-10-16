//! Who else is using an endpoint: the audio sessions Windows tracks for it.
//!
//! Needed because opening an endpoint in WASAPI *exclusive* mode while "Give exclusive
//! mode applications priority" is set (the Windows default) pre-empts every shared-mode
//! stream on it: those streams get AUDCLNT_E_DEVICE_INVALIDATED and stay dead until
//! their owner rebuilds them. Observed on this project's test machine: the self-test's
//! exclusive phase left Voicemeeter's microphone strip silent.

use anyhow::{bail, Result};
use windows::core::Interface;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{
    eCapture, eRender, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub pid: u32,
    /// The session has a running stream (AudioSessionStateActive).
    pub active: bool,
    /// Executable file name, or "pid N" when it cannot be resolved.
    pub exe: String,
}

/// The sessions an exclusive-mode open of the endpoint would pre-empt: running streams
/// of *other* processes (pid 0 is the system-sounds session, which never has a stream
/// of its own).
pub fn preempted_by_exclusive(sessions: &[SessionInfo], own_pid: u32) -> Vec<SessionInfo> {
    sessions
        .iter()
        .filter(|s| s.active && s.pid != 0 && s.pid != own_pid)
        .cloned()
        .collect()
}

/// "voicemeeter_x64.exe (pid 30236), Discord.exe (pid 4242)"
pub fn describe(sessions: &[SessionInfo]) -> String {
    sessions
        .iter()
        .map(|s| format!("{} (pid {})", s.exe, s.pid))
        .collect::<Vec<_>>()
        .join(", ")
}

fn exe_name(pid: u32) -> String {
    // SAFETY: plain Win32 calls; the handle is closed below and the buffer outlives the call.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return format!("pid {pid}");
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            h,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return format!("pid {pid}");
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit('\\').next().unwrap_or(&full).to_string()
    }
}

/// Lists the sessions on the active endpoint with this friendly name.
pub fn endpoint_sessions(name: &str, capture: bool) -> Result<Vec<SessionInfo>> {
    // Runs on its own thread so the caller's COM apartment (if any) does not matter.
    let name = name.to_string();
    match std::thread::spawn(move || unsafe { list(&name, capture) }).join() {
        Ok(r) => r,
        Err(_) => bail!("session enumeration thread panicked"),
    }
}

unsafe fn list(name: &str, capture: bool) -> Result<Vec<SessionInfo>> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            // SAFETY: paired with the CoInitializeEx below on this thread.
            unsafe { CoUninitialize() }
        }
    }
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let _com = Com;
    let out = (|| -> Result<Vec<SessionInfo>> {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let col = en.EnumAudioEndpoints(
            if capture { eCapture } else { eRender },
            DEVICE_STATE_ACTIVE,
        )?;
        for i in 0..col.GetCount()? {
            let d = col.Item(i)?;
            let pv = d
                .OpenPropertyStore(STGM_READ)?
                .GetValue(&PKEY_Device_FriendlyName)?;
            let p = PropVariantToStringAlloc(&pv)?;
            let n = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
            if n != name {
                continue;
            }
            let mgr: IAudioSessionManager2 = d.Activate(CLSCTX_ALL, None)?;
            let sessions = mgr.GetSessionEnumerator()?;
            let mut v = Vec::new();
            for k in 0..sessions.GetCount()? {
                let c = sessions.GetSession(k)?;
                let c2: IAudioSessionControl2 = c.cast()?;
                let pid = c2.GetProcessId().unwrap_or(0);
                v.push(SessionInfo {
                    pid,
                    active: c.GetState()? == AudioSessionStateActive,
                    exe: if pid == 0 {
                        "system".into()
                    } else {
                        exe_name(pid)
                    },
                });
            }
            return Ok(v);
        }
        bail!("endpoint \"{name}\" not found")
    })();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(pid: u32, active: bool) -> SessionInfo {
        SessionInfo {
            pid,
            active,
            exe: format!("app{pid}.exe"),
        }
    }

    #[test]
    fn only_other_running_streams_are_preempted() {
        let all = [s(0, true), s(10, true), s(11, false), s(12, true)];
        let p = preempted_by_exclusive(&all, 12);
        assert_eq!(p, vec![s(10, true)]);
        assert_eq!(describe(&p), "app10.exe (pid 10)");
    }

    #[test]
    fn nothing_to_preempt_when_alone() {
        assert!(preempted_by_exclusive(&[s(5, true), s(0, true)], 5).is_empty());
    }
}
