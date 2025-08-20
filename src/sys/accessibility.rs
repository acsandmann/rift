use std::time::{Duration, Instant};

use accessibility_sys::{AXIsProcessTrusted, AXIsProcessTrustedWithOptions};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::string::CFString;
use core_graphics::display::CFDictionary;
use tracing::{info, trace};

fn prompt() {
    unsafe {
        let key = CFString::new("kAXTrustedCheckOptionPrompt");
        let value = CFBoolean::true_value();

        let _ = AXIsProcessTrustedWithOptions(
            CFDictionary::from_CFType_pairs(&[(key, value)]).as_concrete_TypeRef(),
        );
    }
}

pub fn ensure_accessibility_permission() {
    if unsafe { AXIsProcessTrusted() } {
        return;
    }

    trace!("Accessibility permission is not granted; prompting user for permission now.");

    prompt();

    let start = Instant::now();
    loop {
        if unsafe { AXIsProcessTrusted() } {
            info!(
                "Accessibility permission granted; rift will exit so that new permissions can take effect."
            );
            std::process::exit(0);
        }

        let elapsed = start.elapsed();
        let interval = if elapsed < Duration::from_secs(300) {
            Duration::from_secs(15)
        } else {
            Duration::from_secs(45)
        };

        std::thread::sleep(interval);
    }
}
