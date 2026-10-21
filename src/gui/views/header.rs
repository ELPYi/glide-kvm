//! Top bar: name, overall status, Start/Stop, and tabs. (Locking lives on
//! its shortcut and the tray menu; the Devices tab notes when it's on.)

use eframe::egui::{self, Button, RichText, Ui};

use super::Model;
use crate::config::Role;
use crate::gui::actions::{Action, Actions, Tab};
use crate::gui::theme::{self, Tone};
use crate::gui::widgets::pill;
use crate::status::EngineStatus;

pub fn show(ui: &mut Ui, m: &Model, actions: &mut Actions) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("glide").size(22.0).strong().color(theme::ACCENT));
        if let Some(role) = m.saved.role {
            ui.label(
                RichText::new(match role {
                    Role::Server => "sharing this keyboard & mouse",
                    Role::Client => "controlled by another PC",
                })
                .weak(),
            );
        }
        if m.elevated {
            ui.label(RichText::new("· admin").weak())
                .on_hover_text("Running as administrator: can control Task Manager and other admin windows.");
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if m.saved.role.is_some() && !m.pairing_busy {
                if m.running {
                    if ui.button("Stop").clicked() {
                        actions.push(Action::Stop);
                    }
                } else if ui
                    .add(Button::new(RichText::new("Start").strong().color(egui::Color32::WHITE)).fill(theme::ACCENT))
                    .clicked()
                {
                    actions.push(Action::Start);
                }
            }
            let (text, tone) = overall(m);
            pill(ui, &text, tone);
        });
    });

    if let Some(err) = m.error {
        ui.label(RichText::new(format!("Couldn't start: {err}")).color(theme::color(ui, Tone::Bad)));
    } else if let Some(n) = m.notice {
        ui.label(RichText::new(&n.text).color(theme::color(ui, n.tone)));
    }

    if m.saved.role.is_some() {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            for (tab, name) in [
                (Tab::Devices, "Devices"),
                (Tab::Settings, "Settings"),
                (Tab::Logs, "Logs"),
            ] {
                if ui
                    .selectable_label(m.tab == tab, RichText::new(name).size(15.0))
                    .clicked()
                {
                    actions.push(Action::GoTo(tab));
                }
            }
        });
    }
    ui.add_space(2.0);
}

/// One-line summary for the status pill.
fn overall(m: &Model) -> (String, Tone) {
    match m.status {
        EngineStatus::Stopped if m.error.is_some() => ("Error".into(), Tone::Bad),
        EngineStatus::Stopped => ("Stopped".into(), Tone::Muted),
        EngineStatus::Server(s) => {
            let online = s.peers.iter().filter(|p| p.connected).count();
            match (s.active, online) {
                (Some(i), _) => (format!("On {}", s.peers[i].name), Tone::Active),
                (None, 0) => ("Waiting for devices".into(), Tone::Warn),
                (None, n) => (format!("{n} of {} connected", s.peers.len()), Tone::Good),
            }
        }
        EngineStatus::Client(c) => match (c.connected, c.active) {
            (_, true) => ("In control".into(), Tone::Active),
            (true, false) => ("Connected".into(), Tone::Good),
            (false, _) => ("Waiting for server".into(), Tone::Warn),
        },
    }
}
