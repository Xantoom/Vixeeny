# Known limitations

See section 8 of [VIXEENY_PLAN.md](../VIXEENY_PLAN.md).

## FFmpeg on Windows: shared libraries

The Windows packages ship FFmpeg as DLLs (`avcodec-*.dll`, …) next to the executables, from a
pinned prebuilt GPL build (see `native/versions.toml`: URL, tag and SHA-256). Linking it
statically was considered and dropped: it would save some tens of megabytes but cost hours of CI
and a build of our own to maintain, and the installer is already well under the 120 MB target.
The corresponding source is available from the pinned build's project; the licences are listed in
`THIRD-PARTY-LICENSES.txt` inside every package.

## Linux

- **No recording widget (pause / stop pill).** Neither X11 nor the Wayland portals let a window
  be left out of a screen capture, so the pill would appear in the video. The tray icon and the
  shortcuts control the recording instead.
- **No layer-shell overlay.** The region selector is a full-screen window; Slint (the UI toolkit)
  has no layer-shell support. On some compositors it may be tiled or lose focus; report it.
- **Wayland**: capture goes through the portals, which ask for the screen or window to share
  (the choice is remembered where the portal supports it); the captured application's name is not
  known, so file names use "Screen". Global shortcuts need the GlobalShortcuts portal, else bind
  `vixeeny-daemon ctl <action>` in the compositor.
- **Scrolling capture** works on X11 only: Wayland portals cannot grab a zone fifteen times a
  second, so it is refused there with a message.
- **No zero-copy capture** (DMA-BUF): frames go through memory, which costs CPU at high
  resolutions. Hardware encoding (VAAPI, Vulkan) uses the default render device.
- **Clipboard** needs `wl-clipboard` or `xclip` (bundled in the Flatpak); **OCR** needs Tesseract
  and a language pack (the Flatpak has English and French only).
- **Updates**: the update archive only replaces a portable install; packages, AppImage and
  Flatpak update through their own channel.

## macOS

- Two builds (Apple silicon and Intel, no universal binary), not notarized (right-click → Open the first
  time).
- SDR and stereo only. The recording pill is a floating window that shows on every Space; it is
  kept out of the video by `sharingType = none`, but a plain window cannot be made non-activating
  after creation, so clicking it may bring Vixeeny forward.
- Scrolling capture needs the Screen Recording permission like any capture.
