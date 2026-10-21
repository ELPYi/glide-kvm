//! macOS backend: not implemented yet (uses the shared stubs).
//!
//! Plan:
//! - inject: CGEventCreateMouseEvent / CGEventCreateKeyboardEvent + CGEventPost.
//!   Needs a table from evdev codes to macOS virtual keycodes (kVK_*). Relative
//!   motion means reading the cursor position and posting absolute moves, plus
//!   setting kCGMouseEventDeltaX/Y so games still see deltas.
//! - capture: a CGEventTap at kCGHIDEventTap (returning NULL swallows the
//!   event) plus CGAssociateMouseAndMouseCursorPosition(false) while grabbed.
//! - autostart: a LaunchAgent plist in ~/Library/LaunchAgents.
//! - Both need the Accessibility permission (System Settings > Privacy).

pub use super::unsupported::*;
