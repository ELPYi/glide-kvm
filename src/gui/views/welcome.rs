//! First run: what is this machine?

use eframe::egui::{Button, Color32, RichText, Ui};

use crate::config::Role;
use crate::gui::actions::{Action, Actions};
use crate::gui::theme;
use crate::gui::widgets::{card, hint};

pub fn show(ui: &mut Ui, actions: &mut Actions) {
    ui.add_space(12.0);
    ui.label(RichText::new("Welcome! What is this PC?").size(20.0).strong());
    hint(ui, "You can change this later in Settings.");
    ui.add_space(8.0);
    ui.columns(2, |cols| {
        card(&mut cols[0], |ui| {
            ui.label(RichText::new("It has the keyboard & mouse").strong().size(16.0));
            hint(ui, "Share them with your other PCs. You'll add those PCs next.");
            if ui.add(primary("This one shares")).clicked() {
                actions.push(Action::ChooseRole(Role::Server));
            }
        });
        card(&mut cols[1], |ui| {
            ui.label(RichText::new("It's one of the other PCs").strong().size(16.0));
            hint(
                ui,
                "Be controlled by the PC with the keyboard & mouse. Copy that PC's secret here.",
            );
            if ui.add(primary("This one is controlled")).clicked() {
                actions.push(Action::ChooseRole(Role::Client));
            }
        });
    });
}

fn primary(text: &str) -> Button<'_> {
    Button::new(RichText::new(text).strong().color(Color32::WHITE)).fill(theme::ACCENT)
}
