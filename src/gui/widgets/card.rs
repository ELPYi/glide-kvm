use eframe::egui::{CornerRadius, Frame, Margin, Rect, Stroke, Ui, Vec2};

use crate::gui::theme::{self, Tone};

/// A flat panel with a hairline border that groups related content.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_with(ui, None, add)
}

/// A card marked in `tone` (e.g. the active device): a colored border and a
/// solid bar down its left edge.
pub fn card_with<R>(ui: &mut Ui, highlight: Option<Tone>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let stroke = match highlight {
        Some(t) => Stroke::new(1.0, theme::tone_color(t)),
        None => Stroke::new(1.0, theme::LINE),
    };
    let shown = Frame::new()
        .fill(theme::SURFACE)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    if let Some(t) = highlight {
        let r = shown.response.rect;
        let bar = Rect::from_min_size(r.min, Vec2::new(3.0, r.height()));
        ui.painter().rect_filled(
            bar,
            CornerRadius {
                nw: theme::RADIUS,
                sw: theme::RADIUS,
                ne: 0,
                se: 0,
            },
            theme::tone_color(t),
        );
    }
    shown.inner
}
