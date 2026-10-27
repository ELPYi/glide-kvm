//! The Mac's clipboard (NSPasteboard). macOS has no "clipboard changed"
//! notification, so a background thread checks its change counter a few
//! times a second (one cheap call).
//!
//! What crosses the network stays in Windows' formats (see
//! [`crate::clip::convert`]): pictures go PNG <-> DIB, formatted text
//! HTML <-> "HTML Format". Copies password managers mark as concealed or
//! transient (the nspasteboard.org convention) are never read.

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

use crate::clip::convert;
use crate::clip::sync::{Clipboard, OnCopy};
use crate::clip::ClipData;

const POLL: Duration = Duration::from_millis(300);
const TEXT: &str = "public.utf8-plain-text";
const HTML: &str = "public.html";
const PNG: &str = "public.png";
const TIFF: &str = "public.tiff";
const PRIVATE: [&str; 2] = ["org.nspasteboard.ConcealedType", "org.nspasteboard.TransientType"];
/// NSBitmapImageFileTypePNG
const FILE_TYPE_PNG: usize = 4;
const UTF8: usize = 4;

type Obj = Retained<AnyObject>;

fn string(s: &str) -> Option<Obj> {
    unsafe {
        let alloc: objc2::rc::Allocated<AnyObject> = msg_send![class!(NSString), alloc];
        msg_send![alloc, initWithBytes: s.as_ptr() as *const c_void, length: s.len(), encoding: UTF8]
    }
}

fn to_rust(s: &AnyObject) -> String {
    unsafe {
        let p: *const c_char = msg_send![s, UTF8String];
        if p.is_null() {
            return String::new();
        }
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn bytes(data: &AnyObject) -> Vec<u8> {
    unsafe {
        let p: *const u8 = msg_send![data, bytes];
        let n: usize = msg_send![data, length];
        if p.is_null() || n == 0 {
            return Vec::new();
        }
        std::slice::from_raw_parts(p, n).to_vec()
    }
}

fn data(b: &[u8]) -> Option<Obj> {
    unsafe { msg_send![class!(NSData), dataWithBytes: b.as_ptr() as *const c_void, length: b.len()] }
}

fn pasteboard() -> Option<Obj> {
    unsafe { msg_send![class!(NSPasteboard), generalPasteboard] }
}

fn change_count(pb: &AnyObject) -> isize {
    unsafe { msg_send![pb, changeCount] }
}

fn has_type(pb: &AnyObject, ty: &str) -> bool {
    let Some(types): Option<Obj> = (unsafe { msg_send![pb, types] }) else {
        return false;
    };
    let Some(ty) = string(ty) else { return false };
    unsafe { msg_send![&*types, containsObject: &*ty] }
}

fn data_for(pb: &AnyObject, ty: &str) -> Option<Vec<u8>> {
    let ty = string(ty)?;
    let d: Option<Obj> = unsafe { msg_send![pb, dataForType: &*ty] };
    d.map(|d| bytes(&d))
}

/// Whatever picture is there (PNG or TIFF), as PNG.
fn picture_as_png(pb: &AnyObject) -> Option<Vec<u8>> {
    if let Some(png) = data_for(pb, PNG) {
        return Some(png);
    }
    let tiff = data(&data_for(pb, TIFF)?)?;
    unsafe {
        let rep: Option<Obj> = msg_send![class!(NSBitmapImageRep), imageRepWithData: &*tiff];
        let props: Obj = msg_send![class!(NSDictionary), dictionary];
        let png: Option<Obj> = msg_send![&*rep?, representationUsingType: FILE_TYPE_PNG, properties: &*props];
        png.map(|p| bytes(&p))
    }
}

fn read(pb: &AnyObject) -> Option<ClipData> {
    if PRIVATE.iter().any(|ty| has_type(pb, ty)) {
        return None;
    }
    let text = string(TEXT).and_then(|ty| {
        let s: Option<Obj> = unsafe { msg_send![pb, stringForType: &*ty] };
        s.map(|s| to_rust(&s))
    });
    let html = data_for(pb, HTML).map(|h| convert::to_cf_html(&String::from_utf8_lossy(&h)));
    let image = picture_as_png(pb).and_then(|png| convert::png_to_dib(&png));
    let d = ClipData { text, html, image };
    (!d.is_empty()).then_some(d)
}

fn write(pb: &AnyObject, d: &ClipData) {
    unsafe {
        let _: isize = msg_send![pb, clearContents];
        let put = |ty: &str, bytes: &[u8]| {
            if let (Some(ty), Some(data)) = (string(ty), data(bytes)) {
                let _: bool = msg_send![pb, setData: &*data, forType: &*ty];
            }
        };
        if let Some(t) = &d.text {
            if let (Some(ty), Some(s)) = (string(TEXT), string(t)) {
                let _: bool = msg_send![pb, setString: &*s, forType: &*ty];
            }
        }
        if let Some(html) = d.html.as_deref().and_then(convert::from_cf_html) {
            put(HTML, html.as_bytes());
        }
        if let Some(png) = d.image.as_deref().and_then(convert::dib_to_png) {
            put(PNG, &png);
        }
    }
}

struct MacClipboard {
    /// The change count after glide's own last write (not a copy to share).
    ours: Arc<AtomicIsize>,
}

impl Clipboard for MacClipboard {
    fn set(&self, data: ClipData) {
        autoreleasepool(|_| {
            let Some(pb) = pasteboard() else { return };
            write(&pb, &data);
            self.ours.store(change_count(&pb), Ordering::Relaxed);
        });
    }
}

/// Stops the watcher thread when glide stops sharing.
struct Watching {
    board: MacClipboard,
    running: Arc<AtomicBool>,
}

impl Clipboard for Watching {
    fn set(&self, data: ClipData) {
        self.board.set(data);
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

pub fn open_clipboard(on_copy: OnCopy) -> Result<Box<dyn Clipboard>> {
    let ours = Arc::new(AtomicIsize::new(-1));
    let running = Arc::new(AtomicBool::new(true));
    let (o, r) = (ours.clone(), running.clone());
    std::thread::Builder::new()
        .name("glide-clipboard-os".into())
        .spawn(move || {
            // What's already there when glide starts isn't a new copy.
            let mut seen = autoreleasepool(|_| pasteboard().map_or(0, |pb| change_count(&pb)));
            while r.load(Ordering::Relaxed) {
                std::thread::sleep(POLL);
                autoreleasepool(|_| {
                    let Some(pb) = pasteboard() else { return };
                    let now = change_count(&pb);
                    if now == seen {
                        return;
                    }
                    seen = now;
                    if now == o.load(Ordering::Relaxed) {
                        return; // glide put it there
                    }
                    if let Some(d) = read(&pb) {
                        on_copy(d);
                    }
                });
            }
        })?;
    Ok(Box::new(Watching {
        board: MacClipboard { ours },
        running,
    }))
}
