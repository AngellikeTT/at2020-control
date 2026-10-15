//! Theme system: nine built-in themes, "Follow Windows", user themes loaded from
//! `%APPDATA%\AT2020 Control Panel\themes\*.toml` (hot-reloaded), and the egui styling
//! derived from them.

use crate::settings;
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

pub const FOLLOW_WINDOWS: &str = "Follow Windows";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MeterStyle {
    /// Continuous bar with a colour gradient.
    #[default]
    Bar,
    /// Discrete LED-style segments.
    Segments,
    /// Analogue-style arc gauge with a needle.
    Needle,
}

impl MeterStyle {
    pub const ALL: [MeterStyle; 3] = [MeterStyle::Bar, MeterStyle::Segments, MeterStyle::Needle];
    pub fn label(self) -> &'static str {
        match self {
            MeterStyle::Bar => "Bar",
            MeterStyle::Segments => "Segments",
            MeterStyle::Needle => "Needle",
        }
    }
}

/// An sRGB colour that serialises as "#rrggbb".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    pub fn color(self) -> Color32 {
        Color32::from_rgb(self.0[0], self.0[1], self.0[2])
    }
    pub fn from_color(c: Color32) -> Self {
        Rgb([c.r(), c.g(), c.b()])
    }
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2])
    }
    pub fn parse(s: &str) -> Option<Self> {
        let h = s.trim().trim_start_matches('#');
        if h.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8]))
    }
}

impl Serialize for Rgb {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}
impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad colour \"{s}\"")))
    }
}

const fn c(r: u8, g: u8, b: u8) -> Rgb {
    Rgb([r, g, b])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub bg: Rgb,
    pub panel: Rgb,
    pub widget: Rgb,
    pub widget_hover: Rgb,
    pub text: Rgb,
    pub text_weak: Rgb,
    pub accent: Rgb,
    pub ok: Rgb,
    pub warn: Rgb,
    pub danger: Rgb,
    pub plot_bg: Rgb,
    pub plot_grid: Rgb,
    pub spectrum_in: Rgb,
    pub spectrum_out: Rgb,
    pub rounding: f32,
}

impl Default for Theme {
    fn default() -> Self {
        builtin_themes().remove(0)
    }
}

fn mk(name: &str, dark: bool, cols: [(u8, u8, u8); 14], rounding: f32) -> Theme {
    let r = |i: usize| c(cols[i].0, cols[i].1, cols[i].2);
    Theme {
        name: name.into(),
        dark,
        bg: r(0),
        panel: r(1),
        widget: r(2),
        widget_hover: r(3),
        text: r(4),
        text_weak: r(5),
        accent: r(6),
        ok: r(7),
        warn: r(8),
        danger: r(9),
        plot_bg: r(10),
        plot_grid: r(11),
        spectrum_in: r(12),
        spectrum_out: r(13),
        rounding,
    }
}

/// The nine built-in themes.
pub fn builtin_themes() -> Vec<Theme> {
    vec![
        mk(
            "Midnight",
            true,
            [
                (18, 20, 26),
                (24, 27, 35),
                (38, 43, 56),
                (52, 59, 77),
                (226, 230, 240),
                (140, 148, 165),
                (90, 170, 255),
                (70, 190, 110),
                (225, 190, 60),
                (235, 80, 70),
                (14, 16, 21),
                (44, 50, 64),
                (120, 130, 150),
                (90, 170, 255),
            ],
            6.0,
        ),
        mk(
            "Graphite",
            true,
            [
                (30, 30, 30),
                (37, 37, 38),
                (51, 51, 53),
                (66, 66, 69),
                (224, 224, 224),
                (150, 150, 150),
                (14, 165, 130),
                (80, 200, 120),
                (230, 190, 70),
                (230, 90, 80),
                (24, 24, 24),
                (58, 58, 60),
                (130, 130, 130),
                (14, 165, 130),
            ],
            4.0,
        ),
        mk(
            "Daylight",
            false,
            [
                (243, 244, 247),
                (250, 250, 252),
                (228, 231, 238),
                (210, 215, 226),
                (28, 32, 40),
                (96, 104, 120),
                (0, 102, 204),
                (30, 145, 70),
                (190, 130, 0),
                (200, 45, 40),
                (255, 255, 255),
                (214, 219, 228),
                (120, 128, 144),
                (0, 102, 204),
            ],
            6.0,
        ),
        mk(
            "Solarized Dark",
            true,
            [
                (0, 43, 54),
                (7, 54, 66),
                (20, 70, 84),
                (35, 88, 103),
                (147, 161, 161),
                (101, 123, 131),
                (38, 139, 210),
                (133, 153, 0),
                (181, 137, 0),
                (220, 50, 47),
                (0, 36, 46),
                (30, 76, 90),
                (88, 110, 117),
                (38, 139, 210),
            ],
            4.0,
        ),
        mk(
            "Solarized Light",
            false,
            [
                (253, 246, 227),
                (238, 232, 213),
                (226, 219, 197),
                (210, 202, 178),
                (62, 84, 92),
                (101, 123, 131),
                (38, 139, 210),
                (133, 153, 0),
                (181, 137, 0),
                (220, 50, 47),
                (253, 246, 227),
                (214, 206, 182),
                (131, 148, 150),
                (38, 139, 210),
            ],
            4.0,
        ),
        mk(
            "Nord",
            true,
            [
                (46, 52, 64),
                (59, 66, 82),
                (67, 76, 94),
                (76, 86, 106),
                (216, 222, 233),
                (144, 152, 170),
                (136, 192, 208),
                (163, 190, 140),
                (235, 203, 139),
                (191, 97, 106),
                (41, 46, 57),
                (76, 86, 106),
                (129, 161, 193),
                (136, 192, 208),
            ],
            6.0,
        ),
        mk(
            "Dracula",
            true,
            [
                (40, 42, 54),
                (33, 34, 44),
                (68, 71, 90),
                (86, 90, 114),
                (248, 248, 242),
                (150, 156, 190),
                (189, 147, 249),
                (80, 250, 123),
                (241, 250, 140),
                (255, 85, 85),
                (30, 31, 41),
                (68, 71, 90),
                (139, 233, 253),
                (189, 147, 249),
            ],
            6.0,
        ),
        mk(
            "High Contrast",
            true,
            [
                (0, 0, 0),
                (10, 10, 10),
                (30, 30, 30),
                (60, 60, 60),
                (255, 255, 255),
                (200, 200, 200),
                (0, 200, 255),
                (0, 255, 90),
                (255, 230, 0),
                (255, 60, 60),
                (0, 0, 0),
                (110, 110, 110),
                (255, 255, 255),
                (0, 200, 255),
            ],
            2.0,
        ),
        mk(
            "Amber Console",
            true,
            [
                (20, 14, 6),
                (28, 20, 8),
                (46, 33, 12),
                (66, 48, 18),
                (255, 196, 90),
                (178, 134, 62),
                (255, 170, 30),
                (170, 220, 80),
                (255, 200, 60),
                (240, 90, 50),
                (14, 9, 3),
                (64, 46, 16),
                (190, 140, 60),
                (255, 170, 30),
            ],
            2.0,
        ),
    ]
}

impl Theme {
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
    pub fn from_toml(s: &str) -> Result<Theme, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn visuals(&self) -> egui::Visuals {
        let mut v = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let (bg, panel, widget, hover, text, accent) = (
            self.bg.color(),
            self.panel.color(),
            self.widget.color(),
            self.widget_hover.color(),
            self.text.color(),
            self.accent.color(),
        );
        v.panel_fill = panel;
        v.window_fill = panel;
        v.extreme_bg_color = self.plot_bg.color();
        v.faint_bg_color = bg;
        v.override_text_color = Some(text);
        v.hyperlink_color = accent;
        v.selection.bg_fill = accent.gamma_multiply(if self.dark { 0.55 } else { 0.35 });
        v.selection.stroke = egui::Stroke::new(1.0_f32, accent);
        v.warn_fg_color = self.warn.color();
        v.error_fg_color = self.danger.color();
        let r = egui::Rounding::same(self.rounding);
        v.window_rounding = r;
        v.menu_rounding = r;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.rounding = r;
        }
        v.widgets.noninteractive.bg_fill = panel;
        v.widgets.noninteractive.weak_bg_fill = panel;
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, self.plot_grid.color());
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, text);
        v.widgets.inactive.bg_fill = widget;
        v.widgets.inactive.weak_bg_fill = widget;
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, text);
        v.widgets.hovered.bg_fill = hover;
        v.widgets.hovered.weak_bg_fill = hover;
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5_f32, text);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, accent);
        v.widgets.active.bg_fill = accent.gamma_multiply(0.7);
        v.widgets.active.weak_bg_fill = accent.gamma_multiply(0.7);
        v.widgets.active.fg_stroke = egui::Stroke::new(2.0_f32, text);
        v
    }

    pub fn ok(&self) -> Color32 {
        self.ok.color()
    }
    pub fn warn(&self) -> Color32 {
        self.warn.color()
    }
    pub fn danger(&self) -> Color32 {
        self.danger.color()
    }
    pub fn accent(&self) -> Color32 {
        self.accent.color()
    }
    pub fn weak(&self) -> Color32 {
        self.text_weak.color()
    }
    pub fn plot_bg(&self) -> Color32 {
        self.plot_bg.color()
    }
    pub fn grid(&self) -> Color32 {
        self.plot_grid.color()
    }
}

/// Reads the Windows "apps use light theme" setting. Defaults to dark.
pub fn windows_prefers_light() -> bool {
    device::winsys::apps_use_light_theme().unwrap_or(false)
}

/// Holds built-in and user themes and resolves the active one.
pub struct ThemeStore {
    pub builtin: Vec<Theme>,
    pub user: Vec<Theme>,
    dir: PathBuf,
    mtimes: HashMap<PathBuf, SystemTime>,
    last_scan: Instant,
}

impl ThemeStore {
    pub fn new() -> Self {
        let mut s = Self {
            builtin: builtin_themes(),
            user: Vec::new(),
            dir: settings::themes_dir(),
            mtimes: HashMap::new(),
            last_scan: Instant::now() - Duration::from_secs(10),
        };
        s.scan(true);
        s
    }

    /// Re-reads the theme folder when any file changed. Returns true if the set changed.
    pub fn scan(&mut self, force: bool) -> bool {
        if !force && self.last_scan.elapsed() < Duration::from_millis(750) {
            return false;
        }
        self.last_scan = Instant::now();
        let mut now: HashMap<PathBuf, SystemTime> = HashMap::new();
        if let Ok(rd) = fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("toml") {
                    if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                        now.insert(p, m);
                    }
                }
            }
        }
        if now == self.mtimes && !force {
            return false;
        }
        self.mtimes = now;
        let mut user = Vec::new();
        let mut paths: Vec<_> = self.mtimes.keys().cloned().collect();
        paths.sort();
        for p in paths {
            if let Ok(s) = fs::read_to_string(&p) {
                if let Ok(mut t) = Theme::from_toml(&s) {
                    if t.name.trim().is_empty()
                        || t.name == "Midnight"
                            && self.builtin.iter().any(|b| b.name == t.name)
                            && p.file_stem().and_then(|x| x.to_str()) != Some("Midnight")
                    {
                        t.name = p
                            .file_stem()
                            .and_then(|x| x.to_str())
                            .unwrap_or("User theme")
                            .to_string();
                    }
                    user.push(t);
                }
            }
        }
        let changed = user != self.user;
        self.user = user;
        changed
    }

    pub fn names(&self) -> Vec<String> {
        let mut v = vec![FOLLOW_WINDOWS.to_string()];
        v.extend(self.builtin.iter().map(|t| t.name.clone()));
        v.extend(self.user.iter().map(|t| t.name.clone()));
        v
    }

    pub fn find(&self, name: &str) -> Option<&Theme> {
        self.user
            .iter()
            .chain(self.builtin.iter())
            .find(|t| t.name == name)
    }

    pub fn resolve(&self, name: &str) -> Theme {
        if name == FOLLOW_WINDOWS {
            let n = if windows_prefers_light() {
                "Daylight"
            } else {
                "Midnight"
            };
            return self.find(n).cloned().unwrap_or_default();
        }
        self.find(name).cloned().unwrap_or_default()
    }

    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtin.iter().any(|t| t.name == name)
    }

    pub fn save_user(&mut self, t: &Theme) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let stem: String = t
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let s = t.to_toml().map_err(std::io::Error::other)?;
        fs::write(self.dir.join(format!("{stem}.toml")), s)?;
        self.scan(true);
        Ok(())
    }

    pub fn delete_user(&mut self, name: &str) {
        let paths: Vec<PathBuf> = self.mtimes.keys().cloned().collect();
        for p in paths {
            if let Ok(s) = fs::read_to_string(&p) {
                if Theme::from_toml(&s).is_ok_and(|t| t.name == name) {
                    let _ = fs::remove_file(&p);
                }
            }
        }
        self.scan(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_nine_unique_builtins() {
        let t = builtin_themes();
        assert_eq!(t.len(), 9);
        let mut n: Vec<_> = t.iter().map(|t| t.name.clone()).collect();
        n.sort();
        n.dedup();
        assert_eq!(n.len(), 9);
    }

    #[test]
    fn every_builtin_round_trips_through_toml() {
        for t in builtin_themes() {
            let s = t.to_toml().unwrap();
            let back = Theme::from_toml(&s).unwrap();
            assert_eq!(t, back, "{}", t.name);
        }
    }

    #[test]
    fn hex_colours_parse_and_format() {
        assert_eq!(Rgb::parse("#5aaaff"), Some(Rgb([0x5a, 0xaa, 0xff])));
        assert_eq!(Rgb([1, 2, 255]).hex(), "#0102ff");
        assert_eq!(Rgb::parse("nope"), None);
    }

    #[test]
    fn partial_theme_file_uses_defaults() {
        let t = Theme::from_toml("name = \"Mine\"\naccent = \"#ff0000\"\n").unwrap();
        assert_eq!(t.name, "Mine");
        assert_eq!(t.accent, Rgb([255, 0, 0]));
        assert_eq!(t.bg, Theme::default().bg);
    }

    #[test]
    fn text_is_readable_on_every_background() {
        // WCAG-style relative luminance contrast >= 4.5 for body text on panel fill.
        fn lum(c: Rgb) -> f64 {
            let f = |v: u8| {
                let s = v as f64 / 255.0;
                if s <= 0.03928 {
                    s / 12.92
                } else {
                    ((s + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(c.0[0]) + 0.7152 * f(c.0[1]) + 0.0722 * f(c.0[2])
        }
        for t in builtin_themes() {
            let (a, b) = (lum(t.text), lum(t.panel));
            let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(ratio >= 4.5, "{} contrast {ratio:.2}", t.name);
        }
    }
}
