//! Crew's icon in the notification area, and one copy at a time. Closing the window keeps Crew and its
//! running sessions going behind this icon: a click brings the window back, the right-click menu
//! opens or quits. Launching Crew again brings the running window back too.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use crew_core::Paths;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::Window;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, GetSystemMetrics,
    HICON, IMAGE_ICON, IsIconic, LR_SHARED, LoadImageW, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, SW_HIDE, SW_RESTORE, SW_SHOW,
    SetForegroundWindow, ShowWindow, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};
use windows::core::{PCWSTR, w};
use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub enum TrayEvent {
    Open,
    Quit,
}

const WM_TRAY: u32 = WM_APP + 1;
/// Another launch of Crew asks this one to show itself.
const WM_SHOW_CREW: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
const OPEN: usize = 1;
const QUIT: usize = 2;

static HANDLE: AtomicIsize = AtomicIsize::new(0);
static SHOWN: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static EVENTS: OnceLock<UnboundedSender<TrayEvent>> = OnceLock::new();

/// The few strings the tray shows, which the window's dictionary doesn't have.
fn tr(text: &'static str) -> &'static str {
    if !crate::i18n::zh() {
        return text;
    }
    match text {
        "Open Crew" => "打开 Crew",
        "Quit Crew" => "退出 Crew",
        "Crew is still running" => "Crew 仍在运行",
        "Your sessions keep running. Click the tray icon to reopen, or right-click it to quit." => "你的会话会继续运行。点击托盘图标重新打开，或右键单击退出。",
        other => other,
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn fill(to: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(to.len() - 1).collect();
    to[..units.len()].copy_from_slice(&units);
    to[units.len()] = 0;
}

/// Each data folder is its own Crew: a test copy doesn't reach for the real one.
fn name_for(paths: &Paths) -> String {
    let mut h = DefaultHasher::new();
    paths.data_dir.to_string_lossy().to_lowercase().hash(&mut h);
    format!("CrewRust-{:016x}", h.finish())
}

/// Whether Crew already runs on this data; if so it's been asked to show itself.
pub fn hand_over_to_running(paths: &Paths) -> bool {
    let name = name_for(paths);
    let lock = wide(&format!("Local\\{name}"));
    // Held until the process ends.
    let Ok(_held) = (unsafe { CreateMutexW(None, false, PCWSTR(lock.as_ptr())) }) else { return false };
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return false;
    }
    let class = wide(&name);
    // The first copy may still be starting up.
    for _ in 0..50 {
        if let Ok(running) = unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) } {
            unsafe {
                // This launch may come to the front: let the running copy do so instead.
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = PostMessageW(Some(running), WM_SHOW_CREW, WPARAM(0), LPARAM(0));
            }
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    true
}

fn tray_window() -> HWND {
    HWND(HANDLE.load(Ordering::Acquire) as *mut _)
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: ICON_ID, ..Default::default() }
}

fn add_icon(hwnd: HWND) -> bool {
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    // The exe's own icon, which build.rs put there as resource 1.
    let icon = unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        LoadImageW(Some(module.into()), PCWSTR(1 as *const u16), IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_SHARED)
    };
    match icon {
        Ok(icon) => data.hIcon = HICON(icon.0),
        Err(why) => eprintln!("tray: couldn't load the icon: {why}"),
    }
    fill(&mut data.szTip, "Crew");
    let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool();
    if !added {
        eprintln!("tray: couldn't add the icon");
    }
    SHOWN.store(added, Ordering::Release);
    added
}

fn send(event: TrayEvent) {
    if let Some(events) = EVENTS.get() {
        let _ = events.unbounded_send(event);
    }
}

fn show_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let (open, quit) = (wide(tr("Open Crew")), wide(tr("Quit Crew")));
        let _ = AppendMenuW(menu, MF_STRING, OPEN, PCWSTR(open.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, QUIT, PCWSTR(quit.as_ptr()));
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // Without this the menu stays open when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, at.x, at.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

unsafe extern "system" fn on_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Explorer restarted: the icons have to be added again.
    if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Acquire) {
        add_icon(hwnd);
        return LRESULT(0);
    }
    match msg {
        WM_TRAY => match lparam.0 as u32 & 0xffff {
            WM_LBUTTONUP => {
                // This click lets Crew come to the front.
                let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
                send(TrayEvent::Open);
            }
            WM_RBUTTONUP => show_menu(hwnd),
            _ => {}
        },
        WM_SHOW_CREW => send(TrayEvent::Open),
        WM_COMMAND => match wparam.0 & 0xffff {
            OPEN => {
                let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
                send(TrayEvent::Open);
            }
            QUIT => send(TrayEvent::Quit),
            _ => {}
        },
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

/// Adds the icon, on a thread of its own so its menu never waits on the window.
pub fn start(paths: &Paths) -> UnboundedReceiver<TrayEvent> {
    let (tx, rx) = unbounded();
    let _ = EVENTS.set(tx);
    let class = wide(&name_for(paths));
    std::thread::Builder::new()
        .name("crew-tray".into())
        .spawn(move || unsafe {
            TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Release);
            let module = GetModuleHandleW(None).unwrap_or_default();
            let info = WNDCLASSW { lpfnWndProc: Some(on_message), hInstance: module.into(), lpszClassName: PCWSTR(class.as_ptr()), ..Default::default() };
            RegisterClassW(&info);
            // A hidden top-level window: only those hear that Explorer restarted.
            let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(class.as_ptr()), w!("Crew"), WINDOW_STYLE(0), 0, 0, 0, 0, None, None, Some(module.into()), None) else { return };
            HANDLE.store(hwnd.0 as isize, Ordering::Release);
            add_icon(hwnd);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .expect("couldn't start the tray");
    rx
}

/// Whether the icon is there to come back through. Without it, hiding the window would lose Crew.
pub fn is_shown() -> bool {
    SHOWN.load(Ordering::Acquire)
}

/// The first time the window goes to the tray: where Crew went.
pub fn explain_once() {
    let mut data = icon_data(tray_window());
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    fill(&mut data.szInfoTitle, tr("Crew is still running"));
    fill(&mut data.szInfo, tr("Your sessions keep running. Click the tray icon to reopen, or right-click it to quit."));
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
}

pub fn remove() {
    if SHOWN.swap(false, Ordering::AcqRel) {
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &icon_data(tray_window())) };
    }
}

fn window_handle(window: &Window) -> Option<HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(own) = handle.as_raw() else { return None };
    Some(HWND(own.hwnd.get() as *mut _))
}

pub fn hide_window(window: &Window) {
    if let Some(hwnd) = window_handle(window) {
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

pub fn show_window(window: &Window) {
    let Some(hwnd) = window_handle(window) else { return };
    unsafe {
        let _ = ShowWindow(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
        let _ = SetForegroundWindow(hwnd);
    }
}
