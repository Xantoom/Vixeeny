// SPDX-License-Identifier: GPL-3.0-or-later
//! The Windows resource of the executable: its icon, the tray icon shown while recording and the version information.
//! "Vixeeny" is the description Task Manager and the taskbar show.

/// System libraries the resident daemon rarely or never calls (the shader compiler of HDR screens, 4 MB, Direct3D 12
/// and the version resources of the hardware report, the network): loaded the first time they are, not held from the
/// start.
const DELAYED: [&str; 4] = ["d3dcompiler_47.dll", "d3d12.dll", "ws2_32.dll", "version.dll"];

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc") {
        for dll in DELAYED {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
    println!("cargo:rerun-if-changed=../../packaging/icons/vixeeny.ico");
    println!("cargo:rerun-if-changed=../../packaging/icons/vixeeny-recording.ico");
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon_with_id("../../packaging/icons/vixeeny.ico", "1")
        .set_icon_with_id("../../packaging/icons/vixeeny-recording.ico", "2")
        .set("ProductName", "Vixeeny")
        .set("FileDescription", "Vixeeny")
        .set("OriginalFilename", "Vixeeny.exe")
        .set("LegalCopyright", "GPL-3.0-or-later");
    if let Err(e) = resource.compile() {
        println!("cargo:warning=cannot embed the Windows resource: {e}");
    }
}
