use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Returns true when the OS-level foreground window belongs to `pid`.
///
/// Used by the auto-mute feature to detect when the player has tabbed away
/// from the DOSBox/Ultima V window. We deliberately avoid enumerating the
/// process's windows: Windows guarantees there is at most one foreground
/// window system-wide, so reading it once and comparing the owning PID is
/// both cheap and free of race conditions with window-creation timing.
pub fn is_pid_foreground(pid: u32) -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let mut foreground_pid: u32 = 0;
        let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut foreground_pid));
        thread_id != 0 && foreground_pid == pid
    }
}
