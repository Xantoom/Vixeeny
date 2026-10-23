// SPDX-License-Identifier: GPL-3.0-or-later
//! Administrator rights without a prompt at each start. A program started as administrator (a
//! game, often) does not let programs without these rights take the focus or the input from
//! it: Vixeeny then needs them too. A scheduled task of the user, made once with Windows'
//! consent, starts Vixeeny with them. Only for a Vixeeny in Program Files, which the user's
//! programs cannot change: one in the user's own folders could be swapped for another
//! program, which the task would then start as administrator.

use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, HANDLE, HLOCAL, LocalFree, VARIANT_FALSE,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE, TOKEN_QUERY, TOKEN_USER,
    TokenElevation, TokenElevationType, TokenElevationTypeLimited, TokenUser,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ITaskFolder, ITaskService, TASK_ACTION_EXEC, TASK_CREATE_OR_UPDATE,
    TASK_ENUM_HIDDEN, TASK_INSTANCES_PARALLEL, TASK_LOGON_INTERACTIVE_TOKEN,
    TASK_RUN_USE_SESSION_ID, TASK_RUNLEVEL_HIGHEST, TaskScheduler,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4, VariantClear};
use windows::core::{BSTR, Interface, PCWSTR, PWSTR, w};

use crate::{PlatformError, Result};

/// The folder of the tasks in the Task Scheduler; each user's task is named after their SID.
const FOLDER: &str = r"\Vixeeny";

/// Administrators and the system manage the folder; every account may read it (to find its
/// own task in it).
const FOLDER_SDDL: &str = "D:P(A;;FA;;;BA)(A;;FA;;;SY)(A;;FR;;;AU)";

/// What this process's account can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rights {
    /// This process has the administrator's rights.
    Elevated,
    /// An administrator's account, without them for now (Windows asks before giving them).
    Limited,
    /// A standard account, or Windows that never asks (UAC off): nothing more to have.
    Standard,
}

fn os(what: &str, e: &windows::core::Error) -> PlatformError {
    PlatformError::Os(format!("{what}: {}", e.message()))
}

/// The access token of this process, closed when dropped.
struct Token(HANDLE);

impl Token {
    fn open() -> Result<Self> {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo-handle of this process; `token` is closed on drop.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }
            .map_err(|e| os("cannot read this process's rights", &e))?;
        Ok(Self(token))
    }

    /// A fixed-size piece of information about the token.
    fn fixed<T: Default>(
        &self,
        class: windows::Win32::Security::TOKEN_INFORMATION_CLASS,
    ) -> Option<T> {
        let mut value = T::default();
        let mut size = 0_u32;
        // SAFETY: `value` holds `size_of::<T>()` bytes, the size the class asks for.
        unsafe {
            GetTokenInformation(
                self.0,
                class,
                Some((&raw mut value).cast()),
                std::mem::size_of::<T>() as u32,
                &raw mut size,
            )
        }
        .ok()?;
        Some(value)
    }

    /// The account, as text (`S-1-5-21-…`).
    fn user_sid(&self) -> Result<String> {
        let failed = || PlatformError::Os("cannot read this process's account".into());
        let mut size = 0_u32;
        // SAFETY: asks for the size only.
        let _ = unsafe { GetTokenInformation(self.0, TokenUser, None, 0, &raw mut size) };
        let mut buffer = vec![0_u64; (size as usize).div_ceil(8)];
        // SAFETY: `buffer` holds `size` bytes, aligned for a `TOKEN_USER`.
        unsafe {
            GetTokenInformation(
                self.0,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                size,
                &raw mut size,
            )
        }
        .map_err(|_| failed())?;
        // SAFETY: filled by the call above.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text = PWSTR::null();
        // SAFETY: a valid SID; `text` is freed below.
        unsafe { ConvertSidToStringSidW(sid, &raw mut text) }.map_err(|_| failed())?;
        // SAFETY: a terminated string from the call above.
        let result = unsafe { text.to_string() }.map_err(|_| failed());
        // SAFETY: allocated by `ConvertSidToStringSidW`, freed once.
        let _ = unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        result
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: opened in `open`, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// What this process's account can have; [`Rights::Standard`] when it cannot be told.
pub fn rights() -> Rights {
    let Ok(token) = Token::open() else {
        return Rights::Standard;
    };
    if token
        .fixed::<TOKEN_ELEVATION>(TokenElevation)
        .is_some_and(|e| e.TokenIsElevated != 0)
    {
        return Rights::Elevated;
    }
    match token.fixed::<TOKEN_ELEVATION_TYPE>(TokenElevationType) {
        Some(kind) if kind == TokenElevationTypeLimited => Rights::Limited,
        _ => Rights::Standard,
    }
}

/// Whether `dir` is in Program Files, which only administrators can change.
pub fn protected_install(dir: &Path) -> bool {
    use windows::Win32::UI::Shell::{FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: a known folder id; the returned string is freed below.
    let Ok(text) = (unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, None) })
    else {
        return false;
    };
    // SAFETY: a terminated string from the call above.
    let program_files = unsafe { text.to_string() }.ok().map(PathBuf::from);
    // SAFETY: allocated by the shell, freed once.
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(text.0.cast())) };
    program_files.is_some_and(|root| inside(dir, &root))
}

/// Whether `dir` is `root` or under it, whatever the case of the letters (Windows' paths).
fn inside(dir: &Path, root: &Path) -> bool {
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let (dir, root) = (parts(dir), parts(root));
    // `..` could lead out of it.
    !root.is_empty() && dir.starts_with(&root) && !dir.iter().any(|c| c == "..")
}

/// COM for this thread while it lives (nothing to undo when the thread already had it in
/// another mode).
struct Com(bool);

impl Com {
    fn new() -> Self {
        // SAFETY: undone on drop when it succeeds.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: matches the successful `CoInitializeEx` of `new`.
            unsafe { CoUninitialize() };
        }
    }
}

/// A `VARIANT` holding a string or a number, cleared when dropped.
struct Variant(VARIANT);

impl Variant {
    fn empty() -> Self {
        Self(VARIANT::default())
    }

    fn text(text: &str) -> Self {
        let mut v = VARIANT::default();
        // SAFETY: an empty `VARIANT` given a type and the value of that type; cleared on drop.
        unsafe {
            let inner = &mut *v.Anonymous.Anonymous;
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(text));
        }
        Self(v)
    }

    fn int(n: i32) -> Self {
        let mut v = VARIANT::default();
        // SAFETY: as in `text`, with a number.
        unsafe {
            let inner = &mut *v.Anonymous.Anonymous;
            inner.vt = VT_I4;
            inner.Anonymous.lVal = n;
        }
        Self(v)
    }
}

impl Drop for Variant {
    fn drop(&mut self) {
        // SAFETY: a `VARIANT` made by this type, cleared once.
        let _ = unsafe { VariantClear(&raw mut self.0) };
    }
}

/// The Task Scheduler of this computer.
fn scheduler() -> Result<ITaskService> {
    // SAFETY: the class and interface go together; COM is set up by the caller (`Com`).
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| os("cannot reach the Task Scheduler", &e))?;
    let none = Variant::empty();
    // SAFETY: empty values: this computer, this account.
    unsafe { service.Connect(&none.0, &none.0, &none.0, &none.0) }
        .map_err(|e| os("cannot reach the Task Scheduler", &e))?;
    Ok(service)
}

fn folder(service: &ITaskService) -> Option<ITaskFolder> {
    // SAFETY: a plain path.
    unsafe { service.GetFolder(&BSTR::from(FOLDER)) }.ok()
}

/// The program the task of this account starts, if it has one.
fn task_program(service: &ITaskService, sid: &str) -> Option<PathBuf> {
    // SAFETY: plain COM calls on live interfaces; the collection counts from 1.
    unsafe {
        let task = folder(service)?.GetTask(&BSTR::from(sid)).ok()?;
        let action: IExecAction = task
            .Definition()
            .ok()?
            .Actions()
            .ok()?
            .get_Item(1)
            .ok()?
            .cast()
            .ok()?;
        let mut path = BSTR::new();
        action.Path(&raw mut path).ok()?;
        Some(PathBuf::from(path.to_string().trim_matches('"')))
    }
}

/// Whether this account has a task that starts `exe` with the administrator's rights.
pub fn task_starts(exe: &Path) -> bool {
    let Ok(sid) = Token::open().and_then(|t| t.user_sid()) else {
        return false;
    };
    let _com = Com::new();
    scheduler()
        .ok()
        .and_then(|service| task_program(&service, &sid))
        .is_some_and(|program| same_file(&program, exe))
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
}

/// Makes (or makes again) this account's task that starts `exe` with the administrator's
/// rights, at once and without asking. Needs these rights; `exe` must be in Program Files.
pub fn register_task(exe: &Path) -> Result<()> {
    let dir = exe
        .parent()
        .filter(|dir| protected_install(dir))
        .ok_or_else(|| PlatformError::Os(format!("{} is not in Program Files", exe.display())))?;
    let sid = Token::open()?.user_sid()?;
    let _com = Com::new();
    let service = scheduler()?;
    let failed = |e: windows::core::Error| os("cannot make the task", &e);
    // SAFETY: plain COM calls on live interfaces; the values outlive them.
    unsafe {
        let folder = match folder(&service) {
            Some(folder) => folder,
            None => service
                .GetFolder(&BSTR::from(r"\"))
                .and_then(|root| {
                    root.CreateFolder(&BSTR::from(&FOLDER[1..]), &Variant::text(FOLDER_SDDL).0)
                })
                .map_err(failed)?,
        };
        let task = service.NewTask(0).map_err(failed)?;
        task.RegistrationInfo()
            .and_then(|info| {
                info.SetDescription(&BSTR::from(
                    "Starts Vixeeny with the administrator's rights, without asking each time.",
                ))
            })
            .map_err(failed)?;
        let principal = task.Principal().map_err(failed)?;
        principal
            .SetUserId(&BSTR::from(sid.as_str()))
            .map_err(failed)?;
        principal
            .SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)
            .map_err(failed)?;
        principal
            .SetRunLevel(TASK_RUNLEVEL_HIGHEST)
            .map_err(failed)?;
        let settings = task.Settings().map_err(failed)?;
        settings
            .SetDisallowStartIfOnBatteries(VARIANT_FALSE)
            .map_err(failed)?;
        settings
            .SetStopIfGoingOnBatteries(VARIANT_FALSE)
            .map_err(failed)?;
        // No time limit; a start while Vixeeny runs reaches it (it opens the settings).
        settings
            .SetExecutionTimeLimit(&BSTR::from("PT0S"))
            .map_err(failed)?;
        settings
            .SetMultipleInstances(TASK_INSTANCES_PARALLEL)
            .map_err(failed)?;
        // A task's program gets a lower priority by default (7): Vixeeny keeps the usual one.
        settings.SetPriority(4).map_err(failed)?;
        let action: IExecAction = task
            .Actions()
            .and_then(|actions| actions.Create(TASK_ACTION_EXEC))
            .and_then(|action| action.cast())
            .map_err(failed)?;
        action
            .SetPath(&BSTR::from(exe.as_os_str().to_string_lossy().as_ref()))
            .map_err(failed)?;
        action
            .SetWorkingDirectory(&BSTR::from(dir.as_os_str().to_string_lossy().as_ref()))
            .map_err(failed)?;
        // The account may read and start its task, not change it: what it starts stays the
        // program in Program Files.
        let sddl = format!("D:P(A;;FA;;;BA)(A;;FA;;;SY)(A;;FRFX;;;{sid})");
        folder
            .RegisterTaskDefinition(
                &BSTR::from(sid.as_str()),
                &task,
                TASK_CREATE_OR_UPDATE.0,
                &Variant::empty().0,
                &Variant::empty().0,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &Variant::text(&sddl).0,
            )
            .map_err(failed)?;
    }
    Ok(())
}

/// Starts this account's task, in this session: Vixeeny starts with the administrator's
/// rights, without asking.
pub fn run_task() -> Result<()> {
    let sid = Token::open()?.user_sid()?;
    let mut session = 0_u32;
    // SAFETY: this process's id; `session` is written.
    unsafe {
        windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(
            GetCurrentProcessId(),
            &raw mut session,
        )
    }
    .map_err(|e| os("cannot read the session", &e))?;
    let _com = Com::new();
    let service = scheduler()?;
    let task = folder(&service)
        // SAFETY: a plain name.
        .and_then(|folder| unsafe { folder.GetTask(&BSTR::from(sid.as_str())) }.ok())
        .ok_or_else(|| PlatformError::Os("this account has no Vixeeny task".into()))?;
    // SAFETY: no parameters; this session and the task's own account.
    unsafe {
        task.RunEx(
            &Variant::empty().0,
            TASK_RUN_USE_SESSION_ID.0,
            session as i32,
            &BSTR::new(),
        )
    }
    .map_err(|e| os("cannot start the task", &e))?;
    Ok(())
}

/// Removes the tasks of every account, and their folder (uninstall). Needs the administrator's
/// rights.
pub fn remove_tasks() -> Result<()> {
    let _com = Com::new();
    let service = scheduler()?;
    let Some(folder) = folder(&service) else {
        return Ok(());
    };
    let failed = |e: windows::core::Error| os("cannot remove the tasks", &e);
    // SAFETY: plain COM calls on live interfaces; the collection counts from 1.
    unsafe {
        let tasks = folder.GetTasks(TASK_ENUM_HIDDEN.0).map_err(failed)?;
        let names = (1..=tasks.Count().map_err(failed)?)
            .filter_map(|i| tasks.get_Item(&Variant::int(i).0).ok()?.Name().ok())
            .collect::<Vec<_>>();
        for name in names {
            folder.DeleteTask(&name, 0).map_err(failed)?;
        }
        service
            .GetFolder(&BSTR::from(r"\"))
            .and_then(|root| root.DeleteFolder(&BSTR::from(&FOLDER[1..]), 0))
            .map_err(failed)?;
    }
    Ok(())
}

/// Why [`run_as_admin`] did not start the program.
#[derive(Debug, thiserror::Error)]
pub enum ElevateError {
    /// The user said no to Windows' question.
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Os(#[from] PlatformError),
}

/// Starts `program` with `args` with the administrator's rights: Windows asks the user first.
/// Returns once it is started (or refused); it is not waited for.
pub fn run_as_admin(program: &Path, args: &str) -> std::result::Result<(), ElevateError> {
    use windows::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let file: Vec<u16> = program
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    let params: Vec<u16> = args.encode_utf16().chain([0]).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: the strings are terminated and outlive the call.
    match unsafe { ShellExecuteExW(&raw mut info) } {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Err(ElevateError::Cancelled),
        Err(e) => Err(os(&format!("cannot start {}", program.display()), &e).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_files_and_what_is_under_it() {
        let root = Path::new(r"C:\Program Files");
        assert!(inside(Path::new(r"C:\Program Files\Vixeeny"), root));
        assert!(inside(Path::new(r"c:\program files\vixeeny"), root));
        assert!(!inside(Path::new(r"C:\Program Files (x86)\Vixeeny"), root));
        assert!(!inside(
            Path::new(r"C:\Users\Jean\AppData\Local\Programs\Vixeeny"),
            root
        ));
        assert!(!inside(
            Path::new(r"C:\Program Files\..\Users\Vixeeny"),
            root
        ));
    }

    #[test]
    fn this_install_is_judged_and_the_rights_read() {
        // A test program runs from the build folder, never from Program Files.
        let exe = std::env::current_exe().unwrap();
        assert!(!protected_install(exe.parent().unwrap()));
        let _ = rights();
        assert!(
            Token::open()
                .unwrap()
                .user_sid()
                .unwrap()
                .starts_with("S-1-")
        );
    }
}
