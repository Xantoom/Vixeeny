// SPDX-License-Identifier: GPL-3.0-or-later
//! Notifications: a card with the thumbnail after a capture, a recording or a replay, and a clear message with a way
//! out when something fails. The cards are shown by a process of their own (`vixeeny-app --toast-host`), so they never
//! hold up the next hotkey. It is started once and reads the next cards on its standard input: a card costs no new
//! process, and only a host far behind can make the sender wait on the pipe. It ends with the process that started it
//! (its input closes), once its card is gone. A capture's thumbnail goes along, made from the picture in memory: the
//! card process does not read the file again.

use std::path::PathBuf;

use vixeeny_common::config::Config;
use vixeeny_common::i18n::Key;
use vixeeny_overlay::toast::{ToastContent, ToastEvent, Toasts};

/// What the card announces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toast {
    /// A file was written.
    Saved(Saved, PathBuf),
    /// A recording or a replay was written, with frames lost (the encoder could not keep up).
    Lossy(Saved, PathBuf, u64),
    /// Something failed; the message is already user-facing.
    Failed(Failed, String),
    /// The text read in a zone is on the clipboard.
    TextCopied(String),
    /// A new version can be installed (sent by the daemon); a click opens the settings.
    Update(String),
    /// A game started (its executable, and the name it is shown by): the replay runs with it (`true`) or is off.
    Replay(bool, PathBuf, String),
    /// What a card of this kind looks like (asked for in the settings); a click does nothing.
    Sample(Sample),
}

/// The kinds of cards, as the notifications page lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    Image,
    Text,
    Recording,
    Replay,
    Game,
    Failure,
    Update,
}

impl Sample {
    const ALL: [Self; 7] =
        [Self::Image, Self::Text, Self::Recording, Self::Replay, Self::Game, Self::Failure, Self::Update];

    /// Its name, as in the id of its row (`notify_<name>`) and on the card process's input.
    const fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Text => "text",
            Self::Recording => "recording",
            Self::Replay => "replay",
            Self::Game => "game",
            Self::Failure => "failures",
            Self::Update => "updates",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    /// A card of this kind, with made-up content.
    fn example(self, lang: vixeeny_common::i18n::Lang) -> Toast {
        use vixeeny_common::i18n::tr;
        match self {
            Self::Image => Toast::Saved(Saved::Image, PathBuf::from("Vixeeny.png")),
            Self::Text => Toast::TextCopied(tr(Key::ToastSampleText, lang).into()),
            Self::Recording => Toast::Saved(Saved::Recording, PathBuf::from("Vixeeny.mp4")),
            Self::Replay => Toast::Saved(Saved::Replay, PathBuf::from("Vixeeny.mp4")),
            Self::Game => {
                Toast::Replay(true, std::env::current_exe().unwrap_or_default(), tr(Key::ToastSampleGame, lang).into())
            }
            Self::Failure => Toast::Failed(Failed::Recording, tr(Key::ToastDiskFull, lang).into()),
            Self::Update => {
                Toast::Update(tr(Key::UpdateAvailable, lang).replace("{version}", env!("CARGO_PKG_VERSION")))
            }
        }
    }
}

/// A made-up capture for the samples: the accent colour, darkening down and across.
fn sample_picture(width: u32, height: u32) -> Thumb {
    let [r, g, b] = vixeeny_platform::system_accent().unwrap_or([0, 120, 212]);
    let mut data = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let shade = 1.0 - 0.6 * (x as f32 / width as f32 + y as f32 / height as f32) / 2.0;
            let c = |v: u8| (f32::from(v) * shade) as u8;
            data.extend_from_slice(&[c(r), c(g), c(b), 255]);
        }
    }
    Thumb { width, height, data }
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
    /// The replays' disk is almost full: a warning, before the saves fail.
    DiskLow,
    /// A zone was read and held no text.
    NoText,
    /// The text of a zone could not be read.
    Ocr,
    /// The app stopped unexpectedly (sent by the daemon); a click prepares a report.
    Crash,
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
            Self::DiskLow => "disk-low",
            Self::NoText => "no-text",
            Self::Ocr => "ocr",
            Self::Crash => "crash",
        }
    }

    const fn title(self) -> Key {
        match self {
            Self::Capture => Key::ToastCaptureFailed,
            Self::Recording => Key::ToastRecordingFailed,
            Self::Replay => Key::ToastReplayFailed,
            Self::Audio => Key::ToastAudioLost,
            Self::DiskLow => Key::ToastDiskLow,
            Self::NoText => Key::ToastNoText,
            Self::Ocr => Key::ToastOcrFailed,
            Self::Crash => Key::AppCrashed,
        }
    }
}

impl Toast {
    /// The card for `path`, written with `lost` frames lost.
    pub fn saved(kind: Saved, path: PathBuf, lost: u64) -> Self {
        if lost > 0 { Self::Lossy(kind, path, lost) } else { Self::Saved(kind, path) }
    }

    /// The arguments after `--toast`.
    pub fn to_args(&self) -> [String; 2] {
        match self {
            Self::Saved(kind, path) => [format!("saved-{}", kind.name()), path.display().to_string()],
            Self::Lossy(kind, path, lost) => [format!("lossy-{}", kind.name()), format!("{lost}|{}", path.display())],
            Self::Failed(kind, message) => [format!("failed-{}", kind.name()), message.clone()],
            Self::Update(text) => ["update".to_owned(), text.clone()],
            Self::TextCopied(text) => ["text-copied".to_owned(), text.clone()],
            // A path holds no `|`.
            Self::Replay(on, exe, name) => {
                [if *on { "replay-on" } else { "replay-off" }.to_owned(), format!("{}|{name}", exe.display())]
            }
            Self::Sample(kind) => ["sample".to_owned(), kind.name().to_owned()],
        }
    }

    pub fn from_args(kind: &str, text: &str) -> Option<Self> {
        let saved = |k| Some(Self::Saved(k, PathBuf::from(text)));
        let failed = |k| Some(Self::Failed(k, text.to_owned()));
        match kind {
            "saved-image" => saved(Saved::Image),
            "saved-recording" => saved(Saved::Recording),
            "saved-replay" => saved(Saved::Replay),
            "lossy-recording" | "lossy-replay" => {
                let (lost, path) = text.split_once('|')?;
                let kind = if kind == "lossy-replay" { Saved::Replay } else { Saved::Recording };
                Some(Self::Lossy(kind, PathBuf::from(path), lost.parse().ok()?))
            }
            "failed-capture" => failed(Failed::Capture),
            "failed-recording" => failed(Failed::Recording),
            "failed-replay" => failed(Failed::Replay),
            "failed-audio" => failed(Failed::Audio),
            "failed-disk-low" => failed(Failed::DiskLow),
            "failed-no-text" => failed(Failed::NoText),
            "failed-ocr" => failed(Failed::Ocr),
            "failed-crash" => failed(Failed::Crash),
            "text-copied" => Some(Self::TextCopied(text.to_owned())),
            "update" => Some(Self::Update(text.to_owned())),
            "sample" => Sample::from_name(text).map(Self::Sample),
            "replay-on" | "replay-off" => {
                let (exe, name) = text.split_once('|')?;
                Some(Self::Replay(kind == "replay-on", PathBuf::from(exe), name.to_owned()))
            }
            _ => None,
        }
    }
}

/// The advice for the failures the user can fix: a full disk, a folder without write access.
pub fn hint(error: &anyhow::Error) -> Option<Key> {
    error.chain().filter_map(|e| e.downcast_ref::<std::io::Error>()).find_map(|e| match (e.raw_os_error(), e.kind()) {
        // ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL, ENOSPC.
        (Some(112 | 39 | 28), _) | (_, std::io::ErrorKind::StorageFull) => Some(Key::ToastDiskFull),
        (_, std::io::ErrorKind::PermissionDenied) => Some(Key::ToastAccessDenied),
        _ => None,
    })
}

/// The text a failure card shows: the advice when there is one, else the error itself.
pub fn failure_text(error: &anyhow::Error, lang: vixeeny_common::i18n::Lang) -> String {
    hint(error).map_or_else(|| format!("{error:#}"), |key| vixeeny_common::i18n::tr(key, lang).to_owned())
}

/// A thumbnail.
pub type Thumb = vixeeny_editor::RgbaImage;

/// The largest side of a thumbnail sent to the card process.
const MAX_THUMB_SIDE: u32 = 1024;

/// Shows the card, unless the user turned notifications off. Waits only if the card process is far behind.
pub fn notify(config: &Config, toast: &Toast) {
    notify_with(config, toast, None);
}

/// [`notify`] with the thumbnail of the capture, made from the picture in memory.
pub fn notify_with(config: &Config, toast: &Toast, thumb: Option<Thumb>) {
    if !shown(config, toast) {
        return;
    }
    if let Err(e) = send_to_host(&encode(toast, thumb.as_ref())) {
        tracing::warn!("cannot show the notification: {e}");
    }
}

/// Shows a sample card of `kind`, whatever the switches say.
pub fn notify_sample(kind: Sample) {
    if let Err(e) = send_to_host(&encode(&Toast::Sample(kind), None)) {
        tracing::warn!("cannot show the notification: {e}");
    }
}

/// Whether the user wants the card for `toast` (the notifications page).
fn shown(config: &Config, toast: &Toast) -> bool {
    let n = &config.notifications;
    n.enabled
        && match toast {
            Toast::Sample(_) => true,
            Toast::Saved(kind, _) | Toast::Lossy(kind, ..) => match kind {
                Saved::Image => n.image_saved,
                Saved::Recording => n.recording_saved,
                Saved::Replay => n.replay_saved,
            },
            // The disk alert has its own switch, on the replay page.
            Toast::Failed(Failed::DiskLow, _) => true,
            // Asked for: a zone read, no text in it.
            Toast::Failed(Failed::NoText, _) => n.text_copied,
            Toast::Failed(..) => n.failures,
            Toast::TextCopied(_) => n.text_copied,
            Toast::Update(_) => n.updates,
            Toast::Replay(on, ..) => n.game_start(*on),
        }
}

/// The card's line, then the thumbnail's pixels when there is one (its size ends the line).
fn encode(toast: &Toast, thumb: Option<&Thumb>) -> Vec<u8> {
    let mut line = encode_line(&toast.to_args());
    let Some(thumb) = thumb.filter(|t| {
        (1..=MAX_THUMB_SIDE).contains(&t.width)
            && (1..=MAX_THUMB_SIDE).contains(&t.height)
            && t.data.len() == t.width as usize * t.height as usize * 4
    }) else {
        return line.into_bytes();
    };
    line.pop();
    line.push_str(&format!("\t{}x{}\n", thumb.width, thumb.height));
    let mut out = line.into_bytes();
    out.extend_from_slice(&thumb.data);
    out
}

/// The card process of this process, if started.
static HOST: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

fn send_to_host(message: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut host = HOST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(child) = host.as_mut()
        && matches!(child.try_wait(), Ok(None))
        && let Some(stdin) = child.stdin.as_mut()
        && stdin.write_all(message).and_then(|()| stdin.flush()).is_ok()
    {
        return Ok(());
    }
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .arg("--toast-host")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(message)?;
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

/// The card, and the size of the thumbnail whose pixels follow the line.
fn decode_line(line: &str) -> Option<(Toast, Option<(u32, u32)>)> {
    let (kind, rest) = line.split_once('\t')?;
    let (escaped, thumb) = match rest.split_once('\t') {
        Some((escaped, size)) => {
            let (w, h) = size.split_once('x')?;
            let (w, h) = (w.parse().ok()?, h.parse().ok()?);
            let sides = 1..=MAX_THUMB_SIDE;
            if !sides.contains(&w) || !sides.contains(&h) {
                return None;
            }
            (escaped, Some((w, h)))
        }
        None => (rest, None),
    };
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
    Some((Toast::from_args(kind, &text)?, thumb))
}

/// Reuses supplied pixels, otherwise decodes a media file once at the size needed by its card.
fn thumbnail_for(
    toast: &Toast,
    supplied: Option<Thumb>,
    load: impl FnOnce(&std::path::Path, u32) -> Option<Thumb>,
) -> Option<Thumb> {
    supplied.or_else(|| match toast {
        Toast::Saved(Saved::Image, path) => load(path, 640),
        Toast::Saved(_, path) | Toast::Lossy(_, path, _) => load(path, 256),
        _ => None,
    })
}

/// Prepares the card's text, theme and CPU pixels, without creating a window or a GPU bitmap.
fn content_of(toast: &Toast, thumb: Option<Thumb>) -> ToastContent {
    use vixeeny_common::i18n::tr;

    let config = Config::current();
    let lang = crate::lang(&config.general.language);
    let dark = crate::dark(&config);
    let thumb = thumbnail_for(toast, thumb, crate::thumbnail::thumbnail);
    let mut content = match toast {
        // A screenshot: its picture says it all.
        Toast::Saved(Saved::Image, _) if let Some(thumb) = thumb => ToastContent {
            heading: tr(Key::ToastImageSaved, lang).into(),
            thumb: Some(thumb),
            dark,
            picture_only: true,
            menu: [Key::ToastMenuOpen, Key::ToastMenuEdit, Key::ToastMenuReveal, Key::ToastMenuClose]
                .map(|k| tr(k, lang).to_owned())
                .to_vec(),
            ..ToastContent::default()
        },
        Toast::Saved(kind, path) => ToastContent {
            heading: tr(kind.title(), lang).into(),
            body: path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            thumb,
            error: false,
            dark,
            action_label: tr(Key::ToastOpenFolder, lang).into(),
            ..ToastContent::default()
        },
        Toast::Lossy(kind, _, lost) => ToastContent {
            heading: tr(kind.title(), lang).into(),
            body: crate::widget_math::frames_lost(*lost, lang),
            thumb,
            warning: true,
            dark,
            action_label: tr(Key::ToastOpenFolder, lang).into(),
            ..ToastContent::default()
        },
        Toast::Failed(kind, message) => ToastContent {
            heading: tr(kind.title(), lang).into(),
            body: message.clone(),
            thumb: None,
            // A filling disk is a warning: the replay still runs. A zone without text is no fault.
            error: !matches!(kind, Failed::DiskLow | Failed::NoText),
            warning: *kind == Failed::DiskLow,
            dark,
            action_label: match kind {
                Failed::NoText => String::new(),
                Failed::Ocr => tr(Key::ToastOcrLanguages, lang).into(),
                Failed::Crash => tr(Key::UiReport, lang).into(),
                _ => tr(Key::ToastOpenSettings, lang).into(),
            },
            ..ToastContent::default()
        },
        Toast::TextCopied(text) => ToastContent {
            heading: tr(Key::ToastTextCopied, lang).into(),
            body: text.clone(),
            dark,
            ..ToastContent::default()
        },
        Toast::Replay(on, exe, name) => {
            let path = exe.display().to_string();
            ToastContent {
                heading: tr(if *on { Key::ToastReplayOn } else { Key::ToastReplayOff }, lang).into(),
                body: name.clone(),
                thumb: vixeeny_platform::exe_icon(&path, 64).map(|(width, height, data)| Thumb { width, height, data }),
                dark,
                ..ToastContent::default()
            }
        }
        Toast::Update(text) => ToastContent {
            heading: "Vixeeny".into(),
            body: text.clone(),
            thumb: None,
            error: false,
            dark,
            action_label: tr(Key::UpdateView, lang).into(),
            ..ToastContent::default()
        },
        // The card of its kind, with a made-up picture where the real one has one.
        Toast::Sample(kind) => {
            let example = kind.example(lang);
            let mut content = content_of(&example, Some(sample_picture(640, 360)));
            if matches!(kind, Sample::Recording | Sample::Replay) {
                content.thumb = Some(sample_picture(256, 144));
            }
            content
        }
    };
    content.clickable = clickable(toast);
    content.place = vixeeny_overlay::toast::Place::from_name(&config.notifications.place);
    content.app_icon = std::env::current_exe().ok().and_then(|exe| {
        let (width, height, data) = vixeeny_platform::exe_icon(&exe.display().to_string(), 32)?;
        Some(Thumb { width, height, data })
    });
    content
}

/// A click on the card does something (see [`act`]); a sample shows how a click on its kind looks.
fn clickable(toast: &Toast) -> bool {
    match toast {
        Toast::Sample(kind) => clickable(&kind.example(vixeeny_common::i18n::Lang::En)),
        _ => !matches!(toast, Toast::Failed(Failed::NoText, _) | Toast::TextCopied(_) | Toast::Replay(..)),
    }
}

/// Shows the card for `toast` at its place on the main monitor (see [`ToastContent::place`]); `act` runs once it is gone.
fn show(toasts: &Toasts, toast: Toast, thumb: Option<Thumb>) -> anyhow::Result<()> {
    let content = content_of(&toast, thumb);
    show_prepared(toasts, toast, content)
}

/// Only window/device work runs on the UI thread; file decoding and shell icons are prepared by the reader.
fn show_prepared(toasts: &Toasts, toast: Toast, content: ToastContent) -> anyhow::Result<()> {
    let monitors = vixeeny_platform::monitor_layout()?;
    let monitor = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
        .ok_or_else(|| anyhow::anyhow!("no monitor"))?;
    toasts.show(&content, monitor.work, monitor.dpi, vixeeny_platform::animations_enabled(), move |event| {
        act(event, &toast)
    })?;
    Ok(())
}

/// Does what the user clicked on the card.
fn act(event: Option<ToastEvent>, toast: &Toast) {
    match (event, toast) {
        (Some(ToastEvent::Activated), Toast::Saved(_, path) | Toast::Lossy(_, path, _)) => {
            let _ = vixeeny_platform::open_path(&path.display().to_string());
        }
        (Some(ToastEvent::Action), Toast::Saved(_, path) | Toast::Lossy(_, path, _)) => {
            let _ = vixeeny_platform::reveal(&path.display().to_string());
        }
        (Some(ToastEvent::Edit), Toast::Saved(_, path)) => crate::region::open_editor(path),
        (Some(ToastEvent::Edit), _) => {}
        (Some(_), Toast::Failed(Failed::NoText, _) | Toast::TextCopied(_)) => {}
        // The languages page of Windows, where a recognition language is added.
        (Some(_), Toast::Failed(Failed::Ocr, _)) => {
            let _ = vixeeny_platform::open_path("ms-settings:regionlanguage");
        }
        (Some(_), Toast::Failed(Failed::Crash, _)) => {
            let config = Config::current();
            if let Err(e) = crate::report::create(&config) {
                tracing::error!("report: {e:#}");
            }
        }
        (Some(_), Toast::Failed(..)) => crate::open_settings(),
        (Some(_), Toast::Replay(..)) | (_, Toast::Sample(_)) => {}
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
    let toast = Toast::from_args(kind, text).ok_or_else(|| anyhow::anyhow!("unknown toast `{kind}`"))?;
    vixeeny_platform::ensure_dpi_aware();
    let toasts = Toasts::new()?;
    show(&toasts, toast, None)?;
    vixeeny_overlay::popup::pump_while(|| !toasts.is_empty());
    Ok(())
}

/// `--toast-host`: shows the cards read on standard input, a new one replacing the one on screen. Ends once the input
/// is closed and no card is left.
pub fn run_host() -> anyhow::Result<()> {
    use std::cell::Cell;
    use std::io::{BufRead, Read};
    use std::rc::Rc;

    vixeeny_platform::ensure_dpi_aware();
    let toasts = Rc::new(Toasts::new()?);
    let closed = Rc::new(Cell::new(false));
    // `None`: the input is closed.
    let (_mailbox, sender) = vixeeny_overlay::popup::mailbox({
        let (toasts, closed) = (toasts.clone(), closed.clone());
        move |mail: Option<(Toast, ToastContent)>| match mail {
            Some((toast, content)) => {
                if let Err(e) = show_prepared(&toasts, toast, content) {
                    tracing::error!("notification: {e:#}");
                }
            }
            None => closed.set(true),
        }
    })?;
    std::thread::Builder::new().name("toast-input".into()).spawn(move || {
        let mut input = std::io::stdin().lock();
        let mut line = Vec::new();
        loop {
            line.clear();
            match input.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line);
            let Some((toast, size)) = decode_line(text.trim_end_matches(['\n', '\r'])) else {
                continue;
            };
            let thumb = match size {
                Some((w, h)) => {
                    let mut rgba = vec![0; w as usize * h as usize * 4];
                    if input.read_exact(&mut rgba).is_err() {
                        break;
                    }
                    Some(Thumb { width: w, height: h, data: rgba })
                }
                None => None,
            };
            let content = content_of(&toast, thumb);
            sender.send(Some((toast, content)));
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
    fn supplied_thumbnails_avoid_loading_files_for_every_saved_media_kind() {
        for toast in [
            Toast::Saved(Saved::Image, "a.png".into()),
            Toast::Saved(Saved::Recording, "a.mp4".into()),
            Toast::Lossy(Saved::Replay, "a.mkv".into(), 2),
        ] {
            let thumb = Thumb::filled(2, 2, vixeeny_editor::Color::rgb(12, 34, 56));
            let expected = thumb.clone();
            let got = thumbnail_for(&toast, Some(thumb), |_, _| panic!("must not read the file"));
            assert_eq!(got, Some(expected));
        }
    }

    #[test]
    fn a_failed_thumbnail_is_loaded_only_once_and_non_media_cards_do_not_decode() {
        for (toast, size) in [
            (Toast::Saved(Saved::Image, "missing.png".into()), Some(640)),
            (Toast::Saved(Saved::Recording, "missing.mp4".into()), Some(256)),
            (Toast::Lossy(Saved::Replay, "missing.mkv".into(), 1), Some(256)),
            (Toast::Update("new version".into()), None),
        ] {
            let mut calls = Vec::new();
            assert!(
                thumbnail_for(&toast, None, |_, side| {
                    calls.push(side);
                    None
                })
                .is_none()
            );
            assert_eq!(calls, size.into_iter().collect::<Vec<_>>());
        }
    }

    #[test]
    fn a_toast_reaches_the_card_process_intact() {
        for toast in [
            Toast::Saved(Saved::Image, PathBuf::from(r"C:\Users\x\a b\tc.png")),
            Toast::Saved(Saved::Replay, PathBuf::from("r.mp4")),
            Toast::Lossy(Saved::Recording, PathBuf::from(r"C:\v\a|b.mp4"), 12),
            Toast::Lossy(Saved::Replay, PathBuf::from("r.mp4"), 1),
            Toast::Failed(Failed::Crash, String::new()),
            Toast::Failed(Failed::Recording, "line one\nline two\\n\r \"x\"".into()),
            Toast::Failed(Failed::Audio, "mic: device removed".into()),
            Toast::Update("Version 1.2 is available".into()),
            Toast::TextCopied(
                "Total	12,50 €
Merci"
                    .into(),
            ),
            Toast::Failed(Failed::NoText, String::new()),
            Toast::Failed(Failed::Ocr, "no language".into()),
            Toast::Replay(true, PathBuf::from(r"D:\Games\Hades\Hades.exe"), "Hades".into()),
            Toast::Replay(false, PathBuf::from("Client-Win64-Shipping.exe"), "Wuthering | Waves".into()),
            Toast::Sample(Sample::Image),
            Toast::Sample(Sample::Failure),
            Toast::Sample(Sample::Update),
        ] {
            let line = encode_line(&toast.to_args());
            assert_eq!(line.matches('\n').count(), 1);
            assert_eq!(decode_line(line.trim_end_matches('\n')), Some((toast, None)));
        }
        assert_eq!(
            Toast::saved(Saved::Replay, PathBuf::from("r.mp4"), 0),
            Toast::Saved(Saved::Replay, PathBuf::from("r.mp4"))
        );
        assert_eq!(decode_line("nonsense\tx"), None);
        assert_eq!(decode_line("no tab"), None);
    }

    #[test]
    fn a_thumbnail_follows_its_card_as_pixels() {
        let toast = Toast::Saved(Saved::Image, PathBuf::from(r"C:\a\b.png"));
        let rgba: Vec<u8> = (0..2 * 3 * 4).map(|i| i as u8).collect();
        let message = encode(&toast, Some(&Thumb { width: 2, height: 3, data: rgba.clone() }));
        let end = message.iter().position(|&b| b == b'\n').expect("a line");
        let line = std::str::from_utf8(&message[..end]).expect("text");
        assert_eq!(decode_line(line), Some((toast.clone(), Some((2, 3)))));
        assert_eq!(&message[end + 1..], rgba);
        // A thumbnail that does not match its size, or too large, is left out.
        let bad = encode(&toast, Some(&Thumb { width: 2, height: 3, data: vec![0; 5] }));
        assert_eq!(bad, encode_line(&toast.to_args()).into_bytes());
        assert_eq!(decode_line("saved-image\tx\t5000x2"), None);
    }

    #[test]
    fn each_card_follows_its_switch() {
        let mut config = Config::default();
        let image = Toast::Saved(Saved::Image, PathBuf::from("a.png"));
        let failed = Toast::Failed(Failed::Recording, String::new());
        assert!(shown(&config, &image) && shown(&config, &failed));
        config.notifications.image_saved = false;
        assert!(!shown(&config, &image) && shown(&config, &failed));
        assert!(shown(&config, &Toast::Lossy(Saved::Recording, PathBuf::from("a.mp4"), 3)));
        config.notifications.game_start = "on".into();
        assert!(shown(&config, &Toast::Replay(true, PathBuf::new(), String::new())));
        assert!(!shown(&config, &Toast::Replay(false, PathBuf::new(), String::new())));
        assert!(shown(&config, &Toast::Sample(Sample::Game)));
        // Each row of the notifications page has its sample.
        for name in ["image", "text", "recording", "replay", "game", "failures", "updates"] {
            assert!(Sample::from_name(name).is_some(), "{name}");
        }
        // Off: none at all.
        config.notifications.enabled = false;
        assert!(!shown(&config, &failed) && !shown(&config, &Toast::Failed(Failed::DiskLow, String::new())));
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
