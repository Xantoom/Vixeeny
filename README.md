# Vixeeny

Free, open-source (GPL-3.0-or-later) screen capture for Windows 10 and 11. It lives in the
system tray and answers to global shortcuts; nothing runs on screen until you ask.

- **Screenshots** — region, window, full screen, all monitors, scrolling; the screens freeze
  while you choose, then an editor to annotate, save or copy. PNG, JPEG (jpegli), WebP, AVIF,
  JPEG XL.
- **Screen recording** — hardware encoders (NVENC, AMF, QuickSync) or software, H.264 / HEVC /
  AV1, several audio tracks, replay buffer.
- **Text recognition (OCR)** from any zone, with the Windows engine.
- **Quiet updates** — downloaded and installed in the background (or with one click in
  Settings → About), verified by signature, rolled back if the new version does not start.

Default shortcuts: `PrintScreen` region · `Alt+PrintScreen` window · `Shift+PrintScreen` screen ·
`Ctrl+Shift+R` record · `Ctrl+Shift+P` pause · `Ctrl+Shift+S` save the replay. Everything is
configurable in the settings.

## Install

Download `Vixeeny-<v>-setup.exe` (or the portable `.zip`) from the
[releases page](https://github.com/Xantoom/Vixeeny/releases). The installer needs no
administrator rights.

See [docs/LIMITATIONS.md](docs/LIMITATIONS.md) for what does not work yet, and
[docs/BETA.md](docs/BETA.md) if you want to help test.

## Build

```
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

Release builds need the pinned native libraries (`cargo xtask build-native`); see
[CONTRIBUTING.md](CONTRIBUTING.md), [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and the design
in [VIXEENY_PLAN.md](VIXEENY_PLAN.md).
