# Packaging

- `windows/vixeeny.iss` — Inno Setup script (7.1.1 or later: Windows 11's style, light or dark):
  for all users (Program Files; a scheduled task of the user then starts Vixeeny with the
  administrator's rights) or for the user alone (their own folders, no administrator rights).
  An install for all users removes the user's own one. The installer asks for the mode, then the
  folder, the start with Windows and the updates on one page (written by `vixeeny-app
  --first-install` as the user who installs). When Vixeeny is already installed, a page offers to
  update it (older), repair it (same version) or install over it (newer), or to uninstall it.
  The uninstaller asks first whether to keep the settings; without them nothing of Vixeeny stays
  (settings, logs, caches, the downloaded installer, the files updates brought), and the Print
  Screen key goes back to Windows' Snipping Tool when Vixeeny took it. Captures stay.
- `windows/versions.iss` — the order of versions for the installer's code; `versions-test.iss`
  tests it (CI runs it).
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
| `latest.json` | The version, its notes in English and French, and the files above with their size and signature: what the updater reads first |
| `Vixeeny-<v>-symbols.zip` | Not for users: the linker's maps of the two programs, which name the addresses of a crash of this version (`cargo xtask symbolize`) |

The notes come from `changelog/<v>/en.md`; the French ones (`fr.md`) are in a comment the page
does not show (`<!-- notes fr` … `-->`), and so are the minisign signatures
(`<!-- minisign <file name>` … `-->`): keep them when the notes are edited. A release without both
files of notes is not built.

Updates replace the files of the install folder with those of the portable archive (all but
`portable.flag`), for the installed and the portable layout alike, while Vixeeny runs (Windows lets a running program be
renamed); only `Vixeeny.exe` restarts. The previous files are kept in `.update-backup` until the
new `Vixeeny.exe` answers, and put back if it does not.
