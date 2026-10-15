//! Persistence: settings.json plus named presets under %APPDATA%\AT2020 Control Panel.

use crate::params::ChainParams;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
        }
    }
}

fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("AT2020 Control Panel")
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
        fs::read_to_string(dir().join("settings.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        let d = dir();
        let _ = fs::create_dir_all(&d);
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = fs::write(d.join("settings.json"), s);
        }
    }
}

pub fn list_user_presets() -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir().join("presets"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? != "json" {
                return None;
            }
            Some(p.file_stem()?.to_str()?.to_string())
        })
        .collect();
    v.sort();
    v
}

pub fn load_user_preset(name: &str) -> Option<ChainParams> {
    serde_json::from_str(
        &fs::read_to_string(dir().join("presets").join(format!("{}.json", safe(name)))).ok()?,
    )
    .ok()
}

pub fn save_user_preset(name: &str, p: &ChainParams) -> std::io::Result<()> {
    let d = dir().join("presets");
    fs::create_dir_all(&d)?;
    fs::write(
        d.join(format!("{}.json", safe(name))),
        serde_json::to_string_pretty(p).unwrap_or_default(),
    )
}

pub fn delete_user_preset(name: &str) {
    let _ = fs::remove_file(dir().join("presets").join(format!("{}.json", safe(name))));
}
