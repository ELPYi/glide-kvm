//! The desktop app (egui / eframe).
//!
//! Structure, in the spirit of components:
//!
//! - [`app`]: the one stateful component. Owns the config, the running
//!   [`Engine`](crate::engine::Engine), and applies [`actions::Action`]s.
//! - [`views`]: one module per screen region (header, devices, settings,
//!   logs, welcome). Views *draw* and *emit actions*; they never start
//!   engines or write files themselves, which keeps them simple and swappable.
//! - [`widgets`]: small reusable pieces (card, pill, layout map, form rows).
//! - [`theme`]: colors, spacing and the app icon in one place.
//! - [`tray`]: the tray icon; closing the window hides glide there.
//! - [`engine_slot`]: the running engine, shared by the window and the tray.
//!
//! egui is "immediate mode": every frame the UI is rebuilt from current
//! state by plain function calls, so there are no callbacks or bindings to
//! keep in sync. Frames are redrawn on input and every 250 ms for live status.

mod actions;
mod app;
mod engine_slot;
#[cfg(test)]
mod font_check;
mod theme;
#[cfg(any(windows, target_os = "macos"))]
mod tray;
#[cfg(not(any(windows, target_os = "macos")))]
#[path = "tray_unsupported.rs"]
mod tray;
mod views;
mod widgets;

use std::path::PathBuf;

pub use actions::Tab;

/// Tall enough to show the whole Devices page (pairing included) without
/// scrolling, but never taller than the screen.
fn window_height() -> f32 {
    const WANTED: f32 = 880.0;
    match crate::platform::system::work_area_height() {
        Some(h) => WANTED.min(h - 40.0).max(480.0),
        None => WANTED,
    }
}

/// Open the app window. `minimized` starts it hidden in the tray (used at sign-in).
pub fn run(config_path: PathBuf, minimized: bool) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("glide")
            .with_inner_size([780.0, window_height()])
            .with_min_inner_size([560.0, 420.0])
            .with_icon(theme::app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "glide",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, config_path, minimized)))),
    )
}
