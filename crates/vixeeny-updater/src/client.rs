// SPDX-License-Identifier: GPL-3.0-or-later
//! The update itself, run by the app (a windowed program: no console ever shows): ask GitHub
//! for the latest release, download it with its progress, verify it, then swap the files and
//! restart Vixeeny. Windows lets a running program be renamed, so the files are replaced while
//! everything runs; only the daemon is restarted.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use vixeeny_common::ipc::{self, ControlReply, ControlRequest, Endpoint, Hello};

use crate::state::{self, State, log};
use crate::{Release, check_url, is_newer, swap, verify};

const REPOSITORY: &str = "https://github.com/Xantoom/Vixeeny";
const PLATFORM: &str = "windows-x64";
/// The program that runs in the background (the tray, the shortcuts).
pub const DAEMON: &str = "Vixeeny.exe";
/// Where the next version waits, and where the replaced files go, inside the install folder
/// (the same volume: moving a file is a rename, which works on a running program).
const STAGED: &str = ".update-staged";
const BACKUP: &str = ".update-backup";
/// The new daemon must answer within this long, else the old version comes back.
const START_TIMEOUT: Duration = Duration::from_secs(20);
const CURRENT: &str = env!("CARGO_PKG_VERSION");

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .user_agent(concat!("Vixeeny/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Downloads `url` (at most `limit` bytes); `progress(done, total)` follows it.
fn get(
    agent: &ureq::Agent,
    url: &str,
    limit: u64,
    expected: u64,
    progress: &mut dyn FnMut(u64, u64),
) -> anyhow::Result<Vec<u8>> {
    let response = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(expected);
    let mut reader = response.into_body().into_reader().take(limit);
    let mut bytes = Vec::with_capacity(usize::try_from(total).unwrap_or(0).min(512 << 20));
    let mut chunk = vec![0; 256 * 1024];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        progress(bytes.len() as u64, total.max(bytes.len() as u64));
    }
    Ok(bytes)
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error.chain().any(|e| {
        matches!(
            e.downcast_ref::<ureq::Error>(),
            Some(ureq::Error::StatusCode(404))
        )
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn state_path() -> anyhow::Result<PathBuf> {
    state::file().context("no settings folder")
}

fn install_dir() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe.parent().context("no install folder")?.to_owned())
}

/// Asks GitHub for the latest release and records the answer. `Some` when it is newer than
/// this version.
pub fn check() -> anyhow::Result<Option<Release>> {
    let path = state_path()?;
    // 404: nothing is published yet (drafts are invisible to the API).
    let mut nothing = |_, _| {};
    let release = match get(&agent(), &check_url(REPOSITORY), 4 << 20, 0, &mut nothing) {
        Ok(json) => Release::parse(std::str::from_utf8(&json)?)?,
        Err(e) if is_not_found(&e) => None,
        Err(e) => return Err(e),
    };
    let mut state = State::load(&path);
    state.checked_at = now();
    state.available = release.filter(|r| is_newer(CURRENT, &r.version));
    state.save(&path)?;
    Ok(state.available)
}

/// The version that is downloaded, verified and ready to be installed, if any.
pub fn ready() -> Option<String> {
    let state = State::load(&state::file()?);
    let version = state.ready?;
    let staged = install_dir().ok()?.join(STAGED);
    (is_newer(CURRENT, &version) && staged.join(DAEMON).exists()).then_some(version)
}

/// Downloads the available version, checks its signature and unpacks it next to the programs.
/// `progress(done, total)` is called as the bytes arrive. Returns the version.
pub fn download(progress: &mut dyn FnMut(u64, u64)) -> anyhow::Result<String> {
    let path = state_path()?;
    let mut state = State::load(&path);
    let release = state
        .newer_than(CURRENT)
        .cloned()
        .context("no update is available")?;
    if ready().as_deref() == Some(release.version.as_str()) {
        return Ok(release.version);
    }
    anyhow::ensure!(verify::has_key(), "this build cannot verify updates");
    let files = release
        .files(PLATFORM)
        .context("the release has no file for this system")?;
    let agent = agent();
    log(&format!("downloading {}", release.version));
    let archive = get(
        &agent,
        &files.archive.url,
        600 << 20,
        files.archive.size,
        progress,
    )?;
    let mut nothing = |_, _| {};
    let signature = get(&agent, &files.signature.url, 1 << 16, 0, &mut nothing)?;
    let sums = get(&agent, &files.sums.url, 1 << 20, 0, &mut nothing)?;
    // Nothing is unpacked before both checks pass.
    verify::check_sum(&archive, &String::from_utf8(sums)?, &files.archive.name)?;
    verify::check_signature(&archive, &String::from_utf8(signature)?, verify::PUBLIC_KEY)?;
    let staged = install_dir()?.join(STAGED);
    let _ = std::fs::remove_dir_all(&staged);
    swap::extract(&archive, &staged)?;
    anyhow::ensure!(
        staged.join(DAEMON).exists(),
        "the archive does not have the expected layout"
    );
    state.ready = Some(release.version.clone());
    state.error = None;
    state.save(&path)?;
    log(&format!("{} is ready", release.version));
    Ok(release.version)
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

/// Asks the daemon to quit until it has (it refuses while a recording runs: the update waits
/// for the end of the recording).
fn stop_daemon() -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3 * 3600);
    while daemon_running() {
        let _ = control(ControlRequest::QuitForUpdate);
        std::thread::sleep(Duration::from_millis(400));
        if Instant::now() > deadline {
            bail!("Vixeeny did not stop");
        }
    }
    Ok(())
}

fn start_daemon(dir: &Path) -> std::io::Result<()> {
    std::process::Command::new(dir.join(DAEMON))
        .spawn()
        .map(|_| ())
}

/// Installs the downloaded version and restarts Vixeeny with it; the previous version comes
/// back if the new one does not start. `reopen` is sent to the new daemon once it runs (to
/// show the settings again, for instance).
pub fn install(reopen: Option<ControlRequest>) -> anyhow::Result<()> {
    let path = state_path()?;
    log(&format!("-- install (from {CURRENT})"));
    let result = install_inner(reopen);
    let mut state = State::load(&path);
    match &result {
        Ok(()) => {
            log("update done");
            state = State {
                checked_at: state.checked_at,
                ..State::default()
            };
        }
        Err(e) => {
            log(&format!("update failed: {e:#}"));
            state.error = Some(format!("{e:#}"));
            state.ready = None;
        }
    }
    let _ = state.save(&path);
    result
}

fn install_inner(reopen: Option<ControlRequest>) -> anyhow::Result<()> {
    let version = ready().context("no update is ready")?;
    let dir = install_dir()?;
    let (staged, backup) = (dir.join(STAGED), dir.join(BACKUP));
    // A backup left by an earlier update (its files were still in use then).
    let _ = std::fs::remove_dir_all(&backup);
    stop_daemon()?;
    swap::install(&staged, &dir, &backup).context("cannot replace the files")?;
    let _ = std::fs::remove_dir_all(&staged);
    log(&format!("starting {version}"));
    start_daemon(&dir)?;
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        if daemon_running() {
            // The old programs that still run (this one included) keep the backup busy: what
            // cannot go now goes at the next start (`clean_up`).
            let _ = swap::commit(&backup);
            if let Some(request) = reopen {
                let _ = control(request);
            }
            return Ok(());
        }
    }
    swap::rollback(&dir, &backup)?;
    start_daemon(&dir)?;
    bail!("the new version did not start; the previous one was restored")
}

/// Removes what an update left behind (the old files, still in use while it ran, and the
/// console updater of the versions before 1.0).
pub fn clean_up() {
    if let Ok(dir) = install_dir() {
        let backup = dir.join(BACKUP);
        if backup.exists() {
            let _ = swap::commit(&backup);
        }
        let _ = std::fs::remove_file(dir.join("vixeeny-updater.exe"));
    }
}
