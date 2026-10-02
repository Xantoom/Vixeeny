// SPDX-License-Identifier: GPL-3.0-or-later
//! Daemon ↔ app IPC (plan 4.6): `postcard` messages with a 4-byte little-endian length prefix
//! over a local socket (named pipe on Windows, Unix socket elsewhere).
//!
//! The daemon owns the listener. Binding it is also the single-instance lock: if another
//! daemon already answers on the endpoint, [`bind`] reports [`BindError::AlreadyRunning`].

use std::io::{self, Read, Write};

/// Transport types, re-exported so that users of this module need no `interprocess` dependency.
pub use interprocess::local_socket::traits::{Listener as ListenerTrait, Stream as StreamTrait};
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, ToFsName, ToNsName, prelude::*,
};
pub use interprocess::local_socket::{Listener, RecvHalf, SendHalf, Stream};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
    OcrRegion,
    RecordToggle,
    RecordPause,
    ReplayToggle,
    ReplaySave,
    OverlayToggle,
    OpenSettings,
}

impl ActionId {
    /// All actions, in a stable order.
    pub const ALL: [Self; 12] = [
        Self::CaptureRegion,
        Self::CaptureWindow,
        Self::CaptureFullscreen,
        Self::CaptureAllMonitors,
        Self::CaptureScrolling,
        Self::OcrRegion,
        Self::RecordToggle,
        Self::RecordPause,
        Self::ReplayToggle,
        Self::ReplaySave,
        Self::OverlayToggle,
        Self::OpenSettings,
    ];

    /// Name used on the `vixeeny-app --action <name>` command line.
    pub const fn cli_name(self) -> &'static str {
        match self {
            Self::CaptureRegion => "capture-region",
            Self::CaptureWindow => "capture-window",
            Self::CaptureFullscreen => "capture-fullscreen",
            Self::CaptureAllMonitors => "capture-all-monitors",
            Self::CaptureScrolling => "capture-scrolling",
            Self::OcrRegion => "ocr-region",
            Self::RecordToggle => "record-toggle",
            Self::RecordPause => "record-pause",
            Self::ReplayToggle => "replay-toggle",
            Self::ReplaySave => "replay-save",
            Self::OverlayToggle => "overlay-toggle",
            Self::OpenSettings => "open-settings",
        }
    }

    pub fn from_cli_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.cli_name() == name)
    }
}

/// Shared-memory frozen frame handed over by the daemon (filled in at M7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMemHandle {
    pub name: String,
    pub len: u64,
    pub width: u32,
    pub height: u32,
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
        frozen_frame: Option<SharedMemHandle>,
    },
    ConfigChanged,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppToDaemon {
    Ready,
    RecordingStateChanged(RecState),
    RequestRestartForUpdate,
    /// The app is about to exit.
    Idle,
    /// The UI saved the config; the daemon re-reads it. (Added to the plan's 4.6 list.)
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlReply {
    Ok,
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

/// A named local-socket endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    base: String,
}

impl Endpoint {
    /// The per-user endpoint (one daemon per user session). `VIXEENY_IPC_NAME` overrides the
    /// name, e.g. for benchmarks running next to a normal install.
    pub fn current_user() -> Self {
        let user = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "user".into());
        Self::named(
            &std::env::var("VIXEENY_IPC_NAME").unwrap_or_else(|_| format!("vixeeny-{user}")),
        )
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
        if GenericNamespaced::is_supported() {
            self.base.clone().to_ns_name::<GenericNamespaced>()
        } else {
            // macOS: no namespace; use a socket file in the temp directory.
            std::env::temp_dir()
                .join(format!("{}.sock", self.base))
                .to_fs_name::<GenericFilePath>()
        }
    }

    /// Connects to the running daemon.
    pub fn connect(&self) -> io::Result<Stream> {
        Stream::connect(self.name()?)
    }

    /// Creates the daemon's listener, doubling as the single-instance lock.
    pub fn bind(&self) -> Result<Listener, BindError> {
        match ListenerOptions::new().name(self.name()?).create_sync() {
            Ok(listener) => Ok(listener),
            Err(first) => {
                // Something owns the name: a live daemon, or a corpse socket file left behind
                // by a crashed one (Unix). A successful connection means it is alive.
                if self.connect().is_ok() {
                    return Err(BindError::AlreadyRunning);
                }
                if cfg!(windows) {
                    // A named pipe cannot be stale; failing to connect means it is busy.
                    return Err(BindError::Io(first));
                }
                Ok(ListenerOptions::new()
                    .name(self.name()?)
                    .try_overwrite(true)
                    .create_sync()?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn action_cli_names_roundtrip() {
        for action in ActionId::ALL {
            assert_eq!(ActionId::from_cli_name(action.cli_name()), Some(action));
        }
        assert_eq!(ActionId::from_cli_name("nope"), None);
    }

    #[test]
    fn framing_roundtrip() {
        let mut buf = Vec::new();
        let msg = DaemonToApp::RunAction {
            action: ActionId::CaptureRegion,
            frozen_frame: Some(SharedMemHandle {
                name: "vx-frame".into(),
                len: 1024,
                width: 16,
                height: 16,
            }),
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
    fn truncated_payload_is_an_error() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &Hello::Control(ControlRequest::Ping)).unwrap();
        buf.pop();
        assert!(read_msg::<_, Hello>(&mut Cursor::new(buf)).is_err());
    }

    #[test]
    fn garbage_payload_is_an_error() {
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
}
