fn main() {
    #[cfg(target_os = "macos")]
    add_swift_runtime_link_path();

    tauri_build::build()
}

#[cfg(target_os = "macos")]
fn add_swift_runtime_link_path() {
    use std::path::Path;
    use std::process::Command;

    let Ok(output) = Command::new("xcrun").args(["--find", "swiftc"]).output() else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let swiftc = String::from_utf8_lossy(&output.stdout);
    let Some(usr_dir) = Path::new(swiftc.trim()).parent().and_then(Path::parent) else {
        return;
    };
    let runtime_dir = usr_dir.join("lib/swift/macosx");
    if runtime_dir.is_dir() {
        println!("cargo:rustc-link-search=native={}", runtime_dir.display());
    }
}
