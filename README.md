<div align="center">

<img src="packaging/icons/vixeeny-256.png" width="96" alt="">

# Vixeeny

Screenshots and screen recording for Windows. Free and open source.

[![Latest release](https://img.shields.io/github/v/release/Xantoom/Vixeeny?style=flat&label=release&labelColor=18181b&color=0a84ff&logo=github&logoColor=white)](https://github.com/Xantoom/Vixeeny/releases/latest)
[![Windows 10 and 11](https://img.shields.io/badge/Windows-10%20%C2%B7%2011-0a84ff?style=flat&labelColor=18181b&logo=windows11&logoColor=white)](#install)
[![License](https://img.shields.io/github/license/Xantoom/Vixeeny?style=flat&label=license&labelColor=18181b&color=0a84ff)](LICENSE)

[**Download**](https://github.com/Xantoom/Vixeeny/releases/latest) · [Limitations](docs/LIMITATIONS.md) · [Help test](docs/BETA.md) · [Contribute](CONTRIBUTING.md)

<br>

<img src="docs/images/settings.png" width="760" alt="The Vixeeny settings window">

</div>

<br>

Vixeeny sits in the system tray and waits for a shortcut. Nothing shows on screen until you press
one.

## What it does

**Screenshots.** A region, a window, the screen, every monitor at once, or a long page captured
while it scrolls. The screen freezes while you choose, so menus and tooltips stay where they are.
Files are saved as PNG, JPEG, WebP, AVIF or JPEG XL. On an HDR monitor, the capture keeps its HDR
or is converted to SDR, as you prefer.

**Annotations.** Arrows, shapes, text, highlighter, numbered steps, blur and pixelation, then save
or copy. Text in the selection can be copied as text.

**Recording.** With the graphics card's encoder (NVIDIA, AMD, Intel) or the processor, in H.264,
HEVC or AV1. The game, your microphone and single programs can go on separate audio tracks. A small
widget shows the time and warns when the disk runs low; it never appears in the video.

**Replay.** While you play, Vixeeny keeps the last minutes, in memory or on disk. One shortcut
saves them, another opens them in a cutter first so you keep only the moment that matters.

**Games.** Captures go into one folder per game, named after it. Shortcuts also work in games
started as administrator.

**Updates.** Downloaded in the background and checked against a signature before anything is
replaced. If the new version fails to start, the previous one comes back.

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
