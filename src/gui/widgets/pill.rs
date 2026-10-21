use eframe::egui::{FontId, Pos2, Rect, Sense, Ui, Vec2};

use crate::gui::theme::{self, Tone};

const LED: f32 = 7.0;
const GAP: f32 = 6.0;

/// A status tag: a small square "LED" and an uppercase mono label, e.g.
/// "[#] CONNECTED". Drawn as one piece so the LED stays on the left in any
/// layout direction (and so no font needs a dot character).
pub fn status_tag(ui: &mut Ui, text: &str, tone: Tone) {
    let color = theme::tone_color(tone);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_uppercase(), FontId::monospace(11.0), color);
    let size = Vec2::new(LED + GAP + galley.size().x, galley.size().y.max(LED));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let led = Rect::from_center_size(Pos2::new(rect.left() + LED / 2.0, rect.center().y), Vec2::splat(LED));
    ui.painter().rect_filled(led, 1.0, color);
    let text_pos = Pos2::new(led.right() + GAP, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(text_pos, galley, color);
}
