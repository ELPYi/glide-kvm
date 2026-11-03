//! Build-time identity for updates (see `src/update.rs`), and on Windows
//! the icon embedded in the exe (taskbar, Explorer, Alt+Tab).
//!
//! CI release builds set:
//! - `GLIDE_VERSION`: the release version, e.g. `0.4.17`;
//! - `GLIDE_UPDATE_REPO`: where releases live, e.g. `you/glide-kvm`.
//!
//! Local builds set neither: they're "dev" builds, which never update
//! themselves or change startup settings.

fn main() {
    let version = std::env::var("GLIDE_VERSION").unwrap_or_else(|_| format!("{}-dev", env!("CARGO_PKG_VERSION")));
    let repo = std::env::var("GLIDE_UPDATE_REPO").unwrap_or_default();
    println!("cargo:rustc-env=GLIDE_VERSION={version}");
    println!("cargo:rustc-env=GLIDE_UPDATE_REPO={repo}");
    println!("cargo:rerun-if-env-changed=GLIDE_VERSION");
    println!("cargo:rerun-if-env-changed=GLIDE_UPDATE_REPO");

    // The build *host* is Windows (only it has a resource compiler) and so
    // is the target (not, say, a Mac lint run from Windows).
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/icon/glide.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/glide.ico");
        res.set("FileDescription", "glide");
        res.set("ProductName", "glide");
        if let Err(e) = res.compile() {
            // MinGW's windres can't cope with spaces in the project path.
            // Fine for a local build; a release must have its icon.
            if std::env::var_os("GLIDE_VERSION").is_some() {
                panic!("embedding the icon in glide.exe: {e}");
            }
            println!("cargo:warning=no icon embedded in this dev build: {e}");
        }
    }
}
