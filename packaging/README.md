# Packaging

- `windows/vixeeny.iss` — Inno Setup script (per-user installer, no administrator rights).
- `minisign.pub` — the public key updates are verified with, embedded in `vixeeny-updater`.
  While it contains the word `unconfigured`, the updater refuses every update.
- `linux/` — desktop entry, icon and AppStream data (shared by the .deb, the AppImage and the update archive); `arch/PKGBUILD` — AUR `vixeeny-bin`.
- `.github/workflows/release.yml` — builds, packages, signs and drafts a release on a `v*` tag
  (or builds without publishing on a manual run).

## One-time setup by the maintainer (signing key)

```
cargo install rsign2
rsign generate -p packaging/minisign.pub -s vixeeny.key
```

Commit `packaging/minisign.pub`; keep `vixeeny.key` secret. In the repository settings add the
secrets `MINISIGN_KEY` (the content of `vixeeny.key`) and `MINISIGN_PASSWORD`.

## macOS signature and notarization (optional)

Without the secrets below the macOS build is signed ad hoc (right-click → Open the first time).
With a paid Apple Developer account, add these repository secrets and the release job signs with
the hardened runtime and notarizes the disk image:

| Secret | Content |
|---|---|
| `APPLE_CERTIFICATE` | the "Developer ID Application" certificate exported as `.p12`, base64-encoded |
| `APPLE_CERTIFICATE_PASSWORD` | the password chosen at export |
| `APPLE_ID` | the Apple ID e-mail |
| `APPLE_APP_PASSWORD` | an app-specific password (appleid.apple.com) |
| `APPLE_TEAM_ID` | the 10-character team id |

Locally: `VIXEENY_SIGN_IDENTITY="Developer ID Application: …" cargo xtask dist-mac`.

## Files of a release

| File | Use |
|---|---|
| `Vixeeny-<v>-setup.exe` | Installer |
| `Vixeeny-<v>-windows-x64.zip` | Update archive (what the updater downloads) |
| `Vixeeny-<v>-windows-x64-portable.zip` | Portable: same files plus `portable.flag` (settings in `data\`) |
| `*.minisig` | minisign signature of each archive and of the installer |
| `SHA256SUMS` | SHA-256 of every file above |

Updates replace the files of the install folder with those of the update archive, for the
installed and the portable layout alike; the previous files are kept in `.update-backup` until
the new daemon answers, and put back if it does not.
