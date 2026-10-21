//! `glide-cli selftest`: verifies, on this machine, the assumptions capture
//! relies on, using injected input as a stand-in for a real mouse/keyboard.
//! Freezes input for about a second.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Result};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;

use super::desktop::{cursor_pos, WinDesktop};
use super::{start_capture, SELFTEST};
use crate::input::{InputEvent, Side};
use crate::platform::{Captured, Desktop};

const F24: u16 = 194; // harmless key nothing listens to

pub fn selftest() -> Result<()> {
    super::init_dpi();
    check_edges()?;
    check_grab()
}

/// Warp to each outer edge and check that a push outward is detected.
fn check_edges() -> Result<()> {
    let d = WinDesktop::new();
    let start = cursor_pos().unwrap_or(POINT { x: 0, y: 0 });
    for side in Side::ALL {
        d.warp_to_edge(side, 0.5);
        let (dx, dy) = match side {
            Side::Left => (-1, 0),
            Side::Right => (1, 0),
            Side::Top => (0, -1),
            Side::Bottom => (0, 1),
        };
        let moved_to = cursor_pos().map(|p| (p.x, p.y));
        if side == Side::Left && moved_to == Some((start.x, start.y)) && start.x != 0 {
            anyhow::bail!(
                "couldn't move the cursor: an administrator window is probably in front (Windows blocks this). \
                 Click a normal window, or run the selftest as administrator."
            );
        }
        let hit = d.edge_hit(dx, dy);
        println!(
            "  edge {side:?}: cursor {:?} -> {hit:?}",
            cursor_pos().map(|p| (p.x, p.y))
        );
        ensure!(hit.is_some_and(|h| h.side == side), "edge {side:?} not detected");
    }
    unsafe {
        SetCursorPos(start.x, start.y);
    }
    Ok(())
}

/// While grabbed, hooks swallow input, yet raw motion still arrives.
fn check_grab() -> Result<()> {
    let d = WinDesktop::new();
    SELFTEST.store(true, Ordering::Relaxed);
    let grab = Arc::new(AtomicBool::new(true));
    let motion_x = Arc::new(AtomicI32::new(0));
    let keys = Arc::new(AtomicU32::new(0));
    let (mx, k) = (motion_x.clone(), keys.clone());
    let capture = start_capture(
        grab.clone(),
        Box::new(move |ev| {
            match ev {
                Captured::Input(InputEvent::Motion { dx, .. }) => {
                    mx.fetch_add(dx, Ordering::Relaxed);
                }
                Captured::Input(InputEvent::Key { .. }) => {
                    k.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
            true
        }),
    );
    let result = (|| {
        let _capture = capture?;
        std::thread::sleep(Duration::from_millis(200));
        let before = cursor_pos().unwrap();
        for _ in 0..5 {
            d.inject(&InputEvent::Motion { dx: 20, dy: 0 });
        }
        d.inject(&InputEvent::Key { code: F24, down: true });
        d.inject(&InputEvent::Key { code: F24, down: false });
        std::thread::sleep(Duration::from_millis(300));
        let after = cursor_pos().unwrap();

        let mx = motion_x.load(Ordering::Relaxed);
        let k = keys.load(Ordering::Relaxed);
        let moved = before.x != after.x || before.y != after.y;
        println!("  grabbed: raw motion dx sum = {mx}, key events = {k}, cursor moved = {moved}");
        ensure!(
            mx == 100,
            "raw motion was not delivered while the mouse hook was blocking"
        );
        ensure!(k == 2, "keyboard hook did not see the key events");
        ensure!(!moved, "cursor moved even though it was grabbed");
        Ok(())
    })();
    SELFTEST.store(false, Ordering::Relaxed);
    result
}
