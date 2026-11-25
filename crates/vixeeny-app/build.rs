// SPDX-License-Identifier: GPL-3.0-or-later
//! The Windows resource of the executable: its icon and the version information. "Vixeeny" is the description Task
//! Manager and the taskbar show.

/// System libraries most processes of the app never call (a notification, the widget, the settings): loaded the first
/// time they are, not at every start. The shader compiler (4 MB) for HDR and GPU conversions, Direct3D 12 and the
/// device setup for the hardware report, the network and TLS for the updates, UI Automation for the settings' screen
/// readers, the audio devices for recordings. `dbghelp` stays loaded: it writes the minidump of a crash.
const DELAYED: [&str; 11] = [
    "d3dcompiler_47.dll",
    "d3d12.dll",
    "setupapi.dll",
    "ws2_32.dll",
    "crypt32.dll",
    "ncrypt.dll",
    "secur32.dll",
    "uiautomationcore.dll",
    "mmdevapi.dll",
    "avrt.dll",
    "version.dll",
];

include!("../build-common.rs");

fn main() {
    if is_msvc() {
        delay_load(&DELAYED);
        delay_load_ffmpeg();
        link_map("vixeeny-app.map");
    }
    resource("vixeeny-app.exe", &[("1", "../../packaging/icons/vixeeny.ico")]);
}

/// FFmpeg's DLLs (≈ 65 MB) are loaded when a recording or a video thumbnail first needs them, not when the program
/// starts: a screenshot, the side strip or a notification start without them. The DLLs are those of `FFMPEG_DIR/bin`,
/// whatever their version numbers; `cargo xtask dist` refuses a program that would load them at start.
fn delay_load_ffmpeg() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let Some(dir) = std::env::var_os("FFMPEG_DIR") else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(std::path::Path::new(&dir).join("bin")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".dll") && (lower.starts_with("av") || lower.starts_with("sw")) {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{name}");
        }
    }
}
