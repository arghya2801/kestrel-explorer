//! Crash safety: panics at callback boundaries become `None` instead of
//! aborting, panics are logged with a backtrace, and native crashes write a
//! minidump to `%LOCALAPPDATA%\FileExplorerPro\crashes`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpWithThreadInfo,
    MiniDumpWriteDump, SetUnhandledExceptionFilter,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};

/// Runs `f`, turning a panic into `None`. Wrap every window procedure body and
/// every worker job in this: a panic unwinding out of an `extern "system"` fn
/// aborts the process.
pub fn guard<T>(f: impl FnOnce() -> T) -> Option<T> {
    panic::catch_unwind(AssertUnwindSafe(f)).ok()
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("FileExplorerPro")
}

/// Appends one line to the log file. Never fails loudly: logging must not crash the app.
pub fn log(msg: &str) {
    let dir = data_dir();
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("log.txt"))
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = writeln!(f, "[{secs}] {msg}");
    }
}

/// Installs the panic hook and the native crash handler. Call once at startup.
pub fn install() {
    panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let bt = std::backtrace::Backtrace::force_capture();
        log(&format!(
            "panic on thread {:?}: {info}\n{bt}",
            thread.name().unwrap_or("?")
        ));
    }));
    unsafe { SetUnhandledExceptionFilter(Some(on_native_crash)) };
}

unsafe extern "system" fn on_native_crash(ep: *const EXCEPTION_POINTERS) -> i32 {
    let path = write_minidump(ep);
    log(&format!("native crash, dump: {path:?}"));
    EXCEPTION_CONTINUE_SEARCH
}

/// Writes a minidump of this process (all thread stacks). `ep` may be null.
pub fn write_minidump(ep: *const EXCEPTION_POINTERS) -> Option<PathBuf> {
    let dir = data_dir().join("crashes");
    fs::create_dir_all(&dir).ok()?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let path = dir.join(format!("{secs}-{}.dmp", std::process::id()));
    let file = fs::File::create(&path).ok()?;
    let info = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: unsafe { GetCurrentThreadId() },
        ExceptionPointers: ep as *mut _,
        ClientPointers: false.into(),
    };
    unsafe {
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            HANDLE(file.as_raw_handle()),
            MiniDumpWithThreadInfo,
            (!ep.is_null()).then_some(&info as *const _),
            None,
            None,
        )
        .ok()?;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_passes_value_through() {
        assert_eq!(guard(|| 5), Some(5));
    }

    #[test]
    fn guard_turns_panic_into_none() {
        assert_eq!(guard(|| -> i32 { panic!("boom") }), None);
    }

    #[test]
    fn minidump_is_written() {
        let path = write_minidump(std::ptr::null()).expect("dump written");
        assert!(fs::metadata(&path).unwrap().len() > 0);
        fs::remove_file(path).unwrap();
    }
}
