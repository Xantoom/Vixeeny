// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings window and gallery (plan 5.13), a process of its own (`vixeeny-app --settings`)
//! so the recording hotkeys keep working while it is open. Every change is saved at once and the
//! daemon is asked to reload.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
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
use vixeeny_ui::settings_panel::{GalleryEntry, GalleryRequest, Line, PanelHandle, SettingsPanel};

use crate::gallery::{self, Item, Kind};

const REPO: &str = "https://github.com/Xantoom/Vixeeny";
const THUMB_SIDE: u32 = 256;
/// How many captures the gallery shows.
const GALLERY_LIMIT: usize = 80;

/// Starts the settings window in its own process.
pub fn spawn() -> anyhow::Result<()> {
    std::process::Command::new(std::env::current_exe()?)
        .arg("--settings")
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

fn user_dirs(config: &Config) -> Vec<PathBuf> {
    [
        &config.paths.images,
        &config.paths.videos,
        &config.paths.replays,
    ]
    .into_iter()
    .filter_map(|template| vixeeny_common::paths::expand_user_dir(template))
    .collect()
}

#[derive(Default)]
struct Gallery {
    items: Vec<Item>,
    /// Indexes into `items` that pass the filters.
    shown: Vec<usize>,
    /// Position in `shown`.
    selected: Option<usize>,
    kind: i32,
    app: String,
    thumbs: HashMap<PathBuf, (u32, u32, Vec<u8>)>,
}

type SharedGallery = Arc<Mutex<Gallery>>;

fn entries(g: &Gallery, offset: i64) -> Vec<GalleryEntry> {
    g.shown
        .iter()
        .filter_map(|i| g.items.get(*i))
        .map(|item| GalleryEntry {
            name: item.name.clone(),
            detail: gallery::detail(item, offset),
            video: item.kind == Kind::Video,
            thumb: g.thumbs.get(&item.path).cloned(),
        })
        .collect()
}

/// Shows the gallery now, then fills in the thumbnails that are missing on a thread.
fn show_gallery(shared: &SharedGallery, handle: &PanelHandle, offset: i64) {
    let (shown, selected, missing) = {
        let Ok(g) = shared.lock() else { return };
        let missing: Vec<PathBuf> = g
            .shown
            .iter()
            .filter_map(|i| g.items.get(*i))
            .filter(|i| i.kind == Kind::Image && !g.thumbs.contains_key(&i.path))
            .map(|i| i.path.clone())
            .collect();
        (entries(&g, offset), g.selected, missing)
    };
    handle.set_gallery(shown, selected);
    if missing.is_empty() {
        return;
    }
    let (shared, handle) = (Arc::clone(shared), handle.clone());
    std::thread::spawn(move || {
        for (n, path) in missing.iter().enumerate() {
            let thumb = gallery::thumbnail(path, THUMB_SIDE);
            let Ok(mut g) = shared.lock() else { return };
            if let Some(thumb) = thumb {
                g.thumbs.insert(path.clone(), thumb);
            }
            // The tiles are redrawn every few thumbnails, not for each one.
            if n % 6 == 5 || n + 1 == missing.len() {
                handle.set_gallery(entries(&g, offset), g.selected);
            }
        }
    });
}

fn refilter(g: &mut Gallery) {
    g.shown = gallery::filter(&g.items, g.kind, &g.app);
    g.selected = None;
}

fn selected_item(shared: &SharedGallery) -> Option<Item> {
    let g = shared.lock().ok()?;
    let position = g.selected?;
    g.items.get(*g.shown.get(position)?).cloned()
}

fn copy_to_clipboard(path: &std::path::Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    let decoded = vixeeny_image::decode(&bytes)?;
    let png = vixeeny_image::encode(
        vixeeny_image::ImageFormat::Png,
        &decoded.as_bgra(),
        &vixeeny_image::Settings::default(),
    )?;
    vixeeny_platform::clipboard::copy_image(decoded.width, decoded.height, &decoded.bgra, &png)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn gallery_request(
    request: GalleryRequest,
    shared: &SharedGallery,
    handle: &PanelHandle,
    offset: i64,
) {
    match request {
        GalleryRequest::Select(position) => {
            if let Ok(mut g) = shared.lock() {
                g.selected = (position < g.shown.len()).then_some(position);
            }
            show_gallery(shared, handle, offset);
        }
        GalleryRequest::Filter(kind, app) => {
            if let Ok(mut g) = shared.lock() {
                g.kind = kind;
                g.app = app;
                refilter(&mut g);
            }
            show_gallery(shared, handle, offset);
        }
        GalleryRequest::Action(action) => {
            let Some(item) = selected_item(shared) else {
                return;
            };
            let path = item.path.display().to_string();
            let result = match action.as_str() {
                "open" => vixeeny_platform::open_path(&path).map_err(|e| anyhow::anyhow!("{e}")),
                "folder" => std::process::Command::new("explorer.exe")
                    .arg(format!("/select,{path}"))
                    .spawn()
                    .map(|_| ())
                    .map_err(Into::into),
                "copy" => copy_to_clipboard(&item.path),
                "convert" => {
                    std::process::Command::new(std::env::current_exe().unwrap_or_default())
                        .args(["--convert", &path])
                        .spawn()
                        .map(|_| ())
                        .map_err(Into::into)
                }
                "delete" => {
                    let done = vixeeny_platform::recycle(&path).map_err(|e| anyhow::anyhow!("{e}"));
                    if done.is_ok()
                        && let Ok(mut g) = shared.lock()
                    {
                        g.items.retain(|i| i.path != item.path);
                        refilter(&mut g);
                    }
                    show_gallery(shared, handle, offset);
                    done
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                tracing::error!("gallery {action}: {e:#}");
            }
        }
    }
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
            strong: true,
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
            strong: true,
        });
    }
    lines
}

/// Runs the detection (a child process, up to several seconds) and shows the result.
fn detect_hardware(handle: PanelHandle, lang: Lang, force: bool) {
    handle.set_extra(tr(Key::HwDetecting, lang).into());
    std::thread::spawn(move || match crate::probe::current(force) {
        Ok(result) => {
            handle.set_lines(hardware_lines(lang, &result));
            handle.set_extra(String::new());
        }
        Err(e) => {
            tracing::warn!("hardware detection: {e:#}");
            handle.set_lines(Vec::new());
            handle.set_extra(tr(Key::HwUnavailable, lang).into());
        }
    });
}

fn updater_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join("vixeeny-updater.exe");
    path.exists().then_some(path)
}

/// What the Updates page says: the running version and what the last check found.
fn update_text(lang: Lang) -> String {
    let current = env!("CARGO_PKG_VERSION");
    let state = vixeeny_updater::state::file()
        .map(|p| vixeeny_updater::state::State::load(&p))
        .unwrap_or_default();
    match state.newer_than(current) {
        Some(release) => format!(
            "{}\n\n{}",
            tr(Key::UpdateFound, lang).replace("{version}", &release.version),
            release.notes.trim()
        ),
        None => tr(Key::UpdateUpToDate, lang).replace("{version}", current),
    }
}

fn check_for_update(handle: PanelHandle, lang: Lang) {
    let Some(updater) = updater_exe() else {
        handle.set_extra(tr(Key::UpdateCheckFailed, lang).into());
        return;
    };
    handle.set_extra(tr(Key::UpdateChecking, lang).into());
    std::thread::spawn(move || {
        let done = std::process::Command::new(updater)
            .arg("check")
            .output()
            .is_ok_and(|o| o.status.success());
        handle.set_extra(if done {
            update_text(lang)
        } else {
            tr(Key::UpdateCheckFailed, lang).into()
        });
    });
}

fn start_update(handle: &PanelHandle, lang: Lang) {
    let current = env!("CARGO_PKG_VERSION");
    let available = vixeeny_updater::state::file()
        .map(|p| vixeeny_updater::state::State::load(&p))
        .is_some_and(|s| s.newer_than(current).is_some());
    if !available {
        handle.set_extra(update_text(lang));
        return;
    }
    if !vixeeny_updater::verify::has_key() {
        handle.set_extra(tr(Key::UpdateNoKey, lang).into());
        return;
    }
    match updater_exe().map(|u| std::process::Command::new(u).arg("apply").spawn()) {
        Some(Ok(_)) => handle.set_extra(tr(Key::UpdateInstalling, lang).into()),
        _ => handle.set_extra(tr(Key::UpdateCheckFailed, lang).into()),
    }
}

/// `--install-menu` / `--uninstall-menu`, run by the installer.
pub fn context_menu(install: bool) -> anyhow::Result<()> {
    let lang = crate::lang(&load_config().general.language);
    if install {
        let exe = std::env::current_exe()?;
        vixeeny_platform::context_menu::install(&exe, tr(Key::ConvMenuLabel, lang))
            .map_err(|e| anyhow::anyhow!("{e}"))
    } else {
        vixeeny_platform::context_menu::uninstall().map_err(|e| anyhow::anyhow!("{e}"))
    }
}

fn integration_lines(lang: Lang) -> Vec<Line> {
    let installed = vixeeny_platform::context_menu::is_installed();
    vec![Line {
        text: tr(
            if installed {
                Key::IntegInstalled
            } else {
                Key::IntegNotInstalled
            },
            lang,
        )
        .into(),
        detail: String::new(),
        strong: installed,
    }]
}

fn about_lines(lang: Lang) -> Vec<Line> {
    let logs =
        vixeeny_common::paths::log_dir().map_or_else(String::new, |p| p.display().to_string());
    vec![
        Line {
            text: format!("Vixeeny {}", env!("CARGO_PKG_VERSION")),
            detail: REPO.into(),
            strong: true,
        },
        Line {
            text: tr(Key::AboutLicense, lang).into(),
            ..Line::default()
        },
        Line {
            text: tr(Key::AboutThirdParty, lang).into(),
            ..Line::default()
        },
        Line {
            text: tr(Key::AboutLogs, lang).replace("{path}", &logs),
            ..Line::default()
        },
    ]
}

/// `--settings`: the window, until it is closed.
pub fn run_child() -> anyhow::Result<()> {
    vixeeny_platform::ensure_dpi_aware();
    let config = load_config();
    let lang = crate::lang(&config.general.language);
    // One window per session: a second launch brings the first one forward.
    let Some(_guard) = vixeeny_platform::single_instance("Vixeeny.Settings") else {
        vixeeny_platform::focus_window_titled(tr(Key::SettingsTitle, lang));
        return Ok(());
    };
    let dark = vixeeny_ui::side_panel::dark_theme(
        &config.general.theme,
        vixeeny_platform::system_prefers_dark(),
    );
    if !config.general.first_run_done {
        return first_run(&config, lang);
    }
    let panel = SettingsPanel::new(
        config.clone(),
        vixeeny_platform::user_locale(),
        env!("CARGO_PKG_VERSION"),
        dark,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let handle = panel.handle();
    let offset = gallery::local_offset_secs();

    let weak = panel.window().as_weak();
    panel.on_change(move |config| {
        save(config);
        let dark = vixeeny_ui::side_panel::dark_theme(
            &config.general.theme,
            vixeeny_platform::system_prefers_dark(),
        );
        if let Some(w) = weak.upgrade() {
            w.set_dark(dark);
        }
    });
    panel.on_browse(|current| {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = vixeeny_common::paths::expand_user_dir(current) {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_folder().map(|p| p.display().to_string())
    });

    let gallery_state: SharedGallery = Arc::default();
    {
        let (shared, handle) = (Arc::clone(&gallery_state), handle.clone());
        panel.on_gallery(move |request| gallery_request(request, &shared, &handle, offset));
    }
    {
        let (shared, handle) = (Arc::clone(&gallery_state), handle.clone());
        let config_now = Rc::new(RefCell::new(config));
        let weak = panel.window().as_weak();
        panel.on_section(move |section| {
            let lang = weak
                .upgrade()
                .map_or(lang, |_| crate::lang(&load_config().general.language));
            match section {
                Section::Gallery => {
                    *config_now.borrow_mut() = load_config();
                    let items = gallery::scan(&user_dirs(&config_now.borrow()), GALLERY_LIMIT);
                    if let Ok(mut g) = shared.lock() {
                        g.items = items;
                        refilter(&mut g);
                    }
                    show_gallery(&shared, &handle, offset);
                }
                Section::Hardware => detect_hardware(handle.clone(), lang, false),
                Section::Integration => {
                    handle.set_lines(integration_lines(lang));
                    handle.set_extra(String::new());
                }
                Section::Updates => handle.set_extra(update_text(lang)),
                Section::About => {
                    handle.set_lines(about_lines(lang));
                    handle.set_extra(String::new());
                }
                _ => {}
            }
        });
    }
    {
        let handle = handle.clone();
        panel.on_page_action(move |section, action, _| {
            let lang = crate::lang(&load_config().general.language);
            match (section, action) {
                (Section::Hardware, "redetect") => detect_hardware(handle.clone(), lang, true),
                (Section::Integration, "install" | "remove") => {
                    let result = if action == "install" {
                        let label = tr(Key::ConvMenuLabel, lang);
                        std::env::current_exe()
                            .map_err(|e| anyhow::anyhow!("{e}"))
                            .and_then(|exe| {
                                vixeeny_platform::context_menu::install(&exe, label)
                                    .map_err(|e| anyhow::anyhow!("{e}"))
                            })
                    } else {
                        vixeeny_platform::context_menu::uninstall()
                            .map_err(|e| anyhow::anyhow!("{e}"))
                    };
                    if let Err(e) = result {
                        tracing::error!("context menu: {e:#}");
                    }
                    handle.set_lines(integration_lines(lang));
                }
                (Section::Updates, "check") => check_for_update(handle.clone(), lang),
                (Section::Updates, "update") => start_update(&handle, lang),
                (Section::About, "github") => {
                    let _ = vixeeny_platform::open_path(REPO);
                }
                (Section::About, "logs") => {
                    if let Some(dir) = vixeeny_common::paths::log_dir() {
                        let _ = vixeeny_platform::open_path(&dir.display().to_string());
                    }
                }
                _ => {}
            }
        });
    }

    panel.select_section(Section::General);
    panel.window().run().map_err(|e| anyhow::anyhow!("{e}"))
}

/// The welcome assistant, before the very first use of the settings. Closing it counts as
/// skipping it: it is shown once.
fn first_run(config: &Config, lang: Lang) -> anyhow::Result<()> {
    use vixeeny_ui::wizard_panel::WizardPanel;

    let dark = vixeeny_ui::side_panel::dark_theme(
        &config.general.theme,
        vixeeny_platform::system_prefers_dark(),
    );
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
        dark,
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
