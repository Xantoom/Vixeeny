# Architecture

The design and its reasons are in [VIXEENY_PLAN.md](../VIXEENY_PLAN.md) (section 4 and the
decision log at the end); this page is the map.

## Two processes

```
vixeeny-daemon (always on)                      vixeeny-app (on demand)
  tray icon, global shortcuts, IPC server  ←──→  captures, editor, recording, settings, OCR…
  no Slint, no FFmpeg, no capture code            started by the daemon for an action,
  idles at ~0 % CPU and a few MB                  exits after `app_idle_exit_seconds`
```

The daemon is small so that being resident costs nothing; everything heavy lives in the app,
which the daemon starts when a shortcut fires and which exits when idle. They talk over a local
socket (named pipe on Windows, Unix socket elsewhere) with length-prefixed `postcard` messages
(`vixeeny-common::ipc`). Binding the endpoint is also the single-instance lock. Some work runs
in processes of their own: the settings window (`--settings`), the recording pill (`--widget`,
Windows and macOS), toasts, the hardware probe (`--probe`) and the updater.

The daemon's logic is a pure state machine (`core.rs`: `Event` → `Effect`) driven by a small
runtime (`runtime.rs`); only the platform layer differs per OS (`platform/`):

| OS | Loop | Tray | Shortcuts |
|---|---|---|---|
| Windows | message-only window | `tray-icon` | `RegisterHotKey` (`global-hotkey`) |
| macOS | `NSApplication` (accessory) | `tray-icon` (menu bar) | `global-hotkey` |
| Linux | channel (`recv`) | StatusNotifierItem (`ksni`) | X11 grabs, GlobalShortcuts portal (Wayland), `vixeeny-daemon ctl <action>` |

## Crates

| Crate | Role | OS code |
|---|---|---|
| `vixeeny-common` | config (`config.toml`, profiles), IPC, hotkey parsing, i18n table, paths, naming | no |
| `vixeeny-platform` | monitors/DPI, windows, clipboard, notifications, autostart helpers | yes |
| `vixeeny-capture` | still and video capture: WGC (Windows), ScreenCaptureKit (macOS), X11 / portals / PipeWire (Linux) | yes |
| `vixeeny-audio` | audio sources (WASAPI, ScreenCaptureKit, PipeWire), the time-driven `Mixer` | yes |
| `vixeeny-encode` | codec registry (`codecs/registry.toml`), validation, hardware probe, recorder, replay ring, denoiser | FFmpeg |
| `vixeeny-image` | PNG/JPEG (jpegli)/WebP/AVIF/JPEG XL encode and decode, HDR → SDR tone mapping | no |
| `vixeeny-editor` | the annotation model (tools, undo/redo, rendering) | no |
| `vixeeny-ocr` | `Engine` trait; Windows.Media.Ocr, Vision, Tesseract; language selection | yes |
| `vixeeny-stitch` | scrolling-capture assembly | no |
| `vixeeny-settings` | the settings model: rows, validation, reset | no |
| `vixeeny-ui` | Slint files and their Rust bindings | no |
| `vixeeny-updater` | release check, SHA-256 + minisign verification, file swap with rollback | no |
| `vixeeny-app`, `vixeeny-daemon` | the two programs | yes |
| `xtask` | `build-native`, `dist`, `dist-mac`, `dist-linux`, `icons`, `sums`, benchmarks | no |

Rule of thumb: logic that can be pure is pure and tested without a display, a GPU or an OS (the
editor, the stitcher, the settings, the mixer, the replay ring, the codec rules); the OS crates
are thin and have fakes (`FakeBackend`, `FakeAudioSource`, fake adapters for the probe).

## Capture and recording

- **Images**: a `StillBackend` grabs a monitor, a window or a region into a `CpuFrame` (BGRA).
  The `Capturer` stitches monitors and applies the HDR tone mapper. The Print Screen editor
  freezes one full capture, then shows it in a full-screen Slint window.
- **Video**: a stream per OS delivers frames with master-clock timestamps; the `Recorder`
  (`vixeeny-encode`) converts, encodes and muxes on its own threads, with constant or variable
  frame rate, pause, splitting on key frames and a crash-safe MP4. Windows can keep frames on the
  GPU (D3D11 → NVENC/AMF); elsewhere frames go through memory, with VAAPI/Vulkan uploads on Linux.
- **Audio**: every source delivers 48 kHz float chunks; one `Mixer` per track puts them on the
  master clock (gaps become silence, drift is corrected), then the recorder encodes the track.
- **Replay buffer**: the same pipeline writes encoded packets into a ring cut by key frames;
  saving remuxes the ring.

## Files and data

Settings: `config.toml` (versioned; unknown keys are kept). Logs, the hardware-probe cache and the
update state sit next to it (`vixeeny-common::paths`); a `portable.flag` next to the executables
keeps everything in a `data` folder. Nothing is sent anywhere except the GitHub release check.

## Packaging

`cargo xtask dist` (Windows zip + Inno Setup installer), `dist-mac` (`.app`, zip, dmg),
`dist-linux` (zip, deb, AppDir → AppImage; the Flatpak manifest is in `packaging/linux/flatpak`).
Releases are built by `.github/workflows/release.yml`, hashed (`SHA256SUMS*`) and signed with
minisign; the public key is embedded in `vixeeny-updater`.
