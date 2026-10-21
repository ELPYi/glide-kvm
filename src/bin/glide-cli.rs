//! Headless command line for glide. The desktop app (`glide.exe`) is the
//! usual way in; this is for scripting, servers without a display, and
//! diagnostics.

use anyhow::{bail, Result};
use glide::config::{self, Config, Role};
use glide::engine::Engine;
use glide::platform;

const USAGE: &str = "\
glide-cli: share one keyboard and mouse across computers (headless)

USAGE:
  glide-cli server [-c glide.toml]   share this machine's keyboard + mouse
  glide-cli client [-c glide.toml]   be controlled by a server
  glide-cli genkey                   print a random secret for the config
  glide-cli selftest                 check input capture works on this machine
                                     (freezes your mouse for about a second)

Runs until Ctrl+C. The desktop app (glide.exe) does the same with a UI.
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config_arg = args
        .iter()
        .position(|a| a == "-c" || a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);

    match args.first().map(String::as_str) {
        Some("server") => run(config_arg, Role::Server),
        Some("client") => run(config_arg, Role::Client),
        Some("genkey") => {
            println!("{}", glide::net::crypto::generate_secret());
            Ok(())
        }
        Some("selftest") => {
            println!("running selftest...");
            platform::selftest()?;
            println!("selftest passed");
            Ok(())
        }
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

fn run(config_arg: Option<&str>, role: Role) -> Result<()> {
    let cfg = Config::load(&config::find(config_arg)?)?;
    let _engine = Engine::start(&cfg, role)?;
    // The engine runs on its own threads; park until the process is killed.
    loop {
        std::thread::park();
    }
}
