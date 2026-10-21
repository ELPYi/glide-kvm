//! Build-time identity for updates (see `src/update.rs`).
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
}
