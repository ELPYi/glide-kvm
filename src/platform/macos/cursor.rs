//! Hiding the cursor while it's on another PC.
//!
//! macOS only lets the frontmost app hide the cursor, and glide is usually
//! in the background. A long-standing (private) WindowServer setting lifts
//! that; it's looked up at runtime, so if a future macOS drops it, hiding
//! just quietly does nothing.

use std::ffi::{c_void, CStr};
use std::sync::Once;

type Connection = i32;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: *const c_void;
    fn CFStringCreateWithCString(alloc: *const c_void, s: *const std::ffi::c_char, encoding: u32) -> *const c_void;
}

const UTF8: u32 = 0x0800_0100;

unsafe fn lookup(name: &CStr) -> *mut c_void {
    libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr())
}

pub(super) fn allow_hiding_in_background() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let default_connection = lookup(c"_CGSDefaultConnection");
        let set_property = lookup(c"CGSSetConnectionProperty");
        if default_connection.is_null() || set_property.is_null() {
            return;
        }
        let default_connection: extern "C" fn() -> Connection = std::mem::transmute(default_connection);
        let set_property: extern "C" fn(Connection, Connection, *const c_void, *const c_void) -> i32 =
            std::mem::transmute(set_property);
        let key = CFStringCreateWithCString(std::ptr::null(), c"SetsCursorInBackground".as_ptr(), UTF8);
        if key.is_null() {
            return;
        }
        let cid = default_connection();
        set_property(cid, cid, key, kCFBooleanTrue);
    });
}
