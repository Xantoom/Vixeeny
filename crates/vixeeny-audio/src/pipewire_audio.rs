// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux audio through PipeWire (PulseAudio's compatibility layer is a PipeWire client too):
//! what the speakers play is the monitor of the default output, a microphone is the default
//! input (or a node named in the profile), an application is its own playback stream, found by
//! its executable name. Every capture asks for 48 kHz stereo `f32`; the graph converts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::param::audio::{AudioFormat, AudioInfoRaw};
use spa::pod::Pod;

use crate::{
    AudioChunk, AudioError, AudioSink, AudioSource, CHANNELS, SAMPLE_RATE, SourceEvent, SourceKind,
};

/// `PW_KEY_TARGET_OBJECT` (the crate only names it behind a version feature).
const TARGET_OBJECT: &str = "target.object";

/// Tells the PipeWire thread to leave its loop.
struct Quit;

pub struct PipeWireAudioSource {
    kind: SourceKind,
    running: Option<Running>,
}

struct Running {
    quit: pw::channel::Sender<Quit>,
    thread: JoinHandle<()>,
}

impl PipeWireAudioSource {
    pub fn new(kind: SourceKind) -> Self {
        Self {
            kind,
            running: None,
        }
    }
}

struct Callbacks {
    format: AudioInfoRaw,
    sink: Box<dyn AudioSink>,
    lost: bool,
}

/// Whether `binary` (an executable name, `.exe` allowed) names the process behind a node.
fn same_application(wanted: &str, binary: &str, name: &str) -> bool {
    let wanted = wanted.trim().trim_end_matches(".exe").to_lowercase();
    !wanted.is_empty() && (binary.to_lowercase() == wanted || name.to_lowercase() == wanted)
}

/// The `object.serial` of the playback stream of `exe`, from the registry.
fn find_application(
    mainloop: &pw::main_loop::MainLoopRc,
    core: &pw::core::CoreRc,
    exe: &str,
) -> Result<Option<String>, AudioError> {
    let os = |e: pw::Error| AudioError::Os(e.to_string());
    let registry = core.get_registry_rc().map_err(os)?;
    let found: Rc<RefCell<Option<String>>> = Rc::default();
    let done = Rc::new(Cell::new(false));
    let pending = core.sync(0).map_err(os)?;
    let _core_listener = core
        .add_listener_local()
        .done({
            let (done, mainloop) = (Rc::clone(&done), mainloop.clone());
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    done.set(true);
                    mainloop.quit();
                }
            }
        })
        .register();
    let wanted = exe.to_owned();
    let slot = Rc::clone(&found);
    let _registry_listener = registry
        .add_listener_local()
        .global(move |object| {
            let Some(props) = object.props else {
                return;
            };
            if props.get("media.class") != Some("Stream/Output/Audio") {
                return;
            }
            let binary = props.get("application.process.binary").unwrap_or_default();
            let name = props.get("application.name").unwrap_or_default();
            if same_application(&wanted, binary, name) {
                *slot.borrow_mut() = props.get("object.serial").map(str::to_owned);
            }
        })
        .register();
    while !done.get() {
        mainloop.run();
    }
    let serial = found.borrow().clone();
    Ok(serial)
}

fn on_samples(stream: &pw::stream::Stream, state: &mut Callbacks) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let channels = state.format.channels().max(1) as usize;
    let Some(data) = buffer.datas_mut().first_mut() else {
        return;
    };
    let size = data.chunk().size() as usize;
    let Some(bytes) = data.data() else {
        return;
    };
    let bytes = &bytes[..size.min(bytes.len())];
    let samples: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let frames = samples.len() / channels;
    if frames == 0 {
        return;
    }
    // The callback runs when the cycle ended: the chunk started that long ago.
    let length_ns = frames as i64 * 1_000_000_000 / i64::from(SAMPLE_RATE);
    let time_ns = vixeeny_platform::monotonic_ns() - length_ns;
    if state.lost {
        state.lost = false;
        state.sink.on_event(SourceEvent::Back);
    }
    state.sink.on_audio(AudioChunk {
        time_ns,
        channels,
        samples,
    });
}

fn format_params() -> Result<Vec<u8>, AudioError> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(CHANNELS as u32);
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|e| AudioError::Os(e.to_string()))
}

fn run(
    kind: &SourceKind,
    sink: Box<dyn AudioSink>,
    quit: pw::channel::Receiver<Quit>,
    ready: &Sender<Result<(), AudioError>>,
) -> Result<(), AudioError> {
    let os = |e: pw::Error| AudioError::Os(e.to_string());
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(os)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(os)?;
    let core = context
        .connect_rc(None)
        .map_err(|_| AudioError::Unavailable("PipeWire is not running".into()))?;

    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Production",
        *pw::keys::NODE_NAME => "vixeeny-capture",
    };
    match kind {
        SourceKind::System => props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true"),
        SourceKind::Microphone(None) => {}
        SourceKind::Microphone(Some(name)) => props.insert(TARGET_OBJECT, name.as_str()),
        SourceKind::Application(exe) => {
            let serial = find_application(&mainloop, &core, exe)?
                .ok_or_else(|| AudioError::Unavailable(format!("{exe} is not playing sound")))?;
            props.insert(TARGET_OBJECT, serial);
        }
    }

    let stream = pw::stream::StreamBox::new(&core, "vixeeny-audio", props).map_err(os)?;
    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });
    let _listener = stream
        .add_local_listener_with_user_data(Callbacks {
            format: AudioInfoRaw::new(),
            sink,
            lost: false,
        })
        .state_changed(|_, state, _, new| {
            if let pw::stream::StreamState::Error(e) = new {
                state.lost = true;
                state.sink.on_event(SourceEvent::Lost(e));
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
            if kind == spa::param::format::MediaType::Audio
                && subtype == spa::param::format::MediaSubtype::Raw
            {
                let _ = state.format.parse(param);
            }
        })
        .process(on_samples)
        .register()
        .map_err(os)?;

    let values = format_params()?;
    let mut params =
        [Pod::from_bytes(&values).ok_or_else(|| AudioError::Os("bad format pod".into()))?];
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(os)?;
    let _ = ready.send(Ok(()));
    mainloop.run();
    Ok(())
}

impl AudioSource for PipeWireAudioSource {
    fn start(&mut self, sink: Box<dyn AudioSink>) -> Result<(), AudioError> {
        if self.running.is_some() {
            return Ok(());
        }
        let kind = self.kind.clone();
        let (quit, quit_rx) = pw::channel::channel::<Quit>();
        let (ready_tx, ready_rx): (_, Receiver<Result<(), AudioError>>) = channel();
        let thread = std::thread::Builder::new()
            .name("pipewire-audio".into())
            .spawn(move || {
                if let Err(e) = run(&kind, sink, quit_rx, &ready_tx) {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| AudioError::Os(e.to_string()))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                self.running = Some(Running { quit, thread });
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err(AudioError::Os("PipeWire did not answer".into())),
        }
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        if let Some(running) = self.running.take() {
            let _ = running.quit.send(Quit);
            let _ = running.thread.join();
        }
        Ok(())
    }
}

impl Drop for PipeWireAudioSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_application_is_found_by_binary_or_name() {
        assert!(same_application("Spotify.exe", "spotify", ""));
        assert!(same_application("firefox", "", "Firefox"));
        assert!(!same_application("firefox", "chrome", "Chromium"));
        assert!(!same_application("", "", ""));
    }
}
