use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreatePen, CreateSolidBrush,
    DeleteDC, DeleteObject, EndPaint, FillRect, GetStockObject, InvalidateRect, RedrawWindow,
    SelectObject, SetBkMode, SetTextColor, TextOutW, SetDIBitsToDevice, NULL_BRUSH, PAINTSTRUCT,
    PEN_STYLE, PS_DASH, RDW_INVALIDATE, RDW_UPDATENOW, SRCCOPY, TRANSPARENT, BITMAPINFO,
    BITMAPINFOHEADER, DIB_RGB_COLORS,
};
use windows::Win32::UI::Accessibility::SetWinEventHook;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetClientRect, GetCursorPos, GetWindow, GetWindowRect,
    LoadCursorW, PeekMessageW, RegisterClassExW, SetLayeredWindowAttributes,
    SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, GW_HWNDPREV, HMENU,
    HWND_TOP, IDC_ARROW, LWA_ALPHA, LWA_COLORKEY, MSG, PM_REMOVE, SW_HIDE, SW_SHOWNA,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOSENDCHANGING, SWP_NOZORDER,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::boxes::{Anchor, RedactBox};
use crate::state::{AppMode, AppState, ImageFillMode, MonitorInfo, RedactionStyle};
use crate::tracker::{self, WindowHandle};

const REDACTION_CLASS: &str = "BlindSpotRedactionOverlayClass";
const SELECTION_CLASS: &str = "BlindSpotSelectionOverlayClass";
const COLORKEY: COLORREF = COLORREF(0x00010001);

const BTN_SIZE: f32 = 24.0;

struct CachedImage {
    path: String,
    width: i32,
    height: i32,
    bgr_pixels: Vec<u8>,
}

static CACHED_IMAGE: OnceLock<Mutex<Option<CachedImage>>> = OnceLock::new();

struct OverlayShared {
    overlay_to_target: HashMap<isize, WindowHandle>,
    selection_windows: HashSet<isize>,
}

static OVERLAY_SHARED: OnceLock<Arc<Mutex<OverlayShared>>> = OnceLock::new();
static APP_STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();
static WIN_EVENT_DIRTY: AtomicBool = AtomicBool::new(false);

const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const EVENT_OBJECT_REORDER: u32 = 0x8004;
const EVENT_SYSTEM_MOVESIZESTART: u32 = 0x000A;
const EVENT_SYSTEM_MOVESIZEEND: u32 = 0x000B;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_OBJECT_STATECHANGE: u32 = 0x800A;
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;

#[derive(Clone, Debug)]
struct RedactionTrack {
    rect_left: i32,
    rect_top: i32,
    rect_w: i32,
    rect_h: i32,
    visible: bool,
    box_count: usize,
}

enum EditHit {
    AnchorTopLeft,
    AnchorTopRight,
    AnchorBottomLeft,
    AnchorBottomRight,
    Delete,
    MoveBody,
    None,
}

#[derive(Clone, Debug)]
struct BoxDrag {
    box_index: usize,
    kind: BoxDragKind,
    start_screen: (f32, f32),
    orig_left: f32,
    orig_top: f32,
    orig_right: f32,
    orig_bottom: f32,
    is_dragging: bool,
}

#[derive(Clone, Copy, Debug)]
enum BoxDragKind {
    Move,
    ResizeTopLeft,
    ResizeTopRight,
    ResizeBottomLeft,
    ResizeBottomRight,
}

static BOX_DRAG: OnceLock<Mutex<Option<BoxDrag>>> = OnceLock::new();

pub fn spawn_overlay_thread(state: Arc<Mutex<AppState>>) {
    thread::spawn(move || overlay_thread_main(state));
}

unsafe extern "system" fn win_event_proc(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _event_time: u32,
) {
    WIN_EVENT_DIRTY.store(true, Ordering::Relaxed);
}

fn overlay_thread_main(state: Arc<Mutex<AppState>>) {
    unsafe {
        windows::Win32::Media::timeBeginPeriod(1);
    }

    let shared = Arc::new(Mutex::new(OverlayShared {
        overlay_to_target: HashMap::new(),
        selection_windows: HashSet::new(),
    }));
    let _ = OVERLAY_SHARED.set(shared);
    let _ = APP_STATE.set(state.clone());
    register_overlay_classes();

    unsafe {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let _ = SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE,
            None, Some(win_event_proc), 0, 0, flags,
        );
        let _ = SetWinEventHook(
            EVENT_OBJECT_REORDER, EVENT_OBJECT_REORDER,
            None, Some(win_event_proc), 0, 0, flags,
        );
        let _ = SetWinEventHook(
            EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND,
            None, Some(win_event_proc), 0, 0, flags,
        );
        let _ = SetWinEventHook(
            EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND,
            None, Some(win_event_proc), 0, 0, flags,
        );
        let _ = SetWinEventHook(
            EVENT_OBJECT_STATECHANGE, EVENT_OBJECT_STATECHANGE,
            None, Some(win_event_proc), 0, 0, flags,
        );
    }

    let mut redaction_windows: HashMap<WindowHandle, (WindowHandle, RedactionTrack)> =
        HashMap::new();
    let mut selection_windows: HashMap<usize, WindowHandle> = HashMap::new();

    loop {
        pump_messages();

        for (target, (ov, track)) in &mut redaction_windows {
            let current_rect = tracker::get_rect(*target);
            let minimized = tracker::is_window_minimized(*target);
            let Some(rect) = current_rect else { continue };

            let vis = !minimized;
            if vis != track.visible {
                unsafe {
                    let _ = ShowWindow(HWND(ov.0), if vis { SW_SHOWNA } else { SW_HIDE });
                }
                track.visible = vis;
            }
            if vis {
                let nr = (rect.left, rect.top, rect.width(), rect.height());
                let or = (track.rect_left, track.rect_top, track.rect_w, track.rect_h);

                if nr != or {
                    let above = unsafe { GetWindow(HWND(target.0), GW_HWNDPREV) };
                    let need_z = above.0 != 0 && above.0 != ov.0;
                    let z = if need_z { above } else { HWND_TOP };
                    unsafe {
                        let _ = SetWindowPos(
                            HWND(ov.0), z, nr.0, nr.1, nr.2, nr.3,
                            SWP_NOACTIVATE | SWP_NOSENDCHANGING
                                | if !need_z { SWP_NOZORDER } else { SWP_NOSENDCHANGING },
                        );
                        let _ = InvalidateRect(HWND(ov.0), None, false);
                    }
                    track.rect_left = nr.0;
                    track.rect_top = nr.1;
                    track.rect_w = nr.2;
                    track.rect_h = nr.3;
                } else {
                    let above = unsafe { GetWindow(HWND(target.0), GW_HWNDPREV) };
                    if above.0 != 0 && above.0 != ov.0 {
                        unsafe {
                            let _ = SetWindowPos(
                                HWND(ov.0), above, 0, 0, 0, 0,
                                SWP_NOACTIVATE | SWP_NOSENDCHANGING | SWP_NOMOVE | SWP_NOSIZE,
                            );
                        }
                    }
                }
            }
        }

        let (mode, tracked, z_ordered, monitors, boxes, redaction_overlays_enabled, style, repaint_needed) = {
            let mut s = state.lock().expect("lock");
            let rn = s.redaction_repaint_needed;
            if rn { s.redaction_repaint_needed = false; }
            (
                s.mode.clone(),
                s.tracked_windows.clone(),
                s.z_ordered_windows.clone(),
                s.monitor_infos.clone(),
                s.boxes.clone(),
                s.redaction_overlays_enabled,
                s.config.redaction_style.clone(),
                rn,
            )
        };

        if !matches!(mode, AppMode::Idle) {
            let esc_down = unsafe { (GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16) & 0x8000 != 0 };
            if esc_down {
                let mut s = state.lock().expect("lock");
                s.mode = AppMode::Idle;
                s.selected_window = None;
                s.selected_box = None;
                s.pending_new_box_drag = None;
                s.redaction_overlays_enabled = true;
                let drag_lock = BOX_DRAG.get_or_init(|| Mutex::new(None));
                if let Ok(mut d) = drag_lock.lock() { *d = None; }
            }
        }

        let need_selection = !matches!(mode, AppMode::Idle);
        if need_selection {
            ensure_selection_overlays(&mut selection_windows, &monitors);
        } else {
            destroy_selection_overlays(&mut selection_windows);
        }

        if matches!(mode, AppMode::WindowSelect) {
            let mut cursor = POINT::default();
            unsafe { let _ = GetCursorPos(&mut cursor); }
            let hovered = topmost_window_from_point(
                (cursor.x as f32, cursor.y as f32),
                &z_ordered, &tracked, &redaction_windows, &selection_windows,
            );
            let changed = {
                let mut s = state.lock().expect("lock");
                let prev = s.hovered_window;
                s.hovered_window = hovered;
                prev != hovered
            };
            if changed { invalidate_all_selection(&selection_windows); }
        }

        if matches!(mode, AppMode::DrawTarget(_)) {
            let has_drag = state.lock().expect("lock").pending_new_box_drag.is_some();
            if has_drag { invalidate_all_selection(&selection_windows); }
        }

        let mut active_targets = HashSet::new();
        if redaction_overlays_enabled {
            for hwnd in boxes.keys() {
                if boxes.get(hwnd).map(|b| !b.is_empty()).unwrap_or(false) {
                    if tracked.contains_key(hwnd) {
                        active_targets.insert(*hwnd);
                    }
                }
            }
        }

        for target in &active_targets {
            if !redaction_windows.contains_key(target) {
                if let Some(win) = tracked.get(target) {
                    if let Some(ov) = create_redaction_overlay(*target, win.rect) {
                        let bc = boxes.get(target).map(|b| b.len()).unwrap_or(0);
                        redaction_windows.insert(*target, (ov, RedactionTrack {
                            rect_left: win.rect.left, rect_top: win.rect.top,
                            rect_w: win.rect.width(), rect_h: win.rect.height(),
                            visible: true, box_count: bc,
                        }));
                        with_shared(|s| { s.overlay_to_target.insert(ov.0, *target); });
                    }
                }
            }
        }

        let keys: Vec<_> = redaction_windows.keys().copied().collect();
        for t in keys {
            if !active_targets.contains(&t) || !tracked.contains_key(&t) {
                if let Some((ov, _)) = redaction_windows.remove(&t) {
                    with_shared(|s| { s.overlay_to_target.remove(&ov.0); });
                    unsafe { let _ = DestroyWindow(HWND(ov.0)); }
                }
            }
        }

        for (target, (ov, track)) in &mut redaction_windows {
            let count = boxes.get(target).map(|b| b.len()).unwrap_or(0);
            if count != track.box_count {
                track.box_count = count;
                if track.visible {
                    unsafe { let _ = InvalidateRect(HWND(ov.0), None, false); }
                }
            }
        }

        if repaint_needed {
            for (_, (ov, track)) in &redaction_windows {
                if track.visible {
                    unsafe { let _ = InvalidateRect(HWND(ov.0), None, false); }
                }
            }
        }

        let has_active_drag = BOX_DRAG.get()
            .and_then(|l| l.lock().ok())
            .map(|d| d.as_ref().map(|bd| bd.is_dragging).unwrap_or(false))
            .unwrap_or(false);
        let has_new_box_drag = state.lock().expect("lock").pending_new_box_drag.is_some();
        if has_active_drag || has_new_box_drag {
            for (_, (ov, track)) in &redaction_windows {
                if track.visible {
                    unsafe { let _ = InvalidateRect(HWND(ov.0), None, false); }
                }
            }
        }

        if matches!(style, RedactionStyle::AnimatedNoise) {
            for (_, (ov, track)) in &redaction_windows {
                if track.visible {
                    unsafe { let _ = InvalidateRect(HWND(ov.0), None, false); }
                }
            }
        }

        if redaction_windows.is_empty() && selection_windows.is_empty() {
            thread::sleep(Duration::from_millis(100));
        } else {
            let target = std::time::Instant::now() + Duration::from_millis(4);
            thread::sleep(Duration::from_millis(2));
            while std::time::Instant::now() < target {
                std::hint::spin_loop();
            }
        }
    }

}

fn invalidate_all_selection(sel: &HashMap<usize, WindowHandle>) {
    for h in sel.values() {
        unsafe {
            let _ = InvalidateRect(HWND(h.0), None, false);
        }
    }
}

fn ensure_selection_overlays(sel: &mut HashMap<usize, WindowHandle>, monitors: &[MonitorInfo]) {
    for (i, m) in monitors.iter().enumerate() {
        if sel.contains_key(&i) {
            continue;
        }
        if let Some(h) = create_selection_overlay(i, *m) {
            sel.insert(i, h);
            with_shared(|s| {
                s.selection_windows.insert(h.0);
            });
        }
    }
}

fn destroy_selection_overlays(sel: &mut HashMap<usize, WindowHandle>) {
    for h in sel.values() {
        with_shared(|s| {
            s.selection_windows.remove(&h.0);
        });
        unsafe {
            let _ = DestroyWindow(HWND(h.0));
        }
    }
    sel.clear();
}

fn topmost_window_from_point(
    p: (f32, f32),
    z: &[WindowHandle],
    tracked: &HashMap<WindowHandle, crate::state::TrackedWindow>,
    redaction: &HashMap<WindowHandle, (WindowHandle, RedactionTrack)>,
    selection: &HashMap<usize, WindowHandle>,
) -> Option<WindowHandle> {
    let mut ignore = HashSet::new();
    for (ov, _) in redaction.values() {
        ignore.insert(ov.0);
    }
    for v in selection.values() {
        ignore.insert(v.0);
    }
    for h in z {
        if ignore.contains(&h.0) {
            continue;
        }
        if let Some(w) = tracked.get(h) {
            let r = w.rect;
            if !w.minimized
                && p.0 >= r.left as f32
                && p.0 <= r.right as f32
                && p.1 >= r.top as f32
                && p.1 <= r.bottom as f32
            {
                return Some(*h);
            }
        }
    }
    None
}

fn hit_edit_button(box_px: &crate::boxes::BoxRectPx, point: (f32, f32)) -> EditHit {
    let cx = box_px.x + box_px.w / 2.0;
    let cy = box_px.y + box_px.h / 2.0;
    let half = BTN_SIZE / 2.0;

    if (point.0 - cx).abs() < half && (point.1 - cy).abs() < half {
        return EditHit::Delete;
    }
    if (point.0 - box_px.x).abs() < BTN_SIZE && (point.1 - box_px.y).abs() < BTN_SIZE {
        return EditHit::AnchorTopLeft;
    }
    if (point.0 - (box_px.x + box_px.w)).abs() < BTN_SIZE
        && (point.1 - box_px.y).abs() < BTN_SIZE
    {
        return EditHit::AnchorTopRight;
    }
    if (point.0 - box_px.x).abs() < BTN_SIZE
        && (point.1 - (box_px.y + box_px.h)).abs() < BTN_SIZE
    {
        return EditHit::AnchorBottomLeft;
    }
    if (point.0 - (box_px.x + box_px.w)).abs() < BTN_SIZE
        && (point.1 - (box_px.y + box_px.h)).abs() < BTN_SIZE
    {
        return EditHit::AnchorBottomRight;
    }
    if point.0 >= box_px.x
        && point.0 <= box_px.x + box_px.w
        && point.1 >= box_px.y
        && point.1 <= box_px.y + box_px.h
    {
        return EditHit::MoveBody;
    }
    EditHit::None
}

fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

unsafe extern "system" fn redaction_wndproc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    if msg == WM_PAINT {
        paint_redaction(hwnd);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe extern "system" fn selection_wndproc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_selection(hwnd);
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            let p = (cur.x as f32, cur.y as f32);

            let box_drag_lock = BOX_DRAG.get_or_init(|| Mutex::new(None));
            let did_drag = {
                let mut drag = box_drag_lock.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(ref mut bd) = *drag {
                    let dx = (p.0 - bd.start_screen.0).abs();
                    let dy = (p.1 - bd.start_screen.1).abs();
                    if !bd.is_dragging && (dx > 4.0 || dy > 4.0) {
                        bd.is_dragging = true;
                    }
                    if bd.is_dragging {
                        if let Some(app) = APP_STATE.get() {
                            if let Ok(mut st) = app.lock() {
                                if let AppMode::DrawTarget(target) = st.mode.clone() {
                                    let win_rect = st.tracked_windows.get(&target)
                                        .map(|w| w.rect);
                                    if let Some(rect) = win_rect {
                                        if let Some(box_list) = st.boxes.get_mut(&target) {
                                            if bd.box_index < box_list.len() {
                                                let mouse_dx = p.0 - bd.start_screen.0;
                                                let mouse_dy = p.1 - bd.start_screen.1;
                                                let b = &mut box_list[bd.box_index];
                                                match bd.kind {
                                                    BoxDragKind::Move => {
                                                        let new_left = bd.orig_left + mouse_dx;
                                                        let new_top = bd.orig_top + mouse_dy;
                                                        b.w_px = bd.orig_right - bd.orig_left;
                                                        b.h_px = bd.orig_bottom - bd.orig_top;
                                                        b.compute_offsets_from_position(new_left, new_top, rect);
                                                    }
                                                    BoxDragKind::ResizeTopLeft => {
                                                        let new_left = bd.orig_left + mouse_dx;
                                                        let new_top = bd.orig_top + mouse_dy;
                                                        b.resize_to_pixels(rect,
                                                            new_left, new_top,
                                                            bd.orig_right, bd.orig_bottom);
                                                    }
                                                    BoxDragKind::ResizeTopRight => {
                                                        let new_right = bd.orig_right + mouse_dx;
                                                        let new_top = bd.orig_top + mouse_dy;
                                                        b.resize_to_pixels(rect,
                                                            bd.orig_left, new_top,
                                                            new_right, bd.orig_bottom);
                                                    }
                                                    BoxDragKind::ResizeBottomLeft => {
                                                        let new_left = bd.orig_left + mouse_dx;
                                                        let new_bottom = bd.orig_bottom + mouse_dy;
                                                        b.resize_to_pixels(rect,
                                                            new_left, bd.orig_top,
                                                            bd.orig_right, new_bottom);
                                                    }
                                                    BoxDragKind::ResizeBottomRight => {
                                                        let new_right = bd.orig_right + mouse_dx;
                                                        let new_bottom = bd.orig_bottom + mouse_dy;
                                                        b.resize_to_pixels(rect,
                                                            bd.orig_left, bd.orig_top,
                                                            new_right, new_bottom);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            let new_box_drag = if let Some(app) = APP_STATE.get() {
                app.lock().map(|st| st.pending_new_box_drag.is_some()).unwrap_or(false)
            } else { false };

            if did_drag || new_box_drag {
                let _ = InvalidateRect(hwnd, None, false);

                let ov_hwnds: Vec<isize> = if let Some(shared) = OVERLAY_SHARED.get() {
                    if let Ok(s) = shared.lock() {
                        s.overlay_to_target.keys().copied().collect()
                    } else { vec![] }
                } else { vec![] };
                for ov in ov_hwnds {
                    let _ = RedrawWindow(
                        HWND(ov),
                        None,
                        None,
                        RDW_INVALIDATE | RDW_UPDATENOW,
                    );
                }
            }
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            let p = (cur.x as f32, cur.y as f32);

            if let Some(app) = APP_STATE.get() {
                if let Ok(mut st) = app.lock() {
                    match st.mode.clone() {
                        AppMode::WindowSelect => {
                            if let Some(hovered) = st.hovered_window {
                                st.mode = AppMode::DrawTarget(hovered);
                                st.selected_window = Some(hovered);
                                st.selected_box = None;

                                let win_rect =
                                    st.tracked_windows.get(&hovered).map(|w| w.rect);

                                if let Some(rect) = win_rect {
                                    let mut handled = false;
                                    if let Some(box_list) = st.boxes.get(&hovered) {
                                        for (i, b) in box_list.iter().enumerate().rev() {
                                            if b.hit_test(rect, p) {
                                                st.selected_box = Some(i);
                                                handled = true;
                                                break;
                                            }
                                        }
                                    }
                                    if !handled {
                                        st.pending_new_box_drag = Some((hovered, p));
                                        let _ = SetCapture(hwnd);
                                    }
                                } else {
                                    st.pending_new_box_drag = Some((hovered, p));
                                    let _ = SetCapture(hwnd);
                                }
                            }
                        }
                        AppMode::DrawTarget(target) => {
                            let win_rect =
                                st.tracked_windows.get(&target).map(|w| w.rect);

                            if let Some(rect) = win_rect {
                                let mut edit_handled = false;
                                if let Some(sel_idx) = st.selected_box {
                                    if let Some(box_list) = st.boxes.get(&target) {
                                        if sel_idx < box_list.len() {
                                            let bpx =
                                                box_list[sel_idx].to_pixels(rect);
                                            let hit = hit_edit_button(&bpx, p);
                                            match hit {
                                                EditHit::Delete => {
                                                    st.boxes
                                                        .get_mut(&target)
                                                        .unwrap()
                                                        .remove(sel_idx);
                                                    st.selected_box = None;
                                                    edit_handled = true;
                                                }
                                                EditHit::AnchorTopLeft
                                                | EditHit::AnchorTopRight
                                                | EditHit::AnchorBottomLeft
                                                | EditHit::AnchorBottomRight
                                                | EditHit::MoveBody => {
                                                    let kind = match hit {
                                                        EditHit::AnchorTopLeft => BoxDragKind::ResizeTopLeft,
                                                        EditHit::AnchorTopRight => BoxDragKind::ResizeTopRight,
                                                        EditHit::AnchorBottomLeft => BoxDragKind::ResizeBottomLeft,
                                                        EditHit::AnchorBottomRight => BoxDragKind::ResizeBottomRight,
                                                        EditHit::MoveBody => BoxDragKind::Move,
                                                        _ => unreachable!(),
                                                    };
                                                    let drag_lock = BOX_DRAG.get_or_init(|| Mutex::new(None));
                                                    if let Ok(mut drag) = drag_lock.lock() {
                                                        *drag = Some(BoxDrag {
                                                            box_index: sel_idx,
                                                            kind,
                                                            start_screen: p,
                                                            orig_left: bpx.x,
                                                            orig_top: bpx.y,
                                                            orig_right: bpx.x + bpx.w,
                                                            orig_bottom: bpx.y + bpx.h,
                                                            is_dragging: false,
                                                        });
                                                    }
                                                    let _ = SetCapture(hwnd);
                                                    edit_handled = true;
                                                }
                                                EditHit::None => {}
                                            }
                                        }
                                    }
                                }

                                if !edit_handled {
                                    let mut box_clicked = false;
                                    if let Some(box_list) = st.boxes.get(&target) {
                                        for (i, b) in box_list.iter().enumerate().rev() {
                                            if b.hit_test(rect, p) {
                                                st.selected_box = Some(i);
                                                box_clicked = true;
                                                break;
                                            }
                                        }
                                    }
                                    if !box_clicked {
                                        st.selected_box = None;
                                        st.pending_new_box_drag = Some((target, p));
                                        let _ = SetCapture(hwnd);
                                    }
                                }
                            }
                        }
                        AppMode::Idle => {}
                    }
                }
            }
            invalidate_all_sel_from_hwnd(hwnd);
            LRESULT(0)
        }

        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            let p = (cur.x as f32, cur.y as f32);

            let drag_lock = BOX_DRAG.get_or_init(|| Mutex::new(None));
            let finished_drag = drag_lock.lock()
                .map(|mut d| d.take())
                .unwrap_or(None);

            if let Some(bd) = finished_drag {
                if !bd.is_dragging {
                    if let Some(app) = APP_STATE.get() {
                        if let Ok(mut st) = app.lock() {
                            if let AppMode::DrawTarget(target) = st.mode.clone() {
                                let win_rect = st.tracked_windows.get(&target)
                                    .map(|w| w.rect);
                                if let Some(rect) = win_rect {
                                    if let Some(box_list) = st.boxes.get_mut(&target) {
                                        if bd.box_index < box_list.len() {
                                            let anchor = match bd.kind {
                                                BoxDragKind::ResizeTopLeft => Some(Anchor::TopLeft),
                                                BoxDragKind::ResizeTopRight => Some(Anchor::TopRight),
                                                BoxDragKind::ResizeBottomLeft => Some(Anchor::BottomLeft),
                                                BoxDragKind::ResizeBottomRight => Some(Anchor::BottomRight),
                                                BoxDragKind::Move => None, // body click = no anchor change
                                            };
                                            if let Some(a) = anchor {
                                                let b = &mut box_list[bd.box_index];
                                                b.set_anchor(a, rect);
                                                b.manual_anchor = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                if let Some(app) = APP_STATE.get() {
                    if let Ok(mut st) = app.lock() {
                        if let Some((drag_hwnd, start)) = st.pending_new_box_drag.take() {
                            if let Some(win) = st.tracked_windows.get(&drag_hwnd).cloned() {
                                let dx = (p.0 - start.0).abs();
                                let dy = (p.1 - start.1).abs();
                                if dx >= 10.0 && dy >= 10.0 {
                                    let box_list = st.boxes_for_window_mut(drag_hwnd);
                                    box_list
                                        .push(RedactBox::from_drag(start, p, win.rect));
                                    let new_idx = box_list.len() - 1;
                                    st.selected_box = Some(new_idx);
                                }
                            }
                        }
                    }
                }
            }
            invalidate_all_sel_from_hwnd(hwnd);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn invalidate_all_sel_from_hwnd(_hwnd: HWND) {
    if let Some(shared) = OVERLAY_SHARED.get() {
        if let Ok(s) = shared.lock() {
            for &h in &s.selection_windows {
                unsafe {
                    let _ = InvalidateRect(HWND(h), None, false);
                }
            }
        }
    }
}

fn paint_redaction(hwnd: HWND) {
    let target = match OVERLAY_SHARED.get().and_then(|s| s.lock().ok()) {
        Some(s) => match s.overlay_to_target.get(&hwnd.0).copied() {
            Some(t) => t,
            None => return,
        },
        None => return,
    };
    let (box_list, color, style, custom_image_path, image_fill_mode) = {
        let app = match APP_STATE.get().and_then(|a| a.lock().ok()) {
            Some(a) => a,
            None => return,
        };
        let b = app.boxes.get(&target).cloned();
        let c = app.config.redaction_color;
        let s = app.config.redaction_style.clone();
        let img = app.config.custom_image_path.clone();
        let fm = app.config.image_fill_mode.clone();
        (b, c, s, img, fm)
    };

    let live_rect = match tracker::get_rect(target) {
        Some(r) => r,
        None => return,
    };

    let mut ps = PAINTSTRUCT::default();
    let hdc_screen = unsafe { BeginPaint(hwnd, &mut ps) };
    if hdc_screen.0 == 0 {
        return;
    }
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }

    let cw = client.right - client.left;
    let ch = client.bottom - client.top;
    if cw <= 0 || ch <= 0 {
        unsafe { let _ = EndPaint(hwnd, &ps); }
        return;
    }

    let hdc = unsafe { CreateCompatibleDC(hdc_screen) };
    let hbm = unsafe { CreateCompatibleBitmap(hdc_screen, cw, ch) };
    let old_bm = unsafe { SelectObject(hdc, hbm) };

    unsafe {
        let br = CreateSolidBrush(COLORKEY);
        let _ = FillRect(hdc, &client, br);
        let _ = DeleteObject(br);
    }

    if let Some(ref boxes) = box_list {
        for b in boxes {
            let px = b.to_pixels(live_rect);
            let r = RECT {
                left: (px.x - live_rect.left as f32) as i32,
                top: (px.y - live_rect.top as f32) as i32,
                right: (px.x + px.w - live_rect.left as f32) as i32,
                bottom: (px.y + px.h - live_rect.top as f32) as i32,
            };

            match style {
                RedactionStyle::Solid => {
                    paint_style_solid(hdc, &r, color);
                }
                RedactionStyle::AnimatedNoise => {
                    paint_style_noise(hdc, &r);
                }
                RedactionStyle::CustomImage => {
                    paint_style_custom_image(hdc, &r, color, &custom_image_path, &image_fill_mode);
                }
            }
        }
    }

    unsafe {
        let _ = BitBlt(hdc_screen, 0, 0, cw, ch, hdc, 0, 0, SRCCOPY);
        let _ = SelectObject(hdc, old_bm);
        let _ = DeleteObject(hbm);
        let _ = DeleteDC(hdc);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn paint_style_solid(hdc: windows::Win32::Graphics::Gdi::HDC, r: &RECT, color: [u8; 4]) {
    let cr = COLORREF(
        ((color[2] as u32) << 16) | ((color[1] as u32) << 8) | (color[0] as u32),
    );
    let safe = if cr == COLORKEY { COLORREF(cr.0 ^ 1) } else { cr };
    let br = unsafe { CreateSolidBrush(safe) };
    unsafe {
        let _ = FillRect(hdc, r, br);
        let _ = DeleteObject(br);
    }
}

fn paint_style_noise(hdc: windows::Win32::Graphics::Gdi::HDC, r: &RECT) {
    let bw = (r.right - r.left) as usize;
    let bh = (r.bottom - r.top) as usize;
    if bw == 0 || bh == 0 { return; }

    let block_size: usize = 6;
    let mut pixels = vec![0u8; bw * bh * 4];

    let tick = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32;
    let mut rng = tick
        .wrapping_mul(2654435761)
        .wrapping_add(r.left as u32)
        .wrapping_add((r.top as u32) << 16);
    if rng == 0 { rng = 1; }

    let cols_count = (bw + block_size - 1) / block_size;
    let rows_count = (bh + block_size - 1) / block_size;
    let mut block_colors: Vec<u8> = Vec::with_capacity(cols_count * rows_count);
    for _ in 0..(cols_count * rows_count) {
        let v = xorshift32(&mut rng);
        block_colors.push((v & 0xFF) as u8);
    }

    for row in 0..bh {
        let dib_row = bh - 1 - row;
        let by = row / block_size;
        for col in 0..bw {
            let bx = col / block_size;
            let gray = block_colors[by * cols_count + bx];
            let idx = (dib_row * bw + col) * 4;
            pixels[idx] = gray;
            pixels[idx + 1] = gray;
            pixels[idx + 2] = gray;
            pixels[idx + 3] = 0;
        }
    }

    blit_pixels_to_hdc(hdc, r.left, r.top, bw as i32, bh as i32, &pixels);
}

fn paint_style_custom_image(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    r: &RECT,
    color: [u8; 4],
    custom_image_path: &Option<String>,
    fill_mode: &ImageFillMode,
) {
    let bw = (r.right - r.left) as usize;
    let bh = (r.bottom - r.top) as usize;
    if bw == 0 || bh == 0 { return; }

    let Some(path) = custom_image_path.as_deref() else {
        paint_style_solid(hdc, r, color);
        return;
    };

    let cache_lock = CACHED_IMAGE.get_or_init(|| Mutex::new(None));
    let mut cache = cache_lock.lock().unwrap_or_else(|e| e.into_inner());

    let need_load = match &*cache {
        Some(c) => c.path != path,
        None => true,
    };

    if need_load {
        match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let iw = rgba.width() as i32;
                let ih = rgba.height() as i32;
                let mut bgr_pixels = vec![0u8; (iw * ih * 4) as usize];
                for y in 0..ih {
                    let src_row = y as usize;
                    let dst_row = (ih - 1 - y) as usize;
                    for x in 0..iw as usize {
                        let src_idx = (src_row * iw as usize + x) * 4;
                        let dst_idx = (dst_row * iw as usize + x) * 4;
                        let src = rgba.as_raw();
                        bgr_pixels[dst_idx] = src[src_idx + 2];
                        bgr_pixels[dst_idx + 1] = src[src_idx + 1];
                        bgr_pixels[dst_idx + 2] = src[src_idx];
                        bgr_pixels[dst_idx + 3] = src[src_idx + 3];
                    }
                }
                *cache = Some(CachedImage {
                    path: path.to_string(),
                    width: iw,
                    height: ih,
                    bgr_pixels,
                });
            }
            Err(_) => {
                *cache = None;
                drop(cache);
                paint_style_solid(hdc, r, color);
                return;
            }
        }
    }

    let Some(img) = &*cache else {
        drop(cache);
        paint_style_solid(hdc, r, color);
        return;
    };

    let iw = img.width as usize;
    let ih = img.height as usize;
    if iw == 0 || ih == 0 {
        drop(cache);
        paint_style_solid(hdc, r, color);
        return;
    }

    let mut pixels = vec![0u8; bw * bh * 4];

    let bg_b = color[2];
    let bg_g = color[1];
    let bg_r = color[0];
    for i in 0..(bw * bh) {
        let idx = i * 4;
        pixels[idx] = bg_b;
        pixels[idx + 1] = bg_g;
        pixels[idx + 2] = bg_r;
        pixels[idx + 3] = 0;
    }

    match fill_mode {
        ImageFillMode::Tile => {
            for row in 0..bh {
                let dib_row = bh - 1 - row;
                let src_y = row % ih;
                let src_dib_y = ih - 1 - src_y;
                for col in 0..bw {
                    let src_x = col % iw;
                    let src_idx = (src_dib_y * iw + src_x) * 4;
                    let dst_idx = (dib_row * bw + col) * 4;
                    copy_pixel_safe(&img.bgr_pixels, src_idx, &mut pixels, dst_idx);
                }
            }
        }
        ImageFillMode::Stretch => {
            for row in 0..bh {
                let dib_row = bh - 1 - row;
                let src_y = (row * ih) / bh;
                let src_dib_y = ih - 1 - src_y;
                for col in 0..bw {
                    let src_x = (col * iw) / bw;
                    let src_idx = (src_dib_y * iw + src_x) * 4;
                    let dst_idx = (dib_row * bw + col) * 4;
                    copy_pixel_safe(&img.bgr_pixels, src_idx, &mut pixels, dst_idx);
                }
            }
        }
        ImageFillMode::Center => {
            let offset_x = if bw > iw { (bw - iw) / 2 } else { 0 };
            let offset_y = if bh > ih { (bh - ih) / 2 } else { 0 };
            let src_offset_x = if iw > bw { (iw - bw) / 2 } else { 0 };
            let src_offset_y = if ih > bh { (ih - bh) / 2 } else { 0 };

            let draw_w = bw.min(iw);
            let draw_h = bh.min(ih);

            for row in 0..draw_h {
                let dst_row_y = row + offset_y;
                if dst_row_y >= bh { break; }
                let dib_row = bh - 1 - dst_row_y;
                let src_y = row + src_offset_y;
                let src_dib_y = ih - 1 - src_y;

                for col in 0..draw_w {
                    let dst_col_x = col + offset_x;
                    if dst_col_x >= bw { break; }
                    let src_x = col + src_offset_x;
                    let src_idx = (src_dib_y * iw + src_x) * 4;
                    let dst_idx = (dib_row * bw + dst_col_x) * 4;
                    copy_pixel_safe(&img.bgr_pixels, src_idx, &mut pixels, dst_idx);
                }
            }
        }
    }

    drop(cache);
    blit_pixels_to_hdc(hdc, r.left, r.top, bw as i32, bh as i32, &pixels);
}

fn copy_pixel_safe(src: &[u8], src_idx: usize, dst: &mut [u8], dst_idx: usize) {
    if src_idx + 3 >= src.len() || dst_idx + 3 >= dst.len() { return; }
    let sa = src[src_idx + 3] as u32;

    if sa == 0 {
        dst[dst_idx] = (COLORKEY.0 & 0xFF) as u8;
        dst[dst_idx + 1] = ((COLORKEY.0 >> 8) & 0xFF) as u8;
        dst[dst_idx + 2] = ((COLORKEY.0 >> 16) & 0xFF) as u8;
        dst[dst_idx + 3] = 0;
        return;
    }

    let sb = src[src_idx] as u32;
    let sg = src[src_idx + 1] as u32;
    let sr = src[src_idx + 2] as u32;

    if sa == 255 {
        let pixel_val = ((sr as u32) << 16) | ((sg as u32) << 8) | sb;
        let out_b = if pixel_val == COLORKEY.0 { (sb as u8).wrapping_add(1) } else { sb as u8 };
        dst[dst_idx] = out_b;
        dst[dst_idx + 1] = sg as u8;
        dst[dst_idx + 2] = sr as u8;
        dst[dst_idx + 3] = 0;
        return;
    }

    let db = dst[dst_idx] as u32;
    let dg = dst[dst_idx + 1] as u32;
    let dr = dst[dst_idx + 2] as u32;

    let inv_a = 255 - sa;
    let out_b = ((sb * sa + db * inv_a) / 255) as u8;
    let out_g = ((sg * sa + dg * inv_a) / 255) as u8;
    let out_r = ((sr * sa + dr * inv_a) / 255) as u8;

    let pixel_val = ((out_r as u32) << 16) | ((out_g as u32) << 8) | (out_b as u32);
    dst[dst_idx] = if pixel_val == COLORKEY.0 { out_b.wrapping_add(1) } else { out_b };
    dst[dst_idx + 1] = out_g;
    dst[dst_idx + 2] = out_r;
    dst[dst_idx + 3] = 0;
}

fn blit_pixels_to_hdc(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    pixels: &[u8],
) {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default()],
    };

    unsafe {
        SetDIBitsToDevice(
            hdc,
            x, y,
            w as u32,
            h as u32,
            0, 0,
            0,
            h as u32,
            pixels.as_ptr() as *const _,
            &bmi,
            DIB_RGB_COLORS,
        );
    }
}

fn paint_selection(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc_screen = unsafe { BeginPaint(hwnd, &mut ps) };
    if hdc_screen.0 == 0 {
        return;
    }

    let (mode, hovered_win, pending, selected_box, target_hwnd, target_win) = {
        let app = match APP_STATE.get().and_then(|a| a.lock().ok()) {
            Some(a) => a,
            None => {
                unsafe {
                    let _ = EndPaint(hwnd, &ps);
                }
                return;
            }
        };
        let hw = app
            .hovered_window
            .and_then(|h| app.tracked_windows.get(&h).cloned());
        let (th, tw) = match &app.mode {
            AppMode::DrawTarget(h) => (
                Some(*h),
                app.tracked_windows.get(h).cloned(),
            ),
            _ => (None, None),
        };
        (
            app.mode.clone(),
            hw,
            app.pending_new_box_drag,
            app.selected_box,
            th,
            tw,
        )
    };

    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let mut overlay_rect = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut overlay_rect);
    }

    let cw = client.right - client.left;
    let ch = client.bottom - client.top;

    let hdc = unsafe { CreateCompatibleDC(hdc_screen) };
    let hbm = unsafe { CreateCompatibleBitmap(hdc_screen, cw, ch) };
    let old_bm = unsafe { SelectObject(hdc, hbm) };

    match mode {
        AppMode::WindowSelect => {
            let dark = unsafe { CreateSolidBrush(COLORREF(0x00404040)) };
            unsafe {
                let _ = FillRect(hdc, &client, dark);
                let _ = DeleteObject(dark);
            }

            if let Some(w) = hovered_win {
                let hole = RECT {
                    left: (w.rect.left - overlay_rect.left).max(0),
                    top: (w.rect.top - overlay_rect.top).max(0),
                    right: (w.rect.right - overlay_rect.left).min(client.right),
                    bottom: (w.rect.bottom - overlay_rect.top).min(client.bottom),
                };
                if hole.right > hole.left && hole.bottom > hole.top {
                    let hb = unsafe { CreateSolidBrush(COLORREF(0x00000000)) };
                    unsafe {
                        let _ = FillRect(hdc, &hole, hb);
                        let _ = DeleteObject(hb);
                    }

                    let pen = unsafe { CreatePen(PEN_STYLE(0), 3, COLORREF(0x0044AAFF)) };
                    unsafe {
                        let op = SelectObject(hdc, pen);
                        let nb = GetStockObject(NULL_BRUSH);
                        let ob = SelectObject(hdc, nb);
                        let _ = windows::Win32::Graphics::Gdi::Rectangle(
                            hdc, hole.left, hole.top, hole.right, hole.bottom,
                        );
                        let _ = SelectObject(hdc, ob);
                        let _ = SelectObject(hdc, op);
                        let _ = DeleteObject(pen);
                    }

                    let txt = to_wide(
                        "Click and drag to draw \u{00b7} Click existing box to edit",
                    );
                    unsafe {
                        let _ = SetBkMode(hdc, TRANSPARENT);
                        let _ = SetTextColor(hdc, COLORREF(0x00FFFFFF));
                        let ty = if hole.top >= 24 {
                            hole.top - 24
                        } else {
                            hole.bottom + 4
                        };
                        let _ = TextOutW(hdc, hole.left + 8, ty, &txt[..txt.len() - 1]);
                    }
                }
            }
        }

        AppMode::DrawTarget(_draw_target_hwnd) => {
            let bg = unsafe { CreateSolidBrush(COLORREF(0x00000000)) };
            unsafe {
                let _ = FillRect(hdc, &client, bg);
                let _ = DeleteObject(bg);
            }

            if let (Some(th), Some(tw)) = (target_hwnd, &target_win) {
                let box_list = {
                    let app = APP_STATE.get().and_then(|a| a.lock().ok());
                    app.and_then(|a| a.boxes.get(&th).cloned())
                };
                if let Some(box_list) = box_list {
                    for (i, b) in box_list.iter().enumerate() {
                        let px = b.to_pixels(tw.rect);
                        let bx = (px.x as i32) - overlay_rect.left;
                        let by = (px.y as i32) - overlay_rect.top;
                        let bx2 = (px.x + px.w) as i32 - overlay_rect.left;
                        let by2 = (px.y + px.h) as i32 - overlay_rect.top;

                        let is_selected = selected_box == Some(i);
                        let pen_color = if is_selected {
                            COLORREF(0x0000FFFF)
                        } else {
                            COLORREF(0x00808080)
                        };
                        let pen = unsafe {
                            CreatePen(
                                PEN_STYLE(0),
                                if is_selected { 3 } else { 1 },
                                pen_color,
                            )
                        };
                        unsafe {
                            let op = SelectObject(hdc, pen);
                            let nb = GetStockObject(NULL_BRUSH);
                            let ob = SelectObject(hdc, nb);
                            let _ = windows::Win32::Graphics::Gdi::Rectangle(
                                hdc, bx, by, bx2, by2,
                            );
                            let _ = SelectObject(hdc, ob);
                            let _ = SelectObject(hdc, op);
                            let _ = DeleteObject(pen);
                        }

                        if is_selected {
                            let btn = BTN_SIZE as i32;
                            let half = btn / 2;
                            let cx = (bx + bx2) / 2;
                            let cy = (by + by2) / 2;

                            let active_color = COLORREF(0x0000FF00);
                            let inactive_color = COLORREF(0x00AAAAAA);

                            let corners: [(i32, i32, Anchor); 4] = [
                                (bx, by, Anchor::TopLeft),
                                (bx2, by, Anchor::TopRight),
                                (bx, by2, Anchor::BottomLeft),
                                (bx2, by2, Anchor::BottomRight),
                            ];
                            for (cx_c, cy_c, anchor) in &corners {
                                let is_active = b.anchor == *anchor;
                                let clr =
                                    if is_active { active_color } else { inactive_color };
                                let br = unsafe { CreateSolidBrush(clr) };
                                let ar = RECT {
                                    left: cx_c - half,
                                    top: cy_c - half,
                                    right: cx_c + half,
                                    bottom: cy_c + half,
                                };
                                unsafe {
                                    let _ = FillRect(hdc, &ar, br);
                                    let _ = DeleteObject(br);
                                }

                                let label = to_wide("A");
                                unsafe {
                                    let _ = SetBkMode(hdc, TRANSPARENT);
                                    let _ = SetTextColor(hdc, COLORREF(0x00000000));
                                    let _ = TextOutW(
                                        hdc,
                                        cx_c - 4,
                                        cy_c - 7,
                                        &label[..label.len() - 1],
                                    );
                                }
                            }

                            let del_br = unsafe { CreateSolidBrush(COLORREF(0x000000DD)) };
                            let del_r = RECT {
                                left: cx - half,
                                top: cy - half,
                                right: cx + half,
                                bottom: cy + half,
                            };
                            unsafe {
                                let _ = FillRect(hdc, &del_r, del_br);
                                let _ = DeleteObject(del_br);
                                let _ = SetBkMode(hdc, TRANSPARENT);
                                let _ = SetTextColor(hdc, COLORREF(0x00FFFFFF));
                                let x_label = to_wide("X");
                                let _ = TextOutW(
                                    hdc,
                                    cx - 4,
                                    cy - 7,
                                    &x_label[..x_label.len() - 1],
                                );
                            }
                        }
                    }
                }
            }

            if let Some((_drag_hwnd, start)) = pending {
                let mut cur = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut cur);
                }
                let left = (start.0.min(cur.x as f32) as i32) - overlay_rect.left;
                let top = (start.1.min(cur.y as f32) as i32) - overlay_rect.top;
                let right = (start.0.max(cur.x as f32) as i32) - overlay_rect.left;
                let bottom = (start.1.max(cur.y as f32) as i32) - overlay_rect.top;

                let fill = unsafe { CreateSolidBrush(COLORREF(0x00804020)) };
                let fr = RECT {
                    left,
                    top,
                    right,
                    bottom,
                };
                unsafe {
                    let _ = FillRect(hdc, &fr, fill);
                    let _ = DeleteObject(fill);
                }

                let pen = unsafe { CreatePen(PS_DASH, 2, COLORREF(0x000000FF)) };
                unsafe {
                    let op = SelectObject(hdc, pen);
                    let nb = GetStockObject(NULL_BRUSH);
                    let ob = SelectObject(hdc, nb);
                    let _ = windows::Win32::Graphics::Gdi::Rectangle(
                        hdc, left, top, right, bottom,
                    );
                    let _ = SelectObject(hdc, ob);
                    let _ = SelectObject(hdc, op);
                    let _ = DeleteObject(pen);
                }
            }

            unsafe {
                let _ = SetBkMode(hdc, TRANSPARENT);
                let _ = SetTextColor(hdc, COLORREF(0x00FFFFFF));
                let txt = to_wide(
                    "Drag to draw \u{00b7} Click box to edit \u{00b7} Escape to exit",
                );
                let _ = TextOutW(hdc, 10, 10, &txt[..txt.len() - 1]);
            }
        }

        AppMode::Idle => {
            let clear = unsafe { CreateSolidBrush(COLORREF(0x00000000)) };
            unsafe {
                let _ = FillRect(hdc, &client, clear);
                let _ = DeleteObject(clear);
            }
        }
    }

    unsafe {
        let _ = BitBlt(hdc_screen, 0, 0, cw, ch, hdc, 0, 0, SRCCOPY);
        let _ = SelectObject(hdc, old_bm);
        let _ = DeleteObject(hbm);
        let _ = DeleteDC(hdc);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn create_redaction_overlay(
    target: WindowHandle,
    rect: crate::tracker::RectPx,
) -> Option<WindowHandle> {
    let title = to_wide(&format!("BlindSpot.Redaction.{}", target.0));
    let class = to_wide(REDACTION_CLASS);
    let h = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(
                WS_EX_LAYERED.0
                    | WS_EX_TRANSPARENT.0
                    | WS_EX_TOOLWINDOW.0
                    | WS_EX_NOACTIVATE.0,
            ),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(WS_POPUP.0),
            rect.left,
            rect.top,
            rect.width(),
            rect.height(),
            HWND(0),
            HMENU(0),
            HINSTANCE(0),
            None,
        )
    };
    if h.0 == 0 {
        return None;
    }
    unsafe {
        let _ = SetLayeredWindowAttributes(h, COLORKEY, 0, LWA_COLORKEY);
        let _ = ShowWindow(h, SW_SHOWNA);
    }
    Some(WindowHandle(h.0))
}

fn create_selection_overlay(index: usize, monitor: MonitorInfo) -> Option<WindowHandle> {
    let title = to_wide(&format!("BlindSpot.Selection.{}", index));
    let class = to_wide(SELECTION_CLASS);
    let h = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(
                WS_EX_LAYERED.0
                    | WS_EX_TOPMOST.0
                    | WS_EX_TOOLWINDOW.0
                    | WS_EX_NOACTIVATE.0,
            ),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(WS_POPUP.0),
            monitor.rect.left,
            monitor.rect.top,
            monitor.rect.width(),
            monitor.rect.height(),
            HWND(0),
            HMENU(0),
            HINSTANCE(0),
            None,
        )
    };
    if h.0 == 0 {
        return None;
    }
    unsafe {
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), 160, LWA_ALPHA);
        let _ = ShowWindow(h, SW_SHOWNA);
    }
    Some(WindowHandle(h.0))
}

fn register_overlay_classes() {
    let r = to_wide(REDACTION_CLASS);
    let s = to_wide(SELECTION_CLASS);
    unsafe {
        let _ = RegisterClassExW(&WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(redaction_wndproc),
            hInstance: HINSTANCE(0),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(r.as_ptr()),
            ..Default::default()
        });
        let _ = RegisterClassExW(&WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(selection_wndproc),
            hInstance: HINSTANCE(0),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(s.as_ptr()),
            ..Default::default()
        });
    }
}

fn pump_messages() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, HWND(0), 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }
}

fn with_shared<F: FnOnce(&mut OverlayShared)>(f: F) {
    if let Some(s) = OVERLAY_SHARED.get() {
        if let Ok(mut g) = s.lock() {
            f(&mut g);
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}