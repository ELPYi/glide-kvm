//! "Limbo": the sharing PC thinks it's controlling a PC that silently ignores
//! input (lock screen, UAC prompt, admin window). glide must never strand
//! the cursor there.

mod common;

use common::*;
use glide::input::{InputBlock, Side};
use glide::platform::{Captured, EdgeHit};

fn edge_right() -> Captured {
    Captured::Edge(EdgeHit {
        side: Side::Right,
        frac: 0.5,
    })
}

#[test]
fn never_crosses_into_a_locked_pc() {
    let p = connected_pair(classic());
    *p.client_desk.block.lock().unwrap() = Some(InputBlock::SecureScreen);
    wait_for("server learns it's locked", || {
        p.server.status().peers[0].blocked == Some(InputBlock::SecureScreen)
    });
    p.server.on_captured(edge_right());
    assert_eq!(
        p.server.status().active,
        None,
        "stayed on this PC instead of entering limbo"
    );

    // Unlocked: crossing works again.
    *p.client_desk.block.lock().unwrap() = None;
    wait_for("server learns it's unlocked", || {
        p.server.status().peers[0].blocked.is_none()
    });
    p.server.on_captured(edge_right());
    assert_eq!(p.server.status().active, Some(0));
}

#[test]
fn takes_control_back_when_the_pc_locks_mid_session() {
    let p = connected_pair(classic());
    p.server.on_captured(edge_right());
    wait_for("in control", || p.client.as_ref().unwrap().status().active);
    assert!(p.server.grab_flag().load(std::sync::atomic::Ordering::Relaxed));

    // The laptop locks (or an admin window takes focus) while we're on it.
    *p.client_desk.block.lock().unwrap() = Some(InputBlock::AdminWindow);
    wait_for("control returns to this PC", || p.server.status().active.is_none());
    assert!(
        !p.server.grab_flag().load(std::sync::atomic::Ordering::Relaxed),
        "local mouse and keyboard work again"
    );
    wait_for("client released everything", || {
        !p.client.as_ref().unwrap().status().active
    });
}
