// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS sources through ScreenCaptureKit (macOS 13+ for system and per-application audio, 15+
//! for the microphone): an `SCStream` that only listens. Buffers arrive on the host clock
//! (`vixeeny_platform::monotonic_ns`), planar 32-bit float, and are interleaved here.
//!
//! A source that stops (application quit, device gone) reports `Lost` once; unlike the Windows
//! sources it does not retry yet.

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_audio_types::AudioBufferList;
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMBlockBuffer, CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRunningApplication, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamOutput, SCStreamOutputType,
};

use crate::{
    AudioChunk, AudioError, AudioSink, AudioSource, CHANNELS, SAMPLE_RATE, SourceEvent, SourceKind,
};

const TIMEOUT: Duration = Duration::from_secs(5);

type SharedSink = Arc<Mutex<Option<Box<dyn AudioSink>>>>;

struct Ivars {
    sink: SharedSink,
    wanted: SCStreamOutputType,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "VixeenyAudioOutput"]
    #[ivars = Ivars]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(
            &self,
            _stream: &SCStream,
            buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != self.ivars().wanted {
                return;
            }
            let Some(chunk) = chunk_of(buffer) else {
                return;
            };
            if let Ok(mut sink) = self.ivars().sink.lock()
                && let Some(sink) = sink.as_mut()
            {
                sink.on_audio(chunk);
            }
        }
    }

    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            if let Ok(mut sink) = self.ivars().sink.lock()
                && let Some(sink) = sink.as_mut()
            {
                sink.on_event(SourceEvent::Lost(error.localizedDescription().to_string()));
            }
        }
    }
);

impl Output {
    fn new(sink: SharedSink, wanted: SCStreamOutputType) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { sink, wanted });
        // SAFETY: the superclass initialiser of a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

fn nanoseconds(t: CMTime) -> i64 {
    if t.timescale <= 0 {
        return 0;
    }
    (i128::from(t.value) * 1_000_000_000 / i128::from(t.timescale)) as i64
}

/// The audio of a sample buffer as an interleaved stereo chunk (mono is doubled, channels
/// beyond the second are dropped: ScreenCaptureKit is asked for stereo).
fn chunk_of(buffer: &CMSampleBuffer) -> Option<AudioChunk> {
    // First ask how big the buffer list is, then fetch it (one buffer per channel when planar).
    let mut needed = 0usize;
    let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
    // SAFETY: the size query only writes `needed`.
    let status = unsafe {
        buffer.audio_buffer_list_with_retained_block_buffer(
            &raw mut needed,
            std::ptr::null_mut(),
            0,
            None,
            None,
            0,
            &raw mut block,
        )
    };
    if needed == 0 || (status != 0 && status != -12737) {
        return None;
    }
    // 8-byte aligned storage for the variable-length list.
    let mut storage = vec![0u64; needed.div_ceil(8)];
    let list = storage.as_mut_ptr().cast::<AudioBufferList>();
    // SAFETY: `list` has `needed` bytes; on success `block` holds the +1 retained block buffer
    // that owns the sample memory, released below after the copy.
    let status = unsafe {
        buffer.audio_buffer_list_with_retained_block_buffer(
            std::ptr::null_mut(),
            list,
            needed,
            None,
            None,
            0,
            &raw mut block,
        )
    };
    let owner = std::ptr::NonNull::new(block).map(|b| {
        // SAFETY: the call above returned this block buffer retained.
        unsafe { CFRetained::from_raw(b) }
    });
    if status != 0 {
        return None;
    }
    // SAFETY: `list` was filled by the call above; its buffers point into `owner`.
    let chunk = unsafe {
        let count = (*list).mNumberBuffers as usize;
        let buffers = std::slice::from_raw_parts((*list).mBuffers.as_ptr(), count);
        let planes: Vec<&[f32]> = buffers
            .iter()
            .filter(|b| !b.mData.is_null())
            .map(|b| {
                std::slice::from_raw_parts(
                    b.mData.cast::<f32>(),
                    b.mDataByteSize as usize / size_of::<f32>(),
                )
            })
            .collect();
        interleave(
            &planes,
            buffers.first().map_or(1, |b| b.mNumberChannels as usize),
        )
    };
    drop(owner);
    // SAFETY: reading the timestamp of a live sample buffer.
    let time_ns = nanoseconds(unsafe { buffer.presentation_time_stamp() });
    chunk.map(|samples| AudioChunk {
        time_ns,
        channels: CHANNELS,
        samples,
    })
}

/// Planar (`planes.len()` buffers of one channel) or interleaved (`per_buffer` channels in the
/// first buffer) float samples as interleaved stereo.
fn interleave(planes: &[&[f32]], per_buffer: usize) -> Option<Vec<f32>> {
    match (planes, per_buffer) {
        ([], _) => None,
        ([mono], 1) => Some(mono.iter().flat_map(|v| [*v, *v]).collect()),
        ([left, right, ..], 1) => {
            let frames = left.len().min(right.len());
            Some((0..frames).flat_map(|i| [left[i], right[i]]).collect())
        }
        ([all, ..], 2) => Some(all.to_vec()),
        ([all, ..], n) if n > 2 => Some(all.chunks_exact(n).flat_map(|f| [f[0], f[1]]).collect()),
        _ => None,
    }
}

/// One listening stream.
pub struct SckAudioSource {
    kind: SourceKind,
    sink: SharedSink,
    running: Option<Running>,
}

struct Running {
    stream: Retained<SCStream>,
    _output: Retained<Output>,
    _queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: SCStream and the dispatch queue may be used from any thread (Apple documents the
// stream's start/stop as thread-safe); the output object is only touched by the framework.
unsafe impl Send for Running {}

impl SckAudioSource {
    pub fn new(kind: SourceKind) -> Self {
        Self {
            kind,
            sink: Arc::default(),
            running: None,
        }
    }
}

/// Matches `wanted` (`Spotify.exe`, `spotify`, `com.spotify.client`) against a running app.
fn matches(app: &SCRunningApplication, wanted: &str) -> bool {
    let wanted = wanted
        .trim()
        .trim_end_matches(".exe")
        .trim_end_matches(".app")
        .to_lowercase();
    // SAFETY: reading properties of a live object.
    let (name, bundle) = unsafe { (app.applicationName(), app.bundleIdentifier()) };
    let (name, bundle) = (
        name.to_string().to_lowercase(),
        bundle.to_string().to_lowercase(),
    );
    !wanted.is_empty()
        && (name == wanted || bundle == wanted || bundle.ends_with(&format!(".{wanted}")))
}

impl AudioSource for SckAudioSource {
    fn start(&mut self, sink: Box<dyn AudioSink>) -> Result<(), AudioError> {
        if self.running.is_some() {
            return Ok(());
        }
        if let SourceKind::Microphone(_) = self.kind
            && !vixeeny_platform::microphone_via_screen_capture_kit()
        {
            return Err(AudioError::Unavailable(
                "the microphone through ScreenCaptureKit needs macOS 15".into(),
            ));
        }
        if !vixeeny_platform::screen_capture_allowed() {
            vixeeny_platform::request_screen_capture_access();
            return Err(AudioError::Unavailable(
                "screen recording is not allowed (System Settings → Privacy & Security)".into(),
            ));
        }
        let (tx, rx) = channel();
        let handler = RcBlock::new(
            move |content: *mut SCShareableContent, _error: *mut NSError| {
                // SAFETY: null or valid for the call; retained here.
                let _ = tx.send(unsafe { Retained::retain(content) });
            },
        );
        // SAFETY: the block lives until the call returns and is copied by the framework.
        unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
        let content = rx
            .recv_timeout(TIMEOUT)
            .ok()
            .flatten()
            .ok_or_else(|| AudioError::Os("cannot list what can be captured".into()))?;
        // SAFETY: reading the displays of live content.
        let display = unsafe { content.displays() }
            .firstObject()
            .ok_or_else(|| AudioError::Unavailable("no display".into()))?;
        // SAFETY: object construction and property setters.
        let (filter, config, wanted) = unsafe {
            let filter = match &self.kind {
                SourceKind::Application(exe) => {
                    let app = content
                        .applications()
                        .iter()
                        .find(|a| matches(a, exe))
                        .ok_or_else(|| AudioError::Unavailable(format!("{exe} is not running")))?;
                    SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                        SCContentFilter::alloc(),
                        &display,
                        &NSArray::from_retained_slice(&[app]),
                        &NSArray::new(),
                    )
                }
                _ => SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::new(),
                ),
            };
            let config = SCStreamConfiguration::new();
            // Nothing to see: the smallest, slowest picture the stream accepts.
            config.setWidth(2);
            config.setHeight(2);
            config.setMinimumFrameInterval(CMTime {
                value: 1,
                timescale: 1,
                flags: CMTimeFlags::Valid,
                epoch: 0,
            });
            config.setSampleRate(SAMPLE_RATE as isize);
            config.setChannelCount(CHANNELS as isize);
            let wanted = if matches!(self.kind, SourceKind::Microphone(_)) {
                config.setCaptureMicrophone(true);
                SCStreamOutputType::Microphone
            } else {
                config.setCapturesAudio(true);
                config.setExcludesCurrentProcessAudio(true);
                SCStreamOutputType::Audio
            };
            (filter, config, wanted)
        };
        *self
            .sink
            .lock()
            .map_err(|_| AudioError::Os("poisoned".into()))? = Some(sink);
        let output = Output::new(Arc::clone(&self.sink), wanted);
        let queue = DispatchQueue::new("vixeeny.audio", None);
        // SAFETY: the output outlives the stream (kept in `running`).
        let stream = unsafe {
            let stream = SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*output)),
            );
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*output),
                    wanted,
                    Some(&queue),
                )
                .map_err(|e| AudioError::Os(e.localizedDescription().to_string()))?;
            stream
        };
        let (started_tx, started_rx) = channel();
        let handler = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: null or valid for the call.
            let text =
                unsafe { Retained::retain(error) }.map(|e| e.localizedDescription().to_string());
            let _ = started_tx.send(text);
        });
        // SAFETY: the block lives until the call returns and is copied by the framework.
        unsafe { stream.startCaptureWithCompletionHandler(Some(&handler)) };
        match started_rx.recv_timeout(TIMEOUT) {
            Ok(None) => {}
            Ok(Some(e)) => return Err(AudioError::Os(e)),
            Err(_) => return Err(AudioError::Os("the audio stream did not start".into())),
        }
        self.running = Some(Running {
            stream,
            _output: output,
            _queue: queue,
        });
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        if let Some(running) = self.running.take() {
            // SAFETY: stopping a stream we started; the completion is not awaited.
            unsafe { running.stream.stopCaptureWithCompletionHandler(None) };
        }
        if let Ok(mut sink) = self.sink.lock() {
            *sink = None;
        }
        Ok(())
    }
}

impl Drop for SckAudioSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_interleaved_and_mono_buffers_become_stereo() {
        let l = [1.0f32, 2.0];
        let r = [-1.0f32, -2.0];
        assert_eq!(interleave(&[&l, &r], 1), Some(vec![1.0, -1.0, 2.0, -2.0]));
        assert_eq!(interleave(&[&l], 1), Some(vec![1.0, 1.0, 2.0, 2.0]));
        let both = [0.1f32, 0.2, 0.3, 0.4];
        assert_eq!(interleave(&[&both], 2), Some(both.to_vec()));
        let quad = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        assert_eq!(interleave(&[&quad], 4), Some(vec![1.0, 2.0, 5.0, 6.0]));
        assert_eq!(interleave(&[], 1), None);
    }

    #[test]
    fn media_time_is_converted_to_nanoseconds() {
        let t = CMTime {
            value: 48_000,
            timescale: 48_000,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };
        assert_eq!(nanoseconds(t), 1_000_000_000);
    }
}
