use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    for path in ["--git-dir", "--git-common-dir"] {
        if let Some(dir) = git(&["rev-parse", path]) {
            for entry in ["HEAD", "refs", "packed-refs", "index"] {
                println!("cargo:rerun-if-changed={dir}/{entry}");
            }
        }
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=crates");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let release_tag = format!("v{version}");
    let tagged = git(&["tag", "--points-at", "HEAD"])
        .is_some_and(|tags| tags.lines().any(|tag| tag == release_tag || tag == version));
    let dirty = git(&["diff", "HEAD", "--quiet"]).is_none();
    let display_version = if tagged && !dirty {
        version
    } else {
        let commit = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
        format!("{version}+{commit}{}", if dirty { ".dirty" } else { "" })
    };
    println!("cargo:rustc-env=RIFT_VERSION={display_version}");
    println!("cargo:rustc-link-search=framework=/System/Library/PrivateFrameworks");

    println!("cargo:rustc-link-lib=framework=SkyLight");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=CoreVideo");
    println!("cargo:rustc-link-lib=framework=IOKit");
    println!("cargo:rustc-link-lib=framework=MultitouchSupport");
    println!("cargo:rustc-link-lib=framework=Carbon");
}
