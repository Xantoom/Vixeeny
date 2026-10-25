// SPDX-License-Identifier: GPL-3.0-or-later
//! Playing sound on the speakers (the default output, WASAPI shared mode): the video editor's preview. Stereo 48 kHz
//! floats in, Windows converting them to the device's own format. The position of what the speakers have played is the
//! preview's clock. Windows signals a [`Wake`] each time the speakers take samples, so the feeding thread sleeps until
//! there is room.

use std::sync::Arc;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioClock, IAudioRenderClient, ISimpleAudioVolume,
    WAVEFORMATEX, eConsole, eRender,
};
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
use windows::core::PCWSTR;

use crate::wasapi::{ComGuard, enumerator, os, wave_format};
use crate::{AudioError, CHANNELS, SAMPLE_RATE};

/// What wakes the thread feeding the [`Speakers`]: Windows when they have room, anyone else with [`Wake::wake`] (a new
/// order for that thread).
pub struct Wake(HANDLE);

// SAFETY: an event handle may be waited on and signalled from any thread.
unsafe impl Send for Wake {}
// SAFETY: as above.
unsafe impl Sync for Wake {}

impl Wake {
    pub fn new() -> Result<Self, AudioError> {
        // SAFETY: a plain call; the handle is closed in `Drop`.
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map(Self).map_err(os("CreateEvent"))
    }

    pub fn wake(&self) {
        // SAFETY: a live event.
        let _ = unsafe { SetEvent(self.0) };
    }

    /// Sleeps until woken.
    pub fn wait(&self) {
        // SAFETY: a live event.
        unsafe { WaitForSingleObject(self.0, INFINITE) };
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        // SAFETY: our handle; the speakers holding it are gone with their `Arc`.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The default speakers, open for playing. Lives on the thread that opened it.
pub struct Speakers {
    client: IAudioClient,
    render: IAudioRenderClient,
    clock: IAudioClock,
    volume: ISimpleAudioVolume,
    /// Frames the buffer holds.
    buffer: u32,
    started: bool,
    /// Signalled by Windows; kept alive as long as the client uses it.
    _wake: Arc<Wake>,
    _com: ComGuard,
}

impl Speakers {
    /// Opens the default speakers; Windows signals `wake` each time they have taken samples.
    pub fn open(wake: Arc<Wake>) -> Result<Self, AudioError> {
        let com = ComGuard::new();
        let format = wave_format(CHANNELS);
        // SAFETY: COM is initialised on this thread; `format` outlives `Initialize`.
        unsafe {
            let device = enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole).map_err(os("default output"))?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(os("Activate"))?;
            // 100 ms of buffer: enough to ride over a busy moment, short for a seek.
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    1_000_000,
                    0,
                    (&raw const format).cast::<WAVEFORMATEX>(),
                    None,
                )
                .map_err(os("Initialize"))?;
            client.SetEventHandle(wake.0).map_err(os("SetEventHandle"))?;
            let buffer = client.GetBufferSize().map_err(os("GetBufferSize"))?;
            Ok(Self {
                render: client.GetService().map_err(os("GetService(render)"))?,
                clock: client.GetService().map_err(os("GetService(clock)"))?,
                volume: client.GetService().map_err(os("GetService(volume)"))?,
                client,
                buffer,
                started: false,
                _wake: wake,
                _com: com,
            })
        }
    }

    /// Frames that can be written now.
    pub fn room(&self) -> Result<usize, AudioError> {
        // SAFETY: an initialised client.
        let padding = unsafe { self.client.GetCurrentPadding() }.map_err(os("padding"))?;
        Ok(self.buffer.saturating_sub(padding) as usize)
    }

    /// Writes interleaved stereo samples (a whole number of frames, at most [`Self::room`]). Returns the frames
    /// written.
    pub fn write(&mut self, samples: &[f32]) -> Result<usize, AudioError> {
        let frames = (samples.len() / CHANNELS).min(self.room()?);
        if frames == 0 {
            return Ok(0);
        }
        // SAFETY: the buffer handed out holds `frames` frames of the format set at `Initialize` (stereo floats);
        // exactly that is copied, then released.
        unsafe {
            let data = self.render.GetBuffer(frames as u32).map_err(os("GetBuffer"))?;
            std::ptr::copy_nonoverlapping(samples.as_ptr(), data.cast::<f32>(), frames * CHANNELS);
            self.render.ReleaseBuffer(frames as u32, 0).map_err(os("ReleaseBuffer"))?;
        }
        self.start()?;
        Ok(frames)
    }

    /// Plays what is queued (silence if nothing is), if not playing already.
    pub fn start(&mut self) -> Result<(), AudioError> {
        if !self.started {
            // SAFETY: an initialised client.
            unsafe { self.client.Start() }.map_err(os("Start"))?;
            self.started = true;
        }
        Ok(())
    }

    /// Seconds played since the last [`Self::clear`] (what is heard now).
    pub fn played(&self) -> f64 {
        let (mut position, mut qpc) = (0_u64, 0_u64);
        // SAFETY: valid out-pointers.
        let frequency = unsafe {
            let _ = self.clock.GetPosition(&raw mut position, Some(&raw mut qpc));
            self.clock.GetFrequency().unwrap_or(u64::from(SAMPLE_RATE))
        };
        position as f64 / frequency.max(1) as f64
    }

    /// Stops and forgets what is queued: the next write starts again from zero.
    pub fn clear(&mut self) {
        // SAFETY: an initialised client; `Reset` needs it stopped.
        unsafe {
            let _ = self.client.Stop();
            let _ = self.client.Reset();
        }
        self.started = false;
    }

    /// The preview's own volume (0 to 1), not the system's.
    pub fn set_volume(&self, volume: f32) {
        // SAFETY: plain call; a null event context.
        let _ = unsafe { self.volume.SetMasterVolume(volume.clamp(0.0, 1.0), std::ptr::null()) };
    }

    /// Frames the buffer holds: [`Self::room`] is this much once everything is played.
    pub const fn buffer_frames(&self) -> u32 {
        self.buffer
    }
}

impl Drop for Speakers {
    fn drop(&mut self) {
        // SAFETY: an initialised client.
        let _ = unsafe { self.client.Stop() };
    }
}
