# Adding a codec

Everything the application knows about encoders is data in
[`crates/vixeeny-encode/codecs/registry.toml`](../crates/vixeeny-encode/codecs/registry.toml).
Example: H.266 through VVenC, or AV2 when it exists.

1. Add the library to `native/versions.toml` (repository, commit/version, licence) and its build
   recipe in `xtask/src/native.rs`. Check that the licence is compatible with GPL-3.0.
2. Enable the encoder in the FFmpeg build (`--enable-lib…`); update FFmpeg if the encoder only
   exists in a newer version.
3. Add the family (`vvc`, `av2`) to `Family` in `src/registry.rs`, to the container matrix
   (plan annex 13.2) and to the rules of `src/validate.rs` (frame-rate limit, HDR).
4. Add an `[[encoder]]` entry to `registry.toml`: pixel formats, containers, rate control, the
   four presets (plain FFmpeg options) and the advanced parameters.
5. Add the translation keys (`encoder.param.*`).
6. Run `cargo xtask verify-registry`: it compares every option, enum value, preset option and
   pixel format with what the linked FFmpeg really exposes. Then `cargo test`.
7. For a new hardware generation, usually only step 2 (newer FFmpeg / SDK headers) and the pixel
   formats of the entry are needed: the hardware probe (`vixeeny-app --probe-report`) detects
   what the GPU really supports.
