// SPDX-License-Identifier: GPL-3.0-or-later
//! The Windows resource of the executable: its icon, the tray icon shown while recording and the version information.
//! "Vixeeny" is the description Task Manager and the taskbar show.

/// System libraries the resident daemon rarely or never calls (the shader compiler of HDR screens, 4 MB, Direct3D 12
/// and the version resources of the hardware report, the network): loaded the first time they are, not held from the
/// start.
const DELAYED: [&str; 4] = ["d3dcompiler_47.dll", "d3d12.dll", "ws2_32.dll", "version.dll"];

include!("../build-common.rs");

fn main() {
    if is_msvc() {
        delay_load(&DELAYED);
        link_map("Vixeeny.map");
    }
    resource(
        "Vixeeny.exe",
        &[("1", "../../packaging/icons/vixeeny.ico"), ("2", "../../packaging/icons/vixeeny-recording.ico")],
    );
}
