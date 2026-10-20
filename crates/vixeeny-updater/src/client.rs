// SPDX-License-Identifier: GPL-3.0-or-later
//! The update itself, run by the app (a windowed program: no console ever shows): ask GitHub
//! for the latest release, download it with its progress, verify it, then swap the files and
//! restart Vixeeny. Windows lets a running program be renamed, so the files are replaced while
//! everything runs; only the daemon is restarted.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
/// Where the archive arrives: a download cut short (sleep, no network) goes on from there.
const DOWNLOAD: &str = ".update-download";
/// Held while the files are swapped (see [`lock`]).
const LOCK: &str = ".update-lock";
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

/// Where a download goes: memory, or a file a later attempt goes on with.
trait Sink {
    fn len(&self) -> u64;
    /// The server sends the whole file again.
    fn restart(&mut self) -> std::io::Result<()>;
    fn add(&mut self, bytes: &[u8]) -> std::io::Result<()>;
}

impl Sink for Vec<u8> {
    fn len(&self) -> u64 {
        Vec::len(self) as u64
    }

    fn restart(&mut self) -> std::io::Result<()> {
        self.clear();
        Ok(())
    }

    fn add(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

/// A file being downloaded, kept from one attempt to the next.
struct Part {
    file: std::fs::File,
    len: u64,
}

impl Part {
    fn open(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

impl Sink for Part {
    fn len(&self) -> u64 {
        self.len
    }

    fn restart(&mut self) -> std::io::Result<()> {
        self.file.set_len(0)?;
        self.len = 0;
        Ok(())
    }

    fn add(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(&mut self.file, bytes)?;
        self.len += bytes.len() as u64;
        Ok(())
    }
}

/// Downloads `url` (at most `limit` bytes, `expected` when known) into `sink`, going on from
/// what it already holds; `progress(done, total)` follows it. Each request has [`agent`]'s
/// minute: a slow connection that runs out of it goes on where it stopped (an HTTP range), and
/// only a request that got nothing at all fails.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    sink: &mut dyn Sink,
    limit: u64,
    expected: u64,
    progress: &mut dyn FnMut(u64, u64),
) -> anyhow::Result<()> {
    if expected > 0 && sink.len() > expected {
        sink.restart()?;
    }
    if expected > 0 && sink.len() == expected {
        return Ok(()); // all there already
    }
    let mut total = expected;
    let mut chunk = vec![0; 256 * 1024];
    loop {
        let mut request = agent.get(url);
        if sink.len() > 0 {
            request = request.header("Range", format!("bytes={}-", sink.len()));
        }
        let response = request.call().with_context(|| format!("GET {url}"))?;
        if sink.len() > 0 && response.status().as_u16() != 206 {
            sink.restart()?;
        }
        let length: Option<u64> = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        if let Some(length) = length {
            total = sink.len() + length;
        }
        anyhow::ensure!(total <= limit, "{url} is larger than expected");
        let mut reader = response.into_body().into_reader();
        let before = sink.len();
        let ended = loop {
            match reader.read(&mut chunk) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    sink.add(&chunk[..n])?;
                    anyhow::ensure!(sink.len() <= limit, "{url} is larger than expected");
                    progress(sink.len(), total.max(sink.len()));
                }
                Err(e) => break Err(e),
            }
        };
        match ended {
            Ok(()) => return Ok(()),
            // Cut off after some progress: the rest in a new request.
            Err(_) if sink.len() > before && sink.len() < total => {}
            Err(e) => return Err(e).with_context(|| format!("GET {url}")),
        }
    }
}

/// Downloads a small file (at most `limit` bytes) into memory.
fn get(agent: &ureq::Agent, url: &str, limit: u64) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fetch(agent, url, &mut bytes, limit, 0, &mut |_, _| {})?;
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
    let release = match get(&agent(), &check_url(REPOSITORY), 4 << 20) {
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
    let folder = install_dir()?.join(DOWNLOAD);
    std::fs::create_dir_all(&folder)?;
    let part = folder.join(&files.archive.name);
    // What another version left.
    for entry in std::fs::read_dir(&folder)?.flatten() {
        if entry.path() != part {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    fetch(
        &agent,
        &files.archive.url,
        &mut Part::open(&part)?,
        600 << 20,
        files.archive.size,
        progress,
    )?;
    let signature = String::from_utf8(get(&agent, &files.signature.url, 1 << 16)?)?;
    let sums = match files.sums {
        Some(sums) => Some(String::from_utf8(get(&agent, &sums.url, 1 << 20)?)?),
        None => None,
    };
    // Nothing is unpacked before the checks pass; a file that fails them is downloaded anew.
    let checked = verify::check(
        std::io::BufReader::new(std::fs::File::open(&part)?),
        &files.archive.name,
        &signature,
        sums.as_deref(),
        verify::PUBLIC_KEY,
    );
    if let Err(e) = checked {
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    let staged = install_dir()?.join(STAGED);
    let _ = std::fs::remove_dir_all(&staged);
    swap::extract(
        std::io::BufReader::new(std::fs::File::open(&part)?),
        &staged,
    )?;
    anyhow::ensure!(
        staged.join(DAEMON).exists(),
        "the archive does not have the expected layout"
    );
    let _ = std::fs::remove_dir_all(&folder);
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

/// Asks the daemon to quit, then waits until its process is gone: at once, or at the end of a
/// recording (the update waits for it), at most `limit`. Windows tells when; nothing is asked
/// again.
pub fn stop_daemon(limit: Option<Duration>) -> anyhow::Result<()> {
    // The daemon of a version before 0.20 answers under another name (an update from it may
    // have left it running).
    for endpoint in [Some(Endpoint::current_user()), Endpoint::older_user()]
        .into_iter()
        .flatten()
    {
        if let Some(pid) = endpoint.ask_to_quit()? {
            wait_for_exit(pid, limit)?;
        }
    }
    Ok(())
}

fn wait_for_exit(pid: u32, limit: Option<Duration>) -> anyhow::Result<()> {
    let (ended, end) = std::sync::mpsc::channel();
    anyhow::ensure!(
        vixeeny_platform::on_process_exit(pid, move || {
            let _ = ended.send(());
        }),
        "cannot wait for Vixeeny to stop"
    );
    match limit {
        Some(limit) => anyhow::ensure!(end.recv_timeout(limit).is_ok(), "Vixeeny is still running"),
        None => {
            let _ = end.recv();
        }
    }
    Ok(())
}

/// Starts the daemon in `dir` and waits until it says it runs (`Ok(true)`), it ends, or it
/// stays silent for [`START_TIMEOUT`] (`Ok(false)`). A new version that stays silent is
/// stopped (`stop_silent`); the previous one, put back, is left to run whatever happens.
fn start_daemon(dir: &Path, stop_silent: bool) -> std::io::Result<bool> {
    let mut child = std::process::Command::new(dir.join(DAEMON))
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let Some(out) = child.stdout.take() else {
        if stop_silent {
            let _ = child.kill();
            let _ = child.wait();
        }
        return Ok(false);
    };
    // Its first line, or the end of its output if it stops first.
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(out), &mut line);
        let _ = said.send(line.trim() == "ready");
    });
    let ready = heard.recv_timeout(START_TIMEOUT).unwrap_or(false);
    if !ready && stop_silent {
        // It must not go on running beside the version put back.
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(ready)
}

/// Starts the version on disk again after an update that did not happen, and sends it
/// `reopen` (the settings come back, with the error).
fn restart(dir: &Path, reopen: Option<ControlRequest>) {
    if start_daemon(dir, false).unwrap_or(false)
        && let Some(request) = reopen
    {
        let _ = control(request);
    }
}

/// Held while the files are swapped, so that nothing else takes the backup for one to clean
/// or to undo. Windows lets it go when the process ends, however it ends. `None`: another
/// process holds it.
fn lock(dir: &Path) -> Option<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut options, 0);
    options.open(dir.join(LOCK)).ok()
}

/// Installs the downloaded version and restarts Vixeeny with it; the previous version comes
/// back if the new one does not start. `reopen` is sent to the daemon once it runs (to show
/// the settings again, for instance).
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
            // Not installed again by itself: only by hand.
            if let Some(version) = state.ready.take() {
                state.failed = Some(version);
            }
        }
    }
    let _ = state.save(&path);
    result
}

/// The downloaded version is this one: an older Vixeeny window, still open after an update,
/// downloaded it again and started the installation with the program now on disk.
fn already_installed(state: &State) -> bool {
    state
        .ready
        .as_deref()
        .is_some_and(|v| !is_newer(CURRENT, v))
}

fn install_inner(reopen: Option<ControlRequest>) -> anyhow::Result<()> {
    let dir = install_dir()?;
    let Some(version) = ready() else {
        if already_installed(&State::load(&state_path()?)) {
            log(&format!("{CURRENT} is already installed"));
            let _ = std::fs::remove_dir_all(dir.join(STAGED));
            if let Some(request) = reopen {
                let _ = control(request);
            }
            return Ok(());
        }
        bail!("no update is ready");
    };
    let _lock = lock(&dir).context("another update is running")?;
    let (staged, backup) = (dir.join(STAGED), dir.join(BACKUP));
    // A backup left by an earlier update: its files were still in use then, or it was cut
    // short and never undone.
    if swap::unconfirmed(&backup) {
        let _ = swap::rollback(&dir, &backup);
    }
    let _ = std::fs::remove_dir_all(&backup);
    stop_daemon(None)?;
    // From here Vixeeny is stopped: every way out starts a version again.
    if let Err(e) = swap::install(&staged, &dir, &backup) {
        // The files are as they were.
        restart(&dir, reopen);
        return Err(e).context("cannot replace the files");
    }
    let _ = std::fs::remove_dir_all(&staged);
    log(&format!("starting {version}"));
    if start_daemon(&dir, true).unwrap_or(false) {
        // The old programs that still run (this one included) keep the backup busy: what
        // cannot go now goes at the next start (`clean_up`).
        let _ = swap::commit(&backup);
        if let Some(request) = reopen {
            let _ = control(request);
        }
        return Ok(());
    }
    let back = swap::rollback(&dir, &backup);
    restart(&dir, reopen);
    back.context("the new version did not start, and the previous one could not be restored")?;
    bail!("the new version did not start; the previous one was restored")
}

/// Undoes an install cut short (power cut, process killed): the files of the previous version
/// come back and it starts again. Run by the daemon as it starts (`--update pending`).
/// `true` when there was one: that version is not installed again by itself.
pub fn recover() -> anyhow::Result<bool> {
    let dir = install_dir()?;
    let backup = dir.join(BACKUP);
    if !swap::unconfirmed(&backup) {
        return Ok(false);
    }
    // Held: an install runs now (the daemon it started runs this).
    let Some(_lock) = lock(&dir) else {
        return Ok(false);
    };
    // Written by the updater of a version before 0.20, which takes no lock: it may still be at
    // work (waiting for the new version, which runs this, to start), and finishes on its own.
    if swap::older_and_recent(&backup, Duration::from_secs(600)) {
        return Ok(false);
    }
    let path = state_path()?;
    let mut state = State::load(&path);
    state.error = Some("the install was cut short".into());
    if let Some(version) = state.ready.take() {
        state.failed = Some(version);
    }
    let _ = state.save(&path);
    log("-- undoing an install cut short");
    stop_daemon(None)?;
    let back = swap::rollback(&dir, &backup);
    if back.is_err() {
        // Not tried again at each start.
        let _ = swap::commit(&backup);
    }
    restart(&dir, None);
    back.context("cannot restore the previous version")?;
    log("previous version restored");
    Ok(true)
}

/// Removes what an update left behind (the old files, still in use while it ran, and the
/// console updater of the versions before 0.9.2).
pub fn clean_up() {
    if let Ok(dir) = install_dir() {
        let backup = dir.join(BACKUP);
        // Not while an update runs, and not an install to undo (`recover` does).
        if backup.exists()
            && !swap::unconfirmed(&backup)
            && let Some(_lock) = lock(&dir)
        {
            let _ = swap::commit(&backup);
        }
        // Files of older versions: the console updater (before 0.9.2), the daemon under its old
        // name and FFmpeg's device library (before 0.9.4).
        for old in [
            "vixeeny-updater.exe",
            "vixeeny-daemon.exe",
            "avdevice-63.dll",
        ] {
            let _ = std::fs::remove_file(dir.join(old));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_of_this_version_is_already_installed() {
        let with = |ready: &str| State {
            ready: Some(ready.into()),
            ..State::default()
        };
        assert!(already_installed(&with(CURRENT)));
        assert!(already_installed(&with("0.0.1")));
        assert!(!already_installed(&with("999.0.0")));
        assert!(!already_installed(&State::default()));
    }
}
