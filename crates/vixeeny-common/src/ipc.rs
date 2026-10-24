// SPDX-License-Identifier: GPL-3.0-or-later
//! Daemon ↔ app IPC: `postcard` messages with a 4-byte little-endian length prefix
//! over a local socket (named pipe on Windows, Unix socket elsewhere).
//!
//! The daemon owns the listener. Binding it is also the single-instance lock: if another
//! daemon already answers on the endpoint, [`bind`] reports [`BindError::AlreadyRunning`].

use std::io::{self, Read, Write};

/// Transport types, re-exported so that users of this module need no `interprocess` dependency.
pub use interprocess::local_socket::traits::{Listener as ListenerTrait, Stream as StreamTrait};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name, ToNsName};
pub use interprocess::local_socket::{Listener, RecvHalf, SendHalf, Stream};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Hard cap on a message, so a corrupt length prefix cannot make us allocate gigabytes.
pub const MAX_MESSAGE_BYTES: u32 = 16 * 1024 * 1024;

/// Action the daemon asks the app to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionId {
    CaptureRegion,
    CaptureWindow,
    CaptureFullscreen,
    CaptureAllMonitors,
    CaptureScrolling,
    RecordToggle,
    RecordPause,
    ReplayToggle,
    ReplaySave,
    /// Saves the replay, then opens it in the video editor to cut it.
    ReplayEdit,
    OverlayToggle,
    OpenSettings,
    /// Not a shortcut: keeps the app running while the replay is on, to start it during
    /// full-screen games (the daemon sends it at startup and when the settings change).
    ReplayWatch,
}

impl ActionId {
    /// All actions, in a stable order.
    pub const ALL: [Self; 13] = [
        Self::CaptureRegion,
        Self::CaptureWindow,
        Self::CaptureFullscreen,
        Self::CaptureAllMonitors,
        Self::CaptureScrolling,
        Self::RecordToggle,
        Self::RecordPause,
        Self::ReplayToggle,
        Self::ReplaySave,
        Self::ReplayEdit,
        Self::OverlayToggle,
        Self::OpenSettings,
        Self::ReplayWatch,
    ];

    /// Name used on the `vixeeny-app --action <name>` command line.
    pub const fn cli_name(self) -> &'static str {
        match self {
            Self::CaptureRegion => "capture-region",
            Self::CaptureWindow => "capture-window",
            Self::CaptureFullscreen => "capture-fullscreen",
            Self::CaptureAllMonitors => "capture-all-monitors",
            Self::CaptureScrolling => "capture-scrolling",
            Self::RecordToggle => "record-toggle",
            Self::RecordPause => "record-pause",
            Self::ReplayToggle => "replay-toggle",
            Self::ReplaySave => "replay-save",
            Self::ReplayEdit => "replay-edit",
            Self::OverlayToggle => "overlay-toggle",
            Self::OpenSettings => "open-settings",
            Self::ReplayWatch => "replay-watch",
        }
    }

    /// Actions that start on the frozen screen (a zone is picked on it).
    pub const fn freezes_screen(self) -> bool {
        matches!(self, Self::CaptureRegion | Self::CaptureScrolling)
    }

    pub fn from_cli_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.cli_name() == name)
    }
}

/// The screens the daemon froze the moment a zone capture was asked for: one GPU texture per
/// monitor, shared with the app, and the window that shows it until the editor is up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frozen {
    /// LUID of the graphics adapter that holds the textures (the app must open them there).
    pub adapter: i64,
    pub screens: Vec<FrozenScreen>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenScreen {
    /// The monitor, physical pixels of the virtual desktop.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Shared handle of the texture: BGRA 8-bit, or RGBA half floats (scRGB) when `hdr`.
    pub texture: u64,
    pub hdr: bool,
    /// The window showing the frozen screen; the app posts it [`THAW_MESSAGE`] once its own
    /// windows cover the screens.
    pub window: u64,
}

/// `WM_APP + 0x76` (`WM_APP` = `0x8000`): removes the frozen screens (any of their windows).
pub const THAW_MESSAGE: u32 = 0x8076;

impl Frozen {
    /// Command-line form (`--frozen`), for an app started by the press itself:
    /// `adapter;x,y,w,h,texture,hdr,window;…`.
    pub fn to_arg(&self) -> String {
        let mut out = self.adapter.to_string();
        for s in &self.screens {
            out.push_str(&format!(
                ";{},{},{},{},{},{},{}",
                s.x,
                s.y,
                s.width,
                s.height,
                s.texture,
                u8::from(s.hdr),
                s.window
            ));
        }
        out
    }

    pub fn from_arg(arg: &str) -> Option<Self> {
        let mut parts = arg.split(';');
        let adapter = parts.next()?.parse().ok()?;
        let screens = parts
            .map(|part| {
                let f: Vec<&str> = part.split(',').collect();
                let [x, y, width, height, texture, hdr, window] = f.as_slice() else {
                    return None;
                };
                Some(FrozenScreen {
                    x: x.parse().ok()?,
                    y: y.parse().ok()?,
                    width: width.parse().ok()?,
                    height: height.parse().ok()?,
                    texture: texture.parse().ok()?,
                    hdr: *hdr == "1",
                    window: window.parse().ok()?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        (!screens.is_empty()).then_some(Self { adapter, screens })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecState {
    Idle,
    Recording,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonToApp {
    RunAction {
        action: ActionId,
        frozen: Option<Frozen>,
    },
    ConfigChanged,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppToDaemon {
    Ready,
    RecordingStateChanged(RecState),
    /// The app is about to exit.
    Idle,
    /// The UI saved the config; the daemon re-reads it.
    ConfigChanged,
}

/// One-shot requests from short-lived processes, e.g. a second launch of Vixeeny.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlRequest {
    OpenSettings,
    /// The settings changed on disk (the settings window is a process of its own).
    ReloadConfig,
    Quit,
    Ping,
    /// Like `Quit`, but ignored while a recording runs; the update asks again until it works.
    QuitForUpdate,
    /// Runs an action, like its shortcut would.
    Action(ActionId),
    /// `true` while the settings window records a shortcut: the global shortcuts are released so
    /// that pressing one does not run it. `false` (or any reload of the settings) gives them back.
    PauseHotkeys(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlReply {
    Ok,
    /// To [`ControlRequest::QuitForUpdate`]: the daemon's process, which ends now or after
    /// the recording.
    Quitting {
        pid: u32,
    },
}

/// First message of every connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hello {
    /// `vixeeny-app`: long-lived duplex channel.
    App { pid: u32 },
    /// One request, one [`ControlReply`].
    Control(ControlRequest),
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("ipc i/o: {0}")]
    Io(#[from] io::Error),
    #[error("ipc encoding: {0}")]
    Codec(#[from] postcard::Error),
    #[error("ipc message of {0} bytes exceeds the limit")]
    TooLarge(u32),
}

#[derive(Debug, thiserror::Error)]
pub enum BindError {
    #[error("another Vixeeny daemon is already running")]
    AlreadyRunning,
    #[error("cannot create the ipc endpoint: {0}")]
    Io(#[from] io::Error),
}

/// Writes one framed message.
pub fn write_msg<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), IpcError> {
    let payload = postcard::to_stdvec(msg)?;
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|n| *n <= MAX_MESSAGE_BYTES)
        .ok_or(IpcError::TooLarge(u32::MAX))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&payload)?;
    w.flush()?;
    Ok(())
}

/// Reads one framed message. A clean end of stream before a message is `Ok(None)`.
pub fn read_msg<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<Option<T>, IpcError> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len);
    if len > MAX_MESSAGE_BYTES {
        return Err(IpcError::TooLarge(len));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    Ok(Some(postcard::from_bytes(&payload)?))
}

/// Who may open the daemon's named pipe (SDDL): not network logons, the system and the user
/// `sid`, by name: the owner of what a program with the administrator's rights makes is the
/// Administrators group, which this user's other programs are not. The label lets them write
/// to it all the same (it would take the creator's level otherwise).
fn pipe_sddl(sid: &str) -> String {
    format!("D:P(D;;GA;;;NU)(A;;GA;;;SY)(A;;GA;;;{sid})S:(ML;;NW;;;ME)")
}

/// A named local-socket endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    base: String,
}

impl Endpoint {
    /// The per-user endpoint (one daemon per user session), named after the user's SID: two
    /// accounts never share it, whatever their names. `VIXEENY_IPC_NAME` overrides the name,
    /// e.g. for benchmarks running next to a normal install.
    pub fn current_user() -> Self {
        let user = identity::current_sid()
            .or_else(|| std::env::var("USERNAME").ok())
            .unwrap_or_else(|| "user".into());
        Self::named(
            &std::env::var("VIXEENY_IPC_NAME").unwrap_or_else(|_| format!("vixeeny-{user}")),
        )
    }

    /// The name the versions before 0.20 gave the endpoint (after the user's name, not their
    /// SID): only to stop the daemon of such a version, which an update from it may leave
    /// running. `None` when it is the same, or the name is set by `VIXEENY_IPC_NAME` (tests).
    pub fn older_user() -> Option<Self> {
        if std::env::var_os("VIXEENY_IPC_NAME").is_some() {
            return None;
        }
        let user = std::env::var("USERNAME").ok()?;
        let older = Self::named(&format!("vixeeny-{user}"));
        (older != Self::current_user()).then_some(older)
    }

    /// Asks the daemon here to stop the way an update does; returns its process, which ends
    /// now or once a recording is finished. `Ok(None)`: no daemon of this user answers here.
    pub fn ask_to_quit(&self) -> Result<Option<u32>, IpcError> {
        let Ok(mut stream) = self.connect() else {
            return Ok(None);
        };
        write_msg(&mut stream, &Hello::Control(ControlRequest::QuitForUpdate))?;
        match read_msg::<_, ControlReply>(&mut stream)? {
            Some(ControlReply::Quitting { pid }) => Ok(Some(pid)),
            _ => Err(IpcError::Io(io::Error::other(
                "the daemon did not agree to stop",
            ))),
        }
    }

    /// An endpoint with an explicit name (tests).
    pub fn named(name: &str) -> Self {
        let base = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Self { base }
    }

    fn name(&self) -> io::Result<Name<'static>> {
        self.base.clone().to_ns_name::<GenericNamespaced>()
    }

    /// Connects to the running daemon. The pipe must belong to this user: one another account
    /// made under this name is left before anything is written to it (so it learns nothing and
    /// cannot act as this user).
    pub fn connect(&self) -> io::Result<Stream> {
        let stream = Stream::connect(self.name()?)?;
        let server = interprocess::local_socket::traits::StreamCommon::peer_creds(&stream)?.pid();
        if !server.is_some_and(identity::same_user) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the Vixeeny pipe belongs to another user",
            ));
        }
        Ok(stream)
    }

    /// Listener options. The pipe is closed to everyone but the current user and the system: the
    /// default access control would let any local account connect and, for instance, make the
    /// daemon quit or run actions.
    fn options(&self) -> io::Result<ListenerOptions<'static>> {
        use interprocess::os::windows::local_socket::ListenerOptionsExt;
        use interprocess::os::windows::security_descriptor::SecurityDescriptor;
        // Protected DACL: deny network logons, allow the system and the user who started the
        // daemon; nobody else.
        let sid = identity::current_sid()
            .ok_or_else(|| io::Error::other("cannot read this process's account"))?;
        let sddl = widestring::U16CString::from_str(pipe_sddl(&sid)).map_err(io::Error::other)?;
        Ok(ListenerOptions::new()
            .name(self.name()?)
            .security_descriptor(SecurityDescriptor::deserialize(&sddl)?))
    }

    /// Creates the daemon's listener, doubling as the single-instance lock.
    pub fn bind(&self) -> Result<Listener, BindError> {
        match self.options()?.create_sync() {
            Ok(listener) => Ok(listener),
            Err(first) => {
                // Something owns the name. A successful connection means a live daemon; a named
                // pipe cannot be stale, so failing to connect means it is busy.
                if self.connect().is_ok() {
                    return Err(BindError::AlreadyRunning);
                }
                Err(BindError::Io(first))
            }
        }
    }
}

/// Who runs a process, from its access token.
mod identity {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{
        EqualSid, GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::core::PWSTR;

    /// The `TOKEN_USER` of `process` (aligned for it).
    fn token_user(process: HANDLE) -> Option<Vec<u64>> {
        let mut token = HANDLE::default();
        // SAFETY: a process handle with query access; `token` is closed below.
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) }.ok()?;
        let mut size = 0_u32;
        // SAFETY: asks for the size only.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &raw mut size) };
        let mut buffer = vec![0_u64; (size as usize).div_ceil(8)];
        // SAFETY: `buffer` holds `size` bytes.
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                size,
                &raw mut size,
            )
        };
        // SAFETY: opened above, closed once.
        let _ = unsafe { CloseHandle(token) };
        read.ok()?;
        Some(buffer)
    }

    fn sid(user: &[u64]) -> PSID {
        // SAFETY: `user` is a `TOKEN_USER` filled by `GetTokenInformation`, aligned.
        unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    /// This process's user, as text (`S-1-5-21-…`).
    pub fn current_sid() -> Option<String> {
        // SAFETY: the pseudo-handle of this process.
        let user = token_user(unsafe { GetCurrentProcess() })?;
        let mut text = PWSTR::null();
        // SAFETY: a valid SID; `text` is freed below.
        unsafe { ConvertSidToStringSidW(sid(&user), &raw mut text) }.ok()?;
        // SAFETY: a terminated string from the call above.
        let result = unsafe { text.to_string() }.ok();
        // SAFETY: allocated by `ConvertSidToStringSidW`, freed once.
        let _ = unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        result
    }

    /// Whether `pid` runs as this process's user. `false` when it cannot be told (a process
    /// of another user cannot be opened).
    pub fn same_user(pid: u32) -> bool {
        // SAFETY: the pseudo-handle of this process.
        let Some(ours) = token_user(unsafe { GetCurrentProcess() }) else {
            return false;
        };
        // SAFETY: plain arguments; the handle is closed below.
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return false;
        };
        let theirs = token_user(process);
        // SAFETY: opened above, closed once.
        let _ = unsafe { CloseHandle(process) };
        // SAFETY: two valid SIDs.
        theirs.is_some_and(|theirs| unsafe { EqualSid(sid(&ours), sid(&theirs)) }.is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn the_pipe_is_this_users_and_another_users_is_refused() {
        let sid = identity::current_sid().unwrap_or_default();
        assert!(sid.starts_with("S-1-"), "{sid}");
        assert!(identity::same_user(std::process::id()));
        // The System process.
        assert!(!identity::same_user(4));
    }

    #[test]
    fn action_cli_names_roundtrip() {
        for action in ActionId::ALL {
            assert_eq!(ActionId::from_cli_name(action.cli_name()), Some(action));
        }
        assert_eq!(ActionId::from_cli_name("nope"), None);
    }

    fn frozen() -> Frozen {
        let screen = FrozenScreen {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
            texture: 0x4000_0c42,
            hdr: false,
            window: 0x0012_04ae,
        };
        Frozen {
            adapter: -42,
            screens: vec![
                screen,
                FrozenScreen {
                    x: 0,
                    width: 3840,
                    height: 2160,
                    hdr: true,
                    ..screen
                },
            ],
        }
    }

    #[test]
    fn frozen_screens_go_through_the_command_line() {
        let frozen = frozen();
        assert_eq!(Frozen::from_arg(&frozen.to_arg()), Some(frozen));
        assert_eq!(Frozen::from_arg("12"), None);
        assert_eq!(Frozen::from_arg("12;1,2,3"), None);
    }

    #[test]
    fn framing_roundtrip() {
        let mut buf = Vec::new();
        let msg = DaemonToApp::RunAction {
            action: ActionId::CaptureRegion,
            frozen: Some(frozen()),
        };
        write_msg(&mut buf, &msg).unwrap();
        write_msg(&mut buf, &DaemonToApp::Shutdown).unwrap();
        let mut r = Cursor::new(buf);
        assert_eq!(read_msg::<_, DaemonToApp>(&mut r).unwrap(), Some(msg));
        assert_eq!(
            read_msg::<_, DaemonToApp>(&mut r).unwrap(),
            Some(DaemonToApp::Shutdown)
        );
        assert_eq!(read_msg::<_, DaemonToApp>(&mut r).unwrap(), None);
    }

    #[test]
    fn oversized_length_is_rejected() {
        let mut r = Cursor::new((MAX_MESSAGE_BYTES + 1).to_le_bytes().to_vec());
        assert!(matches!(
            read_msg::<_, Hello>(&mut r),
            Err(IpcError::TooLarge(_))
        ));
    }

    #[test]
    fn a_cut_or_garbled_message_is_an_error() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &Hello::Control(ControlRequest::Ping)).unwrap();
        buf.pop();
        assert!(read_msg::<_, Hello>(&mut Cursor::new(buf)).is_err());
        let mut buf = 3u32.to_le_bytes().to_vec();
        buf.extend_from_slice(&[0xff, 0xff, 0xff]);
        assert!(read_msg::<_, Hello>(&mut Cursor::new(buf)).is_err());
    }

    fn unique(tag: &str) -> Endpoint {
        Endpoint::named(&format!("vx-test-{tag}-{}", std::process::id()))
    }

    #[test]
    fn second_bind_reports_already_running() {
        let ep = unique("bind");
        let _first = ep.bind().unwrap();
        assert!(matches!(ep.bind(), Err(BindError::AlreadyRunning)));
    }

    #[test]
    fn control_request_roundtrip_over_the_socket() {
        let ep = unique("ctl");
        let listener = ep.bind().unwrap();
        let server = std::thread::spawn(move || {
            let mut conn = listener.accept().unwrap();
            let hello: Hello = read_msg(&mut conn).unwrap().unwrap();
            write_msg(&mut conn, &ControlReply::Ok).unwrap();
            hello
        });
        let mut client = ep.connect().unwrap();
        write_msg(&mut client, &Hello::Control(ControlRequest::OpenSettings)).unwrap();
        let reply: ControlReply = read_msg(&mut client).unwrap().unwrap();
        assert_eq!(reply, ControlReply::Ok);
        assert_eq!(
            server.join().unwrap(),
            Hello::Control(ControlRequest::OpenSettings)
        );
    }
    /// What an update, or a newer daemon, asks a daemon of any version that answers.
    #[test]
    fn a_daemon_is_asked_to_quit_and_tells_its_process() {
        let ep = unique("quit");
        assert!(matches!(ep.ask_to_quit(), Ok(None)), "nobody answers yet");
        let listener = ep.bind().unwrap();
        let server = std::thread::spawn(move || {
            let mut conn = listener.accept().unwrap();
            let hello: Hello = read_msg(&mut conn).unwrap().unwrap();
            write_msg(&mut conn, &ControlReply::Quitting { pid: 42 }).unwrap();
            hello
        });
        assert_eq!(ep.ask_to_quit().unwrap(), Some(42));
        assert_eq!(
            server.join().unwrap(),
            Hello::Control(ControlRequest::QuitForUpdate)
        );
    }

    #[test]
    fn the_older_name_is_the_users_name() {
        // Tests name their endpoints: there is no older one to stop then.
        if std::env::var_os("VIXEENY_IPC_NAME").is_some() {
            assert_eq!(Endpoint::older_user(), None);
            return;
        }
        let Ok(user) = std::env::var("USERNAME") else {
            return;
        };
        assert_eq!(
            Endpoint::older_user(),
            Some(Endpoint::named(&format!("vixeeny-{user}")))
        );
    }
}
