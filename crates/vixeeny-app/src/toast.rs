// SPDX-License-Identifier: GPL-3.0-or-later
//! Notifications (plan 5.14): a card with the thumbnail after a capture, a recording or a replay,
//! and a clear message with a way out when something fails. Each card is a process of its own
//! (`vixeeny-app --toast <kind> <text>`), so it never holds up the next hotkey.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use vixeeny_common::config::Config;
use vixeeny_common::i18n::Key;

/// What the card announces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toast {
    /// A file was written.
    Saved(Saved, PathBuf),
    /// Something failed; the message is already user-facing.
    Failed(Failed, String),
    /// A new version can be installed (sent by the daemon); a click opens the settings.
    Update(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    Image,
    Recording,
    Replay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failed {
    Capture,
    Recording,
    Replay,
    /// An audio source went away during a recording.
    Audio,
}

impl Saved {
    const fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Recording => "recording",
            Self::Replay => "replay",
        }
    }

    const fn title(self) -> Key {
        match self {
            Self::Image => Key::ToastImageSaved,
            Self::Recording => Key::ToastRecordingSaved,
            Self::Replay => Key::ToastReplaySaved,
        }
    }
}

impl Failed {
    const fn name(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Recording => "recording",
            Self::Replay => "replay",
            Self::Audio => "audio",
        }
    }

    const fn title(self) -> Key {
        match self {
            Self::Capture => Key::ToastCaptureFailed,
            Self::Recording => Key::ToastRecordingFailed,
            Self::Replay => Key::ToastReplayFailed,
            Self::Audio => Key::ToastAudioLost,
        }
    }
}

impl Toast {
    /// The arguments after `--toast`.
    pub fn to_args(&self) -> [String; 2] {
        match self {
            Self::Saved(kind, path) => {
                [format!("saved-{}", kind.name()), path.display().to_string()]
            }
            Self::Failed(kind, message) => [format!("failed-{}", kind.name()), message.clone()],
            Self::Update(text) => ["update".to_owned(), text.clone()],
        }
    }

    pub fn from_args(kind: &str, text: &str) -> Option<Self> {
        let saved = |k| Some(Self::Saved(k, PathBuf::from(text)));
        let failed = |k| Some(Self::Failed(k, text.to_owned()));
        match kind {
            "saved-image" => saved(Saved::Image),
            "saved-recording" => saved(Saved::Recording),
            "saved-replay" => saved(Saved::Replay),
            "failed-capture" => failed(Failed::Capture),
            "failed-recording" => failed(Failed::Recording),
            "failed-replay" => failed(Failed::Replay),
            "failed-audio" => failed(Failed::Audio),
            "update" => Some(Self::Update(text.to_owned())),
            _ => None,
        }
    }
}

/// The advice for the failures the user can fix: a full disk, a folder without write access.
pub fn hint(error: &anyhow::Error) -> Option<Key> {
    error
        .chain()
        .filter_map(|e| e.downcast_ref::<std::io::Error>())
        .find_map(|e| match (e.raw_os_error(), e.kind()) {
            // ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL, ENOSPC.
            (Some(112 | 39 | 28), _) | (_, std::io::ErrorKind::StorageFull) => {
                Some(Key::ToastDiskFull)
            }
            (_, std::io::ErrorKind::PermissionDenied) => Some(Key::ToastAccessDenied),
            _ => None,
        })
}

/// The text a failure card shows: the advice when there is one, else the error itself.
pub fn failure_text(error: &anyhow::Error, lang: vixeeny_common::i18n::Lang) -> String {
    hint(error).map_or_else(
        || format!("{error:#}"),
        |key| vixeeny_common::i18n::tr(key, lang).to_owned(),
    )
}

/// Shows the card, unless the user turned notifications off. Never blocks.
pub fn notify(config: &Config, toast: &Toast) {
    if !config.general.notifications {
        return;
    }
    #[cfg(windows)]
    {
        let [kind, text] = toast.to_args();
        let spawned = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .args(["--toast", &kind, &text])
                .spawn()
        });
        if let Err(e) = spawned {
            tracing::warn!("cannot show the notification: {e}");
        }
    }
    #[cfg(target_os = "macos")]
    notify_mac(config, toast);
    #[cfg(target_os = "linux")]
    notify_linux(config, toast);
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = config;
        tracing::info!("notification: {toast:?}");
    }
}

/// Linux: a banner through the notification portal (sandbox) or `notify-send`.
/// Clicking a banner is not handled: that needs D-Bus actions, a later refinement.
#[cfg(target_os = "linux")]
fn notify_linux(config: &Config, toast: &Toast) {
    use vixeeny_common::i18n::tr;

    let lang = crate::lang(&config.general.language);
    let (title, body) = match toast {
        Toast::Saved(kind, path) => (
            tr(kind.title(), lang).to_owned(),
            path.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        ),
        Toast::Failed(kind, message) => (tr(kind.title(), lang).to_owned(), message.clone()),
        Toast::Update(text) => ("Vixeeny".to_owned(), text.clone()),
    };
    if let Err(e) = vixeeny_platform::notify::notify(&title, &body) {
        tracing::warn!("cannot show the notification: {e}");
    }
}

/// macOS: a notification-centre banner through `osascript` (no click action: that needs the app
/// to be a bundle with a notification delegate, see the packaging step).
#[cfg(target_os = "macos")]
fn notify_mac(config: &Config, toast: &Toast) {
    use vixeeny_common::i18n::tr;

    let lang = crate::lang(&config.general.language);
    let (title, body) = match toast {
        Toast::Saved(kind, path) => (
            tr(kind.title(), lang).to_owned(),
            path.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        ),
        Toast::Failed(kind, message) => (tr(kind.title(), lang).to_owned(), message.clone()),
        Toast::Update(text) => ("Vixeeny".to_owned(), text.clone()),
    };
    // AppleScript string literals: backslash and quote escaped.
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let script = format!(
        "display notification {} with title \"Vixeeny\" subtitle {}",
        quote(&body),
        quote(&title)
    );
    if let Err(e) = std::process::Command::new("osascript")
        .args(["-e", &script])
        .spawn()
    {
        tracing::warn!("cannot show the notification: {e}");
    }
}

/// The folder that holds `path`.
fn folder_of(path: &Path) -> PathBuf {
    path.parent()
        .map_or_else(|| path.to_owned(), Path::to_owned)
}

/// `--toast <kind> <text>`: shows the card, then does what the user clicked.
#[cfg(windows)]
pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    use vixeeny_common::i18n::tr;
    use vixeeny_ui::toast_panel::{ToastContent, ToastEvent, ToastPanel, corner};

    let [kind, text] = args else {
        anyhow::bail!("usage: --toast <kind> <text>");
    };
    let toast =
        Toast::from_args(kind, text).ok_or_else(|| anyhow::anyhow!("unknown toast `{kind}`"))?;
    vixeeny_platform::ensure_dpi_aware();
    let config = vixeeny_common::paths::config_file()
        .and_then(|p| Config::load(&p).ok())
        .unwrap_or_default();
    let lang = crate::lang(&config.general.language);
    let dark = vixeeny_ui::side_panel::dark_theme(
        &config.general.theme,
        vixeeny_platform::system_prefers_dark(),
    );
    let content = match &toast {
        Toast::Saved(kind, path) => ToastContent {
            heading: tr(kind.title(), lang).into(),
            body: path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            thumb: crate::gallery::thumbnail(path, 160),
            error: false,
            dark,
            action_label: tr(Key::ToastOpenFolder, lang).into(),
        },
        Toast::Failed(kind, message) => ToastContent {
            heading: tr(kind.title(), lang).into(),
            body: message.clone(),
            thumb: None,
            error: true,
            dark,
            action_label: tr(Key::ToastOpenSettings, lang).into(),
        },
        Toast::Update(text) => ToastContent {
            heading: "Vixeeny".into(),
            body: text.clone(),
            thumb: None,
            error: false,
            dark,
            action_label: tr(Key::ToastOpenSettings, lang).into(),
        },
    };
    let panel = ToastPanel::new(&content).map_err(|e| anyhow::anyhow!("{e}"))?;
    let monitors = vixeeny_platform::monitors()?;
    let monitor = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first());
    if let Some(m) = monitor {
        let scale = m.scale_factor();
        let size = ((360.0 * scale) as u32, (96.0 * scale) as u32);
        let (x, y) = corner(
            (m.rect.x, m.rect.y, m.rect.width, m.rect.height),
            size,
            (16.0 * scale) as u32,
            (48.0 * scale) as u32,
        );
        panel.set_geometry(x, y, size.0, size.1);
    }
    let event = panel.run().map_err(|e| anyhow::anyhow!("{e}"))?;
    match (event, &toast) {
        (Some(ToastEvent::Activated), Toast::Saved(_, path)) => {
            let _ = vixeeny_platform::open_path(&path.display().to_string());
        }
        (Some(ToastEvent::Action), Toast::Saved(_, path)) => {
            let _ = std::process::Command::new("explorer.exe")
                .arg(format!("/select,{}", path.display()))
                .spawn();
            let _ = folder_of(path);
        }
        (Some(_), Toast::Failed(..) | Toast::Update(_)) => crate::open_settings(),
        (None, _) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toast_survives_the_command_line() {
        for toast in [
            Toast::Saved(Saved::Image, PathBuf::from(r"C:\Users\x\a b.png")),
            Toast::Saved(Saved::Replay, PathBuf::from("r.mp4")),
            Toast::Failed(Failed::Recording, "no encoder: \"x\"".into()),
            Toast::Failed(Failed::Audio, "mic: device removed".into()),
            Toast::Update("Version 1.2 is available".into()),
        ] {
            let [kind, text] = toast.to_args();
            assert_eq!(Toast::from_args(&kind, &text), Some(toast));
        }
        assert_eq!(Toast::from_args("nonsense", "x"), None);
    }

    #[test]
    fn full_disks_and_refused_folders_get_advice() {
        let full = anyhow::Error::new(std::io::Error::from_raw_os_error(112)).context("writing");
        assert_eq!(hint(&full), Some(Key::ToastDiskFull));
        let denied = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(hint(&denied), Some(Key::ToastAccessDenied));
        assert_eq!(hint(&anyhow::anyhow!("something else")), None);
        let text = failure_text(&anyhow::anyhow!("boom"), vixeeny_common::i18n::Lang::En);
        assert_eq!(text, "boom");
    }

    #[test]
    fn the_folder_of_a_file_is_its_parent() {
        assert_eq!(folder_of(Path::new("a/b.png")), PathBuf::from("a"));
    }
}
