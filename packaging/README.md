# Packaging

- `windows/vixeeny.iss` — Inno Setup script (per-user installer, no administrator rights).
- `minisign.pub` — the public key updates are verified with, embedded in `vixeeny-updater`
  (the library of `vixeeny-app` that downloads and installs updates). While it contains the
  word `unconfigured`, every update is refused.
- `icons/` — the logo at every size and the `.ico` files (`cargo xtask icons` draws them).
- `.github/workflows/release.yml` — builds, packages, signs and drafts the release `v<version>`; run it by hand on the release branch (`gh workflow run release.yml --ref 1.0`)
  (or builds without publishing on a manual run).

## One-time setup by the maintainer (signing key)

```
cargo install rsign2
rsign generate -p packaging/minisign.pub -s vixeeny.key
```

Commit `packaging/minisign.pub`; keep `vixeeny.key` secret. In the repository settings add the
secrets `MINISIGN_KEY` (the content of `vixeeny.key`) and `MINISIGN_PASSWORD`.

## Files of a release

| File | Use |
|---|---|
| `Vixeeny-<v>-setup.exe` | Installer |
| `Vixeeny-<v>-windows-x64.zip` | Update archive (what the app downloads) |
| `Vixeeny-<v>-windows-x64-portable.zip` | Portable: same files plus `portable.flag` (settings in `data\`) |
| `*.minisig` | minisign signature of each archive and of the installer |
| `SHA256SUMS` | SHA-256 of every file above |

Updates replace the files of the install folder with those of the update archive, for the
installed and the portable layout alike, while Vixeeny runs (Windows lets a running program be
renamed); only `Vixeeny.exe` restarts. The previous files are kept in `.update-backup` until the
new `Vixeeny.exe` answers, and put back if it does not.

The archive also holds `vixeeny-daemon.exe`, a copy of `Vixeeny.exe` under its name before 0.9.2:
the updater of those versions restarts the program by that name, and the copy hands over to
`Vixeeny.exe`.
