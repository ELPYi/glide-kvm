//! Test-only: every non-ASCII character in the UI's source must be drawable
//! by the app's fonts (IBM Plex plus egui's built-ins). System fonts aren't
//! loaded, to save memory, so a stray symbol would render as an empty box.

use eframe::egui::{Context, FontFamily, FontId, RawInput};

/// UI source files, embedded at compile time.
const SOURCES: &[(&str, &str)] = &[
    ("app.rs", include_str!("app.rs")),
    ("tray.rs", include_str!("tray.rs")),
    ("views/devices.rs", include_str!("views/devices.rs")),
    ("views/header.rs", include_str!("views/header.rs")),
    ("views/logs.rs", include_str!("views/logs.rs")),
    ("views/settings.rs", include_str!("views/settings.rs")),
    ("views/welcome.rs", include_str!("views/welcome.rs")),
    ("theme.rs", include_str!("theme.rs")),
    ("widgets/card.rs", include_str!("widgets/card.rs")),
    ("widgets/form.rs", include_str!("widgets/form.rs")),
    ("widgets/layout_map.rs", include_str!("widgets/layout_map.rs")),
    ("widgets/pill.rs", include_str!("widgets/pill.rs")),
    ("widgets/this_pc.rs", include_str!("widgets/this_pc.rs")),
];

#[test]
fn every_ui_character_has_a_glyph() {
    let ctx = Context::default();
    super::theme::apply(&ctx); // the app's fonts (Plex, then the built-ins)
    let _ = ctx.run_ui(RawInput::default(), |_| {}); // load fonts
    let mut missing = Vec::new();
    for (file, src) in SOURCES {
        for line in src.lines().filter(|l| !l.trim_start().starts_with("//")) {
            for ch in line.chars().filter(|c| !c.is_ascii()) {
                for family in [FontFamily::Proportional, FontFamily::Monospace] {
                    let font = FontId::new(14.0, family.clone());
                    if !ctx.fonts_mut(|f| f.has_glyph(&font, ch)) {
                        missing.push(format!("{file}: '{ch}' (U+{:04X}) in {family:?}", ch as u32));
                    }
                }
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "characters the built-in fonts can't draw:\n{}",
        missing.join("\n")
    );
}
