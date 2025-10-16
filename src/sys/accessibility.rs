use std::ffi::c_void;
use std::time::{Duration, Instant};
use std::{ptr, thread};

use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use tracing::info;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;

    static kAXTrustedCheckOptionPrompt: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: *const c_void;
}

const AX_POLL_INTERVAL: Duration = Duration::from_millis(250);
const AX_POLL_TIMEOUT: Duration = Duration::from_secs(30);

#[inline]
fn ax_is_trusted() -> bool { unsafe { AXIsProcessTrustedWithOptions(ptr::null()) } }

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn prompt_ax_trust_dialog() {
    autoreleasepool(|_| {
        let keys: [*mut AnyObject; 1] = [kAXTrustedCheckOptionPrompt as *mut AnyObject];
        let vals: [*mut AnyObject; 1] = [kCFBooleanTrue as *mut AnyObject];

        let dict: *mut AnyObject = msg_send![
            class!(NSDictionary),
            dictionaryWithObjects: vals.as_ptr(),
            forKeys:              keys.as_ptr(),
            count:                1usize
        ];

        let _ = AXIsProcessTrustedWithOptions(dict.cast());
    });
}

pub fn ensure_accessibility_permission() {
    if ax_is_trusted() {
        return;
    }

    info!("Accessibility permission is not granted; prompting user for permission now.");

    unsafe { prompt_ax_trust_dialog() };

    let start = Instant::now();
    loop {
        if ax_is_trusted() {
            info!("Accessibility permission granted");
            return;
        }

        if start.elapsed() >= AX_POLL_TIMEOUT {
            break;
        }

        thread::sleep(AX_POLL_INTERVAL);
    }

    println!(
        "Rift still does not have accessibility permission. Enable it in System Settings > Privacy & Security > Accessibility, then restart Rift."
    );

    std::process::exit(1);
}
