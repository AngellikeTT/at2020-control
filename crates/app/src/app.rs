//! The egui application: device control, presets, processing-chain editor, meters.
//! Tab bodies live in `tabs.rs`.

use crate::integration::{Action, Integration};
use crate::settings::{self, Settings};
use crate::theme::{Theme, ThemeStore};
use crate::widgets::*;
use core_dsp::params::*;
use core_dsp::spectrum::Analyzer;
use device::devctl::{Cmd, DevCtl, DevState};
use device::engine::{query_caps, DeviceCaps, Engine, EngineConfig};
use device::recorder::Recorder;
use device::shared::Shared;
use eframe::egui::{self, Color32, RichText, Vec2};
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub type RunCfg = (String, String, Option<String>, EngineConfig);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Chain,
    Spectrum,
    Audio,
    Setup,
    Presets,
    Appearance,
    System,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Tab::Chain,
        Tab::Spectrum,
        Tab::Audio,
        Tab::Setup,
        Tab::Presets,
        Tab::Appearance,
        Tab::System,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Tab::Chain => "Chain",
            Tab::Spectrum => "Spectrum",
            Tab::Audio => "Audio",
            Tab::Setup => "Cable setup",
            Tab::Presets => "Presets",
            Tab::Appearance => "Appearance",
            Tab::System => "System",
        }
    }
    pub fn slug(self) -> &'static str {
        match self {
            Tab::Chain => "chain",
            Tab::Spectrum => "spectrum",
            Tab::Audio => "audio",
            Tab::Setup => "setup",
            Tab::Presets => "presets",
            Tab::Appearance => "appearance",
            Tab::System => "system",
        }
    }
}

#[derive(Default, Clone)]
pub struct LaunchOptions {
    pub minimized: bool,
    pub screenshots: Option<PathBuf>,
}

struct ShotState {
    dir: PathBuf,
    themes: Vec<String>,
    idx: usize,
    wait: u32,
    pending: Option<String>,
    requested: bool,
    engine_wait: u32,
    timeout: u32,
}

pub struct App {
    pub(crate) devctl: DevCtl,
    pub(crate) shared: Arc<Shared>,
    pub(crate) engine: Option<Engine>,
    running_cfg: Option<RunCfg>,
    pub(crate) want_running: bool,
    last_attempt: Instant,
    pub(crate) status: String,

    pub(crate) settings: Settings,
    pub(crate) params: ChainParams,
    pub(crate) ab_on_b: bool,
    pub(crate) ab_other: ChainParams,
    pub(crate) user_presets: Vec<String>,
    pub(crate) new_preset_name: String,
    pub(crate) preset_msg: String,

    pub(crate) dev: DevState,
    seen_rev: u64,
    ui_vol: f32,
    vol_hold_until: Instant,

    pub(crate) m_in: MeterView,
    pub(crate) m_out: MeterView,
    pub(crate) m_hw: MeterView,
    pub(crate) gr: f32,
    pub(crate) deess_gr: f32,
    last_frame: Instant,
    dirty_since: Option<Instant>,

    pub(crate) tab: Tab,
    pub(crate) analyzer: Analyzer,
    pub(crate) caps_in: Option<DeviceCaps>,
    pub(crate) caps_out: Option<DeviceCaps>,
    caps_key: Option<(String, String)>,
    pub(crate) recorder: Option<Recorder>,
    pub(crate) last_recording: Option<(PathBuf, f32)>,
    pub(crate) rec_raw: bool,
    pub(crate) rec_msg: String,
    xrun_base: u64,

    pub(crate) themes: ThemeStore,
    pub(crate) theme: Theme,
    applied_theme: Option<Theme>,
    pub(crate) editing: Theme,
    pub(crate) theme_msg: String,

    integration: Integration,
    pub(crate) hotkey_edit: [String; 3],
    hotkeys_applied: bool,
    hidden: bool,
    hide_on_first_frame: bool,
    last_tip: String,
    quitting: bool,
    shots: Option<ShotState>,
    title_dark: Option<bool>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, opts: LaunchOptions) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        cc.egui_ctx.style_mut(|s| s.spacing.slider_width = 190.0);

        let mut settings = Settings::load();
        let themes = ThemeStore::new();
        let theme = themes.resolve(&settings.theme);
        let params = settings.params;
        let shared = Arc::new(Shared::new(params));
        shared.monitor_vol.store(settings.monitor_volume);
        let devctl = DevCtl::spawn({
            let c = cc.egui_ctx.clone();
            Arc::new(move || c.request_repaint())
        });
        if let Some(id) = settings.input_id.clone() {
            devctl.send(Cmd::Select(id));
        }
        settings.start_with_windows = device::winsys::autostart_enabled();
        let now = Instant::now();
        let shots = opts.screenshots.map(|dir| {
            let mut names = themes.names();
            names.retain(|n| n != crate::theme::FOLLOW_WINDOWS);
            ShotState {
                dir,
                themes: names,
                idx: 0,
                wait: 40,
                pending: None,
                requested: false,
                engine_wait: 0,
                timeout: 0,
            }
        });
        let analyzer = Analyzer::new(shared.spec_in.clone(), shared.spec_out.clone());
        let hotkey_edit = [
            settings.hotkey_mute.clone(),
            settings.hotkey_bypass.clone(),
            settings.hotkey_mic_mute.clone(),
        ];
        let ab_other = settings.ab_b.unwrap_or(params);
        Self {
            devctl,
            shared,
            engine: None,
            running_cfg: None,
            want_running: settings.auto_start,
            last_attempt: now - Duration::from_secs(10),
            status: "Starting…".into(),
            user_presets: settings::list_user_presets(),
            new_preset_name: String::new(),
            preset_msg: String::new(),
            params,
            ab_on_b: false,
            ab_other,
            settings,
            dev: DevState::default(),
            seen_rev: 0,
            ui_vol: 0.0,
            vol_hold_until: now,
            m_in: MeterView::new(),
            m_out: MeterView::new(),
            m_hw: MeterView::new(),
            gr: 0.0,
            deess_gr: 0.0,
            last_frame: now,
            dirty_since: None,
            tab: Tab::Chain,
            analyzer,
            caps_in: None,
            caps_out: None,
            caps_key: None,
            recorder: None,
            last_recording: None,
            rec_raw: false,
            rec_msg: String::new(),
            xrun_base: 0,
            editing: theme.clone(),
            themes,
            theme,
            applied_theme: None,
            theme_msg: String::new(),
            integration: Integration::new(&cc.egui_ctx),
            hotkey_edit,
            hotkeys_applied: false,
            hidden: opts.minimized,
            hide_on_first_frame: opts.minimized,
            last_tip: String::new(),
            quitting: false,
            shots,
            title_dark: None,
        }
    }

    pub(crate) fn mark_dirty(&mut self) {
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    pub(crate) fn engine_cfg(&self) -> EngineConfig {
        EngineConfig {
            sample_rate: self.settings.sample_rate,
            buffer_frames: self.settings.buffer_frames,
            exclusive: self.settings.exclusive_mode,
        }
    }

    fn on_devices_changed(&mut self) {
        if self.settings.input_id.is_none() {
            self.settings.input_id = self.dev.selected_id.clone();
            self.mark_dirty();
        }
        if self.settings.output_name.is_none() {
            let pick = self
                .dev
                .renders
                .iter()
                .find(|d| d.name.to_lowercase().starts_with("cable input"))
                .or_else(|| self.dev.renders.iter().find(|d| d.virtual_cable));
            if let Some(c) = pick {
                self.settings.output_name = Some(c.name.clone());
                self.mark_dirty();
            }
        }
    }

    /// What the engine *should* be running right now, if everything it needs is present.
    fn desired_cfg(&self) -> Option<RunCfg> {
        let inp = self.dev.selected_name.clone()?;
        let out = self
            .settings
            .output_name
            .clone()
            .filter(|n| self.dev.renders.iter().any(|d| &d.name == n))?;
        let mon = if self.settings.monitor_enabled {
            self.settings
                .monitor_name
                .clone()
                .filter(|n| self.dev.renders.iter().any(|d| &d.name == n))
        } else {
            None
        };
        Some((inp, out, mon, self.engine_cfg()))
    }

    fn supervise(&mut self, now: Instant) {
        let desired = self.desired_cfg();
        if self.engine.is_some() && (!self.want_running || self.running_cfg != desired) {
            self.engine = None;
            self.running_cfg = None;
            self.last_attempt = now - Duration::from_secs(10); // deliberate change: restart immediately
            self.status = if self.want_running {
                "Restarting…".into()
            } else {
                "Stopped".into()
            };
        }
        if self.engine.is_none()
            && self.want_running
            && now.duration_since(self.last_attempt) > Duration::from_secs(2)
        {
            self.last_attempt = now;
            match desired {
                None => {
                    self.status = if self.settings.output_name.is_none() {
                        "Choose an output device (a virtual cable such as VB-CABLE).".into()
                    } else {
                        "Waiting for the microphone / output device to be available…".into()
                    }
                }
                Some((i, o, m, c)) => {
                    match Engine::start(&i, &o, m.as_deref(), &c, self.shared.clone()) {
                        Ok(e) => {
                            let inf = e.info.clone();
                            self.status = format!(
                                "Running — in {} Hz/{}ch → out {} Hz/{}ch",
                                inf.in_rate, inf.in_channels, inf.out_rate, inf.out_channels
                            );
                            tracing::info!(
                                input = %i, output = %o, in_rate = inf.in_rate,
                                out_rate = inf.out_rate, "engine started"
                            );
                            self.engine = Some(e);
                            self.running_cfg = Some((i, o, m, c));
                        }
                        Err(e) => {
                            tracing::warn!("engine start failed: {e:#}");
                            self.status = format!("Could not start audio: {e:#}");
                        }
                    }
                }
            }
        }
        if !self.want_running && self.engine.is_none() {
            self.status = "Stopped".into();
        }
    }

    pub(crate) fn apply_params(&mut self, p: ChainParams) {
        self.params = p;
        self.settings.params = p;
        self.shared.params.set(p);
        self.mark_dirty();
    }

    pub(crate) fn apply_preset(&mut self, name: &str) {
        let p = ChainParams::builtin()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| p)
            .or_else(|| settings::load_user_preset(name));
        if let Some(p) = p {
            self.settings.preset = name.to_string();
            self.apply_params(p);
        }
    }

    pub(crate) fn toggle_ab(&mut self) {
        let cur = self.params;
        let other = self.ab_other;
        self.ab_other = cur;
        self.ab_on_b = !self.ab_on_b;
        self.settings.ab_b = Some(if self.ab_on_b { cur } else { other });
        self.apply_params(other);
    }

    /// Refresh device capability lists when the selection changes.
    fn refresh_caps(&mut self) {
        let key = (
            self.dev.selected_name.clone().unwrap_or_default(),
            self.settings.output_name.clone().unwrap_or_default(),
        );
        if self.caps_key.as_ref() == Some(&key) {
            return;
        }
        self.caps_in = if key.0.is_empty() {
            None
        } else {
            query_caps(&key.0, true)
        };
        self.caps_out = if key.1.is_empty() {
            None
        } else {
            query_caps(&key.1, false)
        };
        self.caps_key = Some(key);
    }

    fn handle_actions(&mut self, ctx: &egui::Context) {
        for a in self.integration.poll() {
            match a {
                Action::ShowWindow => {
                    self.hidden = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Action::ToggleOutputMute => {
                    let mut p = self.params;
                    p.output.mute = !p.output.mute;
                    self.apply_params(p);
                }
                Action::ToggleBypass => {
                    let mut p = self.params;
                    p.bypass = !p.bypass;
                    self.apply_params(p);
                }
                Action::ToggleMicMute => {
                    self.devctl.send(Cmd::SetMute(!self.dev.mute));
                }
                Action::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    pub(crate) fn apply_hotkeys(&mut self) {
        self.settings.hotkey_mute = self.hotkey_edit[0].clone();
        self.settings.hotkey_bypass = self.hotkey_edit[1].clone();
        self.settings.hotkey_mic_mute = self.hotkey_edit[2].clone();
        self.integration.set_hotkeys(
            self.settings.hotkeys_enabled,
            &[
                (self.hotkey_edit[0].as_str(), Action::ToggleOutputMute),
                (self.hotkey_edit[1].as_str(), Action::ToggleBypass),
                (self.hotkey_edit[2].as_str(), Action::ToggleMicMute),
            ],
        );
        self.hotkeys_applied = true;
        self.mark_dirty();
    }

    pub(crate) fn hotkey_edit_mut(&mut self) -> &mut [String; 3] {
        &mut self.hotkey_edit
    }

    pub(crate) fn hotkey_errors(&self) -> &[String] {
        &self.integration.hotkey_errors
    }

    pub(crate) fn has_tray(&self) -> bool {
        self.integration.has_tray()
    }

    /// Output xruns since the last reset.
    pub(crate) fn xruns(&self) -> u64 {
        self.shared
            .underruns
            .load(Relaxed)
            .saturating_sub(self.xrun_base)
    }

    pub(crate) fn reset_xruns(&mut self) {
        self.xrun_base = self.shared.underruns.load(Relaxed);
    }

    fn apply_theme(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let wanted = self.themes.resolve(&self.settings.theme);
        if self.applied_theme.as_ref() != Some(&wanted) {
            ctx.set_visuals(wanted.visuals());
            set_theme(ctx, &wanted);
            self.theme = wanted.clone();
            self.applied_theme = Some(wanted);
        }
        if self.title_dark != Some(self.theme.dark) {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = frame.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    device::winsys::set_dark_title_bar(w.hwnd.get(), self.theme.dark);
                    self.title_dark = Some(self.theme.dark);
                }
            }
        }
    }

    fn drive_screenshots(&mut self, ctx: &egui::Context) {
        let Some(s) = self.shots.as_mut() else { return };
        ctx.request_repaint();
        // A finished capture?
        let img = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = img {
            if let Some(name) = s.pending.take() {
                let _ = std::fs::create_dir_all(&s.dir);
                let (w, h) = (img.size[0] as u32, img.size[1] as u32);
                let mut raw = Vec::with_capacity(img.pixels.len() * 4);
                for p in &img.pixels {
                    raw.extend_from_slice(&[p.r(), p.g(), p.b(), p.a()]);
                }
                match image::RgbaImage::from_raw(w, h, raw) {
                    Some(buf) => {
                        let path = s.dir.join(format!("{name}.png"));
                        match buf.save(&path) {
                            Ok(()) => tracing::info!("screenshot saved: {}", path.display()),
                            Err(e) => tracing::warn!("screenshot save failed: {e}"),
                        }
                    }
                    None => tracing::warn!("screenshot buffer size mismatch"),
                }
                s.idx += 1;
                s.wait = 4;
                s.requested = false;
            }
        }
        if s.wait > 0 {
            s.wait -= 1;
            return;
        }
        // Let the engine come up (up to ~15 s) and the meters/analyzer fill before the first shot.
        if s.idx == 0 && s.pending.is_none() && s.engine_wait < 450 {
            s.engine_wait += 1;
            if self.engine.is_none() || s.engine_wait < 60 {
                return;
            }
        }
        if s.pending.is_some() {
            if !s.requested {
                s.requested = true;
                s.timeout = 180;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            } else if s.timeout == 0 {
                tracing::warn!("screenshot timed out; skipping");
                s.pending = None;
                s.idx += 1;
                s.requested = false;
            } else {
                s.timeout -= 1;
            }
            return;
        }
        let per_theme = Tab::ALL.len();
        let total = s.themes.len() * per_theme;
        if s.idx >= total {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let (ti, tabi) = (s.idx / per_theme, s.idx % per_theme);
        let theme_name = s.themes[ti].clone();
        let tab = Tab::ALL[tabi];
        let file = format!(
            "{}__{}",
            theme_name
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect::<String>(),
            tab.slug()
        );
        s.pending = Some(file);
        s.requested = false;
        s.wait = 14; // let the new theme/tab settle for a few frames before capturing
        self.settings.theme = theme_name;
        self.tab = tab;
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.2);
        self.last_frame = now;

        if self.themes.scan(false) {
            self.applied_theme = None;
        }
        self.apply_theme(ctx, frame);
        if !self.hotkeys_applied && self.shots.is_none() {
            self.apply_hotkeys();
        }
        self.handle_actions(ctx);

        // Close button -> hide to tray (when enabled and a tray exists).
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quitting
            && self.settings.minimize_to_tray
            && self.integration.has_tray()
            && self.shots.is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden = true;
        }
        if self.hide_on_first_frame {
            // Start-minimised launch (e.g. from the Run key): hide once, keep running.
            self.hide_on_first_frame = false;
            if self.integration.has_tray() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                self.hidden = false;
            }
        }

        self.dev = self.devctl.snapshot();
        if self.dev.revision != self.seen_rev {
            self.seen_rev = self.dev.revision;
            self.on_devices_changed();
        }
        if let Some(f) = self.shared.take_fault() {
            tracing::warn!("stream fault: {f}");
            self.engine = None;
            self.running_cfg = None;
            self.status = format!("{f} — retrying…");
            self.last_attempt = now;
        }
        self.supervise(now);
        self.refresh_caps();

        self.shared.monitor_vol.store(self.settings.monitor_volume);
        self.m_in.update(self.shared.meters.in_peak.take(), dt);
        self.m_out.update(self.shared.meters.out_peak.take(), dt);
        self.m_hw.update(self.dev.peak, dt);
        let decay = |cur: f32, now: f32| {
            if now > cur {
                now
            } else {
                (cur - 40.0 * dt).max(now)
            }
        };
        self.gr = decay(self.gr, self.shared.meters.gr_db.take());
        self.deess_gr = decay(self.deess_gr, self.shared.meters.deess_gr_db.take());
        if self.engine.is_some() {
            self.analyzer.update(dt);
        }
        if let Some(r) = &self.recorder {
            if r.seconds() > 600.0 {
                // Safety cap: 10 minutes per test recording.
                self.stop_recording();
            }
        }

        self.drive_screenshots(ctx);

        let mut p = self.params;
        let mut changed = false;
        self.top_panel(ctx, &mut p, &mut changed);
        self.bottom_panel(ctx);
        self.left_panel(ctx, now);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                for t in Tab::ALL {
                    if ui.selectable_label(self.tab == t, t.label()).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match self.tab {
                    Tab::Chain => changed |= self.chain_ui(ui, &mut p),
                    Tab::Spectrum => self.spectrum_tab(ui),
                    Tab::Audio => self.audio_tab(ui),
                    Tab::Setup => self.setup_tab(ui),
                    Tab::Presets => changed |= self.presets_tab(ui, &mut p),
                    Tab::Appearance => self.appearance_tab(ui),
                    Tab::System => self.system_tab(ui),
                });
        });

        if changed {
            self.apply_params(p);
        }
        if self
            .dirty_since
            .is_some_and(|t| now.duration_since(t) > Duration::from_millis(800))
        {
            if self.shots.is_none() {
                self.settings.save();
            }
            self.dirty_since = None;
        }
        if self.integration.has_tray() {
            let state = if self.params.output.mute {
                "output muted"
            } else if self.params.bypass {
                "bypassed"
            } else if self.engine.is_some() {
                "running"
            } else {
                "stopped"
            };
            let tip = format!("AT2020USB-X Control Panel — {state}");
            if tip != self.last_tip {
                self.integration.set_tooltip(&tip);
                self.last_tip = tip;
            }
        }
        ctx.request_repaint_after(Duration::from_millis(if self.hidden { 250 } else { 33 }));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_recording();
        self.settings.params = self.params;
        if self.shots.is_none() {
            self.settings.save();
        }
        self.engine = None;
    }
}

impl App {
    pub(crate) fn stop_recording(&mut self) {
        if let Some(r) = self.recorder.take() {
            let path = r.path.clone();
            match r.finish() {
                Ok(secs) => {
                    self.rec_msg = format!("Saved {secs:.1} s to {}", path.display());
                    self.last_recording = Some((path, secs));
                }
                Err(e) => self.rec_msg = format!("Recording failed: {e:#}"),
            }
        }
    }

    pub(crate) fn start_recording(&mut self) {
        let dir = settings::app_dir().join("recordings");
        let name = format!("test-{}.wav", unix_stamp());
        let path = dir.join(name);
        let src = if self.rec_raw {
            device::shared::REC_RAW
        } else {
            device::shared::REC_PROCESSED
        };
        match Recorder::start(self.shared.clone(), &path, src) {
            Ok(r) => {
                self.recorder = Some(r);
                self.rec_msg.clear();
            }
            Err(e) => self.rec_msg = format!("Could not start recording: {e:#}"),
        }
    }
}

fn unix_stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ------------------------------------------------------------------ panels --

impl App {
    fn top_panel(&mut self, ctx: &egui::Context, p: &mut ChainParams, changed: &mut bool) {
        let th = self.theme.clone();
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            // --- row 1: presets + global switches
            ui.horizontal(|ui| {
                ui.label(RichText::new("AT2020USB-X").strong().size(20.0));
                ui.separator();
                ui.label("Preset");
                let mut chosen: Option<String> = None;
                egui::ComboBox::from_id_salt("preset")
                    .width(200.0)
                    .selected_text(self.settings.preset.clone())
                    .show_ui(ui, |ui| {
                        for (n, _) in ChainParams::builtin() {
                            if ui.selectable_label(self.settings.preset == n, n).clicked() {
                                chosen = Some(n.to_string());
                            }
                        }
                        if !self.user_presets.is_empty() {
                            ui.separator();
                        }
                        for n in &self.user_presets {
                            if ui.selectable_label(&self.settings.preset == n, n).clicked() {
                                chosen = Some(n.clone());
                            }
                        }
                    });
                if let Some(n) = chosen {
                    self.apply_preset(&n);
                    *p = self.params;
                }
                // A/B compare: the two slots swap on click.
                let (la, lb) = ("A", "B");
                let on_b = self.ab_on_b;
                if ui
                    .selectable_label(!on_b, RichText::new(la).strong())
                    .on_hover_text("Settings slot A")
                    .clicked()
                    && on_b
                {
                    self.toggle_ab();
                    *p = self.params;
                }
                if ui
                    .selectable_label(on_b, RichText::new(lb).strong())
                    .on_hover_text("Settings slot B: switch to compare two settings instantly")
                    .clicked()
                    && !on_b
                {
                    self.toggle_ab();
                    *p = self.params;
                }
                if self.recorder.is_some() {
                    ui.colored_label(th.danger(), "● REC");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label_col = if p.output.mute {
                        Color32::WHITE
                    } else {
                        th.text.color()
                    };
                    let mut mute_btn =
                        egui::Button::new(RichText::new("Mute output").color(label_col))
                            .selected(p.output.mute);
                    if p.output.mute {
                        mute_btn = mute_btn.fill(th.danger().gamma_multiply(0.8));
                    }
                    if ui.add(mute_btn).clicked() {
                        p.output.mute = !p.output.mute;
                        *changed = true;
                    }
                    let byp = egui::Button::new("Bypass chain").selected(p.bypass);
                    if ui.add(byp).clicked() {
                        p.bypass = !p.bypass;
                        *changed = true;
                    }
                });
            });
            ui.add_space(2.0);
            // --- row 2: routing
            ui.horizontal_wrapped(|ui| {
                let sel_name = self
                    .dev
                    .selected_name
                    .clone()
                    .unwrap_or_else(|| "— none —".into());
                ui.label("Mic");
                let mut pick_in = None;
                egui::ComboBox::from_id_salt("in")
                    .width(250.0)
                    .selected_text(sel_name)
                    .show_ui(ui, |ui| {
                        for d in &self.dev.captures {
                            let label = if d.is_default {
                                format!("{} (default)", d.name)
                            } else {
                                d.name.clone()
                            };
                            if ui
                                .selectable_label(
                                    self.dev.selected_id.as_deref() == Some(&d.id),
                                    label,
                                )
                                .clicked()
                            {
                                pick_in = Some(d.id.clone());
                            }
                        }
                    });
                if let Some(id) = pick_in {
                    self.settings.input_id = Some(id.clone());
                    self.devctl.send(Cmd::Select(id));
                    self.mark_dirty();
                }

                ui.label("→ Output");
                let out_txt = self
                    .settings
                    .output_name
                    .clone()
                    .unwrap_or_else(|| "— choose —".into());
                egui::ComboBox::from_id_salt("out")
                    .width(250.0)
                    .selected_text(out_txt)
                    .show_ui(ui, |ui| {
                        for d in &self.dev.renders {
                            let label = if d.virtual_cable {
                                format!("{}  [virtual cable]", d.name)
                            } else {
                                d.name.clone()
                            };
                            if ui
                                .selectable_label(
                                    self.settings.output_name.as_deref() == Some(&d.name),
                                    label,
                                )
                                .clicked()
                            {
                                self.settings.output_name = Some(d.name.clone());
                                self.dirty_since.get_or_insert(Instant::now());
                            }
                        }
                    });

                ui.separator();
                if ui
                    .checkbox(&mut self.settings.monitor_enabled, "Monitor")
                    .changed()
                {
                    self.mark_dirty();
                }
                ui.add_enabled_ui(self.settings.monitor_enabled, |ui| {
                    let mon_txt = self
                        .settings
                        .monitor_name
                        .clone()
                        .unwrap_or_else(|| "— headphones —".into());
                    egui::ComboBox::from_id_salt("mon")
                        .width(200.0)
                        .selected_text(mon_txt)
                        .show_ui(ui, |ui| {
                            for d in self.dev.renders.iter().filter(|d| !d.virtual_cable) {
                                if ui
                                    .selectable_label(
                                        self.settings.monitor_name.as_deref() == Some(&d.name),
                                        &d.name,
                                    )
                                    .clicked()
                                {
                                    self.settings.monitor_name = Some(d.name.clone());
                                    self.dirty_since.get_or_insert(Instant::now());
                                }
                            }
                        });
                    if ui
                        .add(
                            egui::Slider::new(&mut self.settings.monitor_volume, 0.0..=1.0)
                                .show_value(false)
                                .text("vol"),
                        )
                        .changed()
                    {
                        self.mark_dirty();
                    }
                });
            });
            ui.add_space(2.0);
            // --- row 3: run control + meters
            let style = self.settings.meter_style;
            ui.horizontal(|ui| {
                let (txt, col) = if self.want_running {
                    ("Stop", th.danger().gamma_multiply(0.75))
                } else {
                    ("Start", th.ok().gamma_multiply(0.75))
                };
                if ui
                    .add(
                        egui::Button::new(RichText::new(txt).strong().color(Color32::WHITE))
                            .fill(col)
                            .min_size(Vec2::new(80.0, 26.0)),
                    )
                    .clicked()
                {
                    self.want_running = !self.want_running;
                    self.last_attempt = Instant::now() - Duration::from_secs(10);
                }
                if ui
                    .checkbox(&mut self.settings.auto_start, "Auto-start")
                    .changed()
                {
                    self.mark_dirty();
                }
                ui.separator();
                ui.vertical(|ui| {
                    level_meter(ui, "IN", &self.m_in, Vec2::new(280.0, 12.0), style);
                    level_meter(ui, "OUT", &self.m_out, Vec2::new(280.0, 12.0), style);
                });
                ui.vertical(|ui| {
                    gr_bar(ui, "Comp", self.gr, 24.0, Vec2::new(170.0, 12.0));
                    gr_bar(ui, "De-ess", self.deess_gr, 12.0, Vec2::new(170.0, 12.0));
                });
                ui.vertical(|ui| {
                    let open = self.shared.meters.gate_open.load(Relaxed);
                    let vad_open = self.shared.meters.vad_gate_open.load(Relaxed);
                    let vad = self.shared.meters.vad.load();
                    vad_meter(
                        ui,
                        vad,
                        p.vad_gate.threshold,
                        p.vad_gate.enabled,
                        vad_open,
                        Vec2::new(120.0, 12.0),
                    );
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            if open { th.ok() } else { th.weak() },
                            if !p.gate.enabled {
                                "○ Gate off"
                            } else if open {
                                "● Gate open"
                            } else {
                                "○ Gate closed"
                            },
                        );
                    });
                });
            });
            ui.add_space(4.0);
        });
    }

    fn bottom_panel(&mut self, ctx: &egui::Context) {
        let th = self.theme.clone();
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(&self.status);
                if let Some(e) = &self.engine {
                    ui.separator();
                    ui.label(format!("≈{:.0} ms end-to-end", e.latency_ms()));
                    let u = self.xruns();
                    ui.separator();
                    if u > 0 {
                        ui.colored_label(th.warn(), format!("{u} xruns"));
                    } else {
                        ui.colored_label(th.ok(), "0 xruns");
                    }
                }
                if let Some(m) = &self.dev.message {
                    ui.separator();
                    ui.colored_label(th.danger(), m);
                }
            });
            if self.settings.output_name.is_none() {
                ui.colored_label(
                    th.warn(),
                    "No virtual cable found. See the “Cable setup” tab.",
                );
            }
        });
    }

    fn left_panel(&mut self, ctx: &egui::Context, now: Instant) {
        let th = self.theme.clone();
        egui::SidePanel::left("windows-device").resizable(false).exact_width(290.0).show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading("Microphone (Windows)");
            ui.separator();
            let connected = self.dev.selected_name.is_some();
            let is_at = self.dev.selected_name.as_deref().is_some_and(|n| n.to_lowercase().contains("at2020"));
            match (&self.dev.selected_name, is_at) {
                (Some(n), true) => ui.colored_label(th.ok(), format!("● {n}")),
                (Some(n), false) => ui.colored_label(th.warn(), format!("● {n} (not an AT2020)")),
                _ => ui.colored_label(th.danger(), "○ No microphone connected"),
            };
            ui.add_space(8.0);

            ui.add_enabled_ui(connected && self.dev.has_endpoint, |ui| {
                ui.label("Input volume (Windows endpoint)");
                let shown = if now < self.vol_hold_until { self.ui_vol } else { self.dev.volume };
                let mut v = shown;
                let r = ui.add(egui::Slider::new(&mut v, 0.0..=1.0).custom_formatter(|n, _| format!("{:.0}%", n * 100.0)));
                if r.changed() {
                    self.ui_vol = v;
                    self.devctl.send(Cmd::SetVolume(v));
                }
                if r.changed() || r.dragged() {
                    self.vol_hold_until = now + Duration::from_millis(500);
                }
                ui.label(weak(ui, format!("{:+.1} dB  (range {:.1} … {:+.1} dB)", self.dev.volume_db, self.dev.db_min, self.dev.db_max)));
                ui.add_space(6.0);

                let muted = self.dev.mute;
                let (t, c) = if muted { ("MUTED — click to unmute", th.danger().gamma_multiply(0.8)) } else { ("LIVE — click to mute", th.ok().gamma_multiply(0.8)) };
                if ui.add(egui::Button::new(RichText::new(t).strong().color(Color32::WHITE)).fill(c).min_size(Vec2::new(ui.available_width(), 34.0))).clicked() {
                    self.devctl.send(Cmd::SetMute(!muted));
                }
                ui.add_space(8.0);
                level_meter(ui, "Pk", &self.m_hw, Vec2::new(170.0, 12.0), self.settings.meter_style);
                ui.label(weak(ui, "Raw level as Windows sees it, before this app's chain."));
            });

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("Windows audio enhancements").strong());
            match self.dev.sysfx_disabled {
                Some(d) => {
                    let mut disabled = d;
                    if ui.checkbox(&mut disabled, "Disable enhancements for this mic").changed() {
                        self.devctl.send(Cmd::SetSysFxDisabled(disabled));
                    }
                    ui.label(weak(ui, "Recommended: this app does its own processing. Changing it may require running as administrator."));
                }
                None => {
                    ui.label(weak(ui, "State not exposed by this driver. Check Sound settings → Properties → Advanced."));
                }
            }

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("About hardware controls").strong());
            ui.label(weak(ui, "Anything that exists only as a control on the microphone itself (knobs, switches, touch mute) is not exposed through Windows audio APIs, so it can't be read or changed here."));
        });
    }
}
