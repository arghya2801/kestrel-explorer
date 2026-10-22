//! Owned, modeless Settings and keyboard-help windows.
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{HDC, HFONT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use super::{child, dark, scale, set_font, window_text};
use crate::settings::Settings;
use crate::sort::{Sort, SortKey};
use crate::theme::ThemeChoice;

const APPLY: usize = 3;
const DEFAULTS: usize = 4;
const PAGES: usize = 5;
const THEME: usize = 10;
const SIDEBAR: usize = 11;
const PREVIEW: usize = 12;
const HIDDEN: usize = 13;
const SORT: usize = 14;
const DESCENDING: usize = 15;
const COLUMN: usize = 16;
const FOLDER: usize = 20;
const X: usize = 21;
const Y: usize = 22;
const WIDTH: usize = 23;
const HEIGHT: usize = 24;
const MAXIMIZED: usize = 25;
const ERROR: usize = 26;
const GRID: usize = 31;
const SESSION: usize = 30;
const DUAL: usize = 27;
const RATIO: usize = 28;
const PREVIEW_WIDTH: usize = 29;
const FOLDER_MENU: usize = 32;

thread_local! { static WINDOWS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) }; }

/// Lets another modeless window get Tab, Enter and Esc handling like these dialogs.
pub(super) fn track(hwnd: HWND) {
    WINDOWS.with(|all| all.borrow_mut().push(hwnd));
}
pub(super) fn untrack(hwnd: HWND) {
    WINDOWS.with(|all| all.borrow_mut().retain(|&h| h != hwnd));
}

struct Dialog {
    owner: HWND,
    guide: bool,
    operations: bool,
    font: HFONT,
    fields: Vec<(HWND, HWND, usize)>,
}

impl Drop for Dialog {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(
                self.font.0,
            ));
        }
    }
}

const GUIDE: &str = "Navigation\r\n\
Enter / double-click    Open the selected item\r\n\
Backspace / Alt+Left    Back\r\n\
Alt+Right    Forward\r\n\
Alt+Up    Parent folder\r\n\
Ctrl+L / Alt+D / F4    Edit the address\r\n\
F5    Refresh\r\n\
Tab / Shift+Tab    Next / previous control\r\n\r\n\
Files (when the file list has focus)\r\n\
Ctrl+A    Select all\r\n\
Ctrl+C / Ctrl+X / Ctrl+V    Copy / cut / paste\r\n\
Ctrl+Shift+C    Copy selected paths\r\n\
Ctrl+Shift+N    New folder\r\n\
F2    Rename; with several items selected, batch rename with preview\r\n\
Delete    Recycle\r\n\
Shift+Delete    Delete permanently\r\n\
Ctrl+Z / Ctrl+Y    Undo / redo the last file operation\r\n\
Shift+F10 / Menu key    Context menu\r\n\
Shift+right-click    Full Windows context menu\r\n\
Alt+Enter    Properties\r\n\r\n\
Search\r\n\
Ctrl+F    Focus the filter\r\n\
Enter in the filter    Search subfolders\r\n\
Esc in the filter    Clear the filter\r\n\
Filter words    size:>10MB  size:<1KB  modified:<7d  modified:>2w\r\n\
Search only    content:word finds files containing the word\r\n\
Esc in the list    Cancel loading or search\r\n\
Esc in the address    Restore the current address\r\n\r\n\
Tabs and windows\r\n\
Ctrl+T    New tab\r\n\
Ctrl+Shift+K    Duplicate the active tab\r\n\
Ctrl+W    Close the active tab (the last tab stays)\r\n\
Middle-click a tab    Close that tab\r\n\
Right-click a tab    Tabs above the sidebar, or back along the top\r\n\
Ctrl+Tab / Ctrl+Shift+Tab    Next / previous tab\r\n\
Ctrl+N    New window, side by side\r\n\
Ctrl+Shift+S    Toggle two folder panes\r\n\
F6    Focus the other folder pane\r\n\
Shift+F5 / Shift+F6    Copy / move the selection to the other pane\r\n\
Ctrl+U    Swap the two panes\r\n\
Ctrl+Shift+U    Show this folder in the other pane too\r\n\r\n\
View and help\r\n\
Ctrl+H    Toggle hidden items\r\n\
Alt+P    Toggle preview\r\n\
Ctrl+Shift+E    Show the open folder in the sidebar\r\n\
Ctrl+mouse wheel    Thumbnail size; past the smallest, back to Details\r\n\
F1    Keyboard shortcuts\r\n\r\n\
Text boxes keep their normal editing shortcuts. File commands do not delete, cut, or copy files while you edit text.\r\n\
Close this guide with Esc or the Close button.";

/// The guide, plus the user's own keys.txt bindings.
fn guide_text() -> String {
    let custom = crate::keys::bindings(super::COMMAND_NAMES);
    if custom.is_empty() {
        return GUIDE.to_owned();
    }
    let lines: Vec<&str> = custom.iter().map(|b| b.text.as_str()).collect();
    format!("Your keys (keys.txt; these win over the keys below)\r\n{}\r\n\r\n{GUIDE}", lines.join("\r\n"))
}

pub(super) fn shortcuts(owner: HWND) {
    open(owner, 1);
}
pub(super) fn settings(owner: HWND) {
    open(owner, 0);
}

pub(super) fn operations(owner: HWND) {
    open(owner, 2);
}

fn open(owner: HWND, kind: u8) {
    let guide = kind != 0;
    let operations = kind == 2;
    let class = if operations {
        w!("KestrelOperations")
    } else if guide {
        w!("KestrelShortcuts")
    } else {
        w!("KestrelSettings")
    };
    let existing = WINDOWS.with(|all| {
        all.borrow().iter().copied().find(|&h| unsafe {
            let mut name = [0u16; 64];
            let n = GetClassNameW(h, &mut name);
            GetWindow(h, GW_OWNER).ok() == Some(owner) && name[..n as usize] == class.as_wide()[..]
        })
    });
    if let Some(hwnd) = existing {
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    if let Err(e) = unsafe { create(owner, guide, class, operations) } {
        super::set_status(&format!("Couldn't open dialog: {e}"));
    }
}

unsafe fn create(owner: HWND, guide: bool, class: PCWSTR, operations: bool) -> windows::core::Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let mut r = RECT::default();
        GetWindowRect(owner, &mut r)?;
        let title = if operations {
            w!("File operations - Kestrel")
        } else if guide {
            w!("Keyboard shortcuts - Kestrel")
        } else {
            w!("Settings - Kestrel")
        };
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class,
            title,
            WS_OVERLAPPEDWINDOW,
            r.left + scale(owner, 40),
            r.top + scale(owner, 40),
            scale(owner, 660),
            scale(owner, 570),
            Some(owner),
            None,
            Some(instance),
            None,
        )?;
        let result = (|| -> windows::core::Result<Dialog> {
            let mut data =
                Dialog { owner, guide, operations, font: super::message_font(hwnd), fields: Vec::new() };
            if guide {
                child(
                    hwnd,
                    w!("EDIT"),
                    &HSTRING::from(if operations { crate::activity::text() } else { guide_text() }),
                    WS_TABSTOP
                        | WS_VSCROLL
                        | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32),
                    WS_EX_CLIENTEDGE,
                    30,
                )?;
                child(
                    hwnd,
                    w!("BUTTON"),
                    w!("&Close"),
                    WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
                    WINDOW_EX_STYLE(0),
                    IDCANCEL.0 as usize,
                )?;
            } else {
                let pages = child(hwnd, WC_TABCONTROLW, w!(""), WS_TABSTOP, WINDOW_EX_STYLE(0), PAGES)?;
                for (i, name) in ["Appearance", "File list", "Startup", "Window"].iter().enumerate() {
                    let name = HSTRING::from(*name);
                    let tab = TCITEMW {
                        mask: TCIF_TEXT,
                        pszText: PWSTR(name.as_ptr() as *mut _),
                        ..Default::default()
                    };
                    SendMessageW(
                        pages,
                        TCM_INSERTITEMW,
                        Some(WPARAM(i)),
                        Some(LPARAM(&tab as *const _ as isize)),
                    );
                }
                for (page, id, label, choices, check) in [
                    (0, THEME, "&Theme", Some("System|Light|Dark"), false),
                    (0, SIDEBAR, "&Sidebar width", None, false),
                    (0, PREVIEW, "Show &preview pane", None, true),
                    (0, DUAL, "Two folder panes", None, true),
                    (0, RATIO, "Left pane percentage", None, false),
                    (0, PREVIEW_WIDTH, "Preview width", None, false),
                    (1, GRID, "Thumbnail grid view", None, true),
                    (1, HIDDEN, "Show &hidden items", None, true),
                    (
                        1,
                        SORT,
                        "Default &sort",
                        Some("Name|Date modified|Type|Size|Date created|Attributes"),
                        false,
                    ),
                    (1, DESCENDING, "&Descending", None, true),
                    (1, COLUMN, "&Name column width", None, false),
                    (1, COLUMN + 1, "&Date column width", None, false),
                    (1, COLUMN + 2, "&Type column width", None, false),
                    (1, COLUMN + 3, "Si&ze column width", None, false),
                    (2, SESSION, "Restore tabs and windows", None, true),
                    (2, FOLDER, "&Folder when not restoring", None, false),
                    (2, FOLDER_MENU, "Add \"Open in Kestrel\" to folder menus", None, true),
                    (3, X, "Window &X", None, false),
                    (3, Y, "Window &Y", None, false),
                    (3, WIDTH, "&Width", None, false),
                    (3, HEIGHT, "&Height", None, false),
                    (3, MAXIMIZED, "Start &maximized", None, true),
                ] {
                    let label_hwnd = child(
                        hwnd,
                        w!("STATIC"),
                        &HSTRING::from(if check { "" } else { label }),
                        WINDOW_STYLE(0),
                        WINDOW_EX_STYLE(0),
                        0,
                    )?;
                    let (class, style, ex) = if choices.is_some() {
                        (
                            w!("COMBOBOX"),
                            WINDOW_STYLE(CBS_DROPDOWNLIST as u32) | WS_VSCROLL,
                            WINDOW_EX_STYLE(0),
                        )
                    } else if check {
                        (w!("BUTTON"), WINDOW_STYLE(BS_AUTOCHECKBOX as u32), WINDOW_EX_STYLE(0))
                    } else {
                        (w!("EDIT"), WINDOW_STYLE(ES_AUTOHSCROLL as u32), WS_EX_CLIENTEDGE)
                    };
                    let edit = child(
                        hwnd,
                        class,
                        &HSTRING::from(if check { label } else { "" }),
                        WS_TABSTOP | style,
                        ex,
                        id,
                    )?;
                    if let Some(choices) = choices {
                        for choice in choices.split('|') {
                            let text = HSTRING::from(choice);
                            SendMessageW(edit, CB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
                        }
                    }
                    data.fields.push((label_hwnd, edit, page));
                }
                child(hwnd, w!("STATIC"), w!(""), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), ERROR)?;
                for (id, label) in [
                    (DEFAULTS, "&Restore defaults"),
                    (IDOK.0 as usize, "OK"),
                    (IDCANCEL.0 as usize, "Cancel"),
                    (APPLY, "&Apply"),
                ] {
                    let style = if id == IDOK.0 as usize { BS_DEFPUSHBUTTON } else { BS_PUSHBUTTON };
                    child(
                        hwnd,
                        w!("BUTTON"),
                        &HSTRING::from(label),
                        WS_TABSTOP | WINDOW_STYLE(style as u32),
                        WINDOW_EX_STYLE(0),
                        id,
                    )?;
                }
            }
            Ok(data)
        })();
        let data = match result {
            Ok(data) => data,
            Err(e) => {
                let _ = DestroyWindow(hwnd);
                return Err(e);
            }
        };
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(Box::new(data)) as isize);
        WINDOWS.with(|all| all.borrow_mut().push(hwnd));
        if let Some(data) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Dialog).as_ref() {
            let font = data.font;
            let mut c = GetWindow(hwnd, GW_CHILD).unwrap_or_default();
            while !c.is_invalid() {
                set_font(&[c], HFONT(font.0));
                c = GetWindow(c, GW_HWNDNEXT).unwrap_or_default();
            }
        }
        if !guide && let Some(s) = super::current_settings(owner) {
            fill(hwnd, &s);
        }
        layout(hwnd);
        if operations {
            SetTimer(Some(hwnd), 1, 500, None);
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(GetDlgItem(Some(hwnd), if guide { 30 } else { PAGES as i32 }).ok());
        Ok(())
    }
}

pub(super) fn translate(msg: &MSG) -> bool {
    let all = WINDOWS.with(|all| all.borrow().clone());
    for hwnd in all {
        unsafe {
            if msg.hwnd == hwnd || IsChild(hwnd, msg.hwnd).as_bool() {
                if msg.message == WM_KEYDOWN && msg.wParam.0 == VK_ESCAPE.0 as usize {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    return true;
                }
                return IsDialogMessageW(hwnd, msg).as_bool();
            }
        }
    }
    false
}

fn field(hwnd: HWND, id: usize) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id as i32) }.unwrap_or_default()
}

fn fill(hwnd: HWND, s: &Settings) {
    let text = |id, value: String| unsafe {
        let _ = SetWindowTextW(field(hwnd, id), &HSTRING::from(value));
    };
    let check = |id, value| unsafe {
        SendMessageW(
            field(hwnd, id),
            BM_SETCHECK,
            Some(WPARAM(if value { BST_CHECKED.0 as usize } else { 0 })),
            None,
        );
    };
    let select = |id, value| unsafe {
        SendMessageW(field(hwnd, id), CB_SETCURSEL, Some(WPARAM(value)), None);
    };
    select(
        THEME,
        match s.theme {
            ThemeChoice::System => 0,
            ThemeChoice::Light => 1,
            ThemeChoice::Dark => 2,
        },
    );
    text(SIDEBAR, s.split.unwrap_or(240).to_string());
    check(PREVIEW, s.preview);
    check(DUAL, s.dual_pane);
    check(SESSION, s.restore_session);
    text(RATIO, (s.pane_ratio / 10).to_string());
    text(PREVIEW_WIDTH, s.preview_width.unwrap_or(320).to_string());
    check(HIDDEN, s.show_hidden);
    check(GRID, s.grid);
    select(
        SORT,
        match s.sort.key {
            SortKey::Name => 0,
            SortKey::Modified => 1,
            SortKey::Type => 2,
            SortKey::Size => 3,
            SortKey::Created => 4,
            SortKey::Attributes => 5,
        },
    );
    check(DESCENDING, s.sort.descending);
    for i in 0..4 {
        text(COLUMN + i, s.columns.get(i).copied().unwrap_or(super::COLUMNS[i].1).to_string());
    }
    text(FOLDER, s.last_folder.as_ref().map(|p| p.display().to_string()).unwrap_or_default());
    let (x, y, w, h) = s.window.unwrap_or((0, 0, 1100, 700));
    for (id, value) in [(X, x), (Y, y), (WIDTH, w), (HEIGHT, h)] {
        text(id, value.to_string());
    }
    check(MAXIMIZED, s.maximized);
    check(FOLDER_MENU, crate::shell::folder_menu_registered());
}

fn read(hwnd: HWND) -> std::result::Result<Settings, String> {
    let number = |id, label, min, max| {
        let value = window_text(field(hwnd, id))
            .trim()
            .parse::<i32>()
            .map_err(|_| format!("{label} must be a whole number."))?;
        if !(min..=max).contains(&value) {
            return Err(format!("{label} must be between {min} and {max}."));
        }
        Ok(value)
    };
    let checked =
        |id| unsafe { SendMessageW(field(hwnd, id), BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize };
    let selected = |id| unsafe { SendMessageW(field(hwnd, id), CB_GETCURSEL, None, None).0 };
    let folder = window_text(field(hwnd, FOLDER));
    if folder.contains(['\r', '\n']) {
        return Err("Enter one folder path.".into());
    }
    let last_folder = if folder.trim().is_empty() {
        None
    } else {
        Some(crate::browser::parse_location(&folder, None).ok_or("Enter an absolute folder path.")?)
    };
    Ok(Settings {
        theme: match selected(THEME) {
            1 => ThemeChoice::Light,
            2 => ThemeChoice::Dark,
            _ => ThemeChoice::System,
        },
        split: Some(number(SIDEBAR, "Sidebar width", 120, 4000)?),
        preview: checked(PREVIEW),
        grid: checked(GRID),
        dual_pane: checked(DUAL),
        restore_session: checked(SESSION),
        pane_ratio: number(RATIO, "Left pane percentage", 10, 90)? * 10,
        preview_width: Some(number(PREVIEW_WIDTH, "Preview width", 100, 10000)?),
        show_hidden: checked(HIDDEN),
        sort: Sort {
            key: match selected(SORT) {
                1 => SortKey::Modified,
                2 => SortKey::Type,
                3 => SortKey::Size,
                4 => SortKey::Created,
                5 => SortKey::Attributes,
                _ => SortKey::Name,
            },
            descending: checked(DESCENDING),
        },
        columns: (0..4)
            .map(|i| number(COLUMN + i, "Column width", 20, 10000))
            .collect::<std::result::Result<_, _>>()?,
        last_folder,
        window: Some((
            number(X, "X", -32768, 32767)?,
            number(Y, "Y", -32768, 32767)?,
            number(WIDTH, "Width", 360, 16384)?,
            number(HEIGHT, "Height", 240, 16384)?,
        )),
        maximized: checked(MAXIMIZED),
        ..unsafe { GetWindow(hwnd, GW_OWNER) }.ok().and_then(super::current_settings).unwrap_or_default()
    })
}

fn layout(hwnd: HWND) {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Dialog;
        let Some(data) = ptr.as_ref() else { return };
        let mut r = RECT::default();
        let _ = GetClientRect(hwnd, &mut r);
        let s = |n| scale(hwnd, n);
        let (w, h) = (r.right, r.bottom);
        if data.guide {
            let _ = MoveWindow(field(hwnd, 30), s(12), s(12), (w - s(24)).max(0), (h - s(64)).max(0), true);
            let _ = MoveWindow(
                field(hwnd, IDCANCEL.0 as usize),
                (w - s(102)).max(0),
                h - s(42),
                s(90),
                s(30),
                true,
            );
            return;
        }
        let _ = MoveWindow(field(hwnd, PAGES), s(12), s(12), (w - s(24)).max(0), s(28), true);
        let page = SendMessageW(field(hwnd, PAGES), TCM_GETCURSEL, None, None).0 as usize;
        let mut row = 0;
        for &(label, edit, p) in &data.fields {
            let show = if p == page { SW_SHOWNA } else { SW_HIDE };
            let _ = ShowWindow(label, show);
            let _ = ShowWindow(edit, show);
            if p == page {
                let y = s(56 + row * 38);
                let _ = MoveWindow(label, s(24), y + s(5), s(190), s(25), true);
                let id = GetDlgCtrlID(edit) as usize;
                let height = if matches!(id, THEME | SORT) { s(160) } else { s(28) };
                let _ = MoveWindow(edit, s(218), y, (w - s(244)).max(s(90)), height, true);
                row += 1;
            }
        }
        let _ = MoveWindow(field(hwnd, ERROR), s(24), h - s(90), (w - s(48)).max(0), s(40), true);
        let _ = MoveWindow(field(hwnd, DEFAULTS), s(12), h - s(42), s(140), s(30), true);
        for (i, id) in [IDOK.0 as usize, IDCANCEL.0 as usize, APPLY].into_iter().enumerate() {
            let _ =
                MoveWindow(field(hwnd, id), w - s(282) + (i as i32) * s(90), h - s(42), s(84), s(30), true);
        }
    }
}

extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::crash::guard(|| unsafe {
        match msg {
            WM_COMMAND => match wp.0 & 0xffff {
                id if id == IDCANCEL.0 as usize => {
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                DEFAULTS => {
                    fill(hwnd, &Settings::default());
                    return LRESULT(0);
                }
                id if id == APPLY || id == IDOK.0 as usize => {
                    // A system setting rather than a preference: changed in the registry, not Settings.
                    let menu = SendMessageW(field(hwnd, FOLDER_MENU), BM_GETCHECK, None, None).0
                        == BST_CHECKED.0 as isize;
                    let menu_changed = if menu != crate::shell::folder_menu_registered() {
                        crate::shell::set_folder_menu(menu)
                    } else {
                        Ok(())
                    };
                    match read(hwnd).and_then(|s| menu_changed.map(|()| s)) {
                        Ok(s) => {
                            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Dialog;
                            if let Some(data) = ptr.as_ref() {
                                super::apply_settings(data.owner, s);
                            }
                            let _ = SetWindowTextW(field(hwnd, ERROR), w!("Settings applied."));
                            if id == IDOK.0 as usize {
                                let _ = DestroyWindow(hwnd);
                            }
                        }
                        Err(e) => {
                            let _ = SetWindowTextW(field(hwnd, ERROR), &HSTRING::from(e));
                        }
                    }
                    return LRESULT(0);
                }
                _ => {}
            },
            WM_NOTIFY if lp.0 != 0 => {
                if (*(lp.0 as *const NMHDR)).code == TCN_SELCHANGE {
                    layout(hwnd);
                }
            }
            WM_TIMER => {
                let data = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Dialog).as_ref();
                if data.is_some_and(|d| d.operations) {
                    let text = crate::activity::text();
                    if window_text(field(hwnd, 30)) != text {
                        let _ = SetWindowTextW(field(hwnd, 30), &HSTRING::from(text));
                    }
                }
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let rect = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                if let Some(data) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog).as_mut() {
                    let font = super::message_font(hwnd);
                    let old = std::mem::replace(&mut data.font, font);
                    let mut child = GetWindow(hwnd, GW_CHILD).unwrap_or_default();
                    while !child.is_invalid() {
                        set_font(&[child], font);
                        child = GetWindow(child, GW_HWNDNEXT).unwrap_or_default();
                    }
                    let _ = windows::Win32::Graphics::Gdi::DeleteObject(
                        windows::Win32::Graphics::Gdi::HGDIOBJ(old.0),
                    );
                }
                layout(hwnd);
                return LRESULT(0);
            }
            WM_SIZE => {
                layout(hwnd);
                return LRESULT(0);
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMinTrackSize.x = scale(hwnd, 560);
                info.ptMinTrackSize.y = scale(hwnd, 520);
                return LRESULT(0);
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                if let Some(r) = dark::edit_colors(HDC(wp.0 as _)) {
                    return r;
                }
            }
            WM_ERASEBKGND if dark::erase(hwnd, HDC(wp.0 as _)) => return LRESULT(1),
            WM_NCDESTROY => {
                WINDOWS.with(|all| all.borrow_mut().retain(|&h| h != hwnd));
                let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut Dialog;
                if !ptr.is_null() {
                    drop(Box::from_raw(ptr));
                }
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    })
    .unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}
