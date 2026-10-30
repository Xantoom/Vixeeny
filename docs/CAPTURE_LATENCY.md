# Zone capture latency

The first dimmed frozen frame must come from the daemon, without waiting for the app,
GPU readback, tone mapping, editor construction or an animation. These later steps still
prepare the interactive editor, using the original undimmed shared textures.

The path previously captured each duplicated monitor before starting the next one, with
up to 100 ms of acquisition wait per monitor. It then created the frozen windows and
swap chains, and closed WGC sessions, before revealing the image. The daemon's image
was undimmed; the editor only started its 160 ms veil fade after showing its first frame.
This let the desktop move ahead of the captured image while it was being prepared.

The freeze now prepares its cloaked windows, swap chains, shared textures and named handles
before sampling the desktop.
All capture sessions start before frame acquisition. Desktop Duplication shares one
16 ms acquisition budget across monitors; a missing frame falls back to WGC. The budget
limits acquisition waits, not session creation or the entire freeze. The daemon draws
the black veil directly into the presentation buffer using shaders created at startup.
It reveals the windows before closing WGC sessions. The editor commits its matching veil
before showing its windows, with no additional fade.

The shared original is never rendered into. SDR stays BGRA8, HDR stays scRGB half floats,
and captures with the pointer continue to use WGC. The software-D3D test
`freeze_veil::tests::the_first_frame_is_dimmed_without_changing_the_exported_pixels`
checks the displayed SDR/HDR values and that the source texture remains unchanged.

## Measure on Windows

Use an optimized build (`cargo build --profile quick`) with `RUST_LOG=debug` and compare
the same setup before and after the change. The daemon's `frozen screens submitted to
the compositor` event reports `prepared`, `captured`, `present` and `total`. The outer
`screen(s) frozen in` event also includes monitor enumeration and session cleanup.
These CPU timings do not measure the time from physical key-down to photons: the final
composition and display scanout still follow submission. Use a high-speed camera with
moving on-screen content for that end-to-end measurement and to check frame rollback.

Check first and subsequent presses, one and multiple monitors, different refresh rates,
SDR/HDR, pointer capture on/off, a moving desktop, GPU load and a display mode change.
Also check that the handoff does not flash, the selected/exported image is undimmed,
Escape closes the editor and the safety timeout still removes abandoned freezes.

## API constraints and further work

- [AcquireNextFrame](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe)
  can report pointer-only updates and takes a timeout. An expired shared deadline still
  performs a zero-timeout poll for a frame already available on a later monitor.
- [ReleaseFrame](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-releaseframe)
  invalidates the acquired surface, so its copy must be issued while the frame is owned.
- [DwmFlush](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmflush)
  blocks for outstanding presentation work. The existing wait before uncloaking remains
  to avoid exposing an empty first frame; it needs display-side validation before removal.
- Keeping monitor-specific windows, swap chains or capture sessions resident could reduce
  preparation further, with additional idle memory or capture work. Use the phase timings
  to establish whether this tradeoff is justified. Monitor/HDR caching also needs reliable
  invalidation across display changes, HDR toggles, sleep and device loss.

No end-to-end latency numbers have been measured for these changes in the Linux workspace.

## Switching a selected zone between live and frozen

The editor keeps its windows excluded from capture from setup through both modes, instead
of changing display affinity on each toggle. If exclusion is unavailable, the existing
cloak-and-wait fallback remains. Composition changes are committed before starting or
stopping annotation capture sessions.

Refreezing uses the same shared 16 ms duplication budget as the initial freeze. Its
DirectComposition tiles, visuals and upload bitmaps are reused while the image geometry
stays the same; only their pixels are updated. Retaining the upload bitmaps uses additional
GPU memory while the editor is open, released with the editor. Readback, conversion and
upload still occur synchronously: this is not a fully GPU-resident refreeze.

Live effect streams stay open during selection gestures and reuse the same monitor session
when only the crop changes. A retained full-monitor frame allows a new crop on a static
desktop without waiting for another content update. Unneeded sessions close on a worker
thread, after their retained frames are released.

Space and K ignore Windows key-repeat events, so holding either cannot trigger repeated
captures. A failed capture or changed desktop size leaves the editor live. Copy/save also
stop if they cannot obtain a fresh image, rather than exporting the original stale freeze.
The `editor live mode changed` debug event measures capture/upload time before the caller
refreshes the composition; it is not a physical key-to-display measurement.

Regression coverage includes retained tiles receiving new pixels, successful refreezing,
key-repeat handling, capture failures, desktop-size changes, stale-export prevention and
keeping live streams during a zone move. These Windows rendering/interaction tests must
also be run on Windows; cross-compilation only checks their code. Manually check repeated
toggles with blur/pixelation/highlight, static content, multiple monitors, HDR and exports.
