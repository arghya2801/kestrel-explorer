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
const ACCENT: usize = 40;
const TEXT_SIZE: usize = 41;
const ROWS: usize = 42;
const TOOLBAR: usize = 43;
const STATUS: usize = 44;
const TITLE_PATH: usize = 45;
const EXTENSIONS: usize = 46;
const SYSTEM: usize = 47;
const FOLDERS_FIRST: usize = 48;
const SINGLE_CLICK: usize = 49;
const UP_DOUBLE_CLICK: usize = 50;
const CONFIRM: usize = 51;
const SIZES: usize = 52;
const DATES: usize = 53;
const TAB_POSITION: usize = 54;
const NEW_TAB: usize = 55;
const TAB_CLOSE: usize = 56;
const LAST_TAB: usize = 57;
const SIDEBAR_QUICK: usize = 58;
const SIDEBAR_NETWORK: usize = 59;
const SIDEBAR_FOLLOW: usize = 60;
const TERMINAL: usize = 61;
/// The Keyboard page: commands and their keys, a box that takes a key press, and its buttons.
const KEY_LIST: usize = 62;
const HOTKEY: usize = 63;
const ASSIGN: usize = 64;
const UNASSIGN: usize = 65;
const KEYS_RESET: usize = 66;
/// The Keyboard page's index.
const KEYBOARD: usize = 6;

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
    /// The Keyboard page: your bindings (keys, command id), whether they changed, and which
    /// command each row of the list is.
    keys: Vec<(String, usize)>,
    keys_changed: bool,
    key_rows: Vec<usize>,
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
Tab / Shift+Tab    Next / previous control\r\n\
Ctrl+Shift+P    Find any command or its shortcut\r\n\r\n\
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
Ctrl+Shift+T    Reopen the last closed tab\r\n\
Ctrl+Shift+K    Duplicate the active tab, both panes included\r\n\
Ctrl+W    Close the active tab (the last tab stays)\r\n\
Ctrl+1 to Ctrl+8 / Ctrl+9    That tab / the last tab\r\n\
Ctrl+Tab / Ctrl+Shift+Tab    Next / previous tab\r\n\
Middle-click a tab, or its X    Close that tab\r\n\
Middle-click a folder    Open it in a tab behind this one\r\n\
Drag a tab    Move it\r\n\
Right-click a tab    New, duplicate, reopen, move, close others or to the right, tabs on the right\r\n\
On a focused tab strip or list    Arrows move, Enter opens, Delete closes, Shift+F10 the menu\r\n\
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
            scale(owner, if guide { 660 } else { 740 }),
            scale(owner, if guide { 570 } else { 660 }),
            Some(owner),
            None,
            Some(instance),
            None,
        )?;
        let result = (|| -> windows::core::Result<Dialog> {
            let keys = crate::keys::bindings(super::COMMAND_NAMES)
                .iter()
                .filter_map(|b| Some((b.text.split("    ").next()?.to_string(), b.command)))
                .collect();
            let mut data = Dialog {
                owner,
                guide,
                operations,
                font: super::message_font(hwnd),
                fields: Vec::new(),
                keys,
                keys_changed: false,
                key_rows: Vec::new(),
            };
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
                for (i, name) in ["Appearance", "Files", "Tabs", "Sidebar", "Startup", "Window", "Keyboard"]
                    .iter()
                    .enumerate()
                {
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
                let names = |list: &[&str]| Some(list.join("|"));
                let accents: Vec<&str> = crate::prefs::ACCENTS.iter().map(|a| a.0).collect();
                let text_sizes: Vec<&str> = crate::prefs::TEXT_SIZES.iter().map(|a| a.0).collect();
                let spacing: Vec<&str> = crate::prefs::ROW_SPACING.iter().map(|a| a.0).collect();
                let fields: Vec<(usize, usize, &str, Option<String>, bool)> = vec![
                    (0, THEME, "&Theme", Some("System|Light|Dark".into()), false),
                    (0, ACCENT, "&Accent color", names(&accents), false),
                    (0, TEXT_SIZE, "Te&xt size", names(&text_sizes), false),
                    (0, ROWS, "&Row spacing", names(&spacing), false),
                    (0, TOOLBAR, "Tool&bar", Some("Icons and labels|Icons only".into()), false),
                    (0, STATUS, "Show the status &bar", None, true),
                    (0, TITLE_PATH, "Show the &full path in the title bar", None, true),
                    (0, PREVIEW, "Show the &preview pane", None, true),
                    (0, SIDEBAR, "&Sidebar width (pixels)", None, false),
                    (0, PREVIEW_WIDTH, "Preview &width (pixels)", None, false),
                    (1, EXTENSIONS, "Show file name &extensions", None, true),
                    (1, HIDDEN, "Show &hidden items", None, true),
                    (1, SYSTEM, "Show protected operating &system files", None, true),
                    (1, FOLDERS_FIRST, "List &folders before files", None, true),
                    (1, SINGLE_CLICK, "Open items with a single &click", None, true),
                    (1, UP_DOUBLE_CLICK, "Double-click empty space to go &up a folder", None, true),
                    (1, CONFIRM, "As&k before moving files to the Recycle Bin", None, true),
                    (
                        1,
                        SIZES,
                        "Si&zes",
                        Some("1 KB = 1,024 bytes, as Explorer shows them|1 kB = 1,000 bytes".into()),
                        false,
                    ),
                    (
                        1,
                        DATES,
                        "&Dates",
                        Some("System short date and time|Relative (Today, Yesterday)|ISO 8601 (2026-11-04 22:33)".into()),
                        false,
                    ),
                    (
                        1,
                        SORT,
                        "Default s&ort",
                        Some("Name|Date modified|Type|Size|Date created|Attributes".into()),
                        false,
                    ),
                    (1, DESCENDING, "Sort descendi&ng", None, true),
                    (1, GRID, "Show &thumbnails instead of details", None, true),
                    (2, TAB_POSITION, "Tab &position", Some("Along the top|In a column on the right".into()), false),
                    (
                        2,
                        NEW_TAB,
                        "&New tabs open",
                        Some("The current folder|This PC|Your user folder|Downloads".into()),
                        false,
                    ),
                    (
                        2,
                        TAB_CLOSE,
                        "&Close buttons",
                        Some("On the active tab and the one under the mouse|On every tab|None (Ctrl+W or middle-click)".into()),
                        false,
                    ),
                    (2, LAST_TAB, "Closing the &last tab", Some("Keeps the window open|Closes the window".into()), false),
                    (2, DUAL, "&Two folder panes", None, true),
                    (2, RATIO, "Left pane &percentage", None, false),
                    (3, SIDEBAR_QUICK, "Show &Quick access", None, true),
                    (3, SIDEBAR_NETWORK, "Show &Network", None, true),
                    (3, SIDEBAR_FOLLOW, "&Expand the sidebar to the open folder", None, true),
                    (4, SESSION, "&Restore tabs and windows", None, true),
                    (4, FOLDER, "&Folder when not restoring", None, false),
                    (4, FOLDER_MENU, "Add \"Open in Kestrel\" to folder menus", None, true),
                    (5, X, "Window &X", None, false),
                    (5, Y, "Window &Y", None, false),
                    (5, WIDTH, "&Width", None, false),
                    (5, HEIGHT, "&Height", None, false),
                    (5, MAXIMIZED, "Start &maximized", None, true),
                    (5, COLUMN, "&Name column width (pixels)", None, false),
                    (5, COLUMN + 1, "&Date column width (pixels)", None, false),
                    (5, COLUMN + 2, "&Type column width (pixels)", None, false),
                    (5, COLUMN + 3, "Si&ze column width (pixels)", None, false),
                    (
                        KEYBOARD,
                        TERMINAL,
                        "&Terminal",
                        Some("Windows PowerShell|PowerShell 7 (pwsh)|Windows Terminal|Command Prompt".into()),
                        false,
                    ),
                ];
                for (page, id, label, choices, check) in fields {
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
                    if let Some(choices) = &choices {
                        for choice in choices.split('|') {
                            let text = HSTRING::from(choice);
                            SendMessageW(edit, CB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
                        }
                    }
                    data.fields.push((label_hwnd, edit, page));
                }
                let style = LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS;
                let list = child(
                    hwnd,
                    WC_LISTVIEWW,
                    w!(""),
                    WS_TABSTOP | WINDOW_STYLE(style),
                    WS_EX_CLIENTEDGE,
                    KEY_LIST,
                )?;
                let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
                SendMessageW(
                    list,
                    LVM_SETEXTENDEDLISTVIEWSTYLE,
                    Some(WPARAM(ex as usize)),
                    Some(LPARAM(ex as isize)),
                );
                for (i, (title, cx)) in [("Command", 330), ("Keys", 180)].into_iter().enumerate() {
                    let title = HSTRING::from(title);
                    let col = LVCOLUMNW {
                        mask: LVCF_TEXT | LVCF_WIDTH,
                        cx: scale(hwnd, cx),
                        pszText: PWSTR(title.as_ptr() as *mut _),
                        ..Default::default()
                    };
                    SendMessageW(
                        list,
                        LVM_INSERTCOLUMNW,
                        Some(WPARAM(i)),
                        Some(LPARAM(&col as *const _ as isize)),
                    );
                }
                super::dark::style_new_list(list);
                child(hwnd, w!("msctls_hotkey32"), w!(""), WS_TABSTOP, WS_EX_CLIENTEDGE, HOTKEY)?;
                for (id, label) in [(ASSIGN, "A&ssign"), (UNASSIGN, "Re&move"), (KEYS_RESET, "Reset a&ll")] {
                    child(hwnd, w!("BUTTON"), &HSTRING::from(label), WS_TABSTOP, WINDOW_EX_STYLE(0), id)?;
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
            fill_keys(hwnd);
        }
        layout(hwnd);
        dark::apply_dialog(hwnd);
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
    use crate::prefs::{Dates, NewTab, TabClose, Terminal};
    let p = s.prefs;
    select(ACCENT, p.accent);
    select(TEXT_SIZE, p.text_size);
    select(ROWS, p.row_spacing);
    select(TOOLBAR, usize::from(!p.toolbar_labels));
    check(STATUS, p.status_bar);
    check(TITLE_PATH, p.full_path_title);
    check(EXTENSIONS, p.show_extensions);
    check(SYSTEM, p.show_system);
    check(FOLDERS_FIRST, p.folders_first);
    check(SINGLE_CLICK, p.single_click);
    check(UP_DOUBLE_CLICK, p.up_on_double_click);
    check(CONFIRM, p.confirm_recycle);
    select(SIZES, usize::from(p.decimal_sizes));
    select(
        DATES,
        [Dates::System, Dates::Relative, Dates::Iso].iter().position(|&d| d == p.dates).unwrap_or(0),
    );
    select(TAB_POSITION, usize::from(s.vertical_tabs));
    let new_tabs = [NewTab::CurrentFolder, NewTab::ThisPc, NewTab::UserFolder, NewTab::Downloads];
    select(NEW_TAB, new_tabs.iter().position(|&t| t == p.new_tab).unwrap_or(0));
    let closes = [TabClose::ActiveAndHovered, TabClose::Every, TabClose::Never];
    select(TAB_CLOSE, closes.iter().position(|&c| c == p.tab_close).unwrap_or(0));
    select(LAST_TAB, usize::from(p.last_tab_closes_window));
    check(SIDEBAR_QUICK, p.sidebar_quick_access);
    check(SIDEBAR_NETWORK, p.sidebar_network);
    check(SIDEBAR_FOLLOW, p.sidebar_follows);
    let terminals = [
        Terminal::WindowsPowerShell,
        Terminal::PowerShell7,
        Terminal::WindowsTerminal,
        Terminal::CommandPrompt,
    ];
    select(TERMINAL, terminals.iter().position(|&t| t == p.terminal).unwrap_or(0));
}

/// The Keyboard page's list: every command keys.txt can bind, with your keys or the built-in.
fn fill_keys(hwnd: HWND) {
    let Some(data) = (unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog).as_mut() }) else {
        return;
    };
    let list = field(hwnd, KEY_LIST);
    let entries: Vec<&super::palette::Entry> = super::palette::ENTRIES
        .iter()
        .filter(|e| super::COMMAND_NAMES.iter().any(|(_, id)| *id == e.id))
        .collect();
    data.key_rows = entries.iter().map(|e| e.id).collect();
    let selected = unsafe {
        SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_SELECTED as isize))).0
    };
    unsafe {
        SendMessageW(list, LVM_DELETEALLITEMS, None, None);
        for (row, e) in entries.iter().enumerate() {
            let label = HSTRING::from(format!("{}: {}", e.group, e.label));
            let item = LVITEMW {
                mask: LVIF_TEXT,
                iItem: row as i32,
                pszText: PWSTR(label.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(list, LVM_INSERTITEMW, None, Some(LPARAM(&item as *const _ as isize)));
            let keys = match data.keys.iter().find(|(_, id)| *id == e.id) {
                Some((keys, _)) => format!("{keys}  (yours)"),
                None => e.keys.to_string(),
            };
            let keys = HSTRING::from(keys);
            let sub = LVITEMW { iSubItem: 1, pszText: PWSTR(keys.as_ptr() as *mut _), ..Default::default() };
            SendMessageW(list, LVM_SETITEMTEXTW, Some(WPARAM(row)), Some(LPARAM(&sub as *const _ as isize)));
        }
        if let Ok(row) = usize::try_from(selected) {
            let on = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
            let item = LVITEMW { state: on, stateMask: on, ..Default::default() };
            SendMessageW(list, LVM_SETITEMSTATE, Some(WPARAM(row)), Some(LPARAM(&item as *const _ as isize)));
            SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(row)), Some(LPARAM(0)));
        }
    }
}

/// Assign, Remove and Reset all on the Keyboard page. Changes are saved with Apply or OK.
fn edit_keys(hwnd: HWND, id: usize) {
    let Some(data) = (unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog).as_mut() }) else {
        return;
    };
    let say = |text: &str| unsafe {
        let _ = SetWindowTextW(field(hwnd, ERROR), &HSTRING::from(text));
    };
    let row = unsafe {
        SendMessageW(
            field(hwnd, KEY_LIST),
            LVM_GETNEXTITEM,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(LVNI_SELECTED as isize)),
        )
        .0
    };
    let command = usize::try_from(row).ok().and_then(|r| data.key_rows.get(r).copied());
    let label = |command: usize| {
        super::palette::ENTRIES
            .iter()
            .find(|e| e.id == command)
            .map_or(String::new(), |e| e.label.to_string())
    };
    match (id, command) {
        (KEYS_RESET, _) => {
            data.keys.clear();
            say("Every command is back to its built-in keys. Apply to save.");
        }
        (_, None) => return say("Choose a command in the list first."),
        (UNASSIGN, Some(command)) => {
            data.keys.retain(|(_, c)| *c != command);
            say(&format!("{} is back to its built-in keys. Apply to save.", label(command)));
        }
        (_, Some(command)) => {
            // HKM_GETHOTKEY: the key in the low byte, HOTKEYF_SHIFT/CONTROL/ALT (1, 2, 4) above it.
            let hot = unsafe { SendMessageW(field(hwnd, HOTKEY), WM_USER + 2, None, None) }.0 as u32;
            let (vk, mods) = ((hot & 0xFF) as u16, (hot >> 8) & 0xFF);
            if vk == 0 {
                return say("Click the box left of Assign and press the keys first.");
            }
            let chord =
                crate::keys::Chord { vk, ctrl: mods & 2 != 0, shift: mods & 1 != 0, alt: mods & 4 != 0 };
            let Some(keys) = crate::keys::chord_text(chord) else {
                return say(
                    "That key can't be used. Try a letter, digit, F-key or arrow, with Ctrl, Shift or Alt.",
                );
            };
            // One set of keys per command, and each set of keys runs one command.
            data.keys.retain(|(k, c)| *c != command && !k.eq_ignore_ascii_case(&keys));
            data.keys.push((keys.clone(), command));
            let replaced = super::palette::ENTRIES
                .iter()
                .find(|e| e.id != command && e.keys.eq_ignore_ascii_case(&keys))
                .map(|e| format!(" It used to be {}.", e.label))
                .unwrap_or_default();
            say(&format!("{keys} now runs {}.{replaced} Apply to save.", label(command)));
        }
    }
    data.keys_changed = true;
    fill_keys(hwnd);
}

/// Saves the Keyboard page's bindings to keys.txt, if they changed.
fn save_keys(hwnd: HWND) -> std::result::Result<(), String> {
    let Some(data) = (unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog).as_mut() }) else {
        return Ok(());
    };
    if !data.keys_changed {
        return Ok(());
    }
    let lines: Vec<(String, &str)> = data
        .keys
        .iter()
        .filter_map(|(keys, id)| Some((keys.clone(), super::COMMAND_NAMES.iter().find(|(_, c)| c == id)?.0)))
        .collect();
    crate::keys::save(&lines).map_err(|e| format!("Couldn't save keys.txt: {e}"))?;
    data.keys_changed = false;
    Ok(())
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
        vertical_tabs: selected(TAB_POSITION) == 1,
        prefs: {
            use crate::prefs::{Dates, NewTab, Prefs, TabClose, Terminal};
            let index = |id| selected(id).max(0) as usize;
            Prefs {
                accent: index(ACCENT),
                text_size: index(TEXT_SIZE),
                row_spacing: index(ROWS),
                toolbar_labels: index(TOOLBAR) == 0,
                status_bar: checked(STATUS),
                full_path_title: checked(TITLE_PATH),
                show_extensions: checked(EXTENSIONS),
                show_system: checked(SYSTEM),
                folders_first: checked(FOLDERS_FIRST),
                single_click: checked(SINGLE_CLICK),
                up_on_double_click: checked(UP_DOUBLE_CLICK),
                confirm_recycle: checked(CONFIRM),
                decimal_sizes: index(SIZES) == 1,
                dates: [Dates::System, Dates::Relative, Dates::Iso][index(DATES).min(2)],
                new_tab: [NewTab::CurrentFolder, NewTab::ThisPc, NewTab::UserFolder, NewTab::Downloads]
                    [index(NEW_TAB).min(3)],
                tab_close: [TabClose::ActiveAndHovered, TabClose::Every, TabClose::Never]
                    [index(TAB_CLOSE).min(2)],
                last_tab_closes_window: index(LAST_TAB) == 1,
                sidebar_quick_access: checked(SIDEBAR_QUICK),
                sidebar_this_pc: true,
                sidebar_network: checked(SIDEBAR_NETWORK),
                sidebar_follows: checked(SIDEBAR_FOLLOW),
                terminal: [
                    Terminal::WindowsPowerShell,
                    Terminal::PowerShell7,
                    Terminal::WindowsTerminal,
                    Terminal::CommandPrompt,
                ][index(TERMINAL).min(3)],
            }
        },
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
            let mut class = [0u16; 16];
            let n = GetClassNameW(edit, &mut class) as usize;
            let class = String::from_utf16_lossy(&class[..n]).to_ascii_lowercase();
            // Checkboxes carry their own label and run the width of the page.
            let checkbox = class == "button";
            let _ = ShowWindow(label, if checkbox { SW_HIDE } else { show });
            let _ = ShowWindow(edit, show);
            if p == page {
                let y = s(54 + row * 34);
                if checkbox {
                    let _ = MoveWindow(edit, s(24), y, (w - s(48)).max(s(90)), s(28), true);
                } else {
                    let _ = MoveWindow(label, s(24), y + s(5), s(236), s(25), true);
                    // A combo box's height includes its open list.
                    let height = if class == "combobox" { s(240) } else { s(28) };
                    let _ = MoveWindow(edit, s(266), y, (w - s(290)).max(s(90)), height, true);
                }
                row += 1;
            }
        }
        // The Keyboard page: the command list under the terminal choice, then the key box.
        let keyboard = page == KEYBOARD;
        for id in [KEY_LIST, HOTKEY, ASSIGN, UNASSIGN, KEYS_RESET] {
            let _ = ShowWindow(field(hwnd, id), if keyboard { SW_SHOWNA } else { SW_HIDE });
        }
        if keyboard {
            let top = s(54 + row * 34) + s(6);
            let bottom = (h - s(140)).max(top + s(60));
            let _ = MoveWindow(field(hwnd, KEY_LIST), s(24), top, (w - s(48)).max(0), bottom - top, true);
            // LVSCW_AUTOSIZE_USEHEADER on the last column: it fills the rest of the width.
            SendMessageW(field(hwnd, KEY_LIST), LVM_SETCOLUMNWIDTH, Some(WPARAM(1)), Some(LPARAM(-2)));
            let y = bottom + s(10);
            let _ = MoveWindow(field(hwnd, HOTKEY), s(24), y, s(220), s(28), true);
            let _ = MoveWindow(field(hwnd, ASSIGN), s(254), y, s(90), s(28), true);
            let _ = MoveWindow(field(hwnd, UNASSIGN), s(350), y, s(90), s(28), true);
            let _ = MoveWindow(field(hwnd, KEYS_RESET), (w - s(124)).max(s(446)), y, s(100), s(28), true);
        }
        let _ = MoveWindow(field(hwnd, ERROR), s(24), h - s(90), (w - s(48)).max(0), s(40), true);
        let _ = MoveWindow(field(hwnd, DEFAULTS), s(12), h - s(42), s(140), s(30), true);
        for (i, id) in [IDOK.0 as usize, IDCANCEL.0 as usize, APPLY].into_iter().enumerate() {
            let _ =
                MoveWindow(field(hwnd, id), w - s(282) + (i as i32) * s(90), h - s(42), s(84), s(30), true);
        }
    }
}

/// Whether `hwnd` is a label or a checkbox (drawn on the dialog's background), not a read-only
/// text box.
fn is_label(hwnd: HWND) -> bool {
    let mut class = [0u16; 16];
    let n = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
    let class = String::from_utf16_lossy(&class[..n]);
    class.eq_ignore_ascii_case("Static") || class.eq_ignore_ascii_case("Button")
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
                id @ (ASSIGN | UNASSIGN | KEYS_RESET) => {
                    edit_keys(hwnd, id);
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
                    match read(hwnd)
                        .and_then(|s| menu_changed.map(|()| s))
                        .and_then(|s| save_keys(hwnd).map(|()| s))
                    {
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
                let code = (*(lp.0 as *const NMHDR)).code;
                if code == TCN_SELCHANGE {
                    layout(hwnd);
                }
                if code == NM_CUSTOMDRAW
                    && let Some(r) = dark::checkbox(&*(lp.0 as *const NMCUSTOMDRAW))
                {
                    return r;
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
                info.ptMinTrackSize.x = scale(hwnd, 620);
                info.ptMinTrackSize.y = scale(hwnd, 600);
                return LRESULT(0);
            }
            WM_CTLCOLORSTATIC if is_label(HWND(lp.0 as _)) => {
                if let Some(r) = dark::static_colors(HDC(wp.0 as _)) {
                    return r;
                }
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX => {
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
