//! Persistence: settings.json plus named presets (TOML) under %APPDATA%\AT2020 Control Panel.

use core_dsp::params::ChainParams;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub input_id: Option<String>,
    pub output_name: Option<String>,
    pub monitor_name: Option<String>,
    pub monitor_enabled: bool,
    pub monitor_volume: f32,
    pub auto_start: bool,
    pub preset: String,
    pub params: ChainParams,
    /// Requested stream sample rate (`None` = device default).
    pub sample_rate: Option<u32>,
    /// Requested device buffer size in frames (`None` = device default).
    pub buffer_frames: Option<u32>,
    /// Use WASAPI exclusive mode where the devices accept it.
    pub exclusive_mode: bool,
    pub minimize_to_tray: bool,
    pub start_minimized: bool,
    pub start_with_windows: bool,
    pub hotkeys_enabled: bool,
    pub hotkey_mute: String,
    pub hotkey_bypass: String,
    pub hotkey_mic_mute: String,
    /// Name of the active theme (built-in or user TOML).
    pub theme: String,
    pub meter_style: crate::theme::MeterStyle,
    pub ab_b: Option<ChainParams>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input_id: None,
            output_name: None,
            monitor_name: None,
            monitor_enabled: false,
            monitor_volume: 0.7,
            auto_start: true,
            preset: "Voice (Discord / calls)".into(),
            params: ChainParams::voice(),
            sample_rate: None,
            buffer_frames: None,
            exclusive_mode: false,
            minimize_to_tray: true,
            start_minimized: false,
            start_with_windows: false,
            hotkeys_enabled: true,
            hotkey_mute: "Ctrl+Alt+M".into(),
            hotkey_bypass: "Ctrl+Alt+B".into(),
            hotkey_mic_mute: "Ctrl+Alt+Shift+M".into(),
            theme: "Follow Windows".into(),
            meter_style: crate::theme::MeterStyle::Bar,
            ab_b: None,
        }
    }
}

pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("AT2020 Control Panel")
}

pub fn presets_dir() -> PathBuf {
    app_dir().join("presets")
}

pub fn themes_dir() -> PathBuf {
    app_dir().join("themes")
}

pub fn logs_dir() -> PathBuf {
    app_dir().join("logs")
}

fn safe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Settings {
    pub fn load() -> Self {
        fs::read_to_string(app_dir().join("settings.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        let d = app_dir();
        let _ = fs::create_dir_all(&d);
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = fs::write(d.join("settings.json"), s);
        }
    }
}

// ---------------------------------------------------------------- presets --

pub fn params_to_toml(p: &ChainParams) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(p)
}

pub fn params_from_toml(s: &str) -> Result<ChainParams, toml::de::Error> {
    toml::from_str(s)
}

pub fn export_preset(path: &Path, p: &ChainParams) -> Result<(), String> {
    let s = params_to_toml(p).map_err(|e| e.to_string())?;
    fs::write(path, s).map_err(|e| e.to_string())
}

pub fn import_preset(path: &Path) -> Result<ChainParams, String> {
    let s = fs::read_to_string(path).map_err(|e| e.to_string())?;
    params_from_toml(&s).map_err(|e| e.to_string())
}

pub fn list_user_presets() -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(presets_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if !matches!(p.extension()?.to_str()?, "toml" | "json") {
                return None;
            }
            Some(p.file_stem()?.to_str()?.to_string())
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn load_user_preset(name: &str) -> Option<ChainParams> {
    let d = presets_dir();
    let stem = safe(name);
    if let Ok(s) = fs::read_to_string(d.join(format!("{stem}.toml"))) {
        if let Ok(p) = params_from_toml(&s) {
            return Some(p);
        }
    }
    // Presets written by earlier versions were JSON.
    serde_json::from_str(&fs::read_to_string(d.join(format!("{stem}.json"))).ok()?).ok()
}

pub fn save_user_preset(name: &str, p: &ChainParams) -> std::io::Result<()> {
    let d = presets_dir();
    fs::create_dir_all(&d)?;
    let s = params_to_toml(p).map_err(std::io::Error::other)?;
    fs::write(d.join(format!("{}.toml", safe(name))), s)
}

pub fn delete_user_preset(name: &str) {
    let d = presets_dir();
    let stem = safe(name);
    let _ = fs::remove_file(d.join(format!("{stem}.toml")));
    let _ = fs::remove_file(d.join(format!("{stem}.json")));
}

/// Resolves a preset by built-in name, user preset name, or path to a TOML file.
pub fn resolve_preset(name_or_path: &str) -> Result<ChainParams, String> {
    if let Some((_, p)) = ChainParams::builtin()
        .into_iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name_or_path))
    {
        return Ok(p);
    }
    // Short aliases for the built-ins.
    match name_or_path.to_ascii_lowercase().as_str() {
        "voice" => return Ok(ChainParams::voice()),
        "noisy" | "noisy-room" | "noisy room" => return Ok(ChainParams::noisy_room()),
        "flat" => return Ok(ChainParams::flat()),
        _ => {}
    }
    if let Some(p) = load_user_preset(name_or_path) {
        return Ok(p);
    }
    let path = Path::new(name_or_path);
    if path.is_file() {
        return import_preset(path);
    }
    Err(format!(
        "unknown preset \"{name_or_path}\" (built-ins: {})",
        ChainParams::builtin()
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_toml_round_trip_is_lossless() {
        for (_, p) in ChainParams::builtin() {
            let s = params_to_toml(&p).unwrap();
            let back = params_from_toml(&s).unwrap();
            assert_eq!(p, back);
        }
    }

    #[test]
    fn partial_toml_falls_back_to_defaults() {
        let p = params_from_toml("[comp]\nratio = 6.0\n").unwrap();
        assert_eq!(p.comp.ratio, 6.0);
        assert_eq!(p.gate, ChainParams::default().gate);
    }

    #[test]
    fn resolve_builtin_aliases() {
        assert_eq!(resolve_preset("flat").unwrap(), ChainParams::flat());
        assert!(resolve_preset("definitely-not-a-preset-xyz").is_err());
    }
}
