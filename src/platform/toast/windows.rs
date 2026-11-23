//! Windows notifications: an icon in the notification area owned by one
//! hidden message-only window.

use super::{fill, Toasts, Tray, IDLE, SHUTDOWN_WAIT};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, DispatchMessageW, LoadIconW, PeekMessageW, TranslateMessage, HWND_MESSAGE, IDI_INFORMATION, MSG, PM_REMOVE};

/// The tooltip of the app's icon in the notification area.
const TIP: &str = "Ultimate PC Manager";

/// The app's notification thread.
static TOASTS: Toasts = Toasts::new(IDLE);

pub fn show(title: &str, body: &str) -> Result<(), String> {
    TOASTS.show(title, body, Icon::new)
}

pub fn shutdown() {
    TOASTS.shutdown(SHUTDOWN_WAIT)
}

/// One hidden message-only window owns the icon.
struct Icon {
    hwnd: HWND,
    data: NOTIFYICONDATAW,
}

impl Icon {
    /// The window and the icon's data, on the notification thread.
    fn new() -> Option<Icon> {
        // SAFETY: "STATIC" is a system window class; a message-only
        // window (parent HWND_MESSAGE) has no visible part. Every
        // pointer passed is a NUL-terminated string that outlives the
        // call, or null.
        let hwnd = unsafe {
            let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
            CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null())
        };
        if hwnd.is_null() {
            return None;
        }
        // SAFETY: NOTIFYICONDATAW is plain data, valid zeroed; cbSize
        // is set. The app's icon is resource 1 (winresource); the
        // system's information icon when it is missing.
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.hIcon = unsafe {
            let own = LoadIconW(GetModuleHandleW(std::ptr::null()), 1 as _);
            if own.is_null() {
                LoadIconW(std::ptr::null_mut(), IDI_INFORMATION)
            } else {
                own
            }
        };
        fill(&mut data.szTip, TIP);
        Some(Icon { hwnd, data })
    }
}

impl Tray for Icon {
    fn add(&mut self) -> bool {
        self.data.uFlags = NIF_ICON | NIF_TIP;
        // SAFETY: `data` is a valid, initialised NOTIFYICONDATAW.
        unsafe { Shell_NotifyIconW(NIM_ADD, &self.data) != 0 }
    }

    fn notify(&mut self, title: &str, body: &str) {
        self.data.uFlags = NIF_ICON | NIF_TIP | NIF_INFO;
        self.data.dwInfoFlags = NIIF_INFO;
        fill(&mut self.data.szInfoTitle, title);
        fill(&mut self.data.szInfo, body);
        // SAFETY: as above.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.data) };
    }

    fn remove(&mut self) {
        // SAFETY: removes the icon this thread added.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &self.data) };
    }

    fn pump(&mut self) {
        // SAFETY: MSG is plain data; PeekMessageW fills it for this
        // thread's windows only.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: destroys this thread's own window (on the thread that
        // created it: the icon never leaves the notification thread).
        unsafe { DestroyWindow(self.hwnd) };
    }
}
