# Known limitations

See section 8 of [VIXEENY_PLAN.md](../VIXEENY_PLAN.md).

## FFmpeg on Windows: shared libraries

The Windows packages ship FFmpeg as DLLs (`avcodec-*.dll`, …) next to the executables, from a
pinned prebuilt GPL build (see `native/versions.toml`: URL, tag and SHA-256). Linking it
statically was considered and dropped: it would save some tens of megabytes but cost hours of CI
and a build of our own to maintain, and the installer is already well under the 120 MB target.
The corresponding source is available from the pinned build's project; the licences are listed in
`THIRD-PARTY-LICENSES.txt` inside every package.
