// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Opening a project package from Finder: a double-click, a drop on the
//! Dock icon, `open Name.zornov`. macOS tells an application about these by
//! an Apple event, not on the command line, and only an application delegate
//! hears it; the window library registers none. [`install`] adds one that
//! queues the paths, and the window takes them with [`take`] each frame.
//! Windows and Linux open a project from the command line or the folder
//! dialog, so nothing is installed there.

use std::path::PathBuf;
use std::sync::Mutex;

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Paths Finder asked this application to open since the last call.
pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn push(path: PathBuf) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(path);
}

/// Teaches the window library's application delegate to hear "open these
/// documents" (once, on the main thread, after the library has made the
/// application). The library requires its own delegate to stay, so the
/// method is added to that class rather than a delegate of ours replacing it.
#[cfg(target_os = "macos")]
pub fn install() {
    use objc2::ffi;
    use objc2::runtime::{AnyObject, Sel};
    use objc2::sel;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSArray, NSURL};

    unsafe extern "C" fn open_urls(_this: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *mut NSArray<NSURL>) {
        // SAFETY: AppKit passes a valid NSArray of NSURL for the duration of the call.
        let Some(urls) = (unsafe { urls.as_ref() }) else { return };
        for url in urls.iter() {
            if let Some(path) = unsafe { url.path() } {
                crate::open_files::push(PathBuf::from(path.to_string()));
            }
        }
    }

    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = (unsafe { app.delegate() }) else { return };
    let object: *const AnyObject = objc2::rc::Retained::as_ptr(&delegate).cast();
    // SAFETY: `object` is the live delegate; the method is added to its class once, with the
    // signature AppKit calls it with (void, self, _cmd, NSApplication *, NSArray<NSURL *> *).
    unsafe {
        let class = (*object).class();
        let imp: unsafe extern "C" fn() = std::mem::transmute(open_urls as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>));
        let class_ptr = class as *const _ as *mut ffi::objc_class;
        // A false result means the class already answers it: then it does the opening itself.
        let _ = ffi::class_addMethod(class_ptr, sel!(application:openURLs:).as_ptr().cast(), Some(imp), c"v@:@@".as_ptr());
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_paths_are_taken_once_in_order() {
        push(PathBuf::from("/a/One.zornov"));
        push(PathBuf::from("/a/Two.zornov"));
        let got = take();
        assert!(got.ends_with(&[PathBuf::from("/a/One.zornov"), PathBuf::from("/a/Two.zornov")]));
        assert!(take().is_empty());
    }
}
