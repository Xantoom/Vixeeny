# Known limitations

See section 8 of [VIXEENY_PLAN.md](../VIXEENY_PLAN.md).

## Windows only

Vixeeny runs on Windows 10 and 11, x64. The macOS and Linux
builds of 0.9.0 and 0.9.1 were dropped in 0.9.2: the project focuses on one system done well.

## FFmpeg as shared libraries

The packages ship FFmpeg as DLLs (`avcodec-*.dll`, …) next to the executables, from a pinned
prebuilt GPL build (see `native/versions.toml`: URL, tag and SHA-256). Linking it statically was
considered and dropped: it would save some tens of megabytes but cost hours of CI and a build of
our own to maintain, and the installer is already well under the 120 MB target. The
corresponding source is available from the pinned build's project; the licences are listed in
`THIRD-PARTY-LICENSES.txt` inside every package.

## Not signed with a certificate

The installer and the programs carry no Authenticode signature (it costs money every year), so
SmartScreen may warn the first time: "More info" → "Run anyway". Updates are verified with
minisign and SHA-256 before anything is replaced.
