# Code signing policy

> [!NOTE]
> Signing is being set up: the releases are not signed with this certificate yet.

Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by
[SignPath Foundation](https://signpath.org/).

## What is signed

Every release is built from this repository by the
[release workflow](../.github/workflows/release.yml) on GitHub-hosted runners, then signed after
a manual approval. Signed: Vixeeny's own programs (`Vixeeny.exe`, `vixeeny-app.exe`) and the
installer. The FFmpeg libraries shipped beside them (`av*.dll`, `sw*.dll`, listed in
`THIRD-PARTY-LICENSES.txt`) are not signed with this certificate.

Updates are also verified with [minisign](../packaging/minisign.pub) before anything is
replaced.

## Team roles

| Role | Members |
| --- | --- |
| Authors (commit without further review) | [Xantoom](https://github.com/Xantoom) |
| Reviewers (review every outside contribution) | [Xantoom](https://github.com/Xantoom) |
| Approvers (approve each signing request) | [Xantoom](https://github.com/Xantoom) |

## Privacy

Vixeeny does not transfer any information to other networked systems, except to check for and
download its updates from github.com (the GitHub API and the release files of this repository).
That check can be turned off in the installer and in Settings → Updates. Captures, recordings,
settings and logs stay on your PC. "Report a problem" and the links of the About page open
github.com in your browser only when you click them.

The third-party libraries bundled with Vixeeny do not connect to the network.
