// SPDX-License-Identifier: GPL-3.0-or-later
//! `vixeeny-updater check` asks GitHub whether a newer release exists and records the answer;
//! `vixeeny-updater apply` downloads it, verifies it, stops the daemon, replaces the files and
//! starts the new version, going back to the old one if it does not come up (plan 10.2).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use vixeeny_common::ipc::{self, ControlReply, ControlRequest, Endpoint, Hello};
use vixeeny_updater::state::{self, State};
use vixeeny_updater::{Release, check_url, is_newer, swap, verify};

const REPOSITORY: &str = "https://github.com/Xantoom/Vixeeny";
const PLATFORM: &str = if cfg!(windows) {
    "windows-x64"
} else {
    "unsupported"
};
const DAEMON: &str = if cfg!(windows) {
    "vixeeny-daemon.exe"
} else {
    "vixeeny-daemon"
};
/// The new daemon must answer within this long, else the old version comes back.
const START_TIMEOUT: Duration = Duration::from_secs(20);

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(concat!("Vixeeny/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> anyhow::Result<Vec<u8>> {
    let mut body = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .into_body();
    let mut bytes = Vec::new();
    body.as_reader().take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn state_path() -> anyhow::Result<PathBuf> {
    state::file().context("no settings folder")
}

/// Records the latest release; prints `new <version>` the first time a version is seen, so the
/// daemon (which starts this program) can notify once.
fn check() -> anyhow::Result<()> {
    let path = state_path()?;
    let mut state = State::load(&path);
    let json = get(&agent(), &check_url(REPOSITORY), 4 << 20)?;
    let release = Release::parse(std::str::from_utf8(&json)?)?;
    state.checked_at = now();
    state.available = release.filter(|r| is_newer(env!("CARGO_PKG_VERSION"), &r.version));
    if let Some(r) = &state.available
        && state.notified.as_deref() != Some(&r.version)
    {
        println!("new {}", r.version);
        state.notified = Some(r.version.clone());
    }
    state.save(&path)?;
    Ok(())
}

fn control(request: ControlRequest) -> std::io::Result<()> {
    let mut stream = Endpoint::current_user().connect()?;
    ipc::write_msg(&mut stream, &Hello::Control(request)).map_err(std::io::Error::other)?;
    ipc::read_msg::<_, ControlReply>(&mut stream)
        .map(|_| ())
        .map_err(std::io::Error::other)
}

fn daemon_running() -> bool {
    control(ControlRequest::Ping).is_ok()
}

/// Asks the daemon to quit until it has (it refuses while a recording runs, so this waits for
/// the end of the recording).
fn stop_daemon() -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3 * 3600);
    while daemon_running() {
        let _ = control(ControlRequest::QuitForUpdate);
        std::thread::sleep(Duration::from_secs(2));
        if Instant::now() > deadline {
            bail!("the daemon did not stop");
        }
    }
    // Let the processes release their files.
    std::thread::sleep(Duration::from_millis(800));
    Ok(())
}

fn start_daemon(dir: &Path) -> std::io::Result<()> {
    std::process::Command::new(dir.join(DAEMON))
        .spawn()
        .map(|_| ())
}

fn apply() -> anyhow::Result<()> {
    let path = state_path()?;
    let state = State::load(&path);
    let release = state.available.context("no update is available")?;
    let files = release
        .files(PLATFORM)
        .context("the release has no file for this system")?;
    let agent = agent();
    let archive = get(&agent, &files.archive.url, 400 << 20)?;
    let signature = String::from_utf8(get(&agent, &files.signature.url, 1 << 16)?)?;
    let sums = String::from_utf8(get(&agent, &files.sums.url, 1 << 20)?)?;
    // Nothing is touched before both checks pass.
    verify::check_sum(&archive, &sums, &files.archive.name)?;
    verify::check_signature(&archive, &signature, verify::PUBLIC_KEY)?;

    let exe = std::env::current_exe()?;
    let install = exe.parent().context("no install folder")?.to_owned();
    let work = install.join(".update-staged");
    let backup = install.join(".update-backup");
    let _ = std::fs::remove_dir_all(&work);
    swap::extract(&archive, &work)?;

    stop_daemon()?;
    swap::install(&work, &install, &backup).context("cannot replace the files")?;
    let _ = std::fs::remove_dir_all(&work);
    start_daemon(&install)?;

    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if daemon_running() {
            swap::commit(&backup)?;
            State {
                available: None,
                ..State::load(&path)
            }
            .save(&path)?;
            return Ok(());
        }
    }
    eprintln!("the new version did not start: going back");
    swap::rollback(&install, &backup)?;
    start_daemon(&install)?;
    bail!("the new version did not start; the previous one was restored")
}

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("check") => check(),
        Some("apply") => apply(),
        _ => {
            println!(
                "vixeeny-updater {} (check | apply)",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("vixeeny-updater: {e:#}");
        std::process::exit(1);
    }
}
