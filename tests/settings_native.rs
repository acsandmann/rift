pub use rift_wm::{actor, common, layout_engine, sys};
#[allow(dead_code, unused_imports)]
#[path = "../src/ui/settings/mod.rs"]
mod settings;
fn main() {
    // A single libtest-compatible case lets nextest list this harness without running AppKit.
    if std::env::args().any(|arg| arg == "--list") {
        if !std::env::args().any(|arg| arg == "--ignored") {
            println!("settings_native: test");
        }
        return;
    }
    let mtm = objc2::MainThreadMarker::new().unwrap();
    let ui = cgs::Ui::new(mtm);
    objc2_app_kit::NSApplication::sharedApplication(mtm).finishLaunching();
    objc2::rc::autoreleasepool(|_| settings::native_tests::run(ui));
    println!("Settings defaults, inventory, sidebar, display refresh and teardown passed");
}
