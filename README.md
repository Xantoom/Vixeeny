<div align="center">

<img src="packaging/icons/vixeeny-256.png" width="96" alt="">

# Vixeeny

**Screenshots and screen recording for Windows — free, open source, out of your way.**

[![Release](https://img.shields.io/github/v/release/Xantoom/Vixeeny?style=flat-square&color=0a84ff)](https://github.com/Xantoom/Vixeeny/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/Xantoom/Vixeeny/total?style=flat-square&color=0a84ff)](https://github.com/Xantoom/Vixeeny/releases)
[![License](https://img.shields.io/badge/license-GPL--3.0-0a84ff?style=flat-square)](LICENSE)
[![Windows](https://img.shields.io/badge/Windows-10%20%7C%2011-0a84ff?style=flat-square&logo=windows11&logoColor=white)](#install)

[**Download**](https://github.com/Xantoom/Vixeeny/releases/latest) · [Limitations](docs/LIMITATIONS.md) · [Help test](docs/BETA.md) · [Contribute](CONTRIBUTING.md)

<br>

<img src="docs/images/settings.png" width="760" alt="The Vixeeny settings window">

</div>

<br>

Vixeeny lives in the system tray and answers to global shortcuts. Nothing is on screen until you
ask for it, and nothing runs in the background that you did not start.

## Features

<table>
<tr>
<td width="50%" valign="top">

### 📸 Screenshots

- Region, window, full screen, all monitors, **scrolling capture**
- The screens freeze while you choose
- An editor to annotate: arrows, shapes, text, highlighter, blur, pixelate, numbered markers
- Text recognition on the selected region
- PNG, JPEG (jpegli), WebP, AVIF, JPEG XL
- **HDR** kept as HDR (PQ, Ultra HDR for JPEG) or converted to SDR

</td>
<td width="50%" valign="top">

### 🎬 Recording

- Hardware encoders: **NVENC**, **AMF**, **QuickSync**, or the processor
- H.264, HEVC, AV1
- Several audio tracks: system, microphone, single programs
- **Replay buffer**: save the last minutes after they happened
- A built-in cutter to trim a replay before you share it
- A small widget with the time and the disk space, never in your recordings

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 🎮 Made for games

- Files sorted in one folder per game, named after it
- Works in games started as administrator
- Stays out of capture: no frame of Vixeeny in your shots

</td>
<td width="50%" valign="top">

### 🔄 Quiet updates

- Downloaded and installed in the background, or with one click
- Verified by signature before anything is replaced
- Rolled back on its own if the new version does not start

</td>
</tr>
</table>

## Shortcuts

| Action | Default |
| --- | --- |
| Capture a region | <kbd>PrintScreen</kbd> |
| Capture a window | <kbd>Alt</kbd> + <kbd>PrintScreen</kbd> |
| Capture the screen | <kbd>Ctrl</kbd> + <kbd>PrintScreen</kbd> |
| Scrolling capture | <kbd>SHIFT</kbd> + <kbd>PrintScreen</kbd> |
| Start / stop recording | <kbd>Ctrl</kbd> + <kbd>F9</kbd> |
| Pause recording | <kbd>Ctrl</kbd> + <kbd>F10</kbd> |
| Save the replay | <kbd>Ctrl</kbd> + <kbd>F11</kbd> |
| Save and edit the replay | <kbd>Ctrl</kbd> + <kbd>F12</kbd> |
| Open the side bar | <kbd>Ctrl</kbd> + <kbd>F8</kbd> |

Every shortcut can be changed in the settings.

## Install

Download `Vixeeny-<version>-setup.exe`, or the portable `.zip`, from the
[releases page](https://github.com/Xantoom/Vixeeny/releases/latest).

The installer asks who Vixeeny is for:

- **All accounts on this PC** (the default): installed in Program Files, with the administrator's
  rights granted once during the install. Shortcuts and captures then also work in programs
  started as administrator, such as some games.
- **My account only**: installed in your own folders, without these rights. Settings → General
  can move it to Program Files later.

> [!NOTE]
> The installer is not signed with a paid certificate, so SmartScreen may warn the first time:
> **More info → Run anyway**. Updates are verified with [minisign](packaging/minisign.pub).

## Build from source

Vixeeny is written in Rust, with a [Slint](https://slint.dev) interface and FFmpeg for video.

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

Release builds need the pinned native libraries (`cargo xtask build-native`). See
[CONTRIBUTING.md](CONTRIBUTING.md) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## License

[GPL-3.0-or-later](LICENSE). The licences of the bundled libraries are listed in
`THIRD-PARTY-LICENSES.txt` inside every package.
