# Architecture

The design and its reasons are in [VIXEENY_PLAN.md](../VIXEENY_PLAN.md) (section 4 and the
decision log at the end); this page is the map. Vixeeny is a Windows program (10 and 11, x64).

## Two programs

```
Vixeeny.exe (always on)                         vixeeny-app.exe (on demand)
  tray icon, global shortcuts, IPC server  ←──→  captures, editor, recording, settings…
  no Slint, no FFmpeg, no capture code            started by Vixeeny.exe for an action,
  idles at ~0 % CPU and a few MB                  exits after `app_idle_exit_seconds`
```

`Vixeeny.exe` is small so that being resident costs nothing; everything heavy lives in the app,
which it starts when a shortcut fires and which exits when idle. They talk over a named pipe
with length-prefixed `postcard` messages (`vixeeny-common::ipc`). Binding the pipe is also the
single-instance lock. Some work runs in processes of their own, all `vixeeny-app.exe` with a
flag: the settings window (`--settings`), the recording pill (`--widget`), toasts (`--toast`),
the hardware probe (`--probe`) and updates (`--update`). Both programs use the Windows
subsystem: no console window ever shows. Every window carries the same AppUserModelID
(`Xantoom.Vixeeny`), so the taskbar and Task Manager show them as "Vixeeny".

The daemon's logic is a pure state machine (`core.rs`: `Event` → `Effect`) driven by a small
runtime (`runtime.rs`); the platform layer (`platform/`) is a message-only window, the tray
(`tray-icon`) and `RegisterHotKey` (`global-hotkey`).

## Crates

| Crate | Role | OS code |
|---|---|---|
| `vixeeny-common` | config (`config.toml`), IPC, hotkey parsing, i18n table, paths, naming | no |
| `vixeeny-platform` | monitors/DPI, windows, clipboard, notifications | yes |
| `vixeeny-capture` | still and video capture through Windows.Graphics.Capture | yes |
| `vixeeny-audio` | WASAPI sources (loopback, microphones, per process), the time-driven `Mixer` | yes |
| `vixeeny-encode` | codec registry (`codecs/registry.toml`), validation, hardware probe, recorder, replay ring, denoiser | FFmpeg |
| `vixeeny-image` | PNG/JPEG (jpegli)/WebP/AVIF/JPEG XL encode and decode, HDR → SDR tone mapping | no |
| `vixeeny-editor` | the annotation model (tools, undo/redo, rendering) | no |
| `vixeeny-stitch` | scrolling-capture assembly | no |
| `vixeeny-settings` | the settings model: pages, rows, validation, reset | no |
| `vixeeny-ui` | Slint files and their Rust bindings | no |
| `vixeeny-updater` | release check, download with progress, SHA-256 + minisign verification, file swap with rollback | no |
| `vixeeny-app`, `vixeeny-daemon` | the two programs | yes |
| `xtask` | `build-native`, `dist`, `sums`, `icons`, `verify-registry`, `bench-idle` | no |

Rule of thumb: logic that can be pure is pure and tested without a display, a GPU or an OS (the
editor, the stitcher, the settings, the mixer, the replay ring, the codec rules); the OS crates
are thin and have fakes (`FakeBackend`, `FakeAudioSource`, fake adapters for the probe). The
Slint windows are rendered in the tests by the software renderer.

## Capture and recording

- **Images**: a `StillBackend` grabs a monitor, a window or a region into a `CpuFrame` (BGRA).
  The `Capturer` stitches monitors and applies the HDR tone mapper. The capture shortcut freezes
  every screen at once: one overlay window per monitor shows the screen as it was and dims it,
  then the editor works on that image.
- **Video**: Windows.Graphics.Capture delivers frames with master-clock timestamps; the
  `Recorder` (`vixeeny-encode`) converts, encodes and muxes on its own threads, with constant or
  variable frame rate, pause, splitting on key frames and a crash-safe MP4. Frames can stay on
  the GPU (D3D11 → NVENC/AMF).
- **Audio**: every source delivers 48 kHz float chunks; one `Mixer` per track puts them on the
  master clock (gaps become silence, drift is corrected), then the recorder encodes the track.
- **Replay buffer**: the same pipeline writes encoded packets into a ring cut by key frames;
  saving remuxes the ring.

## Updates

`Vixeeny.exe` starts `vixeeny-app --update background` shortly after it starts, then once a day.
That process asks GitHub for the latest release and, when automatic updates are on, downloads
the archive, checks its SHA-256 and minisign signature, unpacks it into `.update-staged` and
installs it; otherwise it shows a toast once. Settings → About shows the same steps with a
progress bar and a restart button. Installing renames the running files into `.update-backup`
(Windows allows renaming a running program), moves the new ones in, restarts `Vixeeny.exe` only,
and puts the old files back if the new version does not answer within 20 seconds. A recording
in progress delays the restart until it ends.

## Files and data

Settings: `config.toml` (versioned; unknown keys are kept). Logs, the hardware-probe cache and the
update state sit next to it (`vixeeny-common::paths`); a `portable.flag` next to the executables
keeps everything in a `data` folder. Nothing is sent anywhere except the GitHub release check.

## Packaging

`cargo xtask dist` gathers the programs, the FFmpeg DLLs and the licences into `dist/stage` and
writes the update archive and the portable archive; Inno Setup (`packaging/windows/vixeeny.iss`)
builds the per-user installer. Releases are built by `.github/workflows/release.yml`, hashed
(`SHA256SUMS`) and signed with minisign; the public key is embedded in `vixeeny-updater`.
