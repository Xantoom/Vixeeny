// SPDX-License-Identifier: GPL-3.0-or-later
//! The Windows resource of the executable: its icon and the version information. "Vixeeny" is
//! the description Task Manager and the taskbar show.

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/vixeeny.ico");
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
