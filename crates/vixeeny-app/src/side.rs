// SPDX-License-Identifier: GPL-3.0-or-later
//! The side overlay (plan 5.12), `overlay_toggle`: opens the strip on the edge of the monitor
//! under the mouse, waits for a pick, and gives it back as an action to run.

use anyhow::Context;
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::ActionId;
use vixeeny_overlay::dark_theme;
use vixeeny_overlay::side::{
    Choice, Edge, SidePanel, SideState, SideTexts, entries, panel_geometry,
};

/// What the user did in the overlay.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The action to run now.
    pub action: Option<ActionId>,
    /// The strip was pinned: it comes back once the action is done.
    pub pinned: bool,
}

/// The action behind a pick.
pub const fn action_of(choice: Choice) -> ActionId {
    match choice {
        Choice::Region => ActionId::CaptureRegion,
        Choice::Window => ActionId::CaptureWindow,
        Choice::Screen => ActionId::CaptureFullscreen,
        Choice::AllMonitors => ActionId::CaptureAllMonitors,
        Choice::Scrolling => ActionId::CaptureScrolling,
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
        video: tr(Key::OvlVideo, lang).into(),
        record: tr(Key::OvlRecord, lang).into(),
        stop_recording: tr(Key::OvlStopRecording, lang).into(),
        replay_start: tr(Key::OvlReplayStart, lang).into(),
        replay_stop: tr(Key::OvlReplayStop, lang).into(),
        replay_save: tr(Key::OvlReplaySave, lang).into(),
        settings: tr(Key::OvlSettings, lang).into(),
    }
}

/// Shows the overlay until the user picks, dismisses it, or clicks elsewhere.
pub fn run(
    config: &Config,
    recording: bool,
    replay: bool,
    pinned: bool,
) -> anyhow::Result<Outcome> {
    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors()?;
    let (x, y) = vixeeny_platform::cursor_position()?;
    let monitor = vixeeny_platform::monitor_at(&monitors, x, y)
        .or_else(|| monitors.first())
        .context("no monitor")?
        .clone();

    let lang = crate::lang(&config.general.language);
    let state = SideState {
        recording,
        replay,
        dark: dark_theme(
            &config.general.theme,
            vixeeny_platform::system_prefers_dark(),
        ),
        edge: Edge::from_setting(&config.overlay.edge),
        animate: vixeeny_platform::animations_enabled(),
        pinned,
    };
    let texts = texts(lang);
    let geometry = panel_geometry(
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
    let panel = SidePanel::new(&texts, &state, geometry, monitor.dpi)?;

    // A click elsewhere closes it. Losing the focus says so only when Windows let the strip
    // take it, which it does not always do for a window opened by a shortcut: the clicks are
    // watched directly. The strip closes on the next turn of its loop, not inside the hook.
    let dismiss = panel.dismisser();
    let mut outside = None;
    let choice = panel.run_with(|handle| {
        outside = vixeeny_platform::on_click_outside(
            vixeeny_platform::WindowId(handle),
            Box::new(dismiss),
        );
    })?;
    drop(outside);
    Ok(Outcome {
        action: choice.map(action_of),
        pinned: panel.pinned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pick_runs_its_action() {
        assert_eq!(action_of(Choice::Region), ActionId::CaptureRegion);
        assert_eq!(action_of(Choice::Screen), ActionId::CaptureFullscreen);
        assert_eq!(action_of(Choice::ReplaySave), ActionId::ReplaySave);
        assert_eq!(action_of(Choice::Settings), ActionId::OpenSettings);
    }
}
