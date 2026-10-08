# Adding a codec

Everything the application knows about encoders is data in
[`crates/vixeeny-encode/codecs/registry.toml`](../crates/vixeeny-encode/codecs/registry.toml).
Example: H.266 through VVenC, or AV2 when it exists.

1. Check that the encoder's licence is compatible with GPL-3.0.
2. Add the encoder to `ENCODERS` in `packaging/ffmpeg/build.sh` (and its library to the build if
   the BtbN image does not have it). Build it locally with `packaging/ffmpeg/build.sh
   native/build/work/ffmpeg.zip`, then run `cargo xtask build-native ffmpeg`; the CI rebuilds it
   by itself when the script changes.
3. Add the family (`vvc`, `av2`) to `Family` in `src/registry.rs`, to the container matrix
   (plan annex 13.2) and to the rules of `src/validate.rs` (frame-rate limit, HDR).
4. Add an `[[encoder]]` entry to `registry.toml`: pixel formats, containers, the four presets
   (plain FFmpeg options), the rate control of the custom preset (the quality scale and the
   options of each mode: constant quality, VBR, CBR) and the other parameters. The header of
   the file explains the fields.
5. Add the labels of the new parameters and of their values to `param_label` and `value_label`
   in `crates/vixeeny-settings/src/encoders.rs`.
6. Run `cargo xtask verify-registry`: it compares every option, enum value, preset option, rate
   mode option and pixel format with what the linked FFmpeg really exposes. Then `cargo test`:
   `software_encode` opens every encoder in each of its rate modes (the hardware ones when the
   machine has them).
7. For a new hardware generation, usually only step 2 (a newer FFmpeg) and the pixel formats of
   the entry are needed: the hardware probe (`vixeeny-app --probe-report`) detects
   what the GPU really supports.
