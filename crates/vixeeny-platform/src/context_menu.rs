// SPDX-License-Identifier: GPL-3.0-or-later
//! The "Convert with Vixeeny" entry of the file manager's context menu (plan 5.5). On Windows it
//! is a per-user registry verb on every supported image extension (no administrator rights).

use std::path::Path;

/// Extensions that get the entry (the readable formats of `vixeeny-image`).
pub const EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "webp", "avif", "jxl", "bmp", "tif", "tiff", "gif",
];

/// Name of the verb key.
pub const VERB: &str = "VixeenyConvert";

/// One registry key under `HKEY_CURRENT_USER` and its string values (`""` is the default value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub values: Vec<(&'static str, String)>,
}

/// Every key to create. `label` is the menu text; `exe` the `vixeeny-app` executable.
pub fn entries(exe: &Path, label: &str) -> Vec<Entry> {
    let exe = exe.display();
    let mut out = Vec::new();
    for ext in EXTENSIONS {
        let verb = format!("Software\\Classes\\SystemFileAssociations\\.{ext}\\shell\\{VERB}");
        out.push(Entry {
            key: verb.clone(),
            values: vec![
                ("", label.to_owned()),
                ("Icon", format!("\"{exe}\",0")),
                // All selected files in one command line.
                ("MultiSelectModel", "Player".to_owned()),
            ],
        });
        out.push(Entry {
            key: format!("{verb}\\command"),
            values: vec![("", format!("\"{exe}\" --convert \"%1\""))],
        });
    }
    out
}

/// Keys to delete (their sub-keys go with them).
pub fn verb_keys() -> Vec<String> {
    EXTENSIONS
        .iter()
        .map(|ext| format!("Software\\Classes\\SystemFileAssociations\\.{ext}\\shell\\{VERB}"))
        .collect()
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW,
    };
    use windows::core::PCWSTR;

    use super::{Entry, verb_keys};
    use crate::PlatformError;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn os(what: &str, code: impl std::fmt::Display) -> PlatformError {
        PlatformError::Os(format!("registry {what}: {code}"))
    }

    pub fn write(entries: &[Entry]) -> crate::Result<()> {
        for entry in entries {
            let key = wide(&entry.key);
            let mut handle = HKEY::default();
            // SAFETY: `key` is NUL-terminated and outlives the call; `handle` receives the key.
            let status = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(key.as_ptr()),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_WRITE,
                    None,
                    &mut handle,
                    None,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(os("create", status.0));
            }
            let mut result = Ok(());
            for (name, value) in &entry.values {
                let name_w = wide(name);
                let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
                let name_ptr = if name.is_empty() {
                    PCWSTR::null()
                } else {
                    PCWSTR(name_w.as_ptr())
                };
                // SAFETY: the key is open for writing; `data` is a NUL-terminated UTF-16 string.
                let status = unsafe { RegSetValueExW(handle, name_ptr, None, REG_SZ, Some(&data)) };
                if status != ERROR_SUCCESS {
                    result = Err(os("set", status.0));
                    break;
                }
            }
            // SAFETY: the key was opened above and is closed once.
            let _ = unsafe { RegCloseKey(handle) };
            result?;
        }
        Ok(())
    }

    pub fn remove() -> crate::Result<()> {
        for key in verb_keys() {
            let key = wide(&key);
            // SAFETY: `key` is NUL-terminated and outlives the call.
            let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                return Err(os("delete", status.0));
            }
        }
        Ok(())
    }

    pub fn installed() -> bool {
        let Some(first) = verb_keys().into_iter().next() else {
            return false;
        };
        let key = wide(&first);
        let mut handle = HKEY::default();
        // SAFETY: `key` is NUL-terminated and outlives the call; `handle` receives the key.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                None,
                KEY_READ,
                &mut handle,
            )
        };
        if status == ERROR_SUCCESS {
            // SAFETY: the key was opened above and is closed once.
            let _ = unsafe { RegCloseKey(handle) };
            true
        } else {
            false
        }
    }
}

/// Adds the entry for the current user.
#[cfg(windows)]
pub fn install(exe: &Path, label: &str) -> crate::Result<()> {
    imp::write(&entries(exe, label))
}

/// Writes arbitrary per-user registry entries (used by the native notifications).
#[cfg(windows)]
pub(crate) fn write_entries(entries: &[Entry]) -> crate::Result<()> {
    imp::write(entries)
}

/// Removes the entry (a no-op when it is absent).
#[cfg(windows)]
pub fn uninstall() -> crate::Result<()> {
    imp::remove()
}

#[cfg(windows)]
pub fn is_installed() -> bool {
    imp::installed()
}

/// The Finder Quick Action (a `.workflow` bundle in `~/Library/Services`): macOS has no registry
/// verbs, a Services workflow that runs `vixeeny-app --convert` on the selected files is the
/// equivalent.
pub mod quick_action {
    use std::path::{Path, PathBuf};

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// Single-quotes `s` for `sh`.
    fn sh_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    /// `Contents/Info.plist`: the service accepts image files in Finder.
    pub fn info_plist(label: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>{label}</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.image</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#,
            label = xml_escape(label)
        )
    }

    /// `Contents/document.wflow`: one "Run Shell Script" action receiving the files as arguments.
    pub fn workflow(exe: &Path) -> String {
        let script = format!("{} --convert \"$@\"", sh_quote(&exe.display().to_string()));
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>523</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{script}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/sh</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>B6A1C1D2-3E4F-4A5B-8C6D-7E8F9A0B1C2D</string>
				<key>OutputUUID</key>
				<string>C7B2D2E3-4F50-4B6C-9D7E-8F9A0B1C2D3E</string>
				<key>UUID</key>
				<string>D8C3E3F4-5061-4C7D-8E8F-9A0B1C2D3E4F</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
			</dict>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject.image</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<integer>0</integer>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#,
            script = xml_escape(&script)
        )
    }

    /// `~/Library/Services/<label>.workflow`.
    pub fn bundle_path(label: &str) -> Option<PathBuf> {
        let name: String = label.chars().filter(|c| !matches!(c, '/' | ':')).collect();
        std::env::home_dir().map(|h| h.join("Library/Services").join(format!("{name}.workflow")))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_script_quotes_the_executable_and_forwards_the_files() {
            let exe = Path::new("/Apps/My Apps/Vixeeny.app/Contents/MacOS/vixeeny-app");
            assert!(workflow(exe).contains(
                "'/Apps/My Apps/Vixeeny.app/Contents/MacOS/vixeeny-app' --convert \"$@\""
            ));
        }

        #[test]
        fn the_label_is_escaped() {
            assert!(info_plist("A & B").contains("A &amp; B"));
        }
    }
}

/// Installs the Finder Quick Action for the current user.
#[cfg(target_os = "macos")]
pub fn install(exe: &Path, label: &str) -> crate::Result<()> {
    let io = |e: std::io::Error| crate::PlatformError::Os(format!("quick action: {e}"));
    let bundle = quick_action::bundle_path(label)
        .ok_or_else(|| crate::PlatformError::Os("no home folder".into()))?;
    // A previous install may carry another label: drop it first.
    remove_quick_actions();
    let contents = bundle.join("Contents");
    std::fs::create_dir_all(&contents).map_err(io)?;
    std::fs::write(contents.join("Info.plist"), quick_action::info_plist(label)).map_err(io)?;
    std::fs::write(contents.join("document.wflow"), quick_action::workflow(exe)).map_err(io)?;
    // Make Finder re-read its services.
    let _ = std::process::Command::new("/System/Library/CoreServices/pbs")
        .arg("-update")
        .status();
    Ok(())
}

/// Finder Quick Actions written by [`install`] (they all run `--convert`).
#[cfg(target_os = "macos")]
fn quick_actions() -> Vec<std::path::PathBuf> {
    let Some(dir) = std::env::home_dir().map(|h| h.join("Library/Services")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "workflow"))
        .filter(|p| {
            std::fs::read_to_string(p.join("Contents/document.wflow"))
                .is_ok_and(|w| w.contains("vixeeny") && w.contains("--convert"))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn remove_quick_actions() {
    for path in quick_actions() {
        let _ = std::fs::remove_dir_all(path);
    }
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> crate::Result<()> {
    remove_quick_actions();
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn is_installed() -> bool {
    !quick_actions().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_extension_gets_a_verb_and_a_quoted_command() {
        let e = entries(
            Path::new("C:\\Program Files\\Vixeeny\\vixeeny-app.exe"),
            "Convert",
        );
        assert_eq!(e.len(), EXTENSIONS.len() * 2);
        let command = e
            .iter()
            .find(|e| e.key.ends_with(".png\\shell\\VixeenyConvert\\command"))
            .unwrap();
        assert_eq!(
            command.values[0].1,
            "\"C:\\Program Files\\Vixeeny\\vixeeny-app.exe\" --convert \"%1\""
        );
        assert!(e.iter().all(|e| e.key.starts_with("Software\\Classes\\")));
        assert_eq!(verb_keys().len(), EXTENSIONS.len());
    }
}
