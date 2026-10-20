// SPDX-License-Identifier: GPL-3.0-or-later
//! What the GitHub Releases API says about the latest release.
//!
//! A release holds the installer and the portable archive, which is also what an update
//! installs. The archive's signature is in the release notes, in a comment the page does not
//! show (see [`signatures`]). The releases before 0.20.2 held a separate update archive and
//! signature files: they are still read.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Where the latest release is asked for.
pub fn check_url(repository: &str) -> String {
    let path = repository
        .trim_end_matches('/')
        .trim_start_matches("https://github.com/");
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// `1.2.3`, without the `v` of the tag.
    pub version: String,
    pub assets: Vec<Asset>,
    /// The signatures in the notes, by the name of the file signed.
    #[serde(default)]
    pub signatures: BTreeMap<String, String>,
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

/// Where the archive's signature is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signature<'a> {
    /// In the notes of the release.
    Text(&'a str),
    /// A file of the release (before 0.20.2).
    Asset(&'a Asset),
}

/// The files that make one update, found by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFiles<'a> {
    pub archive: &'a Asset,
    pub signature: Signature<'a>,
    /// Not needed: the signature already covers the archive.
    pub sums: Option<&'a Asset>,
}

/// The comment that opens a signature in the notes: `<!-- minisign NAME`, then the text of the
/// `.minisig` file, then `-->`.
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
            assets: api
                .assets
                .into_iter()
                .map(|a| Asset {
                    name: a.name,
                    url: a.browser_download_url,
                    size: a.size,
                })
                .collect(),
        }))
    }

    /// The archive of this version for `platform` (`windows-x64`), its signature and the
    /// checksum list: the portable archive signed in the notes, else (before 0.20.2) the update
    /// archive and its signature file. The archive is found by its full name, version
    /// included: its signature names it too, so an archive of another version is refused.
    pub fn files(&self, platform: &str) -> Option<UpdateFiles<'_>> {
        let find = |name: &str| self.assets.iter().find(|a| a.name == name);
        let sums = find("SHA256SUMS");
        let portable = portable_name(&self.version, platform);
        if let (Some(archive), Some(signature)) = (find(&portable), self.signatures.get(&portable))
        {
            return Some(UpdateFiles {
                archive,
                signature: Signature::Text(signature),
                sums,
            });
        }
        let archive = find(&archive_name(&self.version, platform))?;
        Some(UpdateFiles {
            archive,
            signature: Signature::Asset(find(&format!("{}.minisig", archive.name))?),
            sums,
        })
    }
}

/// The update archive of `version` (`xtask dist` writes it; published until 0.20.2).
pub fn archive_name(version: &str, platform: &str) -> String {
    format!("Vixeeny-{version}-{platform}.zip")
}

/// The portable archive of `version`: the update archive and `portable.flag`.
pub fn portable_name(version: &str, platform: &str) -> String {
    format!("Vixeeny-{version}-{platform}-portable.zip")
}

/// Whether `latest` is a newer version than `current` (both `1.2.3`, a leading `v` allowed).
/// An unreadable version is never newer.
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

    const JSON: &str = r#"{
        "tag_name": "v0.5.1", "body": "Fixes", "draft": false, "prerelease": false,
        "assets": [
            {"name": "Vixeeny-0.5.1-windows-x64.zip", "browser_download_url": "https://x/a.zip", "size": 10},
            {"name": "Vixeeny-0.5.1-windows-x64.zip.minisig", "browser_download_url": "https://x/a.sig", "size": 1},
            {"name": "SHA256SUMS", "browser_download_url": "https://x/sums", "size": 1},
            {"name": "Vixeeny-0.5.1-setup.exe", "browser_download_url": "https://x/s.exe", "size": 5}
        ]
    }"#;

    #[test]
    fn the_latest_release_is_read_and_its_files_found() {
        let release = Release::parse(JSON)
            .unwrap_or_default()
            .unwrap_or_else(|| panic!("none"));
        assert_eq!(release.version, "0.5.1");
        let files = release
            .files("windows-x64")
            .unwrap_or_else(|| panic!("no files"));
        assert_eq!(files.archive.url, "https://x/a.zip");
        assert!(matches!(files.signature, Signature::Asset(a) if a.url == "https://x/a.sig"));
        assert_eq!(files.sums.map(|s| s.url.as_str()), Some("https://x/sums"));
        assert!(release.files("windows-arm64").is_none());
        let without_sums = JSON.replace("\"SHA256SUMS\"", "\"other\"");
        let release = Release::parse(&without_sums).ok().flatten();
        assert!(release.is_some_and(|r| r.files("windows-x64").is_some_and(|f| f.sums.is_none())));
        // An archive named for another version is not this release's.
        let renamed = JSON.replace("\"v0.5.1\"", "\"v0.6.0\"");
        let release = Release::parse(&renamed).ok().flatten();
        assert!(release.is_some_and(|r| r.files("windows-x64").is_none()));
    }

    /// From 0.20.2: the installer and the portable archive, signed in the notes.
    #[test]
    fn the_portable_archive_is_signed_in_the_notes() {
        let signature = "untrusted comment: x\nRUQ=\ntrusted comment: y\nAAA=";
        let notes = format!(
            "## What's new\n- Things\n\n{}\n",
            signature_comment("Vixeeny-0.6.0-windows-x64-portable.zip", signature)
        );
        let json = serde_json::json!({
            "tag_name": "v0.6.0", "body": notes, "draft": false, "prerelease": false,
            "assets": [
                {"name": "Vixeeny-0.6.0-windows-x64-portable.zip",
                 "browser_download_url": "https://x/p.zip", "size": 10},
                {"name": "Vixeeny-0.6.0-setup.exe",
                 "browser_download_url": "https://x/s.exe", "size": 5}
            ]
        })
        .to_string();
        let release = Release::parse(&json)
            .unwrap_or_default()
            .unwrap_or_else(|| panic!("none"));
        let files = release
            .files("windows-x64")
            .unwrap_or_else(|| panic!("no files"));
        assert_eq!(files.archive.url, "https://x/p.zip");
        assert_eq!(files.signature, Signature::Text(signature));
        assert!(files.sums.is_none());
        // A signature for another file signs nothing here.
        let other = json.replace("minisign Vixeeny-0.6.0", "minisign Vixeeny-0.5.0");
        let release = Release::parse(&other).ok().flatten();
        assert!(release.is_some_and(|r| r.files("windows-x64").is_none()));
    }

    #[test]
    fn drafts_and_prereleases_are_not_offered() {
        let draft = JSON.replace("\"draft\": false", "\"draft\": true");
        assert_eq!(Release::parse(&draft).ok(), Some(None));
        let pre = JSON.replace("\"prerelease\": false", "\"prerelease\": true");
        assert_eq!(Release::parse(&pre).ok(), Some(None));
        assert!(Release::parse("nonsense").is_err());
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
