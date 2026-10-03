// SPDX-License-Identifier: GPL-3.0-or-later
//! Wayland video: the ScreenCast portal asks the user what to share (once; a restore token
//! remembers the answer where the desktop supports it), hands over a PipeWire connection and a
//! node, and the frames are read from that node in shared memory. No DMA-BUF is requested yet:
//! the compositor then falls back to memory buffers.
//!
//! The portal chooses the monitor or window: the `monitor` the caller names only serves to size
//! the expectation.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, channel, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::Pod;

use crate::x11::StreamFrame;
use crate::{CaptureError, CpuFrame};

fn os_error(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Os(e.to_string())
}

enum Msg {
    Frame(StreamFrame),
    Stopped(String),
}

/// Tells the PipeWire thread to leave its loop.
struct Quit;

/// A running portal capture; stops (and closes the portal session) when dropped.
pub struct PipeWireVideoStream {
    rx: Receiver<Msg>,
    quit: pw::channel::Sender<Quit>,
    thread: Option<JoinHandle<()>>,
    size: (u32, u32),
}

struct Callbacks {
    format: VideoInfoRaw,
    tx: SyncSender<Msg>,
}

/// One row of 4-byte pixels to opaque BGRA. `swap` exchanges red and blue (RGBx / RGBA sources).
fn row_to_bgra(src: &[u8], dst: &mut [u8], swap: bool) {
    for (s, d) in src
        .as_chunks::<4>()
        .0
        .iter()
        .zip(dst.as_chunks_mut::<4>().0)
    {
        *d = if swap {
            [s[2], s[1], s[0], 255]
        } else {
            [s[0], s[1], s[2], 255]
        };
    }
}

/// Whether the negotiated format stores red first.
fn red_first(format: VideoFormat) -> Option<bool> {
    if format == VideoFormat::BGRx || format == VideoFormat::BGRA {
        Some(false)
    } else if format == VideoFormat::RGBx || format == VideoFormat::RGBA {
        Some(true)
    } else {
        None
    }
}

fn on_frame(stream: &pw::stream::Stream, state: &mut Callbacks) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let size = state.format.size();
    let (width, height) = (size.width as usize, size.height as usize);
    let Some(swap) = red_first(state.format.format()) else {
        return;
    };
    let Some(data) = buffer.datas_mut().first_mut() else {
        return;
    };
    let (chunk_stride, chunk_offset) = (data.chunk().stride(), data.chunk().offset());
    let stride = if chunk_stride > 0 {
        chunk_stride as usize
    } else {
        width * 4
    };
    let offset = chunk_offset as usize;
    let Some(bytes) = data.data() else {
        // A DMA-BUF buffer: not mappable here.
        return;
    };
    if width == 0 || height == 0 || bytes.len() < offset + stride * height {
        return;
    }
    let mut out = vec![0u8; width * height * 4];
    for y in 0..height {
        let from = offset + y * stride;
        row_to_bgra(
            &bytes[from..from + width * 4],
            &mut out[y * width * 4..(y + 1) * width * 4],
            swap,
        );
    }
    let Ok(frame) = CpuFrame::from_raw(size.width, size.height, width * 4, out) else {
        return;
    };
    // A slow consumer loses frames; the compositor is never blocked on it.
    let _ = state.tx.try_send(Msg::Frame(StreamFrame {
        time_ns: vixeeny_platform::monotonic_ns(),
        frame,
    }));
}

/// What the portal gives: the PipeWire node, its connection, and the session to keep open.
struct Opened {
    node: u32,
    size: (u32, u32),
    fd: std::os::fd::OwnedFd,
    session: ashpd::desktop::Session<Screencast>,
}

async fn open_portal(cursor: bool, token_file: Option<PathBuf>) -> Result<Opened, CaptureError> {
    let proxy = Screencast::new().await.map_err(os_error)?;
    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(os_error)?;
    let token = token_file
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(if cursor {
                    CursorMode::Embedded
                } else {
                    CursorMode::Hidden
                })
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_restore_token(token.as_deref())
                .set_persist_mode(PersistMode::ExplicitlyRevoked),
        )
        .await
        .map_err(os_error)?;
    let streams = proxy
        .start(&session, None, Default::default())
        .await
        .map_err(os_error)?
        .response()
        .map_err(os_error)?;
    if let (Some(file), Some(new)) = (&token_file, streams.restore_token()) {
        // Best effort: without the token the user is simply asked again.
        let _ = std::fs::write(file, new);
    }
    let stream = streams
        .streams()
        .first()
        .ok_or_else(|| CaptureError::Os("nothing was shared".into()))?;
    let size = stream
        .size()
        .map_or((0, 0), |(w, h)| (w.max(0) as u32, h.max(0) as u32));
    let node = stream.pipe_wire_node_id();
    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(os_error)?;
    Ok(Opened {
        node,
        size,
        fd,
        session,
    })
}

fn format_params(fps: u32) -> Result<Vec<u8>, CaptureError> {
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: 16384,
                height: 16384
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction {
                num: fps.max(1),
                denom: 1
            },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(os_error)
}

/// The thread: open the portal, connect, report, run the loop until told to quit.
fn run(
    fps: u32,
    cursor: bool,
    token_file: Option<PathBuf>,
    tx: SyncSender<Msg>,
    quit: pw::channel::Receiver<Quit>,
    ready: &std::sync::mpsc::Sender<Result<(u32, u32), CaptureError>>,
) -> Result<(), CaptureError> {
    let opened = async_io::block_on(open_portal(cursor, token_file))?;
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(os_error)?;
    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(os_error)?;
    let core = context.connect_fd_rc(opened.fd, None).map_err(os_error)?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "vixeeny-screen",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(os_error)?;
    let stopped = tx.clone();
    let _listener = stream
        .add_local_listener_with_user_data(Callbacks {
            format: VideoInfoRaw::default(),
            tx,
        })
        .state_changed(move |_, _, _, new| {
            if let pw::stream::StreamState::Error(e) = new {
                let _ = stopped.try_send(Msg::Stopped(e));
            }
        })
        .param_changed(|_, state, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((kind, subtype)) = spa::param::format_utils::parse_format(param) else {
                return;
            };
            if kind != spa::param::format::MediaType::Video
                || subtype != spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            let _ = state.format.parse(param);
        })
        .process(on_frame)
        .register()
        .map_err(os_error)?;
    let values = format_params(fps)?;
    let mut params =
        [Pod::from_bytes(&values).ok_or_else(|| CaptureError::Os("bad format pod".into()))?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(opened.node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(os_error)?;
    let _ = ready.send(Ok(opened.size));
    mainloop.run();
    let _ = async_io::block_on(opened.session.close());
    Ok(())
}

impl PipeWireVideoStream {
    /// Asks the portal (a dialog the first time) and starts reading. `token_file` keeps the
    /// answer for the next time where the desktop supports restoring it.
    pub fn start(
        fps: u32,
        cursor: bool,
        token_file: Option<PathBuf>,
    ) -> Result<Self, CaptureError> {
        let (frames_tx, rx) = sync_channel(4);
        let (quit, quit_rx) = pw::channel::channel::<Quit>();
        let (ready_tx, ready_rx) = channel();
        let thread = std::thread::Builder::new()
            .name("pipewire-video".into())
            .spawn(move || {
                let reply = frames_tx.clone();
                if let Err(e) = run(fps, cursor, token_file, frames_tx, quit_rx, &ready_tx) {
                    let text = e.to_string();
                    let _ = ready_tx.send(Err(e));
                    let _ = reply.try_send(Msg::Stopped(text));
                }
            })
            .map_err(os_error)?;
        // The user may take a while to pick what to share.
        let size = ready_rx
            .recv_timeout(Duration::from_secs(120))
            .map_err(|_| CaptureError::Timeout)??;
        Ok(Self {
            rx,
            quit,
            thread: Some(thread),
            size,
        })
    }

    /// The size the portal announced for the shared source (`(0, 0)` when it did not).
    pub fn source_size(&self) -> (u32, u32) {
        self.size
    }

    pub fn recv(&self, timeout: Duration) -> Result<Option<StreamFrame>, CaptureError> {
        match self.rx.recv_timeout(timeout) {
            Ok(Msg::Frame(f)) => Ok(Some(f)),
            Ok(Msg::Stopped(e)) => Err(CaptureError::Os(e)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(CaptureError::Os("capture ended".into())),
        }
    }
}

impl Drop for PipeWireVideoStream {
    fn drop(&mut self) {
        let _ = self.quit.send(Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_become_opaque_bgra() {
        let mut out = [0u8; 8];
        row_to_bgra(&[1, 2, 3, 0, 4, 5, 6, 0], &mut out, false);
        assert_eq!(out, [1, 2, 3, 255, 4, 5, 6, 255]);
        row_to_bgra(&[1, 2, 3, 0, 4, 5, 6, 0], &mut out, true);
        assert_eq!(out, [3, 2, 1, 255, 6, 5, 4, 255]);
    }
}
