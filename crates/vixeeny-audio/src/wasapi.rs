// SPDX-License-Identifier: GPL-3.0-or-later
//! WASAPI sources (Windows): the speakers' loopback, microphones, and per-application loopback (Windows 10 2004+
//! "process loopback": the audio one process tree plays, whatever the speakers do).
//!
//! All streams are opened as 48 kHz float with `AUTOCONVERTPCM`, so Windows does the sample-rate conversion. Speakers
//! and microphones keep their own layout (stereo, 5.1 or 7.1, up to what the track asked for); process loopback is
//! stereo. Each packet is dated with the QPC time the driver gives (100 ns units), the same clock as
//! `vixeeny_platform::monotonic_ns`.
//!
//! A source that fails (device unplugged) reports `Lost`, an application that closes `Exited`, and tries again each
//! time Windows reports a device or a new audio session: the track is silent meanwhile and recovers by itself. Nothing
//! is polled: the capture thread sleeps until the driver, the device list, the followed process or `stop` wakes it.

use std::sync::Arc;
use std::thread::JoinHandle;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, PROPERTYKEY, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync, DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow,
    ERole, IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient, IAudioSessionControl,
    IAudioSessionControl2, IAudioSessionManager2, IAudioSessionNotification, IAudioSessionNotification_Impl, IMMDevice,
    IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0_0};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, INFINITE, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, ResetEvent, SetEvent,
    WaitForMultipleObjects, WaitForSingleObject,
};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{GUID, Interface, PCWSTR, PWSTR, Ref, implement};

use crate::{
    AudioChunk, AudioError, AudioSink, AudioSource, CHANNELS, SAMPLE_RATE, SourceEvent, SourceKind, track_channels,
};

/// `AUDCLNT_E_DEVICE_INVALIDATED`.
const DEVICE_INVALIDATED: i32 = 0x8889_0004_u32 as i32;

pub(crate) fn os(what: &str) -> impl FnOnce(windows::core::Error) -> AudioError + '_ {
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

pub(crate) struct ComGuard;

impl ComGuard {
    pub(crate) fn new() -> Self {
        // SAFETY: balanced by `CoUninitialize` in `Drop`; an already initialised thread (S_FALSE) is fine, a different
        // apartment model (an error) leaves COM usable as it is.
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

pub(crate) fn enumerator() -> Result<IMMDeviceEnumerator, AudioError> {
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
    // SAFETY: the property store and the value are used and cleared within this function; the string pointer is only
    // read while the value is alive and only when its type says it is one.
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
            PWSTR(inner.Anonymous.pwszVal.0).to_string().unwrap_or_default()
        } else {
            String::new()
        };
        let _ = windows::Win32::System::Com::StructuredStorage::PropVariantClear(&raw mut value);
        name
    }
}

/// The active capture devices.
pub fn list_microphones() -> Vec<DeviceInfo> {
    list_endpoints(eCapture)
}

/// The active output devices (what `out:` sources record).
pub fn list_outputs() -> Vec<DeviceInfo> {
    list_endpoints(eRender)
}

fn list_endpoints(flow: windows::Win32::Media::Audio::EDataFlow) -> Vec<DeviceInfo> {
    let _com = ComGuard::new();
    let Ok(enumerator) = enumerator() else {
        return Vec::new();
    };
    // SAFETY: plain COM calls on live interfaces; each result is checked.
    unsafe {
        let Ok(devices) = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else {
            return Vec::new();
        };
        let count = devices.GetCount().unwrap_or(0);
        (0..count)
            .filter_map(|i| devices.Item(i).ok())
            .map(|d| DeviceInfo { id: device_id(&d), name: friendly_name(&d) })
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

/// The applications that have an audio session on any output device.
pub fn list_applications() -> Vec<AppInfo> {
    let _com = ComGuard::new();
    let Ok(enumerator) = enumerator() else {
        return Vec::new();
    };
    let mut apps: Vec<AppInfo> = Vec::new();
    // SAFETY: plain COM calls on live interfaces; each result is checked.
    unsafe {
        let Ok(devices) = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
            return Vec::new();
        };
        for device in (0..devices.GetCount().unwrap_or(0)).filter_map(|i| devices.Item(i).ok()) {
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = manager.GetSessionEnumerator() else {
                continue;
            };
            for i in 0..sessions.GetCount().unwrap_or(0) {
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
                let active =
                    control.GetState().is_ok_and(|s| s == windows::Win32::Media::Audio::AudioSessionStateActive);
                match apps.iter_mut().find(|a| a.exe.eq_ignore_ascii_case(&exe)) {
                    Some(a) => a.active |= active,
                    None => apps.push(AppInfo { pid, exe, active }),
                }
            }
        }
    }
    apps
}

/// The root process of `exe` (the one whose parent is not the same program): its tree is what process loopback
/// captures.
fn find_process(exe: &str) -> Option<u32> {
    // SAFETY: a Toolhelp snapshot walked with the documented First/Next calls; handle closed.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
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

/// The capture format: 48 kHz float. More than two channels need the extensible form, with the speaker mask (FL FR FC
/// LFE BL BR [SL SR]).
pub(crate) fn wave_format(channels: usize) -> WAVEFORMATEXTENSIBLE {
    let bytes_per_frame = (channels * 4) as u16;
    let mut format = WAVEFORMATEX {
        wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
        nChannels: channels as u16,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * u32::from(bytes_per_frame),
        nBlockAlign: bytes_per_frame,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    let mut mask = 0;
    if channels > 2 {
        format.wFormatTag = 0xfffe; // WAVE_FORMAT_EXTENSIBLE
        format.cbSize = 22;
        mask = if channels >= 8 { 0x63f } else { 0x3f };
    }
    WAVEFORMATEXTENSIBLE {
        Format: format,
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
        dwChannelMask: mask,
        // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        SubFormat: GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71),
    }
}

/// Channels of the device's own mix format.
fn mix_channels(client: &IAudioClient) -> Option<usize> {
    // SAFETY: GetMixFormat returns a CoTaskMem block that is read once and freed.
    unsafe {
        let format = client.GetMixFormat().ok()?;
        if format.is_null() {
            return None;
        }
        let channels = usize::from((*format).nChannels);
        CoTaskMemFree(Some(format.cast_const().cast()));
        Some(channels)
    }
}

/// Completion handler of `ActivateAudioInterfaceAsync`: signals an event.
#[implement(IActivateAudioInterfaceCompletionHandler, windows::Win32::System::Com::IAgileObject)]
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
    follow: Follow,
    /// Channels of the samples this stream delivers.
    channels: usize,
    /// For an application: its process, signalled when it ends.
    process: Option<HANDLE>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the client was started by `open`; the handles are ours.
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
            if let Some(process) = self.process {
                let _ = CloseHandle(process);
            }
        }
    }
}

/// What a stream reads from its endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    /// What a microphone hears.
    Capture,
    /// What a render endpoint (speakers, a process) plays.
    Loopback,
}

/// Whether a stream ends when Windows picks another default device, so that it opens again on the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Follow {
    Default,
    Fixed,
}

/// The endpoint a speakers or microphone source reads.
struct Endpoint {
    device: IMMDevice,
    read: Read,
    follow: Follow,
}

/// Initialises `client` for event-driven float capture.
fn initialise(client: &IAudioClient, read: Read, channels: usize) -> Result<HANDLE, AudioError> {
    let format = wave_format(channels);
    let mut flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if read == Read::Loopback {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    // SAFETY: `format` outlives the call; the event is created, attached and returned.
    unsafe {
        client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, (&raw const format).cast::<WAVEFORMATEX>(), None)
            .map_err(os("Initialize"))?;
        let event = CreateEventW(None, false, false, PCWSTR::null()).map_err(os("CreateEvent"))?;
        client.SetEventHandle(event).map_err(os("SetEventHandle"))?;
        Ok(event)
    }
}

fn finish_open(
    client: IAudioClient,
    read: Read,
    device_id: Option<String>,
    follow: Follow,
    max_channels: usize,
) -> Result<Stream, AudioError> {
    // Endpoints keep their layout; the process loopback has no mix format and stays stereo.
    let channels = if device_id.is_some() {
        track_channels(mix_channels(&client).unwrap_or(CHANNELS), max_channels)
    } else {
        CHANNELS
    };
    let event = initialise(&client, read, channels)?;
    // SAFETY: the client is initialised; the service is requested for the capture interface.
    let capture: IAudioCaptureClient = unsafe { client.GetService() }.map_err(os("GetService(capture)"))?;
    // SAFETY: as above.
    unsafe { client.Start() }.map_err(os("Start"))?;
    Ok(Stream { client, capture, event, device_id, follow, channels, process: None })
}

fn open_device(endpoint: &Endpoint, max_channels: usize) -> Result<Stream, AudioError> {
    // SAFETY: activating the audio client of a live endpoint.
    let client: IAudioClient = unsafe { endpoint.device.Activate(CLSCTX_ALL, None) }.map_err(os("Activate"))?;
    finish_open(client, endpoint.read, Some(device_id(&endpoint.device)), endpoint.follow, max_channels)
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
    // A `PROPVARIANT` is cleared when dropped, which frees its blob: this one points at `params`, on the stack, so it
    // is never dropped.
    let mut variant = std::mem::ManuallyDrop::new(PROPVARIANT::default());
    // SAFETY: the blob points at `params`, which outlives the activation call.
    unsafe {
        let inner: &mut PROPVARIANT_0_0 = &mut variant.Anonymous.Anonymous;
        inner.vt = VT_BLOB;
        inner.Anonymous.blob.cbSize = size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
        inner.Anonymous.blob.pBlobData = (&raw const params).cast_mut().cast();
    }
    // SAFETY: a manual-reset-free event, closed below.
    let done = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map_err(os("CreateEvent"))?;
    let handler: IActivateAudioInterfaceCompletionHandler = ActivateHandler { done }.into();
    let iid: GUID = IAudioClient::IID;
    // SAFETY: the parameters live until the wait below ends; the handler is agile.
    let result = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &raw const iid,
            Some(&raw const *variant),
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
            return Err(AudioError::Os("process loopback activation timed out".into()));
        }
        let mut hr = windows::core::HRESULT(0);
        let mut unknown = None;
        operation.GetActivateResult(&raw mut hr, &raw mut unknown).map_err(os("GetActivateResult"))?;
        hr.ok().map_err(os("activation"))?;
        unknown
            .ok_or_else(|| AudioError::Os("no audio client".into()))?
            .cast::<IAudioClient>()
            .map_err(os("IAudioClient"))?
    };
    let mut stream = finish_open(client, Read::Loopback, None, Follow::Fixed, CHANNELS)?;
    // SAFETY: a plain call; the handle is closed with the stream.
    stream.process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.ok();
    Ok(stream)
}

/// The endpoint a speakers/microphone source reads, `None` for an application.
fn endpoint(kind: &SourceKind) -> Result<Option<Endpoint>, AudioError> {
    let at = |device, read, follow| Some(Endpoint { device, read, follow });
    Ok(match kind {
        SourceKind::System => {
            let e = enumerator()?;
            // SAFETY: a live enumerator.
            let device = unsafe { e.GetDefaultAudioEndpoint(eRender, eConsole) }.map_err(os("default output"))?;
            at(device, Read::Loopback, Follow::Default)
        }
        SourceKind::Output(wanted) => {
            let e = enumerator()?;
            // SAFETY: live enumerator and collection; every item is checked.
            let found = unsafe {
                let devices = e.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).map_err(os("render devices"))?;
                let count = devices.GetCount().unwrap_or(0);
                (0..count).filter_map(|i| devices.Item(i).ok()).find(|d| {
                    device_id(d) == *wanted || friendly_name(d).to_lowercase().contains(&wanted.to_lowercase())
                })
            };
            let device =
                found.ok_or_else(|| AudioError::Unavailable(format!("no output device matches `{wanted}`")))?;
            at(device, Read::Loopback, Follow::Fixed)
        }
        SourceKind::Microphone(None) => {
            let e = enumerator()?;
            // SAFETY: a live enumerator.
            let device = unsafe { e.GetDefaultAudioEndpoint(eCapture, eConsole) }.map_err(os("default microphone"))?;
            at(device, Read::Capture, Follow::Default)
        }
        SourceKind::Microphone(Some(wanted)) | SourceKind::Input(wanted) => {
            let e = enumerator()?;
            // SAFETY: live enumerator and collection; every item is checked.
            let found = unsafe {
                let devices = e.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE).map_err(os("capture devices"))?;
                let count = devices.GetCount().unwrap_or(0);
                (0..count).filter_map(|i| devices.Item(i).ok()).find(|d| {
                    device_id(d) == *wanted || friendly_name(d).to_lowercase().contains(&wanted.to_lowercase())
                })
            };
            let device = found.ok_or_else(|| AudioError::Unavailable(format!("no microphone matches `{wanted}`")))?;
            at(device, Read::Capture, Follow::Fixed)
        }
        SourceKind::Application(_) => None,
    })
}

fn open(kind: &SourceKind, max_channels: usize) -> Result<Stream, AudioError> {
    if let Some(endpoint) = endpoint(kind)? {
        return open_device(&endpoint, max_channels);
    }
    let SourceKind::Application(exe) = kind else {
        return Err(AudioError::Unsupported);
    };
    let pid = find_process(exe).ok_or_else(|| AudioError::Unavailable(format!("{exe} is not running")))?;
    open_process_loopback(pid)
}

/// The channels `kind` would deliver now when allowed up to `max`: its device's own layout (stereo, 5.1 or 7.1), stereo
/// for an application or when the device cannot be examined.
pub fn source_channels(kind: &SourceKind, max: usize) -> usize {
    let _com = ComGuard::new();
    let Ok(Some(Endpoint { device, .. })) = endpoint(kind) else {
        return CHANNELS;
    };
    // SAFETY: activating the audio client of a live endpoint, only to read its mix format.
    let client: Option<IAudioClient> = unsafe { device.Activate(CLSCTX_ALL, None) }.ok();
    client.and_then(|c| mix_channels(&c)).map_or(CHANNELS, |n| track_channels(n, max))
}

/// A Windows event shared between threads.
struct Event(HANDLE);

// SAFETY: an event handle may be waited on and signalled from any thread.
unsafe impl Send for Event {}
// SAFETY: as above.
unsafe impl Sync for Event {}

impl Event {
    /// `manual`: stays signalled until reset (otherwise one wait consumes it).
    fn new(manual: bool) -> Result<Self, AudioError> {
        // SAFETY: a plain call; the handle is closed in `Drop`.
        unsafe { CreateEventW(None, manual, false, PCWSTR::null()) }.map(Self).map_err(os("CreateEvent"))
    }

    fn set(&self) {
        // SAFETY: a live event.
        let _ = unsafe { SetEvent(self.0) };
    }

    fn reset(&self) {
        // SAFETY: a live event.
        let _ = unsafe { ResetEvent(self.0) };
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: our handle, used by no one any more.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Signals an event when Windows reports a device change or a new audio session.
#[implement(IMMNotificationClient, IAudioSessionNotification)]
struct Changes(Arc<Event>);

impl IMMNotificationClient_Impl for Changes_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        self.0.set();
        Ok(())
    }

    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.0.set();
        Ok(())
    }

    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.0.set();
        Ok(())
    }

    fn OnDefaultDeviceChanged(&self, _: EDataFlow, role: ERole, _: &PCWSTR) -> windows::core::Result<()> {
        if role == eConsole {
            self.0.set();
        }
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

impl IAudioSessionNotification_Impl for Changes_Impl {
    fn OnSessionCreated(&self, _: Ref<'_, IAudioSessionControl>) -> windows::core::Result<()> {
        self.0.set();
        Ok(())
    }
}

/// What a capture thread listens to besides its stream: the device list, and (while an application is not playing) the
/// new audio sessions of every output.
struct Listening {
    enumerator: IMMDeviceEnumerator,
    changes: IMMNotificationClient,
    sessions: Vec<IAudioSessionManager2>,
    /// Signalled by `changes`.
    changed: Arc<Event>,
}

impl Listening {
    fn new() -> Result<Self, AudioError> {
        let changed = Arc::new(Event::new(false)?);
        let changes: IMMNotificationClient = Changes(Arc::clone(&changed)).into();
        let enumerator = enumerator()?;
        // SAFETY: unregistered in `Drop`.
        unsafe { enumerator.RegisterEndpointNotificationCallback(&changes) }
            .map_err(os("RegisterEndpointNotificationCallback"))?;
        Ok(Self { enumerator, changes, sessions: Vec::new(), changed })
    }

    /// Listens to the sessions that start on the outputs present now (an application that starts playing), replacing
    /// the previous registrations.
    fn follow_sessions(&mut self) {
        self.forget_sessions();
        let Ok(notify) = self.changes.cast::<IAudioSessionNotification>() else {
            return;
        };
        // SAFETY: live enumerator and collection; each manager is unregistered later. Windows only reports new sessions
        // once the session list was asked for.
        unsafe {
            let Ok(devices) = self.enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
                return;
            };
            for i in 0..devices.GetCount().unwrap_or(0) {
                let Ok(device) = devices.Item(i) else {
                    continue;
                };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                    continue;
                };
                let _ = manager.GetSessionEnumerator();
                if manager.RegisterSessionNotification(&notify).is_ok() {
                    self.sessions.push(manager);
                }
            }
        }
    }

    fn forget_sessions(&mut self) {
        let Ok(notify) = self.changes.cast::<IAudioSessionNotification>() else {
            return;
        };
        for manager in self.sessions.drain(..) {
            // SAFETY: registered by `follow_sessions`.
            let _ = unsafe { manager.UnregisterSessionNotification(&notify) };
        }
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.forget_sessions();
        // SAFETY: registered by `new`.
        let _ = unsafe { self.enumerator.UnregisterEndpointNotificationCallback(&self.changes) };
    }
}

/// Why a capture loop ended.
enum Ended {
    Stopped,
    Lost(String),
    /// The application closed.
    Exited,
}

fn capture_loop(stream: &Stream, kind: &SourceKind, sink: &mut dyn AudioSink, stop: &Event, changed: &Event) -> Ended {
    let mut handles = vec![stream.event, stop.0, changed.0];
    handles.extend(stream.process);
    loop {
        // SAFETY: live handles, owned by the stream, the source and the listener.
        let waited = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        match waited.0.wrapping_sub(WAIT_OBJECT_0.0) {
            0 => {}
            1 => return Ended::Stopped,
            2 => {
                if stream.follow == Follow::Default && default_changed(stream, kind) {
                    return Ended::Lost("default device changed".into());
                }
                // A device that went away fails here.
                // SAFETY: a started client.
                if let Err(e) = unsafe { stream.client.GetCurrentPadding() } {
                    return Ended::Lost(e.message());
                }
                continue;
            }
            3 => return Ended::Exited,
            _ => return Ended::Lost("wait failed".into()),
        }
        loop {
            // SAFETY: documented GetNextPacketSize / GetBuffer / ReleaseBuffer sequence; the slice is only read before
            // the buffer is released.
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
                if let Err(e) =
                    stream.capture.GetBuffer(&raw mut data, &raw mut frames, &raw mut flags, None, Some(&raw mut qpc))
                {
                    return if e.code().0 == DEVICE_INVALIDATED {
                        Ended::Lost("device removed".into())
                    } else {
                        Ended::Lost(e.message())
                    };
                }
                let count = frames as usize * stream.channels;
                let samples = if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                    vec![0.0f32; count]
                } else {
                    std::slice::from_raw_parts(data.cast::<f32>(), count).to_vec()
                };
                let _ = stream.capture.ReleaseBuffer(frames);
                AudioChunk { time_ns: (qpc as i64).saturating_mul(100), channels: stream.channels, samples }
            };
            sink.on_audio(step);
        }
    }
}

fn default_changed(stream: &Stream, kind: &SourceKind) -> bool {
    let flow = if matches!(kind, SourceKind::System) { eRender } else { eCapture };
    let Ok(e) = enumerator() else { return false };
    // SAFETY: a live enumerator.
    let current = unsafe { e.GetDefaultAudioEndpoint(flow, eConsole) }.ok().map(|d| device_id(&d));
    current.is_some() && current != stream.device_id
}

/// A WASAPI source; see the module documentation.
pub struct WasapiSource {
    kind: SourceKind,
    /// The most channels the sink wants (a stereo track gains nothing from 7.1).
    max_channels: usize,
    /// Set to end the capture thread.
    stop: Option<Arc<Event>>,
    thread: Option<JoinHandle<()>>,
}

impl WasapiSource {
    pub fn new(kind: SourceKind) -> Self {
        Self::with_max_channels(kind, CHANNELS)
    }

    /// A source that keeps its device's surround layout, up to `max_channels`.
    pub fn with_max_channels(kind: SourceKind, max_channels: usize) -> Self {
        Self { kind, max_channels, stop: None, thread: None }
    }
}

/// Has Windows schedule this thread as audio work (the "Audio" task of the multimedia class scheduler): a busy game
/// does not delay it into gaps. Undone when dropped.
struct AudioPriority(HANDLE);

impl AudioPriority {
    fn new() -> Option<Self> {
        let mut index = 0;
        // SAFETY: a constant task name and a valid out-pointer.
        unsafe { AvSetMmThreadCharacteristicsW(windows::core::w!("Audio"), &raw mut index) }.ok().map(Self)
    }
}

impl Drop for AudioPriority {
    fn drop(&mut self) {
        // SAFETY: the handle `AvSetMmThreadCharacteristicsW` returned on this thread.
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}

fn run(kind: &SourceKind, max_channels: usize, mut sink: Box<dyn AudioSink>, stop: &Event) {
    let _com = ComGuard::new();
    let _priority = AudioPriority::new();
    let mut listening = match Listening::new() {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("audio source {kind:?}: no device notifications: {e}");
            sink.on_event(SourceEvent::Lost(e.to_string()));
            return;
        }
    };
    let mut lost = false;
    loop {
        let ended = match open(kind, max_channels) {
            Ok(stream) => {
                listening.forget_sessions();
                if lost {
                    sink.on_event(SourceEvent::Back);
                    lost = false;
                }
                match capture_loop(&stream, kind, sink.as_mut(), stop, &listening.changed) {
                    Ended::Stopped => return,
                    ended => ended,
                }
            }
            Err(e) => Ended::Lost(e.to_string()),
        };
        if !lost {
            match ended {
                Ended::Lost(reason) => {
                    tracing::warn!("audio source {kind:?} lost: {reason}");
                    sink.on_event(SourceEvent::Lost(reason));
                }
                _ => {
                    tracing::info!("audio source {kind:?}: the application closed");
                    sink.on_event(SourceEvent::Exited);
                }
            }
            lost = true;
        }
        // Try again when a device comes or goes, or (for an application) a session starts.
        if matches!(kind, SourceKind::Application(_)) {
            listening.follow_sessions();
        }
        // SAFETY: live events.
        let waited = unsafe { WaitForMultipleObjects(&[stop.0, listening.changed.0], false, INFINITE) };
        if waited.0 != WAIT_OBJECT_0.0 + 1 {
            return;
        }
    }
}

impl AudioSource for WasapiSource {
    fn start(&mut self, sink: Box<dyn AudioSink>) -> Result<(), AudioError> {
        if self.thread.is_some() {
            return Ok(());
        }
        let stop = match &self.stop {
            Some(stop) => Arc::clone(stop),
            None => Arc::clone(self.stop.insert(Arc::new(Event::new(true)?))),
        };
        stop.reset();
        let kind = self.kind.clone();
        let max_channels = self.max_channels;
        let thread = std::thread::Builder::new()
            .name("audio-source".into())
            .spawn(move || run(&kind, max_channels, sink, &stop))
            .map_err(|e| AudioError::Os(e.to_string()))?;
        self.thread = Some(thread);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        if let Some(stop) = &self.stop {
            stop.set();
        }
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

#[cfg(test)]
mod tests {
    use windows::Win32::System::Memory::{GetProcessHeap, HEAP_FLAGS, HeapValidate};

    use super::*;

    /// Opening an application's sound leaves the heap whole (its activation parameters are on the stack, never freed).
    #[test]
    fn a_process_loopback_leaves_the_heap_whole() {
        let _com = ComGuard::new();
        let stream = open_process_loopback(std::process::id()).expect("process loopback");
        drop(stream);
        // SAFETY: plain calls on this process's heap.
        assert!(unsafe { HeapValidate(GetProcessHeap().expect("heap"), HEAP_FLAGS(0), None) }.as_bool());
    }
}
