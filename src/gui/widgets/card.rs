use eframe::egui::{CornerRadius, Frame, Margin, Stroke, Ui};

use crate::gui::theme::{self, Tone};

/// A rounded panel that groups related content.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_with(ui, None, add)
}

/// A card whose border is highlighted in `tone` (e.g. the active device).
pub fn card_with<R>(ui: &mut Ui, highlight: Option<Tone>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let stroke = match highlight {
        Some(t) => Stroke::new(1.5, theme::color(ui, t)),
        None => ui.visuals().widgets.noninteractive.bg_stroke,
    };
    Frame::new()
        .fill(theme::card_fill(ui))
        .stroke(stroke)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}
