// SPDX-License-Identifier: GPL-3.0-or-later
// Shared by the build scripts of the two programs (`include!`d): their Windows resource and their link map.

/// System libraries a program rarely calls: loaded the first time they are, not held from the start.
fn delay_load(dlls: &[&str]) {
    for dll in dlls {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bins=delayimp.lib");
}

fn is_msvc() -> bool {
    std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc")
}

/// The Windows resource of the program `file`: its icons (id, path from the crate), the version information ("Vixeeny"
/// is the description Task Manager and the taskbar show, "Xantoom" the publisher of its startup apps) and the manifest.
fn resource(file: &str, icons: &[(&str, &str)]) {
    const MANIFEST: &str = "../../packaging/windows/vixeeny.manifest";
    println!("cargo:rerun-if-changed={MANIFEST}");
    // Windows refuses to start a program whose manifest is not well-formed XML: the build refuses it first.
    let text = std::fs::read_to_string(MANIFEST).unwrap_or_default();
    if let Err(e) = roxmltree::Document::parse(&text) {
        panic!("{MANIFEST} is not well-formed XML: {e}");
    }
    let mut resource = winresource::WindowsResource::new();
    for (id, icon) in icons {
        println!("cargo:rerun-if-changed={icon}");
        resource.set_icon_with_id(icon, id);
    }
    resource
        .set("ProductName", "Vixeeny")
        .set("FileDescription", "Vixeeny")
        .set("CompanyName", "Xantoom")
        .set("OriginalFilename", file)
        .set("LegalCopyright", "GPL-3.0-or-later")
        .set_manifest_file(MANIFEST);
    if let Err(e) = resource.compile() {
        println!("cargo:warning=cannot embed the Windows resource: {e}");
    }
}

/// A release build also writes the linker's map of the program beside it: the function at each address, which names the
/// addresses of a crash note (`cargo xtask symbolize`). The program is the same with or without it.
fn link_map(name: &str) {
    // `<target>/release/build/<crate>/<hash>/out` (`build/<crate>-<hash>/out` before Cargo 1.100): the folder above
    // `build`.
    let out = std::env::var_os("OUT_DIR").map(std::path::PathBuf::from);
    let profile = out.as_deref().and_then(|out| {
        out.ancestors().find(|dir| dir.file_name().is_some_and(|name| name == "build")).and_then(|build| build.parent())
    });
    if let Some(profile) = profile
        && profile.file_name().is_some_and(|name| name == "release")
    {
        println!("cargo:rustc-link-arg-bins=/MAP:{}", profile.join(name).display());
    }
}
