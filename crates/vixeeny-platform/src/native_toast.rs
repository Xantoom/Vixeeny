// SPDX-License-Identifier: GPL-3.0-or-later
//! Native Windows notifications (toasts) that react to a click without any process running:
//! the click and the buttons are *protocol activations*, i.e. a URL the shell opens. A `file:`
//! URL opens the file (or the folder) with the default program; `vixeeny:` URLs are handled by
//! `vixeeny-app --uri` (registered for the current user the first time a toast is shown).

use std::path::Path;

use crate::context_menu::Entry;

/// The identity the toasts are shown under (name in the Action Center).
pub const APP_ID: &str = "Vixeeny";

/// The URL scheme the app registers.
pub const SCHEME: &str = "vixeeny";

/// What to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToast {
    pub title: String,
    pub body: String,
    /// Picture shown beside the text.
    pub image: Option<std::path::PathBuf>,
    /// Opened when the toast itself is clicked.
    pub launch: String,
    /// Buttons: label and the URL they open.
    pub actions: Vec<(String, String)>,
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// `C:\a b\c.png` → `file:///C:/a%20b/c.png`.
pub fn file_uri(path: &Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    let mut out = String::from("file:///");
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(char::from(b));
            }
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl NativeToast {
    /// The XML the notification system parses.
    pub fn xml(&self) -> String {
        let mut xml = format!(
            "<toast activationType=\"protocol\" launch=\"{}\"><visual><binding template=\"ToastGeneric\">",
            escape(&self.launch)
        );
        if let Some(image) = &self.image {
            xml.push_str(&format!(
                "<image placement=\"appLogoOverride\" src=\"{}\"/>",
                escape(&file_uri(image))
            ));
        }
        xml.push_str(&format!(
            "<text>{}</text><text>{}</text></binding></visual>",
            escape(&self.title),
            escape(&self.body)
        ));
        if !self.actions.is_empty() {
            xml.push_str("<actions>");
            for (label, url) in &self.actions {
                xml.push_str(&format!(
                    "<action content=\"{}\" arguments=\"{}\" activationType=\"protocol\"/>",
                    escape(label),
                    escape(url)
                ));
            }
            xml.push_str("</actions>");
        }
        xml.push_str("</toast>");
        xml
    }
}

/// The registry keys that make the toasts work: the name they appear under, and the handler of
/// `vixeeny:` URLs (`app`: the `vixeeny-app` executable).
pub fn registration(app: &Path) -> Vec<Entry> {
    let app = app.display();
    vec![
        Entry {
            key: format!("Software\\Classes\\AppUserModelId\\{APP_ID}"),
            values: vec![("DisplayName", "Vixeeny".to_owned())],
        },
        Entry {
            key: format!("Software\\Classes\\{SCHEME}"),
            values: vec![
                ("", "URL:Vixeeny".to_owned()),
                ("URL Protocol", String::new()),
            ],
        },
        Entry {
            key: format!("Software\\Classes\\{SCHEME}\\shell\\open\\command"),
            values: vec![("", format!("\"{app}\" --uri \"%1\""))],
        },
    ]
}

/// Registers (idempotent) and shows `toast`. `app`: the `vixeeny-app` executable.
#[cfg(windows)]
pub fn show(toast: &NativeToast, app: &Path) -> crate::Result<()> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
    use windows::core::HSTRING;

    let fail = |e: windows::core::Error| crate::PlatformError::Os(format!("toast: {e}"));
    crate::context_menu::write_entries(&registration(app))?;
    // SAFETY: plain COM initialisation; "already initialised" is not an error for us.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let doc = XmlDocument::new().map_err(fail)?;
    doc.LoadXml(&HSTRING::from(toast.xml())).map_err(fail)?;
    let notification = ToastNotification::CreateToastNotification(&doc).map_err(fail)?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
        .map_err(fail)?;
    notifier.Show(&notification).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn paths_become_file_urls() {
        assert_eq!(
            file_uri(Path::new("C:\\Users\\Zoé\\a b.png")),
            "file:///C:/Users/Zo%C3%A9/a%20b.png"
        );
    }

    #[test]
    fn the_xml_escapes_and_carries_the_buttons() {
        let toast = NativeToast {
            title: "Saved <1>".into(),
            body: "a&b \"c\"".into(),
            image: Some(PathBuf::from("C:\\x\\y.png")),
            launch: "file:///C:/x/y.png".into(),
            actions: vec![("Open folder".into(), "file:///C:/x/".into())],
        };
        let xml = toast.xml();
        assert!(
            xml.starts_with("<toast activationType=\"protocol\" launch=\"file:///C:/x/y.png\">")
        );
        assert!(xml.contains("<text>Saved &lt;1&gt;</text><text>a&amp;b &quot;c&quot;</text>"));
        assert!(xml.contains("src=\"file:///C:/x/y.png\""));
        assert!(xml.contains("<action content=\"Open folder\" arguments=\"file:///C:/x/\" activationType=\"protocol\"/>"));
        let plain = NativeToast {
            image: None,
            actions: vec![],
            ..toast
        };
        assert!(!plain.xml().contains("<image") && !plain.xml().contains("<actions>"));
    }

    #[test]
    fn the_registration_points_the_scheme_at_the_app() {
        let keys = registration(Path::new("C:\\V\\vixeeny-app.exe"));
        let command = keys
            .iter()
            .find(|e| e.key.ends_with("open\\command"))
            .unwrap();
        assert_eq!(
            command.values[0].1,
            "\"C:\\V\\vixeeny-app.exe\" --uri \"%1\""
        );
        assert!(
            keys.iter()
                .all(|e| e.key.starts_with("Software\\Classes\\"))
        );
    }
}
