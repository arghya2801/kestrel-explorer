//! Windows' full right-click menu (Open with, Send to, Properties, and every
//! installed add-on), shown by a helper copy of this program. Third-party menu
//! handlers are a common cause of crashes and hangs; in a helper process they
//! can only take down the helper, never the window you're browsing in.

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMF_EXPLORE, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, IContextMenu,
    IContextMenu2, IContextMenu3, ILFindLastID, ILFree, IShellFolder, SHBindToParent, SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, Interface, PCSTR, PCWSTR, Result, w};

use crate::crash;

pub const MENU_FLAG: &str = "--shell-menu";
/// Prints the menu's items instead of showing it (used by tests).
pub const LIST_FLAG: &str = "--shell-menu-list";
const FIRST_ID: u32 = 1;
/// Not in this version of the bindings; same value as SEE_MASK_UNICODE.
const CMIC_MASK_UNICODE: u32 = 0x4000;
const LAST_ID: u32 = 0x7FFF;
/// The helper never lingers longer than this, whatever a command left open.
const MAX_LINGER: Duration = Duration::from_secs(3600);

pub fn helper_args(x: i32, y: i32, paths: &[PathBuf]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![MENU_FLAG.into(), x.to_string().into(), y.to_string().into()];
    args.extend(paths.iter().map(|p| p.as_os_str().to_owned()));
    args
}

pub fn parse_helper_args(args: &[OsString]) -> Option<(i32, i32, Vec<PathBuf>)> {
    let (flag, rest) = args.split_first()?;
    if flag != MENU_FLAG {
        return None;
    }
    let num = |i: usize| rest.get(i)?.to_str()?.parse::<i32>().ok();
    let (x, y) = (num(0)?, num(1)?);
    let paths: Vec<PathBuf> = rest.get(2..)?.iter().map(PathBuf::from).collect();
    (!paths.is_empty()).then_some((x, y, paths))
}

/// Shows the full menu for `paths` at screen point (x, y) in a helper process. Doesn't wait.
pub fn launch(x: i32, y: i32, paths: &[PathBuf]) -> std::io::Result<()> {
    // The helper must be allowed to come to the front, or its menu won't close properly.
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    std::process::Command::new(std::env::current_exe()?).args(helper_args(x, y, paths)).spawn().map(drop)
}

/// Item ID lists, freed on drop.
struct Pidls(Vec<*mut ITEMIDLIST>);
impl Drop for Pidls {
    fn drop(&mut self) {
        for &p in &self.0 {
            unsafe { ILFree(Some(p)) };
        }
    }
}

struct Menu {
    cm: IContextMenu,
    menu: HMENU,
    _pidls: Pidls,
}
impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.menu);
        }
    }
}

/// The shell's menu for items in one folder.
fn build(paths: &[PathBuf], owner: HWND) -> Result<Menu> {
    let mut pidls = Pidls(Vec::new());
    unsafe {
        for p in paths {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None)?;
            pidls.0.push(pidl);
        }
        let folder: IShellFolder = SHBindToParent(pidls.0[0], None)?;
        let children: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|&p| ILFindLastID(p) as *const _).collect();
        let cm: IContextMenu = folder.GetUIObjectOf(owner, &children, None)?;
        let menu = CreatePopupMenu()?;
        let menu = Menu { cm, menu, _pidls: pidls };
        menu.cm.QueryContextMenu(menu.menu, 0, FIRST_ID, LAST_ID, CMF_NORMAL | CMF_EXPLORE).ok()?;
        Ok(menu)
    }
}

/// The top-level item names of the menu for `paths`.
pub fn menu_items(paths: &[PathBuf]) -> Result<Vec<String>> {
    let menu = build(paths, HWND::default())?;
    let mut items = Vec::new();
    unsafe {
        for i in 0..GetMenuItemCount(Some(menu.menu)).max(0) {
            let mut buf = [0u16; 256];
            let n = GetMenuStringW(menu.menu, i as u32, Some(&mut buf), MF_BYPOSITION);
            if n > 0 {
                items.push(String::from_utf16_lossy(&buf[..n as usize]).replace('&', ""));
            }
        }
    }
    Ok(items)
}

thread_local! {
    /// Menu handlers that draw their own items (Send to, Open with) need these messages.
    static HANDLER: RefCell<Option<(Option<IContextMenu3>, Option<IContextMenu2>)>> = const { RefCell::new(None) };
}

extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crash::guard(|| {
        if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
            let handled = HANDLER.with(|h| {
                let h = h.borrow();
                let (cm3, cm2) = h.as_ref()?;
                let mut r = LRESULT(0);
                if let Some(cm3) = cm3 {
                    unsafe { cm3.HandleMenuMsg2(msg, wp, lp, Some(&mut r)) }.ok()?;
                } else {
                    unsafe { cm2.as_ref()?.HandleMenuMsg(msg, wp, lp) }.ok()?;
                }
                Some(r)
            });
            if let Some(r) = handled {
                return r;
            }
        }
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    })
    .unwrap_or(LRESULT(0))
}

/// Entry point when this program was started as the menu helper (`args` without the program name).
/// Returns the process exit code.
pub fn run_helper(args: &[OsString]) -> i32 {
    if args.first().is_some_and(|a| a == LIST_FLAG) {
        let paths: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
        return match menu_items(&paths) {
            Ok(items) => {
                println!("{}", items.join("\n"));
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        };
    }
    let Some((x, y, paths)) = parse_helper_args(args) else { return 2 };
    match crash::guard(|| show(x, y, &paths)) {
        Some(Ok(())) => 0,
        Some(Err(e)) => {
            crash::log(&format!("shell menu failed: {e}"));
            1
        }
        None => 1,
    }
}

fn show(x: i32, y: i32, paths: &[PathBuf]) -> Result<()> {
    unsafe {
        let hinstance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?.into();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_proc),
            hInstance: hinstance,
            lpszClassName: w!("FileExplorerProMenuHost"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        // Invisible owner for the menu and for dialogs the chosen command opens.
        let host = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("FileExplorerProMenuHost"),
            w!("File Explorer Pro"),
            WS_POPUP,
            x,
            y,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        )?;
        let menu = build(paths, host)?;
        HANDLER.with(|h| *h.borrow_mut() = Some((menu.cm.cast().ok(), menu.cm.cast().ok())));
        let _ = SetForegroundWindow(host);
        let cmd = TrackPopupMenuEx(menu.menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, x, y, host, None).0;
        let _ = PostMessageW(Some(host), WM_NULL, WPARAM(0), LPARAM(0)); // lets the menu close cleanly
        if cmd >= FIRST_ID as i32 {
            let verb = (cmd as u32 - FIRST_ID) as usize;
            let info = CMINVOKECOMMANDINFOEX {
                cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                fMask: CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE,
                hwnd: host,
                lpVerb: PCSTR(verb as *const u8),
                lpVerbW: PCWSTR(verb as *const u16),
                nShow: SW_SHOWNORMAL.0,
                ptInvoke: POINT { x, y },
                ..Default::default()
            };
            menu.cm.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)?;
        }
        // Properties and similar commands open windows in this process: stay until they close.
        linger(host);
        HANDLER.with(|h| *h.borrow_mut() = None);
        drop(menu);
        let _ = DestroyWindow(host);
    }
    Ok(())
}

/// Pumps messages until no visible window of this process is left (or an hour passes).
fn linger(host: HWND) {
    let start = Instant::now();
    let mut quiet_checks = 0;
    while start.elapsed() < MAX_LINGER && quiet_checks < 2 {
        let until = Instant::now() + Duration::from_millis(500);
        let mut msg = MSG::default();
        while Instant::now() < until {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        quiet_checks = if has_visible_windows(host) { 0 } else { quiet_checks + 1 };
    }
}

fn has_visible_windows(except: HWND) -> bool {
    struct Search {
        pid: u32,
        except: HWND,
        found: bool,
    }
    extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        let s = unsafe { &mut *(lp.0 as *mut Search) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == s.pid && hwnd != s.except && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            s.found = true;
            return false.into();
        }
        true.into()
    }
    let mut s = Search { pid: unsafe { GetCurrentProcessId() }, except, found: false };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut s as *mut _ as isize));
    }
    s.found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_arguments_round_trip() {
        let paths = vec![PathBuf::from(r"C:\a b\x.txt"), PathBuf::from(r"C:\a b\ünï 😀")];
        let args = helper_args(-12, 340, &paths);
        assert_eq!(args[0], MENU_FLAG);
        assert_eq!(parse_helper_args(&args), Some((-12, 340, paths)));
    }

    #[test]
    fn malformed_helper_arguments_are_rejected() {
        let s = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(parse_helper_args(&s(&[MENU_FLAG, "1", "2"])), None, "no paths");
        assert_eq!(parse_helper_args(&s(&[MENU_FLAG, "x", "2", r"C:\a"])), None);
        assert_eq!(parse_helper_args(&s(&["--other", "1", "2", r"C:\a"])), None);
        assert_eq!(parse_helper_args(&s(&[])), None);
    }

    #[test]
    fn menu_for_a_file_has_items() {
        let d = std::env::temp_dir().join(format!("fep-menu-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("x.txt"), "").unwrap();
        let _ole = crate::shell::Com::init();
        let items = menu_items(&[d.join("x.txt")]).unwrap();
        assert!(items.len() >= 3, "{items:?}");
        std::fs::remove_dir_all(d).unwrap();
    }
}
