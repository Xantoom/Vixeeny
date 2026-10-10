// SPDX-License-Identifier: GPL-3.0-or-later
//! Notifications (plan 5.14): a card with the thumbnail after a capture, a recording or a replay,
//! and a clear message with a way out when something fails. The cards are shown by a process of
//! their own (`vixeeny-app --toast-host`), so they never hold up the next hotkey. It is started
//! once and reads the next cards on its standard input: a card costs no new process. It ends with
//! the process that started it (its input closes), once its card is gone.

use std::path::PathBuf;

use vixeeny_common::config::Config;
use vixeeny_common::i18n::Key;
use vixeeny_overlay::toast::{ToastContent, ToastEvent, Toasts};

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
    let line = encode_line(&toast.to_args());
    if let Err(e) = send_to_host(&line) {
        tracing::warn!("cannot show the notification: {e}");
    }
}

/// The card process of this process, if started.
static HOST: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

fn send_to_host(line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut host = HOST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(child) = host.as_mut()
        && matches!(child.try_wait(), Ok(None))
        && let Some(stdin) = child.stdin.as_mut()
        && stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_ok()
    {
        return Ok(());
    }
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .arg("--toast-host")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(line.as_bytes())?;
        stdin.flush()?;
    }
    *host = Some(child);
    Ok(())
}

/// `kind<TAB>text<LF>`, the text with `\`, tabs and line breaks escaped.
fn encode_line([kind, text]: &[String; 2]) -> String {
    let mut out = String::with_capacity(kind.len() + text.len() + 2);
    out.push_str(kind);
    out.push('\t');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('\n');
    out
}

fn decode_line(line: &str) -> Option<Toast> {
    let (kind, escaped) = line.split_once('\t')?;
    let mut text = String::with_capacity(escaped.len());
    let mut chars = escaped.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next()? {
                'n' => text.push('\n'),
                'r' => text.push('\r'),
                't' => text.push('\t'),
                other => text.push(other),
            }
        } else {
            text.push(c);
        }
    }
    Toast::from_args(kind, &text)
}

/// What the card shows, in the current language and theme.
fn content_of(toast: &Toast) -> ToastContent {
    use vixeeny_common::i18n::tr;

    let config = vixeeny_common::paths::config_file()
        .and_then(|p| Config::load(&p).ok())
        .unwrap_or_default();
    let lang = crate::lang(&config.general.language);
    let dark = vixeeny_overlay::dark_theme(
        &config.general.theme,
        vixeeny_platform::system_prefers_dark(),
    );
    match toast {
        Toast::Saved(kind, path) => ToastContent {
            heading: tr(kind.title(), lang).into(),
            body: path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            thumb: crate::thumbnail::thumbnail(path, 256),
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
            action_label: tr(Key::UpdateView, lang).into(),
        },
    }
}

/// Shows the card for `toast` in the bottom-right corner of the main monitor, above its taskbar;
/// `act` runs once it
/// is gone.
fn show(toasts: &Toasts, toast: Toast) -> anyhow::Result<()> {
    let content = content_of(&toast);
    let monitors = vixeeny_platform::monitors()?;
    let monitor = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
        .ok_or_else(|| anyhow::anyhow!("no monitor"))?;
    toasts.show(
        &content,
        (
            monitor.work.x,
            monitor.work.y,
            monitor.work.width,
            monitor.work.height,
        ),
        monitor.dpi,
        vixeeny_platform::animations_enabled(),
        move |event| act(event, &toast),
    )?;
    Ok(())
}

/// Does what the user clicked on the card.
fn act(event: Option<ToastEvent>, toast: &Toast) {
    match (event, toast) {
        (Some(ToastEvent::Activated), Toast::Saved(_, path)) => {
            let _ = vixeeny_platform::open_path(&path.display().to_string());
        }
        (Some(ToastEvent::Action), Toast::Saved(_, path)) => {
            let _ = vixeeny_platform::reveal(&path.display().to_string());
        }
        (Some(_), Toast::Failed(..)) => crate::open_settings(),
        (Some(_), Toast::Update(_)) => {
            if let Err(e) = crate::settings::spawn(true) {
                tracing::error!("{e:#}");
            }
        }
        (None, _) => {}
    }
}

/// `--toast <kind> <text>`: shows one card, then does what the user clicked.
pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    let [kind, text] = args else {
        anyhow::bail!("usage: --toast <kind> <text>");
    };
    let toast =
        Toast::from_args(kind, text).ok_or_else(|| anyhow::anyhow!("unknown toast `{kind}`"))?;
    vixeeny_platform::ensure_dpi_aware();
    let toasts = Toasts::new()?;
    show(&toasts, toast)?;
    vixeeny_overlay::popup::pump_while(|| !toasts.is_empty());
    Ok(())
}

/// `--toast-host`: shows the cards read on standard input, a new one replacing the one on
/// screen. Ends once the input is closed and no card is left.
pub fn run_host() -> anyhow::Result<()> {
    use std::cell::Cell;
    use std::io::BufRead;
    use std::rc::Rc;

    vixeeny_platform::ensure_dpi_aware();
    let toasts = Rc::new(Toasts::new()?);
    let closed = Rc::new(Cell::new(false));
    // `None`: the input is closed.
    let (_mailbox, sender) = vixeeny_overlay::popup::mailbox({
        let (toasts, closed) = (toasts.clone(), closed.clone());
        move |mail: Option<Toast>| match mail {
            Some(toast) => {
                if let Err(e) = show(&toasts, toast) {
                    tracing::error!("notification: {e:#}");
                }
            }
            None => closed.set(true),
        }
    })?;
    std::thread::Builder::new()
        .name("toast-input".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                if let Some(toast) = decode_line(&line) {
                    sender.send(Some(toast));
                }
            }
            sender.send(None);
        })?;
    vixeeny_overlay::popup::pump_while(|| !(closed.get() && toasts.is_empty()));
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
    fn a_toast_survives_the_line_to_the_card_process() {
        for toast in [
            Toast::Saved(Saved::Image, PathBuf::from(r"C:\Users\x\a\tb.png")),
            Toast::Failed(Failed::Recording, "line one\nline two\\n\r".into()),
            Toast::Update("Version 1.2".into()),
        ] {
            let line = encode_line(&toast.to_args());
            assert_eq!(line.matches('\n').count(), 1);
            assert!(line.ends_with('\n'));
            assert_eq!(decode_line(line.trim_end_matches('\n')), Some(toast));
        }
        assert_eq!(decode_line("no tab"), None);
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
}
