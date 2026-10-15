//! Small custom egui widgets: sections, sliders, level meters, EQ graph, compressor curve.

use crate::dsp::{comp_curve_db, eq_response_db, lin_to_db};
use crate::params::*;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use std::ops::RangeInclusive;

pub const GREEN: Color32 = Color32::from_rgb(70, 190, 110);
pub const YELLOW: Color32 = Color32::from_rgb(225, 190, 60);
pub const RED: Color32 = Color32::from_rgb(235, 80, 70);
pub const ACCENT: Color32 = Color32::from_rgb(90, 170, 255);
const GRID: Color32 = Color32::from_gray(48);
const BG: Color32 = Color32::from_gray(22);

/// Titled group with an optional enable checkbox. `add` returns whether anything changed.
pub fn section(
    ui: &mut Ui,
    title: &str,
    enabled: Option<&mut bool>,
    add: impl FnOnce(&mut Ui) -> bool,
) -> bool {
    let mut changed = false;
    let mut on = true;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if let Some(e) = enabled {
                changed |= ui.checkbox(e, "").changed();
                on = *e;
            }
            ui.label(RichText::new(title).strong().size(15.0));
        });
        ui.add_enabled_ui(on, |ui| {
            changed |= add(ui);
        });
    });
    changed
}

pub fn row_slider(
    ui: &mut Ui,
    label: &str,
    v: &mut f32,
    r: RangeInclusive<f32>,
    suffix: &str,
    log: bool,
) -> bool {
    ui.horizontal(|ui| {
        ui.add_sized([96.0, 20.0], egui::Label::new(label));
        ui.add(
            egui::Slider::new(v, r)
                .suffix(suffix)
                .logarithmic(log)
                .fixed_decimals(if log { 0 } else { 1 }),
        )
        .changed()
    })
    .inner
}

// ---------------------------------------------------------------- meters ---

#[derive(Clone, Copy)]
pub struct MeterView {
    pub level_db: f32,
    pub hold_db: f32,
    hold_t: f32,
}

impl MeterView {
    pub fn new() -> Self {
        Self {
            level_db: -90.0,
            hold_db: -90.0,
            hold_t: 0.0,
        }
    }
    pub fn update(&mut self, peak_lin: f32, dt: f32) {
        let db = lin_to_db(peak_lin).max(-90.0);
        self.level_db = if db > self.level_db {
            db
        } else {
            (self.level_db - 55.0 * dt).max(db)
        };
        if db >= self.hold_db {
            self.hold_db = db;
            self.hold_t = 1.5;
        } else {
            self.hold_t -= dt;
            if self.hold_t <= 0.0 {
                self.hold_db = (self.hold_db - 25.0 * dt).max(self.level_db);
            }
        }
    }
}

pub fn level_bar(ui: &mut Ui, label: &str, m: &MeterView, size: Vec2) {
    ui.horizontal(|ui| {
        ui.add_sized([34.0, size.y], egui::Label::new(label));
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 2.0, BG);
        let x_of = |db: f32| rect.left() + rect.width() * ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        for (a, b, c) in [
            (-60.0f32, -18.0f32, GREEN),
            (-18.0, -6.0, YELLOW),
            (-6.0, 0.0, RED),
        ] {
            let (x0, x1) = (x_of(a), x_of(b.min(m.level_db)));
            if x1 > x0 {
                p.rect_filled(
                    Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x1, rect.bottom())),
                    0.0,
                    c,
                );
            }
        }
        for db in [-48.0f32, -36.0, -24.0, -12.0] {
            p.line_segment(
                [
                    Pos2::new(x_of(db), rect.top()),
                    Pos2::new(x_of(db), rect.bottom()),
                ],
                Stroke::new(1.0_f32, Color32::from_black_alpha(90)),
            );
        }
        let hx = x_of(m.hold_db);
        p.line_segment(
            [Pos2::new(hx, rect.top()), Pos2::new(hx, rect.bottom())],
            Stroke::new(2.0_f32, Color32::WHITE),
        );
        let txt = if m.hold_db <= -89.0 {
            "-inf".to_string()
        } else {
            format!("{:.0} dB", m.hold_db)
        };
        ui.label(RichText::new(txt).monospace().size(11.0));
    });
}

pub fn gr_bar(ui: &mut Ui, label: &str, gr_db: f32, size: Vec2) {
    ui.horizontal(|ui| {
        ui.add_sized([34.0, size.y], egui::Label::new(label));
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 2.0, BG);
        let w = rect.width() * (gr_db / 24.0).clamp(0.0, 1.0);
        p.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(w, rect.height())),
            0.0,
            Color32::from_rgb(240, 150, 50),
        );
        ui.label(
            RichText::new(format!("-{gr_db:.1} dB"))
                .monospace()
                .size(11.0),
        );
    });
}

// -------------------------------------------------------------- EQ graph ---

const EQ_RANGE: f32 = 18.0;

/// Interactive curve. Drag a handle to set frequency/gain; scroll over it to change Q.
pub fn eq_graph(ui: &mut Ui, bands: &mut [EqBand; EQ_BANDS], size: Vec2) -> bool {
    let mut changed = false;
    let eq_on = ui.is_enabled();
    let (resp, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 4.0, BG);
    let span = 1000.0f32.ln();
    let fx = |f: f32| rect.left() + rect.width() * (f.max(20.0) / 20.0).ln() / span;
    let xf = |x: f32| 20.0 * ((x - rect.left()) / rect.width() * span).exp();
    let gy = |g: f32| rect.center().y - g / EQ_RANGE * rect.height() / 2.0;
    let yg = |y: f32| -(y - rect.center().y) / (rect.height() / 2.0) * EQ_RANGE;

    for f in [
        50.0f32, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0,
    ] {
        painter.line_segment(
            [
                Pos2::new(fx(f), rect.top()),
                Pos2::new(fx(f), rect.bottom()),
            ],
            Stroke::new(1.0_f32, GRID),
        );
        let t = if f >= 1000.0 {
            format!("{}k", f / 1000.0)
        } else {
            format!("{f}")
        };
        painter.text(
            Pos2::new(fx(f) + 2.0, rect.bottom() - 2.0),
            Align2::LEFT_BOTTOM,
            t,
            FontId::proportional(10.0),
            Color32::from_gray(120),
        );
    }
    for g in [-12.0f32, -6.0, 0.0, 6.0, 12.0] {
        painter.line_segment(
            [
                Pos2::new(rect.left(), gy(g)),
                Pos2::new(rect.right(), gy(g)),
            ],
            Stroke::new(
                1.0_f32,
                if g == 0.0 {
                    Color32::from_gray(80)
                } else {
                    GRID
                },
            ),
        );
        painter.text(
            Pos2::new(rect.left() + 3.0, gy(g) - 1.0),
            Align2::LEFT_BOTTOM,
            format!("{g:+.0}"),
            FontId::proportional(10.0),
            Color32::from_gray(120),
        );
    }

    let curve_color = if eq_on {
        ACCENT
    } else {
        Color32::from_gray(110)
    };
    let n = (rect.width() / 2.0) as usize;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let x = rect.left() + rect.width() * i as f32 / n as f32;
            Pos2::new(
                x,
                gy(eq_response_db(bands, xf(x) as f64).clamp(-EQ_RANGE, EQ_RANGE)),
            )
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(2.0_f32, curve_color)));

    #[allow(clippy::needless_range_loop)] // index also keys the widget id
    for i in 0..EQ_BANDS {
        let b = bands[i];
        let c = Pos2::new(fx(b.freq), gy(b.gain_db.clamp(-EQ_RANGE, EQ_RANGE)));
        let r = ui.interact(
            Rect::from_center_size(c, Vec2::splat(20.0)),
            ui.id().with(("eq-handle", i)),
            Sense::drag(),
        );
        if r.dragged() {
            if let Some(p) = r.interact_pointer_pos() {
                let nb = &mut bands[i];
                nb.freq = xf(p.x.clamp(rect.left(), rect.right())).clamp(20.0, 20_000.0);
                nb.gain_db = yg(p.y.clamp(rect.top(), rect.bottom())).clamp(-EQ_RANGE, EQ_RANGE);
                changed = true;
            }
        }
        if r.hovered() {
            let sc = ui.input(|i| i.smooth_scroll_delta.y);
            if sc != 0.0 {
                let nb = &mut bands[i];
                nb.q = (nb.q * (sc * 0.004).exp()).clamp(0.2, 10.0);
                changed = true;
            }
        }
        let col = if !eq_on || !b.enabled {
            Color32::from_gray(110)
        } else {
            Color32::from_rgb(255, 200, 90)
        };
        painter.circle_filled(c, if r.hovered() || r.dragged() { 8.0 } else { 6.0 }, col);
        painter.text(
            c,
            Align2::CENTER_CENTER,
            format!("{}", i + 1),
            FontId::proportional(9.0),
            Color32::BLACK,
        );
    }
    changed
}

// ------------------------------------------------------ compressor curve ---

pub fn comp_graph(ui: &mut Ui, p: &CompParams, side: f32) {
    let (resp, painter) = ui.allocate_painter(Vec2::splat(side), Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 4.0, BG);
    let x_of = |db: f32| rect.left() + rect.width() * ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let y_of = |db: f32| rect.bottom() - rect.height() * ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    for db in [-48.0f32, -36.0, -24.0, -12.0] {
        painter.line_segment(
            [
                Pos2::new(x_of(db), rect.top()),
                Pos2::new(x_of(db), rect.bottom()),
            ],
            Stroke::new(1.0_f32, GRID),
        );
        painter.line_segment(
            [
                Pos2::new(rect.left(), y_of(db)),
                Pos2::new(rect.right(), y_of(db)),
            ],
            Stroke::new(1.0_f32, GRID),
        );
    }
    painter.line_segment(
        [
            Pos2::new(x_of(-60.0), y_of(-60.0)),
            Pos2::new(x_of(0.0), y_of(0.0)),
        ],
        Stroke::new(1.0_f32, Color32::from_gray(80)),
    );
    let pts: Vec<Pos2> = (0..=120)
        .map(|i| {
            let x = -60.0 + i as f32 * 0.5;
            Pos2::new(x_of(x), y_of(comp_curve_db(x, p)))
        })
        .collect();
    painter.add(egui::Shape::line(
        pts,
        Stroke::new(
            2.0_f32,
            if p.enabled {
                ACCENT
            } else {
                Color32::from_gray(110)
            },
        ),
    ));
    painter.line_segment(
        [
            Pos2::new(x_of(p.threshold_db), rect.top()),
            Pos2::new(x_of(p.threshold_db), rect.bottom()),
        ],
        Stroke::new(1.0_f32, Color32::from_rgb(255, 200, 90)),
    );
    painter.text(
        rect.left_top() + Vec2::new(4.0, 2.0),
        Align2::LEFT_TOP,
        "out dB",
        FontId::proportional(10.0),
        Color32::from_gray(120),
    );
    painter.text(
        rect.right_bottom() - Vec2::new(4.0, 2.0),
        Align2::RIGHT_BOTTOM,
        "in dB",
        FontId::proportional(10.0),
        Color32::from_gray(120),
    );
}
