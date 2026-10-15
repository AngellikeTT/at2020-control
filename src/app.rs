//! The egui application: device control, preset handling, processing-chain editor, meters.

use crate::devctl::{Cmd, DevCtl, DevState};
use crate::dsp::comp_auto_makeup_db;
use crate::engine::Engine;
use crate::params::*;
use crate::settings::{self, Settings};
use crate::shared::Shared;
use crate::widgets::*;
use eframe::egui::{self, Color32, RichText, Vec2};
use std::sync::Arc;
use std::time::{Duration, Instant};

type RunCfg = (String, String, Option<String>);

pub struct App {
    devctl: DevCtl,
    shared: Arc<Shared>,
    engine: Option<Engine>,
    running_cfg: Option<RunCfg>,
    want_running: bool,
    last_attempt: Instant,
    status: String,

    settings: Settings,
    params: ChainParams,
    user_presets: Vec<String>,
    new_preset_name: String,

    dev: DevState,
    seen_rev: u64,
    ui_vol: f32,
    vol_hold_until: Instant,

    m_in: MeterView,
    m_out: MeterView,
    m_hw: MeterView,
    gr: f32,
    last_frame: Instant,
    dirty_since: Option<Instant>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut v = egui::Visuals::dark();
        v.selection.bg_fill = Color32::from_rgb(40, 100, 170);
        cc.egui_ctx.set_visuals(v);
        cc.egui_ctx.style_mut(|s| s.spacing.slider_width = 200.0);

        let settings = Settings::load();
        let params = settings.params;
        let shared = Arc::new(Shared::new(params));
        shared.monitor_vol.store(settings.monitor_volume);
        let devctl = DevCtl::spawn(cc.egui_ctx.clone());
        if let Some(id) = settings.input_id.clone() {
            devctl.send(Cmd::Select(id));
        }
        let now = Instant::now();
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
            settings,
            params,
            dev: DevState::default(),
            seen_rev: 0,
            ui_vol: 0.0,
            vol_hold_until: now,
            m_in: MeterView::new(),
            m_out: MeterView::new(),
            m_hw: MeterView::new(),
            gr: 0.0,
            last_frame: now,
            dirty_since: None,
        }
    }

    fn mark_dirty(&mut self) {
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    fn on_devices_changed(&mut self) {
        if self.settings.input_id.is_none() {
            self.settings.input_id = self.dev.selected_id.clone();
            self.mark_dirty();
        }
        if self.settings.output_name.is_none() {
            if let Some(c) = self.dev.renders.iter().find(|d| d.virtual_cable) {
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
        Some((inp, out, mon))
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
                Some((i, o, m)) => match Engine::start(&i, &o, m.as_deref(), self.shared.clone()) {
                    Ok(e) => {
                        let inf = e.info.clone();
                        self.status = format!(
                            "Running — in {} Hz/{}ch → out {} Hz/{}ch",
                            inf.in_rate, inf.in_channels, inf.out_rate, inf.out_channels
                        );
                        self.engine = Some(e);
                        self.running_cfg = Some((i, o, m));
                    }
                    Err(e) => self.status = format!("Could not start audio: {e:#}"),
                },
            }
        }
        if !self.want_running && self.engine.is_none() {
            self.status = "Stopped".into();
        }
    }

    fn apply_preset(&mut self, name: &str) {
        let p = ChainParams::builtin()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| p)
            .or_else(|| settings::load_user_preset(name));
        if let Some(p) = p {
            self.params = p;
            self.settings.preset = name.to_string();
            self.shared.params.set(p);
            self.settings.params = p;
            self.mark_dirty();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.2);
        self.last_frame = now;

        self.dev = self.devctl.snapshot();
        if self.dev.revision != self.seen_rev {
            self.seen_rev = self.dev.revision;
            self.on_devices_changed();
        }
        if let Some(f) = self.shared.take_fault() {
            self.engine = None;
            self.running_cfg = None;
            self.status = format!("{f} — retrying…");
            self.last_attempt = now;
        }
        self.supervise(now);

        self.shared.monitor_vol.store(self.settings.monitor_volume);
        self.m_in.update(self.shared.meters.in_peak.take(), dt);
        self.m_out.update(self.shared.meters.out_peak.take(), dt);
        self.m_hw.update(self.dev.peak, dt);
        let gr_now = self.shared.meters.gr_db.take();
        self.gr = if gr_now > self.gr {
            gr_now
        } else {
            (self.gr - 40.0 * dt).max(gr_now)
        };

        let mut p = self.params;
        let mut changed = false;

        self.top_panel(ctx, &mut p, &mut changed);
        self.bottom_panel(ctx);
        self.left_panel(ctx, now);
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    changed |= self.chain_ui(ui, &mut p);
                });
        });

        if changed {
            self.params = p;
            self.settings.params = p;
            self.shared.params.set(p);
            self.mark_dirty();
        }
        if self
            .dirty_since
            .is_some_and(|t| now.duration_since(t) > Duration::from_millis(800))
        {
            self.settings.save();
            self.dirty_since = None;
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.settings.params = self.params;
        self.settings.save();
        self.engine = None;
    }
}

// ------------------------------------------------------------------ panels --

impl App {
    fn top_panel(&mut self, ctx: &egui::Context, p: &mut ChainParams, changed: &mut bool) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            // --- row 1: presets + global switches
            ui.horizontal(|ui| {
                ui.label(RichText::new("AT2020USB-X").strong().size(20.0));
                ui.separator();
                ui.label("Preset");
                let mut chosen: Option<String> = None;
                egui::ComboBox::from_id_salt("preset")
                    .width(210.0)
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
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_preset_name)
                        .hint_text("preset name")
                        .desired_width(130.0),
                );
                if ui
                    .add_enabled(
                        !self.new_preset_name.trim().is_empty(),
                        egui::Button::new("Save as"),
                    )
                    .clicked()
                {
                    let name = self.new_preset_name.trim().to_string();
                    match settings::save_user_preset(&name, p) {
                        Ok(()) => {
                            self.user_presets = settings::list_user_presets();
                            self.settings.preset = name;
                            self.new_preset_name.clear();
                        }
                        Err(e) => self.status = format!("Could not save preset: {e}"),
                    }
                }
                if self.user_presets.contains(&self.settings.preset)
                    && ui.button("Delete").clicked()
                {
                    settings::delete_user_preset(&self.settings.preset);
                    self.user_presets = settings::list_user_presets();
                    self.settings.preset = "Voice (Discord / calls)".into();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mute_col = if p.output.mute { RED } else { Color32::GRAY };
                    *changed |= ui
                        .toggle_value(
                            &mut p.output.mute,
                            RichText::new("Mute output").color(mute_col),
                        )
                        .changed();
                    *changed |= ui.toggle_value(&mut p.bypass, "Bypass chain").changed();
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
            ui.horizontal(|ui| {
                let (txt, col) = if self.want_running {
                    ("Stop", Color32::from_rgb(150, 50, 50))
                } else {
                    ("Start", Color32::from_rgb(40, 120, 70))
                };
                if ui
                    .add(
                        egui::Button::new(RichText::new(txt).strong())
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
                    level_bar(ui, "IN", &self.m_in, Vec2::new(300.0, 12.0));
                    level_bar(ui, "OUT", &self.m_out, Vec2::new(300.0, 12.0));
                });
                ui.vertical(|ui| {
                    gr_bar(ui, "GR", self.gr, Vec2::new(200.0, 12.0));
                    let open = self
                        .shared
                        .meters
                        .gate_open
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let vad = self.shared.meters.vad.load();
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            if open { GREEN } else { Color32::from_gray(120) },
                            if open {
                                "● Gate open"
                            } else {
                                "○ Gate closed"
                            },
                        );
                        ui.label(RichText::new(format!("Voice {:>3.0}%", vad * 100.0)).monospace());
                    });
                });
            });
            ui.add_space(4.0);
        });
    }

    fn bottom_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(&self.status);
                if let Some(e) = &self.engine {
                    ui.separator();
                    ui.label(format!("≈{:.0} ms + device buffers", e.latency_ms()));
                    let u = self.shared.underruns.load(std::sync::atomic::Ordering::Relaxed);
                    if u > 0 {
                        ui.separator();
                        ui.colored_label(YELLOW, format!("{u} output underruns"));
                    }
                }
                if let Some(m) = &self.dev.message {
                    ui.separator();
                    ui.colored_label(RED, m);
                }
            });
            if self.settings.output_name.is_none() {
                ui.colored_label(YELLOW, "No virtual cable found. Install VB-CABLE (or similar), pick \"CABLE Input\" as Output above, then choose \"CABLE Output\" as the microphone in Discord / OBS / your game.");
            }
        });
    }

    fn left_panel(&mut self, ctx: &egui::Context, now: Instant) {
        egui::SidePanel::left("windows-device").resizable(false).exact_width(290.0).show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading("Microphone (Windows)");
            ui.separator();
            let connected = self.dev.selected_name.is_some();
            let is_at = self.dev.selected_name.as_deref().is_some_and(|n| n.to_lowercase().contains("at2020"));
            match (&self.dev.selected_name, is_at) {
                (Some(n), true) => ui.colored_label(GREEN, format!("● {n}")),
                (Some(n), false) => ui.colored_label(YELLOW, format!("● {n} (not an AT2020)")),
                _ => ui.colored_label(RED, "○ No microphone connected"),
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
                ui.label(RichText::new(format!("{:+.1} dB  (range {:.1} … {:+.1} dB)", self.dev.volume_db, self.dev.db_min, self.dev.db_max)).small().weak());
                ui.add_space(6.0);

                let muted = self.dev.mute;
                let (t, c) = if muted { ("MUTED — click to unmute", Color32::from_rgb(170, 50, 50)) } else { ("LIVE — click to mute", Color32::from_rgb(40, 110, 70)) };
                if ui.add(egui::Button::new(RichText::new(t).strong()).fill(c).min_size(Vec2::new(ui.available_width(), 34.0))).clicked() {
                    self.devctl.send(Cmd::SetMute(!muted));
                }
                ui.add_space(8.0);
                level_bar(ui, "Pk", &self.m_hw, Vec2::new(190.0, 12.0));
                ui.label(RichText::new("Raw level as Windows sees it, before this app's chain.").small().weak());
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
                    ui.label(RichText::new("Recommended: this app does its own processing. Changing it may require running as administrator.").small().weak());
                }
                None => {
                    ui.label(RichText::new("State not exposed by this driver. Check Sound settings → Properties → Advanced.").small().weak());
                }
            }

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("About hardware controls").strong());
            ui.label(RichText::new("Anything that exists only as a control on the microphone itself (knobs, switches, touch mute) is not exposed through Windows audio APIs, so it can't be read or changed here.").small().weak());
            ui.add_space(8.0);
            ui.label(RichText::new("Other apps").strong());
            ui.label(RichText::new("Windows can't create a microphone without a driver. This app plays the processed audio into a virtual cable's playback side; in Discord / OBS / games choose the cable's recording side (e.g. \"CABLE Output\") as the microphone.").small().weak());
        });
    }

    fn chain_ui(&mut self, ui: &mut egui::Ui, p: &mut ChainParams) -> bool {
        let mut c = false;
        ui.spacing_mut().slider_width = 170.0;

        // ---- EQ (full width)
        c |= section(ui, "Equalizer", Some(&mut p.eq.enabled), |ui| {
            let mut c = eq_graph(ui, &mut p.eq.bands, Vec2::new(ui.available_width(), 190.0));
            ui.label(
                RichText::new(
                    "Drag points to set frequency & gain; scroll over a point to change Q.",
                )
                .small()
                .weak(),
            );
            ui.horizontal_wrapped(|ui| {
                for (i, b) in p.eq.bands.iter_mut().enumerate() {
                    ui.group(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                c |= ui.checkbox(&mut b.enabled, "").changed();
                                let kind = match b.kind {
                                    BandKind::LowShelf => "Low shelf",
                                    BandKind::Peak => "Peak",
                                    BandKind::HighShelf => "High shelf",
                                };
                                ui.label(RichText::new(format!("{}  {}", i + 1, kind)).strong());
                            });
                            c |= ui
                                .add(
                                    egui::DragValue::new(&mut b.freq)
                                        .speed(4.0)
                                        .range(20.0..=20_000.0)
                                        .suffix(" Hz"),
                                )
                                .changed();
                            c |= ui
                                .add(
                                    egui::DragValue::new(&mut b.gain_db)
                                        .speed(0.1)
                                        .range(-18.0..=18.0)
                                        .suffix(" dB")
                                        .fixed_decimals(1),
                                )
                                .changed();
                            c |= ui
                                .add(
                                    egui::DragValue::new(&mut b.q)
                                        .speed(0.01)
                                        .range(0.2..=10.0)
                                        .prefix("Q ")
                                        .fixed_decimals(2),
                                )
                                .changed();
                        });
                    });
                }
            });
            c
        });

        // ---- Row of three
        let comp_snap = p.comp; // read-only copy for the curve (the closure below borrows p.comp fields mutably)
        ui.columns(3, |cols| {
            c |= section(&mut cols[0], "Input", None, |ui| {
                let mut c = false;
                ui.horizontal(|ui| {
                    ui.label("Channel");
                    c |= ui
                        .selectable_value(&mut p.input.channel, ChannelMode::Mix, "Mix")
                        .changed();
                    c |= ui
                        .selectable_value(&mut p.input.channel, ChannelMode::Left, "Left")
                        .changed();
                    c |= ui
                        .selectable_value(&mut p.input.channel, ChannelMode::Right, "Right")
                        .changed();
                });
                c |= row_slider(ui, "Gain", &mut p.input.gain_db, -24.0..=24.0, " dB", false);
                c |= ui
                    .checkbox(&mut p.input.phase_invert, "Invert phase")
                    .changed();
                c |= ui
                    .checkbox(&mut p.input.lowcut_on, "Low-cut filter (12 dB/oct)")
                    .changed();
                c |= row_slider(
                    ui,
                    "Cutoff",
                    &mut p.input.lowcut_hz,
                    20.0..=300.0,
                    " Hz",
                    true,
                );
                c
            });
            c |= section(
                &mut cols[0],
                "Noise suppression (RNNoise)",
                Some(&mut p.denoise.enabled),
                |ui| {
                    let c = row_slider(
                        ui,
                        "Strength",
                        &mut p.denoise.strength,
                        0.0..=1.0,
                        "",
                        false,
                    );
                    ui.label(
                        RichText::new("Neural suppressor tuned for voice. Adds ~10 ms.")
                            .small()
                            .weak(),
                    );
                    c
                },
            );

            c |= section(
                &mut cols[1],
                "Noise gate",
                Some(&mut p.gate.enabled),
                |ui| {
                    let mut c = row_slider(
                        ui,
                        "Threshold",
                        &mut p.gate.threshold_db,
                        -80.0..=0.0,
                        " dB",
                        false,
                    );
                    c |= row_slider(
                        ui,
                        "Hysteresis",
                        &mut p.gate.hysteresis_db,
                        0.0..=20.0,
                        " dB",
                        false,
                    );
                    c |= row_slider(
                        ui,
                        "Attack",
                        &mut p.gate.attack_ms,
                        0.1..=50.0,
                        " ms",
                        false,
                    );
                    c |= row_slider(ui, "Hold", &mut p.gate.hold_ms, 0.0..=500.0, " ms", false);
                    c |= row_slider(
                        ui,
                        "Release",
                        &mut p.gate.release_ms,
                        10.0..=1000.0,
                        " ms",
                        false,
                    );
                    c |= row_slider(ui, "Range", &mut p.gate.range_db, -80.0..=0.0, " dB", false);
                    c
                },
            );

            c |= section(
                &mut cols[2],
                "Compressor",
                Some(&mut p.comp.enabled),
                |ui| {
                    let mut c = false;
                    comp_graph(ui, &comp_snap, 120.0);
                    c |= row_slider(
                        ui,
                        "Threshold",
                        &mut p.comp.threshold_db,
                        -60.0..=0.0,
                        " dB",
                        false,
                    );
                    c |= row_slider(ui, "Ratio", &mut p.comp.ratio, 1.0..=20.0, " : 1", false);
                    c |= row_slider(
                        ui,
                        "Attack",
                        &mut p.comp.attack_ms,
                        0.1..=100.0,
                        " ms",
                        false,
                    );
                    c |= row_slider(
                        ui,
                        "Release",
                        &mut p.comp.release_ms,
                        10.0..=1000.0,
                        " ms",
                        false,
                    );
                    c |= row_slider(ui, "Knee", &mut p.comp.knee_db, 0.0..=24.0, " dB", false);
                    c |= ui
                        .checkbox(&mut p.comp.auto_makeup, "Auto make-up")
                        .changed();
                    if p.comp.auto_makeup {
                        ui.label(format!(
                            "Make-up: +{:.1} dB (auto)",
                            comp_auto_makeup_db(&comp_snap)
                        ));
                    } else {
                        c |= row_slider(
                            ui,
                            "Make-up",
                            &mut p.comp.makeup_db,
                            0.0..=24.0,
                            " dB",
                            false,
                        );
                    }
                    c
                },
            );
        });

        // ---- Output / limiter
        ui.columns(2, |cols| {
            c |= section(
                &mut cols[0],
                "Limiter",
                Some(&mut p.limiter.enabled),
                |ui| {
                    let mut c = row_slider(
                        ui,
                        "Ceiling",
                        &mut p.limiter.ceiling_db,
                        -12.0..=0.0,
                        " dB",
                        false,
                    );
                    c |= row_slider(
                        ui,
                        "Release",
                        &mut p.limiter.release_ms,
                        5.0..=500.0,
                        " ms",
                        false,
                    );
                    c
                },
            );
            c |= section(&mut cols[1], "Output", None, |ui| {
                let mut c = row_slider(
                    ui,
                    "Gain",
                    &mut p.output.gain_db,
                    -24.0..=24.0,
                    " dB",
                    false,
                );
                c |= ui.checkbox(&mut p.output.mute, "Mute output").changed();
                c
            });
        });
        c
    }
}
