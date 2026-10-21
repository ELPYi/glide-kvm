//! The glide desktop app. Double-click to open; `--minimized` starts it
//! minimized (used when launching at sign-in).
//!
//! Admin mode: when enabled in Settings, a normal launch hands off to the
//! "glide (administrator)" scheduled task (no UAC prompt) and exits. The
//! task-started copy runs with admin rights. See `platform::windows::admin`.

// No console window behind the app on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glide::config::{self, Config};
use glide::platform::system;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    // Helper modes, run elevated by the app itself (one UAC prompt each).
    if has("--install-admin-task") {
        std::process::exit(exit_code(system::install_admin_task(has("--sign-in"))));
    }
    if has("--remove-admin-task") {
        std::process::exit(exit_code(system::remove_admin_task()));
    }

    // Already running (maybe hidden in the tray)? Bring it forward instead.
    // A copy started by the admin task waits a moment: the copy that
    // launched it may still be closing.
    let from_task = has(system::FROM_TASK_ARG);
    let after_update = has(glide::update::AFTER_UPDATE_ARG);
    let wait = if after_update {
        Duration::from_secs(15) // the old version is still shutting down
    } else if from_task {
        Duration::from_secs(5)
    } else {
        Duration::ZERO
    };
    let Some(instance) = claim_within(wait) else {
        if let Some(window) = system::find_running_window() {
            system::show_window(window);
        }
        return Ok(());
    };

    if after_update {
        system::show_osd(&format!("glide updated to {}", glide::VERSION));
    }
    // Remove the previous version's files left by an update.
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        std::thread::spawn(move || glide::update::clean_up_old(&dir));
    }

    let config_path = config::default_path();
    let launch_file = config_path.with_file_name(config::LAUNCH_FILE);
    let mut minimized = has("--minimized");

    if from_task {
        minimized = take_launch_mode(&launch_file).unwrap_or(true);
    } else if !system::is_elevated() && wants_admin(&config_path) && system::admin_task_installed() {
        let _ = std::fs::write(&launch_file, if minimized { "minimized" } else { "normal" });
        drop(instance); // let the administrator copy claim "running"
        if system::run_admin_task().is_ok() {
            return Ok(()); // the administrator copy takes it from here
        }
        let _ = std::fs::remove_file(&launch_file);
        let Some(instance) = system::claim_single_instance() else {
            return Ok(());
        };
        return run_app(instance, config_path, minimized);
    }
    run_app(instance, config_path, minimized)
}

fn run_app(instance: system::InstanceGuard, config_path: PathBuf, minimized: bool) -> eframe::Result {
    let result = glide::gui::run(config_path, minimized);
    drop(instance); // held for as long as the window runs
    result
}

/// Claim the single-instance slot, retrying for up to `wait`.
fn claim_within(wait: Duration) -> Option<system::InstanceGuard> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(guard) = system::claim_single_instance() {
            return Some(guard);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wants_admin(config_path: &Path) -> bool {
    Config::load(config_path).is_ok_and(|c| c.run_as_admin)
}

/// Read and delete the launch-mode file: `Some(minimized)`, or `None` if
/// there wasn't one (started at sign-in).
fn take_launch_mode(path: &PathBuf) -> Option<bool> {
    let mode = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    Some(mode.trim() == "minimized")
}

fn exit_code(r: anyhow::Result<()>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}
