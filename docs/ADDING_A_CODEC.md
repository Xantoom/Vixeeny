# Adding a codec

Everything the application knows about encoders is data in
[`crates/vixeeny-encode/codecs/registry.toml`](../crates/vixeeny-encode/codecs/registry.toml).
Example: H.266 through VVenC, or AV2 when it exists.

1. Check that the encoder's licence is compatible with GPL-3.0.
2. Point `[ffmpeg]` in `native/versions.toml` at a prebuilt FFmpeg that includes the encoder (URL,
   tag, SHA-256, and the bundled libraries in `license`), then run `cargo xtask build-native`.
3. Add the family (`vvc`, `av2`) to `Family` in `src/registry.rs`, to the container matrix
   (plan annex 13.2) and to the rules of `src/validate.rs` (frame-rate limit, HDR).
4. Add an `[[encoder]]` entry to `registry.toml`: pixel formats, containers, rate control, the
   four presets (plain FFmpeg options) and the advanced parameters.
5. Add the translation keys (`encoder.param.*`).
6. Run `cargo xtask verify-registry`: it compares every option, enum value, preset option and
   pixel format with what the linked FFmpeg really exposes. Then `cargo test`.
7. For a new hardware generation, usually only step 2 (a newer FFmpeg) and the pixel formats of
   the entry are needed: the hardware probe (`vixeeny-app --probe-report`) detects
   what the GPU really supports.
