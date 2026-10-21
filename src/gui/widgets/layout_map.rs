//! A 3×3 grid showing where each machine sits around this one (the middle
//! cell). Devices can be dragged to another side; dropping onto an occupied
//! side swaps the two. Corner cells complete the grid but can't hold a
//! device, since the cursor only crosses straight edges.

use eframe::egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::gui::theme::{self, Tone};
use crate::input::Side;

pub struct MapDevice<'a> {
    pub name: &'a str,
    pub side: Side,
    pub tone: Tone,
}

/// Every cell in the grid is this size.
const SCREEN: Vec2 = Vec2::new(120.0, 62.0);
const GAP: f32 = 8.0;
const MAP_HEIGHT: f32 = SCREEN.y * 3.0 + GAP * 2.0 + 8.0;
/// Drop targets reach this far into the gaps, so drops feel forgiving.
const DROP_MARGIN: f32 = GAP;

/// Draws "this machine" in the middle with each device on its side.
/// With `draggable`, returns `(device index, new side)` when a drag ends on
/// a different side.
pub fn layout_map(
    ui: &mut Ui,
    center_label: &str,
    center_tone: Tone,
    devices: &[MapDevice],
    draggable: bool,
) -> Option<(usize, Side)> {
    let size = Vec2::new(ui.available_width().min(520.0), MAP_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let center = Rect::from_center_size(rect.center(), SCREEN);
    let slot = |side: Side| slot_rect(center, side);

    // Interaction first, then painting, so the dragged box draws on top.
    let mut dragging: Option<(usize, Pos2)> = None;
    let mut dropped = None;
    if draggable {
        let base = ui.make_persistent_id("layout_map");
        for (i, d) in devices.iter().enumerate() {
            let resp = ui.interact(slot(d.side), base.with(i), Sense::drag());
            let pointer = ui.input(|inp| inp.pointer.interact_pos());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
            if resp.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                if let Some(p) = pointer {
                    dragging = Some((i, p));
                }
            }
            if resp.drag_stopped() {
                if let Some(target) = pointer.and_then(|p| side_under(center, p)) {
                    if target != d.side {
                        dropped = Some((i, target));
                    }
                }
            }
        }
    }

    let painter = ui.painter_at(rect);
    let hover_target = dragging.and_then(|(_, p)| side_under(center, p));

    // Corners: very faint, just completing the grid.
    let faint = ui.visuals().weak_text_color().gamma_multiply(0.12);
    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let r = cell(center, dx, dy);
        painter.rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, faint), StrokeKind::Inside);
    }
    // Empty sides: faint outlines, highlighted while dragging over them.
    for side in Side::ALL {
        let occupied = devices.iter().any(|d| d.side == side);
        let highlighted = hover_target == Some(side);
        if !occupied || highlighted {
            let color = if highlighted {
                theme::ACCENT
            } else {
                ui.visuals().weak_text_color().gamma_multiply(0.35)
            };
            let stroke = Stroke::new(if highlighted { 2.0 } else { 1.0 }, color);
            painter.rect_stroke(slot(side), CornerRadius::same(8), stroke, StrokeKind::Inside);
        }
    }
    for (i, d) in devices.iter().enumerate() {
        // A device being dragged leaves a faint ghost where it came from.
        let opacity = if dragging.is_some_and(|(j, _)| j == i) {
            0.3
        } else {
            1.0
        };
        screen_box(ui, &painter, slot(d.side), d.name, d.tone, opacity);
    }
    screen_box(ui, &painter, center, center_label, center_tone, 1.0);

    if let Some((i, p)) = dragging {
        let d = &devices[i];
        let r = Rect::from_center_size(p, slot(d.side).size());
        screen_box(ui, ui.painter(), r, d.name, d.tone, 0.9);
    }
    dropped
}

/// Grid cell at column `dx`, row `dy` (each -1, 0 or 1) around `center`.
fn cell(center: Rect, dx: f32, dy: f32) -> Rect {
    let step = SCREEN + Vec2::splat(GAP);
    Rect::from_center_size(center.center() + Vec2::new(dx * step.x, dy * step.y), SCREEN)
}

fn slot_rect(center: Rect, side: Side) -> Rect {
    match side {
        Side::Left => cell(center, -1.0, 0.0),
        Side::Right => cell(center, 1.0, 0.0),
        Side::Top => cell(center, 0.0, -1.0),
        Side::Bottom => cell(center, 0.0, 1.0),
    }
}

/// Which side's drop zone contains `p`, if any.
fn side_under(center: Rect, p: Pos2) -> Option<Side> {
    Side::ALL
        .into_iter()
        .find(|s| slot_rect(center, *s).expand(DROP_MARGIN / 2.0).contains(p))
}

fn screen_box(ui: &Ui, painter: &Painter, r: Rect, label: &str, tone: Tone, opacity: f32) {
    let c = theme::color(ui, tone).gamma_multiply(opacity);
    let fill = if tone == Tone::Active {
        theme::tint(ui, tone)
    } else {
        theme::card_fill(ui)
    };
    painter.rect_filled(r, CornerRadius::same(8), fill.gamma_multiply(opacity));
    painter.rect_stroke(r, CornerRadius::same(8), Stroke::new(1.5, c), StrokeKind::Inside);
    let text: Color32 = ui.visuals().strong_text_color().gamma_multiply(opacity);
    painter.text(
        r.center(),
        Align2::CENTER_CENTER,
        truncate(label, 16),
        FontId::proportional(13.0),
        text,
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use eframe::egui::{Context, Event, PointerButton, RawInput};
    use std::cell::Cell;

    /// Drives the real widget headlessly: press on a device, move, release.
    fn drag(from: Side, to_point: impl Fn(Rect) -> Pos2) -> Option<(usize, Side)> {
        let ctx = Context::default();
        let devices = [MapDevice {
            name: "laptop",
            side: from,
            tone: Tone::Good,
        }];
        let center = Cell::new(Rect::NOTHING);
        let mut time = 0.0;
        let mut frame = |events: Vec<Event>| {
            time += 0.05;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut moved = None;
            let _ = ctx.run_ui(input, |ui| {
                let origin = ui.cursor().min;
                let width = ui.available_width().min(520.0);
                center.set(Rect::from_center_size(
                    origin + Vec2::new(width / 2.0, MAP_HEIGHT / 2.0),
                    SCREEN,
                ));
                moved = layout_map(ui, "This PC", Tone::Active, &devices, true);
            });
            moved
        };
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        frame(vec![]);
        let start = slot_rect(center.get(), from).center();
        let end = to_point(center.get());
        frame(vec![Event::PointerMoved(start)]);
        frame(vec![button(start, true)]);
        for i in 1..=8 {
            frame(vec![Event::PointerMoved(start + (end - start) * (i as f32 / 8.0))]);
        }
        frame(vec![button(end, false)])
    }

    #[test]
    fn dragging_a_device_reports_its_new_side() {
        assert_eq!(
            drag(Side::Right, |c| slot_rect(c, Side::Left).center()),
            Some((0, Side::Left))
        );
        assert_eq!(
            drag(Side::Right, |c| slot_rect(c, Side::Top).center()),
            Some((0, Side::Top))
        );
    }

    #[test]
    fn dropping_nowhere_or_in_place_changes_nothing() {
        assert_eq!(drag(Side::Right, |c| c.center()), None, "dropped on this PC");
        assert_eq!(
            drag(Side::Right, |c| slot_rect(c, Side::Right).center()),
            None,
            "same side"
        );
    }

    #[test]
    fn drop_zones_map_to_sides() {
        let center = Rect::from_center_size(Pos2::new(300.0, 100.0), SCREEN);
        for side in Side::ALL {
            assert_eq!(side_under(center, slot_rect(center, side).center()), Some(side));
        }
        assert_eq!(side_under(center, center.center()), None, "the middle is this PC");
        assert_eq!(side_under(center, Pos2::new(-500.0, -500.0)), None);
        assert_eq!(
            side_under(center, cell(center, 1.0, 1.0).center()),
            None,
            "corners hold nothing"
        );
        for side in Side::ALL {
            assert_eq!(
                slot_rect(center, side).size(),
                center.size(),
                "every cell is the same size"
            );
        }
    }
}
