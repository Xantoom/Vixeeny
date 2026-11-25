# Vixeeny

Screenshots and screen recording for Windows 10 and 11, in Rust. Two programs: a tray daemon that waits for
shortcuts and captures, and the app (settings, editor, updates). Read this before changing anything; the details are in
[CONTRIBUTING.md](CONTRIBUTING.md) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Commands

The workspace builds for Windows only (`x86_64-pc-windows-msvc`). Every build links FFmpeg (see CONTRIBUTING.md).

```
cargo xtask build-native ffmpeg
cargo fmt --all --check
cargo clippy --workspace --lib --bins --tests --examples --features vixeeny-app/native-codecs -- -D warnings
cargo test --workspace --tests --features vixeeny-app/native-codecs
cargo build --profile quick
```

- From Linux or WSL, run the same commands through `cargo xwin … --target x86_64-pc-windows-msvc`. Only `xtask` and the
  pure crates (such as `vixeeny-editor`) build natively.
- CI checks with `--features vixeeny-app/native-codecs`: without it the native image codecs and bindgen's generated code
  are not built, and a change can pass locally yet fail in CI.
- After `cargo update -p …`, read the whole `git diff Cargo.lock`: it can also move range dependencies.
- New dependencies must pass `cargo deny check`. Native libraries are recorded in `native/versions.toml`.
- `cargo test -p vixeeny-ui` renders every window without a display; `VIXEENY_SCREENSHOTS=<folder>` keeps the images.

## Layout

- `crates/vixeeny-daemon` and `crates/vixeeny-app`: the two programs.
- `crates/vixeeny-common`: shared types, config, IPC and every user-facing string (`src/i18n.rs`, English and French,
  see [docs/TRANSLATING.md](docs/TRANSLATING.md)).
- `crates/vixeeny-capture`, `-encode`, `-audio`, `-image`, `-stitch`: capture, video and audio encoding, image formats,
  scrolling capture.
- `crates/vixeeny-editor`: the annotation editor's pure logic. `crates/vixeeny-overlay`: native windows (zone editor,
  side strip, notifications, recording widget). `crates/vixeeny-ui`: Slint windows. `crates/vixeeny-settings`: the
  settings model without UI. `crates/vixeeny-platform`: Windows APIs. `crates/vixeeny-updater`: updates.
- `xtask/`: native builds, release files (`dist`), crash symbolization. `native/`, `packaging/`: native libraries,
  installer, FFmpeg build.
- A new codec is data: see [docs/ADDING_A_CODEC.md](docs/ADDING_A_CODEC.md).

## Rules of the product

- Lowest latency first, then the least RAM, CPU and disk. A saving that makes the user wait is not an improvement;
  extra CPU is fine when it buys speed, with the measured cost and gain stated.
- No polling anywhere: no timer, sleep loop or periodic wake-up. Block on Windows events instead (WinEvent hooks,
  `WaitForMultipleObjects`, `RegNotifyChangeKeyValue`, device notifications, blocking channels). The only timers are
  the ones the user asks for, such as a countdown.
- Encoder settings fit the hardware (GPU generation, driver, probed capabilities) and the other settings, and are
  backed by measured tests. A setting that costs a lot of throughput for a small quality gain is not worth it.
- An SDK or dependency bump that drops older hardware is only worth it if Vixeeny uses what it adds.
- No console window, ever. No paid service or certificate: updates are verified with minisign.

## Rules of the code

- Logic that can be pure is pure, in a crate without OS code, with tests. A bug fix comes with a test that fails
  without it.
- No `unsafe` without a `// SAFETY:` comment, no `unwrap` outside tests. Comments wrap at 120 columns.
- Code, comments, commits and release notes are in English. Commits: short imperative subject (`feat(editor): …`,
  `fix: …`, `docs: …`).
- Comments and docs describe the code as it is: no plans, milestones or review notes in the repository.
- Releases start from a version bump in `Cargo.toml` on branch `1.0`. Release notes are short user-facing bullets under
  headings such as `## Settings` and `## Fixes`, in `changelog/<version>/en.md` and `fr.md` (built into the programs;
  GitHub shows the English ones).

## Rules of the interface

- Show a state with colour, an icon or a short tooltip on hover, not with an extra line of text.
- Neutral wording: say what a setting does and what it costs, never sell it.
- Shortcut keys are written as they are: `CTRL + SHIFT + R`, `SHIFT` (not `Maj`), no keycap pills.
- Every string goes through `i18n.rs`, in both languages.
