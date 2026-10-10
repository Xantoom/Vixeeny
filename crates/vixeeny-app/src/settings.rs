// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings window (plan 5.13), a process of its own (`vixeeny-app --settings`)
//! so the recording hotkeys keep working while it is open. Changes are saved as soon as they stop
//! for a moment, and the daemon is asked to reload.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::{self, ControlRequest, Endpoint, Hello};
use vixeeny_encode::probe::ProbeResult;
use vixeeny_encode::registry::Registry;
use vixeeny_settings::Section;
use vixeeny_ui::ComponentHandle;
use vixeeny_ui::settings_panel::{Line, PanelHandle, SettingsPanel, UpdateStage, UpdateView};
use vixeeny_updater::client;
use vixeeny_updater::state::State as UpdateState;

const REPO: &str = "https://github.com/Xantoom/Vixeeny";
/// Marks the settings window so that a second launch can bring it forward.
const SETTINGS_TAG: &str = "Vixeeny.Settings";
/// Quiet time after the last change before it is saved.
const SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// Starts the settings window in its own process (on the updates page with `updates`).
pub fn spawn(updates: bool) -> anyhow::Result<()> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.arg("--settings");
    if updates {
        command.arg("--updates");
    }
    command
        .spawn()
        .context("cannot start the settings window")?;
    Ok(())
}

fn load_config() -> Config {
    vixeeny_common::paths::config_file()
        .and_then(|path| Config::load(&path).ok())
        .unwrap_or_default()
}

/// Writes the settings, then tells the daemon to reload them (off the UI thread).
pub fn save(config: &Config) {
    let Some(path) = vixeeny_common::paths::config_file() else {
        tracing::error!("no settings folder: the change is not saved");
        return;
    };
    if let Err(e) = config.save(&path) {
        tracing::error!("cannot save the settings: {e}");
        return;
    }
    std::thread::spawn(|| {
        let result = Endpoint::current_user().connect().and_then(|mut stream| {
            ipc::write_msg(&mut stream, &Hello::Control(ControlRequest::ReloadConfig))
                .map_err(std::io::Error::other)?;
            ipc::read_msg::<_, ipc::ControlReply>(&mut stream)
                .map(|_| ())
                .map_err(std::io::Error::other)
        });
        if let Err(e) = result {
            tracing::debug!("the daemon was not told about the new settings: {e}");
        }
    });
}

pub fn hardware_lines(lang: Lang, result: &ProbeResult) -> Vec<Line> {
    let registry = Registry::builtin().ok();
    let name_of = |id: &str| {
        registry
            .as_ref()
            .and_then(|r| r.get(id))
            .map_or_else(|| id.to_owned(), |e| e.display_name.clone())
    };
    let vendor_of = |id: &str| registry.as_ref().and_then(|r| r.get(id)).map(|e| e.vendor);
    let mut lines = Vec::new();
    let real: Vec<_> = result.adapters.iter().filter(|a| !a.software).collect();
    if real.is_empty() {
        lines.push(Line {
            text: tr(Key::HwNoGpu, lang).into(),
            ..Line::default()
        });
    }
    for adapter in real {
        let encoders: Vec<String> = result
            .encoders
            .iter()
            .filter(|e| {
                e.adapter == Some(adapter.index) && vendor_of(&e.id) == Some(adapter.vendor)
            })
            .map(|e| name_of(&e.id))
            .collect();
        lines.push(Line {
            text: adapter.name.clone(),
            detail: format!(
                "{} — {}",
                tr(Key::HwDriver, lang).replace("{version}", &adapter.driver_version),
                encoders.join(", ")
            ),
        });
    }
    let software: Vec<String> = result
        .encoders
        .iter()
        .filter(|e| e.adapter.is_none())
        .map(|e| name_of(&e.id))
        .collect();
    if !software.is_empty() {
        lines.push(Line {
            text: tr(Key::HwSoftware, lang).into(),
            detail: software.join(", "),
        });
    }
    lines
}

fn update_state() -> UpdateState {
    vixeeny_updater::state::file()
        .map(|p| UpdateState::load(&p))
        .unwrap_or_default()
}

/// What the update card says, from what the last check and download left.
fn update_view(lang: Lang) -> UpdateView {
    let state = update_state();
    let current = env!("CARGO_PKG_VERSION");
    if let Some(version) = client::ready() {
        return UpdateView {
            stage: UpdateStage::Ready,
            title: tr(Key::UpdateReady, lang).replace("{version}", &version),
            detail: tr(Key::UpdateReadyHint, lang).into(),
            progress: 1.0,
            action: tr(Key::UpdateRestart, lang).into(),
        };
    }
    // An error only matters while its version is still to be installed (a newer Vixeeny,
    // installed since, by hand for instance, forgets it).
    if let Some(error) = state
        .error
        .as_ref()
        .filter(|_| state.newer_than(current).is_some())
    {
        return UpdateView {
            stage: UpdateStage::Failed,
            title: tr(Key::UpdateFailedShort, lang).into(),
            detail: error.clone(),
            progress: 0.0,
            action: tr(Key::UiRetry, lang).into(),
        };
    }
    match state.newer_than(current) {
        Some(release) => UpdateView {
            stage: UpdateStage::Available,
            title: tr(Key::UpdateFound, lang).replace("{version}", &release.version),
            detail: String::new(),
            progress: 0.0,
            action: tr(Key::UpdateNow, lang).into(),
        },
        None => UpdateView {
            stage: UpdateStage::Idle,
            title: tr(Key::UpdateUpToDate, lang).into(),
            detail: if state.checked_at == 0 {
                tr(Key::UpdateNever, lang).into()
            } else {
                String::new()
            },
            progress: 0.0,
            action: tr(Key::SetCheckNow, lang).into(),
        },
    }
}

fn check_for_update(handle: PanelHandle, lang: Lang) {
    handle.set_update(UpdateView {
        stage: UpdateStage::Checking,
        title: tr(Key::UpdateChecking, lang).into(),
        ..UpdateView::default()
    });
    std::thread::spawn(move || match client::check() {
        Ok(_) => handle.set_update(update_view(lang)),
        Err(e) => {
            tracing::warn!("update check: {e:#}");
            handle.set_update(UpdateView {
                stage: UpdateStage::Failed,
                title: tr(Key::UpdateCheckFailedShort, lang).into(),
                detail: tr(Key::UpdateCheckFailed, lang).into(),
                progress: 0.0,
                action: tr(Key::UiRetry, lang).into(),
            });
        }
    });
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// Downloads with the progress on the card; the card then offers to restart.
fn download_update(handle: PanelHandle, lang: Lang) {
    let version = update_state()
        .newer_than(env!("CARGO_PKG_VERSION"))
        .map(|r| r.version.clone())
        .unwrap_or_default();
    let title = tr(Key::UpdateDownloading, lang).replace("{version}", &version);
    handle.set_update(UpdateView {
        stage: UpdateStage::Downloading,
        title: title.clone(),
        ..UpdateView::default()
    });
    std::thread::spawn(move || {
        let mut shown = u64::MAX;
        let result = client::download(&mut |done, total| {
            let percent = done * 100 / total.max(1);
            if percent != shown {
                shown = percent;
                handle.set_update(UpdateView {
                    stage: UpdateStage::Downloading,
                    title: title.clone(),
                    detail: format!("{} / {}", megabytes(done), megabytes(total)),
                    progress: done as f32 / total.max(1) as f32,
                    action: String::new(),
                });
            }
        });
        if let Err(e) = &result {
            tracing::error!("update download: {e:#}");
            if let Some(path) = vixeeny_updater::state::file() {
                let mut state = UpdateState::load(&path);
                state.error = Some(format!("{e:#}"));
                let _ = state.save(&path);
            }
        }
        handle.set_update(update_view(lang));
    });
}

/// Installs in a process of its own and closes this window: the new version opens it again.
fn restart_for_update(handle: &PanelHandle, lang: Lang) {
    handle.set_update(UpdateView {
        stage: UpdateStage::Restarting,
        title: tr(Key::UpdateInstalling, lang).into(),
        progress: 1.0,
        ..UpdateView::default()
    });
    match crate::update::spawn_install() {
        Ok(()) => handle.close(),
        Err(e) => {
            tracing::error!("cannot start the installation: {e:#}");
            handle.set_update(update_view(lang));
        }
    }
}

fn about_lines(lang: Lang) -> Vec<Line> {
    vec![Line {
        text: tr(Key::AboutLicense, lang).into(),
        detail: REPO.into(),
    }]
}

/// `--settings [--updates]`: the window, until it is closed.
pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    // `--about` came from versions before 0.9.11.
    let open_updates = args.iter().any(|a| a == "--updates" || a == "--about");
    vixeeny_platform::ensure_dpi_aware();
    let config = load_config();
    let lang = crate::lang(&config.general.language);
    // One window per session: a second launch brings the first one forward.
    let Some(_guard) = vixeeny_platform::single_instance("Vixeeny.Settings") else {
        vixeeny_platform::focus_tagged_window(SETTINGS_TAG);
        return Ok(());
    };
    let look = look_of(&config);
    if !config.general.first_run_done {
        return first_run(&config, lang);
    }
    let panel = SettingsPanel::new(
        config,
        vixeeny_platform::user_locale(),
        env!("CARGO_PKG_VERSION"),
        look,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    panel.capture_keys();
    panel.set_audio_source(audio_devices);
    panel.set_machine_source(machine);
    panel.set_display(vixeeny_settings::Display {
        hdr: vixeeny_platform::hdr_active(),
        max_refresh: vixeeny_platform::max_refresh_hz(),
    });
    panel.on_recording(pause_hotkeys);
    start_probe_watch(&panel);
    let handle = panel.handle();
    panel.on_meter(meter(handle.clone()));
    // The shortcuts the daemon could not register (it registers them again after each change).
    panel.watch_taken_shortcuts(|| {
        vixeeny_common::paths::hotkeys_taken_file()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    });

    let weak = panel.window().as_weak();
    // Saved once the changes stop for a moment: a slider sends one per step, and each save
    // makes the daemon reload. Closing the window saves what is left.
    let saving = vixeeny_ui::slint::Timer::default();
    let unsaved = Rc::new(RefCell::new(None::<Config>));
    let pending = Rc::clone(&unsaved);
    panel.on_change(move |config| {
        *pending.borrow_mut() = Some(config.clone());
        let pending = Rc::clone(&pending);
        saving.start(
            vixeeny_ui::slint::TimerMode::SingleShot,
            SAVE_DELAY,
            move || {
                if let Some(config) = pending.borrow_mut().take() {
                    save(&config);
                }
            },
        );
        let look = look_of(config);
        if let Some(w) = weak.upgrade() {
            vixeeny_ui::theme::apply(&w, look);
        }
    });
    panel.on_browse(|current| {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = vixeeny_common::paths::expand_user_dir(current) {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_folder().map(|p| p.display().to_string())
    });

    {
        let handle = handle.clone();
        let weak = panel.window().as_weak();
        panel.on_section(move |section| {
            let lang = weak
                .upgrade()
                .map_or(lang, |_| crate::lang(&load_config().general.language));
            match section {
                Section::About => {
                    handle.set_lines(about_lines(lang));
                    handle.set_extra(String::new());
                }
                Section::Updates => {
                    let checked = update_state().checked_at;
                    let due = UpdateState {
                        checked_at: checked,
                        ..UpdateState::default()
                    }
                    .due(now());
                    if due && client::ready().is_none() {
                        check_for_update(handle.clone(), lang);
                    } else {
                        handle.set_update(update_view(lang));
                    }
                }
                _ => {}
            }
        });
    }
    {
        let handle = handle;
        panel.on_page_action(move |section, action, value| {
            let lang = crate::lang(&load_config().general.language);
            match (section, action) {
                (Section::Updates, "update-check") => check_for_update(handle.clone(), lang),
                (Section::Updates, "update-download") => {
                    // A failed attempt is forgotten when the user tries again.
                    if let Some(path) = vixeeny_updater::state::file() {
                        let mut state = UpdateState::load(&path);
                        state.error = None;
                        let _ = state.save(&path);
                    }
                    download_update(handle.clone(), lang);
                }
                (Section::Updates, "update-restart") => restart_for_update(&handle, lang),
                (Section::About, "github") => {
                    let _ = vixeeny_platform::open_path(REPO);
                }
                (Section::About, "copy-info") => {
                    let handle = handle.clone();
                    // The hardware probe may need a few seconds the first time.
                    std::thread::spawn(move || {
                        let config = load_config();
                        let probe = crate::probe::current(false).ok();
                        let text = crate::sysinfo::format(&crate::sysinfo::collect(
                            &config,
                            probe.as_ref(),
                        ));
                        let message = match vixeeny_platform::clipboard::copy_text(&text) {
                            Ok(()) => tr(Key::AboutInfoCopied, lang).to_owned(),
                            Err(e) => e.to_string(),
                        };
                        handle.set_extra(message);
                    });
                }
                // Shipped beside the programs.
                (Section::About, "licenses") => {
                    let file = std::env::current_exe()
                        .ok()
                        .and_then(|exe| Some(exe.parent()?.join("THIRD-PARTY-LICENSES.txt")));
                    if let Some(file) = file.filter(|f| f.exists())
                        && let Err(e) = vixeeny_platform::open_path(&file.display().to_string())
                    {
                        tracing::error!("{e}");
                    }
                }
                (Section::About, "logs") => {
                    if let Some(dir) = vixeeny_common::paths::log_dir() {
                        let _ = vixeeny_platform::open_path(&dir.display().to_string());
                    }
                }
                (_, "open-folder") => {
                    if let Some(dir) = vixeeny_common::paths::expand_user_dir(value) {
                        let _ = std::fs::create_dir_all(&dir);
                        if let Err(e) = vixeeny_platform::open_path(&dir.display().to_string()) {
                            tracing::error!("{e}");
                        }
                    }
                }
                _ => {}
            }
        });
    }

    // The sidebar shows when an update waits for a restart.
    panel.set_update(&update_view(lang));
    panel.select_section(if open_updates {
        Section::Updates
    } else {
        Section::General
    });
    // A second launch finds the window by this mark and brings it forward.
    vixeeny_ui::theme::when_native(panel.window(), |handle| {
        if let Err(e) =
            vixeeny_platform::tag_window(vixeeny_platform::WindowId(handle), SETTINGS_TAG)
        {
            tracing::warn!("{e}");
        }
    });
    let result = panel.window().run().map_err(|e| anyhow::anyhow!("{e}"));
    if let Some(config) = unsaved.borrow_mut().take() {
        save(&config);
        // `save` tells the daemon from a thread: give it the moment it needs.
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    result
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Light or dark by the setting (or the system), with the accent colour of the system.
pub fn look_of(config: &Config) -> vixeeny_ui::theme::Look {
    let dark = vixeeny_overlay::dark_theme(
        &config.general.theme,
        vixeeny_platform::system_prefers_dark(),
    );
    vixeeny_ui::theme::Look::new(dark, vixeeny_platform::system_accent(), true)
}

/// The programs, microphones and outputs the audio page offers.
fn audio_devices() -> vixeeny_settings::AudioDevices {
    {
        use vixeeny_settings::{AudioDevices, AudioEntry};
        let devices = |list: Vec<vixeeny_audio::DeviceInfo>| {
            list.into_iter()
                .map(|d| AudioEntry {
                    id: d.id,
                    name: d.name,
                    icon: None,
                })
                .collect()
        };
        AudioDevices {
            outputs: devices(vixeeny_audio::list_outputs()),
            inputs: devices(vixeeny_audio::list_microphones()),
            programs: programs(vixeeny_audio::list_applications()),
        }
    }
}

/// Windows' own programs that have windows but nothing to record.
const SHELL_PROGRAMS: &[&str] = &[
    "applicationframehost.exe",
    "explorer.exe",
    "lockapp.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "systemsettings.exe",
    "textinputhost.exe",
];

/// The programs that can be recorded, once each (a browser has many processes): those with an
/// audio session, and those with a window (a browser closes its audio process while silent).
/// Named as their executable describes itself ("Spotify" rather than "Spotify.exe"), with
/// their icon; those playing first, then by name.
fn programs(apps: Vec<vixeeny_audio::AppInfo>) -> Vec<vixeeny_settings::AudioEntry> {
    let windowed = vixeeny_platform::top_level_windows()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|w| {
            let path = w.exe_path?;
            let exe = path.rsplit(['\\', '/']).next()?.to_owned();
            Some(vixeeny_audio::AppInfo {
                pid: w.pid,
                exe,
                active: false,
            })
        });
    let mut seen = std::collections::HashSet::new();
    let mut entries: Vec<(bool, vixeeny_settings::AudioEntry)> = apps
        .into_iter()
        .chain(windowed)
        .filter(|a| {
            let exe = a.exe.to_ascii_lowercase();
            !exe.is_empty()
                && !exe.starts_with("vixeeny")
                && !SHELL_PROGRAMS.contains(&exe.as_str())
        })
        .filter(|a| seen.insert(a.exe.to_ascii_lowercase()))
        .map(|a| {
            let path = vixeeny_platform::process_path(a.pid);
            let meta = path
                .as_deref()
                .map(vixeeny_platform::exe_metadata)
                .unwrap_or_default();
            let name = meta
                .file_description
                .or(meta.product_name)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| a.exe.trim_end_matches(".exe").to_owned());
            // Drawn at 22 px: 48 stays sharp up to 200 %.
            let icon = path
                .as_deref()
                .and_then(|p| vixeeny_platform::exe_icon(p, 48))
                .map(|(width, height, rgba)| vixeeny_settings::Icon {
                    width,
                    height,
                    rgba: rgba.into(),
                });
            let entry = vixeeny_settings::AudioEntry {
                id: a.exe,
                name: name.trim().to_owned(),
                icon,
            };
            (a.active, entry)
        })
        .collect();
    entries.sort_by(|(a_on, a), (b_on, b)| {
        b_on.cmp(a_on)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries.into_iter().map(|(_, e)| e).collect()
}

/// What this computer is made of, for the general page.
fn machine() -> vixeeny_settings::Machine {
    use vixeeny_settings::machine::{Cpu, Disk, Gpu, Screen};
    let m = vixeeny_platform::machine::machine();
    vixeeny_settings::Machine {
        cpu: m.cpu.map(|c| Cpu {
            name: c.name,
            cores: c.cores,
            threads: c.threads,
        }),
        ram_bytes: m.ram_bytes,
        gpus: m
            .gpus
            .into_iter()
            .map(|g| Gpu {
                name: g.name,
                integrated: g.integrated,
                memory_bytes: g.memory_bytes,
            })
            .collect(),
        disks: m
            .disks
            .into_iter()
            .map(|d| Disk {
                letter: d.letter,
                label: d.label,
                model: d.model,
                bytes: d.bytes,
                free_bytes: d.free_bytes,
            })
            .collect(),
        screens: m
            .screens
            .into_iter()
            .map(|s| Screen {
                number: s.number,
                primary: s.primary,
                name: s.name,
                width: s.width,
                height: s.height,
                hz: s.hz,
            })
            .collect(),
    }
}

/// Measures the microphone the audio page shows the level of: a capture of it while the page
/// is on screen, its loudest sample every 50 ms sent to the window.
fn meter(handle: PanelHandle) -> impl Fn(Option<String>) {
    use vixeeny_audio::{
        AudioChunk, AudioSink, AudioSource, SourceEvent, SourceSpec, WasapiSource,
    };
    struct Level {
        handle: PanelHandle,
        peak: f32,
        sent: std::time::Instant,
    }
    impl AudioSink for Level {
        fn on_audio(&mut self, chunk: AudioChunk) {
            let peak = chunk.samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
            self.peak = self.peak.max(peak);
            if self.sent.elapsed() >= std::time::Duration::from_millis(50) {
                // -60 dB to 0 dB across the bar: what the ear hears as even steps.
                let db = 20.0 * self.peak.max(1e-6).log10();
                self.handle
                    .set_mic_level(((db + 60.0) / 60.0).clamp(0.0, 1.0));
                self.peak = 0.0;
                self.sent = std::time::Instant::now();
            }
        }
        fn on_event(&mut self, _: SourceEvent) {
            self.handle.set_mic_level(0.0);
        }
    }
    let running: std::cell::RefCell<Option<WasapiSource>> = std::cell::RefCell::default();
    move |spec| {
        if let Some(mut old) = running.borrow_mut().take() {
            let _ = old.stop();
        }
        let Some(spec) = spec.as_deref().and_then(SourceSpec::parse) else {
            return;
        };
        let mut source = WasapiSource::new(spec.kind);
        let sink = Level {
            handle: handle.clone(),
            peak: 0.0,
            sent: std::time::Instant::now(),
        };
        match source.start(Box::new(sink)) {
            Ok(()) => *running.borrow_mut() = Some(source),
            Err(e) => tracing::warn!("cannot measure the microphone: {e}"),
        }
    }
}

/// Releases (or gives back) the global shortcuts of the daemon while a shortcut is recorded.
fn pause_hotkeys(paused: bool) {
    std::thread::spawn(move || {
        let _ = Endpoint::current_user().connect().and_then(|mut stream| {
            ipc::write_msg(
                &mut stream,
                &Hello::Control(ControlRequest::PauseHotkeys(paused)),
            )
            .map_err(std::io::Error::other)?;
            ipc::read_msg::<_, ipc::ControlReply>(&mut stream)
                .map(|_| ())
                .map_err(std::io::Error::other)
        });
    });
}

/// The hardware probe for the video page: the cache if it exists, else a background run whose
/// answer the page picks up when it is there.
fn start_probe_watch(panel: &SettingsPanel) {
    let state: Arc<Mutex<(Option<ProbeResult>, bool)>> =
        Arc::new(Mutex::new((crate::probe::cached(), false)));
    if let Ok(mut s) = state.lock()
        && s.0.is_none()
    {
        s.1 = true;
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            let result = crate::probe::current(false).ok();
            if let Ok(mut s) = shared.lock() {
                *s = (result, false);
            }
        });
    }
    panel.watch_probe(move || state.lock().map_or((None, false), |s| s.clone()));
}

/// The welcome assistant, before the very first use of the settings. Closing it counts as
/// skipping it: it is shown once.
fn first_run(config: &Config, lang: Lang) -> anyhow::Result<()> {
    use vixeeny_ui::wizard_panel::WizardPanel;

    let look = look_of(config);
    let browse = |current: &str| {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = vixeeny_common::paths::expand_user_dir(current) {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_folder().map(|p| p.display().to_string())
    };
    let panel = WizardPanel::new(
        config.clone(),
        vixeeny_platform::user_locale(),
        look,
        Box::new(browse),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let handle = panel.handle();
    std::thread::spawn(move || {
        let text = match crate::probe::current(false) {
            Ok(result) => hardware_lines(lang, &result)
                .iter()
                .map(|l| {
                    if l.detail.is_empty() {
                        l.text.clone()
                    } else {
                        format!("{}\n{}", l.text, l.detail)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            Err(e) => {
                tracing::warn!("hardware detection: {e:#}");
                tr(Key::HwUnavailable, lang).to_owned()
            }
        };
        handle.set_hardware(text);
    });
    let chosen = panel.run().map_err(|e| anyhow::anyhow!("{e}"))?;
    save(&chosen);
    // `save` tells the daemon from a thread: give it the moment it needs before the process ends.
    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok(())
}
