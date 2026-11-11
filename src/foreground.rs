//! Foreground window title lookup.
//!
//! The title is optional metadata for the PAD server: it lets the server
//! detect the active application, split episodes on window changes, and drop
//! captures of sensitive windows (password managers, banking, private mode).

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};

/// Maximum number of UTF-16 code units read from the title bar.
const MAX_TITLE_CHARS: usize = 512;

/// Return the current foreground window title, or `None` when unavailable.
///
/// The call is best-effort: a missing window, an empty title, or an OS error
/// all yield `None` rather than failing the capture iteration.
pub fn foreground_window_title() -> Option<String> {
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }

        let mut buffer = [0u16; MAX_TITLE_CHARS];
        let len = GetWindowTextW(hwnd, &mut buffer);
        if len <= 0 {
            return None;
        }

        let title = String::from_utf16_lossy(&buffer[..len as usize]);
        let title = title.trim();
        if title.is_empty() {
            None
        } else {
            Some(title.to_string())
        }
    }
}
