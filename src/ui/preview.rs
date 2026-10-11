//! Preview pane (Alt+P). Text files: read on a worker, shown in a read-only
//! box. Other files: the type's preview handler, created out of process
//! (Windows' prevhost.exe), driven from a dedicated thread because cross-process
//! calls can block. No handler: the thumbnail. All results carry a token; a
//! newer selection makes older results stale.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, HDC, HGDIOBJ};
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance, STGM_READ, STGM_SHARE_DENY_NONE};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemServices::{SS_BITMAP, SS_CENTER, SS_CENTERIMAGE};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    IPreviewHandler, IShellItemImageFactory, SHCreateItemFromParsingName, SHCreateStreamOnFileEx,
    SIIGBF_BIGGERSIZEOK, SIIGBF_THUMBNAILONLY,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{GUID, HSTRING, Interface, PCWSTR, w};

use super::grid::Bitmap;
use super::{Event, child, spawn, with_app};
use crate::preview::{handler_for, is_text, read_text};

const CLASS: PCWSTR = w!("KestrelPreview");
const TIMER: usize = 7;
/// Wait this long after the selection settles before previewing.
const DEBOUNCE_MS: u32 = 150;

/// Controls inside the pane; exactly one is visible at a time.
pub(super) struct Pane {
    pub(super) hwnd: HWND,
    text: HWND,
    image: HWND,
    message: HWND,
    /// Parent for preview handler windows.
    host: HWND,
    token: u64,
    worker: Option<(Sender<Cmd>, u32)>,
    bitmap: Option<Bitmap>,
}

enum Cmd {
    Show { token: u64, path: PathBuf, handler: Option<GUID>, host: isize, rect: RECT },
    Resize(RECT),
    Clear,
}

/// What a preview attempt produced.
pub(super) enum Shown {
    Text(String),
    Handler,
    Image(Bitmap),
    Nothing(&'static str),
}

pub(super) unsafe fn create(parent: HWND) -> windows::core::Result<Pane> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(pane_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let none = WINDOW_EX_STYLE(0);
        let hwnd = child(parent, CLASS, w!(""), WS_CLIPCHILDREN, none, 0)?;
        let _ = ShowWindow(hwnd, SW_HIDE);
        let style =
            WS_VSCROLL | WS_HSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32);
        let text = child(hwnd, w!("EDIT"), w!(""), style, none, 0)?;
        let image = child(hwnd, w!("STATIC"), w!(""), WINDOW_STYLE(SS_BITMAP.0 | SS_CENTERIMAGE.0), none, 0)?;
        let message =
            child(hwnd, w!("STATIC"), w!(""), WINDOW_STYLE(SS_CENTER.0 | SS_CENTERIMAGE.0), none, 0)?;
        let host = child(hwnd, w!("STATIC"), w!(""), WS_CLIPCHILDREN, none, 0)?;
        let pane = Pane { hwnd, text, image, message, host, token: 0, worker: None, bitmap: None };
        show_only(&pane, pane.message, Some("Select a file to preview."));
        Ok(pane)
    }
}

pub(super) fn controls(p: &Pane) -> [HWND; 2] {
    [p.text, p.message]
}

fn show_only(p: &Pane, which: HWND, message: Option<&str>) {
    unsafe {
        for c in [p.text, p.image, p.message, p.host] {
            let _ = ShowWindow(c, if c == which { SW_SHOW } else { SW_HIDE });
        }
        if let Some(m) = message {
            let _ = SetWindowTextW(p.message, &HSTRING::from(m));
        }
    }
}

pub(super) fn is_visible() -> bool {
    with_app(|a| is_visible_in(a)) == Some(true)
}

pub(super) fn is_visible_in(a: &super::App) -> bool {
    unsafe { IsWindowVisible(a.preview.hwnd) }.as_bool()
}

/// Shows the pane at startup if it was open last time.
pub(super) fn restore(main: HWND, visible: bool) {
    if visible && !is_visible() {
        toggle(main);
    }
}

/// Stops the preview thread (the window is closing).
pub(super) fn shutdown() {
    if let Some((bitmap, worker, image)) =
        with_app(|a| (a.preview.bitmap.take(), a.preview.worker.take(), a.preview.image))
    {
        let old = unsafe {
            SendMessageW(image, STM_SETIMAGE, Some(WPARAM(IMAGE_BITMAP.0 as usize)), Some(LPARAM(0)))
        };
        if old.0 != 0 && bitmap.as_ref().is_none_or(|b| b.0 != old.0) {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(old.0 as _));
            }
        }
        drop(bitmap);
        if let Some((tx, thread)) = worker {
            let _ = tx.send(Cmd::Clear);
            unsafe {
                let _ = PostThreadMessageW(thread, WM_APP, WPARAM(0), LPARAM(0));
                let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Alt+P: shows or hides the pane.
pub(super) fn toggle(main: HWND) {
    let Some(pane) = with_app(|a| a.preview.hwnd) else { return };
    let show = !is_visible();
    unsafe {
        let _ = ShowWindow(pane, if show { SW_SHOW } else { SW_HIDE });
    }
    super::layout(main);
    if show {
        selection_changed(main);
    } else {
        send(Cmd::Clear);
    }
}

/// Lays out the pane's children to fill it; tells a running handler its new size.
pub(super) fn resize() {
    let Some((hwnd, kids)) =
        with_app(|a| (a.preview.hwnd, [a.preview.text, a.preview.image, a.preview.message, a.preview.host]))
    else {
        return;
    };
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
        for c in kids {
            let _ = MoveWindow(c, 0, 0, r.right, r.bottom, true);
        }
    }
    send(Cmd::Resize(r));
}

/// The list's selection moved: preview it once it settles.
pub(super) fn selection_changed(main: HWND) {
    if is_visible() {
        unsafe { SetTimer(Some(main), TIMER, DEBOUNCE_MS, None) };
    }
}

/// WM_TIMER from the main window. Returns false if not ours.
pub(super) fn on_timer(main: HWND, id: usize) -> bool {
    if id != TIMER {
        return false;
    }
    unsafe {
        let _ = KillTimer(Some(main), TIMER);
    }
    let path = super::focused_path();
    show(path);
    true
}

fn show(target: Option<(PathBuf, bool)>) {
    let Some((token, host, events)) = with_app(|a| {
        a.preview.token += 1;
        (a.preview.token, a.preview.host, a.events.clone())
    }) else {
        return;
    };
    send(Cmd::Clear);
    let path = match target {
        Some((path, false)) => path,
        _ => return finish(token, Shown::Nothing("Select a file to preview.")),
    };
    if is_text(&path) {
        spawn("preview-text", move || {
            let shown = match crate::crash::guard(|| read_text(&path)).and_then(Result::ok) {
                Some(Some(text)) => Shown::Text(text),
                Some(None) => Shown::Nothing("This file doesn't look like text."),
                None => Shown::Nothing("Can't read this file."),
            };
            events.send(Event::Preview { token, shown });
        });
        return;
    }
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let handler = handler_for(&ext);
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(host, &mut rect);
    }
    if handler.is_some() {
        with_app(|a| show_only(&a.preview, a.preview.host, None));
    } else {
        with_app(|a| show_only(&a.preview, a.preview.message, Some("Loading preview…")));
    }
    ensure_worker();
    send(Cmd::Show { token, path, handler, host: host.0 as isize, rect });
}

/// Applies a preview result on the UI thread (stale tokens are ignored).
pub(super) fn finish(token: u64, shown: Shown) {
    let Some((current, text, image, message)) =
        with_app(|a| (a.preview.token, a.preview.text, a.preview.image, a.preview.message))
    else {
        return;
    };
    if token != current {
        return;
    }
    match shown {
        Shown::Text(t) => {
            unsafe {
                let _ = SetWindowTextW(text, &HSTRING::from(t));
            }
            with_app(|a| show_only(&a.preview, text, None));
        }
        Shown::Handler => {}
        Shown::Image(b) => {
            let old = unsafe {
                SendMessageW(image, STM_SETIMAGE, Some(WPARAM(IMAGE_BITMAP.0 as usize)), Some(LPARAM(b.0)))
            };
            let previous = with_app(|a| a.preview.bitmap.replace(b)).flatten();
            if old.0 != 0 && previous.as_ref().is_none_or(|p| p.0 != old.0) {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(old.0 as _));
                }
            }
            drop(previous);
            with_app(|a| show_only(&a.preview, image, None));
        }
        Shown::Nothing(m) => {
            with_app(|a| show_only(&a.preview, message, Some(m)));
        }
    }
}

fn send(cmd: Cmd) {
    if let Some((tx, thread)) = with_app(|a| a.preview.worker.clone()).flatten() {
        let _ = tx.send(cmd);
        unsafe {
            let _ = PostThreadMessageW(thread, WM_APP, WPARAM(0), LPARAM(0));
        }
    }
}

fn ensure_worker() {
    if with_app(|a| a.preview.worker.is_some()) != Some(false) {
        return;
    }
    let Some(events) = with_app(|a| a.events.clone()) else { return };
    let (tx, rx) = mpsc::channel();
    let (id_tx, id_rx) = mpsc::channel();
    spawn("preview", move || {
        // Create the thread's message queue before saying where to post:
        // messages posted to a thread without one are lost.
        let mut msg = MSG::default();
        unsafe {
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        }
        let _ = id_tx.send(unsafe { GetCurrentThreadId() });
        crate::crash::guard(|| worker(rx, |token, shown| events.send(Event::Preview { token, shown })));
    });
    if let Ok(thread) = id_rx.recv_timeout(std::time::Duration::from_secs(2)) {
        with_app(|a| a.preview.worker = Some((tx, thread)));
    }
}

/// The preview thread: owns the handler, pumps messages for it.
fn worker(rx: Receiver<Cmd>, report: impl Fn(u64, Shown)) {
    let _com = crate::shell::Com::init();
    let mut handler: Option<IPreviewHandler> = None;
    let unload = |h: &mut Option<IPreviewHandler>| {
        if let Some(h) = h.take() {
            unsafe {
                let _ = h.Unload();
            }
        }
    };
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Only the newest Show matters; earlier ones are already stale.
        let mut cmds: Vec<Cmd> = rx.try_iter().collect();
        if let Some(last_show) = cmds.iter().rposition(|c| matches!(c, Cmd::Show { .. })) {
            cmds.drain(..last_show);
        }
        for cmd in cmds {
            match cmd {
                Cmd::Clear => unload(&mut handler),
                Cmd::Resize(r) => {
                    if let Some(h) = &handler {
                        unsafe {
                            let _ = h.SetRect(&r);
                        }
                    }
                }
                Cmd::Show { token, path, handler: clsid, host, rect } => {
                    unload(&mut handler);
                    let shown = match clsid.and_then(|c| start_handler(&c, &path, HWND(host as _), &rect)) {
                        Some(h) => {
                            handler = Some(h);
                            Shown::Handler
                        }
                        None => {
                            thumbnail(&path).map_or(Shown::Nothing("No preview available."), Shown::Image)
                        }
                    };
                    report(token, shown);
                }
            }
        }
    }
    unload(&mut handler);
}

/// Creates the handler in Windows' preview host process and shows `path` in `host`.
fn start_handler(clsid: &GUID, path: &Path, host: HWND, rect: &RECT) -> Option<IPreviewHandler> {
    unsafe {
        let h: IPreviewHandler = CoCreateInstance(clsid, None, CLSCTX_LOCAL_SERVER).ok()?;
        let file = HSTRING::from(path.as_os_str());
        if let Ok(init) = h.cast::<IInitializeWithStream>() {
            let stream =
                SHCreateStreamOnFileEx(&file, (STGM_READ | STGM_SHARE_DENY_NONE).0, 0, false, None).ok()?;
            init.Initialize(&stream, STGM_READ.0).ok()?;
        } else {
            h.cast::<IInitializeWithFile>().ok()?.Initialize(&file, STGM_READ.0).ok()?;
        }
        h.SetWindow(host, rect).ok()?;
        h.DoPreview().ok()?;
        Some(h)
    }
}

/// A real thumbnail (not just the type's icon), rendered by Windows' thumbnail cache.
fn thumbnail(path: &Path) -> Option<Bitmap> {
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
        let bmp =
            factory.GetImage(SIZE { cx: 512, cy: 512 }, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK).ok()?;
        Some(Bitmap(bmp.0 as isize))
    }
}

/// Pane window: colors its text in dark mode, otherwise default handling.
extern "system" fn pane_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::crash::guard(|| {
        if matches!(msg, WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT)
            && let Some(r) = super::dark::edit_colors(HDC(wp.0 as _))
        {
            return r;
        }
        if msg == WM_ERASEBKGND && super::dark::erase(hwnd, HDC(wp.0 as _)) {
            return LRESULT(1);
        }
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    })
    .unwrap_or(LRESULT(0))
}

pub(super) fn invalidate() {
    with_app(|a| a.preview.token += 1);
    send(Cmd::Clear);
    with_app(|a| show_only(&a.preview, a.preview.message, Some("Select a file to preview.")));
    if let Some(hwnd) = with_app(|a| a.hwnd) {
        selection_changed(hwnd);
    }
}
