// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux through X11 (native sessions and XWayland): monitors from RandR, the pointer, the window
//! list and the active window from the EWMH properties. Positions are physical pixels, like on
//! the other systems.
//!
//! A Wayland-only session has no X server to ask: the calls fail with an error and the capture
//! crate falls back to the portals. What is not here yet falls back to [`crate::unsupported`].

use std::path::Path;

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, MapState, Window};
use x11rb::rust_connection::RustConnection;

pub use crate::unsupported::*;

use crate::{MonitorId, MonitorInfo, PhysicalRect, PlatformError, Result, WindowId, WindowInfo};

fn os_error(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Os(e.to_string())
}

/// A connection to the X server and its root window.
pub struct X11 {
    pub conn: RustConnection,
    pub root: Window,
}

impl X11 {
    pub fn connect() -> Result<Self> {
        let (conn, screen) = x11rb::connect(None).map_err(os_error)?;
        let root = conn.setup().roots[screen].root;
        Ok(Self { conn, root })
    }

    fn atom(&self, name: &str) -> Result<Atom> {
        Ok(self
            .conn
            .intern_atom(false, name.as_bytes())
            .map_err(os_error)?
            .reply()
            .map_err(os_error)?
            .atom)
    }

    /// 32-bit items of a property (empty when it is missing).
    fn items(&self, window: Window, property: &str, kind: AtomEnum) -> Result<Vec<u32>> {
        let atom = self.atom(property)?;
        let reply = self
            .conn
            .get_property(false, window, atom, kind, 0, 4096)
            .map_err(os_error)?
            .reply()
            .map_err(os_error)?;
        Ok(reply.value32().map(Iterator::collect).unwrap_or_default())
    }

    fn text(&self, window: Window, property: &str) -> Result<String> {
        let atom = self.atom(property)?;
        let reply = self
            .conn
            .get_property(false, window, atom, AtomEnum::ANY, 0, 1024)
            .map_err(os_error)?
            .reply()
            .map_err(os_error)?;
        Ok(String::from_utf8_lossy(&reply.value).into_owned())
    }

    /// The desktop's text scale (`Xft.dpi`), 96 when nothing sets it.
    fn dpi(&self) -> u32 {
        let resources = self
            .conn
            .get_property(
                false,
                self.root,
                AtomEnum::RESOURCE_MANAGER,
                AtomEnum::STRING,
                0,
                65536,
            )
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.value).into_owned())
            .unwrap_or_default();
        parse_xft_dpi(&resources).unwrap_or(96)
    }
}

/// The `Xft.dpi` line of the X resources.
fn parse_xft_dpi(resources: &str) -> Option<u32> {
    let value = resources
        .lines()
        .find_map(|l| l.strip_prefix("Xft.dpi:"))?
        .trim()
        .parse::<f64>()
        .ok()?;
    (48.0..=480.0)
        .contains(&value)
        .then_some(value.round() as u32)
}

pub fn monitors() -> Result<Vec<MonitorInfo>> {
    let x = X11::connect()?;
    let dpi = x.dpi();
    let reply = x
        .conn
        .randr_get_monitors(x.root, true)
        .map_err(os_error)?
        .reply()
        .map_err(os_error)?;
    let mut out = Vec::new();
    for m in reply.monitors {
        let name = x
            .conn
            .get_atom_name(m.name)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.name).into_owned())
            .unwrap_or_default();
        out.push(MonitorInfo {
            // RandR has no handle for a monitor: the name is the stable identity.
            id: MonitorId(u64::from(m.name)),
            name,
            rect: PhysicalRect::new(
                i32::from(m.x),
                i32::from(m.y),
                u32::from(m.width),
                u32::from(m.height),
            ),
            primary: m.primary,
            dpi,
            hdr: None,
        });
    }
    if out.is_empty() {
        // No RandR 1.5 monitors: the whole screen is the one monitor.
        let screen = &x.conn.setup().roots[0];
        out.push(MonitorInfo {
            id: MonitorId(0),
            name: "screen".into(),
            rect: PhysicalRect::new(
                0,
                0,
                u32::from(screen.width_in_pixels),
                u32::from(screen.height_in_pixels),
            ),
            primary: true,
            dpi,
            hdr: None,
        });
    }
    Ok(out)
}

pub fn cursor_position() -> Result<(i32, i32)> {
    let x = X11::connect()?;
    let p = x
        .conn
        .query_pointer(x.root)
        .map_err(os_error)?
        .reply()
        .map_err(os_error)?;
    Ok((i32::from(p.root_x), i32::from(p.root_y)))
}

fn exe_of(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// The frame of `window` in root coordinates: client area plus the decorations the window
/// manager announces in `_NET_FRAME_EXTENTS`.
fn describe(x: &X11, window: Window) -> Result<Option<WindowInfo>> {
    let attrs = x
        .conn
        .get_window_attributes(window)
        .map_err(os_error)?
        .reply()
        .map_err(os_error)?;
    if attrs.map_state != MapState::VIEWABLE {
        return Ok(None);
    }
    if x.items(window, "_NET_WM_STATE", AtomEnum::ATOM)?
        .contains(&x.atom("_NET_WM_STATE_HIDDEN")?)
    {
        return Ok(None);
    }
    let geometry = x
        .conn
        .get_geometry(window)
        .map_err(os_error)?
        .reply()
        .map_err(os_error)?;
    let origin = x
        .conn
        .translate_coordinates(window, x.root, 0, 0)
        .map_err(os_error)?
        .reply()
        .map_err(os_error)?;
    // left, right, top, bottom
    let extents = x.items(window, "_NET_FRAME_EXTENTS", AtomEnum::CARDINAL)?;
    let [left, right, top, bottom] = extents.as_slice() else {
        return finish(
            x,
            window,
            i32::from(origin.dst_x),
            i32::from(origin.dst_y),
            u32::from(geometry.width),
            u32::from(geometry.height),
        );
    };
    finish(
        x,
        window,
        i32::from(origin.dst_x) - *left as i32,
        i32::from(origin.dst_y) - *top as i32,
        u32::from(geometry.width) + left + right,
        u32::from(geometry.height) + top + bottom,
    )
}

fn finish(x: &X11, window: Window, px: i32, py: i32, w: u32, h: u32) -> Result<Option<WindowInfo>> {
    let mut title = x.text(window, "_NET_WM_NAME")?;
    if title.is_empty() {
        title = x.text(window, "WM_NAME")?;
    }
    let pid = x
        .items(window, "_NET_WM_PID", AtomEnum::CARDINAL)?
        .first()
        .copied()
        .unwrap_or(0);
    Ok(Some(WindowInfo {
        id: WindowId(u64::from(window)),
        title,
        rect: PhysicalRect::new(px, py, w, h),
        pid,
        exe_path: exe_of(pid),
    }))
}

pub fn top_level_windows() -> Result<Vec<WindowInfo>> {
    let x = X11::connect()?;
    // Bottom to top; the callers want the front first.
    let mut ids = x.items(x.root, "_NET_CLIENT_LIST_STACKING", AtomEnum::WINDOW)?;
    if ids.is_empty() {
        ids = x.items(x.root, "_NET_CLIENT_LIST", AtomEnum::WINDOW)?;
    }
    let mut out = Vec::new();
    for id in ids.into_iter().rev() {
        if let Ok(Some(info)) = describe(&x, id)
            && info.rect.width >= 50
            && info.rect.height >= 50
        {
            out.push(info);
        }
    }
    Ok(out)
}

pub fn foreground_window() -> Result<Option<WindowInfo>> {
    let x = X11::connect()?;
    let active = x.items(x.root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW)?;
    match active.first() {
        Some(&id) if id != 0 => describe(&x, id),
        _ => Ok(None),
    }
}

pub fn window_info(id: WindowId) -> Result<Option<WindowInfo>> {
    let x = X11::connect()?;
    describe(&x, id.0 as Window)
}

/// `Name=` of the `.desktop` file whose `Exec` starts `exe` (how Linux names an application).
pub fn exe_metadata(path: &str) -> crate::ExeMetadata {
    let wanted = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let Some(wanted) = wanted else {
        return crate::ExeMetadata::default();
    };
    let mut dirs = vec![
        std::path::PathBuf::from("/usr/share/applications"),
        std::path::PathBuf::from("/usr/local/share/applications"),
        std::path::PathBuf::from("/var/lib/flatpak/exports/share/applications"),
    ];
    if let Some(home) = std::env::home_dir() {
        dirs.push(home.join(".local/share/applications"));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if let Some(name) = desktop_name_for(&text, &wanted) {
                return crate::ExeMetadata {
                    product_name: Some(name),
                    file_description: None,
                };
            }
        }
    }
    crate::ExeMetadata::default()
}

/// The `Name` of a desktop entry when its `Exec` command runs `exe_name`.
fn desktop_name_for(entry: &str, exe_name: &str) -> Option<String> {
    let mut in_entry = false;
    let (mut name, mut exec) = (None, None);
    for line in entry.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry {
            if let Some(v) = line.strip_prefix("Name=") {
                name = Some(v.to_owned());
            } else if let Some(v) = line.strip_prefix("Exec=") {
                exec = Some(v.to_owned());
            }
        }
    }
    let command = exec?;
    let first = command.split_whitespace().next()?;
    let base = Path::new(first.trim_matches('"'))
        .file_name()?
        .to_string_lossy();
    (base == exe_name).then_some(name?)
}

/// The GPUs of the machine, from the DRM devices of `/sys/class/drm` (`cardN`, not the
/// connectors `cardN-HDMI-A-1`). The name comes from the PCI id database when the system has one.
pub fn gpu_adapters() -> Result<Vec<crate::GpuInfo>> {
    let entries = std::fs::read_dir("/sys/class/drm").map_err(os_error)?;
    let mut cards: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.strip_prefix("card")
                    .is_some_and(|r| r.chars().all(|c| c.is_ascii_digit()))
            })
        })
        .collect();
    cards.sort();
    let hex = |path: std::path::PathBuf| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| u32::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok())
    };
    let ids = pci_ids();
    let mut out = Vec::new();
    for card in cards {
        let device = card.join("device");
        let (Some(vendor_id), Some(device_id)) =
            (hex(device.join("vendor")), hex(device.join("device")))
        else {
            continue;
        };
        let driver = std::fs::read_link(device.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let version = driver
            .as_deref()
            .and_then(|d| std::fs::read_to_string(format!("/sys/module/{d}/version")).ok())
            .map(|v| v.trim().to_owned())
            .or_else(|| {
                std::fs::read_to_string("/proc/sys/kernel/osrelease")
                    .ok()
                    .map(|v| v.trim().to_owned())
            })
            .unwrap_or_default();
        let name = pci_name(&ids, vendor_id, device_id)
            .unwrap_or_else(|| format!("GPU {vendor_id:04x}:{device_id:04x}"));
        out.push(crate::GpuInfo {
            name,
            vendor_id,
            device_id,
            driver_version: version,
            software: false,
        });
    }
    Ok(out)
}

/// The text of the PCI id database, empty when the system has none.
fn pci_ids() -> String {
    [
        "/usr/share/hwdata/pci.ids",
        "/usr/share/misc/pci.ids",
        "/usr/share/pci.ids",
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(p).ok())
    .unwrap_or_default()
}

/// `vendor name device name` from the database format (`vvvv  Vendor`, `\tdddd  Device`).
fn pci_name(ids: &str, vendor: u32, device: u32) -> Option<String> {
    let (v, d) = (format!("{vendor:04x}  "), format!("\t{device:04x}  "));
    let mut lines = ids.lines();
    let vendor_name = lines.find_map(|l| l.strip_prefix(v.as_str()))?.trim();
    for line in lines {
        if !line.starts_with('\t') && !line.starts_with('#') && !line.is_empty() {
            break;
        }
        if let Some(name) = line.strip_prefix(d.as_str()) {
            return Some(format!("{vendor_name} {}", name.trim()));
        }
    }
    Some(vendor_name.to_owned())
}

pub fn user_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .map(|v| locale_tag(&v))
}

/// `fr_FR.UTF-8` → `fr-FR`.
fn locale_tag(value: &str) -> String {
    value
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('_', "-")
}

pub fn open_path(path: &str) -> Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(os_error)
}

pub fn recycle(path: &str) -> Result<()> {
    let status = std::process::Command::new("gio")
        .args(["trash", path])
        .status()
        .map_err(os_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(PlatformError::Os("gio trash failed".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_xft_dpi_resource_is_read() {
        assert_eq!(
            parse_xft_dpi("Xft.antialias:\t1\nXft.dpi:\t144\n"),
            Some(144)
        );
        assert_eq!(parse_xft_dpi("Xft.dpi: 96.0"), Some(96));
        assert_eq!(parse_xft_dpi("Xft.dpi: 5"), None);
        assert_eq!(parse_xft_dpi(""), None);
    }

    #[test]
    fn a_desktop_entry_names_its_executable() {
        let entry = "[Desktop Entry]\nName=Firefox\nExec=/usr/bin/firefox %u\n[Desktop Action x]\nName=No\n";
        assert_eq!(
            desktop_name_for(entry, "firefox").as_deref(),
            Some("Firefox")
        );
        assert_eq!(desktop_name_for(entry, "chrome"), None);
    }

    #[test]
    fn a_gpu_is_named_from_the_pci_database() {
        let ids = "# comment\n10de  NVIDIA Corporation\n\t2684  AD102 [GeForce RTX 4090]\n\t2685  AD102 [x]\n1002  Advanced Micro Devices, Inc.\n\t744c  Navi 31\n";
        assert_eq!(
            pci_name(ids, 0x10de, 0x2684).as_deref(),
            Some("NVIDIA Corporation AD102 [GeForce RTX 4090]")
        );
        assert_eq!(
            pci_name(ids, 0x1002, 0x9999).as_deref(),
            Some("Advanced Micro Devices, Inc.")
        );
        assert_eq!(pci_name(ids, 0x8086, 0x1), None);
    }

    #[test]
    fn the_locale_becomes_a_language_tag() {
        assert_eq!(locale_tag("fr_FR.UTF-8"), "fr-FR");
        assert_eq!(locale_tag("de_DE@euro"), "de-DE");
    }
}
