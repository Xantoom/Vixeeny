# Vixeeny

Free, open-source (GPL-3.0-or-later) screen capture for Windows 10 and 11. It lives in the
system tray and answers to global shortcuts; nothing runs on screen until you ask.

- **Screenshots** — region, window, full screen, all monitors, scrolling; the screens freeze
  while you choose, then an editor to annotate, save or copy. PNG, JPEG (jpegli), WebP, AVIF,
  JPEG XL; HDR screens kept as HDR (PQ; Ultra HDR for JPEG) or converted to SDR.
- **Screen recording** — hardware encoders (NVENC, AMF, QuickSync) or software, H.264 / HEVC /
  AV1, several audio tracks, replay buffer.
- **Quiet updates** — downloaded and installed in the background (or with one click in
  Settings → About), verified by signature, rolled back if the new version does not start.

Default shortcuts: `PrintScreen` region · `Alt+PrintScreen` window · `Ctrl+PrintScreen` screen ·
`Shift+PrintScreen` scrolling capture · `Ctrl+F9` record · `Ctrl+F10` pause · `Ctrl+F11` save the
replay. Everything is configurable in the settings.

## Install

Download `Vixeeny-<v>-setup.exe` (or the portable `.zip`) from the
[releases page](https://github.com/Xantoom/Vixeeny/releases). The installer asks who it is for:

- **All accounts on this PC** (the default): Vixeeny goes in Program Files and starts with the
  administrator's rights, without asking each time. Its shortcuts and captures then also work
  in programs started as administrator (some games). Windows asks once, during the install.
- **My account only**: Vixeeny goes in your own folders, without these rights. Settings → General
  can move it to Program Files later.

Then the folder and whether Vixeeny starts with Windows. Everything else is in the settings.

See [docs/LIMITATIONS.md](docs/LIMITATIONS.md) for what does not work yet, and
[docs/BETA.md](docs/BETA.md) if you want to help test.

## Build

```
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

Release builds need the pinned native libraries (`cargo xtask build-native`); see
[CONTRIBUTING.md](CONTRIBUTING.md) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
