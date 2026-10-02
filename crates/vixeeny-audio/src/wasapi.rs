// SPDX-License-Identifier: GPL-3.0-or-later
//! WASAPI sources (Windows): the speakers' loopback, microphones, and per-application loopback
//! (Windows 10 2004+ "process loopback": the audio one process tree plays, whatever the
//! speakers do).
//!
//! All streams are opened as 48 kHz stereo float with `AUTOCONVERTPCM`, so Windows does the
//! sample-rate and channel conversion. Each packet is dated with the QPC time the driver gives
//! (100 ns units), the same clock as `vixeeny_platform::monotonic_ns`.
//!
//! A source that fails (device unplugged, application gone) reports `Lost` and retries every
//! second: the track is silent meanwhile and recovers by itself (CA-AUD-2, CA-AUD-3).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync, DEVICE_STATE_ACTIVE,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0_0};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    CreateEventW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    WaitForSingleObject,
};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{GUID, Interface, PCWSTR, PWSTR, implement};

use crate::{
    AudioChunk, AudioError, AudioSink, AudioSource, CHANNELS, SAMPLE_RATE, SourceEvent, SourceKind,
};

/// `AUDCLNT_E_DEVICE_INVALIDATED`.
const DEVICE_INVALIDATED: i32 = 0x8889_0004_u32 as i32;

fn os(what: &str) -> impl FnOnce(windows::core::Error) -> AudioError + '_ {
    move |e| AudioError::Os(format!("{what}: {e}"))
}

/// A device the user can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}

/// An application that has an audio session (it plays, or could play, sound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub pid: u32,
    /// File name of the executable, e.g. `Spotify.exe`: what `app:` takes.
    pub exe: String,
    pub active: bool,
}

struct ComGuard;

impl ComGuard {
    fn new() -> Self {
        // SAFETY: balanced by `CoUninitialize` in `Drop`; an already initialised thread (S_FALSE)
        // is fine, a different apartment model (an error) leaves COM usable as it is.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: matches the `CoInitializeEx` of `new`.
        unsafe { CoUninitialize() };
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator, AudioError> {
    // SAFETY: standard COM activation of the system's device enumerator.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(os("enumerator"))
}

fn device_id(device: &IMMDevice) -> String {
    // SAFETY: `GetId` returns a CoTaskMem string that is freed after copying.
    unsafe {
        match device.GetId() {
            Ok(id) => {
                let s = id.to_string().unwrap_or_default();
                CoTaskMemFree(Some(id.as_ptr().cast()));
                s
            }
            Err(_) => String::new(),
        }
    }
}

fn friendly_name(device: &IMMDevice) -> String {
    // SAFETY: the property store and the value are used and cleared within this function; the
    // string pointer is only read while the value is alive and only when its type says it is one.
    unsafe {
        let Ok(store) = device.OpenPropertyStore(STGM_READ) else {
            return String::new();
        };
        let Ok(mut value) = store.GetValue(&PKEY_Device_FriendlyName) else {
            return String::new();
        };
        let inner = &value.Anonymous.Anonymous;
        let name = if inner.vt.0 == 31 {
            // VT_LPWSTR
            PWSTR(inner.Anonymous.pwszVal.0)
                .to_string()
                .unwrap_or_default()
        } else {
            String::new()
        };
        let _ = windows::Win32::System::Com::StructuredStorage::PropVariantClear(&raw mut value);
        name
    }
}

/// The active capture devices.
pub fn list_microphones() -> Vec<DeviceInfo> {
    let _com = ComGuard::new();
    let Ok(enumerator) = enumerator() else {
        return Vec::new();
    };
    // SAFETY: plain COM calls on live interfaces; each result is checked.
    unsafe {
        let Ok(devices) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else {
            return Vec::new();
        };
        let count = devices.GetCount().unwrap_or(0);
        (0..count)
            .filter_map(|i| devices.Item(i).ok())
            .map(|d| DeviceInfo {
                id: device_id(&d),
                name: friendly_name(&d),
            })
            .collect()
    }
}

fn exe_of(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed; the buffer length is passed and updated by the call.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            windows::Win32::System::Threading::PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &raw mut len,
        );
        let _ = CloseHandle(process);
        ok.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit(['\\', '/']).next().map(str::to_owned)
    }
}

/// The applications that have an audio session on the default output device.
pub fn list_applications() -> Vec<AppInfo> {
    let _com = ComGuard::new();
    let Ok(enumerator) = enumerator() else {
        return Vec::new();
    };
    // SAFETY: plain COM calls on live interfaces; each result is checked.
    unsafe {
        let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) else {
            return Vec::new();
        };
        let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
            return Vec::new();
        };
        let Ok(sessions) = manager.GetSessionEnumerator() else {
            return Vec::new();
        };
        let count = sessions.GetCount().unwrap_or(0);
        let mut apps: Vec<AppInfo> = Vec::new();
        for i in 0..count {
            let Ok(control) = sessions.GetSession(i) else {
                continue;
            };
            let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                continue;
            };
            if control2.IsSystemSoundsSession().0 == 0 {
                continue; // S_OK: the system sounds session
            }
            let Ok(pid) = control2.GetProcessId() else {
                continue;
            };
            let Some(exe) = exe_of(pid) else { continue };
            let active = control
                .GetState()
                .is_ok_and(|s| s == windows::Win32::Media::Audio::AudioSessionStateActive);
            match apps.iter_mut().find(|a| a.exe.eq_ignore_ascii_case(&exe)) {
                Some(a) => a.active |= active,
                None => apps.push(AppInfo { pid, exe, active }),
            }
        }
        apps
    }
}

/// The root process of `exe` (the one whose parent is not the same program): its tree is what
/// process loopback captures.
fn find_process(exe: &str) -> Option<u32> {
    // SAFETY: a Toolhelp snapshot walked with the documented First/Next calls; handle closed.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut matching: Vec<(u32, u32)> = Vec::new();
        let mut more = Process32FirstW(snapshot, &raw mut entry).is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(0);
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if name.eq_ignore_ascii_case(exe) {
                matching.push((entry.th32ProcessID, entry.th32ParentProcessID));
            }
            more = Process32NextW(snapshot, &raw mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        matching
            .iter()
            .find(|(_, parent)| !matching.iter().any(|(pid, _)| pid == parent))
            .or(matching.first())
            .map(|(pid, _)| *pid)
    }
}

fn wave_format() -> WAVEFORMATEX {
    let bytes_per_frame = (CHANNELS * 4) as u16;
    WAVEFORMATEX {
        wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
        nChannels: CHANNELS as u16,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * u32::from(bytes_per_frame),
        nBlockAlign: bytes_per_frame,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

/// Completion handler of `ActivateAudioInterfaceAsync`: signals an event.
#[implement(
    IActivateAudioInterfaceCompletionHandler,
    windows::Win32::System::Com::IAgileObject
)]
struct ActivateHandler {
    done: HANDLE,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivateHandler_Impl {
    fn ActivateCompleted(
        &self,
        _operation: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        // SAFETY: `done` is a live event handle owned by the waiting thread.
        unsafe { windows::Win32::System::Threading::SetEvent(self.done) }
    }
}

// The handler is called on a thread of the MTA pool, hence agile.
impl windows::Win32::System::Com::IAgileObject_Impl for ActivateHandler_Impl {}

/// An open, started capture stream.
struct Stream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
    /// The endpoint it was opened on (to notice a change of default device).
    device_id: Option<String>,
    follows_default: bool,
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the client was started by `open`; the event handle is ours.
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}

/// Initialises `client` for event-driven float capture; `loopback` for render endpoints.
fn initialise(client: &IAudioClient, loopback: bool) -> Result<HANDLE, AudioError> {
    let format = wave_format();
    let mut flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if loopback {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    // SAFETY: `format` outlives the call; the event is created, attached and returned.
    unsafe {
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                2_000_000,
                0,
                &raw const format,
                None,
            )
            .map_err(os("Initialize"))?;
        let event = CreateEventW(None, false, false, PCWSTR::null()).map_err(os("CreateEvent"))?;
        client.SetEventHandle(event).map_err(os("SetEventHandle"))?;
        Ok(event)
    }
}

fn finish_open(
    client: IAudioClient,
    loopback: bool,
    device_id: Option<String>,
    follows_default: bool,
) -> Result<Stream, AudioError> {
    let event = initialise(&client, loopback)?;
    // SAFETY: the client is initialised; the service is requested for the capture interface.
    let capture: IAudioCaptureClient =
        unsafe { client.GetService() }.map_err(os("GetService(capture)"))?;
    // SAFETY: as above.
    unsafe { client.Start() }.map_err(os("Start"))?;
    Ok(Stream {
        client,
        capture,
        event,
        device_id,
        follows_default,
    })
}

fn open_device(
    device: &IMMDevice,
    loopback: bool,
    follows_default: bool,
) -> Result<Stream, AudioError> {
    // SAFETY: activating the audio client of a live endpoint.
    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(os("Activate"))?;
    finish_open(client, loopback, Some(device_id(device)), follows_default)
}

fn open_process_loopback(pid: u32) -> Result<Stream, AudioError> {
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    let mut variant = PROPVARIANT::default();
    // SAFETY: the blob points at `params`, which outlives the activation call; the variant is
    // never cleared (it does not own the blob).
    unsafe {
        let inner: &mut PROPVARIANT_0_0 = &mut variant.Anonymous.Anonymous;
        inner.vt = VT_BLOB;
        inner.Anonymous.blob.cbSize = size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
        inner.Anonymous.blob.pBlobData = (&raw const params).cast_mut().cast();
    }
    // SAFETY: a manual-reset-free event, closed below.
    let done =
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map_err(os("CreateEvent"))?;
    let handler: IActivateAudioInterfaceCompletionHandler = ActivateHandler { done }.into();
    let iid: GUID = IAudioClient::IID;
    // SAFETY: the parameters live until the wait below ends; the handler is agile.
    let result = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &raw const iid,
            Some(&raw const variant),
            &handler,
        )
    };
    let operation = match result {
        Ok(op) => op,
        Err(e) => {
            // SAFETY: the event is ours and not in use any more.
            unsafe {
                let _ = CloseHandle(done);
            }
            return Err(AudioError::Os(format!("ActivateAudioInterfaceAsync: {e}")));
        }
    };
    // SAFETY: wait for the completion handler, then read the activation result.
    let client = unsafe {
        let waited = WaitForSingleObject(done, 5_000);
        let _ = CloseHandle(done);
        if waited != WAIT_OBJECT_0 {
            return Err(AudioError::Os(
                "process loopback activation timed out".into(),
            ));
        }
        let mut hr = windows::core::HRESULT(0);
        let mut unknown = None;
        operation
            .GetActivateResult(&raw mut hr, &raw mut unknown)
            .map_err(os("GetActivateResult"))?;
        hr.ok().map_err(os("activation"))?;
        unknown
            .ok_or_else(|| AudioError::Os("no audio client".into()))?
            .cast::<IAudioClient>()
            .map_err(os("IAudioClient"))?
    };
    finish_open(client, true, None, false)
}

fn open(kind: &SourceKind) -> Result<Stream, AudioError> {
    match kind {
        SourceKind::System => {
            let e = enumerator()?;
            // SAFETY: a live enumerator.
            let device = unsafe { e.GetDefaultAudioEndpoint(eRender, eConsole) }
                .map_err(os("default output"))?;
            open_device(&device, true, true)
        }
        SourceKind::Microphone(None) => {
            let e = enumerator()?;
            // SAFETY: a live enumerator.
            let device = unsafe { e.GetDefaultAudioEndpoint(eCapture, eConsole) }
                .map_err(os("default microphone"))?;
            open_device(&device, false, true)
        }
        SourceKind::Microphone(Some(wanted)) => {
            let e = enumerator()?;
            // SAFETY: live enumerator and collection; every item is checked.
            let found = unsafe {
                let devices = e
                    .EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)
                    .map_err(os("capture devices"))?;
                let count = devices.GetCount().unwrap_or(0);
                (0..count).filter_map(|i| devices.Item(i).ok()).find(|d| {
                    device_id(d) == *wanted
                        || friendly_name(d)
                            .to_lowercase()
                            .contains(&wanted.to_lowercase())
                })
            };
            let device = found.ok_or_else(|| {
                AudioError::Unavailable(format!("no microphone matches `{wanted}`"))
            })?;
            open_device(&device, false, false)
        }
        SourceKind::Application(exe) => {
            let pid = find_process(exe)
                .ok_or_else(|| AudioError::Unavailable(format!("{exe} is not running")))?;
            open_process_loopback(pid)
        }
    }
}

/// Why a capture loop ended.
enum Ended {
    Stopped,
    Lost(String),
}

fn capture_loop(
    stream: &Stream,
    kind: &SourceKind,
    sink: &mut dyn AudioSink,
    stop: &AtomicBool,
) -> Ended {
    let mut idle_ticks = 0u32;
    while !stop.load(Ordering::Acquire) {
        // SAFETY: the event belongs to the stream.
        let waited = unsafe { WaitForSingleObject(stream.event, 100) };
        // Packets (also drained on timeout: a stalled event must not lose data).
        loop {
            // SAFETY: documented GetNextPacketSize / GetBuffer / ReleaseBuffer sequence; the
            // slice is only read before the buffer is released.
            let step = unsafe {
                let available = match stream.capture.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(e) => return Ended::Lost(e.message()),
                };
                if available == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut::<u8>();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut qpc = 0u64;
                if let Err(e) = stream.capture.GetBuffer(
                    &raw mut data,
                    &raw mut frames,
                    &raw mut flags,
                    None,
                    Some(&raw mut qpc),
                ) {
                    return if e.code().0 == DEVICE_INVALIDATED {
                        Ended::Lost("device removed".into())
                    } else {
                        Ended::Lost(e.message())
                    };
                }
                let count = frames as usize * CHANNELS;
                let samples =
                    if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                        vec![0.0f32; count]
                    } else {
                        std::slice::from_raw_parts(data.cast::<f32>(), count).to_vec()
                    };
                let _ = stream.capture.ReleaseBuffer(frames);
                AudioChunk {
                    time_ns: (qpc as i64).saturating_mul(100),
                    samples,
                }
            };
            sink.on_audio(step);
        }
        if waited.0 == WAIT_OBJECT_0.0 {
            idle_ticks = 0;
            continue;
        }
        // Quiet for 100 ms: now and then check that what we follow is still there.
        idle_ticks += 1;
        if idle_ticks.is_multiple_of(20) {
            if stream.follows_default && default_changed(stream, kind) {
                return Ended::Lost("default device changed".into());
            }
            if let SourceKind::Application(exe) = kind
                && find_process(exe).is_none()
            {
                return Ended::Lost(format!("{exe} exited"));
            }
        }
    }
    Ended::Stopped
}

fn default_changed(stream: &Stream, kind: &SourceKind) -> bool {
    let flow = if matches!(kind, SourceKind::System) {
        eRender
    } else {
        eCapture
    };
    let Ok(e) = enumerator() else { return false };
    // SAFETY: a live enumerator.
    let current = unsafe { e.GetDefaultAudioEndpoint(flow, eConsole) }
        .ok()
        .map(|d| device_id(&d));
    current.is_some() && current != stream.device_id
}

/// A WASAPI source; see the module documentation.
pub struct WasapiSource {
    kind: SourceKind,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WasapiSource {
    pub fn new(kind: SourceKind) -> Self {
        Self {
            kind,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }
}

fn run(kind: &SourceKind, mut sink: Box<dyn AudioSink>, stop: &AtomicBool) {
    let _com = ComGuard::new();
    let mut lost = false;
    while !stop.load(Ordering::Acquire) {
        let reason = match open(kind) {
            Ok(stream) => {
                if lost {
                    sink.on_event(SourceEvent::Back);
                    lost = false;
                }
                match capture_loop(&stream, kind, sink.as_mut(), stop) {
                    Ended::Stopped => return,
                    Ended::Lost(reason) => reason,
                }
            }
            Err(e) => e.to_string(),
        };
        if !lost {
            tracing::warn!("audio source {kind:?} lost: {reason}");
            sink.on_event(SourceEvent::Lost(reason));
            lost = true;
        }
        // Try again in a second.
        for _ in 0..10 {
            if stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl AudioSource for WasapiSource {
    fn start(&mut self, sink: Box<dyn AudioSink>) -> Result<(), AudioError> {
        if self.thread.is_some() {
            return Ok(());
        }
        self.stop.store(false, Ordering::Release);
        let kind = self.kind.clone();
        let stop = Arc::clone(&self.stop);
        let thread = std::thread::Builder::new()
            .name("audio-source".into())
            .spawn(move || run(&kind, sink, &stop))
            .map_err(|e| AudioError::Os(e.to_string()))?;
        self.thread = Some(thread);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        Ok(())
    }
}

impl Drop for WasapiSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
