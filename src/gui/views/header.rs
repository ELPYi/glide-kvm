//! Top bar: name, overall status, Start/Stop, and tabs. (Locking lives on
//! its shortcut and the tray menu; the Devices tab notes when it's on.)

use eframe::egui::{self, RichText, Ui};

use super::Model;
use crate::config::Role;
use crate::gui::actions::{Action, Actions, Tab};
use crate::gui::theme::{self, Tone};
use crate::gui::widgets::status_tag;
use crate::status::EngineStatus;

pub fn show(ui: &mut Ui, m: &Model, actions: &mut Actions) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        wordmark(ui);
        if let Some(role) = m.saved.role {
            ui.add_space(6.0);
            ui.label(theme::eyebrow(match role {
                Role::Server => "sharing this keyboard & mouse",
                Role::Client => "controlled by another PC",
            }));
        }
        if m.elevated {
            ui.label(theme::eyebrow("· admin"))
                .on_hover_text("Running as administrator: can control Task Manager and other admin windows.");
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if m.saved.role.is_none() {
                return; // first run: nothing to start yet
            }
            if !m.pairing_busy {
                if m.running {
                    if ui.button("Stop").clicked() {
                        actions.push(Action::Stop);
                    }
                } else if ui.add(theme::primary("Start")).clicked() {
                    actions.push(Action::Start);
                }
            }
            ui.add_space(4.0);
            let (text, tone) = overall(m);
            status_tag(ui, &text, tone);
        });
    });

    if let Some(err) = m.error {
        ui.label(RichText::new(format!("Couldn't start: {err}")).color(theme::color(ui, Tone::Bad)));
    } else if let Some(n) = m.notice {
        ui.label(RichText::new(&n.text).color(theme::color(ui, n.tone)));
    }

    if m.saved.role.is_some() {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 22.0;
            for (tab, name) in [
                (Tab::Devices, "Devices"),
                (Tab::Settings, "Settings"),
                (Tab::Logs, "Logs"),
            ] {
                if tab_label(ui, name, m.tab == tab) {
                    actions.push(Action::GoTo(tab));
                }
            }
        });
    } else {
        ui.add_space(6.0);
    }
}

/// "glide" plus a lime block, like a text cursor mid-glide.
fn wordmark(ui: &mut Ui) {
    let r = ui.label(
        RichText::new("glide")
            .font(theme::heavy(22.0))
            .color(theme::TEXT_STRONG),
    );
    let rect = r.rect;
    let block = egui::Rect::from_min_size(
        egui::pos2(rect.right() + 3.0, rect.center().y - 6.0),
        egui::vec2(8.0, 14.0),
    );
    ui.painter().rect_filled(block, 1.0, theme::ACCENT);
    ui.add_space(12.0);
}

/// A tab: plain text, with a lime underline when selected.
fn tab_label(ui: &mut Ui, name: &str, selected: bool) -> bool {
    let color = if selected { theme::TEXT_STRONG } else { theme::TEXT_DIM };
    let text = RichText::new(name).font(if selected {
        theme::heavy(15.0)
    } else {
        egui::FontId::proportional(15.0)
    });
    let resp = ui.add(egui::Label::new(text.color(color)).sense(egui::Sense::click()));
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, name));
    let hovered = resp.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if selected || hovered {
        let r = resp.rect;
        let y = r.bottom() + 5.0;
        let c = if selected { theme::ACCENT } else { theme::LINE_STRONG };
        ui.painter().line_segment(
            [egui::pos2(r.left(), y), egui::pos2(r.right(), y)],
            egui::Stroke::new(2.0, c),
        );
    }
    resp.clicked()
}

/// One-line summary for the status tag.
fn overall(m: &Model) -> (String, Tone) {
    match m.status {
        EngineStatus::Stopped if m.error.is_some() => ("Error".into(), Tone::Bad),
        EngineStatus::Stopped => ("Stopped".into(), Tone::Muted),
        EngineStatus::Server(s) => {
            let online = s.peers.iter().filter(|p| p.connected).count();
            match (s.active, online) {
                (Some(i), _) => (format!("On {}", s.peers[i].name), Tone::Active),
                (None, _) if s.game.is_some() => ("In game: staying here".into(), Tone::Warn),
                (None, 0) => ("Waiting for devices".into(), Tone::Warn),
                (None, n) => (format!("{n} of {} connected", s.peers.len()), Tone::Good),
            }
        }
        EngineStatus::Client(c) => match (c.connected, c.active) {
            (_, true) if c.game.is_some() => ("In game: staying here".into(), Tone::Active),
            (_, true) => ("In control".into(), Tone::Active),
            (true, false) => ("Connected".into(), Tone::Good),
            (false, _) => ("Waiting for server".into(), Tone::Warn),
        },
    }
}
