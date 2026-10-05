// SPDX-License-Identifier: GPL-3.0-or-later
//! The Windows resource of the executable: its icon and the version information. "Vixeeny" is
//! the description Task Manager and the taskbar show.

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/vixeeny.ico");
    delay_load_ffmpeg();
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon_with_id("../../packaging/icons/vixeeny.ico", "1")
        .set("ProductName", "Vixeeny")
        .set("FileDescription", "Vixeeny")
        .set("OriginalFilename", "vixeeny-app.exe")
        .set("LegalCopyright", "GPL-3.0-or-later");
    if let Err(e) = resource.compile() {
        println!("cargo:warning=cannot embed the Windows resource: {e}");
    }
}

/// FFmpeg's DLLs (≈ 180 MB) are loaded when a recording or a video thumbnail first needs them,
/// not when the program starts: a screenshot, the side strip or a notification start without
/// them. The DLLs are those of `FFMPEG_DIR/bin`, whatever their version numbers.
fn delay_load_ffmpeg() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc");
    if !msvc || std::env::var_os("CARGO_FEATURE_FFMPEG").is_none() {
        return;
    }
    let Some(dir) = std::env::var_os("FFMPEG_DIR") else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(std::path::Path::new(&dir).join("bin")) else {
        return;
    };
    let mut any = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".dll") && (lower.starts_with("av") || lower.starts_with("sw")) {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{name}");
            any = true;
        }
    }
    if any {
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
