# Known limitations

See section 8 of [VIXEENY_PLAN.md](../VIXEENY_PLAN.md).

## Windows only

Vixeeny runs on Windows 10 and 11, x64. The macOS and Linux
builds of 0.9.0 and 0.9.1 were dropped in 0.9.2: the project focuses on one system done well.

## FFmpeg as shared libraries

The packages ship FFmpeg as DLLs (`avcodec-*.dll`, …) next to the executables, built by
`packaging/ffmpeg/build.sh` from a pinned FFmpeg commit inside the pinned BtbN build image, with
only the parts Vixeeny uses. Linking it statically was considered and dropped: it would save
some tens of megabytes but cost hours of CI, and the installer is already well under the 120 MB
target. The licences are listed in `THIRD-PARTY-LICENSES.txt` inside every package.

## Graphics card encoders

- NVIDIA: NVENC needs driver 570 or newer. FFmpeg is built with the SDK 13.0 headers: those of
  13.1 ask for driver 610, which the GTX 900 and 10 series (stopped at 580) will never get.
- AMD RX 7000, AV1 in 1080p: the encoder works in blocks of 64 × 16 pixels and writes 1082
  lines. The file tells the player to crop the two extra lines (MP4 and MKV); a player that
  ignores it shows a thin black stripe at the bottom. Fixed in hardware on the RX 9000.

## Not signed with a certificate

The installer and the programs carry no Authenticode signature (it costs money every year), so
SmartScreen may warn the first time: "More info" → "Run anyway". Updates are verified with
minisign and SHA-256 before anything is replaced.
