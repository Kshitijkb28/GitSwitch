//! Windows: the speed bar, docked on the taskbar.
//!
//! A small window of GitSwitch's own that the taskbar *owns*. Windows keeps an
//! owned window above its owner, so the taskbar can never cover it: no
//! putting it back on top every tick, no disappearing while apps open. It also
//! goes behind a full-screen app together with the taskbar. An owned window
//! shares its input with the owner's thread, so the bar lives on a thread of
//! its own that never waits on anything: Explorer never waits on GitSwitch.
//!
//! Where the taskbar's buttons are comes from UI Automation, asked on another
//! thread (a client must not ask from a thread whose windows share input with
//! what it asks about). The bar keeps to the free stretches between them
//! ([`super::dock`]), so it never sits on an app's icon or on the tray; when
//! there is room nowhere it lets clicks through to whatever is under it.
//!
//! Drawn with GDI into a per-pixel-alpha layered window: the taskbar shows
//! through and only the text is drawn. Drag it along the taskbar; a
//! double-click opens the Net speed tab.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GetLastError, BOOL, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GdiFlush, GetDC, GetMonitorInfoW, GetTextExtentPoint32W, MonitorFromWindow, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, TextOutW, AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DIB_RGB_COLORS, FF_DONTCARE, FW_NORMAL, HDC, HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_TT_PRECIS, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, VK_LBUTTON, VK_RBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowExW, FindWindowW, GetCursorPos, GetWindow, GetWindowLongW, GetWindowRect, IsWindow,
    GetSystemMetrics, IsWindowVisible, LoadCursorW, SM_SWAPBUTTON, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_DWMCOMPOSITIONCHANGED, WM_SETTINGCHANGE, WM_THEMECHANGED,
    MsgWaitForMultipleObjectsEx, PeekMessageW, PostThreadMessageW, RegisterClassExW, SetCursor, SetWindowLongW, SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow,
    CS_DBLCLKS, GWL_EXSTYLE, GW_OWNER, IDC_ARROW, MA_NOACTIVATE, MSG, MWMO_INPUTAVAILABLE, PM_NOREMOVE, PM_REMOVE, QS_ALLINPUT, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WM_APP, WM_CAPTURECHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_QUIT, WM_SETCURSOR, WM_USER,
    WNDCLASSEXW, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::dock::{Dock, Layout, Span};

/// What the bar shows.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub up: String,
    pub down: String,
    /// "both", "down" or "up".
    pub show: String,
    /// The taskbar is light: dark text.
    pub light: bool,
    pub bits: bool,
}

impl View {
    const fn empty() -> View {
        View { up: String::new(), down: String::new(), show: String::new(), light: false, bits: false }
    }
}

/// The taskbar's buttons as UI Automation last found them, for a taskbar
/// spanning `left..right`.
#[derive(Debug, Clone)]
struct Measured {
    left: i32,
    right: i32,
    tray_left: i32,
    spans: Vec<Span>,
}

struct Shared {
    want: bool,
    view: View,
    /// Where the bar is to be: the settings', or a drag's not saved yet.
    dock: Option<Dock>,
    /// The settings' dock as last given: a different one (Put it back) wins
    /// over a drag, the same one again does not.
    dock_given: Option<Dock>,
    /// The bar's thread: 0 none, STARTING while it starts.
    thread: u32,
    uia: bool,
    measured: Option<Measured>,
    /// Ask UI Automation again now: the taskbar changed.
    remeasure: bool,
    uia_error: Option<String>,
    /// Where the bar is (x, y, w, h), and whether clicks pass through it.
    placed: Option<([i32; 4], bool)>,
    /// The bar's window (0 none).
    hwnd: usize,
    /// What the bar did and why, not yet written to `netspeed-bar.log` ([`note`]).
    log: Vec<String>,
    /// The bar stopped on errors too often: no new thread until it is turned
    /// off and on again (or GitSwitch restarts).
    gave_up: bool,
}

const STARTING: u32 = u32::MAX;
/// To the bar's thread: look again now.
const WM_LOOK: u32 = WM_APP + 0x51;

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    want: false,
    view: View::empty(),
    dock: None,
    dock_given: None,
    thread: 0,
    uia: false,
    measured: None,
    remeasure: false,
    uia_error: None,
    placed: None,
    hwnd: 0,
    log: Vec::new(),
    gave_up: false,
});
static WAKE: Condvar = Condvar::new();

fn shared() -> MutexGuard<'static, Shared> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
}

fn post(thread: u32, msg: u32) {
    if thread != 0 && thread != STARTING {
        // SAFETY: posting to a thread's queue; a thread that is gone just fails it.
        unsafe { PostThreadMessageW(thread, msg, 0, 0) };
    }
}

/// A line for `netspeed-bar.log`: what the bar did and why, so a bar that went
/// missing can be explained afterwards. Kept here until the sampler writes it
/// out ([`take_log`]): the bar's thread never touches the disk, since it shares
/// its input with the taskbar's and must never keep Explorer waiting. Never
/// called with `shared()` held.
fn note(what: impl Into<String>) {
    // UTC: the local zone would be a registry read on the bar's thread, and
    // chrono panics when Windows cannot say what the zone is.
    let line = format!("{} {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3fZ"), what.into());
    let mut sh = shared();
    if sh.log.len() >= 200 {
        sh.log.remove(0);
    }
    sh.log.push(line);
}

/// The lines noted since the last call, oldest first.
pub fn take_log() -> Vec<String> {
    std::mem::take(&mut shared().log)
}

/// One kind of event for the log: said at once the first time, then at most
/// once a minute with how many there were since the last line, so a burst
/// shows as a count and a rare event as itself.
#[derive(Debug, Default)]
struct Tally {
    since: u32,
    said: Option<Instant>,
}

impl Tally {
    /// One more; how many since the last line, when it is time for one.
    fn hit(&mut self) -> Option<u32> {
        self.since += 1;
        if self.said.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return None;
        }
        self.said = Some(Instant::now());
        Some(std::mem::take(&mut self.since))
    }
}

/// " (×n since the last line)" for a count above one.
fn times(n: u32) -> String {
    if n > 1 {
        format!(" (×{} since the last line)", n)
    } else {
        String::new()
    }
}

/// How DWM has cloaked `h` (0: not at all). A cloaked window is invisible
/// though `IsWindowVisible` says it is shown: the shell cloaks the windows of
/// other virtual desktops, for one.
fn cloaked(h: HWND) -> u32 {
    let mut v: u32 = 0;
    // SAFETY: a valid out-pointer of the size given; a handle that is gone just fails.
    let hr = unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut u32 as *mut c_void, std::mem::size_of::<u32>() as u32) };
    if hr == 0 {
        v
    } else {
        0
    }
}

// The one DWM call the bar needs, declared here rather than through a
// windows-sys feature (which would rebuild every crate that uses windows-sys).
#[link(name = "dwmapi")]
extern "system" {
    fn DwmGetWindowAttribute(hwnd: HWND, attribute: u32, value: *mut c_void, size: u32) -> i32;
}
/// `DWMWA_CLOAKED`, and its "by the shell" bit (dwmapi.h).
const DWMWA_CLOAKED: u32 = 14;
const DWM_CLOAKED_SHELL: u32 = 2;

/// Show, update or take away the bar. Called by the sampler on every tick.
pub fn sync(want: bool, view: View, dock: Option<Dock>) {
    let mut sh = shared();
    sh.want = want;
    sh.view = view;
    if dock != sh.dock_given {
        sh.dock_given = dock;
        sh.dock = dock;
    }
    if want {
        if sh.thread == 0 && !sh.gave_up {
            sh.thread = STARTING;
            if std::thread::Builder::new().name("gitswitch-speed-bar".into()).spawn(bar_thread).is_err() {
                sh.thread = 0;
            }
        }
        if !sh.uia {
            sh.uia = true;
            if std::thread::Builder::new().name("gitswitch-speed-bar-uia".into()).spawn(uia_thread).is_err() {
                sh.uia = false;
            }
        }
        post(sh.thread, WM_LOOK);
    } else {
        sh.gave_up = false;
        post(sh.thread, WM_QUIT);
        WAKE.notify_all();
    }
}

// ---- the bar's thread ------------------------------------------------------

fn bar_thread() {
    // SAFETY: calls on this thread only; the PeekMessage makes its queue so
    // messages posted from now on are kept.
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mut msg: MSG = std::mem::zeroed();
        PeekMessageW(&mut msg, null_mut(), WM_USER, WM_USER, PM_NOREMOVE);
        shared().thread = GetCurrentThreadId();
    }
    // Should the thread end on a panic outside a look (none is expected), its
    // slot is freed all the same: no message may go to a thread id Windows can
    // hand to another program. No new thread follows until the bar is turned
    // off and on (a loop of failing threads would be worse).
    struct Ending;
    impl Drop for Ending {
        fn drop(&mut self) {
            if std::thread::panicking() {
                {
                    let mut sh = shared();
                    sh.thread = 0;
                    sh.placed = None;
                    sh.hwnd = 0;
                    sh.gave_up = true;
                }
                note("the bar's thread stopped on an error: off until it is turned off and on, or GitSwitch restarts");
            }
        }
    }
    let _ending = Ending;
    note("bar started");
    let mut bar = Bar::new();
    // Looks that ended on an error in the last 10 minutes. Each makes the bar
    // afresh (before, the thread died and the bar was gone until GitSwitch
    // restarted); a 4th within 10 minutes turns it off.
    let mut errors: Vec<Instant> = Vec::new();
    loop {
        // SAFETY: this thread's own message loop.
        unsafe {
            MsgWaitForMultipleObjectsEx(0, null(), 250, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                // Messages to the thread (look again, quit) only wake it.
                if msg.hwnd.is_null() {
                    continue;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if !shared().want {
            bar.close();
            let mut sh = shared();
            if !sh.want {
                sh.thread = 0;
                sh.placed = None;
                drop(sh);
                note("bar taken away: turned off");
                return;
            }
        }
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bar.look())) {
            let what = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_else(|| "no message".into());
            errors.retain(|t| t.elapsed() < Duration::from_secs(600));
            errors.push(Instant::now());
            // The old window goes with the old bar; the next look makes one.
            bar = Bar::new();
            if errors.len() > 3 {
                {
                    let mut sh = shared();
                    sh.thread = 0;
                    sh.placed = None;
                    sh.gave_up = true;
                }
                note(format!("the bar stopped on an error a 4th time in 10 minutes ({}): off until it is turned off and on, or GitSwitch restarts", what));
                return;
            }
            note(format!("the bar stopped on an error ({}): made afresh", what));
        }
    }
}

/// The geometry the mouse needs (set by `look`, read by the window procedure).
#[derive(Debug, Clone, Copy)]
struct Geom {
    left: i32,
    right: i32,
    tray_left: i32,
    w: i32,
    x: i32,
    y: i32,
    scale: f64,
    horizontal: bool,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    cursor: i32,
    from: i32,
    moved: bool,
}

thread_local! {
    static GEOM: Cell<Option<Geom>> = const { Cell::new(None) };
    static DRAG: Cell<Option<Drag>> = const { Cell::new(None) };
    /// Moved by hand since it was last drawn: draw it again where it belongs.
    static MOVED: Cell<bool> = const { Cell::new(false) };
    /// A display, scale or theme change since the last look: draw it afresh.
    static REDRAW: Cell<bool> = const { Cell::new(false) };
}

/// Sizes at one DPI.
#[derive(Debug, Clone, Copy)]
struct Metrics {
    w: i32,
    h: i32,
    pad: i32,
    gap: i32,
    vpad: i32,
    line_h: i32,
    arrow_w: i32,
    text_h: i32,
}

#[derive(Debug, Clone, PartialEq)]
struct Drawn {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    dpi: u32,
    view: View,
    through: bool,
}

struct Bar {
    hwnd: HWND,
    owner: HWND,
    shown: bool,
    drawn: Option<Drawn>,
    font: Option<(u32, HFONT)>,
    /// The taskbar (extent, notification-area edge) UI Automation was last asked about.
    asked: Option<(i32, i32, i32)>,
    /// Since when the buttons measured are not those of the taskbar as it is.
    stale_since: Option<Instant>,
    /// Its sizes for this DPI and text (measured only when they change).
    sized: Option<(u32, View, Metrics)>,
    /// What the log is told, each kind at most once a minute: put right after
    /// outside changes, hidden with the taskbar, made again, not made. The
    /// last drawing error, while it lasts.
    reshown: Tally,
    moved_back: Tally,
    uncloaked: Tally,
    off_screen: Tally,
    remade: Tally,
    make_failed: Tally,
    draw_failed: Option<u32>,
    /// Draw at the next look even if nothing about it changed (a display,
    /// scale or theme change; moved from outside). Where it was drawn stays
    /// known: the layout goes by it.
    force: bool,
    /// When it was last asked whether it is cloaked (once a second), and
    /// since when the shell has kept it cloaked.
    cloak_checked: Option<Instant>,
    cloaked_since: Option<Instant>,
    /// No taskbar was found at the last look (said once, not at every look).
    no_taskbar: bool,
    /// When the bar last said where it is.
    beat: Option<Instant>,
}

const UP: [u8; 3] = [0xd9, 0x59, 0x26];
const DOWN: [u8; 3] = [0x39, 0x87, 0xe5];
/// Text on a dark taskbar, and on a light one.
const INK_ON_DARK: [u8; 3] = [0xf4, 0xf4, 0xf5];
const INK_ON_LIGHT: [u8; 3] = [0x18, 0x18, 0x1b];

/// The lines, top to bottom: arrow, text, the arrow's colour.
fn lines_of(v: &View) -> Vec<(&'static str, &str, [u8; 3])> {
    match v.show.as_str() {
        "down" => vec![("↓", v.down.as_str(), DOWN)],
        "up" => vec![("↑", v.up.as_str(), UP)],
        _ => vec![("↑", v.up.as_str(), UP), ("↓", v.down.as_str(), DOWN)],
    }
}

/// One pixel of the bar: `colour` at text coverage `cov` (0–255), as
/// premultiplied BGRA; where there is no text, a background so faint it can't
/// be seen but still takes the mouse (or none at all: clicks pass through).
pub(crate) fn pixel(cov: u32, colour: [u8; 3], background: u32) -> u32 {
    if cov == 0 {
        return background << 24;
    }
    let c = |v: u8| (v as u32 * cov + 127) / 255;
    (cov << 24) | (c(colour[0]) << 16) | (c(colour[1]) << 8) | c(colour[2])
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rect(h: HWND) -> Option<RECT> {
    // SAFETY: a valid out-pointer; the handle is only read.
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        (GetWindowRect(h, &mut r) != 0).then_some(r)
    }
}

fn taskbar() -> Option<HWND> {
    // SAFETY: a valid class-name string.
    let h = unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), null()) };
    (!h.is_null()).then_some(h)
}

fn tray_rect(tb: HWND) -> Option<RECT> {
    // SAFETY: a valid class-name string; the handle is only read.
    let h = unsafe { FindWindowExW(tb, null_mut(), wide("TrayNotifyWnd").as_ptr(), null()) };
    if h.is_null() {
        None
    } else {
        rect(h)
    }
}

/// Where the notification area starts on a taskbar along the bottom (or top).
fn tray_left_of(tb: HWND, r: &RECT) -> i32 {
    tray_rect(tb).map(|t| t.left).filter(|&l| l > r.left && l <= r.right).unwrap_or(r.right)
}

/// The taskbar is shown, not auto-hidden off the screen.
fn on_screen(tb: HWND, r: &RECT) -> bool {
    // SAFETY: handles are only read; MONITORINFO is sized as declared.
    unsafe {
        if IsWindowVisible(tb) == 0 {
            return false;
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(tb, MONITOR_DEFAULTTONEAREST), &mut info) == 0 {
            return true;
        }
        let m = info.rcMonitor;
        let iw = (r.right.min(m.right) - r.left.max(m.left)).max(0) as i64;
        let ih = (r.bottom.min(m.bottom) - r.top.max(m.top)).max(0) as i64;
        let area = ((r.right - r.left) as i64 * (r.bottom - r.top) as i64).max(1);
        iw * ih * 2 >= area
    }
}

fn cursor_x() -> i32 {
    // SAFETY: a valid out-pointer.
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        p.x
    }
}

fn text_out(dc: HDC, x: i32, y: i32, s: &str) {
    let w: Vec<u16> = s.encode_utf16().collect();
    // SAFETY: the buffer and its length match.
    unsafe { TextOutW(dc, x, y, w.as_ptr(), w.len() as i32) };
}

fn extent(dc: HDC, s: &str) -> (i32, i32) {
    let w: Vec<u16> = s.encode_utf16().collect();
    // SAFETY: the buffer and its length match; a valid out-pointer.
    unsafe {
        let mut sz = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut sz);
        (sz.cx, sz.cy)
    }
}

impl Bar {
    fn new() -> Bar {
        Bar {
            hwnd: null_mut(),
            owner: null_mut(),
            shown: false,
            drawn: None,
            font: None,
            asked: None,
            stale_since: None,
            sized: None,
            reshown: Tally::default(),
            moved_back: Tally::default(),
            uncloaked: Tally::default(),
            off_screen: Tally::default(),
            remade: Tally::default(),
            make_failed: Tally::default(),
            draw_failed: None,
            force: false,
            cloak_checked: None,
            cloaked_since: None,
            no_taskbar: false,
            beat: None,
        }
    }

    fn font_for(&mut self, dpi: u32) -> HFONT {
        if let Some((d, f)) = self.font {
            if d == dpi {
                return f;
            }
            // SAFETY: our own font, no longer selected anywhere.
            unsafe { DeleteObject(f) };
        }
        let px = (12.0 * dpi as f64 / 96.0).round() as i32;
        // SAFETY: a valid face-name string. Grey antialiasing (not ClearType):
        // the coverage becomes the alpha.
        let f = unsafe {
            CreateFontW(
                -px,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_TT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                (DEFAULT_PITCH | FF_DONTCARE) as u32,
                wide("Segoe UI").as_ptr(),
            )
        };
        self.font = Some((dpi, f));
        f
    }

    fn metrics(&mut self, v: &View, dpi: u32) -> Metrics {
        let font = self.font_for(dpi);
        let s = dpi as f64 / 96.0;
        let px = |l: f64| (l * s).round() as i32;
        // SAFETY: a memory DC made, used and released here.
        unsafe {
            let screen = GetDC(null_mut());
            let dc = CreateCompatibleDC(screen);
            let old = SelectObject(dc, font);
            // The widest a number gets, so the bar keeps its size as the numbers change.
            let samples: [&str; 4] = if v.bits { ["88.8 Mbps", "88.8 Gbps", "88.8 Kbps", "888 Mbps"] } else { ["88.8 MB/s", "88.8 GB/s", "88.8 KB/s", "888 MB/s"] };
            let num_w = samples.iter().copied().chain([v.up.as_str(), v.down.as_str()]).map(|t| extent(dc, t).0).max().unwrap_or(0);
            let (up_w, text_h) = extent(dc, "↑");
            let arrow_w = up_w.max(extent(dc, "↓").0);
            SelectObject(dc, old);
            DeleteDC(dc);
            ReleaseDC(null_mut(), screen);
            let (pad, gap, vpad) = (px(4.0), px(4.0), px(2.0));
            let line_h = px(14.0).max(text_h);
            let lines = if v.show == "both" { 2 } else { 1 };
            Metrics { w: pad + arrow_w + gap + num_w + pad, h: vpad * 2 + line_h * lines, pad, gap, vpad, line_h, arrow_w, text_h }
        }
    }

    /// Make the window, owned by the taskbar `tb`; the Windows error if it could not be made.
    fn create(&mut self, tb: HWND) -> Result<(), u32> {
        // SAFETY: plain window creation on this thread; strings outlive the calls.
        unsafe {
            let inst = GetModuleHandleW(null());
            let class = wide("GitSwitchSpeedBar");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: null_mut(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class.as_ptr(),
                hIconSm: null_mut(),
            };
            // Registered once per process; again just fails.
            RegisterClassExW(&wc);
            // Topmost exactly when the taskbar is: Windows keeps the two in step from then on.
            let topmost = (GetWindowLongW(tb, GWL_EXSTYLE) as u32) & WS_EX_TOPMOST != 0;
            let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | if topmost { WS_EX_TOPMOST } else { 0 };
            let title = wide("GitSwitch net speed");
            let h = CreateWindowExW(ex, class.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 0, 0, tb, null_mut(), inst, null());
            if h.is_null() {
                return Err(GetLastError());
            }
            self.hwnd = h;
            self.owner = tb;
            self.shown = false;
            self.drawn = None;
            shared().hwnd = h as usize;
            Ok(())
        }
    }

    fn close(&mut self) {
        // SAFETY: our own window, destroyed on the thread that made it.
        unsafe {
            if !self.hwnd.is_null() && IsWindow(self.hwnd) != 0 {
                DestroyWindow(self.hwnd);
            }
        }
        {
            // Only its own: a bar of an older thread going away must not wipe a newer one's.
            let mut sh = shared();
            if sh.hwnd == self.hwnd as usize {
                sh.hwnd = 0;
            }
        }
        self.hwnd = null_mut();
        self.owner = null_mut();
        self.shown = false;
        self.drawn = None;
        self.stale_since = None;
        GEOM.with(|g| g.set(None));
        DRAG.with(|d| d.set(None));
    }

    fn hide(&mut self) {
        if self.shown {
            // SAFETY: our own window.
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.shown = false;
        }
    }

    /// Put the bar where it belongs now, drawn as it should be.
    fn look(&mut self) {
        if DRAG.with(|d| d.get()).is_some() {
            // A drag whose button came up where this window never heard of it
            // would stop every look for good: let go once neither the capture
            // nor the button is held any more.
            // SAFETY: state reads only.
            let held = unsafe {
                let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
                GetCapture() == self.hwnd || (GetAsyncKeyState(button as i32) as u16) & 0x8000 != 0
            };
            if held {
                return;
            }
            DRAG.with(|d| d.set(None));
            MOVED.with(|c| c.set(true));
            note("a drag ended without its button coming up on the bar: let go");
        }
        let (view, dock, measured) = {
            let sh = shared();
            (sh.view.clone(), sh.dock, sh.measured.clone())
        };
        if view.up.is_empty() && view.down.is_empty() {
            return;
        }
        let Some(tb) = taskbar() else {
            if !self.no_taskbar {
                self.no_taskbar = true;
                note("no taskbar found (Explorer not running?): the bar waits for one");
            }
            self.close();
            return;
        };
        self.no_taskbar = false;
        // SAFETY: a handle check only.
        if self.hwnd.is_null() || unsafe { IsWindow(self.hwnd) } == 0 || self.owner != tb {
            // First time, or Explorer restarted (the old taskbar took the bar with it).
            let why = if self.hwnd.is_null() {
                "made"
            } else if self.owner != tb {
                "made again: the taskbar is a new one (Explorer restarted)"
            } else {
                "made again: its window was gone"
            };
            self.close();
            if let Err(e) = self.create(tb) {
                if let Some(n) = self.make_failed.hit() {
                    note(format!("the bar's window could not be made (error {}){}", e, times(n)));
                }
                return;
            }
            if why == "made" {
                note(format!("bar made for taskbar {:#x}", tb as usize));
            } else if let Some(n) = self.remade.hit() {
                note(format!("bar {} for taskbar {:#x}{}", why, tb as usize, times(n)));
            }
        }
        let Some(tr) = rect(tb) else { return };
        if !on_screen(tb, &tr) {
            if self.shown {
                if let Some(n) = self.off_screen.hit() {
                    note(format!("hidden while the taskbar is off the screen or hidden{}", times(n)));
                }
            }
            self.hide();
            return;
        }
        // SAFETY: the handle is only read.
        let dpi = unsafe { GetDpiForWindow(tb) }.max(96);
        let scale = dpi as f64 / 96.0;
        let m = match &self.sized {
            Some((d, v, m)) if *d == dpi && *v == view => *m,
            _ => {
                let m = self.metrics(&view, dpi);
                self.sized = Some((dpi, view.clone(), m));
                m
            }
        };
        // Just dragged: where it was drawn before is no reference any more (a
        // bar dropped on icons goes to the free spot nearest the drop).
        if MOVED.with(|c| c.replace(false)) {
            self.drawn = None;
        }
        // A display, scale or theme change (see `wndproc`): drawn afresh, in place.
        if REDRAW.with(|c| c.replace(false)) {
            self.force = true;
        }
        // What was done to the window from outside GitSwitch. Explorer can hide
        // the windows the taskbar owns, and Windows moves windows about when a
        // display changes. Going by its own record, the bar then stayed hidden,
        // or away from where it belongs, until the taskbar's icons next moved
        // it (opening GitSwitch's window was enough: its button moves them).
        // SAFETY: reads of our own window.
        if self.shown && unsafe { IsWindowVisible(self.hwnd) } == 0 {
            self.shown = false;
            if let Some(n) = self.reshown.hit() {
                note(format!("hidden from outside GitSwitch: shown again{}", times(n)));
            }
        }
        if let (Some(d), Some(r)) = (self.drawn.as_ref(), rect(self.hwnd)) {
            if (r.left, r.top, r.right - r.left, r.bottom - r.top) != (d.x, d.y, d.w, d.h) {
                if let Some(n) = self.moved_back.hit() {
                    note(format!(
                        "moved from outside GitSwitch to {},{} ({}×{}): put back at {},{}{}",
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        d.x,
                        d.y,
                        times(n)
                    ));
                }
                self.force = true;
            }
        }
        // Cloaked by the shell while the taskbar is not (as a window of another
        // virtual desktop is): invisible though Windows calls it shown. After a
        // moment it is made afresh, which puts it on the desktop in front.
        if self.cloak_checked.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            self.cloak_checked = Some(Instant::now());
            if cloaked(self.hwnd) & DWM_CLOAKED_SHELL != 0 && cloaked(tb) == 0 {
                let since = *self.cloaked_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(2) {
                    self.cloaked_since = None;
                    if let Some(n) = self.uncloaked.hit() {
                        note(format!("cloaked by Windows (another virtual desktop?): made afresh{}", times(n)));
                    }
                    // Made again at the next look.
                    self.close();
                    return;
                }
            } else {
                self.cloaked_since = None;
            }
        }
        let margin = (4.0 * scale).round() as i32;
        let tray = tray_rect(tb);
        let horizontal = tr.right - tr.left >= tr.bottom - tr.top;
        let (x, y, through, tray_left) = if horizontal {
            let tray_left = tray_left_of(tb, &tr);
            let now = (tr.left, tr.right, tray_left);
            let fresh = measured.filter(|ms| (ms.left, ms.right, ms.tray_left) == now);
            let y = tr.top + (tr.bottom - tr.top - m.h) / 2;
            // The buttons as measured are not those of the taskbar as it is: ask
            // again, and for a moment don't trust them. Not shown yet, the bar
            // waits (so it doesn't appear on an icon and then jump); shown, it
            // stays where it is while the icons slide.
            let mut hold = None;
            if fresh.is_some() {
                self.stale_since = None;
            } else {
                if self.asked != Some(now) {
                    self.asked = Some(now);
                    shared().remeasure = true;
                    WAKE.notify_all();
                }
                let since = *self.stale_since.get_or_insert_with(Instant::now);
                if since.elapsed() < Duration::from_millis(1500) && shared().uia_error.is_none() {
                    if !self.shown {
                        return;
                    }
                    hold = self.drawn.as_ref().map(|d| (d.x, d.through));
                }
            }
            match hold {
                Some((x, through)) => (x, y, through, tray_left),
                None => {
                    let mut occupied = vec![Span { lo: tray_left, hi: tr.right }];
                    if let Some(ms) = fresh {
                        occupied.extend(ms.spans);
                    }
                    let layout = Layout { left: tr.left, right: tr.right, tray_left, occupied, w: m.w, margin, scale };
                    let at = self.drawn.as_ref().filter(|_| self.shown).map(|d| d.x);
                    let (x, through) = layout.settle(layout.preferred_x(dock), at);
                    (x, y, through, tray_left)
                }
            }
        } else {
            // A taskbar on a side of the screen: centred, just above the notification area.
            let top = tray.map(|t| t.top).unwrap_or(tr.bottom);
            (tr.left + (tr.right - tr.left - m.w) / 2, top - m.h - margin, false, tr.right)
        };
        GEOM.with(|g| g.set(Some(Geom { left: tr.left, right: tr.right, tray_left, w: m.w, x, y, scale, horizontal })));
        let d = Drawn { x, y, w: m.w, h: m.h, dpi, view, through };
        if self.force || self.drawn.as_ref() != Some(&d) {
            // Recorded as drawn only once Windows took it: else the next look tries again.
            if let Err(e) = self.draw(&d, &m) {
                if self.draw_failed != Some(e) {
                    note(format!("could not be drawn (error {}): trying again", e));
                }
                self.draw_failed = Some(e);
                return;
            }
            if let Some(e) = self.draw_failed.take() {
                note(format!("drawn again after error {}", e));
            }
            shared().placed = Some(([x, y, m.w, m.h], through));
            self.drawn = Some(d);
            self.force = false;
        }
        if !self.shown {
            // SAFETY: our own window; shown without taking the focus.
            unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
            self.shown = true;
        }
        // Where it is, now and then: a gap in these lines says the bar's thread stopped.
        if self.beat.is_none_or(|t| t.elapsed() >= Duration::from_secs(600)) {
            self.beat = Some(Instant::now());
            note(format!("shown at {},{} ({}×{}) on taskbar {:#x}, {} dpi, cloaked {}", x, y, m.w, m.h, tb as usize, dpi, cloaked(self.hwnd)));
        }
    }

    /// Draw `d` into the window; the Windows error when it did not take it.
    fn draw(&mut self, d: &Drawn, m: &Metrics) -> Result<(), u32> {
        let font = self.font_for(d.dpi);
        // SAFETY: GDI objects made, used and released here; the pixel buffer
        // is the DIB section's own, sized w × h × 4 bytes.
        unsafe {
            let screen = GetDC(null_mut());
            let dc = CreateCompatibleDC(screen);
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = d.w;
            bi.bmiHeader.biHeight = -d.h;
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            bi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut c_void = null_mut();
            let dib = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if dib.is_null() || bits.is_null() {
                let e = GetLastError();
                DeleteDC(dc);
                ReleaseDC(null_mut(), screen);
                return Err(e);
            }
            let old_bmp = SelectObject(dc, dib);
            let old_font = SelectObject(dc, font);
            let px = std::slice::from_raw_parts_mut(bits as *mut u32, (d.w * d.h) as usize);
            px.fill(0);
            SetBkMode(dc, TRANSPARENT as _);
            SetTextColor(dc, 0x00ff_ffff);
            let lines = lines_of(&d.view);
            for (i, (arrow, text, _)) in lines.iter().enumerate() {
                let top = m.vpad + i as i32 * m.line_h + (m.line_h - m.text_h) / 2;
                text_out(dc, m.pad, top, arrow);
                text_out(dc, d.w - m.pad - extent(dc, text).0, top, text);
            }
            GdiFlush();
            // White on black with grey antialiasing: the coverage, coloured.
            let ink = if d.view.light { INK_ON_LIGHT } else { INK_ON_DARK };
            let split = m.pad + m.arrow_w + m.gap / 2;
            let background = if d.through { 0 } else { 1 };
            for y in 0..d.h {
                let line = (((y - m.vpad).max(0) / m.line_h.max(1)) as usize).min(lines.len().saturating_sub(1));
                let arrow = lines.get(line).map(|l| l.2).unwrap_or(ink);
                for x in 0..d.w {
                    let p = &mut px[(y * d.w + x) as usize];
                    let cov = ((*p >> 16) & 0xff).max((*p >> 8) & 0xff).max(*p & 0xff);
                    *p = pixel(cov, if x < split { arrow } else { ink }, background);
                }
            }
            let at = POINT { x: d.x, y: d.y };
            let size = SIZE { cx: d.w, cy: d.h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let taken = UpdateLayeredWindow(self.hwnd, screen, &at, &size, dc, &src, 0, &blend, ULW_ALPHA) != 0;
            let e = if taken { 0 } else { GetLastError() };
            SelectObject(dc, old_font);
            SelectObject(dc, old_bmp);
            DeleteObject(dib);
            DeleteDC(dc);
            ReleaseDC(null_mut(), screen);
            // Clicks pass through only when it has nowhere to go but over something.
            let ex = GetWindowLongW(self.hwnd, GWL_EXSTYLE) as u32;
            let want = if d.through { ex | WS_EX_TRANSPARENT } else { ex & !WS_EX_TRANSPARENT };
            if want != ex {
                SetWindowLongW(self.hwnd, GWL_EXSTYLE, want as i32);
            }
            if taken {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        self.close();
        if let Some((_, f)) = self.font.take() {
            // SAFETY: our own font, selected nowhere.
            unsafe { DeleteObject(f) };
        }
    }
}

/// A drop: remembered as a distance from the nearer end, and looked at again
/// (dropped on icons, it moves clear of them).
fn dropped(g: Geom) {
    let dock = Layout { left: g.left, right: g.right, tray_left: g.tray_left, occupied: Vec::new(), w: g.w, margin: 0, scale: g.scale }.dock_at(g.x);
    shared().dock = Some(dock);
    remember(dock);
}

// What a drop or a double-click asks of the app, off the bar's thread. A
// probe runs the bar in a test binary, where there is no app (and linking
// the app's windows in would need a manifest a test binary doesn't have).
#[cfg(not(test))]
fn remember(dock: Dock) {
    std::thread::spawn(move || super::bar_docked(dock));
}

#[cfg(test)]
fn remember(_dock: Dock) {}

#[cfg(not(test))]
fn open_the_tab() {
    std::thread::spawn(super::open_tab);
}

#[cfg(test)]
fn open_the_tab() {}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Clicking it never takes the focus from the app in front.
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(null_mut(), IDC_ARROW));
            1
        }
        WM_LBUTTONDOWN => {
            if let Some(g) = GEOM.with(|g| g.get()).filter(|g| g.horizontal) {
                DRAG.with(|d| d.set(Some(Drag { cursor: cursor_x(), from: g.x, moved: false })));
                SetCapture(h);
            }
            0
        }
        WM_MOUSEMOVE => {
            if let (Some(mut d), Some(mut g)) = (DRAG.with(|d| d.get()), GEOM.with(|g| g.get())) {
                let dx = cursor_x() - d.cursor;
                if d.moved || dx.abs() > 3 {
                    d.moved = true;
                    DRAG.with(|c| c.set(Some(d)));
                    // Along the taskbar only.
                    g.x = (d.from + dx).clamp(g.left, (g.right - g.w).max(g.left));
                    GEOM.with(|c| c.set(Some(g)));
                    MOVED.with(|c| c.set(true));
                    SetWindowPos(h, null_mut(), g.x, g.y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(d) = DRAG.with(|c| c.take()) {
                ReleaseCapture();
                if d.moved {
                    if let Some(g) = GEOM.with(|c| c.get()) {
                        dropped(g);
                    }
                }
            }
            0
        }
        // The mouse was taken away mid-drag: the next look puts the bar back.
        WM_CAPTURECHANGED => {
            DRAG.with(|c| c.set(None));
            0
        }
        WM_LBUTTONDBLCLK => {
            open_the_tab();
            0
        }
        // The screen, its scale or the theme changed, or the compositor
        // restarted: drawn afresh at the next look, from the bar's own
        // measurements (never from what these messages suggest).
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE | WM_THEMECHANGED | WM_DWMCOMPOSITIONCHANGED => {
            REDRAW.with(|c| c.set(true));
            DefWindowProcW(h, msg, wp, lp)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

// ---- UI Automation: where the taskbar's buttons are -------------------------

struct Finder {
    uia: ::windows::Win32::UI::Accessibility::IUIAutomation,
    cache: ::windows::Win32::UI::Accessibility::IUIAutomationCacheRequest,
    all: ::windows::Win32::UI::Accessibility::IUIAutomationCondition,
}

impl Finder {
    fn new() -> ::windows::core::Result<Finder> {
        use ::windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use ::windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId};
        // SAFETY: COM is initialised on this thread by the caller.
        unsafe {
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let cache = uia.CreateCacheRequest()?;
            cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
            cache.AddProperty(UIA_ControlTypePropertyId)?;
            let all = uia.CreateTrueCondition()?;
            Ok(Finder { uia, cache, all })
        }
    }

    /// Every button on the taskbar (Widgets, Start, Search, the apps, the
    /// tray's), as stretches of it: one batched question to Explorer.
    fn buttons(&self, tb: HWND, r: &RECT) -> ::windows::core::Result<Measured> {
        use ::windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_ButtonControlTypeId};
        // SAFETY: COM calls on interfaces this thread made.
        unsafe {
            let root = self.uia.ElementFromHandle(::windows::Win32::Foundation::HWND(tb))?;
            let found = root.FindAllBuildCache(TreeScope_Descendants, &self.all, &self.cache)?;
            let mut spans = Vec::new();
            for i in 0..found.Length()? {
                let e = found.GetElement(i)?;
                if e.CachedControlType()? != UIA_ButtonControlTypeId {
                    continue;
                }
                let b = e.CachedBoundingRectangle()?;
                if b.right > b.left && b.bottom > r.top && b.top < r.bottom {
                    spans.push(Span { lo: b.left, hi: b.right });
                }
            }
            Ok(Measured { left: r.left, right: r.right, tray_left: tray_left_of(tb, r), spans })
        }
    }
}

/// Which windows have a taskbar button now, as one number: when it changes,
/// an icon may have come or gone. Cheap: nothing is asked of Explorer.
fn windows_now() -> u64 {
    use std::hash::{Hash, Hasher};
    unsafe extern "system" fn each(h: HWND, list: LPARAM) -> BOOL {
        // `list` is the Vec handed to EnumWindows below, alive for the call;
        // the handles are only read.
        let list = &mut *(list as *mut Vec<usize>);
        if IsWindowVisible(h) != 0 && GetWindow(h, GW_OWNER).is_null() {
            let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW == 0 || ex & WS_EX_APPWINDOW != 0 {
                list.push(h as usize);
            }
        }
        1
    }
    let mut list: Vec<usize> = Vec::new();
    // SAFETY: the callback only touches `list`, which outlives the call.
    unsafe { EnumWindows(Some(each), &mut list as *mut Vec<usize> as LPARAM) };
    list.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    list.hash(&mut h);
    h.finish()
}

/// What decides where the taskbar's icons are, as one number: the windows
/// with a taskbar button, the taskbar, and the notification area (a tray icon
/// coming or going slides a crowded row of app icons along).
fn layout_key() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    windows_now().hash(&mut h);
    if let Some(tb) = taskbar() {
        if let Some(r) = rect(tb) {
            (r.left, r.top, r.right, r.bottom).hash(&mut h);
        }
        if let Some(t) = tray_rect(tb) {
            (t.left, t.right).hash(&mut h);
        }
    }
    h.finish()
}

fn uia_thread() {
    use ::windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    // Physical pixels, as the bar's thread and UI Automation count them: the
    // taskbar's extent must compare equal to the bar's own reading of it.
    // SAFETY: a call on this thread only.
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // SAFETY: COM on this thread, every interface released before it ends.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    match Finder::new() {
        Ok(finder) => {
            // Each question costs Explorer some milliseconds, so it is asked when
            // an icon may have come or gone (the set of windows changed: at once,
            // and again once the icons have slid into place), when the bar asks,
            // and otherwise every few seconds (pins, badges).
            let mut seen: Option<u64> = None;
            let mut last: Option<Instant> = None;
            let mut again: Option<Instant> = None;
            loop {
                if !shared().want {
                    break;
                }
                let now = layout_key();
                let changed = seen != Some(now);
                let due = changed || shared().remeasure || again.is_some_and(|t| Instant::now() >= t) || last.is_none_or(|t| t.elapsed() >= Duration::from_secs(5));
                if due {
                    if let Some((tb, r)) = taskbar().and_then(|tb| rect(tb).map(|r| (tb, r))) {
                        let got = finder.buttons(tb, &r);
                        let mut sh = shared();
                        sh.remeasure = false;
                        match got {
                            Ok(m) => {
                                sh.measured = Some(m);
                                sh.uia_error = None;
                                post(sh.thread, WM_LOOK);
                            }
                            Err(e) => sh.uia_error = Some(e.message()),
                        }
                    }
                    last = Some(Instant::now());
                    again = if changed { Some(Instant::now() + Duration::from_millis(700)) } else { again.filter(|t| Instant::now() < *t) };
                    seen = Some(now);
                }
                let sh = shared();
                let _ = WAKE.wait_timeout_while(sh, Duration::from_millis(250), |s| s.want && !s.remeasure);
            }
        }
        Err(e) => shared().uia_error = Some(format!("UI Automation is not available: {}", e.message())),
    }
    if com {
        // SAFETY: matches the CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    shared().uia = false;
}

// ---- probes (read-only checks on a real computer) ----------------------------

/// The taskbar as this computer has it now: its buttons, the free stretches,
/// and where the bar would go. Nothing is shown.
#[cfg(test)]
pub fn probe() -> serde_json::Value {
    use ::windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    // SAFETY: calls on this thread only.
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // SAFETY: COM on this thread; the Finder is dropped inside the closure.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let out = (|| -> Result<serde_json::Value, String> {
        let tb = taskbar().ok_or("no taskbar")?;
        let r = rect(tb).ok_or("no taskbar rectangle")?;
        // SAFETY: the handle is only read.
        let dpi = unsafe { GetDpiForWindow(tb) }.max(96);
        let finder = Finder::new().map_err(|e| e.message())?;
        let t = Instant::now();
        finder.buttons(tb, &r).map_err(|e| e.message())?;
        let first_ms = t.elapsed().as_millis();
        let t = Instant::now();
        let again = finder.buttons(tb, &r).map_err(|e| e.message())?;
        let again_ms = t.elapsed().as_millis();
        let view = View { up: "300 KB/s".into(), down: "12.4 MB/s".into(), show: "both".into(), light: false, bits: false };
        let m = Bar::new().metrics(&view, dpi);
        let scale = dpi as f64 / 96.0;
        let tray_left = tray_rect(tb).map(|t| t.left).unwrap_or(r.right);
        let mut occupied = vec![Span { lo: tray_left, hi: r.right }];
        occupied.extend(again.spans.iter().copied());
        let layout = Layout { left: r.left, right: r.right, tray_left, occupied, w: m.w, margin: (4.0 * scale).round() as i32, scale };
        let want = layout.preferred_x(None);
        Ok(serde_json::json!({
            "taskbar": [r.left, r.top, r.right, r.bottom],
            "tray_left": tray_left,
            "dpi": dpi,
            "buttons": again.spans.len(),
            "first_ms": first_ms,
            "again_ms": again_ms,
            "spans": again.spans.iter().map(|s| [s.lo, s.hi]).collect::<Vec<_>>(),
            "gaps": layout.gaps().iter().map(|s| [s.lo, s.hi]).collect::<Vec<_>>(),
            "bar": [m.w, m.h],
            "beside_the_clock": want,
            "placed": layout.place(want),
        }))
    })();
    if com {
        // SAFETY: matches the CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    out.unwrap_or_else(|e| serde_json::json!({ "error": e }))
}

/// The real bar, shown for `secs` seconds with this text (kept fresh as the
/// sampler does), then taken away; where it was put.
#[cfg(test)]
pub fn show_for(secs: u64, view: View) -> serde_json::Value {
    // SAFETY: a call on this thread only (the extent reported below is in physical pixels).
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        sync(true, view.clone(), None);
        std::thread::sleep(Duration::from_millis(500));
    }
    let (placed, buttons, measured_for, error) = {
        let sh = shared();
        (sh.placed, sh.measured.as_ref().map(|m| m.spans.len()), sh.measured.as_ref().map(|m| [m.left, m.right]), sh.uia_error.clone())
    };
    sync(false, view, None);
    let t = Instant::now();
    while (shared().thread != 0 || shared().uia) && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let sh = shared();
    serde_json::json!({
        "placed": placed.map(|(r, through)| serde_json::json!({ "rect": r, "clicks_through": through })),
        "buttons": buttons,
        "measured_for": measured_for,
        "taskbar": taskbar().and_then(rect).map(|r| [r.left, r.right]),
        "uia_error": error,
        "ended": sh.thread == 0 && !sh.uia,
    })
}

/// The real bar, then hidden from outside it and then moved from outside it,
/// as Explorer or a display change might do: whether it showed itself again
/// and went back to its place, how long each took, and what it noted.
#[cfg(test)]
pub fn heal_check(view: View) -> serde_json::Value {
    use windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOOWNERZORDER;
    // SAFETY: a call on this thread only (the bar's extent is in physical pixels).
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let _ = take_log();
    // Keeps the bar fed, as the sampler does, until `done` (or 5 s): how long it took.
    let feed_until = |done: &dyn Fn() -> bool| -> Option<u128> {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            sync(true, view.clone(), None);
            if done() {
                return Some(t.elapsed().as_millis());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    };
    let hwnd = || shared().hwnd as HWND;
    // SAFETY: a read of a window handle that may be gone (then it says no).
    let visible = || !hwnd().is_null() && unsafe { IsWindowVisible(hwnd()) } != 0;
    let at_place = || {
        let placed = shared().placed;
        match (placed, rect(hwnd())) {
            (Some(([x, y, w, h], _)), Some(r)) => (r.left, r.top, r.right - r.left, r.bottom - r.top) == (x, y, w, h),
            _ => false,
        }
    };
    let shown_ms = feed_until(&|| visible() && at_place());
    let mut out = serde_json::json!({ "shown_ms": shown_ms });
    let mut moved_to: Option<(i32, i32)> = None;
    if shown_ms.is_some() {
        // Hidden from outside, as ShowOwnedPopups(taskbar, FALSE) would.
        // SAFETY: a window of this process; the bar's thread answers it.
        unsafe { ShowWindow(hwnd(), SW_HIDE) };
        out["shown_again_ms"] = serde_json::json!(feed_until(&|| visible()));
        // Moved from outside, as a display change might.
        let placed = shared().placed;
        if let Some(([x, y, _, _], _)) = placed {
            // SAFETY: as above.
            unsafe { SetWindowPos(hwnd(), null_mut(), x + 40, y - 30, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER) };
            moved_to = Some((x + 40, y - 30));
            out["put_back_ms"] = serde_json::json!(feed_until(&|| at_place()));
        }
    }
    sync(false, view, None);
    let t = Instant::now();
    while shared().thread != 0 && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(50));
    }
    out["ended"] = serde_json::json!(shared().thread == 0);
    // The bar puts itself right within its own turn, often before this thread
    // looks again: what it noticed is in its log.
    let log = take_log();
    out["noticed_hidden"] = serde_json::json!(log.iter().any(|l| l.contains("hidden from outside GitSwitch")));
    out["noticed_moved"] = serde_json::json!(moved_to.is_some_and(|(mx, my)| log.iter().any(|l| l.contains(&format!("moved from outside GitSwitch to {},{}", mx, my)))));
    out["log"] = serde_json::json!(log);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_said_at_once_then_at_most_once_a_minute_with_its_count() {
        let mut t = Tally::default();
        assert_eq!(t.hit(), Some(1), "the first time, at once");
        assert_eq!(t.hit(), None);
        assert_eq!(t.hit(), None);
        // A minute on, the next one carries the count since the last line (itself included).
        t.said = Instant::now().checked_sub(Duration::from_secs(61));
        assert_eq!(t.hit(), Some(3));
        assert_eq!(t.hit(), None);
        assert_eq!(times(1), "");
        assert_eq!(times(3), " (×3 since the last line)");
    }

    #[test]
    fn text_coverage_becomes_premultiplied_colour_and_the_rest_still_takes_the_mouse() {
        assert_eq!(pixel(0, UP, 1), 0x0100_0000, "background: alpha 1, no colour");
        assert_eq!(pixel(0, UP, 0), 0, "letting clicks through: nothing at all");
        assert_eq!(pixel(255, [0x39, 0x87, 0xe5], 1), 0xff39_87e5);
        let half = pixel(128, [0xff, 0x80, 0x00], 1);
        assert_eq!(half >> 24, 128);
        assert_eq!((half >> 16) & 0xff, 128, "premultiplied: never above the alpha");
        assert_eq!((half >> 8) & 0xff, 64);
        assert_eq!(half & 0xff, 0);
    }

    #[test]
    fn the_lines_are_upload_over_download_or_the_one_asked_for() {
        let v = View { up: "300 KB/s".into(), down: "12.4 MB/s".into(), show: "both".into(), light: false, bits: false };
        let l = lines_of(&v);
        assert_eq!(l.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(), vec![("↑", "300 KB/s"), ("↓", "12.4 MB/s")]);
        assert_eq!(lines_of(&View { show: "down".into(), ..v.clone() }).len(), 1);
        assert_eq!(lines_of(&View { show: "up".into(), ..v }).first().map(|x| x.0), Some("↑"));
    }
}
