//! First run: what is this machine?
//!
//! Two same-size cards side by side. Each whole card is one big button: it
//! lights up on hover, and its call to action sits at the same height in both.

use eframe::egui::{
    Align, Align2, Color32, CornerRadius, Layout, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder,
    Vec2, WidgetInfo,
};

use crate::config::Role;
use crate::gui::actions::{Action, Actions};
use crate::gui::theme;

const MAX_WIDTH: f32 = 680.0;
const CARD_HEIGHT: f32 = 270.0;
const GAP: f32 = 16.0;
const PAD: f32 = 20.0;
const BUTTON_HEIGHT: f32 = 40.0;

struct Choice {
    role: Role,
    tag: &'static str,
    title: &'static str,
    body: &'static str,
    button: &'static str,
    art: fn(&Painter, Rect, Color32, Color32),
}

const CHOICES: [Choice; 2] = [
    Choice {
        role: Role::Server,
        tag: "01 / shares",
        title: "It has the keyboard & mouse",
        body: "Share them with your other PCs. You'll pair those next.",
        button: "This one shares",
        art: keyboard_and_mouse,
    },
    Choice {
        role: Role::Client,
        tag: "02 / is controlled",
        title: "It's one of the other PCs",
        body: "The PC with the keyboard & mouse controls this one. You'll type the code it shows.",
        button: "This one is controlled",
        art: screen_with_cursor,
    },
];

pub fn show(ui: &mut Ui, actions: &mut Actions) {
    let area = ui.available_rect_before_wrap();
    let width = area.width().min(MAX_WIDTH);
    let content_height = 120.0 + CARD_HEIGHT;
    let top = area.top() + ((area.height() - content_height) * 0.4).max(8.0);
    let column = Rect::from_min_size(
        Pos2::new(area.center().x - width / 2.0, top),
        Vec2::new(width, content_height),
    );

    ui.scope_builder(
        UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.label(theme::eyebrow("Set up this PC"));
            ui.add_space(2.0);
            ui.label(
                eframe::egui::RichText::new("Which PC is this?")
                    .font(theme::heavy(28.0))
                    .color(theme::TEXT_STRONG),
            );
            ui.label(
                eframe::egui::RichText::new(
                    "glide shares one keyboard and mouse between your PCs. You can change this later.",
                )
                .color(theme::TEXT_DIM),
            );
            ui.add_space(18.0);

            let (row, _) = ui.allocate_exact_size(Vec2::new(width, CARD_HEIGHT), Sense::hover());
            let card_width = (width - GAP) / 2.0;
            for (i, choice) in CHOICES.iter().enumerate() {
                let x = row.left() + i as f32 * (card_width + GAP);
                let rect = Rect::from_min_size(Pos2::new(x, row.top()), Vec2::new(card_width, CARD_HEIGHT));
                if role_card(ui, rect, choice) {
                    actions.push(Action::ChooseRole(choice.role));
                }
            }
        },
    );
}

/// Draws one choice; returns true when clicked.
fn role_card(ui: &mut Ui, rect: Rect, c: &Choice) -> bool {
    let resp = ui.interact(rect, ui.make_persistent_id(c.tag), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(eframe::egui::Role::Button, true, c.button));
    let hot = resp.hovered() || resp.has_focus();
    if resp.hovered() {
        ui.ctx().set_cursor_icon(eframe::egui::CursorIcon::PointingHand);
    }

    let p = ui.painter();
    let radius = CornerRadius::same(theme::RADIUS);
    let border = if hot { theme::ACCENT } else { theme::LINE };
    p.rect_filled(rect, radius, theme::SURFACE);
    p.rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);

    let inner = rect.shrink(PAD);
    let wrap = inner.width();
    let mut y = inner.top();

    // Tag + illustration.
    let tag = p.layout_no_wrap(
        c.tag.to_uppercase(),
        eframe::egui::FontId::monospace(10.5),
        theme::TEXT_DIM,
    );
    p.galley(Pos2::new(inner.left(), y), tag, theme::TEXT_DIM);
    let art = Rect::from_min_size(Pos2::new(inner.left(), y + 26.0), Vec2::new(wrap, 56.0));
    let lit = if hot { theme::ACCENT } else { theme::TEXT };
    (c.art)(p, art, theme::TEXT_DIM, lit);
    y = art.bottom() + 18.0;

    // Title + body, wrapped to the card.
    let title = p.layout(c.title.into(), theme::heavy(17.0), theme::TEXT_STRONG, wrap);
    let title_h = title.size().y;
    p.galley(Pos2::new(inner.left(), y), title, theme::TEXT_STRONG);
    y += title_h + 6.0;
    let body = p.layout(
        c.body.into(),
        eframe::egui::FontId::proportional(13.0),
        theme::TEXT_DIM,
        wrap,
    );
    p.galley(Pos2::new(inner.left(), y), body, theme::TEXT_DIM);

    // The call to action, pinned to the bottom so both cards line up.
    let button = Rect::from_min_size(
        Pos2::new(inner.left(), inner.bottom() - BUTTON_HEIGHT),
        Vec2::new(wrap, BUTTON_HEIGHT),
    );
    let (fill, text) = if hot {
        (theme::ACCENT, theme::ON_ACCENT)
    } else {
        (theme::RAISED, theme::TEXT_STRONG)
    };
    p.rect_filled(button, radius, fill);
    if !hot {
        p.rect_stroke(button, radius, Stroke::new(1.0, theme::LINE_STRONG), StrokeKind::Inside);
    }
    p.text(
        button.center(),
        Align2::CENTER_CENTER,
        c.button,
        theme::heavy(15.0),
        text,
    );
    resp.clicked()
}

// ------------------------------------------------------------------ line art

/// A keyboard with a mouse beside it; the mouse is "lit".
fn keyboard_and_mouse(p: &Painter, r: Rect, dim: Color32, lit: Color32) {
    let line = Stroke::new(1.5, dim);
    let kb = Rect::from_min_size(Pos2::new(r.left(), r.center().y - 16.0), Vec2::new(96.0, 36.0));
    p.rect_stroke(kb, CornerRadius::same(4), line, StrokeKind::Inside);
    // Three rows of keys, then a space bar.
    for row in 0..3 {
        for col in 0..9 {
            let k = Rect::from_min_size(
                Pos2::new(kb.left() + 7.0 + col as f32 * 9.4, kb.top() + 6.0 + row as f32 * 7.0),
                Vec2::new(6.0, 4.0),
            );
            p.rect_filled(k, 1.0, dim.gamma_multiply(0.7));
        }
    }
    let space = Rect::from_min_size(Pos2::new(kb.left() + 26.0, kb.top() + 27.0), Vec2::new(44.0, 4.0));
    p.rect_filled(space, 1.0, dim.gamma_multiply(0.7));

    let mouse = Rect::from_min_size(Pos2::new(kb.right() + 14.0, r.center().y - 16.0), Vec2::new(22.0, 34.0));
    p.rect_stroke(mouse, CornerRadius::same(11), Stroke::new(1.5, lit), StrokeKind::Inside);
    p.line_segment(
        [
            Pos2::new(mouse.center().x, mouse.top() + 5.0),
            Pos2::new(mouse.center().x, mouse.top() + 12.0),
        ],
        Stroke::new(1.5, lit),
    );
}

/// A screen with a cursor arriving from the left edge; the cursor is "lit".
fn screen_with_cursor(p: &Painter, r: Rect, dim: Color32, lit: Color32) {
    let line = Stroke::new(1.5, dim);
    let screen = Rect::from_min_size(Pos2::new(r.left() + 30.0, r.top() + 2.0), Vec2::new(78.0, 46.0));
    p.rect_stroke(screen, CornerRadius::same(4), line, StrokeKind::Inside);
    let foot = screen.center_bottom();
    p.line_segment([foot, foot + Vec2::new(0.0, 6.0)], line);
    p.line_segment([foot + Vec2::new(-14.0, 6.0), foot + Vec2::new(14.0, 6.0)], line);

    // Motion trail from outside the screen, then the arrow.
    let y = screen.center().y - 4.0;
    for (i, x) in [r.left(), r.left() + 10.0, r.left() + 20.0].into_iter().enumerate() {
        p.line_segment(
            [Pos2::new(x, y + 6.0), Pos2::new(x + 5.0, y + 6.0)],
            Stroke::new(1.5, lit.gamma_multiply(0.3 + 0.25 * i as f32)),
        );
    }
    let tip = Pos2::new(screen.left() + 16.0, y - 4.0);
    // Arrow head (convex, so it fills cleanly) plus its tail.
    let head = vec![tip, tip + Vec2::new(0.0, 16.0), tip + Vec2::new(11.5, 11.5)];
    p.add(Shape::convex_polygon(head, lit, Stroke::NONE));
    p.line_segment(
        [tip + Vec2::new(5.0, 12.0), tip + Vec2::new(9.0, 19.0)],
        Stroke::new(3.0, lit),
    );
}
