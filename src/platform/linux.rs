//! Linux backend: not implemented yet (uses the shared stubs).
//!
//! Plan:
//! - inject: a virtual device through /dev/uinput (the `evdev` crate makes this
//!   easy). Wire key codes already *are* evdev codes, so keys need no mapping.
//!   Works on both X11 and Wayland.
//! - capture: read /dev/input/event* and EVIOCGRAB them while grabbed (needs
//!   the `input` group). Edge detection needs the cursor position:
//!   XQueryPointer on X11, or the compositor's input-capture portal on Wayland.
//! - autostart: a .desktop file in ~/.config/autostart.
//! - firewall: usually none; ufw users need `ufw allow 24801/udp`.

pub use super::unsupported::*;
