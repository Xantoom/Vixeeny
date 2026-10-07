// SPDX-License-Identifier: GPL-3.0-or-later
//! Notifications (plan 5.14): a card with the thumbnail after a capture, a recording or a replay,
//! and a clear message with a way out when something fails. The cards are shown by a process of
//! their own (`vixeeny-app --toast-host`), so they never hold up the next hotkey. It is started
//! once and reads the next cards on its standard input: a card costs no new process. It ends with
//! the process that started it (its input closes), once its card is gone.

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

/// The folder that holds `path`.
fn folder_of(path: &Path) -> PathBuf {
    path.parent()
        .map_or_else(|| path.to_owned(), Path::to_owned)
}

/// What the card shows, in the current language and theme.
fn content_of(toast: &Toast) -> vixeeny_ui::toast_panel::ToastContent {
    use vixeeny_common::i18n::tr;
    use vixeeny_ui::toast_panel::ToastContent;

    let config = vixeeny_common::paths::config_file()
        .and_then(|p| Config::load(&p).ok())
        .unwrap_or_default();
    let lang = crate::lang(&config.general.language);
    let dark = vixeeny_ui::side_panel::dark_theme(
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

/// A card for `toast`, in the bottom-right corner of the main monitor.
fn panel_for(toast: &Toast) -> anyhow::Result<vixeeny_ui::toast_panel::ToastPanel> {
    use vixeeny_ui::toast_panel::{ToastPanel, corner};

    let content = content_of(toast);
    let panel = ToastPanel::new(&content).map_err(|e| anyhow::anyhow!("{e}"))?;
    let monitors = vixeeny_platform::monitors()?;
    let monitor = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first());
    if let Some(m) = monitor {
        let scale = m.scale_factor();
        let (w, h) = vixeeny_ui::toast_panel::size_for(&content);
        let size = ((w * scale) as u32, (h * scale) as u32);
        let (x, y) = corner(
            (m.rect.x, m.rect.y, m.rect.width, m.rect.height),
            size,
            (16.0 * scale) as u32,
            (48.0 * scale) as u32,
        );
        panel.set_geometry(x, y, size.0, size.1);
    }
    Ok(panel)
}

/// Does what the user clicked on the card.
fn act(event: Option<vixeeny_ui::toast_panel::ToastEvent>, toast: &Toast) {
    use vixeeny_ui::toast_panel::ToastEvent;
    match (event, toast) {
        (Some(ToastEvent::Activated), Toast::Saved(_, path)) => {
            let _ = vixeeny_platform::open_path(&path.display().to_string());
        }
        (Some(ToastEvent::Action), Toast::Saved(_, path)) => {
            let _ = std::process::Command::new("explorer.exe")
                .arg(format!("/select,{}", path.display()))
                .spawn();
            let _ = folder_of(path);
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
    let event = panel_for(&toast)?
        .run()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    act(event, &toast);
    Ok(())
}

/// `--toast-host`: shows the cards read on standard input, a new one replacing the one on
/// screen. Ends once the input is closed and no card is left.
pub fn run_host() -> anyhow::Result<()> {
    use std::cell::{Cell, RefCell};
    use std::io::BufRead;
    use std::rc::Rc;

    thread_local! {
        /// The card on screen, numbered.
        static SHOWN: RefCell<Option<(u64, vixeeny_ui::toast_panel::ToastPanel)>> =
            const { RefCell::new(None) };
        static COUNT: Cell<u64> = const { Cell::new(0) };
        static INPUT_CLOSED: Cell<bool> = const { Cell::new(false) };
    }
    fn quit_if_done() {
        if INPUT_CLOSED.get() && SHOWN.with(|s| s.borrow().is_none()) {
            let _ = vixeeny_ui::slint::quit_event_loop();
        }
    }
    fn show(toast: Toast) {
        let panel = match panel_for(&toast) {
            Ok(panel) => panel,
            Err(e) => {
                tracing::error!("notification: {e:#}");
                return;
            }
        };
        let id = COUNT.get() + 1;
        COUNT.set(id);
        let toast = Rc::new(toast);
        let result = panel.show(move |event| {
            act(event, &toast);
            // Forget the card unless a newer one took its place; it is dropped after its own
            // callback has returned.
            let gone = SHOWN.with(|s| {
                let mut s = s.borrow_mut();
                if s.as_ref().is_some_and(|(shown, _)| *shown == id) {
                    s.take()
                } else {
                    None
                }
            });
            vixeeny_ui::slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                drop(gone);
                quit_if_done();
            });
        });
        if let Err(e) = result {
            tracing::error!("notification: {e}");
            return;
        }
        // The previous card goes: the new one has its place.
        if let Some((_, old)) = SHOWN.with(|s| s.borrow_mut().replace((id, panel))) {
            let _ = vixeeny_ui::ComponentHandle::hide(old.window());
        }
    }

    vixeeny_platform::ensure_dpi_aware();
    // The input is read once the event loop runs: before, the cards it posts would be lost.
    vixeeny_ui::slint::Timer::single_shot(std::time::Duration::ZERO, || {
        let reader = std::thread::Builder::new()
            .name("toast-input".into())
            .spawn(|| {
                for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                    if let Some(toast) = decode_line(&line) {
                        let _ = vixeeny_ui::slint::invoke_from_event_loop(move || show(toast));
                    }
                }
                let _ = vixeeny_ui::slint::invoke_from_event_loop(|| {
                    INPUT_CLOSED.set(true);
                    quit_if_done();
                });
            });
        if let Err(e) = reader {
            tracing::error!("notifications: {e}");
            let _ = vixeeny_ui::slint::quit_event_loop();
        }
    });
    vixeeny_ui::slint::run_event_loop_until_quit().map_err(|e| anyhow::anyhow!("{e}"))
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

    #[test]
    fn the_folder_of_a_file_is_its_parent() {
        assert_eq!(folder_of(Path::new("a/b.png")), PathBuf::from("a"));
    }
}
