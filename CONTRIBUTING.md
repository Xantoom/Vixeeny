# Contributing

Thanks for helping. The short version: open an issue first for anything big, keep changes small,
and make sure the checks below pass.

## Build and check

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Clippy runs with warnings as errors (the lints are in the workspace `Cargo.toml`). Code that is
specific to an OS is behind `cfg`, so a plain run on Linux does not compile the Windows and macOS
parts: CI does (three operating systems), and you can check them locally with
`--target x86_64-pc-windows-msvc` or `--target aarch64-apple-darwin` (add `CC_<target>`/`AR_<target>`
set to a stub if a C build script complains, nothing is linked by `check`).

Linux needs `libfontconfig1-dev`; PipeWire support needs `libpipewire-0.3-dev libspa-0.2-dev clang`
(`--features pipewire` on `vixeeny-app`).

Release builds also need the pinned native libraries (FFmpeg, codecs): `cargo xtask build-native`
(see `native/versions.toml`), then `--features "ffmpeg native-codecs"` on `vixeeny-app`.

## Rules the project holds to

- **Windows is the priority**; macOS and Linux are tested by volunteers. Do not break Windows for
  them, and say in the pull request what you could and could not test.
- **Logic that can be pure is pure** and has tests: put it in a crate without OS code. A bug fix
  comes with a test that fails without it.
- **No unsafe without a `// SAFETY:` comment**, no `unwrap` in non-test code (clippy enforces it).
- **Licences**: GPL-3.0-or-later project; a new dependency must pass `cargo deny check`
  (`deny.toml` lists the allowed licences). Record native libraries in `native/versions.toml`.
- **Text for users** lives in `crates/vixeeny-common/src/i18n.rs` (English and French);
  see [docs/TRANSLATING.md](docs/TRANSLATING.md). A new codec is data: see
  [docs/ADDING_A_CODEC.md](docs/ADDING_A_CODEC.md).
- **Decisions and deviations** from [VIXEENY_PLAN.md](VIXEENY_PLAN.md) go in its journal (the
  table at the end), with the reason.
- Commits: a short imperative subject (`feat(linux): …`, `fix: …`, `docs: …`).

## Reporting a bug

Use the issue templates and paste **Settings → About → Copy system info**
(or `vixeeny-app --system-info`). See [docs/BETA.md](docs/BETA.md).
