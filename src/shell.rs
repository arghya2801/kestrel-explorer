//! Shell calls. These can block or load third-party code, so they only run
//! on worker threads, each with its own STA COM apartment.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_CANCELLED, ERROR_NO_ASSOCIATION, HWND};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::{
    IShellLinkW, SEE_MASK_DEFAULT, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, Interface, PCWSTR, w};

/// COM for the current worker thread, released on drop.
pub(crate) struct Com;
impl Com {
    pub(crate) fn init() -> Self {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        }
        Com
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// Opens `path` like a double-click in Explorer. A shortcut to a folder is not
/// opened; its target comes back as `Ok(Some(dir))` so we can show it ourselves.
pub fn open(path: &Path, owner: HWND) -> Result<Option<PathBuf>, String> {
    let _com = Com::init();
    if is_shortcut(path)
        && let Some(target) = shortcut_target(path)
        && target.is_dir()
    {
        return Ok(Some(target));
    }
    let file = HSTRING::from(path.as_os_str());
    let execute = |verb: PCWSTR, mask| {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            // NOASYNC: the thread ends right after this call, so it must not return early.
            fMask: SEE_MASK_NOASYNC | mask,
            hwnd: owner,
            lpVerb: verb,
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut info) }
    };
    // FLAG_NO_UI: errors come back to us (status bar) instead of a modal dialog.
    let result = match execute(PCWSTR::null(), SEE_MASK_FLAG_NO_UI) {
        // No app for this file type: Windows' "How do you want to open this file?" is the useful answer.
        Err(e) if e.code() == ERROR_NO_ASSOCIATION.to_hresult() => execute(w!("openas"), SEE_MASK_DEFAULT),
        r => r,
    };
    match result {
        Ok(()) => Ok(None),
        // The user closed the "open with" dialog.
        Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Ok(None),
        Err(e) => Err(format!("Can't open {}: {}", path.display(), e.message())),
    }
}

pub fn is_shortcut(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
}

/// Where a `.lnk` points, without resolving moved targets (that can search the disk).
pub fn shortcut_target(lnk: &Path) -> Option<PathBuf> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).ok()?;
        let mut buf = vec![0u16; 32768];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let len = buf.iter().position(|&c| c == 0)?;
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }
}

#[cfg(test)]
pub(crate) fn create_shortcut(lnk: &Path, target: &Path) {
    let _com = Com::init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
        link.SetPath(&HSTRING::from(target.as_os_str())).unwrap();
        link.cast::<IPersistFile>().unwrap().Save(&HSTRING::from(lnk.as_os_str()), true).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_extension_is_case_insensitive() {
        assert!(is_shortcut(Path::new(r"C:\x\Thing.LNK")));
        assert!(!is_shortcut(Path::new(r"C:\x\lnk")));
        assert!(!is_shortcut(Path::new(r"C:\x\a.lnk.txt")));
    }

    #[test]
    fn shortcut_to_folder_opens_as_navigation() {
        let dir = std::env::temp_dir().join(format!("fep-shell-{}", std::process::id()));
        let target = dir.join("target dir");
        std::fs::create_dir_all(&target).unwrap();
        let lnk = dir.join("go there.lnk");
        create_shortcut(&lnk, &target);

        let _com = Com::init();
        // %TEMP% may be an 8.3 short path; the shortcut stores the long one.
        let same = |p: PathBuf| std::fs::canonicalize(p).unwrap() == std::fs::canonicalize(&target).unwrap();
        assert!(same(shortcut_target(&lnk).unwrap()));
        assert!(same(open(&lnk, HWND::default()).unwrap().unwrap()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_file_reports_an_error() {
        let r = open(Path::new(r"C:\definitely\not\here.txt"), HWND::default());
        assert!(r.as_ref().is_err_and(|e| e.starts_with(r"Can't open C:\definitely")), "{r:?}");
    }
}
