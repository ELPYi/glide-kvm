//! Form layout helpers so every settings row looks the same.

use eframe::egui::{Align, Layout, RichText, Ui, Vec2};

const LABEL_WIDTH: f32 = 170.0;

pub fn section_title(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).heading().size(17.0));
}

/// Secondary explanatory text.
pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).weak());
}

/// "Label    [control]" with an optional hint underneath the control.
pub fn field<R>(ui: &mut Ui, label: &str, help: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(LABEL_WIDTH, 24.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(LABEL_WIDTH);
                ui.label(label);
            },
        );
        ui.vertical(|ui| {
            let r = add(ui);
            if let Some(h) = help {
                hint(ui, h);
            }
            r
        })
        .inner
    })
    .inner
}
