# Vixeeny

Free, open-source (GPL-3.0-or-later) screen capture for Windows, macOS and Linux. It lives in the
system tray and answers to global shortcuts; nothing runs on screen until you ask.

- **Screenshots** — region, window, full screen, all monitors, scrolling (Windows); an editor to
  annotate, then save or copy. PNG, JPEG (jpegli), WebP, AVIF, JPEG XL.
- **Screen recording** — hardware encoders (NVENC, AMF, QuickSync, VAAPI, VideoToolbox) or
  software, H.264 / HEVC / AV1, several audio tracks, replay buffer.
- **Text recognition (OCR)** from any zone, with the engine of the system.

Default shortcuts: `PrintScreen` region · `Alt+PrintScreen` window · `Shift+PrintScreen` screen ·
`Ctrl+Shift+R` record · `Ctrl+Shift+P` pause · `Ctrl+Shift+S` save the replay. Everything is
configurable in the settings.

## Install

Download from the [releases page](https://github.com/Xantoom/Vixeeny/releases).

| System | File |
|---|---|
| Windows 10 / 11 | `Vixeeny-<v>-setup.exe`, or the portable `.zip` |
| macOS 13+ (Apple silicon) | `Vixeeny-<v>-macos-arm64.dmg` (not notarized: right-click → Open the first time) |
| Debian 13 / Ubuntu 24.04+ | `vixeeny_<v>_amd64.deb` |
| Any recent Linux (glibc 2.39+) | `Vixeeny-<v>-x86_64.AppImage` or `Vixeeny-<v>.flatpak` |
| Arch | `vixeeny-bin` (AUR; `packaging/arch/PKGBUILD`) |

Linux needs a system tray (KDE, GNOME with the AppIndicator extension, waybar…). On Wayland the
shortcuts go through the GlobalShortcuts portal; where it is missing, bind
`vixeeny-daemon ctl capture-region` (or any other action, `vixeeny-daemon ctl` lists them) to a key
in your compositor. Text recognition needs Tesseract (`tesseract-ocr` and a language pack) and the
clipboard needs `wl-clipboard` (Wayland) or `xclip` (X11); the Flatpak bundles them.

See [docs/LIMITATIONS.md](docs/LIMITATIONS.md) for what does not work yet, and
[docs/BETA.md](docs/BETA.md) if you want to help test.

## Build

```
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Release builds need the pinned native libraries (`cargo xtask build-native`); the design is in
[VIXEENY_PLAN.md](VIXEENY_PLAN.md) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
