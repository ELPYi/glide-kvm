//! "This PC · DESKTOP-PC · 192.168.1.7 [Copy]": the address to type on
//! another PC, with any other addresses on a quieter second line.

use eframe::egui::{RichText, Ui};

use crate::gui::theme;
use crate::gui::views::ThisPc;
use crate::gui::widgets::card;
use crate::net::addr::AddrKind;

pub fn this_pc_card(ui: &mut Ui, pc: &ThisPc) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::eyebrow("This PC"));
            ui.label(RichText::new(&pc.name).color(theme::TEXT_STRONG));
            match pc.addrs.first() {
                Some(main) => {
                    ui.label(RichText::new("·").weak());
                    let ip = main.ip.to_string();
                    ui.label(RichText::new(&ip).monospace().size(17.0).color(theme::TEXT_STRONG))
                        .on_hover_text("Type this on another PC to connect to this one");
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(ip);
                    }
                }
                None => {
                    ui.label(RichText::new("· not on a network").weak());
                }
            }
        });
        let others = pc.addrs.iter().skip(1).collect::<Vec<_>>();
        if !others.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(theme::eyebrow("Also at"));
                for (i, a) in others.into_iter().enumerate() {
                    if i > 0 {
                        ui.label(RichText::new("·").small().weak());
                    }
                    let ip = a.ip.to_string();
                    let label = match a.kind {
                        AddrKind::Tailscale => format!("{ip} (Tailscale)"),
                        _ => ip.clone(),
                    };
                    if ui
                        .link(RichText::new(label).small().monospace())
                        .on_hover_text("Click to copy")
                        .clicked()
                    {
                        ui.ctx().copy_text(ip);
                    }
                }
            });
        }
    });
}
