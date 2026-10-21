//! The switching brain: given each physical input event, decide whether it
//! stays on this machine or goes to a client, and when to change machines.
//!
//! Pure logic: no sockets, no OS calls, time passed in. That keeps every
//! rule here (drag protection, edge delay, lock, hotkeys, stuck-key
//! avoidance) directly unit-testable. The server feeds it events and carries
//! out the [`Decision`]s.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::config::SwitchSettings;
use crate::hotkey::{self, Hotkey, Hotkeys, Mods};
use crate::input::{Button, InputEvent, Side};
use crate::platform::{Captured, EdgeHit};

/// A pause in edge pushing longer than this restarts the edge-delay timer.
const PUSH_GAP: Duration = Duration::from_millis(100);
const KEY_ESC: u16 = 1;

/// What the server should do with one captured event.
#[derive(Debug, Default, PartialEq)]
pub struct Decision {
    /// Swallow the event locally.
    pub block: bool,
    /// Send this to the active client.
    pub forward: Option<Forward>,
    /// Change which machine has input.
    pub switch: Option<Switch>,
    /// The lock was toggled (new state).
    pub lock_changed: Option<bool>,
    /// Shortcut recording finished: `Some(key)` recorded, `None` cancelled (Esc).
    pub recorded: Option<Option<Hotkey>>,
}

#[derive(Debug, PartialEq)]
pub enum Forward {
    /// Unreliable, latest-wins.
    Motion { dx: i32, dy: i32 },
    /// Keys, buttons, wheel: must arrive, in order.
    Reliable(InputEvent),
}

#[derive(Debug, PartialEq)]
pub enum Switch {
    /// Hand input to client `peer`, entering at `frac` along the shared edge.
    Enter { peer: usize, frac: f32 },
    /// Take input back from `peer`. `warp` = where along our edge the cursor
    /// reappears; `None` leaves the cursor where it was.
    Leave { peer: usize, warp: Option<f32> },
}

/// Which edge is being pushed: one of ours, or the facing edge of a client.
#[derive(Clone, Copy, PartialEq)]
enum PushedEdge {
    Local(Side),
    Remote(usize),
}

struct EdgePush {
    edge: PushedEdge,
    started: Instant,
    last: Instant,
}

pub struct Switcher {
    settings: SwitchSettings,
    hotkeys: Hotkeys,
    /// The next key combination pressed becomes a shortcut (Settings → Change).
    recording: bool,
    active: Option<usize>,
    locked: bool,
    /// A game is in focus here: the edges don't switch (see `crate::game`).
    in_game: bool,
    /// Every physical key currently held, wherever its events are going.
    phys_down: HashSet<u16>,
    /// Mouse buttons held while input is local.
    local_buttons: HashSet<Button>,
    /// Mouse buttons held down on the active client (a drag in progress there).
    remote_buttons: HashSet<Button>,
    /// Keys/buttons already held here when we switched away. Their releases
    /// must reach this machine, or they'd stay stuck down here.
    held_keys_at_grab: HashSet<u16>,
    held_buttons_at_grab: HashSet<Button>,
    /// Hotkeys whose autorepeat and release should be swallowed.
    swallow: HashSet<u16>,
    edge_push: Option<EdgePush>,
}

impl Switcher {
    pub fn new(settings: SwitchSettings, hotkeys: Hotkeys) -> Self {
        Switcher {
            settings,
            hotkeys,
            recording: false,
            active: None,
            locked: false,
            in_game: false,
            phys_down: HashSet::new(),
            local_buttons: HashSet::new(),
            remote_buttons: HashSet::new(),
            held_keys_at_grab: HashSet::new(),
            held_buttons_at_grab: HashSet::new(),
            swallow: HashSet::new(),
            edge_push: None,
        }
    }

    pub fn active(&self) -> Option<usize> {
        self.active
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }

    pub fn in_game(&self) -> bool {
        self.in_game
    }

    pub fn set_in_game(&mut self, in_game: bool) {
        self.in_game = in_game;
    }

    pub fn set_settings(&mut self, settings: SwitchSettings) {
        self.settings = settings;
    }

    pub fn settings(&self) -> SwitchSettings {
        self.settings
    }

    pub fn set_hotkeys(&mut self, hotkeys: Hotkeys) {
        self.hotkeys = hotkeys;
    }

    /// Capture the next key combination instead of acting on it.
    pub fn record_hotkey(&mut self, on: bool) {
        self.recording = on;
    }

    pub fn recording(&self) -> bool {
        self.recording
    }

    /// Process one captured event. `target(side)` names the *connected*
    /// client beyond that edge of this machine, if any.
    pub fn handle(&mut self, ev: Captured, now: Instant, target: impl Fn(Side) -> Option<usize>) -> Decision {
        match ev {
            Captured::Edge(hit) => self.on_edge(hit, now, target),
            Captured::Modifiers(actual) => {
                // Forget modifiers we think are held but the OS says aren't.
                for code in actual.released_codes() {
                    self.phys_down.remove(&code);
                }
                Decision::default()
            }
            Captured::Input(InputEvent::Motion { dx, dy }) => Decision {
                forward: self.active.map(|_| Forward::Motion { dx, dy }),
                ..Default::default()
            },
            Captured::Input(ev @ InputEvent::Key { code, down }) => self.on_key(ev, code, down),
            Captured::Input(ev @ InputEvent::Button { button, down }) => self.on_button(ev, button, down),
            Captured::Input(ev @ InputEvent::Wheel { .. }) => match self.active {
                Some(_) => Decision {
                    block: true,
                    forward: Some(Forward::Reliable(ev)),
                    ..Default::default()
                },
                None => Decision::default(),
            },
        }
    }

    /// The active client reports the cursor being pushed off one of its
    /// edges. Returns control here if it's the edge facing us, under the same
    /// rules as leaving this machine: not while locked, not mid-drag (if
    /// enabled), and only after the edge delay.
    pub fn client_left(&mut self, peer: usize, facing_us: bool, frac: f32, now: Instant) -> Option<Switch> {
        if self.active != Some(peer) || !facing_us || self.locked {
            return None;
        }
        // Dragging on the client (e.g. snapping a window to its edge): stay there.
        if self.settings.block_while_dragging && !self.remote_buttons.is_empty() {
            self.edge_push = None;
            return None;
        }
        if !self.pushed_long_enough(PushedEdge::Remote(peer), now) {
            return None;
        }
        self.leave(Some(frac))
    }

    /// Take input back unconditionally (client lost, engine stopping, hotkey).
    pub fn force_leave(&mut self) -> Option<Switch> {
        self.leave(None)
    }

    fn on_edge(&mut self, hit: EdgeHit, now: Instant, target: impl Fn(Side) -> Option<usize>) -> Decision {
        let pass = Decision::default();
        if self.active.is_some() || self.locked || self.in_game {
            self.edge_push = None;
            return pass;
        }
        // Dragging something (a window to snap, a file, a selection):
        // stay put so the drag ends here.
        if self.settings.block_while_dragging && !self.local_buttons.is_empty() {
            self.edge_push = None;
            return pass;
        }
        let Some(peer) = target(hit.side) else { return pass };
        if !self.pushed_long_enough(PushedEdge::Local(hit.side), now) {
            return pass;
        }
        self.active = Some(peer);
        self.remote_buttons.clear();
        self.held_keys_at_grab = self.phys_down.clone();
        self.held_buttons_at_grab = std::mem::take(&mut self.local_buttons);
        Decision {
            switch: Some(Switch::Enter { peer, frac: hit.frac }),
            ..Default::default()
        }
    }

    /// Edge delay: true once `edge` has been pushed continuously (no pause
    /// longer than [`PUSH_GAP`]) for `edge_delay_ms`.
    fn pushed_long_enough(&mut self, edge: PushedEdge, now: Instant) -> bool {
        let started = match &self.edge_push {
            Some(p) if p.edge == edge && now.duration_since(p.last) < PUSH_GAP => p.started,
            _ => now,
        };
        if now.duration_since(started) < Duration::from_millis(self.settings.edge_delay_ms as u64) {
            self.edge_push = Some(EdgePush {
                edge,
                started,
                last: now,
            });
            return false;
        }
        self.edge_push = None;
        true
    }

    fn leave(&mut self, warp: Option<f32>) -> Option<Switch> {
        let peer = self.active.take()?;
        self.remote_buttons.clear();
        self.local_buttons = std::mem::take(&mut self.held_buttons_at_grab);
        self.held_keys_at_grab.clear();
        Some(Switch::Leave { peer, warp })
    }

    fn on_key(&mut self, ev: InputEvent, code: u16, down: bool) -> Decision {
        let block = Decision {
            block: true,
            ..Default::default()
        };
        let repeat = if down {
            !self.phys_down.insert(code)
        } else {
            self.phys_down.remove(&code);
            false
        };

        if self.swallow.contains(&code) {
            if !down {
                self.swallow.remove(&code);
            }
            return block;
        }

        if down && !repeat {
            if self.recording && !hotkey::is_modifier(code) {
                self.recording = false;
                self.swallow.insert(code);
                let mods = Mods::from_held(&self.phys_down);
                let cancelled = code == KEY_ESC && mods == Mods::default();
                return Decision {
                    block: true,
                    recorded: Some((!cancelled).then_some(Hotkey::new(mods, code))),
                    ..Default::default()
                };
            }
            if self.hotkeys.lock.matches(code, &self.phys_down) {
                self.locked = !self.locked;
                self.swallow.insert(code);
                return Decision {
                    block: true,
                    lock_changed: Some(self.locked),
                    ..Default::default()
                };
            }
        }

        if self.active.is_none() {
            return Decision::default();
        }
        if self.held_keys_at_grab.contains(&code) {
            // Pressed before we crossed over: it belongs to this machine.
            if !down {
                self.held_keys_at_grab.remove(&code);
            }
            return Decision::default();
        }
        Decision {
            block: true,
            forward: Some(Forward::Reliable(ev)),
            ..Default::default()
        }
    }

    fn on_button(&mut self, ev: InputEvent, button: Button, down: bool) -> Decision {
        if self.active.is_none() {
            if down {
                self.local_buttons.insert(button);
            } else {
                self.local_buttons.remove(&button);
            }
            return Decision::default();
        }
        if !down && self.held_buttons_at_grab.remove(&button) {
            return Decision::default();
        }
        if down {
            self.remote_buttons.insert(button);
        } else {
            self.remote_buttons.remove(&button);
        }
        Decision {
            block: true,
            forward: Some(Forward::Reliable(ev)),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keys::*;

    const MS: Duration = Duration::from_millis(1);
    const SHIFT: u16 = 42;
    const A: u16 = 30;
    const L: u16 = 38;
    const KEY_LEFTSHIFT: u16 = 42;

    /// One client, connected, on the right.
    fn right_only(side: Side) -> Option<usize> {
        (side == Side::Right).then_some(0)
    }

    fn edge(side: Side, frac: f32) -> Captured {
        Captured::Edge(EdgeHit { side, frac })
    }

    fn key(code: u16, down: bool) -> Captured {
        Captured::Input(InputEvent::Key { code, down })
    }

    fn button(down: bool) -> Captured {
        Captured::Input(InputEvent::Button {
            button: Button::Left,
            down,
        })
    }

    /// A switcher whose lock is the single key Scroll Lock (easy to press in
    /// tests; the real default is checked in `default_lock_is_ctrl_shift_l`).
    fn switcher() -> Switcher {
        Switcher::new(
            SwitchSettings::default(),
            Hotkeys {
                lock: "ScrollLock".parse().unwrap(),
            },
        )
    }

    /// Cross to the client on the right at `now`.
    fn enter(s: &mut Switcher, now: Instant) {
        let d = s.handle(edge(Side::Right, 0.5), now, right_only);
        assert_eq!(d.switch, Some(Switch::Enter { peer: 0, frac: 0.5 }));
    }

    #[test]
    fn crosses_at_an_edge_with_a_client() {
        let mut s = switcher();
        let now = Instant::now();
        assert_eq!(
            s.handle(edge(Side::Left, 0.5), now, right_only),
            Decision::default(),
            "nobody on the left"
        );
        enter(&mut s, now);
        assert_eq!(s.active(), Some(0));
    }

    #[test]
    fn local_input_passes_through_until_crossing() {
        let mut s = switcher();
        let now = Instant::now();
        let motion = Captured::Input(InputEvent::Motion { dx: 3, dy: 0 });
        assert_eq!(s.handle(motion, now, right_only), Decision::default());
        assert_eq!(s.handle(key(A, true), now, right_only), Decision::default());
        s.handle(key(A, false), now, right_only);

        enter(&mut s, now);
        let d = s.handle(motion, now, right_only);
        assert_eq!(d.forward, Some(Forward::Motion { dx: 3, dy: 0 }));
        let d = s.handle(key(A, true), now, right_only);
        assert!(d.block);
        assert_eq!(
            d.forward,
            Some(Forward::Reliable(InputEvent::Key { code: A, down: true }))
        );
    }

    #[test]
    fn dragging_blocks_crossing_when_enabled() {
        let mut s = switcher();
        let now = Instant::now();
        s.handle(button(true), now, right_only); // grab a window title bar
        let d = s.handle(edge(Side::Right, 0.5), now, right_only);
        assert_eq!(d.switch, None, "a window drag to the edge must snap, not cross");
        s.handle(button(false), now, right_only); // let go
        enter(&mut s, now);
    }

    #[test]
    fn dragging_can_cross_when_setting_is_off() {
        let mut s = Switcher::new(
            SwitchSettings {
                block_while_dragging: false,
                edge_delay_ms: 0,
                ..Default::default()
            },
            Hotkeys::default(),
        );
        let now = Instant::now();
        s.handle(button(true), now, right_only);
        enter(&mut s, now);
        // The button release must still reach this machine, not the client.
        let d = s.handle(button(false), now, right_only);
        assert_eq!(d, Decision::default());
        // Later clicks go to the client.
        assert!(s.handle(button(true), now, right_only).block);
    }

    #[test]
    fn edge_delay_requires_sustained_push() {
        let mut s = Switcher::new(
            SwitchSettings {
                block_while_dragging: true,
                edge_delay_ms: 200,
                ..Default::default()
            },
            Hotkeys::default(),
        );
        let t0 = Instant::now();
        for i in 0..20 {
            let d = s.handle(edge(Side::Right, 0.5), t0 + i * 10 * MS, right_only);
            assert_eq!(d.switch, None, "only {}ms of pushing", i * 10);
        }
        let d = s.handle(edge(Side::Right, 0.5), t0 + 200 * MS, right_only);
        assert!(d.switch.is_some(), "200ms of pushing crosses");
    }

    #[test]
    fn edge_delay_restarts_after_a_pause() {
        let mut s = Switcher::new(
            SwitchSettings {
                block_while_dragging: true,
                edge_delay_ms: 200,
                ..Default::default()
            },
            Hotkeys::default(),
        );
        let t0 = Instant::now();
        s.handle(edge(Side::Right, 0.5), t0, right_only);
        // 150ms pause (> PUSH_GAP), then a continuous push every 10ms.
        for ms in (150..350).step_by(10) {
            let d = s.handle(edge(Side::Right, 0.5), t0 + ms * MS, right_only);
            assert_eq!(d.switch, None, "timer restarted at 150ms; only at {ms}ms");
        }
        let d = s.handle(edge(Side::Right, 0.5), t0 + 350 * MS, right_only);
        assert!(d.switch.is_some());
    }

    #[test]
    fn scroll_lock_toggles_lock_and_is_swallowed() {
        let mut s = switcher();
        let now = Instant::now();
        let d = s.handle(key(KEY_SCROLLLOCK, true), now, right_only);
        assert_eq!(d.lock_changed, Some(true));
        assert!(d.block);
        assert!(
            s.handle(key(KEY_SCROLLLOCK, true), now, right_only).block,
            "autorepeat swallowed"
        );
        assert!(
            s.handle(key(KEY_SCROLLLOCK, false), now, right_only).block,
            "release swallowed"
        );

        assert_eq!(s.handle(edge(Side::Right, 0.5), now, right_only).switch, None, "locked");

        s.handle(key(KEY_SCROLLLOCK, true), now, right_only);
        s.handle(key(KEY_SCROLLLOCK, false), now, right_only);
        assert!(!s.locked());
        enter(&mut s, now);
    }

    #[test]
    fn a_game_in_focus_keeps_you_here() {
        let mut s = switcher();
        let now = Instant::now();
        s.set_in_game(true);
        assert!(s.in_game());
        assert_eq!(
            s.handle(edge(Side::Right, 0.5), now, right_only).switch,
            None,
            "mid-game"
        );
        s.set_in_game(false);
        assert!(
            s.handle(edge(Side::Right, 0.5), now, right_only).switch.is_some(),
            "game closed"
        );
    }

    #[test]
    fn lock_keeps_you_on_the_client() {
        let mut s = switcher();
        let now = Instant::now();
        enter(&mut s, now);
        s.set_locked(true);
        assert_eq!(s.client_left(0, true, 0.5, Instant::now()), None);
        s.set_locked(false);
        assert_eq!(
            s.client_left(0, true, 0.4, Instant::now()),
            Some(Switch::Leave {
                peer: 0,
                warp: Some(0.4)
            })
        );
    }

    #[test]
    fn client_left_only_through_the_facing_edge() {
        let mut s = switcher();
        enter(&mut s, Instant::now());
        assert_eq!(
            s.client_left(0, false, 0.5, Instant::now()),
            None,
            "far edge of the client"
        );
        assert_eq!(
            s.client_left(1, true, 0.5, Instant::now()),
            None,
            "not the active client"
        );
        assert!(s.client_left(0, true, 0.5, Instant::now()).is_some());
        assert_eq!(s.active(), None);
    }

    #[test]
    fn dragging_on_the_client_blocks_coming_back() {
        let mut s = switcher();
        let now = Instant::now();
        enter(&mut s, now);
        s.handle(button(true), now, right_only); // grab a window on the client
        assert_eq!(
            s.client_left(0, true, 0.5, now),
            None,
            "snap it on the client, don't come back"
        );
        s.handle(button(false), now, right_only); // let go
        assert!(s.client_left(0, true, 0.5, now).is_some());
    }

    #[test]
    fn dragging_on_the_client_can_cross_when_setting_is_off() {
        let mut s = Switcher::new(
            SwitchSettings {
                block_while_dragging: false,
                ..Default::default()
            },
            Hotkeys::default(),
        );
        let now = Instant::now();
        enter(&mut s, now);
        s.handle(button(true), now, right_only);
        assert!(s.client_left(0, true, 0.5, now).is_some());
    }

    #[test]
    fn edge_delay_applies_when_coming_back_too() {
        let mut s = Switcher::new(
            SwitchSettings {
                edge_delay_ms: 200,
                ..Default::default()
            },
            Hotkeys::default(),
        );
        let t0 = Instant::now();
        for i in 0..20 {
            s.handle(edge(Side::Right, 0.5), t0 + i * 10 * MS, right_only);
        }
        let back_at = t0 + 200 * MS;
        assert!(s.handle(edge(Side::Right, 0.5), back_at, right_only).switch.is_some());
        for i in 0..20 {
            let d = s.client_left(0, true, 0.5, back_at + i * 10 * MS);
            assert_eq!(d, None, "only {}ms of pushing on the client", i * 10);
        }
        assert!(s.client_left(0, true, 0.5, back_at + 200 * MS).is_some());
    }

    #[test]
    fn custom_shortcuts_replace_the_defaults() {
        let mut s = switcher();
        s.set_hotkeys(Hotkeys {
            lock: "Ctrl+Shift+L".parse().unwrap(),
        });
        let now = Instant::now();
        // The old default no longer does anything...
        assert_eq!(s.handle(key(KEY_SCROLLLOCK, true), now, right_only).lock_changed, None);
        s.handle(key(KEY_SCROLLLOCK, false), now, right_only);
        // ...the new one does, with exactly its modifiers.
        s.handle(key(KEY_LEFTCTRL, true), now, right_only);
        assert_eq!(
            s.handle(key(L, true), now, right_only).lock_changed,
            None,
            "Shift missing"
        );
        s.handle(key(L, false), now, right_only);
        s.handle(key(KEY_LEFTSHIFT, true), now, right_only);
        assert_eq!(s.handle(key(L, true), now, right_only).lock_changed, Some(true));
    }

    #[test]
    fn default_lock_is_ctrl_shift_l() {
        let mut s = Switcher::new(SwitchSettings::default(), Hotkeys::default());
        let now = Instant::now();
        s.handle(key(KEY_LEFTCTRL, true), now, right_only);
        s.handle(key(KEY_LEFTSHIFT, true), now, right_only);
        let d = s.handle(key(L, true), now, right_only);
        assert_eq!(d.lock_changed, Some(true));
        assert!(d.block, "the L doesn't also type into the focused app");
    }

    #[test]
    fn shortcuts_can_be_disabled() {
        let mut s = switcher();
        s.set_hotkeys(Hotkeys { lock: Hotkey::NONE });
        let d = s.handle(key(KEY_SCROLLLOCK, true), Instant::now(), right_only);
        assert_eq!(d, Decision::default(), "Scroll Lock is just a key again");
    }

    #[test]
    fn missed_key_releases_dont_break_shortcuts() {
        // Ctrl+Shift+Esc opens Task Manager, an admin window: the releases go
        // there and are never seen, so Ctrl and Shift look held forever.
        let mut s = switcher();
        let now = Instant::now();
        s.handle(key(KEY_LEFTCTRL, true), now, right_only);
        s.handle(key(KEY_LEFTSHIFT, true), now, right_only);
        s.handle(key(1, true), now, right_only); // Esc
                                                 // ...releases lost. Exact matching now fails for Scroll Lock:
        assert_eq!(s.handle(key(KEY_SCROLLLOCK, true), now, right_only).lock_changed, None);
        s.handle(key(KEY_SCROLLLOCK, false), now, right_only);

        // Capture reports what the OS actually has held (nothing), and the
        // shortcut works again.
        s.handle(Captured::Modifiers(Mods::default()), now, right_only);
        assert_eq!(
            s.handle(key(KEY_SCROLLLOCK, true), now, right_only).lock_changed,
            Some(true)
        );
    }

    #[test]
    fn modifier_sync_keeps_modifiers_that_are_really_held() {
        let mut s = Switcher::new(SwitchSettings::default(), Hotkeys::default());
        let now = Instant::now();
        s.handle(key(KEY_LEFTCTRL, true), now, right_only);
        s.handle(key(KEY_LEFTSHIFT, true), now, right_only);
        let held = Mods {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        s.handle(Captured::Modifiers(held), now, right_only);
        let d = s.handle(key(L, true), now, right_only);
        assert_eq!(d.lock_changed, Some(true), "Ctrl+Shift+L still works");
    }

    #[test]
    fn recording_captures_the_next_combination() {
        let mut s = switcher();
        let now = Instant::now();
        s.record_hotkey(true);
        assert_eq!(
            s.handle(key(KEY_LEFTCTRL, true), now, right_only),
            Decision::default(),
            "modifiers pass"
        );
        s.handle(key(KEY_LEFTALT, true), now, right_only);
        let d = s.handle(key(L, true), now, right_only);
        assert!(d.block, "the recorded key doesn't reach apps");
        assert_eq!(d.recorded, Some(Some("Ctrl+Alt+L".parse().unwrap())));
        assert!(!s.recording());
        assert!(s.handle(key(L, false), now, right_only).block, "nor does its release");
    }

    #[test]
    fn escape_cancels_recording() {
        let mut s = switcher();
        s.record_hotkey(true);
        let d = s.handle(key(1, true), Instant::now(), right_only);
        assert_eq!(d.recorded, Some(None));
        assert!(!s.recording());
    }

    #[test]
    fn key_held_while_crossing_releases_locally() {
        let mut s = switcher();
        let now = Instant::now();
        s.handle(key(SHIFT, true), now, right_only);
        enter(&mut s, now);
        assert_eq!(
            s.handle(key(SHIFT, true), now, right_only),
            Decision::default(),
            "autorepeat stays local"
        );
        assert_eq!(
            s.handle(key(SHIFT, false), now, right_only),
            Decision::default(),
            "release reaches this machine"
        );
        // Pressed again on the client side: now it's forwarded.
        assert!(s.handle(key(SHIFT, true), now, right_only).forward.is_some());
    }

    #[test]
    fn force_leave_when_idle_is_noop() {
        let mut s = switcher();
        assert_eq!(s.force_leave(), None);
        enter(&mut s, Instant::now());
        assert_eq!(s.force_leave(), Some(Switch::Leave { peer: 0, warp: None }));
    }

    #[test]
    fn settings_apply_live() {
        let mut s = switcher();
        let now = Instant::now();
        s.handle(button(true), now, right_only);
        assert_eq!(s.handle(edge(Side::Right, 0.5), now, right_only).switch, None);
        s.set_settings(SwitchSettings {
            block_while_dragging: false,
            edge_delay_ms: 0,
            ..Default::default()
        });
        assert!(s.handle(edge(Side::Right, 0.5), now, right_only).switch.is_some());
    }
}
