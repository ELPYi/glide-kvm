//! The app's single stateful component: owns the config file, the running
//! engine and the settings draft, draws the views each frame, and applies
//! the actions they emit.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Frame, Ui, ViewportCommand};

use super::actions::{Action, Actions, HotkeyAction, Tab};
use super::engine_slot::EngineSlot;
use super::theme::{self, Tone};
use super::tray::{self, Tray};
use super::views::{self, devices::DevicesUi, settings::SettingsUi, Model, Notice, ThisPc};
use crate::config::{ClientEntry, Config, Role};
use crate::engine::{self, Engine};
use crate::hotkey::Hotkey;
use crate::input::Side;
use crate::log;
use crate::net::{addr, pairing::Paired};
use crate::platform::system;
use crate::status::EngineStatus;
use crate::update::{self, Updater};

/// Live status is refreshed at least this often.
const REFRESH: Duration = Duration::from_millis(250);
const NOTICE_FOR: Duration = Duration::from_secs(5);
/// Network addresses can change (Wi-Fi switch, VPN up), so re-read them.
const ADDRESS_REFRESH: Duration = Duration::from_secs(10);

/// An administrator-mode change running in the background (it may wait on a
/// UAC prompt).
#[derive(Clone, Copy, Debug)]
enum AdminChange {
    Enable,
    Disable,
    /// Admin mode is on; change whether its task starts glide at sign-in.
    SignIn(bool),
}

/// A client-side "pair with server" attempt running in the background.
struct PairJob {
    address: String,
    result: Receiver<anyhow::Result<Paired>>,
}

pub struct App {
    config_path: PathBuf,
    /// What's on disk and running.
    saved: Config,
    /// What the Settings screen is editing.
    draft: Config,
    engine: EngineSlot,
    error: Option<String>,
    tab: Tab,
    settings_ui: SettingsUi,
    devices_ui: DevicesUi,
    this_pc: ThisPc,
    this_pc_read: Instant,
    autostart: bool,
    firewall_job: Option<Receiver<anyhow::Result<()>>>,
    pair_job: Option<PairJob>,
    /// Which shortcut is being recorded (the engine captures the keys).
    recording_hotkey: Option<HotkeyAction>,
    elevated: bool,
    admin_job: Option<(AdminChange, Receiver<anyhow::Result<()>>)>,
    /// Close the window at the end of this frame (handing off to the admin copy).
    close_requested: bool,
    notice: Option<(Notice, Instant)>,
    /// Start hidden in the tray (or minimized, if there's no tray).
    hide_on_first_frame: bool,
    /// The OS window handle, for showing the window from the tray.
    window: Option<isize>,
    /// `None` if the tray icon couldn't be created (then closing quits).
    tray: Option<Tray>,
    updater: Arc<Updater>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, config_path: PathBuf, minimized: bool) -> Self {
        theme::apply(&cc.egui_ctx);
        let (saved, error) = load_or_create(&config_path);
        let saved_for_autostart = saved.clone();
        let engine = EngineSlot::default();
        let window = window_handle(cc);
        let updater = start_updater(&saved, engine.clone(), window);
        let mut app = App {
            config_path,
            draft: saved.clone(),
            saved,
            engine,
            error,
            tab: Tab::Devices,
            settings_ui: SettingsUi::default(),
            devices_ui: DevicesUi::default(),
            this_pc: read_this_pc(),
            this_pc_read: Instant::now(),
            autostart: autostart_state(&saved_for_autostart),
            firewall_job: None,
            pair_job: None,
            recording_hotkey: None,
            elevated: system::is_elevated(),
            admin_job: None,
            close_requested: false,
            notice: None,
            hide_on_first_frame: minimized,
            window,
            tray: None,
            updater,
        };
        app.set_up_sign_in();
        if let Some(w) = app.window {
            match tray::install(cc.egui_ctx.clone(), w, app.engine.clone()) {
                Ok(t) => app.tray = Some(t),
                Err(e) => log!("no tray icon: {e:#}"),
            }
        }
        if app.error.is_none() && app.saved.role.is_some() && app.saved.start_on_open {
            app.start();
        }
        app
    }

    fn start(&mut self) {
        let Some(role) = self.saved.role else { return };
        self.stop();
        match Engine::start(&self.saved, role) {
            Ok(e) => {
                self.engine.set(e);
                self.error = None;
            }
            Err(e) => {
                let msg = format!("{e:#}");
                log!("couldn't start: {msg}");
                self.error = Some(msg);
            }
        }
    }

    fn stop(&mut self) {
        self.engine.stop();
    }

    /// The first time a release build runs, have it launch at sign-in (so
    /// sharing and updates just work). After that it's the user's choice.
    fn set_up_sign_in(&mut self) {
        if !crate::is_release_build() || self.saved.sign_in_set_up || self.saved.role.is_none() {
            return;
        }
        if !self.saved.run_as_admin && !self.autostart {
            match system::set_autostart(true) {
                Ok(()) => log!("glide will now launch when you sign in (change this in Settings)"),
                Err(e) => log!("couldn't set up launching at sign-in: {e:#}"),
            }
            self.autostart = autostart_state(&self.saved);
        }
        self.saved.sign_in_set_up = true;
        self.draft.sign_in_set_up = true;
        self.save();
    }

    fn notify(&mut self, text: impl Into<String>, tone: Tone) {
        self.notice = Some((
            Notice {
                text: text.into(),
                tone,
            },
            Instant::now(),
        ));
    }

    fn save(&mut self) -> bool {
        match self.saved.save(&self.config_path) {
            Ok(()) => true,
            Err(e) => {
                self.notify(format!("Couldn't save: {e:#}"), Tone::Bad);
                false
            }
        }
    }

    fn save_settings(&mut self) {
        let role = self.draft.role.unwrap_or(Role::Server);
        if let Err(e) = self.draft.validate(role) {
            self.notify(format!("{e}"), Tone::Warn);
            return;
        }
        let restart = needs_restart(&self.saved, &self.draft);
        let old = std::mem::replace(&mut self.saved, self.draft.clone());
        if !self.save() {
            self.saved = old;
            return;
        }
        let running = self.engine.is_running();
        if running && !restart {
            let saved = &self.saved;
            self.engine.with(|e| {
                e.set_switching(saved.switching);
                e.set_clipboard(saved.clipboard);
                e.set_hotkeys(saved.hotkeys);
                e.set_games(saved.games.clone());
                let _ = e.set_layout(&sides(&saved.clients));
            });
        } else if running || self.error.is_some() {
            self.start(); // restart, or retry after fixing the problem
        }
        self.notify(
            if running && restart {
                "Saved and restarted"
            } else {
                "Saved"
            },
            Tone::Good,
        );
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Start => self.start(),
            Action::Stop => {
                self.stop();
                self.error = None;
            }
            Action::ChooseRole(role) => {
                self.saved.role = Some(role);
                self.draft.role = Some(role);
                self.save();
                self.tab = Tab::Devices;
                self.start();
                // A new sharing PC's next step is always pairing, so start it.
                if role == Role::Server {
                    self.apply(Action::StartPairing { poke: None });
                }
            }
            Action::SaveSettings => self.save_settings(),
            Action::DiscardSettings => self.draft = self.saved.clone(),
            Action::GenerateSecret => self.draft.secret = crate::net::crypto::generate_secret(),
            Action::SetAutostart(on) if self.saved.run_as_admin => self.start_admin_job(AdminChange::SignIn(on)),
            Action::SetRunAsAdmin(on) => {
                self.start_admin_job(if on { AdminChange::Enable } else { AdminChange::Disable })
            }
            Action::SetAutostart(on) => match system::set_autostart(on) {
                Ok(()) => {
                    self.autostart = system::autostart_enabled();
                    let msg = if on {
                        "glide will launch when you sign in"
                    } else {
                        "glide won't launch at sign-in"
                    };
                    self.notify(msg, Tone::Good);
                }
                Err(e) => self.notify(format!("{e:#}"), Tone::Bad),
            },
            Action::OpenFirewall => {
                let port = self.saved.port;
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(system::open_firewall(port));
                });
                self.firewall_job = Some(rx);
            }
            Action::ClearLogs => crate::logging::clear(),
            Action::GoTo(tab) => self.tab = tab,
            Action::StartPairing { poke } => match self.engine.with(|e| e.start_pairing(poke.as_deref())) {
                None => self.notify("Start glide first", Tone::Warn),
                Some(Err(e)) => self.notify(format!("{e:#}"), Tone::Warn),
                Some(Ok(_)) => {}
            },
            Action::CancelPairing => {
                self.engine.with(|e| e.cancel_pairing());
            }
            Action::PairWithServer { address, code } => {
                // Pairing needs glide's port, so pause the client while it runs.
                self.stop();
                let port = self.saved.port;
                let (tx, rx) = mpsc::channel();
                let addr = address.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(engine::pair_with_server(port, &addr, &code));
                });
                self.pair_job = Some(PairJob { address, result: rx });
            }
            Action::RecordHotkey(which) => match self.engine.with(|e| e.record_hotkey(which.is_some())) {
                None => self.notify("Start glide first, then record the shortcut", Tone::Warn),
                Some(Ok(())) => self.recording_hotkey = which,
                Some(Err(e)) => self.notify(format!("{e:#}"), Tone::Warn),
            },
            Action::SetHotkey(which, hotkey) => self.set_hotkey(which, hotkey),
            Action::SetAutoUpdate(on) => {
                self.saved.auto_update = on;
                self.draft.auto_update = on;
                if self.save() {
                    self.updater.set_enabled(on);
                }
            }
            Action::CheckForUpdates => self.updater.check_now(),
            Action::InstallUpdate => self.updater.install_now(),
            Action::SetAudio { settings, save } => {
                self.saved.audio = settings;
                self.draft.audio = settings;
                self.engine.with(|e| e.set_audio(settings));
                if save {
                    self.save();
                }
            }
            Action::SetDeviceMuted { index, muted } => {
                if let Some(c) = self.saved.clients.get_mut(index) {
                    c.muted = muted;
                    if let Some(d) = self.draft.clients.get_mut(index) {
                        d.muted = muted;
                    }
                    if self.save() {
                        self.engine.with(|e| e.set_device_muted(index, muted));
                    }
                }
            }
            Action::RenameDevice { index, name } => {
                let name = name.trim().to_string();
                self.change_device(index, "Renamed", |clients| clients[index].name = name.clone());
            }
            Action::RemoveDevice { index } => {
                self.change_device(index, "Removed", |clients| {
                    clients.remove(index);
                });
            }
            Action::MoveDevice { index, side } => {
                if move_device(&mut self.saved.clients, index, side) && self.save() {
                    // Keep the settings draft's sides in step (names/addresses untouched).
                    if self.draft.clients.len() == self.saved.clients.len() {
                        for (d, s) in self.draft.clients.iter_mut().zip(&self.saved.clients) {
                            d.side = s.side;
                        }
                    }
                    let sides = sides(&self.saved.clients);
                    self.engine.with(|e| e.set_layout(&sides));
                }
            }
        }
    }

    /// Rename or remove a device from its card: saved at once (the settings
    /// draft follows) and sharing restarts to pick it up.
    fn change_device(&mut self, index: usize, done: &str, change: impl Fn(&mut Vec<ClientEntry>)) {
        if index >= self.saved.clients.len() {
            return;
        }
        let old = self.saved.clone();
        change(&mut self.saved.clients);
        if !self.save() {
            self.saved = old;
            return;
        }
        if self.draft.clients.len() == old.clients.len() {
            change(&mut self.draft.clients);
        }
        if self.engine.is_running() {
            self.start();
        }
        self.notify(done, Tone::Good);
    }

    /// Change one shortcut, save it and apply it live.
    fn set_hotkey(&mut self, which: HotkeyAction, hotkey: Hotkey) {
        let mut hotkeys = self.saved.hotkeys;
        match which {
            HotkeyAction::Lock => hotkeys.lock = hotkey,
        }
        self.saved.hotkeys = hotkeys;
        self.draft.hotkeys = hotkeys;
        if self.save() {
            self.engine.with(|e| e.set_hotkeys(hotkeys));
            let msg = if hotkey.is_none() {
                "Shortcut turned off".to_string()
            } else {
                format!("Shortcut set to {hotkey}")
            };
            self.notify(msg, Tone::Good);
        }
    }

    /// Run an admin-mode change off the UI thread. Without admin rights it
    /// goes through a UAC prompt (a short-lived elevated copy of glide).
    fn start_admin_job(&mut self, change: AdminChange) {
        let elevated = self.elevated;
        let sign_in = match change {
            AdminChange::SignIn(on) => on,
            _ => self.autostart,
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = match (change, elevated) {
                (AdminChange::Disable, true) => system::remove_admin_task(),
                (AdminChange::Disable, false) => system::run_elevated("--remove-admin-task"),
                (_, true) => system::install_admin_task(sign_in),
                (_, false) => {
                    let flag = if sign_in { " --sign-in" } else { "" };
                    system::run_elevated(&format!("--install-admin-task{flag}"))
                }
            };
            let _ = tx.send(result);
        });
        self.admin_job = Some((change, rx));
    }

    fn finish_admin_job(&mut self, change: AdminChange, result: anyhow::Result<()>) {
        if let Err(e) = result {
            self.notify(format!("{e:#}"), Tone::Bad);
            return;
        }
        match change {
            AdminChange::Enable => {
                let sign_in = self.autostart;
                self.saved.run_as_admin = true;
                self.draft.run_as_admin = true;
                self.save();
                // The admin task now handles sign-in; drop the plain entry.
                let _ = system::set_autostart(false);
                self.autostart = sign_in;
                if self.elevated {
                    self.notify("Administrator mode is on", Tone::Good);
                } else {
                    self.relaunch_as_admin();
                }
            }
            AdminChange::Disable => {
                let sign_in = self.autostart;
                self.saved.run_as_admin = false;
                self.draft.run_as_admin = false;
                self.save();
                if sign_in {
                    let _ = system::set_autostart(true);
                }
                self.autostart = autostart_state(&self.saved);
                self.notify("glide will start without admin rights from the next launch", Tone::Good);
            }
            AdminChange::SignIn(on) => {
                self.autostart = autostart_state(&self.saved);
                let msg = if on {
                    "glide will launch when you sign in"
                } else {
                    "glide won't launch at sign-in"
                };
                self.notify(msg, Tone::Good);
            }
        }
    }

    /// Hand over to the administrator copy: free the port, start the task, close.
    fn relaunch_as_admin(&mut self) {
        self.stop();
        let launch_file = self.config_path.with_file_name(crate::config::LAUNCH_FILE);
        let _ = std::fs::write(&launch_file, "normal");
        match system::run_admin_task() {
            Ok(()) => self.close_requested = true,
            Err(e) => {
                let _ = std::fs::remove_file(&launch_file);
                self.notify(format!("Couldn't restart as administrator: {e:#}"), Tone::Bad);
                self.start();
            }
        }
    }

    fn poll_background_jobs(&mut self) {
        if let Some((change, rx)) = &self.admin_job {
            if let Ok(result) = rx.try_recv() {
                let change = *change;
                self.admin_job = None;
                self.finish_admin_job(change, result);
            }
        }
        if let Some(rx) = &self.firewall_job {
            if let Ok(result) = rx.try_recv() {
                self.firewall_job = None;
                match result {
                    Ok(()) => self.notify("Firewall rule added", Tone::Good),
                    Err(e) => self.notify(format!("{e:#}"), Tone::Bad),
                }
            }
        }
        if let Some(job) = &self.pair_job {
            if let Ok(result) = job.result.try_recv() {
                let address = job.address.clone();
                self.pair_job = None;
                match result {
                    Ok(paired) => {
                        self.saved.role = Some(Role::Client);
                        self.saved.secret = paired.secret;
                        self.saved.server_address = Some(server_address(&address, paired.port, self.saved.port));
                        self.draft = self.saved.clone();
                        self.save();
                        self.notify(format!("Paired with {address}"), Tone::Good);
                    }
                    Err(e) => self.notify(format!("Pairing failed: {e}"), Tone::Bad),
                }
                self.start();
            }
        }
        // A shortcut was recorded: it applies right away.
        if let Some(which) = self.recording_hotkey {
            match self.engine.with(Engine::take_recorded_hotkey).flatten() {
                Some(Some(hotkey)) => {
                    self.recording_hotkey = None;
                    self.apply(Action::SetHotkey(which, hotkey));
                }
                Some(None) => self.recording_hotkey = None,
                None if !self.engine.is_running() => self.recording_hotkey = None,
                None => {}
            }
        }
        // Devices the server just paired: save them.
        let added = self.engine.with(Engine::take_paired).unwrap_or_default();
        if !added.is_empty() {
            for entry in added {
                self.notify(format!("Paired with {}", entry.name), Tone::Good);
                if merge_paired(&mut self.saved.clients, entry.clone()) {
                    merge_paired(&mut self.draft.clients, entry);
                }
            }
            self.save();
        }
        if self.this_pc_read.elapsed() > ADDRESS_REFRESH {
            self.this_pc = read_this_pc();
            self.this_pc_read = Instant::now();
        }
        if self.notice.as_ref().is_some_and(|(_, t)| t.elapsed() > NOTICE_FOR) {
            self.notice = None;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.hide_on_first_frame) {
            let cmd = if self.tray.is_some() {
                ViewportCommand::Visible(false)
            } else {
                ViewportCommand::Minimized(true)
            };
            ui.ctx().send_viewport_cmd(cmd);
        }
        // The window's X: hide to the tray (sharing keeps running) unless
        // we're really quitting or the user turned this off.
        if ui.ctx().input(|i| i.viewport().close_requested())
            && !self.close_requested
            && self.saved.close_to_tray
            && self.tray.is_some()
        {
            ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
            // Hide through the OS, the same way the tray shows it again. Going
            // through the window library here would leave it believing the
            // window is still hidden after the tray reopens it, and every
            // later X would then do nothing.
            match self.window {
                Some(w) => system::hide_window(w),
                None => ui.ctx().send_viewport_cmd(ViewportCommand::Visible(false)),
            }
        }
        self.poll_background_jobs();

        let status = self.engine.with(Engine::status).unwrap_or(EngineStatus::Stopped);
        let mut actions = Actions::default();
        let model = Model {
            status: &status,
            running: self.engine.is_running(),
            saved: &self.saved,
            error: self.error.as_deref(),
            autostart: self.autostart,
            firewall_busy: self.firewall_job.is_some(),
            tab: self.tab,
            notice: self.notice.as_ref().map(|(n, _)| n),
            this_pc: &self.this_pc,
            pairing_busy: self.pair_job.is_some(),
            recording_hotkey: self.recording_hotkey,
            elevated: self.elevated,
            admin_busy: self.admin_job.is_some(),
            update: self.updater.state(),
        };

        let page = Frame::new()
            .inner_margin(theme::page_margin())
            .fill(ui.visuals().panel_fill);
        egui::Panel::top("header")
            .frame(page)
            .show(ui, |ui| views::header::show(ui, &model, &mut actions));
        egui::CentralPanel::default()
            .frame(page)
            .show(ui, |ui| match (self.saved.role, self.tab) {
                (None, _) => views::welcome::show(ui, &mut actions),
                (Some(_), Tab::Devices) => views::devices::show(ui, &model, &mut self.devices_ui, &mut actions),
                (Some(_), Tab::Settings) => {
                    views::settings::show(ui, &model, &mut self.draft, &mut self.settings_ui, &mut actions)
                }
                (Some(_), Tab::Logs) => views::logs::show(ui, &mut actions),
            });

        for a in actions.drain() {
            self.apply(a);
        }
        if self.close_requested {
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        }
        ui.ctx().request_repaint_after(REFRESH);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Closing the window stops sharing and restores normal input.
        self.stop();
    }
}

/// Start the background updater. It restarts glide into a new version only
/// while nobody is using this PC and it isn't controlling another one.
fn start_updater(cfg: &Config, engine: EngineSlot, window: Option<isize>) -> Arc<Updater> {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_default();
    let idle_engine = engine.clone();
    update::start(update::UpdaterOptions {
        enabled: cfg.auto_update,
        dir,
        is_idle: Box::new(move || {
            let controlling = matches!(
                idle_engine.with(Engine::status),
                Some(EngineStatus::Server(s)) if s.active.is_some()
            );
            !controlling && system::idle_for() >= update::IDLE_BEFORE_INSTALL
        }),
        restart: Box::new(move |version| {
            let hidden = window.is_some_and(|w| !system::window_visible(w));
            let mut args = vec![update::AFTER_UPDATE_ARG];
            if hidden {
                args.push("--minimized");
            }
            let exe = match std::env::current_exe() {
                Ok(exe) => exe,
                Err(e) => return log!("can't restart into {version}: {e}"),
            };
            engine.stop(); // restore normal input and free the port first
            match std::process::Command::new(exe).args(&args).spawn() {
                Ok(_) => std::process::exit(0),
                // The new files are in place; they'll run next launch.
                Err(e) => log!("couldn't restart into {version}: {e}"),
            }
        }),
    })
}

/// Whether glide starts at sign-in: via the admin task in admin mode,
/// otherwise via the normal per-user startup entry.
fn autostart_state(cfg: &Config) -> bool {
    if cfg.run_as_admin {
        system::admin_task_starts_at_sign_in()
    } else {
        system::autostart_enabled()
    }
}

/// The OS handle of the app window (an HWND, or on a Mac its NSView).
fn window_handle(cc: &eframe::CreationContext) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as isize),
        _ => None,
    }
}

fn read_this_pc() -> ThisPc {
    ThisPc {
        name: addr::machine_name(),
        addrs: addr::local_addresses(),
    }
}

/// Load `path`, or create it with a fresh secret on first run.
fn load_or_create(path: &std::path::Path) -> (Config, Option<String>) {
    if path.exists() {
        match Config::load(path) {
            Ok(c) => (c, None),
            Err(e) => (
                Config::new_default(),
                Some(format!("{e:#} (fix or delete the file, then restart glide)")),
            ),
        }
    } else {
        let c = Config::new_default();
        let err = c.save(path).err().map(|e| format!("{e:#}"));
        if err.is_none() {
            log!("created {}", path.display());
        }
        (c, err)
    }
}

/// What to store as the server's address: include its port only if it
/// differs from ours (each PC keeps its own port).
pub fn server_address(typed: &str, server_port: u16, our_port: u16) -> String {
    let typed = typed.trim();
    if server_port == our_port || typed.contains(':') {
        typed.to_string()
    } else {
        format!("{typed}:{server_port}")
    }
}

fn sides(clients: &[ClientEntry]) -> Vec<Side> {
    clients.iter().map(|c| c.side).collect()
}

/// Put device `index` on `side`, swapping with any device already there.
/// Returns whether anything changed.
pub fn move_device(clients: &mut [ClientEntry], index: usize, side: Side) -> bool {
    let Some(old) = clients.get(index).map(|c| c.side) else {
        return false;
    };
    if old == side {
        return false;
    }
    if let Some(other) = clients.iter_mut().find(|c| c.side == side) {
        other.side = old;
    }
    clients[index].side = side;
    true
}

/// Add a freshly paired device unless one with that address exists.
/// Returns whether it was added.
pub fn merge_paired(clients: &mut Vec<ClientEntry>, entry: ClientEntry) -> bool {
    if clients.iter().any(|c| c.address == entry.address) {
        return false;
    }
    clients.push(entry);
    true
}

/// Whether going from `old` to `new` requires restarting the engine, as
/// opposed to applying live (switching settings, sides) or not affecting it.
pub fn needs_restart(old: &Config, new: &Config) -> bool {
    let strip = |c: &Config| {
        let mut c = Config {
            switching: Default::default(),
            hotkeys: Default::default(),
            games: Default::default(),
            audio: Default::default(),
            start_on_open: false,
            run_as_admin: false,
            close_to_tray: false,
            auto_update: false,
            sign_in_set_up: false,
            clipboard: false,
            ..c.clone()
        };
        for e in &mut c.clients {
            e.side = Side::Left;
            e.muted = false;
        }
        c
    };
    strip(old) != strip(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SwitchSettings;

    fn entry(name: &str, side: Side) -> ClientEntry {
        ClientEntry {
            name: name.into(),
            address: format!("10.0.0.{}", name.len()),
            side,
            muted: false,
        }
    }

    fn base() -> Config {
        Config {
            role: Some(Role::Server),
            clients: vec![entry("a", Side::Right)],
            ..Config::new_default_with_secret("0123456789abcdef0123")
        }
    }

    #[test]
    fn live_changes_dont_restart() {
        let old = base();
        let mut new = old.clone();
        new.switching = SwitchSettings {
            block_while_dragging: false,
            edge_delay_ms: 200,
            match_pointer_feel: false,
            mouse_rate_hz: 4000,
            keyboard_follows_click: false,
        };
        new.run_as_admin = true;
        new.start_on_open = !old.start_on_open;
        new.clients[0].side = Side::Left;
        assert!(!needs_restart(&old, &new));
    }

    #[test]
    fn connection_changes_restart() {
        let old = base();
        for change in [
            |c: &mut Config| c.port += 1,
            |c: &mut Config| c.secret.push('x'),
            |c: &mut Config| c.role = Some(Role::Client),
            |c: &mut Config| c.clients[0].address = "10.0.0.3".into(),
            |c: &mut Config| c.clients.clear(),
            |c: &mut Config| c.server_address = Some("x".into()),
        ] {
            let mut new = old.clone();
            change(&mut new);
            assert!(needs_restart(&old, &new), "{new:?}");
        }
    }

    #[test]
    fn moving_a_device_swaps_with_the_occupant() {
        let mut c = vec![entry("a", Side::Right), entry("bb", Side::Left)];
        assert!(move_device(&mut c, 0, Side::Left));
        assert_eq!((c[0].side, c[1].side), (Side::Left, Side::Right));
        assert!(move_device(&mut c, 1, Side::Top));
        assert_eq!((c[0].side, c[1].side), (Side::Left, Side::Top));
        assert!(!move_device(&mut c, 1, Side::Top), "no-op");
        assert!(!move_device(&mut c, 9, Side::Top), "bad index");
    }

    #[test]
    fn server_port_is_kept_only_when_different() {
        assert_eq!(server_address(" 192.168.1.2 ", 24801, 24801), "192.168.1.2");
        assert_eq!(server_address("192.168.1.2", 25000, 24801), "192.168.1.2:25000");
        assert_eq!(server_address("192.168.1.2:9", 25000, 24801), "192.168.1.2:9");
    }

    #[test]
    fn paired_devices_are_not_duplicated() {
        let mut c = vec![entry("a", Side::Right)];
        assert!(!merge_paired(&mut c, entry("a", Side::Left)), "same address = re-pair");
        assert!(merge_paired(&mut c, entry("bb", Side::Left)));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn first_run_creates_a_config() {
        let dir = std::env::temp_dir().join(format!("glide-test-{}", crate::now_micros()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("glide.toml");
        let (c, err) = load_or_create(&path);
        assert!(err.is_none());
        assert!(path.exists());
        let (again, err) = load_or_create(&path);
        assert!(err.is_none());
        assert_eq!(again, c, "second launch reads the same file");

        std::fs::write(&path, "not = [valid").unwrap();
        let (_, err) = load_or_create(&path);
        assert!(err.is_some(), "a broken file is reported, not silently replaced");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
