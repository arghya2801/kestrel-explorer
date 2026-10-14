//! Shell calls. These can block or load third-party code, so they only run
//! on worker threads, each with its own STA COM apartment.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_CANCELLED, ERROR_NO_ASSOCIATION, HWND};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::{
    IShellLinkW, SEE_MASK_DEFAULT, SEE_MASK_FLAG_NO_UI, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC,
    SHELLEXECUTEINFOW, SHParseDisplayName, ShellExecuteExW, ShellLink,
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
    // A .zip opens as a folder, as in Explorer; another app is under Open with in the full menu.
    if is_zip(path) && path.is_file() {
        return Ok(Some(path.to_path_buf()));
    }
    let file = HSTRING::from(path.as_os_str());
    // Inside an archive there is no file path: open the shell item, which extracts it first.
    let pidl = in_archive(path).then(|| unsafe {
        let mut pidl = std::ptr::null_mut();
        SHParseDisplayName(&file, None, &mut pidl, 0, None).ok().map(|_| pidl)
    });
    let pidl = pidl.flatten();
    let execute = |verb: PCWSTR, mask| {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            // NOASYNC: the thread ends right after this call, so it must not return early.
            fMask: SEE_MASK_NOASYNC
                | mask
                | if pidl.is_some() { SEE_MASK_INVOKEIDLIST } else { SEE_MASK_DEFAULT },
            hwnd: owner,
            lpVerb: verb,
            lpFile: if pidl.is_some() { PCWSTR::null() } else { PCWSTR(file.as_ptr()) },
            lpIDList: pidl.unwrap_or(std::ptr::null_mut()) as *mut _,
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut info) }
    };
    let result = open_with(execute);
    if let Some(p) = pidl {
        unsafe { windows::Win32::UI::Shell::ILFree(Some(p)) };
    }
    match result {
        Ok(()) => Ok(None),
        // The user closed the "open with" dialog.
        Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Ok(None),
        Err(e) => Err(format!("Can't open {}: {}", path.display(), e.message())),
    }
}

fn open_with(execute: impl Fn(PCWSTR, u32) -> windows::core::Result<()>) -> windows::core::Result<()> {
    // FLAG_NO_UI: errors come back to us (status bar) instead of a modal dialog.
    match execute(PCWSTR::null(), SEE_MASK_FLAG_NO_UI) {
        // No app for this file type: Windows' "How do you want to open this file?" is the useful answer.
        Err(e) if e.code() == ERROR_NO_ASSOCIATION.to_hresult() => execute(w!("openas"), SEE_MASK_DEFAULT),
        r => r,
    }
}

pub fn is_zip(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// Whether `path` is inside a .zip (or is one), which only the shell can list.
/// Checks the disk: worker threads only.
pub fn in_archive(path: &Path) -> bool {
    path.ancestors().any(|a| is_zip(a) && a.is_file())
}

/// Lists an archive folder through the shell (Windows' built-in zip support).
pub fn list_archive(dir: &Path) -> std::io::Result<crate::listing::Snapshot> {
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
    use windows::Win32::System::SystemServices::SFGAO_FOLDER;
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2, SHCreateItemFromParsingName,
        SIGDN_PARENTRELATIVEPARSING,
    };
    // System.Size and System.DateModified.
    const FMTID: windows::core::GUID = windows::core::GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac);
    const SIZE: PROPERTYKEY = PROPERTYKEY { fmtid: FMTID, pid: 12 };
    const MODIFIED: PROPERTYKEY = PROPERTYKEY { fmtid: FMTID, pid: 14 };
    let _com = Com::init();
    let mut snap = crate::listing::Snapshot::new(dir.to_path_buf());
    unsafe {
        let folder: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(dir.as_os_str()), None)?;
        let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
        loop {
            let mut item = [None];
            if items.Next(&mut item, None).is_err() {
                break;
            }
            let Some(item) = item[0].take() else { break };
            let Ok(name) = item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING) else { continue };
            let wide = name.as_wide().to_vec();
            windows::Win32::System::Com::CoTaskMemFree(Some(name.0 as _));
            let folder = item.GetAttributes(SFGAO_FOLDER).is_ok_and(|a| a.contains(SFGAO_FOLDER));
            let (size, modified) = match item.cast::<IShellItem2>() {
                Ok(i) => (
                    i.GetUInt64(&SIZE).unwrap_or(0),
                    i.GetFileTime(&MODIFIED)
                        .map(|t| (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64)
                        .unwrap_or(0),
                ),
                Err(_) => (0, 0),
            };
            let attrs = if folder { FILE_ATTRIBUTE_DIRECTORY.0 } else { 0 };
            snap.add(&wide, attrs, size, modified);
        }
    }
    Ok(snap)
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

/// Launch an interactive terminal with the directory passed as process state, never as shell code.
pub fn terminal(dir: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let executable = PathBuf::from(system).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    std::process::Command::new(executable)
        .arg("-NoExit")
        .current_dir(dir)
        .creation_flags(windows::Win32::System::Threading::CREATE_NEW_CONSOLE.0)
        .spawn()?;
    Ok(())
}

/// Folder, folder-background and drive menus that can offer "Open in Kestrel" (per user).
const MENU_KEYS: [&str; 3] = [
    r"Software\Classes\Directory\shell\Kestrel",
    r"Software\Classes\Directory\Background\shell\Kestrel",
    r"Software\Classes\Drive\shell\Kestrel",
];

fn menu_command() -> Option<String> {
    Some(format!("\"{}\" \"%V\"", std::env::current_exe().ok()?.display()))
}

/// Whether the folder menus open this copy of Kestrel (a moved executable reads as off).
pub fn folder_menu_registered() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let key = HSTRING::from(format!(r"{}\command", MENU_KEYS[0]));
    let found = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    let text = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
    found.is_ok() && Some(text) == menu_command()
}

/// Adds or removes "Open in Kestrel" on folder and drive menus, for this user only.
pub fn set_folder_menu(on: bool) -> Result<(), String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteTreeW, RegSetKeyValueW};
    let fail = |e: windows::core::Error| format!("Couldn't change the folder menu: {}", e.message());
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let command = menu_command().unwrap_or_default();
    for key in MENU_KEYS {
        unsafe {
            if !on {
                let r = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key));
                if r.is_err() && r != windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
                    return Err(fail(r.into()));
                }
                continue;
            }
            for (sub, name, value) in [
                (key.to_string(), PCWSTR::null(), "Open in Kestrel".to_string()),
                (key.to_string(), w!("Icon"), format!("\"{}\",0", exe.display())),
                (format!(r"{key}\command"), PCWSTR::null(), command.clone()),
            ] {
                let data: Vec<u16> = value.encode_utf16().chain([0]).collect();
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    &HSTRING::from(sub),
                    name,
                    REG_SZ.0,
                    Some(data.as_ptr().cast()),
                    (data.len() * 2) as u32,
                )
                .ok()
                .map_err(fail)?;
            }
        }
    }
    Ok(())
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
    fn zip_files_list_and_open_as_folders() {
        let dir = std::env::temp_dir().join(format!("fep-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src").join("sub")).unwrap();
        std::fs::write(dir.join("src").join("a.txt"), "hello").unwrap();
        std::fs::write(dir.join("src").join("sub").join("b.txt"), "").unwrap();
        let zip = dir.join("x.zip");
        let made = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Compress-Archive -Path $env:FEP_SRC -DestinationPath $env:FEP_ZIP",
            ])
            .env("FEP_SRC", dir.join("src").join("*"))
            .env("FEP_ZIP", &zip)
            .status()
            .unwrap();
        assert!(made.success());
        let s = crate::network::list_location(&zip).unwrap();
        let mut found: Vec<_> =
            (0..s.len()).map(|i| (s.name_string(i), s.is_dir(i), s.entries[i].size)).collect();
        found.sort();
        assert_eq!(found, [("a.txt".to_string(), false, 5), ("sub".to_string(), true, 0)]);
        assert_eq!(crate::network::list_location(&zip.join("sub")).unwrap().name_string(0), "b.txt");
        assert_eq!(open(&zip, HWND::default()), Ok(Some(zip.clone())));
        let out = dir.join("out");
        std::fs::create_dir(&out).unwrap();
        let copy = crate::ops::Op::Copy { paths: vec![zip.join("a.txt")], dest: out.clone() };
        crate::ops::run(&copy, crate::ops::Mode::Silent).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("a.txt")).unwrap(), "hello", "copied out of the zip");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Writes to the real per-user registry, so it only runs when asked: cargo test -- --ignored folder_menu
    #[test]
    #[ignore]
    fn folder_menu_registers_and_unregisters() {
        let before = folder_menu_registered();
        set_folder_menu(true).unwrap();
        assert!(folder_menu_registered());
        set_folder_menu(false).unwrap();
        assert!(!folder_menu_registered());
        set_folder_menu(false).unwrap(); // removing twice is fine
        if before {
            set_folder_menu(true).unwrap();
        }
    }

    #[test]
    fn missing_file_reports_an_error() {
        let r = open(Path::new(r"C:\definitely\not\here.txt"), HWND::default());
        assert!(r.as_ref().is_err_and(|e| e.starts_with(r"Can't open C:\definitely")), "{r:?}");
    }
}
