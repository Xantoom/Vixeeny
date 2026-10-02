// SPDX-License-Identifier: GPL-3.0-or-later
//! Source names as the profile writes them: `system`, `mic`, `mic:<device id or name>`,
//! `app:<executable name>`.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// Everything the speakers play.
    System,
    /// A capture device; `None` = the default one.
    Microphone(Option<String>),
    /// One application (and its child processes), by executable name, e.g. `spotify.exe`.
    Application(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceSpec {
    /// The text of the profile, also the key of the volume setting.
    pub name: String,
    pub kind: SourceKind,
}

impl SourceSpec {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let kind = match text.split_once(':') {
            None => match text {
                "system" => SourceKind::System,
                "mic" => SourceKind::Microphone(None),
                _ => return None,
            },
            Some(("mic", id)) if !id.trim().is_empty() => {
                SourceKind::Microphone(Some(id.trim().to_owned()))
            }
            Some(("app", exe)) if !exe.trim().is_empty() => {
                SourceKind::Application(exe.trim().to_owned())
            }
            _ => return None,
        };
        Some(Self {
            name: text.to_owned(),
            kind,
        })
    }

    /// The name a track carries when it holds this source alone (plan 5.10).
    pub fn title(&self) -> String {
        match &self.kind {
            SourceKind::System => "System".into(),
            SourceKind::Microphone(_) => "Micro".into(),
            SourceKind::Application(exe) => exe
                .strip_suffix(".exe")
                .or_else(|| exe.strip_suffix(".EXE"))
                .unwrap_or(exe)
                .to_owned(),
        }
    }
}

/// Parses a list, returning what could not be understood separately.
pub fn parse_sources(texts: &[String]) -> (Vec<SourceSpec>, Vec<String>) {
    let mut good = Vec::new();
    let mut bad = Vec::new();
    for t in texts {
        match SourceSpec::parse(t) {
            Some(s) if !good.contains(&s) => good.push(s),
            Some(_) => {}
            None => bad.push(t.clone()),
        }
    }
    (good, bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_forms() {
        assert_eq!(SourceSpec::parse("system").unwrap().kind, SourceKind::System);
        assert_eq!(
            SourceSpec::parse("mic").unwrap().kind,
            SourceKind::Microphone(None)
        );
        assert_eq!(
            SourceSpec::parse("mic:Blue Yeti").unwrap().kind,
            SourceKind::Microphone(Some("Blue Yeti".into()))
        );
        assert_eq!(
            SourceSpec::parse(" app:Spotify.exe ").unwrap().kind,
            SourceKind::Application("Spotify.exe".into())
        );
        for bad in ["", "app:", "mic:", "speakers", "app", "sys"] {
            assert!(SourceSpec::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn titles_name_the_source() {
        assert_eq!(SourceSpec::parse("system").unwrap().title(), "System");
        assert_eq!(SourceSpec::parse("mic:X").unwrap().title(), "Micro");
        assert_eq!(SourceSpec::parse("app:Spotify.exe").unwrap().title(), "Spotify");
    }

    #[test]
    fn duplicates_and_garbage_are_reported() {
        let list: Vec<String> = ["system", "mic", "system", "nope"].map(String::from).to_vec();
        let (good, bad) = parse_sources(&list);
        assert_eq!(good.len(), 2);
        assert_eq!(bad, ["nope"]);
    }
}
