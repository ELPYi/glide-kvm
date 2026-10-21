use eframe::egui::{CornerRadius, Frame, Margin, Pos2, RichText, Ui};

use crate::gui::theme::{self, Tone};

/// A small rounded status label with a colored dot, e.g. "(•) Connected".
pub fn pill(ui: &mut Ui, text: &str, tone: Tone) {
    let color = theme::color(ui, tone);
    // A wide left margin leaves room for the dot, and a single label keeps the
    // pill sized to its text whatever the surrounding row's direction.
    let frame = Frame::new()
        .fill(theme::tint(ui, tone))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin {
            left: 20,
            right: 9,
            top: 3,
            bottom: 3,
        })
        .show(ui, |ui| ui.label(RichText::new(text).small().strong().color(color)));
    // Drawn, not a "●" character: the built-in fonts don't have one.
    let r = frame.response.rect;
    ui.painter()
        .circle_filled(Pos2::new(r.left() + 11.0, r.center().y), 3.5, color);
}
