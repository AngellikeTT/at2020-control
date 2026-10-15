//! Small Windows integrations: theme preference, start-with-Windows, title-bar colour mode.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const PERSONALIZE_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
const RUN_VALUE: PCWSTR = w!("AT2020 Control Panel");

/// `Some(true)` when Windows apps use the light theme.
pub fn apps_use_light_theme() -> Option<bool> {
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: out-pointers reference live locals of the declared size.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PERSONALIZE_KEY,
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(data != 0)
}

/// Whether the Run-key entry for this app exists.
pub fn autostart_enabled() -> bool {
    let mut size = 0u32;
    // SAFETY: size query only.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    r == ERROR_SUCCESS
}

/// Adds or removes the per-user Run entry (no administrator rights needed).
pub fn set_autostart(enable: bool) -> Result<(), String> {
    // SAFETY: standard registry calls; the key handle is closed on every path.
    unsafe {
        let mut key = HKEY::default();
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_SET_VALUE, &mut key);
        if r != ERROR_SUCCESS {
            return Err(format!("could not open the Run key ({})", r.0));
        }
        let res = if enable {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let cmd = format!("\"{}\" --minimized", exe.display());
            let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            let r = RegSetValueExW(key, RUN_VALUE, 0, REG_SZ, Some(bytes));
            if r == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!("could not write the Run entry ({})", r.0))
            }
        } else {
            let r = RegDeleteValueW(key, RUN_VALUE);
            // Deleting something that is not there is fine.
            if r == ERROR_SUCCESS || r.0 == 2 {
                Ok(())
            } else {
                Err(format!("could not remove the Run entry ({})", r.0))
            }
        };
        let _ = RegCloseKey(key);
        res
    }
}

/// Switches the native title bar between dark and light (Windows 10 20H1+ / 11).
pub fn set_dark_title_bar(hwnd: isize, dark: bool) {
    const DWMWA_USE_IMMERSIVE_DARK_MODE: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(20);
    let v: i32 = i32::from(dark);
    // SAFETY: valid HWND supplied by winit; value pointer/size match the attribute.
    unsafe {
        let _ = DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &v as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}
