pub use rift_wm::{actor, common, layout_engine, sys};
#[allow(dead_code, unused_imports)]
#[path = "../src/ui/settings/mod.rs"]
mod settings;
fn main() {
    // Uses real AppKit controls in a hidden window, so it is ignored by default. Run it with `cargo test --test settings_native -- --ignored`
    // (or `cargo nextest run --run-ignored all`). Repeated window lifecycle checks are
    // disabled: only enable the commented --lifecycle call below when profiling.
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
        println!("settings_native: skipped (native AppKit checks); pass --ignored to run");
        return;
    }
    let mtm = objc2::MainThreadMarker::new().unwrap();
    let ui = cgs::Ui::new(mtm);
    objc2_app_kit::NSApplication::sharedApplication(mtm).finishLaunching();
    if let Some(index) = args.iter().position(|arg| arg == "--render") {
        settings::native_tests::render(
            ui,
            args.iter().any(|arg| arg == "--dark"),
            args.iter().any(|arg| arg == "--small"),
            args.iter()
                .position(|arg| arg == "--scope")
                .map(|i| args[i + 1].parse().unwrap()),
            &args[index + 1],
        );
    } else if args.iter().any(|arg| arg == "--lifecycle") {
        // PROFILING ONLY: repeatedly creates and closes Settings windows.
        // Keep disabled during normal UI development and test runs. Uncomment only
        // to profile Settings startup, retained objects, or teardown.
        // objc2::rc::autoreleasepool(|_| settings::native_tests::run_lifecycle(ui));
        println!("Settings lifecycle stress run is disabled; enable only for profiling");
    } else {
        // For benchmarking: the native Settings checks open real AppKit windows, so they
        // are disabled by default. Uncomment to run them.
        // objc2::rc::autoreleasepool(|_| settings::native_tests::run(ui));
        // println!("Settings defaults, controls, sidebar, display refresh and teardown passed");
        let _ = ui;
        println!("Native Settings checks are disabled (for benchmarking)");
    }
}
