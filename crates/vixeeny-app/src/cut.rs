// SPDX-License-Identifier: GPL-3.0-or-later
//! The video editor of the replays (`--cut <file>`), a process of its own: the window ([`vixeeny_overlay::cutter`]) on
//! the main thread; the pictures, the sound and the thumbnails decoded on threads of their own, which wake the window
//! when they have something for it.
//!
//! What is kept is written beside the replay (`<name>-cut.<ext>`) or where the user chooses; the replay itself is left
//! as it is.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use anyhow::Context;
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_encode::cut::{Clip, Next, Picture, Pictures, Reencode, Sound};
use vixeeny_image::anim::AnimFormat;
use vixeeny_overlay::cutter::{Cutter, CutterEvent, CutterHost, CutterTexts, Frames, Waker};

use crate::toast::{Failed, Saved, Toast};

/// Pictures decoded ahead of the one shown while playing (each one the size of the video on screen).
const AHEAD: usize = 4;

/// Requests this close together are a drag (or the wheel): a frame far from its key frame shows the key frame first,
/// then itself unless the next request comes before.
const DRAGGING: Duration = Duration::from_millis(250);

/// From this many frames after its key frame, a frame asked for while dragging shows the key frame first (about a tenth
/// of a second of 4K decoding on the graphics card).
const FAR_FROM_KEY: usize = 24;

/// Opens the editor on `path` (a replay just written), in its own process.
pub fn open(path: &Path) {
    // The editor comes to the front: this process may hand it the foreground.
    vixeeny_platform::allow_foreground_handoff();
    let spawned =
        std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--cut").arg(path).spawn());
    if let Err(e) = spawned {
        tracing::error!("cannot start the video editor: {e}");
    }
}

fn load_config() -> Config {
    vixeeny_common::paths::config_file().and_then(|path| Config::load(&path).ok()).unwrap_or_default()
}

fn texts(lang: Lang, path: &Path) -> CutterTexts {
    let t = |k| tr(k, lang).to_owned();
    CutterTexts {
        title: path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        save: t(Key::CutSave),
        save_as: t(Key::CutSaveAs),
        cancel: t(Key::CutCancel),
        lossless: t(Key::CutLossless),
        reencode: t(Key::CutReencode),
        play: t(Key::CutPlay),
        pause: t(Key::CutPause),
        previous: t(Key::CutPrevious),
        next: t(Key::CutNext),
        mute: t(Key::CutMute),
        close: t(Key::CutClose),
    }
}

/// `--cut <file>`.
pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    let path = PathBuf::from(args.first().context("no file to edit")?);
    vixeeny_platform::ensure_dpi_aware();
    let config = load_config();
    let lang = crate::lang(&config.general.language);
    let clip = Clip::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
    if clip.frames.is_empty() {
        anyhow::bail!("{} has no frame", path.display());
    }
    // The decoder is opened while the window is made (on the graphics card: about 0.2 s).
    let (tx, rx) = channel();
    let (give_waker, waker_given) = channel();
    let video = Video::start(&clip, tx.clone(), waker_given);
    let frames = Frames {
        times: (0..clip.frames.len()).map(|i| clip.time(i)).collect(),
        keys: (0..clip.frames.len()).filter(|&i| clip.frames[i].key).collect(),
        rate: 1.0 / clip.frame_seconds().max(1e-6),
        aspect: clip.width as f32 / clip.height.max(1) as f32,
    };

    // On the monitor of the pointer (the game's, most often).
    let monitors = vixeeny_platform::monitors()?;
    let cursor = vixeeny_platform::cursor_position()?;
    let monitor = vixeeny_platform::monitor_at(&monitors, cursor.0, cursor.1)
        .or_else(|| monitors.first())
        .context("no monitor")?;
    let u = monitor.dpi.max(48) as f32 / 96.0;
    let geometry = vixeeny_overlay::cutter::placement(monitor.work, u, frames.aspect);
    let cutter = Cutter::new(geometry, monitor.dpi, vixeeny_ui::theme::default_look(), frames, texts(lang, &path))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let waker = cutter.waker();
    let _ = give_waker.send(waker.clone());
    let sound = Speaker::start(&clip, waker.clone());
    thumbnails(&clip, &cutter, tx.clone(), waker.clone());
    let mut host = Host {
        clip,
        config,
        lang,
        video,
        sound,
        rx,
        tx,
        waker,
        size: cutter.video_size(),
        ahead: VecDeque::new(),
        playing: None,
        export: None,
        closing: false,
    };
    host.video.still(0, host.size);
    cutter.run(&mut host).map_err(|e| anyhow::anyhow!("{e}"))
}

/// What the threads hand the window.
enum Mail {
    /// A frame to show now (after a seek), of seek `generation`.
    Still(u64, Picture),
    /// A frame played, of play `generation`.
    Played(u64, usize, Picture),
    /// The pictures ran out while playing.
    End(u64),
    Thumbnail(usize, Picture),
    /// The writing ended: the file, or why not.
    Written(Result<PathBuf, String>),
}

/// What the picture thread is told, each request with its generation.
enum ToVideo {
    Still(u64, usize, (u32, u32)),
    /// From that frame up to the other one.
    Play(u64, usize, usize, (u32, u32)),
    Pause,
    /// The pixels of a picture the window is done with.
    Back(Vec<u8>),
}

/// The picture thread: decodes the frame asked for, or the ones after it while playing.
struct Video {
    asks: Sender<ToVideo>,
    /// The generation of the last request: what an older one still sends is dropped.
    generation: u64,
    /// The same, for the picture thread: it gives up a frame no longer asked for.
    newest: Arc<AtomicU64>,
}

impl Video {
    /// The window's waker comes once it is made: the decoder opens meanwhile.
    fn start(clip: &Clip, mail: Sender<Mail>, waker: Receiver<Waker>) -> Self {
        let (asks, rx) = channel();
        let clip = clip.clone();
        let newest = Arc::new(AtomicU64::new(0));
        let asked = Arc::clone(&newest);
        let spawned = std::thread::Builder::new().name("cut-video".into()).spawn(move || {
            if let Err(e) = video_thread(&clip, &rx, &asked, &mail, &waker) {
                tracing::error!("video editor: pictures: {e}");
            }
        });
        if let Err(e) = spawned {
            tracing::error!("video editor: {e}");
        }
        Self { asks, generation: 0, newest }
    }

    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.newest.store(self.generation, Ordering::Release);
        self.generation
    }

    fn still(&mut self, frame: usize, size: (u32, u32)) {
        let g = self.next_generation();
        let _ = self.asks.send(ToVideo::Still(g, frame, size));
    }

    fn play(&mut self, from: usize, last: usize, size: (u32, u32)) {
        self.next_generation();
        let _ = self.asks.send(ToVideo::Play(self.generation, from, last, size));
    }

    fn pause(&mut self) {
        self.next_generation();
        let _ = self.asks.send(ToVideo::Pause);
    }

    /// Hands a picture back to be written over.
    fn give_back(&self, picture: Picture) {
        let _ = self.asks.send(ToVideo::Back(picture.bgra));
    }
}

fn video_thread(
    clip: &Clip,
    rx: &Receiver<ToVideo>,
    newest: &AtomicU64,
    mail: &Sender<Mail>,
    waker: &Receiver<Waker>,
) -> Result<(), vixeeny_encode::recorder::RecordError> {
    let mut pictures = Pictures::open(clip)?;
    let Ok(waker) = waker.recv() else {
        return Ok(());
    };
    // When the last still frame was asked for: a drag asks for one after another.
    let mut last_still: Option<Instant> = None;
    // Where the decoder is: the next read gives this frame (no seek needed to go on).
    let mut next: Option<usize> = None;
    // While playing: its generation, its last frame, the size.
    let mut playing: Option<(u64, usize, (u32, u32))> = None;
    // Pictures sent and not handed back yet: playing stays this far ahead of the window.
    let mut out = 0_usize;
    loop {
        // Waits when there is nothing to decode (paused, or far enough ahead); a picture handed back or a request wakes
        // it.
        let decoding = playing.is_some() && out < AHEAD;
        let first = if decoding {
            match rx.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return Ok(()),
            }
        };
        // The newest request wins: seeks pile up while a slider is dragged.
        let mut ask = None;
        for message in first.into_iter().chain(rx.try_iter()) {
            match message {
                ToVideo::Back(bgra) => {
                    out = out.saturating_sub(1);
                    pictures.recycle(bgra);
                }
                other => ask = Some(other),
            }
        }
        match ask {
            Some(ToVideo::Still(g, frame, size)) => {
                playing = None;
                let dragging = last_still.is_some_and(|t| t.elapsed() < DRAGGING);
                last_still = Some(Instant::now());
                let stop = || newest.load(Ordering::Acquire) != g;
                if next != Some(frame) {
                    let key = clip.key_before(frame);
                    // Something to show at once while the frame is being reached.
                    if dragging && frame - key >= FAR_FROM_KEY && next != Some(key) {
                        pictures.seek(key)?;
                        if let Next::Frame((_, picture)) = pictures.read_unless(size, &stop)? {
                            out += 1;
                            let _ = mail.send(Mail::Still(g, picture));
                            waker.wake();
                        }
                    }
                    pictures.seek(frame)?;
                }
                match pictures.read_unless(size, &stop)? {
                    Next::Frame((i, picture)) => {
                        next = Some(i + 1);
                        out += 1;
                        let _ = mail.send(Mail::Still(g, picture));
                        waker.wake();
                    }
                    // Given up for a newer request: the decoder is somewhere before `frame`.
                    Next::Stopped | Next::End => next = None,
                }
                continue;
            }
            Some(ToVideo::Play(g, from, last, size)) => {
                if next != Some(from) {
                    pictures.seek(from)?;
                }
                playing = Some((g, last, size));
            }
            Some(ToVideo::Pause) => playing = None,
            Some(ToVideo::Back(_)) | None => {}
        }
        let Some((g, last, size)) = playing else {
            continue;
        };
        if out >= AHEAD {
            continue;
        }
        match pictures.read(size)? {
            Some((i, picture)) => {
                next = Some(i + 1);
                out += 1;
                let _ = mail.send(Mail::Played(g, i, picture));
                if i >= last {
                    playing = None;
                }
                waker.wake();
            }
            None => {
                next = None;
                playing = None;
                let _ = mail.send(Mail::End(g));
                waker.wake();
            }
        }
    }
}

enum SoundAsk {
    /// Play `.1` from this second until that one.
    Play(u64, f64, f64),
    Stop,
    Volume(f32),
}

/// The sound thread: the speakers and the clock of the preview.
struct Speaker {
    asks: Sender<SoundAsk>,
    /// Wakes the sound thread for a new ask.
    wake: Arc<vixeeny_audio::Wake>,
    /// Seconds of the clip heard now (`f64` bits), of play `heard_epoch`.
    clock: Arc<AtomicU64>,
    heard_epoch: Arc<AtomicU64>,
    /// The play asked for last (0: none).
    epoch: AtomicU64,
    /// The speakers failed: the preview runs on the wall clock.
    broken: Arc<AtomicBool>,
}

impl Speaker {
    fn start(clip: &Clip, waker: Waker) -> Option<Self> {
        if clip.audio_tracks == 0 {
            return None;
        }
        let (asks, rx) = channel();
        let wake = match vixeeny_audio::Wake::new() {
            Ok(wake) => Arc::new(wake),
            Err(e) => {
                tracing::warn!("video editor: sound: {e}");
                return None;
            }
        };
        let clock = Arc::new(AtomicU64::new(0));
        let heard_epoch = Arc::new(AtomicU64::new(0));
        let broken = Arc::new(AtomicBool::new(false));
        let clip = clip.clone();
        let shared = (clock.clone(), heard_epoch.clone(), broken.clone(), wake.clone());
        let spawned = std::thread::Builder::new().name("cut-sound".into()).spawn(move || {
            let (clock, heard_epoch, broken, wake) = shared;
            if let Err(e) = sound_thread(&clip, &rx, wake, &clock, &heard_epoch) {
                tracing::warn!("video editor: sound: {e}");
                broken.store(true, Ordering::SeqCst);
                waker.wake();
            }
        });
        if let Err(e) = spawned {
            tracing::error!("video editor: {e}");
            return None;
        }
        Some(Self { asks, wake, clock, heard_epoch, epoch: AtomicU64::new(0), broken })
    }

    fn play(&self, from: f64, until: f64) {
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.asks.send(SoundAsk::Play(epoch, from, until));
        self.wake.wake();
    }

    fn send(&self, ask: SoundAsk) {
        if matches!(ask, SoundAsk::Stop) {
            self.epoch.fetch_add(1, Ordering::SeqCst);
        }
        let _ = self.asks.send(ask);
        self.wake.wake();
    }

    /// Seconds of the clip heard now, once the sound of the last play has started.
    fn heard(&self) -> Option<f64> {
        let clock = f64::from_bits(self.clock.load(Ordering::SeqCst));
        (self.heard_epoch.load(Ordering::SeqCst) == self.epoch.load(Ordering::SeqCst)).then_some(clock)
    }

    fn works(&self) -> bool {
        !self.broken.load(Ordering::SeqCst)
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        // Out of a play, the thread waits on the asks, and ends when they close.
        self.send(SoundAsk::Stop);
    }
}

/// A play of the sound thread.
struct SoundPlay {
    epoch: u64,
    /// Where it started, seconds.
    from: f64,
    /// Samples decoded, not written yet.
    pending: VecDeque<f32>,
    /// Frames still to write.
    left: usize,
    /// Since when (and from what second) the speakers have played everything: the sound ended before the pictures.
    drained: Option<(Instant, f64)>,
}

fn sound_thread(
    clip: &Clip,
    rx: &Receiver<SoundAsk>,
    wake: Arc<vixeeny_audio::Wake>,
    clock: &AtomicU64,
    heard_epoch: &AtomicU64,
) -> anyhow::Result<()> {
    let mut speakers = vixeeny_audio::Speakers::open(wake.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut sound = Sound::open(clip)?.context("no sound")?;
    let channels = vixeeny_encode::cut::SOUND_CHANNELS;
    let rate = f64::from(vixeeny_encode::cut::SOUND_RATE);
    let mut play: Option<SoundPlay> = None;
    loop {
        let ask = if play.is_some() {
            // Woken by the speakers taking samples, or by a new ask.
            wake.wait();
            match rx.try_recv() {
                Ok(ask) => Some(ask),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        } else {
            match rx.recv() {
                Ok(ask) => Some(ask),
                Err(_) => return Ok(()),
            }
        };
        match ask {
            Some(SoundAsk::Play(epoch, from, to)) => {
                speakers.clear();
                sound.seek(from)?;
                let frames = ((to - from).max(0.0) * rate) as usize;
                play = Some(SoundPlay { epoch, from, pending: VecDeque::new(), left: frames, drained: None });
            }
            Some(SoundAsk::Stop) => {
                speakers.clear();
                play = None;
            }
            Some(SoundAsk::Volume(v)) => speakers.set_volume(v),
            None => {}
        }
        let Some(SoundPlay { epoch, from, pending, left, drained }) = play.as_mut() else {
            continue;
        };
        // Keeps the speakers' buffer full, up to the end of what is played.
        let mut room = speakers.room().map_err(|e| anyhow::anyhow!("{e}"))?;
        while room > 0 && *left > 0 {
            if pending.is_empty() {
                match sound.read()? {
                    Some(block) => pending.extend(block),
                    None => {
                        *left = 0;
                        break;
                    }
                }
            }
            let n = (pending.len() / channels).min(room).min(*left);
            let written =
                speakers.write(&pending.make_contiguous()[..n * channels]).map_err(|e| anyhow::anyhow!("{e}"))?;
            pending.drain(..written * channels);
            room -= written;
            *left -= written;
            if written < n {
                break;
            }
        }
        // Playing even with nothing written: the speakers' signal paces this loop.
        speakers.start().map_err(|e| anyhow::anyhow!("{e}"))?;
        let played = *from + speakers.played();
        // Past the end of the sound, the clock goes on by itself (a shorter sound track).
        let heard = if *left == 0 && room >= speakers.buffer_frames() as usize {
            let (since, at) = *drained.get_or_insert((Instant::now(), played));
            at + since.elapsed().as_secs_f64()
        } else {
            played
        };
        clock.store(heard.to_bits(), Ordering::SeqCst);
        heard_epoch.store(*epoch, Ordering::SeqCst);
    }
}

/// Decodes the thumbnails of the strip, one per slot, on a thread of its own.
fn thumbnails(clip: &Clip, cutter: &Cutter, mail: Sender<Mail>, waker: Waker) {
    let (slots, size) = cutter.thumbnail_slots();
    let wanted: Vec<(usize, usize)> = (0..slots).map(|slot| (slot, cutter.thumbnail_frame(slot))).collect();
    let clip = clip.clone();
    let _ = std::thread::Builder::new().name("cut-thumbnails".into()).spawn(move || {
        // Key frames only, one after the other: the processor does it without a second decoder (and its memory) on the
        // graphics card.
        let Ok(mut pictures) = Pictures::open_with(&clip, false) else {
            return;
        };
        for (slot, frame) in wanted {
            if pictures.seek(frame).is_err() {
                continue;
            }
            if let Ok(Some((_, picture))) = pictures.read(size) {
                if mail.send(Mail::Thumbnail(slot, picture)).is_err() {
                    return;
                }
                waker.wake();
            }
        }
    });
}

/// A writing under way.
struct Export {
    cancel: Arc<AtomicBool>,
    /// The share done (`f32` bits).
    progress: Arc<AtomicU32>,
}

struct Host {
    clip: Clip,
    config: Config,
    lang: Lang,
    video: Video,
    sound: Option<Speaker>,
    rx: Receiver<Mail>,
    tx: Sender<Mail>,
    waker: Waker,
    size: (u32, u32),
    /// Frames decoded ahead, waiting for their time.
    ahead: VecDeque<(usize, Picture)>,
    /// While playing: where and when it started, and its last frame.
    playing: Option<Playing>,
    export: Option<Export>,
    /// Closing once the writing has stopped (its file removed).
    closing: bool,
}

#[derive(Clone, Copy)]
struct Playing {
    from: f64,
    started: Instant,
    last: usize,
}

impl Host {
    fn play(&mut self, cutter: &Cutter) {
        let s = cutter.state();
        // Up to the end of the part kept, or of the clip from beyond it.
        let last = if s.position <= s.to { s.to } else { self.clip.frames.len() - 1 };
        let from = self.clip.time(s.position);
        let until = self.clip.time(last) + self.clip.frame_seconds();
        self.drop_ahead();
        self.video.play(s.position, last, self.size);
        if let Some(sound) = &self.sound {
            sound.play(from, until);
        }
        self.playing = Some(Playing { from, started: Instant::now(), last });
    }

    fn pause(&mut self, cutter: &Cutter) {
        self.playing = None;
        self.drop_ahead();
        self.video.pause();
        if let Some(sound) = &self.sound {
            sound.send(SoundAsk::Stop);
        }
        cutter.set_playing(false);
    }

    /// Hands the pictures decoded ahead back to the picture thread.
    fn drop_ahead(&mut self) {
        for (_, picture) in self.ahead.drain(..) {
            self.video.give_back(picture);
        }
    }

    /// The second of the clip playing now.
    fn now(&self, playing: &Playing) -> f64 {
        match &self.sound {
            Some(sound) if sound.works() => sound.heard().unwrap_or(playing.from),
            _ => playing.from + playing.started.elapsed().as_secs_f64(),
        }
    }

    fn save(&mut self, cutter: &Cutter, dest: PathBuf) {
        if self.export.is_some() {
            return;
        }
        if self.playing.is_some() {
            self.pause(cutter);
        }
        let s = cutter.state();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0));
        let clip = self.clip.clone();
        let config = self.config.clone();
        let (mail, waker) = (self.tx.clone(), self.waker.clone());
        let (stop, share) = (cancel.clone(), progress.clone());
        let range = (s.from, s.to);
        let spawned = std::thread::Builder::new().name("cut-export".into()).spawn(move || {
            // The encoders are looked up here, not on the window's thread.
            let reencoders = if clip.lossless(range.0, range.1) || anim_format(&dest).is_some() {
                Vec::new()
            } else {
                crate::record::reencoders(&config, &clip)
            };
            // The bar moves by whole pixels: the window is woken every half percent.
            let told = std::cell::Cell::new(0.0_f32);
            let tell = |p: f32| {
                if p - told.get() >= 0.005 || p >= 1.0 {
                    told.set(p);
                    share.store(p.to_bits(), Ordering::SeqCst);
                    waker.wake();
                }
            };
            let result = match anim_format(&dest) {
                Some(format) => {
                    let settings = crate::image_output(&config).1;
                    write_animation(&clip, range, &dest, format, &settings, &tell, &stop)
                }
                None => write(&clip, range, &dest, &reencoders, &tell, &stop),
            };
            let _ = mail.send(Mail::Written(result.map(|()| dest)));
            waker.wake();
        });
        match spawned {
            Ok(_) => {
                cutter.set_progress(Some(0.0));
                self.export = Some(Export { cancel, progress });
            }
            Err(e) => tracing::error!("video editor: {e}"),
        }
    }

    fn save_as(&mut self, cutter: &Cutter) {
        let ext = self.clip.path.extension().map_or_else(|| "mp4".to_owned(), |e| e.to_string_lossy().into_owned());
        let mut dialog = rfd::FileDialog::new()
            .set_title(tr(Key::CutSaveAs, self.lang).trim_end_matches('…'))
            .add_filter(ext.to_uppercase(), &[ext.as_str()]);
        // The part as an animated picture, to drop in a message or a ticket.
        let animated = tr(Key::CutAnimated, self.lang);
        for format in AnimFormat::ALL {
            let name = format.extension().to_uppercase();
            let label = if format == AnimFormat::Gif { name } else { format!("{name} ({animated})") };
            dialog = dialog.add_filter(label, &[format.extension()]);
        }
        let suggested = cut_name(&self.clip.path);
        if let Some(dir) = suggested.parent() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        if let Some(mut dest) = dialog.save_file() {
            if dest.extension().is_none() {
                dest.set_extension(&ext);
            }
            self.save(cutter, dest);
        }
    }

    fn mail(&mut self, cutter: &Cutter, mail: Mail) {
        let generation = self.video.generation;
        match mail {
            Mail::Still(g, picture) => {
                if g == generation {
                    cutter.show_frame(picture.width, picture.height, &picture.bgra);
                }
                self.video.give_back(picture);
            }
            Mail::Played(g, i, picture) => {
                if g == generation && self.playing.is_some() {
                    self.ahead.push_back((i, picture));
                } else {
                    self.video.give_back(picture);
                }
            }
            Mail::End(g) if g == generation => {
                if let Some(p) = self.playing.as_mut() {
                    // Nothing after this: the end comes with the last frame decoded.
                    p.last = self.ahead.back().map_or(cutter.state().position, |(i, _)| *i);
                }
            }
            Mail::Thumbnail(slot, picture) => {
                cutter.set_thumbnail(slot, picture.width, picture.height, &picture.bgra);
            }
            Mail::Written(result) => {
                self.export = None;
                cutter.set_progress(None);
                match result {
                    Ok(path) => {
                        tracing::info!("cut saved: {}", path.display());
                        // An animation is shown as a picture.
                        let kind = if anim_format(&path).is_some() { Saved::Image } else { Saved::Replay };
                        crate::toast::notify(&self.config, &Toast::Saved(kind, path));
                        cutter.close();
                    }
                    Err(e) if e == CANCELLED => {
                        if self.closing {
                            cutter.close();
                        }
                    }
                    Err(e) => {
                        tracing::error!("cannot write the cut: {e}");
                        crate::toast::notify(&self.config, &Toast::Failed(Failed::Replay, e));
                    }
                }
            }
            _ => {}
        }
    }
}

const CANCELLED: &str = "cancelled";

/// Writes the part, trying the encoders in turn when it must be encoded again.
fn write(
    clip: &Clip,
    range: (usize, usize),
    dest: &Path,
    reencoders: &[Reencode],
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(), String> {
    use vixeeny_encode::cut::export;
    if clip.lossless(range.0, range.1) {
        return export(clip, range, dest, None, progress, cancel).map_err(|e| e.to_string());
    }
    let mut last = String::from("no encoder available");
    for reencode in reencoders {
        match export(clip, range, dest, Some(reencode), progress, cancel) {
            Ok(()) => return Ok(()),
            Err(_) if cancel.load(Ordering::SeqCst) => return Err(CANCELLED.into()),
            Err(e) => {
                tracing::warn!("cut with {}: {e}", reencode.encoder.id);
                last = e.to_string();
            }
        }
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.into());
    }
    Err(last)
}

/// The animated format `path` asks for, by its extension.
fn anim_format(path: &Path) -> Option<AnimFormat> {
    AnimFormat::from_extension(path.extension()?.to_str()?)
}

/// Writes frames `from..=to` as an animated picture: at most the format's frame rate and size, in SDR (an HDR clip is
/// tone mapped, as in the preview).
fn write_animation(
    clip: &Clip,
    (from, to): (usize, usize),
    dest: &Path,
    format: AnimFormat,
    settings: &vixeeny_image::Settings,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(), String> {
    use vixeeny_image::anim::{Animation, fit, pick};

    if from > to || to >= clip.frames.len() {
        return Err("empty cut".into());
    }
    let start = clip.time(from);
    let ms = |t: f64| ((t - start) * 1000.0).round().max(0.0) as u64;
    let times: Vec<f64> = (from..=to).map(|i| clip.time(i)).collect();
    let mut keep = vec![false; times.len()];
    for i in pick(&times, format.max_fps()) {
        keep[i] = true;
    }
    let size = fit(clip.width, clip.height, format.max_side());
    // Written aside, then renamed: a cancelled or failed export leaves nothing behind.
    let tmp = dest.with_extension(format!("{}.part", format.extension()));
    let result = (|| {
        let mut animation = Animation::new(format, size.0, size.1, settings, &tmp).map_err(|e| e.to_string())?;
        let mut pictures = Pictures::open(clip).map_err(|e| e.to_string())?;
        pictures.seek(from).map_err(|e| e.to_string())?;
        while let Some((i, picture)) = pictures.read(size).map_err(|e| e.to_string())? {
            if i > to {
                break;
            }
            if cancel.load(Ordering::SeqCst) {
                return Err(CANCELLED.to_owned());
            }
            if keep[i - from] {
                animation.push(&picture.bgra, ms(clip.time(i))).map_err(|e| e.to_string())?;
            }
            pictures.recycle(picture.bgra);
            progress((i - from + 1) as f32 / times.len() as f32);
        }
        animation.finish(ms(clip.time(to) + clip.frame_seconds())).map_err(|e| e.to_string())
    })();
    match result {
        Ok(()) => std::fs::rename(&tmp, dest).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// `<name>-cut.<ext>` beside `original`, numbered when it is taken.
fn cut_name(original: &Path) -> PathBuf {
    let stem = original.file_stem().map_or_else(|| "replay".to_owned(), |s| s.to_string_lossy().into_owned());
    let ext = original.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|n| {
            let suffix = if n == 1 { "-cut".to_owned() } else { format!("-cut-{n}") };
            original.with_file_name(format!("{stem}{suffix}{ext}"))
        })
        .find(|p| !p.exists())
        .unwrap_or_else(|| original.with_file_name(format!("{stem}-cut{ext}")))
}

impl CutterHost for Host {
    fn event(&mut self, cutter: &Cutter, event: CutterEvent) {
        match event {
            CutterEvent::Seek(frame) => {
                if self.playing.is_some() {
                    self.pause(cutter);
                }
                self.video.still(frame, self.size);
            }
            CutterEvent::Play => self.play(cutter),
            CutterEvent::Pause => self.pause(cutter),
            CutterEvent::Range(from, to) => cutter.set_lossless(self.clip.lossless(from, to)),
            CutterEvent::Volume(volume, muted) => {
                if let Some(sound) = &self.sound {
                    sound.send(SoundAsk::Volume(if muted { 0.0 } else { volume }));
                }
            }
            CutterEvent::Save => {
                let dest = cut_name(&self.clip.path);
                self.save(cutter, dest);
            }
            CutterEvent::SaveAs => self.save_as(cutter),
            CutterEvent::Cancel => {
                if let Some(export) = &self.export {
                    export.cancel.store(true, Ordering::SeqCst);
                }
            }
            CutterEvent::Close => match &self.export {
                Some(export) => {
                    export.cancel.store(true, Ordering::SeqCst);
                    self.closing = true;
                    cutter.minimise();
                }
                None => cutter.close(),
            },
        }
    }

    fn tick(&mut self, cutter: &Cutter) -> Option<Duration> {
        while let Ok(mail) = self.rx.try_recv() {
            self.mail(cutter, mail);
        }
        if let Some(export) = &self.export {
            let p = f32::from_bits(export.progress.load(Ordering::SeqCst));
            if cutter.state().progress != Some(p) {
                cutter.set_progress(Some(p));
            }
        }
        let playing = self.playing?;
        let now = self.now(&playing);
        // The newest frame whose time has come; those before it are skipped.
        let mut shown: Option<(usize, Picture)> = None;
        while let Some((i, _)) = self.ahead.front() {
            if self.clip.time(*i) > now {
                break;
            }
            if let Some((_, skipped)) = shown.take() {
                self.video.give_back(skipped);
            }
            shown = self.ahead.pop_front();
        }
        if let Some((i, picture)) = shown {
            cutter.show_frame(picture.width, picture.height, &picture.bgra);
            cutter.set_position(i);
            self.video.give_back(picture);
        }
        if now >= self.clip.time(playing.last) + self.clip.frame_seconds() {
            self.pause(cutter);
            cutter.set_position(playing.last);
            return None;
        }
        // Wakes up for the next frame (the threads wake the loop when one is decoded).
        let next = self.ahead.front().map_or(self.clip.frame_seconds(), |(i, _)| (self.clip.time(*i) - now).max(0.0));
        Some(Duration::from_secs_f64(next.min(0.05)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two seconds at 30 fps, 320×180, no sound: each frame a grey as bright as its number.
    fn recording(dir: &Path) -> PathBuf {
        use vixeeny_encode::recorder::{FrameFormat, OutputContainer, RecordConfig, Recorder, Split, VideoFrame};
        use vixeeny_encode::registry::{Chroma, PresetName, Registry};
        const MS: i64 = 1_000_000;
        let registry = Registry::builtin().unwrap();
        let encoder = registry.get("libx264").unwrap().clone();
        let options =
            encoder.presets.get(PresetName::Performance).iter().map(|(k, v)| (k.clone(), v.to_ffmpeg())).collect();
        let cfg = RecordConfig {
            crop: None,
            encoder,
            options,
            container: OutputContainer::Mp4Hybrid,
            output_size: (320, 180),
            fps: vixeeny_encode::clock::Fps::whole(30),
            depth: 8,
            chroma: Chroma::C420,
            hdr: false,
            peak_nits: 1000.0,
            sdr_white_nits: 203.0,
            split: Split::Off,
            keyframe_seconds: 1.0,
            queue: 1_000,
            vfr: false,
            audio: vec![],
            gpu: None,
            replay_seconds: None,
            replay_storage: vixeeny_encode::replay::Storage::Ram,
            files: true,
            buffers: Default::default(),
        };
        let out = dir.join("rec.mp4");
        let path = out.clone();
        let rec = Recorder::start(cfg, Box::new(move |_| path.clone())).unwrap();
        for i in 0..60_usize {
            let v = (i * 4) as u8;
            let frame = VideoFrame {
                width: 320,
                height: 180,
                stride: 320 * 4,
                format: FrameFormat::Bgra8,
                data: [v, v, v, 255].repeat(320 * 180),
            };
            rec.push_frame(i as i64 * 1_000 * MS / 30, frame);
        }
        rec.stop(2_000 * MS).unwrap();
        out
    }

    #[test]
    fn a_part_of_a_clip_becomes_an_animation() {
        let dir = std::env::temp_dir().join(format!("vixeeny-cut-anim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let clip = match Clip::open(&recording(&dir)) {
            Ok(clip) => clip,
            Err(e) => {
                vixeeny_common::skip_test(format!("no FFmpeg: {e}"));
                return;
            }
        };
        let settings = vixeeny_image::Settings::default();
        let cancel = AtomicBool::new(false);
        let last = std::cell::Cell::new(0.0);
        // One second (frames 15 to 44) as a GIF: 15 frames at 15 fps.
        let gif = dir.join("part.gif");
        write_animation(&clip, (15, 44), &gif, AnimFormat::Gif, &settings, &|p| last.set(p), &cancel).unwrap();
        assert!((last.get() - 1.0).abs() < 1e-6);
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::Indexed);
        let mut decoder = options.read_info(std::fs::File::open(&gif).unwrap()).unwrap();
        let (mut frames, mut hundredths) = (0, 0_u32);
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            frames += 1;
            hundredths += u32::from(frame.delay);
            assert_eq!((frame.width, frame.height), (320, 180));
        }
        assert_eq!((frames, hundredths), (15, 100));
        // WebP keeps the clip's 30 fps; nothing is left beside the file.
        let webp = dir.join("part.webp");
        write_animation(&clip, (0, 59), &webp, AnimFormat::WebP, &settings, &|_| {}, &cancel).unwrap();
        assert!(std::fs::metadata(&webp).unwrap().len() > 0);
        assert_eq!(anim_format(&webp), Some(AnimFormat::WebP));
        // Cancelled: no file, no leftover.
        cancel.store(true, Ordering::SeqCst);
        let gone = dir.join("gone.avif");
        let result = write_animation(&clip, (0, 59), &gone, AnimFormat::Avif, &settings, &|_| {}, &cancel);
        assert_eq!(result, Err(CANCELLED.to_owned()));
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 3, "{left:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_cut_is_named_after_its_replay_and_never_overwrites_one() {
        let dir = std::env::temp_dir().join(format!("vixeeny-cut-name-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let original = dir.join("Replay 2026-10-15.mp4");
        assert_eq!(cut_name(&original), dir.join("Replay 2026-10-15-cut.mp4"));
        std::fs::write(dir.join("Replay 2026-10-15-cut.mp4"), b"").unwrap_or_default();
        assert_eq!(cut_name(&original), dir.join("Replay 2026-10-15-cut-2.mp4"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
