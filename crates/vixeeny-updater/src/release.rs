// SPDX-License-Identifier: GPL-3.0-or-later
//! What the GitHub Releases API says about the latest release.

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
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
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
    pub signature: &'a Asset,
    /// Not needed: the signature already covers the archive.
    pub sums: Option<&'a Asset>,
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
    /// checksum list. The archive is found by its full name, version included: its signature
    /// names it too, so an archive of another version is refused.
    pub fn files(&self, platform: &str) -> Option<UpdateFiles<'_>> {
        let find = |name: &str| self.assets.iter().find(|a| a.name == name);
        let archive = find(&archive_name(&self.version, platform))?;
        Some(UpdateFiles {
            archive,
            signature: find(&format!("{}.minisig", archive.name))?,
            sums: find("SHA256SUMS"),
        })
    }
}

/// The update archive of `version` (`xtask dist` writes it).
pub fn archive_name(version: &str, platform: &str) -> String {
    format!("Vixeeny-{version}-{platform}.zip")
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
        assert_eq!(files.signature.url, "https://x/a.sig");
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
