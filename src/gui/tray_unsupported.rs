//! Stand-in where there's no tray icon yet (Linux, until its backend is
//! written). Without a tray, closing the window quits, as usual.

use anyhow::{bail, Result};
use eframe::egui;

use super::engine_slot::EngineSlot;

pub struct Tray;

pub fn install(_ctx: egui::Context, _window: isize, _engine: EngineSlot) -> Result<Tray> {
    bail!("no tray icon on this OS yet")
}
