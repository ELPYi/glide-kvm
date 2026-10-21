//! "Logs": what glide has been doing, newest at the bottom.

use eframe::egui::{RichText, ScrollArea, Ui};

use crate::gui::actions::{Action, Actions};
use crate::logging;

pub fn show(ui: &mut Ui, actions: &mut Actions) {
    let lines = logging::recent();
    ui.horizontal(|ui| {
        if ui.button("Copy all").clicked() {
            let text: String = lines.iter().map(|l| format!("[{}] {}\n", l.time, l.text)).collect();
            ui.ctx().copy_text(text);
        }
        if ui.button("Clear").clicked() {
            actions.push(Action::ClearLogs);
        }
    });
    ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if lines.is_empty() {
                ui.label(RichText::new("Nothing yet.").weak());
            }
            for l in &lines {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&l.time).monospace().weak());
                    ui.label(RichText::new(&l.text).monospace());
                });
            }
        });
}
