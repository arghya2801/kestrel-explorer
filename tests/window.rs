use file_explorer_pro::ui;
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, IsWindow};

#[test]
fn main_window_creates_and_destroys() {
    ui::init_process().unwrap();
    let hwnd = ui::create_main_window().unwrap();
    assert!(unsafe { IsWindow(Some(hwnd)) }.as_bool());
    unsafe { DestroyWindow(hwnd) }.unwrap();
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
}
