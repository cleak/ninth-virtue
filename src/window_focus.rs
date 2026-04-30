use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClipCursor, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowRect,
    GetWindowThreadProcessId, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

/// Returns true while `pid` owns the foreground window and the mouse still
/// appears captured by that window.
///
/// DOSBox releases mouse capture before Windows moves focus to a different
/// app. When that happens, `GetClipCursor` expands from the game window back
/// to the virtual desktop, which lets auto-mute react immediately instead of
/// waiting for a later focus change.
pub fn is_pid_foreground_with_captured_cursor(pid: u32) -> bool {
    let Some(hwnd) = foreground_window_for_pid(pid) else {
        return false;
    };

    match is_cursor_clipped_to_window(hwnd) {
        Some(captured) => captured,
        None => is_cursor_inside_window(hwnd).unwrap_or(true),
    }
}

fn foreground_window_for_pid(pid: u32) -> Option<HWND> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut foreground_pid: u32 = 0;
        let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut foreground_pid));
        (thread_id != 0 && foreground_pid == pid).then_some(hwnd)
    }
}

fn is_cursor_clipped_to_window(hwnd: HWND) -> Option<bool> {
    let mut clip = RECT::default();
    let mut window = RECT::default();

    unsafe {
        GetClipCursor(&mut clip).ok()?;
        GetWindowRect(hwnd, &mut window).ok()?;
    }

    let virtual_screen = virtual_screen_rect();
    if same_rect(&clip, &virtual_screen) {
        // Full-screen DOSBox can legitimately cover the virtual desktop.
        // Treat that as active to keep the existing focus behavior there.
        return Some(rect_contains(&window, &virtual_screen));
    }

    Some(rect_contains(&window, &clip))
}

fn is_cursor_inside_window(hwnd: HWND) -> Option<bool> {
    let mut point = POINT::default();
    let mut window = RECT::default();

    unsafe {
        GetCursorPos(&mut point).ok()?;
        GetWindowRect(hwnd, &mut window).ok()?;
    }

    Some(
        point.x >= window.left
            && point.x < window.right
            && point.y >= window.top
            && point.y < window.bottom,
    )
}

fn virtual_screen_rect() -> RECT {
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        RECT {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

fn rect_contains(outer: &RECT, inner: &RECT) -> bool {
    inner.left >= outer.left
        && inner.top >= outer.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

fn same_rect(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}
