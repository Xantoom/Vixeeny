// SPDX-License-Identifier: GPL-3.0-or-later
//! What the GitHub Releases API says about the latest release.
//!
//! A release holds the installer and the portable archive, which is also what an update installs. The archive's
//! signature is in the release notes, in a comment the page does not show (see [`signatures`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Where the latest release is asked for.
pub fn check_url(repository: &str) -> String {
    let path = repository.trim_end_matches('/').trim_start_matches("https://github.com/");
    format!("https://api.github.com/repos/{path}/releases/latest")
}

/// A file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A published release.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// `1.2.3`, without the `v` of the tag.
    pub version: String,
    pub assets: Vec<Asset>,
    /// The signatures in the notes, by the name of the file signed.
    #[serde(default)]
    pub signatures: BTreeMap<String, String>,
    /// What changed, as the release page shows it (Markdown, without the signatures).
    #[serde(default)]
    pub notes: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// The files that make one update, found by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFiles<'a> {
    pub archive: &'a Asset,
    /// The text of its `.minisig` file.
    pub signature: &'a str,
}

/// The comment that opens a signature in the notes: `<!-- minisign NAME`, then the text of the `.minisig` file, then
/// `-->`.
const SIGNATURE_OPENING: &str = "<!-- minisign ";

/// The signatures written in release notes, by the name of the file signed.
pub fn signatures(notes: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut rest = notes;
    while let Some(start) = rest.find(SIGNATURE_OPENING) {
        rest = &rest[start + SIGNATURE_OPENING.len()..];
        let Some((name, after)) = rest.split_once('\n') else {
            break;
        };
        let Some((text, after)) = after.split_once("-->") else {
            break;
        };
        found.insert(name.trim().to_owned(), text.trim().to_owned());
        rest = after;
    }
    found
}

/// The notes as the release page shows them: without the comments (the signatures).
pub fn shown_notes(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = rest[start..].split_once("-->").map_or("", |(_, after)| after);
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// What a line of the notes is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// A paragraph.
    Text,
    /// A section's title (`## Fixes`).
    Heading,
    /// A list item, `level` deep (0 at the margin).
    Item { level: u8 },
}

/// A line of the notes as it is read: without its marks, the emphasis and the links' targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteLine {
    pub kind: NoteKind,
    pub text: String,
}

/// Markdown notes as lines to read: headings, list items (their depth from their indent), paragraphs. Empty lines go.
pub fn note_lines(notes: &str) -> Vec<NoteLine> {
    notes
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            let indent = line.len() - line.trim_start().len();
            let text = line.trim_start();
            let (kind, text) = if let Some(item) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
                (NoteKind::Item { level: u8::try_from(indent / 2).unwrap_or(u8::MAX) }, item)
            } else if text.starts_with('#') {
                (NoteKind::Heading, text.trim_start_matches('#').trim_start())
            } else {
                (NoteKind::Text, text)
            };
            let text = inline_plain(text);
            (!text.is_empty()).then_some(NoteLine { kind, text })
        })
        .collect()
}

/// `text` without its emphasis, code marks and links' targets.
fn inline_plain(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '`' => {}
            // `[text](url)`: the text.
            ']' if chars.peek() == Some(&'(') => {
                for c in chars.by_ref() {
                    if c == ')' {
                        break;
                    }
                }
            }
            '[' => {}
            c => plain.push(c),
        }
    }
    plain.trim().to_owned()
}

/// Where the release of `version` is asked for.
pub fn tag_url(repository: &str, version: &str) -> String {
    let path = repository.trim_end_matches('/').trim_start_matches("https://github.com/");
    format!("https://api.github.com/repos/{path}/releases/tags/v{}", version.trim_start_matches(['v', 'V']))
}

/// The comment holding `signature` (the text of a `.minisig` file) of the file `name`.
pub fn signature_comment(name: &str, signature: &str) -> String {
    format!("{SIGNATURE_OPENING}{name}\n{}\n-->", signature.trim())
}

impl Release {
    /// Parses the answer of `releases/latest`. Drafts and pre-releases are not offered.
    pub fn parse(json: &str) -> Result<Option<Self>, serde_json::Error> {
        let api: ApiRelease = serde_json::from_str(json)?;
        if api.draft || api.prerelease {
            return Ok(None);
        }
        Ok(Some(Self {
            version: api.tag_name.trim_start_matches(['v', 'V']).to_owned(),
            signatures: api.body.as_deref().map(signatures).unwrap_or_default(),
            notes: api.body.as_deref().map(shown_notes).unwrap_or_default(),
            assets: api
                .assets
                .into_iter()
                .map(|a| Asset { name: a.name, url: a.browser_download_url, size: a.size })
                .collect(),
        }))
    }

    /// The archive of this version for `platform` (`windows-x64`) and its signature. The archive is found by its full
    /// name, version included: its signature names it too, so an archive of another version is refused.
    pub fn files(&self, platform: &str) -> Option<UpdateFiles<'_>> {
        let name = portable_name(&self.version, platform);
        Some(UpdateFiles {
            archive: self.assets.iter().find(|a| a.name == name)?,
            signature: self.signatures.get(&name)?,
        })
    }

    /// The installer of this version and its signature.
    pub fn setup(&self) -> Option<(&Asset, &str)> {
        let name = setup_name(&self.version);
        Some((self.assets.iter().find(|a| a.name == name)?, self.signatures.get(&name)?))
    }
}

/// The installer of `version`.
pub fn setup_name(version: &str) -> String {
    format!("Vixeeny-{version}-setup.exe")
}

/// The portable archive of `version`: the update archive and `portable.flag`.
pub fn portable_name(version: &str, platform: &str) -> String {
    format!("Vixeeny-{version}-{platform}-portable.zip")
}

/// Whether `latest` is a newer version than `current` (both `1.2.3`, a leading `v` allowed). An unreadable version is
/// never newer.
pub fn is_newer(current: &str, latest: &str) -> bool {
    let parse = |v: &str| semver::Version::parse(v.trim().trim_start_matches(['v', 'V'])).ok();
    match (parse(current), parse(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNATURE: &str = "untrusted comment: x\nRUQ=\ntrusted comment: y\nAAA=";

    fn json(version: &str, signed: &str) -> String {
        let notes = format!(
            "## What's new\n- Things\n\n{}\n",
            signature_comment(&format!("Vixeeny-{signed}-windows-x64-portable.zip"), SIGNATURE)
        );
        serde_json::json!({
            "tag_name": format!("v{version}"), "body": notes, "draft": false, "prerelease": false,
            "assets": [
                {"name": format!("Vixeeny-{version}-windows-x64-portable.zip"),
                 "browser_download_url": "https://x/p.zip", "size": 10},
                {"name": format!("Vixeeny-{version}-setup.exe"),
                 "browser_download_url": "https://x/s.exe", "size": 5}
            ]
        })
        .to_string()
    }

    #[test]
    fn the_portable_archive_is_found_with_its_signature_from_the_notes() {
        let release = Release::parse(&json("0.6.0", "0.6.0")).unwrap_or_default().expect("none");
        assert_eq!(release.version, "0.6.0");
        let files = release.files("windows-x64").expect("no files");
        assert_eq!(files.archive.url, "https://x/p.zip");
        assert_eq!(files.signature, SIGNATURE);
        assert!(release.files("windows-arm64").is_none());
        // A signature for another version signs nothing here.
        let release = Release::parse(&json("0.6.0", "0.5.0")).ok().flatten();
        assert!(release.is_some_and(|r| r.files("windows-x64").is_none()));
    }

    #[test]
    fn the_installer_is_found_only_when_signed() {
        let release = Release::parse(&json("0.6.0", "0.6.0")).ok().flatten();
        // Only the archive is signed in these notes.
        assert!(release.as_ref().is_some_and(|r| r.setup().is_none()));
        let mut release = release.expect("none");
        release.signatures.insert(setup_name("0.6.0"), SIGNATURE.to_owned());
        let (setup, signature) = release.setup().expect("no installer");
        assert_eq!(setup.url, "https://x/s.exe");
        assert_eq!(signature, SIGNATURE);
    }

    #[test]
    fn drafts_and_prereleases_are_not_offered() {
        let json = json("0.6.0", "0.6.0");
        let draft = json.replace("\"draft\":false", "\"draft\":true");
        assert_eq!(Release::parse(&draft).ok(), Some(None));
        let pre = json.replace("\"prerelease\":false", "\"prerelease\":true");
        assert_eq!(Release::parse(&pre).ok(), Some(None));
        assert!(Release::parse("nonsense").is_err());
    }

    #[test]
    fn the_notes_are_kept_without_the_signatures_and_read_as_plain_text() {
        let release = Release::parse(&json("0.6.0", "0.6.0")).ok().flatten().expect("none");
        assert_eq!(release.notes, "## What's new\n- Things");
        let notes = "## Fixes\n\n\n- A **window** capture, see [the page](https://x).\n  - `Nested`\n";
        let line = |kind, text: &str| NoteLine { kind, text: text.into() };
        assert_eq!(
            note_lines(notes),
            [
                line(NoteKind::Heading, "Fixes"),
                line(NoteKind::Item { level: 0 }, "A window capture, see the page."),
                line(NoteKind::Item { level: 1 }, "Nested"),
            ]
        );
        assert_eq!(shown_notes("a<!-- x -->b<!-- open"), "ab");
        assert!(tag_url("https://github.com/Xantoom/Vixeeny", "1.2.0").ends_with("/releases/tags/v1.2.0"));
    }

    #[test]
    fn versions_compare_as_semver() {
        assert!(is_newer("0.5.0", "v0.5.1"));
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("1.0.0", "garbage"));
        assert!(is_newer("1.0.0-beta.1", "1.0.0"));
    }
}
