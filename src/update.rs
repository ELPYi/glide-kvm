//! Over-the-air updates from GitHub Releases.
//!
//! CI publishes each release with `glide.exe`, `glide-cli.exe`, the Mac's
//! `glide-macos` and a small `latest.toml` manifest (version + SHA-256 of
//! each file). On a Mac only the program inside `glide.app` is replaced; the
//! app around it never changes, so its signature (and the permissions macOS
//! gave it) stay valid. A release build
//! checks the manifest shortly after starting and every 30 minutes. When a newer
//! version exists it downloads it next to itself (`*.new`), verifies the
//! checksums, then waits until you're idle (not using this PC, not
//! controlling another) to swap the files and restart itself.
//!
//! Local ("dev") builds never update themselves.
//!
//! Pure parts (version comparison, manifest, staging/installing files) are
//! separate from the background thread, and tested without the network.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::log;

pub const MANIFEST: &str = "latest.toml";
#[cfg(windows)]
pub const APP_EXE: &str = "glide.exe";
#[cfg(windows)]
pub const CLI_EXE: &str = "glide-cli.exe";
#[cfg(not(windows))]
pub const APP_EXE: &str = "glide";
#[cfg(not(windows))]
pub const CLI_EXE: &str = "glide-cli";
/// This OS's files in a release.
#[cfg(target_os = "macos")]
const APP_ASSET: &str = "glide-macos";
#[cfg(target_os = "macos")]
const CLI_ASSET: &str = "glide-cli-macos";
#[cfg(not(target_os = "macos"))]
const APP_ASSET: &str = APP_EXE;
#[cfg(not(target_os = "macos"))]
const CLI_ASSET: &str = CLI_EXE;
/// Passed to the new version when it's started after an update.
pub const AFTER_UPDATE_ARG: &str = "--after-update";

const FIRST_CHECK_AFTER: Duration = Duration::from_secs(20);
const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(10 * 60);
/// Install only after this long with no input and no remote control.
pub const IDLE_BEFORE_INSTALL: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_secs(1);

/// `latest.toml`, written by CI next to the release files.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub glide_sha256: String,
    pub cli_sha256: String,
    /// Empty when that release has no Mac build.
    #[serde(default)]
    pub macos_glide_sha256: String,
    #[serde(default)]
    pub macos_cli_sha256: String,
}

impl Manifest {
    /// Checksums of this OS's (app, command-line tool).
    fn checksums(&self) -> (&str, &str) {
        if cfg!(target_os = "macos") {
            (&self.macos_glide_sha256, &self.macos_cli_sha256)
        } else {
            (&self.glide_sha256, &self.cli_sha256)
        }
    }
}

pub fn manifest_url(repo: &str) -> String {
    format!("https://github.com/{repo}/releases/latest/download/{MANIFEST}")
}

pub fn asset_url(repo: &str, version: &str, file: &str) -> String {
    format!("https://github.com/{repo}/releases/download/v{version}/{file}")
}

/// Is `candidate` a newer dotted version than `current` ("0.4.12" > "0.4.9")?
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<u64>> { v.split('.').map(|p| p.parse().ok()).collect() };
    match (parse(candidate), parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Write a verified download next to `target` as `target.new`.
pub fn stage(target: &Path, bytes: &[u8], expected_sha256: &str) -> Result<PathBuf> {
    let actual = sha256_hex(bytes);
    if !actual.eq_ignore_ascii_case(expected_sha256.trim()) {
        bail!("download of {} is corrupt (checksum mismatch)", target.display());
    }
    let staged = with_suffix(target, ".new");
    std::fs::write(&staged, bytes).with_context(|| format!("writing {}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(staged)
}

/// Swap `target.new` into place. The running program can't be deleted, but
/// Windows allows renaming it, so it becomes `target.old` (removed on the
/// next start). A Mac allows either.
pub fn install(target: &Path) -> Result<()> {
    let staged = with_suffix(target, ".new");
    // A Mac replaces the file in one step (the running copy carries on from
    // the old one), leaving nothing extra inside the signed app.
    if cfg!(unix) {
        return std::fs::rename(&staged, target).with_context(|| format!("installing {}", target.display()));
    }
    let old = with_suffix(target, ".old");
    let _ = std::fs::remove_file(&old);
    if target.exists() {
        std::fs::rename(target, &old).with_context(|| format!("moving {} aside", target.display()))?;
    }
    if let Err(e) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&old, target); // put the working version back
        return Err(e).with_context(|| format!("installing {}", target.display()));
    }
    Ok(())
}

/// Remove `*.old` leftovers from an update (they may still be closing).
pub fn clean_up_old(dir: &Path) {
    for file in [APP_EXE, CLI_EXE] {
        let old = with_suffix(&dir.join(file), ".old");
        for _ in 0..20 {
            if !old.exists() || std::fs::remove_file(&old).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

// ------------------------------------------------------------------ background

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateState {
    /// Local build: never updates.
    DevBuild,
    TurnedOff,
    Waiting,
    Checking,
    UpToDate,
    Downloading(String),
    /// Downloaded and verified; installs when you're idle.
    ReadyToInstall(String),
    Failed(String),
}

impl UpdateState {
    pub fn describe(&self) -> String {
        match self {
            UpdateState::DevBuild => "Local build: doesn't update itself".into(),
            UpdateState::TurnedOff => "Automatic updates are off".into(),
            UpdateState::Waiting => "Will check for updates shortly".into(),
            UpdateState::Checking => "Checking for updates...".into(),
            UpdateState::UpToDate => "Up to date".into(),
            UpdateState::Downloading(v) => format!("Downloading {v}..."),
            UpdateState::ReadyToInstall(v) => format!("{v} is ready; installs when you're away for a minute, or now"),
            UpdateState::Failed(e) => format!("Update check failed: {e}"),
        }
    }
}

pub struct UpdaterOptions {
    pub enabled: bool,
    /// Folder holding glide.exe (and glide-cli.exe).
    pub dir: PathBuf,
    /// Is it a good moment to restart? (No input for a while, not in control.)
    pub is_idle: Box<dyn Fn() -> bool + Send + Sync>,
    /// Restart into the new version (called after the files are swapped).
    pub restart: Box<dyn Fn(&str) + Send + Sync>,
}

pub struct Updater {
    state: Mutex<UpdateState>,
    check_now: AtomicBool,
    /// Install the downloaded version without waiting for idle.
    install_now: AtomicBool,
    enabled: AtomicBool,
}

impl Updater {
    pub fn state(&self) -> UpdateState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn check_now(&self) {
        self.check_now.store(true, Ordering::Relaxed);
    }

    /// Install the downloaded update right away (glide restarts).
    pub fn install_now(&self) {
        self.install_now.store(true, Ordering::Relaxed);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if crate::is_release_build() {
            self.set(if on {
                UpdateState::Waiting
            } else {
                UpdateState::TurnedOff
            });
            if on {
                self.check_now();
            }
        }
    }

    fn set(&self, s: UpdateState) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }
}

/// Start the background updater (it does nothing in dev builds).
pub fn start(opts: UpdaterOptions) -> Arc<Updater> {
    let release = crate::is_release_build();
    let initial = match (release, opts.enabled) {
        (false, _) => UpdateState::DevBuild,
        (true, false) => UpdateState::TurnedOff,
        (true, true) => UpdateState::Waiting,
    };
    let updater = Arc::new(Updater {
        state: Mutex::new(initial),
        check_now: AtomicBool::new(false),
        install_now: AtomicBool::new(false),
        enabled: AtomicBool::new(opts.enabled),
    });
    if release {
        let u = updater.clone();
        let _ = std::thread::Builder::new()
            .name("glide-update".into())
            .spawn(move || run(&u, &opts));
    }
    updater
}

fn run(u: &Updater, opts: &UpdaterOptions) {
    let mut next_check = Instant::now() + FIRST_CHECK_AFTER;
    let mut ready: Option<String> = None;
    loop {
        std::thread::sleep(TICK);
        let enabled = u.enabled.load(Ordering::Relaxed);
        if let Some(version) = &ready {
            let asked = u.install_now.swap(false, Ordering::Relaxed);
            if enabled && (asked || (opts.is_idle)()) {
                match install_all(&opts.dir) {
                    Ok(()) => {
                        log!("installed {version}; restarting");
                        (opts.restart)(version);
                        return;
                    }
                    Err(e) => {
                        log!("couldn't install the update: {e:#}");
                        u.set(UpdateState::Failed(format!("{e:#}")));
                        ready = None;
                        next_check = Instant::now() + RETRY_AFTER_FAILURE;
                    }
                }
            }
            continue;
        }
        let asked = u.check_now.swap(false, Ordering::Relaxed);
        if !enabled || (!asked && Instant::now() < next_check) {
            continue;
        }
        u.set(UpdateState::Checking);
        match check_and_download(u, &opts.dir) {
            Ok(Some(version)) => {
                u.set(UpdateState::ReadyToInstall(version.clone()));
                ready = Some(version);
            }
            Ok(None) => {
                u.set(UpdateState::UpToDate);
                next_check = Instant::now() + CHECK_EVERY;
            }
            Err(e) => {
                log!("update check failed: {e:#}");
                u.set(UpdateState::Failed(format!("{e:#}")));
                next_check = Instant::now() + RETRY_AFTER_FAILURE;
            }
        }
    }
}

/// Returns the version staged, or None if already up to date.
fn check_and_download(u: &Updater, dir: &Path) -> Result<Option<String>> {
    let repo = crate::UPDATE_REPO;
    let text = crate::platform::http_get(&manifest_url(repo))?;
    let manifest: Manifest = toml::from_str(std::str::from_utf8(&text)?).context("reading the release manifest")?;
    let (app_sha, cli_sha) = manifest.checksums();
    if !is_newer(&manifest.version, crate::VERSION) || app_sha.is_empty() {
        return Ok(None);
    }
    u.set(UpdateState::Downloading(manifest.version.clone()));
    log!("downloading glide {}", manifest.version);
    let app = crate::platform::http_get(&asset_url(repo, &manifest.version, APP_ASSET))?;
    stage(&dir.join(APP_EXE), &app, app_sha)?;
    if dir.join(CLI_EXE).exists() && !cli_sha.is_empty() {
        let cli = crate::platform::http_get(&asset_url(repo, &manifest.version, CLI_ASSET))?;
        stage(&dir.join(CLI_EXE), &cli, cli_sha)?;
    }
    Ok(Some(manifest.version))
}

fn install_all(dir: &Path) -> Result<()> {
    install(&dir.join(APP_EXE))?;
    let cli = dir.join(CLI_EXE);
    if with_suffix(&cli, ".new").exists() {
        // The command-line tool is optional: don't fail the update over it.
        if let Err(e) = install(&cli) {
            log!("couldn't update {CLI_EXE}: {e:#}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.4.12", "0.4.9"), "numeric, not alphabetical");
        assert!(is_newer("0.5.0", "0.4.99"));
        assert!(!is_newer("0.4.9", "0.4.9"));
        assert!(!is_newer("0.4.8", "0.4.9"));
        assert!(!is_newer("0.4.12", "0.4.0-dev"), "dev builds never compare as older");
        assert!(!is_newer("garbage", "0.4.1"));
    }

    #[test]
    fn manifest_format() {
        let m: Manifest = toml::from_str("version = \"0.4.17\"\nglide_sha256 = \"ab\"\ncli_sha256 = \"cd\"\n").unwrap();
        assert_eq!(m.version, "0.4.17");
        assert_eq!(m.macos_glide_sha256, "", "releases from before Mac builds still read");
        assert_eq!(
            manifest_url("me/glide-kvm"),
            "https://github.com/me/glide-kvm/releases/latest/download/latest.toml"
        );
        assert_eq!(
            asset_url("me/glide-kvm", "0.4.17", "glide.exe"),
            "https://github.com/me/glide-kvm/releases/download/v0.4.17/glide.exe"
        );
    }

    fn temp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("glide-update-test-{}", crate::now_micros()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn stage_install_and_clean_up() {
        let dir = temp_dir();
        let exe = dir.join(APP_EXE);
        std::fs::write(&exe, b"old version").unwrap();

        let new = b"new version";
        assert!(stage(&exe, new, "0000").is_err(), "a bad checksum is refused");
        stage(&exe, new, &sha256_hex(new)).unwrap();
        install(&exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), new);
        if cfg!(windows) {
            assert_eq!(std::fs::read(with_suffix(&exe, ".old")).unwrap(), b"old version");
        }

        clean_up_old(&dir);
        assert!(!with_suffix(&exe, ".old").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_works_while_the_program_file_is_in_use() {
        // Windows forbids deleting a running exe but allows renaming it, which
        // is what install relies on. An open handle stands in for "running".
        let dir = temp_dir();
        let exe = dir.join(APP_EXE);
        std::fs::write(&exe, b"running").unwrap();
        let _in_use = std::fs::File::open(&exe).unwrap();
        stage(&exe, b"next", &sha256_hex(b"next")).unwrap();
        install(&exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"next");
        drop(_in_use);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dev_builds_report_themselves() {
        if !crate::is_release_build() {
            let u = start(UpdaterOptions {
                enabled: true,
                dir: std::env::temp_dir(),
                is_idle: Box::new(|| true),
                restart: Box::new(|_| {}),
            });
            assert_eq!(u.state(), UpdateState::DevBuild);
        }
    }
}
