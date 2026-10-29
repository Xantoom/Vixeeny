# Performance audit — 2026-10-30

The audit covers the daemon and app lifecycle, capture/readback, the annotation editor, image export,
scrolling capture, audio mixing/encoding, video recording/replay, and settings notifications. The baseline
is commit `5fd24ec`; the results below compare that code with the accompanying working-tree changes.

## Corrections

| Area | Finding | Change |
|---|---|---|
| Editor conversion | Even a small BGRA image created and joined up to eight OS threads. | Convert images below 65,536 pixels on the caller; use the existing Rayon worker pool for larger images. |
| Blur | Both axes created new OS threads; even the single-worker path spawned a thread. | Run small regions directly and reuse Rayon workers for large regions. The blur passes, rounding and edge replication are preserved. |
| Audio mixer | Every 20 ms block drained the front of a `Vec`, shifting all later samples for each source. Buffered bursts therefore had quadratic copying cost. | Use a circular queue and mix its two contiguous slices. Drop stale samples before channel conversion. |
| Audio encoder | Every codec frame was collected into a fresh allocation, shifting the remaining FIFO contents. | Encode complete frames from the input slice; buffer only the incomplete tail and reuse that allocation. |
| Capture and recording | CPU frames refused by the capture queue, discarded by the media clock, or received while paused freed their large allocations instead of recycling them. | Return these buffers to the same bounded pool as successfully converted frames. GPU frames retain their existing lifetime rules. |
| Scroll matching | Signatures updated a scattered accumulator for every pixel. | Sum contiguous bands, retaining the exact original column-to-band mapping, including narrow and uneven widths. |
| HDR scroll layers | Improving an existing canvas row allocated its HDR layer again. | Copy into an existing layer of the same length. |

The pool corrections address allocation churn, not a permanent memory leak: the previous code freed the
discarded buffers. One tightly packed 4K BGRA frame holds 31.6 MiB; half-float HDR holds twice that.
The pool still retains at most two spare buffers. No compression, quality, capture-rate or crash-recovery
setting was reduced to obtain the gains.

## Measurements

Windows x64, Ryzen 7 9800X3D, Rust 1.99.1, `quick` profile (optimized, no LTO), cross-compiled with
`cargo-xwin` and executed natively on Windows from WSL. Inputs are deterministic synthetic images/audio.
Benchmarks ran sequentially after the test suite, with no concurrent build. Each number is a median of
seven samples/batches. Editor measurements additionally use the median of three process runs, with the
before/after order reversed in the last pair. Default worker counts were used.

| Operation | Before | After | Interpretation |
|---|---:|---:|---|
| BGRA → RGBA, 128 × 128 | 0.3820 ms | 0.0096 ms | About 40× faster |
| BGRA → RGBA, 1920 × 1080 | 0.4700 ms | 0.2688 ms | 43% less time |
| BGRA → RGBA, 3840 × 2160 | 0.9498 ms | 0.9044 ms | Small gain; close to memory-bandwidth limits |
| BGRA → RGBA, two 4K screens | 1.7874 ms | 1.7833 ms | Essentially unchanged |
| Blur, 128 × 128, sigma 8 | 0.7839 ms | 0.5857 ms | 25% less time |
| Blur, 1920 × 1080, sigma 8 | 19.2774 ms | 19.0988 ms | Essentially unchanged |
| Mix 1 s buffered audio, four stereo sources | 0.3143 ms | 0.0999 ms | 68% less time |
| Mix 10 s buffered audio, four stereo sources | 70.8895 ms | 1.7124 ms | 98% less time |
| Simulate 60 s streaming audio, four stereo sources | 6.4554 ms | 4.1091 ms | 36% less CPU time for the simulated minute |
| Scroll assembly, 1920 × 1080 | 2.1775 ms/frame | 1.8270 ms/frame | 16% less time |
| Scroll assembly, 3840 × 2160 | 6.3499 ms/frame | 6.2878 ms/frame | Essentially unchanged |

These timings isolate particular operations. They are not end-to-end shortcut latency, a recording FPS
increase, or a promise for every machine. In particular, the audio burst benchmark exercises a backlog;
the streaming benchmark is the more representative steady-state workload. Large-image results vary
with scheduling and memory bandwidth, so sub-5% differences are not treated as significant gains.
Buffer recycling and encoder FIFO improvements are verified by regression tests; no separate time or
RAM reduction is attributed to them.

Machine-readable measurements are in [performance-2026-10-30.json](performance-2026-10-30.json).

## Validation

- `cargo fmt --all --check` and `git diff --check` pass.
- Windows `cargo clippy --workspace --all-targets -- -D warnings` passes.
- Windows `cargo test --workspace` passes: **633 tests, 0 failures, 0 ignored**. Some hardware tests can
  return successfully when the required hardware is absent; this count is not a cross-vendor certification.
- The seven new tests cover padded serial/parallel pixel conversion, uneven signature bands, audio
  overlap across a wrapped queue, partially stale surround audio, chunk-independent encoded packet
  bytes/timestamps, and buffer recycling at capture refusal and media-clock/pause rejection.
- The existing tests cover blur output, annotations/snapshots, scrolling with sticky overlays, hour-long
  audio clock drift, pause/resume, codecs/containers, replay, splitting and crash-safe recording.
- A separate capture smoke run with output enabled exercised both connected 3840 × 2160 monitors,
  window streaming and capture gating. **There was no HDR monitor**: HDR algorithms have synthetic tests,
  but actual HDR desktop capture was not exercised.

Reproduce the CPU benchmarks on Windows:

```powershell
cargo run --profile quick -p vixeeny-editor --example bench_editor
cargo run --profile quick -p vixeeny-audio --example bench_audio
cargo run --profile quick -p vixeeny-stitch --example bench_stitch
```

The editor and stitch benchmarks also run directly on Linux. The workspace as a whole targets Windows;
use the cross-compilation setup in [CONTRIBUTING.md](../CONTRIBUTING.md) from WSL. Native Windows tests
can be launched through a runner that executes the `.exe` directly; `cargo-xwin` otherwise defaults to
Wine. The FFmpeg DLLs must be available beside the test executables. For an actual daemon build under
WSL, set `RC_PATH` to the installed `llvm-rc` executable so the required tray-icon resources are linked.

## Other reviewed paths and remaining profiling

| Path | Audit result / next measurement |
|---|---|
| Startup and idle | The main daemon loop blocks in `GetMessageW`; the app waits on IPC and exits after its configured idle interval. The capture device is deliberately kept warm. See the idle observation below. |
| UI/settings | Change notifications use `Bell`/OS notifications. Settings writes are debounced, and native decoration is reapplied only when the look changes. No continuous refresh loop was introduced. |
| Still/HDR export | PNG bands and tone mapping already use Rayon; GPU capture/annotation paths already avoid CPU round trips. Remaining codec time depends on image content and selected quality. HDR hardware and native image-codec feature builds were not independently benchmarked. |
| Video/replay | Capture pacing rejects unwanted frames before readback/conversion; recorder queues are bounded; GPU paths and reference-counted replay packets are already present. Follow up with sustained 4K recording, disk replay and capture-to-file latency distributions on actual vendor hardware. |
| Scroll worst case | Correctness tests cover lost overlap and recovery. An exhaustive search after losing position can still scan the full assembled page. The new benchmark measures successful sequential scrolling, not that worst case. |
| Thumbnail/export UI | The video editor decodes its thumbnail strip and runs exports on background workers; notification thumbnails use a separate path. Measure long clips and large annotated scroll exports before introducing additional caches or changing decode/quality settings. |
| Updates/logging | Updates run in separate processes, use bounded download chunks and resume partial downloads. Default capture/encode loops do not log every frame. Logs rotate and limit repeated lines. |

### Idle observation

An optimized daemon was started with a private IPC endpoint and portable data directory, autostart,
hotkeys, notifications and updates disabled, then stopped automatically. Three approximately 21-second
samples after a five-second settle measured **54.5–55.2 MiB committed private memory**, **42.6–46.3 MiB
working set**, 30 threads, and **0–0.146% of one CPU core**. The nonzero CPU readings correspond to about
one or two 15.6 ms accounting ticks, so these short runs cannot establish a precise sub-0.05% CPU budget.

There were about 5,800 context switches/minute, almost all on one waiting thread. A follow-up query of
that thread's start address maps it to **`nvwgf2umx.dll` (NVIDIA's user-mode driver)**, rather than the
daemon's event loop. Keeping the capture device ready thus has an observable driver cost on this machine;
releasing it would need a separate capture-latency/power tradeoff evaluation. Context switches are not
equivalent to timer wakeups. The existing `xtask bench-idle` also labels `WorkingSetPrivate` as
private bytes and compares it against a 15 MiB limit, whereas the architecture documents the cost of a
warm Direct3D device at about 55 MB. These metrics and budgets must be distinguished before treating
that command's verdict as a performance certification. No idle-memory reduction is claimed here.

Long battery/power measurements, cold-start latency, HDR desktop capture and the full NVENC/AMF/QSV
hardware matrix remain outside these synthetic measurements.
