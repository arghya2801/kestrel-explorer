//! keys.txt bindings, in their own process: bindings load once, on the first key press.

use std::time::{Duration, Instant};

use kestrel::ui;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

#[test]
fn a_keys_txt_binding_runs_its_command() {
    let data = kestrel::crash::data_dir();
    assert!(
        data.starts_with(env!("CARGO_TARGET_TMPDIR").trim_end_matches("tmp")),
        "set LOCALAPPDATA as in HANDOFF.md"
    );
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("keys.txt"), "# test binding\nCtrl+Shift+F7 = shortcuts\n").unwrap();
    let dir = std::env::temp_dir();
    ui::init_process().unwrap();
    let hwnd = ui::create_main_window(&dir, &kestrel::settings::Settings::default()).unwrap();
    let list = ui::active_list().unwrap();
    unsafe {
        let mut keys = [0u8; 256];
        GetKeyboardState(&mut keys).unwrap();
        keys[VK_CONTROL.0 as usize] = 0x80;
        keys[VK_SHIFT.0 as usize] = 0x80;
        SetKeyboardState(&keys).unwrap();
        PostMessageW(Some(list), WM_KEYDOWN, WPARAM(VK_F7.0 as usize), LPARAM(0)).unwrap();
        let start = Instant::now();
        let mut msg = MSG::default();
        while start.elapsed() < Duration::from_secs(5) && FindWindowW(w!("KestrelShortcuts"), None).is_err() {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if !ui::translate(&msg) {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        keys[VK_CONTROL.0 as usize] = 0;
        keys[VK_SHIFT.0 as usize] = 0;
        SetKeyboardState(&keys).unwrap();
        let guide =
            FindWindowW(w!("KestrelShortcuts"), None).expect("Ctrl+Shift+F7 opened the shortcut guide");
        let text = {
            let edit = GetDlgItem(Some(guide), 30).unwrap();
            let mut buf = vec![0u16; 8192];
            let n = GetWindowTextW(edit, &mut buf);
            String::from_utf16_lossy(&buf[..n as usize])
        };
        assert!(text.starts_with("Your keys"), "the guide lists the binding first");
        assert!(text.contains("Ctrl+Shift+F7    shortcuts"));
        DestroyWindow(guide).unwrap();
        DestroyWindow(hwnd).unwrap();
    }
    std::fs::remove_file(data.join("keys.txt")).unwrap();
}
