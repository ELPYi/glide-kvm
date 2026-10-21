//! End-to-end: a real server and client talking encrypted UDP over loopback,
//! with mock desktops standing in for the OS. Exercises everything except
//! the OS hooks themselves (those are covered by `glide-cli selftest`).

mod common;

use std::time::{Duration, Instant};

use common::*;
use glide::config::SwitchSettings;
use glide::input::{keys::*, Button, InputEvent, Side};
use glide::platform::{Captured, EdgeHit};

fn key(code: u16, down: bool) -> Captured {
    Captured::Input(InputEvent::Key { code, down })
}

/// Press and release the lock shortcut (default Ctrl+Shift+L). Returns
/// whether the L press was swallowed.
fn press_lock(srv: &glide::server::Server) -> bool {
    const L: u16 = 38;
    const KEY_LEFTSHIFT: u16 = 42;
    srv.on_captured(key(KEY_LEFTCTRL, true));
    srv.on_captured(key(KEY_LEFTSHIFT, true));
    let swallowed = srv.on_captured(key(L, true));
    srv.on_captured(key(L, false));
    srv.on_captured(key(KEY_LEFTSHIFT, false));
    srv.on_captured(key(KEY_LEFTCTRL, false));
    swallowed
}

fn edge_right(frac: f32) -> Captured {
    Captured::Edge(EdgeHit {
        side: Side::Right,
        frac,
    })
}

#[test]
fn input_crosses_over_and_back() {
    let p = connected_pair(SwitchSettings::default());
    let srv = &p.server;

    // Cross the right edge: client is entered on its left edge, same height.
    assert!(!srv.on_captured(edge_right(0.3)));
    assert!(srv.grab_flag().load(std::sync::atomic::Ordering::Relaxed));
    wait_for("enter", || !p.client_desk.warps.lock().unwrap().is_empty());
    assert_eq!(p.client_desk.warps.lock().unwrap()[0], (Side::Left, 0.3));
    wait_for("client reports active", || p.client.as_ref().unwrap().status().active);
    assert_eq!(srv.status().active, Some(0));

    // Keys are swallowed locally and arrive remotely, in order.
    assert!(srv.on_captured(key(30, true)));
    assert!(srv.on_captured(key(30, false)));
    assert!(srv.on_captured(Captured::Input(InputEvent::Wheel { dx: 0, dy: 120 })));
    wait_for("keys", || p.client_desk.events().len() >= 3);
    assert_eq!(
        p.client_desk.events(),
        vec![
            InputEvent::Key { code: 30, down: true },
            InputEvent::Key { code: 30, down: false },
            InputEvent::Wheel { dx: 0, dy: 120 }
        ]
    );

    // Push off the client's left edge: control returns, and the server's
    // cursor lands on its right edge at the same height.
    *p.client_desk.edge.lock().unwrap() = Some(EdgeHit {
        side: Side::Left,
        frac: 0.6,
    });
    srv.on_captured(Captured::Input(InputEvent::Motion { dx: -4, dy: 0 }));
    wait_for("return", || srv.status().active.is_none());
    assert_eq!(p.server_desk.warps.lock().unwrap()[0], (Side::Right, 0.6));
    wait_for("client reports inactive", || {
        !p.client.as_ref().unwrap().status().active
    });
}

#[test]
fn latency_hook_to_injection() {
    let p = connected_pair(SwitchSettings::default());
    p.server.on_captured(edge_right(0.5));
    wait_for("enter", || p.client.as_ref().unwrap().status().active);

    // The first movement after a pause goes out immediately (no pacing delay).
    p.client_desk.clear();
    let mut lat = Vec::new();
    for i in 0..300 {
        std::thread::sleep(Duration::from_millis(2));
        let t0 = Instant::now();
        p.server
            .on_captured(Captured::Input(InputEvent::Motion { dx: 1, dy: 0 }));
        wait_for("motion", || p.client_desk.injected.lock().unwrap().len() > i);
        lat.push(p.client_desk.injected.lock().unwrap()[i].0 - t0);
    }
    lat.sort();
    let (p50, p99) = (lat[lat.len() / 2], lat[lat.len() * 99 / 100]);
    eprintln!(
        "hook->inject over loopback UDP: p50 {p50:?}, p99 {p99:?}, max {:?}",
        lat[lat.len() - 1]
    );
    assert!(p50 < Duration::from_millis(2), "p50 {p50:?} is far too slow");
}

#[test]
fn drag_protection_end_to_end() {
    let p = connected_pair(SwitchSettings {
        block_while_dragging: true,
        edge_delay_ms: 0,
        ..Default::default()
    });
    let left = |down| {
        Captured::Input(InputEvent::Button {
            button: Button::Left,
            down,
        })
    };
    assert!(!p.server.on_captured(left(true)));
    p.server.on_captured(edge_right(0.5));
    assert_eq!(
        p.server.status().active,
        None,
        "dragging a window to the edge must not cross"
    );
    p.server.on_captured(left(false));

    // Switch the setting off live: now the same drag crosses.
    p.server.set_switching(SwitchSettings {
        block_while_dragging: false,
        edge_delay_ms: 0,
        ..Default::default()
    });
    p.server.on_captured(left(true));
    p.server.on_captured(edge_right(0.5));
    assert_eq!(p.server.status().active, Some(0));
    assert!(
        !p.server.on_captured(left(false)),
        "the release belongs to this machine"
    );
}

#[test]
fn lock_and_hotkeys() {
    let p = connected_pair(SwitchSettings::default());
    let srv = &p.server;

    // Ctrl+Shift+L locks; the L itself never leaks to the focused app.
    assert!(press_lock(srv));
    assert!(srv.status().locked);
    srv.on_captured(edge_right(0.5));
    assert_eq!(srv.status().active, None, "crossed while locked");

    // The tray can unlock too.
    srv.set_locked(false);
    assert!(!srv.status().locked);

    // Keys held on the other PC are released when control comes back.
    srv.on_captured(edge_right(0.5));
    wait_for("in control", || p.client.as_ref().unwrap().status().active);
    p.client_desk.clear();
    assert!(srv.on_captured(key(KEY_LEFTALT, true)));
    *p.client_desk.edge.lock().unwrap() = Some(EdgeHit {
        side: Side::Left,
        frac: 0.5,
    });
    srv.on_captured(Captured::Input(InputEvent::Motion { dx: -3, dy: 0 }));
    wait_for("back here", || srv.status().active.is_none());
    wait_for("release on leave", || {
        p.client_desk.events().contains(&InputEvent::Key {
            code: KEY_LEFTALT,
            down: false,
        })
    });
}

#[test]
fn status_reports_connection_and_rtt() {
    let p = connected_pair(SwitchSettings::default());
    let st = p.server.status();
    assert_eq!(st.peers.len(), 1);
    assert_eq!(st.peers[0].name, "test-client");
    assert_eq!(st.peers[0].resolved.map(|a| a.port()), Some(p.client_port));
    wait_for("rtt", || p.server.status().peers[0].rtt_ms.is_some());
    assert!(p.server.status().peers[0].rtt_ms.unwrap() < 50.0);

    let cs = p.client.as_ref().unwrap().status();
    assert!(cs.connected);
    assert_eq!(cs.server.map(|a| a.port()), Some(p.server_port));
}

#[test]
fn client_restart_is_detected() {
    let mut p = connected_pair(SwitchSettings::default());
    p.server.on_captured(edge_right(0.5));
    wait_for("enter", || p.client.as_ref().unwrap().status().active);

    // Restart the client on the same port (new instance id).
    p.client.take().unwrap().stop();
    let sock = std::net::UdpSocket::bind(("127.0.0.1", p.client_port)).expect("port freed on stop");
    p.client = Some(start_client(sock, p.client_desk.clone(), None));
    wait_for("server notices the restart", || p.server.status().active.is_none());

    // And it works again from scratch.
    wait_for("reconnect", || p.server.status().peers[0].connected);
    p.client_desk.clear();
    p.server.on_captured(edge_right(0.5));
    p.server.on_captured(key(30, true));
    wait_for("key after restart", || {
        p.client_desk
            .events()
            .contains(&InputEvent::Key { code: 30, down: true })
    });
}

#[test]
fn wrong_secret_never_connects() {
    use glide::client::{self, ClientOptions};
    let client_sock = localhost();
    let client_port = client_sock.local_addr().unwrap().port();
    let desk = std::sync::Arc::new(MockDesktop::default());
    let opts = ClientOptions {
        secret: "a-completely-different-secret".into(),
        server_address: None,
        server_port: 0,
        games: Default::default(),
    };
    let _client = client::start(client_sock, desk, opts).unwrap();

    let cfg = glide::config::Config {
        clients: vec![glide::config::ClientEntry {
            name: "x".into(),
            address: format!("127.0.0.1:{client_port}"),
            side: Side::Right,
        }],
        ..glide::config::Config::new_default_with_secret(SECRET)
    };
    let server = glide::server::Server::start(&cfg, std::sync::Arc::new(MockDesktop::default()), localhost()).unwrap();
    std::thread::sleep(Duration::from_millis(700));
    assert!(!server.status().peers[0].connected);
    server.stop();
}

#[test]
fn shortcuts_are_recorded_and_applied_live() {
    let p = connected_pair(SwitchSettings::default());
    let srv = &p.server;

    // Settings → Change: the next combination is captured, not acted on.
    srv.record_hotkey(true);
    assert!(srv.status().recording_hotkey);
    assert!(!srv.on_captured(key(KEY_LEFTCTRL, true)), "modifiers pass through");
    assert!(srv.on_captured(key(38, true)), "the L is swallowed"); // L
    srv.on_captured(key(38, false));
    srv.on_captured(key(KEY_LEFTCTRL, false));
    let recorded = srv.take_recorded_hotkey().expect("a result").expect("not cancelled");
    assert_eq!(recorded.to_string(), "Ctrl+L");
    assert!(!srv.status().recording_hotkey);

    // Applied: the new lock shortcut works immediately; the default no longer does.
    srv.set_hotkeys(glide::hotkey::Hotkeys { lock: recorded });
    press_lock(srv);
    assert!(!srv.status().locked);
    srv.on_captured(key(KEY_LEFTCTRL, true));
    assert!(srv.on_captured(key(38, true)));
    assert!(srv.status().locked);
}

#[test]
fn shortcuts_confirm_on_the_screen_you_are_using() {
    let p = connected_pair(SwitchSettings::default());
    let srv = &p.server;

    // Locking while here: the confirmation shows on this PC.
    press_lock(srv);
    assert_eq!(*p.server_desk.notices.lock().unwrap(), vec!["Switching locked"]);
    srv.set_locked(false); // tray / UI: also confirmed
    assert_eq!(p.server_desk.notices.lock().unwrap().len(), 2);

    // Toggling while on the other PC: it shows there instead.
    srv.on_captured(edge_right(0.5));
    wait_for("in control", || p.client.as_ref().unwrap().status().active);
    press_lock(srv);
    wait_for("notice on the other PC", || {
        *p.client_desk.notices.lock().unwrap() == vec!["Switching locked".to_string()]
    });
    assert_eq!(
        p.server_desk.notices.lock().unwrap().len(),
        2,
        "not on the PC you aren't looking at"
    );
}

#[test]
fn cursor_hides_on_the_pc_you_are_not_using() {
    let p = connected_pair(SwitchSettings::default());
    let srv = &p.server;
    *p.client_desk.cursor.lock().unwrap() = Some((500, 300));
    let hidden = |d: &MockDesktop| d.cursor_hidden.lock().unwrap().last().copied();

    // Crossing over: the cursor disappears here...
    srv.on_captured(edge_right(0.5));
    wait_for("in control", || p.client.as_ref().unwrap().status().active);
    assert_eq!(hidden(&p.server_desk), Some(true), "hidden here while on the other PC");
    assert_ne!(hidden(&p.client_desk), Some(true), "visible where you are");

    // ...and coming back shows it here and hides it over there.
    *p.client_desk.edge.lock().unwrap() = Some(EdgeHit {
        side: Side::Left,
        frac: 0.5,
    });
    srv.on_captured(Captured::Input(InputEvent::Motion { dx: -3, dy: 0 }));
    wait_for("back here", || srv.status().active.is_none());
    assert_eq!(hidden(&p.server_desk), Some(false));
    wait_for("hidden on the other PC", || hidden(&p.client_desk) == Some(true));

    // Using the other PC's own mouse brings its cursor straight back.
    *p.client_desk.cursor.lock().unwrap() = Some((520, 300));
    wait_for("shown on local use", || hidden(&p.client_desk) == Some(false));
}

#[test]
fn a_game_in_focus_keeps_the_mouse_on_its_pc() {
    use glide::game::Foreground;
    use glide::server::GAME_CHECK_EVERY;
    let p = connected_pair(SwitchSettings::default());
    let srv = &p.server;
    let client = || p.client.as_ref().unwrap().status();
    let fullscreen = |app: &str| Foreground {
        app: app.into(),
        fullscreen: true,
        ..Default::default()
    };

    // Fullscreen video (a browser) isn't a game.
    *p.server_desk.foreground.lock().unwrap() = Some(fullscreen("chrome.exe"));
    std::thread::sleep(GAME_CHECK_EVERY * 2);
    assert_eq!(srv.status().game, None, "fullscreen video doesn't lock");

    // A fullscreen game: the edge doesn't switch.
    *p.server_desk.foreground.lock().unwrap() = Some(fullscreen("eldenring.exe"));
    wait_for("game noticed", || srv.status().game.is_some());
    srv.on_captured(edge_right(0.5));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(srv.status().active, None, "stayed on this PC mid-game");

    // Out of the game: switching is back.
    *p.server_desk.foreground.lock().unwrap() = None;
    wait_for("game gone", || srv.status().game.is_none());
    srv.on_captured(edge_right(0.5));
    wait_for("crossed", || client().active);

    // A game on the other PC keeps control there.
    *p.client_desk.foreground.lock().unwrap() = Some(fullscreen("eldenring.exe"));
    wait_for("game noticed over there", || client().game.is_some());
    *p.client_desk.edge.lock().unwrap() = Some(EdgeHit {
        side: Side::Left,
        frac: 0.5,
    });
    srv.on_captured(Captured::Input(InputEvent::Motion { dx: -3, dy: 0 }));
    std::thread::sleep(Duration::from_millis(150));
    assert!(srv.status().active.is_some(), "stayed on the other PC mid-game");

    // Its game closes: the edge back works again.
    *p.client_desk.foreground.lock().unwrap() = None;
    wait_for("game gone over there", || client().game.is_none());
    srv.on_captured(Captured::Input(InputEvent::Motion { dx: -3, dy: 0 }));
    wait_for("back here", || srv.status().active.is_none());
}
