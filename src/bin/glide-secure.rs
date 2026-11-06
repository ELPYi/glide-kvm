//! Windows: the "glide sign-in screen" service and its helper, which let
//! glide control the lock screen and UAC prompts. Installed from glide's
//! Settings; see `platform::windows::signin`. Not started by hand.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        std::process::exit(glide::platform::signin::main(&args));
    }
    #[cfg(not(windows))]
    {
        eprintln!("glide-secure is only for Windows");
        std::process::exit(2);
    }
}
