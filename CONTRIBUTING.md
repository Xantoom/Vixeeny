# Contributing

Thanks for helping. The short version: open an issue first for anything big, keep changes small,
and make sure the checks below pass.

## Build and check

On Windows, with the MSVC toolchain:

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Clippy runs with warnings as errors (the lints are in the workspace `Cargo.toml`). Vixeeny is a
Windows program; from Linux or WSL the workspace can be checked and its tests run with
[`cargo-xwin`](https://github.com/rust-cross/cargo-xwin) (`cargo xwin clippy --target
x86_64-pc-windows-msvc …`).

Release builds also need the pinned native libraries: `cargo xtask build-native` from a Visual
Studio developer prompt (it downloads the prebuilt FFmpeg and builds SVT-AV1, dav1d and the image
libraries; see `native/versions.toml`), then set `FFMPEG_DIR=native\build\work\ffmpeg-prebuilt`
and `PKG_CONFIG_PATH=native\build\work\prefix\lib\pkgconfig` and build `vixeeny-app` with
`--features "ffmpeg native-codecs"`. `cargo xtask dist` gathers the release files.

The UI tests render every window without a display; set `VIXEENY_SCREENSHOTS=<folder>` to keep
the images (`cargo test -p vixeeny-ui`).

## Rules the project holds to

- **Logic that can be pure is pure** and has tests: put it in a crate without OS code. A bug fix
  comes with a test that fails without it.
- **No console window, ever**: both programs use the Windows subsystem, and anything they start
  is a windowed program or runs with `CREATE_NO_WINDOW`.
- **No unsafe without a `// SAFETY:` comment**, no `unwrap` in non-test code (clippy enforces it).
- **Licences**: GPL-3.0-or-later project; a new dependency must pass `cargo deny check`
  (`deny.toml` lists the allowed licences). Record native libraries in `native/versions.toml`.
- **Text for users** lives in `crates/vixeeny-common/src/i18n.rs` (English and French);
  see [docs/TRANSLATING.md](docs/TRANSLATING.md). A new codec is data: see
  [docs/ADDING_A_CODEC.md](docs/ADDING_A_CODEC.md).
- **Decisions and deviations** from [VIXEENY_PLAN.md](VIXEENY_PLAN.md) go in its journal (the
  table at the end), with the reason.
- Commits: a short imperative subject (`feat(editor): …`, `fix: …`, `docs: …`).

## Reporting a bug

Use the issue templates and paste **Settings → About → Copy system info**
(or `vixeeny-app --system-info`). See [docs/BETA.md](docs/BETA.md).
