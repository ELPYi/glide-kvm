//! Feel: high-rate mice, a busy receiving PC, mirrored pointer settings, and
//! live layout changes.

mod common;

use std::time::{Duration, Instant};

use common::*;
use glide::config::SwitchSettings;
use glide::input::{InputEvent, PointerFeel, Side};
use glide::platform::{Captured, EdgeHit};

fn edge(side: Side) -> Captured {
    Captured::Edge(EdgeHit { side, frac: 0.5 })
}

/// Feed an 8 kHz mouse for `dur`: one 1-count move every 125 µs.
/// Returns (total moved, when the last move happened).
fn stream_8khz(p: &Pair, dur: Duration) -> (i32, Instant) {
    let start = Instant::now();
    let mut sent = 0;
    let mut next = start;
    while start.elapsed() < dur {
        if Instant::now() >= next {
            p.server
                .on_captured(Captured::Input(InputEvent::Motion { dx: 1, dy: 0 }));
            sent += 1;
            next += Duration::from_micros(125);
        }
        std::hint::spin_loop();
    }
    (sent, Instant::now())
}

fn enter(p: &Pair) {
    p.server.on_captured(edge(Side::Right));
    wait_for("enter", || p.client.as_ref().unwrap().status().active);
    p.client_desk.clear();
}

#[test]
fn fast_mouse_is_merged_not_lost() {
    let p = connected_pair(SwitchSettings::default());
    enter(&p);
    let (sent, _) = stream_8khz(&p, Duration::from_millis(250));
    wait_for("all motion", || p.client_desk.motion_sum().0 == sent);
    let injections = p.client_desk.events().len() as i32;
    eprintln!("{sent} mouse reports -> {injections} injected moves");
    // Loopback is a wired-class link, so motion is paced at 4 kHz: an 8 kHz
    // mouse is at least halved (more when the receiver is busy).
    assert!(
        injections <= sent / 2 + 10,
        "motion should be merged ({injections} of {sent})"
    );
}

#[test]
fn busy_receiver_does_not_build_a_backlog() {
    let p = connected_pair(SwitchSettings::default());
    enter(&p);
    // Every injected move takes 3 ms, like Windows mid window-drag. Replaying
    // packets one by one would fall ~1 s behind over this stream.
    *p.client_desk.inject_delay.lock().unwrap() = Duration::from_millis(3);
    let (sent, last) = stream_8khz(&p, Duration::from_millis(400));
    wait_for("catch up", || p.client_desk.motion_sum().0 == sent);
    let behind = last.elapsed();
    eprintln!("caught up {behind:?} after the last movement");
    assert!(behind < Duration::from_millis(40), "fell {behind:?} behind");
}

#[test]
fn pointer_feel_is_mirrored_then_restored() {
    let p = connected_pair(SwitchSettings::default());
    let feel = PointerFeel {
        speed: 6,
        acceleration: false,
    };
    *p.server_desk.feel.lock().unwrap() = Some(feel);

    p.server.on_captured(edge(Side::Right));
    wait_for("enter", || p.client.as_ref().unwrap().status().active);
    assert_eq!(*p.client_desk.feel_calls.lock().unwrap(), vec![Some(feel)]);

    p.server.set_locked(false);
    *p.client_desk.edge.lock().unwrap() = Some(EdgeHit {
        side: Side::Left,
        frac: 0.5,
    });
    p.server
        .on_captured(Captured::Input(InputEvent::Motion { dx: -3, dy: 0 }));
    wait_for("leave", || !p.client.as_ref().unwrap().status().active);
    assert_eq!(
        *p.client_desk.feel_calls.lock().unwrap(),
        vec![Some(feel), None],
        "restored on leave"
    );
}

#[test]
fn pointer_feel_can_be_turned_off() {
    let settings = SwitchSettings {
        match_pointer_feel: false,
        ..Default::default()
    };
    let p = connected_pair(settings);
    *p.server_desk.feel.lock().unwrap() = Some(PointerFeel {
        speed: 6,
        acceleration: false,
    });
    enter(&p);
    assert_eq!(
        *p.client_desk.feel_calls.lock().unwrap(),
        vec![None],
        "keeps its own settings"
    );
}

#[test]
fn layout_changes_apply_live() {
    let p = connected_pair(SwitchSettings::default());
    p.server.set_layout(&[Side::Left]).unwrap();
    p.server.on_captured(edge(Side::Right));
    assert_eq!(p.server.status().active, None, "nothing on the right any more");
    p.server.on_captured(edge(Side::Left));
    assert_eq!(p.server.status().active, Some(0));
    wait_for("enter", || !p.client_desk.warps.lock().unwrap().is_empty());
    assert_eq!(
        p.client_desk.warps.lock().unwrap()[0].0,
        Side::Right,
        "enters from the facing edge"
    );

    assert!(p.server.set_layout(&[Side::Left, Side::Right]).is_err(), "wrong length");
}

#[test]
fn latency_while_moving() {
    // A mouse being moved: one report per millisecond. Each one should reach
    // the other PC's injector within a fraction of a millisecond.
    let p = connected_pair(SwitchSettings::default());
    enter(&p);
    let mut lat = Vec::new();
    let mut next = Instant::now();
    for i in 0..500 {
        while Instant::now() < next {
            std::hint::spin_loop();
        }
        next += Duration::from_millis(1);
        let t0 = Instant::now();
        p.server
            .on_captured(Captured::Input(InputEvent::Motion { dx: 1, dy: 0 }));
        wait_for("motion", || p.client_desk.motion_sum().0 > i);
        lat.push(p.client_desk.injected.lock().unwrap().last().unwrap().0 - t0);
    }
    lat.sort();
    let (p50, p99) = (lat[lat.len() / 2], lat[lat.len() * 99 / 100]);
    eprintln!("while moving: p50 {p50:?}, p99 {p99:?}, max {:?}", lat[lat.len() - 1]);
    assert!(p50 < Duration::from_micros(500), "p50 {p50:?}");
}

/// Benchmark, not run by default: latency while moving with every CPU core
/// saturated (a game or a build running on the PC). Run with
/// `cargo test --release --test snappy under_cpu_load -- --ignored --nocapture`.
#[test]
#[ignore]
fn under_cpu_load() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let stop = Arc::new(AtomicBool::new(false));
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let hogs: Vec<_> = (0..cores * 2)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut x = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    std::hint::black_box(x);
                }
            })
        })
        .collect();

    let p = connected_pair(SwitchSettings::default());
    enter(&p);
    // This thread plays the capture thread, which the real app boosts.
    glide::platform::boost_latency_thread();
    glide::platform::raise_process_priority();
    let mut lat = Vec::new();
    let mut next = Instant::now();
    for i in 0..1000 {
        while Instant::now() < next {
            std::hint::spin_loop();
        }
        next += Duration::from_millis(1);
        let t0 = Instant::now();
        p.server
            .on_captured(Captured::Input(InputEvent::Motion { dx: 1, dy: 0 }));
        wait_for("motion", || p.client_desk.motion_sum().0 > i);
        lat.push(p.client_desk.injected.lock().unwrap().last().unwrap().0 - t0);
    }
    stop.store(true, Ordering::Relaxed);
    for h in hogs {
        let _ = h.join();
    }
    lat.sort();
    eprintln!(
        "under full CPU load ({} busy threads): p50 {:?}, p99 {:?}, max {:?}",
        cores * 2,
        lat[lat.len() / 2],
        lat[lat.len() * 99 / 100],
        lat[lat.len() - 1]
    );
}
