//! "Devices": who's connected, where they sit (drag to rearrange), how fast
//! the link is, and pairing new PCs.

use eframe::egui::{self, RichText, TextEdit, Ui};

use super::Model;
use crate::gui::actions::{Action, Actions, Tab};
use crate::gui::theme::{self, Tone};
use crate::gui::widgets::{card, card_with, hint, layout_map, section_title, status_tag, this_pc_card, MapDevice};
use crate::status::{ClientStatus, EngineStatus, PairingStatus, PeerStatus, ServerStatus};

/// Form fields on this screen that aren't settings.
#[derive(Default)]
pub struct DevicesUi {
    /// Client: the sharing PC's address to pair with.
    pub server_address: String,
    /// Client: the code shown on the sharing PC.
    pub code: String,
    /// Server: the new PC's address, for the firewall fallback.
    pub poke_address: String,
    /// Server: the device being renamed, and the name so far.
    pub renaming: Option<(usize, String)>,
    /// Server: the device whose removal is being confirmed.
    pub removing: Option<usize>,
}

pub fn show(ui: &mut Ui, m: &Model, local: &mut DevicesUi, actions: &mut Actions) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        this_pc_card(ui, m.this_pc);
        match m.status {
            EngineStatus::Server(s) => server(ui, m, s, local, actions),
            EngineStatus::Client(c) => client(ui, m, c, local, actions),
            EngineStatus::Stopped if m.pairing_busy => {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Pairing…");
                    });
                });
            }
            EngineStatus::Stopped => stopped(ui, m, actions),
        }
    });
}

fn stopped(ui: &mut Ui, m: &Model, actions: &mut Actions) {
    card(ui, |ui| {
        section_title(ui, "glide is stopped");
        if m.error.is_some() {
            hint(ui, "Fix the problem above (usually in Settings), then press Start.");
        } else {
            hint(
                ui,
                "Press Start to begin. To skip this step next time, turn on \"Start sharing automatically\" under Settings, Startup.",
            );
        }
        if ui.button("Open settings").clicked() {
            actions.push(Action::GoTo(Tab::Settings));
        }
    });
}

// ------------------------------------------------------------------ server

fn peer_tone(s: &ServerStatus, i: usize, p: &PeerStatus) -> Tone {
    if s.active == Some(i) {
        Tone::Active
    } else if p.blocked.is_some() {
        Tone::Warn
    } else if p.connected {
        Tone::Good
    } else {
        Tone::Muted
    }
}

fn server(ui: &mut Ui, m: &Model, s: &ServerStatus, local: &mut DevicesUi, actions: &mut Actions) {
    // Pairing first: it's the main thing to do here, so it's never below the fold.
    match &s.pairing {
        Some(p) => pairing_card(ui, m, p, local, actions),
        None if s.peers.len() < 4 && ui.add(theme::primary("+ Pair a device")).clicked() => {
            actions.push(Action::StartPairing { poke: None });
        }
        None => {}
    }

    let devices: Vec<MapDevice> = s
        .peers
        .iter()
        .enumerate()
        .map(|(i, p)| MapDevice {
            name: &p.name,
            side: p.side,
            tone: peer_tone(s, i, p),
        })
        .collect();
    let center_tone = if s.active.is_none() { Tone::Active } else { Tone::Good };
    let moved = ui
        .vertical_centered(|ui| layout_map(ui, "This PC", center_tone, &devices, true))
        .inner;
    if let Some((index, side)) = moved {
        actions.push(Action::MoveDevice { index, side });
    }
    ui.vertical_centered(|ui| {
        if s.peers.is_empty() {
            hint(ui, "No devices yet. Pair one to get started.");
        } else {
            hint(ui, "Drag a device to the side of your screens it's on.");
        }
    });
    if let Some(g) = &s.game {
        hint(
            ui,
            &format!(
                "{} is in focus, so the screen edges won't switch PCs until you leave it.",
                crate::game::display_name(g)
            ),
        );
    }
    if s.locked {
        hint(
            ui,
            &format!(
                "Switching is locked: the cursor stays where it is. Press {} (or use the tray menu) to unlock.",
                m.saved.hotkeys.lock
            ),
        );
    }

    for (i, p) in s.peers.iter().enumerate() {
        device_card(ui, m, s, i, p, local, actions);
    }
}

/// The device's name, or a box to rename it in.
fn name_or_rename(ui: &mut Ui, i: usize, p: &PeerStatus, local: &mut DevicesUi, actions: &mut Actions) {
    let Some((_, name)) = local.renaming.as_mut().filter(|(at, _)| *at == i) else {
        ui.label(
            RichText::new(&p.name)
                .font(theme::heavy(16.0))
                .color(theme::TEXT_STRONG),
        );
        return;
    };
    let edit = ui.add(TextEdit::singleline(name).desired_width(160.0));
    if !edit.has_focus() && !edit.lost_focus() {
        edit.request_focus();
    }
    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if ui.small_button("Save").clicked() || enter {
        actions.push(Action::RenameDevice {
            index: i,
            name: name.clone(),
        });
        local.renaming = None;
    } else if ui.small_button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        local.renaming = None;
    }
}

fn device_card(
    ui: &mut Ui,
    m: &Model,
    s: &ServerStatus,
    i: usize,
    p: &PeerStatus,
    local: &mut DevicesUi,
    actions: &mut Actions,
) {
    let tone = peer_tone(s, i, p);
    card_with(ui, (tone == Tone::Active).then_some(Tone::Active), |ui| {
        ui.horizontal(|ui| {
            name_or_rename(ui, i, p, local, actions);
            let state = match (tone, p.blocked) {
                (Tone::Active, _) => "In control",
                (_, Some(b)) => b.label(),
                (Tone::Good, _) => "Connected",
                _ => "Offline",
            };
            status_tag(ui, state, tone);
            if p.muted {
                ui.add_space(8.0);
                status_tag(ui, "Muted", Tone::Muted);
            } else if p.sound {
                ui.add_space(8.0);
                status_tag(ui, "Sound", Tone::Muted);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Remove").on_hover_text("Forget this PC").clicked() {
                    local.removing = Some(i);
                    local.renaming = None;
                }
                if ui.small_button("Rename").clicked() {
                    local.renaming = Some((i, p.name.clone()));
                    local.removing = None;
                }
                ui.add_space(6.0);
                // Only offered while its sound would play here at all.
                if m.saved.audio.play {
                    let (label, tip) = if p.muted {
                        ("Unmute", "Play this PC's sound here again")
                    } else {
                        (
                            "Mute",
                            "Silence this PC's sound. It stays silent; its own speakers stay muted too.",
                        )
                    };
                    if ui.small_button(label).on_hover_text(tip).clicked() {
                        actions.push(Action::SetDeviceMuted {
                            index: i,
                            muted: !p.muted,
                        });
                    }
                    ui.add_space(6.0);
                }
                latency(ui, p);
            });
        });
        if local.removing == Some(i) {
            ui.horizontal(|ui| {
                ui.label(format!("Remove {}? To use it again, pair it again.", p.name));
                if ui.button("Remove").clicked() {
                    actions.push(Action::RemoveDevice { index: i });
                    local.removing = None;
                }
                if ui.button("Cancel").clicked() {
                    local.removing = None;
                }
            });
        }
        if p.rtt_ms.is_some_and(|ms| ms > SLOW_LINK_MS) {
            hint(
                ui,
                "This link is Wi-Fi speed. A network cable on either PC is the biggest latency win.",
            );
        }
        let ip = p.resolved.map(|a| a.ip().to_string());
        let at = match &ip {
            Some(ip) if *ip != p.address.split(':').next().unwrap_or("") => format!("{} ({})", p.address, ip),
            Some(_) => p.address.clone(),
            None => format!("{} (not found yet)", p.address),
        };
        hint(ui, &format!("{} · {}", p.side.label(), at));
        if let Some(b) = p.blocked {
            hint(ui, b.advice());
        } else if !p.connected {
            hint(
                ui,
                "Is glide open on it? If it was set up by hand, check it uses this PC's address and the same secret.",
            );
        }
    });
}

/// Above this typical round trip, suggest a cable.
const SLOW_LINK_MS: f32 = 2.0;

/// "5.8 ms" plus "spikes 13 ms" when the worst case is much worse.
fn latency(ui: &mut Ui, p: &PeerStatus) {
    let Some(typical) = p.rtt_ms else { return };
    let worst = p.rtt_worst_ms.unwrap_or(typical);
    let tip = "Round-trip time on the network, typical and worst recent.
               Under 1 ms on a cable; Wi-Fi adds a few ms plus occasional spikes.";
    if worst > typical * 1.5 + 1.0 {
        ui.label(
            RichText::new(format!("spikes {worst:.0} ms"))
                .small()
                .color(theme::color(ui, Tone::Warn)),
        )
        .on_hover_text(tip);
    }
    ui.label(RichText::new(format!("{typical:.1} ms")).monospace())
        .on_hover_text(tip);
}

fn pairing_card(ui: &mut Ui, m: &Model, p: &PairingStatus, local: &mut DevicesUi, actions: &mut Actions) {
    card_with(ui, Some(Tone::Active), |ui| {
        ui.horizontal(|ui| {
            section_title(ui, if p.done { "Paired!" } else { "Pair a new PC" });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(if p.done { "Done" } else { "Cancel" }).clicked() {
                    actions.push(Action::CancelPairing);
                }
            });
        });
        if p.done {
            hint(
                ui,
                "It's been added below. Drag it on the map to the side of your screens it's on.",
            );
            return;
        }
        hint(
            ui,
            "On the other PC, open glide, choose \"This one is controlled\", and enter:",
        );
        ui.add_space(4.0);
        egui::Grid::new("pair_grid")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.label(theme::eyebrow("This PC's address"));
                let addr = m.this_pc.main_address().unwrap_or_else(|| "?".into());
                ui.label(RichText::new(addr).monospace().size(20.0).color(theme::TEXT_STRONG));
                ui.end_row();
                ui.label(theme::eyebrow("Code"));
                ui.label(RichText::new(&p.code).monospace().size(34.0).color(theme::ACCENT));
                ui.end_row();
            });
        let secs = p.expires_in.as_secs();
        let status = if p.checking {
            "Someone's entering the code…".to_string()
        } else if p.failed_attempts > 0 {
            let tries = if p.failed_attempts == 1 { "try" } else { "tries" };
            format!(
                "{} wrong {tries}. Code changes in {}:{:02}",
                p.failed_attempts,
                secs / 60,
                secs % 60
            )
        } else {
            format!("Code changes in {}:{:02}", secs / 60, secs % 60)
        };
        hint(ui, &status);
        egui::CollapsingHeader::new("Not connecting?").id_salt("pair_help").show(ui, |ui| {
            hint(
                ui,
                "Some firewalls block a first contact. Enter the other PC's address (glide shows it at the top) so this PC reaches out to it too:",
            );
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut local.poke_address).hint_text("e.g. 192.168.1.50").desired_width(180.0));
                if ui.button("Reach out").clicked() {
                    actions.push(Action::StartPairing { poke: Some(local.poke_address.trim().to_string()) });
                }
            });
            if cfg!(windows) {
                hint(ui, "Still stuck? Settings, Advanced: Allow glide through the firewall.");
            }
        });
    });
}

// ------------------------------------------------------------------ client

fn client(ui: &mut Ui, m: &Model, c: &ClientStatus, local: &mut DevicesUi, actions: &mut Actions) {
    let tone = match (c.connected, c.active) {
        (_, true) => Tone::Active,
        (true, _) => Tone::Good,
        _ => Tone::Warn,
    };
    card_with(ui, (tone == Tone::Active).then_some(Tone::Active), |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Sharing PC")
                    .font(theme::heavy(16.0))
                    .color(theme::TEXT_STRONG),
            );
            let label = match tone {
                Tone::Active => "You're on this PC",
                Tone::Good => "Connected",
                _ => "Not connected",
            };
            status_tag(ui, label, tone);
            if c.sending_sound {
                ui.add_space(8.0);
                status_tag(ui, "Sound", Tone::Muted);
            }
        });
        match (c.server, m.saved.server_address.as_deref()) {
            (Some(addr), _) if c.connected => hint(ui, &format!("Connected to {}", addr.ip())),
            (_, Some(a)) if !a.trim().is_empty() => hint(ui, &format!("Looking for {a}…")),
            _ => hint(ui, "Not paired yet."),
        }
        if let Some(g) = &c.game {
            hint(
                ui,
                &format!(
                    "{} is in focus, so the edge back won't switch PCs until you leave it.",
                    crate::game::display_name(g)
                ),
            );
        }
    });

    if c.connected {
        egui::CollapsingHeader::new("Pair again")
            .id_salt("client_pair")
            .show(ui, |ui| pair_form(ui, m, local, actions));
    } else {
        card(ui, |ui| {
            section_title(ui, "Pair with the sharing PC");
            pair_form(ui, m, local, actions);
        });
    }
}

fn pair_form(ui: &mut Ui, m: &Model, local: &mut DevicesUi, actions: &mut Actions) {
    hint(
        ui,
        "On the PC with the keyboard & mouse, click \"+ Pair a device\". Then type what it shows:",
    );
    if local.server_address.is_empty() {
        local.server_address = m.saved.server_address.clone().unwrap_or_default();
    }
    egui::Grid::new("client_pair_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Its address");
            ui.add(
                TextEdit::singleline(&mut local.server_address)
                    .hint_text("e.g. 192.168.1.20")
                    .desired_width(200.0),
            );
            ui.end_row();
            ui.label("Code");
            ui.add(
                TextEdit::singleline(&mut local.code)
                    .hint_text("123 456")
                    .desired_width(120.0)
                    .font(egui::TextStyle::Monospace),
            );
            ui.end_row();
        });
    let ready = !local.server_address.trim().is_empty() && crate::net::pairing::normalize_code(&local.code).len() == 6;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(ready && !m.pairing_busy, theme::primary("Pair"))
            .clicked()
        {
            actions.push(Action::PairWithServer {
                address: local.server_address.trim().to_string(),
                code: std::mem::take(&mut local.code),
            });
        }
        if m.pairing_busy {
            ui.spinner();
            ui.label("Pairing…");
        }
    });
}
