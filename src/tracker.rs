use std::mem::size_of;

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    IsIconic, IsWindowVisible,
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct WindowHandle(pub isize);

#[derive(Clone, Copy, Debug, Default)]
pub struct RectPx {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl RectPx {
    pub fn width(self) -> i32 {
        self.right - self.left
    }

    pub fn height(self) -> i32 {
        self.bottom - self.top
    }
}

#[derive(Clone, Debug)]
pub struct WindowSnapshot {
    pub hwnd: WindowHandle,
    pub title: String,
    pub rect: RectPx,
    pub minimized: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MonitorSnapshot {
    pub rect: RectPx,
}

pub fn set_dpi_awareness_per_monitor_v2() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn enumerate_monitors() -> Vec<MonitorSnapshot> {
    let mut monitors: Vec<MonitorSnapshot> = Vec::new();
    unsafe {
        let lparam = LPARAM((&mut monitors as *mut Vec<MonitorSnapshot>) as isize);
        let _ = EnumDisplayMonitors(HDC(0), None, Some(enum_monitor_proc), lparam);
    }
    monitors
}

pub fn enumerate_windows() -> Vec<WindowSnapshot> {
    let mut out: Vec<WindowSnapshot> = Vec::new();
    unsafe {
        let lparam = LPARAM((&mut out as *mut Vec<WindowSnapshot>) as isize);
        let _ = EnumWindows(Some(enum_windows_proc), lparam);
    }
    out
}

pub fn get_rect(hwnd: WindowHandle) -> Option<RectPx> {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(HWND(hwnd.0), &mut rect).is_ok() {
            return Some(RectPx {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            });
        }
    }
    None
}

pub fn is_window_minimized(hwnd: WindowHandle) -> bool {
    unsafe { IsIconic(HWND(hwnd.0)).as_bool() }
}

fn read_window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }

        let mut buf = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buf);
        if copied <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..copied as usize])
    }
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<WindowSnapshot>);

    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }

    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return BOOL(1);
    }
    if (rect.right - rect.left) < 50 || (rect.bottom - rect.top) < 50 {
        return BOOL(1);
    }

    let title = read_window_text(hwnd);

    out.push(WindowSnapshot {
        hwnd: WindowHandle(hwnd.0),
        title,
        rect: RectPx {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        },
        minimized: IsIconic(hwnd).as_bool(),
    });

    BOOL(1)
}

unsafe extern "system" fn enum_monitor_proc(
    _monitor: HMONITOR,
    _hdc: HDC,
    rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<MonitorSnapshot>);
    if rect.is_null() {
        return BOOL(1);
    }
    let r = *rect;
    if size_of::<RECT>() > 0 {
        out.push(MonitorSnapshot {
            rect: RectPx {
                left: r.left,
                top: r.top,
                right: r.right,
                bottom: r.bottom,
            },
        });
    }
    BOOL(1)
}