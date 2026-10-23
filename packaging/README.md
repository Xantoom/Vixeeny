# Packaging

- `windows/vixeeny.iss` — Inno Setup script: for all users (Program Files; a scheduled task
  of the user then starts Vixeeny with the administrator's rights) or for the user alone (their
  own folders, no administrator rights). An install for all users removes the user's own one.
- `minisign.pub` — the public key updates are verified with, embedded in `vixeeny-updater`
  (the library of `vixeeny-app` that downloads and installs updates). While it contains the
  word `unconfigured`, every update is refused.
- `icons/` — the logo at every size and the `.ico` files (`cargo xtask icons` draws them).
- `.github/workflows/release.yml` — builds, packages, signs and drafts the release `v<version>`; built when the version of `Cargo.toml` changes on the release branch
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
| `Vixeeny-<v>-setup.exe` | Installer; its signature is in the notes too (Settings → General downloads it to move an install to Program Files) |
| `Vixeeny-<v>-windows-x64-portable.zip` | Portable: the programs plus `portable.flag` (settings in `data\`); also what an update downloads |

The minisign signature of the portable archive is in the release notes, in a comment the page
does not show (`<!-- minisign <file name>` … `-->`): keep it when the notes are edited.

Updates replace the files of the install folder with those of the portable archive (all but
`portable.flag`), for the installed and the portable layout alike, while Vixeeny runs (Windows lets a running program be
renamed); only `Vixeeny.exe` restarts. The previous files are kept in `.update-backup` until the
new `Vixeeny.exe` answers, and put back if it does not.
