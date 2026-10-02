// SPDX-License-Identifier: GPL-3.0-or-later
//! The side overlay (plan 5.12), `overlay_toggle`: opens the strip on the edge of the monitor
//! under the mouse, waits for a pick, and gives it back as an action to run.

use anyhow::Context;
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::ActionId;
use vixeeny_ui::side_panel::{
    Choice, Edge, SidePanel, SideState, SideTexts, dark_theme, entries, panel_geometry,
};

/// What the user did in the overlay.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The action to run now.
    pub action: Option<ActionId>,
    /// A new video profile, to save to the settings.
    pub profile: Option<String>,
}

/// The action behind a pick.
pub const fn action_of(choice: Choice) -> ActionId {
    match choice {
        Choice::Region => ActionId::CaptureRegion,
        Choice::Window => ActionId::CaptureWindow,
        Choice::Screen => ActionId::CaptureFullscreen,
        Choice::AllMonitors => ActionId::CaptureAllMonitors,
        Choice::Scrolling => ActionId::CaptureScrolling,
        Choice::Ocr => ActionId::OcrRegion,
        Choice::RecordToggle => ActionId::RecordToggle,
        Choice::ReplayToggle => ActionId::ReplayToggle,
        Choice::ReplaySave => ActionId::ReplaySave,
        Choice::Settings => ActionId::OpenSettings,
    }
}

fn texts(lang: Lang) -> SideTexts {
    SideTexts {
        image: tr(Key::OvlImage, lang).into(),
        region: tr(Key::OvlRegion, lang).into(),
        window: tr(Key::OvlWindow, lang).into(),
        screen: tr(Key::OvlScreen, lang).into(),
        all_monitors: tr(Key::OvlAllMonitors, lang).into(),
        scrolling: tr(Key::OvlScrolling, lang).into(),
        ocr: tr(Key::OvlOcr, lang).into(),
        video: tr(Key::OvlVideo, lang).into(),
        record: tr(Key::OvlRecord, lang).into(),
        stop_recording: tr(Key::OvlStopRecording, lang).into(),
        replay_start: tr(Key::OvlReplayStart, lang).into(),
        replay_stop: tr(Key::OvlReplayStop, lang).into(),
        replay_save: tr(Key::OvlReplaySave, lang).into(),
        profile: tr(Key::OvlProfile, lang).into(),
        settings: tr(Key::OvlSettings, lang).into(),
    }
}

/// Shows the overlay until the user picks, dismisses it, or clicks elsewhere.
#[cfg(windows)]
pub fn run(config: &Config, recording: bool, replay: bool) -> anyhow::Result<Outcome> {
    use std::cell::RefCell;
    use std::rc::Rc;

    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors()?;
    let (x, y) = vixeeny_platform::cursor_position()?;
    let monitor = vixeeny_platform::monitor_at(&monitors, x, y)
        .or_else(|| monitors.first())
        .context("no monitor")?
        .clone();

    let lang = crate::lang(&config.general.language);
    let profiles: Vec<String> = config.profiles.keys().cloned().collect();
    let current = profiles
        .iter()
        .position(|p| *p == config.video.profile)
        .unwrap_or(0);
    let state = SideState {
        recording,
        replay,
        profiles: profiles.clone(),
        profile: current,
        dark: dark_theme(
            &config.general.theme,
            vixeeny_platform::system_prefers_dark(),
        ),
        edge: Edge::from_setting(&config.overlay.edge),
        animate: vixeeny_platform::animations_enabled(),
        backdrop: false,
    };
    let texts = texts(lang);
    let panel = SidePanel::new(&texts, &state).map_err(|e| anyhow::anyhow!("{e}"))?;
    let (gx, gy, gw, gh) = panel_geometry(
        state.edge,
        (
            monitor.rect.x,
            monitor.rect.y,
            monitor.rect.width,
            monitor.rect.height,
        ),
        monitor.dpi,
        &entries(&texts, &state),
    );
    panel.set_geometry(gx, gy, gw, gh);
    let picked = Rc::new(RefCell::new(None));
    let sink = picked.clone();
    let names = profiles.clone();
    panel.on_profile(move |i| *sink.borrow_mut() = names.get(i).cloned());

    // The blurred Windows 11 backdrop needs the native window, which exists once it is shown:
    // the strip is shown first, then dressed (a failure just leaves the opaque fill).
    let choice = panel
        .run_with(|handle| {
            if let Err(e) = vixeeny_platform::apply_acrylic(vixeeny_platform::WindowId(handle)) {
                tracing::debug!("no acrylic backdrop: {e}");
                return false;
            }
            true
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let profile = picked
        .borrow()
        .clone()
        .filter(|p| *p != config.video.profile);
    Ok(Outcome {
        action: choice.map(action_of),
        profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pick_runs_its_action() {
        assert_eq!(action_of(Choice::Region), ActionId::CaptureRegion);
        assert_eq!(action_of(Choice::Screen), ActionId::CaptureFullscreen);
        assert_eq!(action_of(Choice::Ocr), ActionId::OcrRegion);
        assert_eq!(action_of(Choice::ReplaySave), ActionId::ReplaySave);
        assert_eq!(action_of(Choice::Settings), ActionId::OpenSettings);
    }
}
