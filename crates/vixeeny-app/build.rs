// SPDX-License-Identifier: GPL-3.0-or-later
//! Gives the Windows executable its icon and version information. Only done when building on
//! Windows: cross-checks from another system have no resource compiler and need no icon.

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/vixeeny.ico");
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
            let mut resource = winresource::WindowsResource::new();
            resource
                .set_icon("../../packaging/icons/vixeeny.ico")
                .set("ProductName", "Vixeeny")
                .set("FileDescription", "Vixeeny app (capture, editor, settings)")
                .set("LegalCopyright", "GPL-3.0-or-later");
            if let Err(e) = resource.compile() {
                println!("cargo:warning=cannot embed the Windows resource: {e}");
            }
        }
    }
}
