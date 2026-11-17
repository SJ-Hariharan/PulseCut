//! Small reusable widgets: stat tiles, badges, status dots, the Hype Map chart, timeline strip.

use crate::theme::*;
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use pulsecut_detect::scorer::{HypePoint, N_SIGNALS, Signal};

pub fn heading(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).size(24.0).strong());
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(MUTED));
    }
    ui.add_space(8.0);
}

pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title.to_uppercase()).size(11.5).color(MUTED).strong());
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.linear_multiply(0.18))
        .stroke(Stroke::new(1.0, color.linear_multiply(0.6)))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(color).strong());
        });
}

pub fn dot(ui: &mut Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.5, color);
}

pub fn status_line(ui: &mut Ui, ok: Option<bool>, label: &str, detail: &str) {
    ui.horizontal(|ui| {
        dot(
            ui,
            match ok {
                Some(true) => OK,
                Some(false) => BAD,
                None => MUTED,
            },
        );
        ui.label(RichText::new(label).strong());
        if !detail.is_empty() {
            ui.label(RichText::new(detail).color(MUTED));
        }
    });
}

pub fn stat(ui: &mut Ui, label: &str, value: String, color: Color32) {
    card().show(ui, |ui| {
        ui.set_min_width(150.0);
        ui.label(RichText::new(label).size(12.0).color(MUTED));
        ui.label(RichText::new(value).size(26.0).strong().color(color));
    });
}

pub fn primary_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).strong().color(Color32::WHITE))
            .fill(ACCENT)
            .min_size(vec2(0.0, 34.0)),
    )
}

pub fn signal_color(i: usize) -> Color32 {
    const C: [Color32; N_SIGNALS] = [
        Color32::from_rgb(96, 165, 250),
        Color32::from_rgb(56, 189, 248),
        Color32::from_rgb(255, 77, 109),
        Color32::from_rgb(244, 114, 182),
        Color32::from_rgb(251, 146, 60),
        Color32::from_rgb(250, 204, 21),
        Color32::from_rgb(139, 92, 246),
        Color32::from_rgb(52, 211, 153),
        Color32::from_rgb(167, 139, 250),
    ];
    C[i % N_SIGNALS]
}

/// Hype Map: stacked signal contributions over time, threshold line, event markers.
pub fn hype_map(ui: &mut Ui, pts: &[HypePoint], events: &[(i64, Color32)], height: f32) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(12, 13, 18));
    p.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, BORDER),
        StrokeKind::Inside,
    );
    if pts.len() < 2 {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Waiting for activity…",
            FontId::proportional(13.0),
            MUTED,
        );
        return;
    }
    let inner = rect.shrink2(vec2(8.0, 10.0));
    let (t0, t1) = (pts[0].sec as f32, pts[pts.len() - 1].sec as f32);
    let span = (t1 - t0).max(1.0);
    let x = |sec: f32| inner.left() + (sec - t0) / span * inner.width();
    let y = |v: f32| inner.bottom() - v.clamp(0.0, 1.0) * inner.height();
    // Stacked contributions as thin bars.
    let bw = (inner.width() / pts.len() as f32).max(1.0);
    for hp in pts {
        let mut acc = 0.0;
        for i in 0..N_SIGNALS {
            let c = hp.comps[i];
            if c <= 0.001 {
                continue;
            }
            let r = Rect::from_min_max(pos2(x(hp.sec as f32), y(acc + c)), pos2(x(hp.sec as f32) + bw, y(acc)));
            p.rect_filled(r, CornerRadius::ZERO, signal_color(i).linear_multiply(0.75));
            acc += c;
        }
    }
    // Score line and threshold.
    let line: Vec<Pos2> = pts.iter().map(|h| pos2(x(h.sec as f32), y(h.score))).collect();
    p.line(line, Stroke::new(1.6, Color32::WHITE));
    let thr: Vec<Pos2> = pts.iter().map(|h| pos2(x(h.sec as f32), y(h.threshold))).collect();
    p.line(thr, Stroke::new(1.0, ACCENT.linear_multiply(0.8)));
    for (sec, c) in events {
        let sx = x(*sec as f32);
        if sx >= inner.left() && sx <= inner.right() {
            p.line_segment([pos2(sx, inner.top()), pos2(sx, inner.bottom())], Stroke::new(2.0, *c));
            p.circle_filled(pos2(sx, inner.top() + 4.0), 4.0, *c);
        }
    }
    p.text(
        pos2(inner.left(), rect.top() + 2.0),
        Align2::LEFT_TOP,
        "score",
        FontId::proportional(10.5),
        MUTED,
    );
    p.text(
        pos2(inner.right(), rect.top() + 2.0),
        Align2::RIGHT_TOP,
        format!("last {:.0} min", span / 60.0),
        FontId::proportional(10.5),
        MUTED,
    );
    if let Some(pos) = resp.hover_pos() {
        let sec = t0 + (pos.x - inner.left()) / inner.width() * span;
        if let Some(h) = pts.iter().min_by_key(|h| ((h.sec as f32 - sec).abs() * 10.0) as i64) {
            p.line_segment(
                [pos2(pos.x, inner.top()), pos2(pos.x, inner.bottom())],
                Stroke::new(1.0, MUTED),
            );
            let top: Vec<String> = (0..N_SIGNALS)
                .filter(|i| h.comps[*i] > 0.01)
                .map(|i| format!("{} {:.2}", Signal::ALL[i].label(), h.comps[i]))
                .collect();
            resp.on_hover_text(format!(
                "{}\nscore {:.2} / threshold {:.2}\n{} msgs · {:.0} dB\n{}",
                pulsecut_core::time::fmt_local(h.sec * 1000),
                h.score,
                h.threshold,
                h.msgs,
                h.loudness_db,
                top.join("\n")
            ));
        }
    }
}

pub fn legend(ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        for (i, s) in Signal::ALL.iter().enumerate() {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(2), signal_color(i));
            ui.label(RichText::new(s.label()).size(11.5).color(MUTED));
        }
    });
}

/// Horizontal bars showing current signal contributions.
pub fn signal_bars(ui: &mut Ui, comps: &[f32; N_SIGNALS]) {
    for (i, s) in Signal::ALL.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.add_sized(vec2(130.0, 18.0), egui::Label::new(RichText::new(s.label()).size(12.0)));
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().min(240.0), 10.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(4), SURFACE_2);
            let mut f = r;
            f.set_width(r.width() * (comps[i] * 3.0).clamp(0.0, 1.0));
            ui.painter().rect_filled(f, CornerRadius::same(4), signal_color(i));
        });
    }
}

/// Clip timeline: trim window, moment marker, effect blocks. Returns a clicked time (ms) if any.
pub fn timeline_strip(
    ui: &mut Ui,
    src_len: i64,
    trim: (i64, i64),
    moment: Option<i64>,
    blocks: &[(i64, i64, Color32, String)],
    playhead: Option<i64>,
) -> Option<i64> {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 54.0), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(12, 13, 18));
    let len = src_len.max(1) as f32;
    let x = |t: i64| rect.left() + (t as f32 / len) * rect.width();
    let tr = Rect::from_min_max(pos2(x(trim.0), rect.top() + 4.0), pos2(x(trim.1), rect.bottom() - 4.0));
    p.rect_filled(tr, CornerRadius::same(6), SURFACE_2);
    p.rect_stroke(tr, CornerRadius::same(6), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    for (k, (a, b, c, label)) in blocks.iter().enumerate() {
        let lane = (k % 2) as f32;
        let r = Rect::from_min_max(
            pos2(x(*a), rect.top() + 8.0 + lane * 20.0),
            pos2(x(*b).max(x(*a) + 4.0), rect.top() + 24.0 + lane * 20.0),
        );
        p.rect_filled(r, CornerRadius::same(4), c.linear_multiply(0.8));
        p.text(
            r.left_center() + vec2(3.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(10.0),
            Color32::WHITE,
        );
    }
    if let Some(m) = moment {
        p.line_segment(
            [pos2(x(m), rect.top()), pos2(x(m), rect.bottom())],
            Stroke::new(2.0, ACCENT),
        );
    }
    if let Some(ph) = playhead {
        p.line_segment(
            [pos2(x(ph), rect.top()), pos2(x(ph), rect.bottom())],
            Stroke::new(1.5, Color32::WHITE),
        );
    }
    resp.clicked()
        .then(|| resp.interact_pointer_pos())
        .flatten()
        .map(|pos| (((pos.x - rect.left()) / rect.width()) * len) as i64)
}

pub fn empty_state(ui: &mut Ui, title: &str, body: &str) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).size(18.0).strong());
        ui.label(RichText::new(body).color(MUTED));
    });
    ui.add_space(30.0);
}

pub fn size_of(img: &egui::TextureHandle, max: Vec2) -> Vec2 {
    let s = img.size_vec2();
    let k = (max.x / s.x).min(max.y / s.y);
    s * k
}
