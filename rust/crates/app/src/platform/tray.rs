//! The notification-area (tray) icon: closing the window hides it there, and the icon brings it back (a
//! click) or quits (its menu). The icon's messages go to a hidden window of ours, dispatched by winit's own
//! message loop; what they ask for is picked up with [`Tray::take_events`].

use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Bring the window back.
    Show,
    Quit,
}

static EVENTS: Mutex<Vec<TrayEvent>> = Mutex::new(Vec::new());
/// The message the icon sends its window.
const CALLBACK: u32 = WM_APP + 1;
/// Sent by [`show_window`], from any thread.
const SHOW: u32 = WM_APP + 2;
/// The tray's window, for [`show_window`] (0 without one).
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// The unread mentions shown on the icon, and the icon drawn for them (0 for the exe's own).
static MENTIONS: AtomicU32 = AtomicU32::new(0);
static ICON: AtomicIsize = AtomicIsize::new(0);
const CLASS: PCWSTR = w!("SwiftcordTrayWindow");
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;
/// Enter on the icon (`NIN_SELECT | NINF_KEY`, which the bindings don't name).
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

pub struct Tray {
    hwnd: HWND,
}

impl Tray {
    /// Adds the icon; `None` if the shell wouldn't have it.
    pub fn new() -> Option<Tray> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).ok()?.into();
            let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance, lpszClassName: CLASS, ..Default::default() };
            RegisterClassW(&wc);
            // A plain hidden window rather than a message-only one: the menu needs one that can come to the front.
            // Titled for this exe's instance, so starting it again finds this one (see `single_instance`).
            let title = HSTRING::from(super::single_instance::key());
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), CLASS, &title, WS_POPUP, 0, 0, 0, 0, None, None, Some(instance), None).ok()?;
            WINDOW.store(hwnd.0 as isize, Ordering::Relaxed);
            add_icon(hwnd).then_some(Tray { hwnd })
        }
    }

    /// What the icon has been asked for since last time.
    pub fn take_events() -> Vec<TrayEvent> {
        std::mem::take(&mut *EVENTS.lock().unwrap())
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        WINDOW.store(0, Ordering::Relaxed);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(self.hwnd));
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

/// Puts the icon in the notification area (again, after Explorer restarts).
fn add_icon(hwnd: HWND) -> bool {
    unsafe {
        let mut data = icon_data(hwnd);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = CALLBACK;
        set_icon_and_tip(&mut data);
        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            return false;
        }
        // Version 4: clicks arrive as NIN_SELECT, the menu key and right-clicks as WM_CONTEXTMENU.
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        true
    }
}

/// The icon (with a red dot for unread mentions) and its tooltip (with their count).
fn set_icon_and_tip(data: &mut NOTIFYICONDATAW) {
    let mentions = MENTIONS.load(Ordering::Relaxed);
    let drawn = super::badge::tray_icon(mentions);
    let icon = drawn.or_else(|| unsafe {
        // The exe's icon (resource 1), at the size the notification area uses.
        let instance: Option<HINSTANCE> = GetModuleHandleW(None).ok().map(Into::into);
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        LoadImageW(instance, PCWSTR(1 as *const u16), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR).ok().map(|i| HICON(i.0))
    });
    if let Some(icon) = icon {
        data.hIcon = icon;
        // The shell copies it; the one shown before can go.
        let old = ICON.swap(icon.0 as isize, Ordering::Relaxed);
        if old != 0 {
            let _ = unsafe { DestroyIcon(HICON(old as _)) };
        }
    }
    let tip = match mentions {
        0 => super::APP_NAME.to_owned(),
        1 => format!("{} - 1 unread mention", super::APP_NAME),
        n => format!("{} - {n} unread mentions", super::APP_NAME),
    };
    data.szTip = [0; 128];
    for (slot, c) in data.szTip.iter_mut().take(127).zip(tip.encode_utf16()) {
        *slot = c;
    }
}

/// Shows `mentions` on the icon: a red dot, and the count in its tooltip.
pub fn set_mentions(mentions: u32) {
    let hwnd = WINDOW.load(Ordering::Relaxed);
    if hwnd == 0 || MENTIONS.swap(mentions, Ordering::Relaxed) == mentions && ICON.load(Ordering::Relaxed) != 0 {
        return;
    }
    let mut data = icon_data(HWND(hwnd as _));
    data.uFlags = NIF_ICON | NIF_TIP;
    set_icon_and_tip(&mut data);
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
}

/// Asks the instance of this exe that's already running to bring its window back (see `single_instance`),
/// letting it come to the front: this process, just started by the user, may.
pub fn show_running() {
    unsafe {
        let title = HSTRING::from(super::single_instance::key());
        let Ok(hwnd) = FindWindowW(CLASS, &title) else { return };
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let _ = AllowSetForegroundWindow(pid);
        let _ = PostMessageW(Some(hwnd), SHOW, WPARAM(0), LPARAM(0));
    }
}

/// Explorer's broadcast when the taskbar is (re)created, when icons have to be added again.
fn taskbar_created() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// Brings the window back (from the tray, minimized or behind others), as clicking the icon does. From any
/// thread: it goes through the tray window's messages, which wakes the event loop.
pub fn show_window() {
    let hwnd = WINDOW.load(Ordering::Relaxed);
    if hwnd != 0 {
        let _ = unsafe { PostMessageW(Some(HWND(hwnd as _)), SHOW, WPARAM(0), LPARAM(0)) };
    }
}

fn push(event: TrayEvent) {
    EVENTS.lock().unwrap().push(event);
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == CALLBACK {
        match lparam.0 as u32 & 0xFFFF {
            NIN_SELECT | NIN_KEYSELECT => push(TrayEvent::Show),
            WM_CONTEXTMENU => menu(hwnd),
            _ => {}
        }
        return LRESULT(0);
    }
    if msg == SHOW {
        push(TrayEvent::Show);
        return LRESULT(0);
    }
    if msg == taskbar_created() {
        add_icon(hwnd);
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The icon's menu, at the pointer.
fn menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("Open '08cord"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit '08cord"));
        let _ = SetMenuDefaultItem(menu, MENU_OPEN as u32, 0);
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // Without coming to the front, the menu wouldn't close when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, at.x, at.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match chosen.0 as usize {
            MENU_OPEN => push(TrayEvent::Show),
            MENU_QUIT => push(TrayEvent::Quit),
            _ => {}
        }
    }
}
