//! The speed bar: a small window that shows the speed in readable text — on
//! Windows on the taskbar, just left of the notification area; on macOS and
//! Linux under the menu bar, top right. It can be dragged anywhere, and is
//! remembered there. On Windows it is put back on top each tick (the taskbar
//! is a topmost window too) and hidden while a full-screen app is in front.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder, WindowEvent};

pub const LABEL: &str = "speedbar";

/// Its size in logical pixels: two lines of "↓ 999.9 MB/s".
const WIDTH: f64 = 116.0;
const HEIGHT: f64 = 36.0;

/// GitSwitch moving it (not the person): not remembered as their choice.
static PLACED_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// Hidden by GitSwitch while a full-screen app is in front.
static HIDDEN_FOR_FULL_SCREEN: Mutex<bool> = Mutex::new(false);

/// Show or close the bar; while shown, keep it where it belongs.
pub fn sync(app: &AppHandle, want: bool, saved: Option<[i32; 2]>) {
    match (want, app.get_webview_window(LABEL)) {
        (true, None) => {
            if let Err(e) = create(app, saved) {
                eprintln!("GitSwitch: the speed bar could not be opened: {}", e);
            }
        }
        (false, Some(w)) => {
            let _ = w.close();
        }
        (true, Some(w)) => keep(&w),
        (false, None) => {}
    }
}

fn create(app: &AppHandle, saved: Option<[i32; 2]>) -> tauri::Result<()> {
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("GitSwitch — net speed")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focusable(false)
        .shadow(false)
        .visible(false);
    // Clear on the Windows taskbar; a solid strip elsewhere (macOS would need its private API).
    #[cfg(windows)]
    let builder = builder.transparent(true);
    #[cfg(target_os = "macos")]
    let builder = builder.visible_on_all_workspaces(true);
    let win = builder.build()?;
    let pos = saved.filter(|p| on_a_screen(app, *p)).or_else(|| default_position(app, &win));
    if let Some([x, y]) = pos {
        *PLACED_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }
    let _ = win.show();
    let app2 = app.clone();
    win.on_window_event(move |e| {
        if let WindowEvent::Moved(p) = e {
            let ours = PLACED_AT.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed() < Duration::from_millis(800));
            if !ours {
                super::bar_moved(&app2, p.x, p.y);
            }
        }
    });
    Ok(())
}

/// Whether a saved position is on one of the screens there are now.
fn on_a_screen(app: &AppHandle, [x, y]: [i32; 2]) -> bool {
    app.available_monitors().map(|ms| ms.iter().any(|m| {
        let (p, s) = (m.position(), m.size());
        x >= p.x && y >= p.y && x < p.x + s.width as i32 && y < p.y + s.height as i32
    })).unwrap_or(false)
}

fn default_position(app: &AppHandle, win: &tauri::WebviewWindow) -> Option<[i32; 2]> {
    let size = win.outer_size().ok()?;
    #[cfg(windows)]
    if let Some(p) = sys::beside_the_clock(size.width as i32, size.height as i32) {
        return Some(p);
    }
    let m = app.primary_monitor().ok()??;
    let area = m.work_area();
    let (w, h) = (size.width as i32, size.height as i32);
    if cfg!(windows) {
        // Bottom right, just above the taskbar.
        Some([area.position.x + area.size.width as i32 - w - 12, area.position.y + area.size.height as i32 - h - 8])
    } else {
        // Top right, under the menu bar.
        Some([area.position.x + area.size.width as i32 - w - 16, area.position.y + 8])
    }
}

fn keep(win: &tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        let Ok(hwnd) = win.hwnd() else { return };
        let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
        let full = sys::full_screen_in_front(hwnd);
        let mut hidden = HIDDEN_FOR_FULL_SCREEN.lock().unwrap_or_else(|e| e.into_inner());
        if full && !*hidden {
            let _ = win.hide();
            *hidden = true;
        } else if !full {
            if *hidden {
                let _ = win.show();
                *hidden = false;
            }
            sys::to_top(hwnd);
        }
    }
    #[cfg(not(windows))]
    let _ = win;
}

#[cfg(windows)]
mod sys {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowRect, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn rect(h: HWND) -> Option<RECT> {
        // SAFETY: a valid out-pointer.
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            (GetWindowRect(h, &mut r) != 0).then_some(r)
        }
    }

    /// On the taskbar, just left of the notification area, when the taskbar
    /// is at the bottom of the screen.
    pub fn beside_the_clock(w: i32, h: i32) -> Option<[i32; 2]> {
        // SAFETY: class names are valid wide strings; handles are only read.
        unsafe {
            let bar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null());
            if bar.is_null() {
                return None;
            }
            let b = rect(bar)?;
            // A taskbar at the bottom is wider than tall.
            if b.right - b.left < (b.bottom - b.top) * 2 {
                return None;
            }
            let tray = FindWindowExW(bar, std::ptr::null_mut(), wide("TrayNotifyWnd").as_ptr(), std::ptr::null());
            let right = if tray.is_null() { b.right - 220 } else { rect(tray)?.left };
            Some([right - w - 6, b.top + ((b.bottom - b.top) - h) / 2])
        }
    }

    /// Back on top of the taskbar (which is topmost too), without taking focus.
    pub fn to_top(h: HWND) {
        // SAFETY: the bar's own window handle.
        unsafe {
            SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// A full-screen window (a video, a game) in front, on the bar's screen.
    pub fn full_screen_in_front(bar: HWND) -> bool {
        // SAFETY: handles are only read; buffers are sized as passed.
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_null() || fg == bar {
                return false;
            }
            let mut class = [0u16; 64];
            let n = GetClassNameW(fg, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            // The desktop and the taskbar are not apps.
            if ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&class.as_str()) {
                return false;
            }
            let Some(r) = rect(fg) else { return false };
            let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut info) == 0 {
                return false;
            }
            let m = info.rcMonitor;
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
        }
    }
}
