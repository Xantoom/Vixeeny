// SPDX-License-Identifier: GPL-3.0-or-later
//! What a crash leaves behind. A fault in native code (a library, the graphics driver) never reaches the panic hook:
//! the last-chance exception filter writes, without allocating, a note (`<date>_<time>_<program>.txt`: the exception,
//! the module and offset it happened at, the thread) and a minidump beside it in the crashes folder, then lets Windows
//! end the process as it would have. [`prune`] keeps the newest [`KEEP`] crashes.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, HMODULE};
use windows::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, WriteFile,
};
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpNormal, MiniDumpWithThreadInfo, MiniDumpWriteDump,
    SetUnhandledExceptionFilter,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};
use windows::core::PCWSTR;

/// Crashes kept (a note and a minidump each).
pub const KEEP: usize = 5;

/// `EXCEPTION_CONTINUE_SEARCH`: Windows goes on as without the filter (its error report, the restart of a registered
/// program).
const CONTINUE_SEARCH: i32 = 0;

/// The longest crashes folder path kept (UTF-16 units, with room for the file name).
const MAX_DIR: usize = 400;

struct Target {
    /// The crashes folder, UTF-16, without its terminating zero.
    dir: Vec<u16>,
    /// `vixeeny-app (settings)`: the executable and what the process does.
    program: String,
}

static TARGET: OnceLock<Target> = OnceLock::new();
/// A crash while writing the files of the first one is left to Windows.
static CRASHING: AtomicBool = AtomicBool::new(false);

/// Installs the filter for this process: its crashes go to `dir`, under `exe` and `program` (`vixeeny-app`,
/// `settings`). Nothing is created until a crash happens.
pub fn catch(dir: &Path, exe: &str, program: &str) {
    use std::os::windows::ffi::OsStrExt;
    let dir: Vec<u16> = dir.as_os_str().encode_wide().collect();
    if dir.len() > MAX_DIR {
        return;
    }
    let program = if exe.ends_with(program) { exe.to_owned() } else { format!("{exe}-{program}") };
    if TARGET.set(Target { dir, program }).is_ok() {
        // SAFETY: the filter only reads `TARGET`, set above for the life of the process.
        unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    }
}

/// Has Windows start this program again after it crashes or hangs (once it ran for a minute: a crash at start does not
/// loop), but not after an update or a reboot (the installer and the start of the session do that).
pub fn restart_after_crash() -> Result<(), crate::PlatformError> {
    use windows::Win32::System::Recovery::{RESTART_NO_PATCH, RESTART_NO_REBOOT, RegisterApplicationRestart};
    // SAFETY: no command line; plain flags.
    unsafe { RegisterApplicationRestart(None, RESTART_NO_PATCH | RESTART_NO_REBOOT) }
        .map_err(|e| crate::PlatformError::Os(format!("RegisterApplicationRestart: {e}")))
}

/// Removes the crashes beyond the newest [`KEEP`] (their note and minidump).
pub fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut stems: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".txt").or_else(|| name.strip_suffix(".dmp")).map(str::to_owned)
        })
        .collect();
    // The names start with the date and time: they sort by age.
    stems.sort();
    stems.dedup();
    let excess = stems.len().saturating_sub(KEEP);
    for old in &stems[..excess] {
        for ext in ["txt", "dmp"] {
            let _ = std::fs::remove_file(dir.join(format!("{old}.{ext}")));
        }
    }
}

/// What an exit code means, when it is a Windows exception (a crash): `None` for an ordinary code.
pub fn describe_exit(code: u32) -> Option<&'static str> {
    Some(match code {
        0xc000_0005 => "access violation",
        0xc000_0409 => "fast fail (a panic, or a corrupted stack)",
        0xc000_00fd => "stack overflow",
        0xc000_0374 => "heap corruption",
        0xc000_001d => "illegal instruction",
        0xc000_0135 => "a DLL was not found",
        0xc000_0142 => "a DLL failed to start",
        0xc000_0017 | 0xc000_012d => "out of memory",
        0xc000_0094 => "integer division by zero",
        0xc000_0006 => "in-page error (a file or the disk failed)",
        0xc000_0026 => "invalid disposition",
        0xc000_0420 => "assertion failure",
        0x8000_0003 => "breakpoint",
        0xe06d_7363 => "C++ exception",
        c if c >= 0xc000_0000 => "exception",
        _ => return None,
    })
}

/// A fixed buffer for text, filled without allocating.
struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Text<N> {
    const fn new() -> Self {
        Self { bytes: [0; N], len: 0 }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl<const N: usize> std::fmt::Write for Text<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        // What does not fit is cut, on a character boundary.
        let mut n = s.len().min(N - self.len);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// `dir`, then `\<stem>.<ext>` when there is a stem, zero-terminated, in `out`.
fn file_path(out: &mut [u16; 600], dir: &[u16], stem: &[u8], ext: &str) -> PCWSTR {
    let mut n = 0;
    let mut push = |u: u16| {
        if n < out.len() - 1 {
            out[n] = u;
            n += 1;
        }
    };
    dir.iter().for_each(|&u| push(u));
    if !stem.is_empty() {
        push(u16::from(b'\\'));
        stem.iter().for_each(|&b| push(u16::from(b)));
        push(u16::from(b'.'));
        ext.bytes().for_each(|b| push(u16::from(b)));
    }
    out[n] = 0;
    PCWSTR(out.as_ptr())
}

/// A new file to write (replacing one of the same name).
///
/// # Safety
/// `path` is a valid zero-terminated string.
unsafe fn create(path: PCWSTR) -> Option<HANDLE> {
    // SAFETY: per the caller; no security attributes or template.
    unsafe { CreateFileW(path, GENERIC_WRITE.0, FILE_SHARE_READ, None, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, None) }
        .ok()
}

/// The module `address` is in, and the address's offset from its start, into `name`.
fn module_of(address: usize, name: &mut Text<128>) -> usize {
    let mut module = HMODULE::default();
    // SAFETY: the flags make the "name" an address; the handle is not counted (nor freed).
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(address as *const u16),
            &mut module,
        )
    }
    .is_ok();
    if !found {
        let _ = name.write_str("?");
        return address;
    }
    let mut path = [0u16; 260];
    // SAFETY: a valid module handle and buffer.
    let n = unsafe { GetModuleFileNameW(Some(module), &mut path) } as usize;
    let file = path[..n].rsplit(|&u| u == u16::from(b'\\')).next().unwrap_or_default();
    for c in char::decode_utf16(file.iter().copied()) {
        let _ = name.write_char(c.unwrap_or('?'));
    }
    address.wrapping_sub(module.0 as usize)
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    if CRASHING.swap(true, Ordering::AcqRel) {
        return CONTINUE_SEARCH;
    }
    let Some(target) = TARGET.get() else {
        return CONTINUE_SEARCH;
    };
    // SAFETY: Windows hands the filter valid exception pointers.
    let record = unsafe { info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) };
    let (code, address) = record.map_or((0, 0), |r| (r.ExceptionCode.0 as u32, r.ExceptionAddress as usize));

    // SAFETY: plain call.
    let t = unsafe { GetLocalTime() };
    let mut stem = Text::<160>::new();
    let _ = write!(
        stem,
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}_{}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, target.program
    );
    let mut module = Text::<128>::new();
    let offset = module_of(address, &mut module);

    let mut note = Text::<1024>::new();
    let _ = writeln!(
        note,
        "Vixeeny {} ({}) crashed: exception 0x{code:08x} ({}) at {}+0x{offset:x}",
        env!("CARGO_PKG_VERSION"),
        target.program,
        describe_exit(code).unwrap_or("exception"),
        std::str::from_utf8(module.as_bytes()).unwrap_or("?"),
    );
    // An access violation says what was read or written, and where.
    if let Some(r) = record
        && code == 0xc000_0005
        && r.NumberParameters >= 2
    {
        let what = match r.ExceptionInformation[0] {
            0 => "reading",
            1 => "writing",
            _ => "executing",
        };
        let _ = writeln!(note, "while {what} 0x{:x}", r.ExceptionInformation[1]);
    }
    // SAFETY: plain call.
    let thread = unsafe { GetCurrentThreadId() };
    let _ = writeln!(note, "thread {thread}");

    let mut path = [0u16; 600];
    // SAFETY: the folder's path, zero-terminated; an existing folder is fine.
    let _ = unsafe { CreateDirectoryW(file_path(&mut path, &target.dir, b"", ""), None) };
    // SAFETY: zero-terminated paths built above; the handles are closed once.
    unsafe {
        if let Some(file) = create(file_path(&mut path, &target.dir, stem.as_bytes(), "txt")) {
            let _ = WriteFile(file, Some(note.as_bytes()), None, None);
            let _ = CloseHandle(file);
        }
        if let Some(file) = create(file_path(&mut path, &target.dir, stem.as_bytes(), "dmp")) {
            let exception = MINIDUMP_EXCEPTION_INFORMATION {
                ThreadId: thread,
                ExceptionPointers: info.cast_mut(),
                ClientPointers: false.into(),
            };
            let _ = MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                file,
                MiniDumpNormal | MiniDumpWithThreadInfo,
                Some(&raw const exception),
                None,
                None,
            );
            let _ = CloseHandle(file);
        }
    }
    CONTINUE_SEARCH
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set in the child process this test starts: it crashes there.
    const CRASH_INTO: &str = "VIXEENY_TEST_CRASH_INTO";

    #[test]
    fn a_crash_leaves_a_note_and_a_minidump() {
        if let Some(dir) = std::env::var_os(CRASH_INTO) {
            catch(Path::new(&dir), "vixeeny-test", "crash");
            // SAFETY: none: the point is the access violation.
            unsafe { std::ptr::null_mut::<u8>().add(16).write_volatile(1) };
            return;
        }
        let dir = std::env::temp_dir().join(format!("vixeeny-crash-test-{}", std::process::id()));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash::tests::a_crash_leaves_a_note_and_a_minidump"])
            .env(CRASH_INTO, &dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        let code = status.code().unwrap() as u32;
        assert_eq!(code, 0xc000_0005);
        assert_eq!(describe_exit(code), Some("access violation"));
        let files: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        let note = files.iter().find(|n| n.ends_with("_vixeeny-test-crash.txt")).unwrap();
        let text = std::fs::read_to_string(dir.join(note)).unwrap();
        assert!(text.contains("exception 0xc0000005 (access violation) at "), "{text}");
        assert!(text.contains(".exe+0x"), "{text}");
        assert!(text.contains("while writing 0x10"), "{text}");
        let dump = dir.join(note.replace(".txt", ".dmp"));
        let size = std::fs::metadata(&dump).unwrap().len();
        // "MDMP", and a dump worth reading.
        assert_eq!(&std::fs::read(&dump).unwrap()[..4], b"MDMP");
        assert!(size > 10_000, "{size}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_newest_crashes_stay() {
        let dir = std::env::temp_dir().join(format!("vixeeny-prune-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for day in 1..=7 {
            for ext in ["txt", "dmp"] {
                std::fs::write(dir.join(format!("2026-10-0{day}_10-00-00_x.{ext}")), "").unwrap();
            }
        }
        prune(&dir);
        let mut left: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left.len(), 2 * KEEP);
        assert!(left[0].starts_with("2026-10-03"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(describe_exit(1), None);
        assert_eq!(describe_exit(0xc000_0409).map(|d| d.starts_with("fast fail")), Some(true));
    }
}
