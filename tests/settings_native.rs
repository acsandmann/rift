pub use rift_wm::{actor, common, layout_engine, sys};
#[allow(dead_code, unused_imports)]
#[path = "../src/ui/settings/mod.rs"]
mod settings;
fn main() {
    // Opens real AppKit UI, so it is ignored by default. Run it with `cargo test --test settings_native -- --ignored`
    // (or `cargo nextest run --run-ignored all`).
    // nextest discovers it via `--list`; report it only in the ignored listing so it is skipped by default.
    let args: Vec<String> = std::env::args().collect();
    let ignored = args.iter().any(|arg| arg == "--ignored" || arg == "--include-ignored");
    if args.iter().any(|arg| arg == "--list") {
        if args.iter().any(|arg| arg == "--ignored") {
            println!("settings_native: test");
        }
        return;
    }
    if !ignored {
        println!("settings_native: skipped (opens UI); pass --ignored to run");
        return;
    }
    let mtm = objc2::MainThreadMarker::new().unwrap();
    let ui = cgs::Ui::new(mtm);
    objc2_app_kit::NSApplication::sharedApplication(mtm).finishLaunching();
    objc2::rc::autoreleasepool(|_| settings::native_tests::run(ui));
    println!("Settings defaults, inventory, sidebar, display refresh and teardown passed");
}
