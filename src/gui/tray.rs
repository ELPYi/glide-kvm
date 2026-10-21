//! The tray icon (the hidden-icons area by the clock). Closing the window
//! hides glide here; sharing keeps running. Left-click reopens the window;
//! right-click offers Open, Lock / unlock switching, and Quit.
//!
//! Tray events arrive on the UI thread's message loop even while the window
//! is hidden and egui isn't running frames, so the handlers act directly:
//! they show the window through the OS, or use the shared [`EngineSlot`].

use anyhow::Result;
use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::engine_slot::EngineSlot;
use super::theme;
use crate::platform::system;
use crate::status::EngineStatus;

/// Keeps the icon alive; dropping it removes the icon.
pub struct Tray {
    _icon: TrayIcon,
}

/// Add the tray icon. `window` is the OS handle of the app window.
pub fn install(ctx: egui::Context, window: isize, engine: EngineSlot) -> Result<Tray> {
    let open = MenuItem::new("Open glide", true, None);
    let lock = MenuItem::new("Lock / unlock switching", true, None);
    let quit = MenuItem::new("Quit glide", true, None);
    let menu = Menu::new();
    menu.append(&open)?;
    menu.append(&lock)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit)?;

    let art = theme::app_icon();
    let icon = TrayIconBuilder::new()
        .with_icon(Icon::from_rgba(art.rgba, art.width, art.height)?)
        .with_tooltip("glide")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    let show = {
        let ctx = ctx.clone();
        move || {
            system::show_window(window);
            ctx.request_repaint();
        }
    };

    let on_click = show.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            on_click();
        }
    }));

    let (open_id, lock_id, quit_id) = (open.id().clone(), lock.id().clone(), quit.id().clone());
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == open_id {
            show();
        } else if event.id == lock_id {
            engine.with(|e| {
                let locked = matches!(e.status(), EngineStatus::Server(s) if s.locked);
                e.set_locked(!locked);
            });
            ctx.request_repaint();
        } else if event.id == quit_id {
            // Restore normal input and say goodbye to clients, then exit.
            engine.stop();
            std::process::exit(0);
        }
    }));

    Ok(Tray { _icon: icon })
}
