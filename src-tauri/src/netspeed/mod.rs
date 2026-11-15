//! Net speed: how fast this computer's connection moves data right now — in
//! the Net speed tab, and when asked in the taskbar (Windows: drawn in the
//! tray icon, or a speed bar on the taskbar), the menu bar (macOS) or the
//! top bar (Ubuntu); what was used today and this month; a speed test.
//!
//! One background thread reads the adapters' byte counters: every second (or
//! as set) while anything shows the speed, every ten seconds otherwise (for
//! the usage counts). Nothing is sent anywhere, except by the speed test.

/// Starting at sign-in, to show the speed (only when the person ticks it).
pub mod autostart;
/// The floating speed bar, anywhere on the screen: a small webview window.
pub mod bar;
pub mod dock;
mod format;
mod icon;
mod linux;
mod macos;
pub mod speedtest;
/// The speed bar docked on the Windows taskbar: a native window of its own.
#[cfg(windows)]
mod taskbar;
mod usage;
mod windows;

use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Wifi,
    Ethernet,
    Cellular,
    /// The computer's own hardware of another kind (Bluetooth networking…).
    Other,
    /// Not hardware: a VPN, a virtual machine's or a container's adapter.
    Virtual,
}

/// One network adapter, as the system counts it.
#[derive(Debug, Clone, Serialize)]
pub struct Iface {
    pub id: String,
    pub name: String,
    /// The hardware's name, or the macOS port.
    pub detail: Option<String>,
    pub kind: Kind,
    pub up: bool,
    pub rx: u64,
    pub tx: u64,
    /// Bits per second.
    pub link_bps: Option<u64>,
    /// The one internet traffic goes through now.
    pub in_use: bool,
}

/// The Wi-Fi connection, as far as the system says.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct WifiInfo {
    pub ssid: Option<String>,
    /// 0–100.
    pub signal: Option<u32>,
    pub rssi_dbm: Option<i32>,
    pub rx_mbps: Option<f64>,
    pub tx_mbps: Option<f64>,
    /// Why something isn't shown.
    pub note: Option<String>,
    /// The setting that would show it: "location".
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Show the speed in the taskbar (Windows), menu bar (macOS) or top bar (Linux).
    pub tray: bool,
    /// Windows: "icon" (numbers in the tray icon), "bar" (the speed bar) or "both".
    pub style: String,
    /// macOS / Linux: also the floating speed bar.
    pub bar: bool,
    /// Windows: the speed bar "taskbar" (docked on it, clear of the icons,
    /// moved along it) or "anywhere" (floating, dragged anywhere on the screen).
    pub bar_place: String,
    /// Where the floating speed bar was put.
    pub bar_pos: Option<[i32; 2]>,
    /// Where on the taskbar the speed bar was put (Windows); none: beside the clock.
    pub bar_dock: Option<dock::Dock>,
    /// "both", "down" or "up".
    pub show: String,
    /// "bytes" (MB/s) or "bits" (Mbps).
    pub unit: String,
    /// 1000, 2000 or 5000.
    pub interval_ms: u64,
    /// "auto" (the connection in use), "all" (every connection of this
    /// computer's, added up) or one adapter's id.
    pub source: String,
    /// Start GitSwitch when the person signs in, without a window, to show the
    /// speed. Only while the speed is shown somewhere (see `checked`).
    pub start_at_login: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            tray: false,
            style: "icon".into(),
            bar: false,
            bar_place: "taskbar".into(),
            bar_pos: None,
            bar_dock: None,
            show: "both".into(),
            unit: "bytes".into(),
            interval_ms: 1000,
            source: "auto".into(),
            start_at_login: false,
        }
    }
}

impl Settings {
    /// Unknown values back to their defaults.
    fn checked(mut self) -> Settings {
        let d = Settings::default();
        if !["icon", "bar", "both"].contains(&self.style.as_str()) {
            self.style = d.style;
        }
        if !["taskbar", "anywhere"].contains(&self.bar_place.as_str()) {
            self.bar_place = d.bar_place;
        }
        if !["both", "down", "up"].contains(&self.show.as_str()) {
            self.show = d.show;
        }
        if !["bytes", "bits"].contains(&self.unit.as_str()) {
            self.unit = d.unit;
        }
        if ![1000, 2000, 5000].contains(&self.interval_ms) {
            self.interval_ms = d.interval_ms;
        }
        if self.source.trim().is_empty() {
            self.source = d.source;
        }
        // Starting at sign-in is to show the speed: with nothing shown, it is
        // off, so GitSwitch never starts unseen.
        if !(self.wants_tray_text() || self.wants_bar()) {
            self.start_at_login = false;
        }
        self
    }

    fn bits(&self) -> bool {
        self.unit == "bits"
    }

    /// The speed as text beside the tray icon (macOS, Linux) or in it (Windows).
    pub fn wants_tray_text(&self) -> bool {
        self.tray && (!cfg!(windows) || self.style != "bar")
    }

    pub fn wants_bar(&self) -> bool {
        if cfg!(windows) {
            self.tray && self.style != "icon"
        } else {
            self.bar
        }
    }

    /// The speed bar is the one docked on the Windows taskbar (not the floating one).
    pub fn bar_on_taskbar(&self) -> bool {
        cfg!(windows) && self.bar_place != "anywhere"
    }
}

/// An adapter with its speed now (bytes per second).
#[derive(Debug, Clone, Serialize)]
pub struct IfaceView {
    pub id: String,
    pub name: String,
    pub detail: Option<String>,
    pub kind: Kind,
    pub up: bool,
    pub in_use: bool,
    pub link_bps: Option<u64>,
    pub down: f64,
    pub upload: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageView {
    /// [received, sent] bytes.
    pub today: [u64; 2],
    pub month: [u64; 2],
    pub session: [u64; 2],
    /// When GitSwitch started counting this time (seconds since 1970).
    pub since: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub platform: &'static str,
    /// Bytes per second, of the chosen source.
    pub down: f64,
    pub up: f64,
    /// "Wi-Fi", "All connections", "Not connected"…
    pub source_name: String,
    pub connection: Option<IfaceView>,
    pub wifi: Option<WifiInfo>,
    /// [milliseconds since 1970, down, up], oldest first.
    pub history: Vec<[f64; 3]>,
    pub interfaces: Vec<IfaceView>,
    pub usage: UsageView,
    pub settings: Settings,
    /// Starting at sign-in: the choice, and what the system has.
    pub startup: autostart::StartupView,
}

/// What the speed bar shows.
#[derive(Debug, Clone, Serialize)]
pub struct BarText {
    pub down: String,
    pub up: String,
    pub show: String,
    /// Windows' taskbar is light: dark text.
    pub light: bool,
}

/// The app, for the taskbar bar's double-click (set by `start`). Kept out of
/// `State` on purpose: a test that reads the settings reaches `State`, and the
/// clean-up of a half-made `State` holding an AppHandle would link all of
/// Tauri's window code into the test binary, which has no manifest
/// (0xc0000139 at start, comctl32 v6's SetWindowSubclass and TaskDialogIndirect).
static APP: OnceLock<AppHandle> = OnceLock::new();

struct State {
    settings: Settings,
    prev: Option<(Instant, HashMap<String, [u64; 2]>)>,
    ifaces: Vec<Iface>,
    rates: HashMap<String, [f64; 2]>,
    history: VecDeque<[f64; 3]>,
    session: [u64; 2],
    since: i64,
    ledger: usage::Ledger,
    saved: Instant,
    /// The tab (or anything else asking) wants fresh numbers until then.
    watch_until: Instant,
    wifi: Option<WifiInfo>,
    wifi_at: Option<Instant>,
    wifi_busy: bool,
    /// What the tray shows now, to put back once when turned off.
    showing_text: bool,
    /// Settings changed: show them at once.
    dirty: bool,
    /// The last time the start-up entry was made to match: what failed.
    startup_error: Option<String>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            settings: Settings::default(),
            prev: None,
            ifaces: Vec::new(),
            rates: HashMap::new(),
            history: VecDeque::new(),
            session: [0, 0],
            since: chrono::Utc::now().timestamp(),
            ledger: usage::Ledger::default(),
            saved: Instant::now(),
            watch_until: Instant::now(),
            wifi: None,
            wifi_at: None,
            wifi_busy: false,
            showing_text: false,
            dirty: true,
            startup_error: None,
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

fn data_dir() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("com.gitswitch.app"))
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(name: &str) -> T {
    data_dir().and_then(|d| std::fs::read_to_string(d.join(name)).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_json<T: Serialize>(name: &str, v: &T) {
    let Some(dir) = data_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(text) = serde_json::to_vec_pretty(v) {
        let tmp = dir.join(format!("{}.tmp", name));
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(name));
        }
    }
}

pub(crate) fn read() -> Vec<Iface> {
    if cfg!(windows) {
        windows::read()
    } else if cfg!(target_os = "macos") {
        macos::read()
    } else {
        linux::read()
    }
}

fn wifi_now() -> Option<WifiInfo> {
    if cfg!(windows) {
        windows::wifi()
    } else if cfg!(target_os = "macos") {
        macos::wifi()
    } else {
        linux::wifi()
    }
}

#[cfg(test)]
pub(crate) fn wifi_for_probe() -> Option<WifiInfo> {
    wifi_now()
}

fn boot_time() -> i64 {
    if cfg!(windows) {
        windows::boot_time()
    } else if cfg!(target_os = "macos") {
        macos::boot_time()
    } else {
        linux::boot_time()
    }
}

fn platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// The computer's own connections (not VPNs or virtual adapters): what is counted as used.
fn physical(i: &Iface) -> bool {
    i.kind != Kind::Virtual
}

fn counters(ifaces: &[Iface], only_physical: bool) -> HashMap<String, [u64; 2]> {
    ifaces.iter().filter(|i| !only_physical || physical(i)).map(|i| (i.id.clone(), [i.rx, i.tx])).collect()
}

/// Rates per adapter, and what moved through the computer's own connections.
pub(crate) fn rates_between(prev: &HashMap<String, [u64; 2]>, now: &[Iface], secs: f64) -> (HashMap<String, [f64; 2]>, [u64; 2]) {
    let secs = secs.max(0.05);
    let mut rates = HashMap::new();
    let mut moved = [0u64, 0u64];
    for i in now {
        // New adapters start from now; a counter that went backwards (reset) adds nothing.
        let (d_rx, d_tx) = match prev.get(&i.id) {
            Some(p) => (i.rx.saturating_sub(p[0]), i.tx.saturating_sub(p[1])),
            None => (0, 0),
        };
        // A jump no connection makes (100 GB/s) is a counter that wrapped or was replaced.
        let plausible = |d: u64| if d as f64 / secs > 1e11 { 0 } else { d };
        let (d_rx, d_tx) = (plausible(d_rx), plausible(d_tx));
        rates.insert(i.id.clone(), [d_rx as f64 / secs, d_tx as f64 / secs]);
        if physical(i) {
            moved[0] += d_rx;
            moved[1] += d_tx;
        }
    }
    (rates, moved)
}

/// The chosen source's speed and its name.
pub(crate) fn source_rate(source: &str, ifaces: &[Iface], rates: &HashMap<String, [f64; 2]>) -> (f64, f64, String) {
    let rate = |i: &Iface| rates.get(&i.id).copied().unwrap_or([0.0, 0.0]);
    match source {
        "all" => {
            let (d, u) = ifaces.iter().filter(|i| physical(i) && i.up).fold((0.0, 0.0), |a, i| (a.0 + rate(i)[0], a.1 + rate(i)[1]));
            (d, u, "All connections".into())
        }
        "auto" => match ifaces.iter().find(|i| i.in_use) {
            Some(i) => (rate(i)[0], rate(i)[1], i.name.clone()),
            None => (0.0, 0.0, "Not connected".into()),
        },
        id => match ifaces.iter().find(|i| i.id == id) {
            Some(i) => (rate(i)[0], rate(i)[1], i.name.clone()),
            None => (0.0, 0.0, "Not connected".into()),
        },
    }
}

fn view(i: &Iface, rates: &HashMap<String, [f64; 2]>) -> IfaceView {
    let r = rates.get(&i.id).copied().unwrap_or([0.0, 0.0]);
    IfaceView { id: i.id.clone(), name: i.name.clone(), detail: i.detail.clone(), kind: i.kind, up: i.up, in_use: i.in_use, link_bps: i.link_bps, down: r[0], upload: r[1] }
}

/// Start counting (from `setup`): settings and usage read, what moved while
/// GitSwitch was closed added, the display put up, the thread started.
pub fn start(app: &AppHandle) {
    let ifaces = read();
    {
        let mut st = lock();
        let _ = APP.set(app.clone());
        st.settings = read_json::<Settings>("netspeed.json").checked();
        st.ledger = read_json("netspeed-usage.json");
        let day = today();
        st.ledger.catch_up(boot_time(), &counters(&ifaces, true), &day);
        st.ledger.saved_at = chrono::Utc::now().timestamp();
        write_json("netspeed-usage.json", &st.ledger);
        st.prev = Some((Instant::now(), counters(&ifaces, false)));
        st.ifaces = ifaces;
        st.dirty = true;
    }
    // The start-up entry as chosen: made again if an update's uninstall step
    // took it away, corrected if GitSwitch moved, removed if it was turned off.
    let want = lock().settings.start_at_login;
    let startup = autostart::apply(want);
    lock().startup_error = startup.error;
    crate::tray::set_netspeed_checked(app, lock().settings.tray);
    let app = app.clone();
    let _ = std::thread::Builder::new().name("netspeed".into()).spawn(move || {
        let mut next = Instant::now() + Duration::from_secs(1);
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let (dirty, every) = {
                let st = lock();
                let fast = st.settings.wants_tray_text() || st.settings.wants_bar() || st.watch_until > Instant::now();
                (st.dirty, if fast { Duration::from_millis(st.settings.interval_ms) } else { Duration::from_secs(10) })
            };
            // Coming back to fast (the tab opened): the next tick is soon.
            next = next.min(Instant::now() + every);
            if Instant::now() >= next {
                tick();
                next = Instant::now() + every;
                show(&app);
            } else if dirty {
                show(&app);
            }
        }
    });
}

fn tick() {
    let now = Instant::now();
    let ifaces = read();
    let mut st = lock();
    if let Some((t0, prev)) = st.prev.take() {
        let (rates, moved) = rates_between(&prev, &ifaces, now.duration_since(t0).as_secs_f64());
        st.rates = rates;
        st.session[0] += moved[0];
        st.session[1] += moved[1];
        let day = today();
        st.ledger.add(&day, moved[0], moved[1]);
        st.ledger.counters = counters(&ifaces, true);
        let source = st.settings.source.clone();
        let (d, u, _) = source_rate(&source, &ifaces, &st.rates);
        st.history.push_back([chrono::Utc::now().timestamp_millis() as f64, d, u]);
        while st.history.len() > 120 {
            st.history.pop_front();
        }
    }
    st.prev = Some((now, counters(&ifaces, false)));
    st.ifaces = ifaces;
    if st.saved.elapsed() > Duration::from_secs(60) {
        st.saved = Instant::now();
        st.ledger.saved_at = chrono::Utc::now().timestamp();
        write_json("netspeed-usage.json", &st.ledger);
    }
}

/// Write the usage now (GitSwitch is quitting).
pub fn save() {
    let mut st = lock();
    st.ledger.saved_at = chrono::Utc::now().timestamp();
    write_json("netspeed-usage.json", &st.ledger);
}

/// The text for the menu bar, top bar or tooltip.
fn title(s: &Settings, down: f64, up: f64) -> String {
    let d = format::padded(&format::full(down, s.bits()), 10);
    let u = format::padded(&format::full(up, s.bits()), 10);
    match s.show.as_str() {
        "down" => format!("↓{}", d),
        "up" => format!("↑{}", u),
        _ => format!("↓{}  ↑{}", d, u),
    }
}

#[cfg(windows)]
fn light_taskbar() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("SystemUsesLightTheme"))
        .map(|v| v == 1)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn light_taskbar() -> bool {
    false
}

/// Put the speed where the settings say: beside or in the tray icon, in the
/// speed bar — or take it away again.
fn show(app: &AppHandle) {
    let (s, down, up, name, was_showing) = {
        let mut st = lock();
        st.dirty = false;
        let (d, u, name) = source_rate(&st.settings.source, &st.ifaces, &st.rates);
        (st.settings.clone(), d, u, name, st.showing_text)
    };
    let want = s.wants_tray_text();
    if let Some(tray) = app.tray_by_id(crate::tray::ID) {
        if want {
            let text = title(&s, down, up);
            if cfg!(windows) {
                show_in_icon(app, &tray, &s, down, up);
            } else {
                let _ = tray.set_title(Some(text.as_str()));
            }
            let _ = tray.set_tooltip(Some(format!("GitSwitch — {} · {}", text.split_whitespace().collect::<Vec<_>>().join(" "), name)));
        } else if was_showing {
            let _ = tray.set_title(None::<&str>);
            let _ = tray.set_icon(app.default_window_icon().cloned());
            let _ = tray.set_tooltip(Some("GitSwitch"));
        }
    }
    lock().showing_text = want;
    // One speed bar at a time: docked on the taskbar, or floating anywhere.
    let bar = s.wants_bar();
    #[cfg(windows)]
    taskbar::sync(
        bar && s.bar_on_taskbar(),
        taskbar::View { up: format::full(up, s.bits()), down: format::full(down, s.bits()), show: s.show.clone(), light: light_taskbar(), bits: s.bits() },
        s.bar_dock,
    );
    bar::sync(app, bar && !s.bar_on_taskbar(), s.bar_pos);
}

#[cfg(windows)]
fn show_in_icon(_app: &AppHandle, tray: &tauri::tray::TrayIcon, s: &Settings, down: f64, up: f64) {
    // SAFETY: no arguments.
    let size = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON) }.clamp(16, 64) as u32;
    let (dc, uc) = icon::colours(light_taskbar());
    let lines: Vec<(String, [u8; 4])> = match s.show.as_str() {
        "down" => vec![(format::compact(down, s.bits()), dc)],
        "up" => vec![(format::compact(up, s.bits()), uc)],
        _ => vec![(format::compact(down, s.bits()), dc), (format::compact(up, s.bits()), uc)],
    };
    let img = tauri::image::Image::new_owned(icon::render(&lines, size), size, size);
    let _ = tray.set_icon(Some(img));
}

#[cfg(not(windows))]
fn show_in_icon(_app: &AppHandle, _tray: &tauri::tray::TrayIcon, _s: &Settings, _down: f64, _up: f64) {}

/// The speed is shown outside the window: closing it keeps GitSwitch running.
pub fn keeps_running() -> bool {
    let st = lock();
    st.settings.wants_tray_text() || st.settings.wants_bar()
}

pub fn settings() -> Settings {
    lock().settings.clone()
}

pub fn set_settings(app: &AppHandle, new: Settings) -> Settings {
    let s = set_settings_core(new);
    crate::tray::set_netspeed_checked(app, s.tray);
    s
}

/// `set_settings` but for the tray menu's tick, which needs the app (the probe has none).
pub(crate) fn set_settings_core(new: Settings) -> Settings {
    let new = new.checked();
    let want = {
        let mut st = lock();
        // Where the bar was dragged to stays unless said otherwise; the taskbar
        // spot is the bar's own (a page's copy may be older than the last drag).
        let (keep_pos, keep_dock) = (st.settings.bar_pos, st.settings.bar_dock);
        st.settings = Settings { bar_pos: new.bar_pos.or(keep_pos), bar_dock: keep_dock, ..new };
        st.dirty = true;
        write_json("netspeed.json", &st.settings);
        st.settings.start_at_login
    };
    // Outside the lock: the registry or a file. Nothing changes when the entry already matches.
    let startup = autostart::apply(want);
    lock().startup_error = startup.error;
    settings()
}

/// The tray menu's "Show net speed".
pub fn toggle_tray(app: &AppHandle) {
    let mut s = settings();
    s.tray = !s.tray;
    set_settings(app, s);
}

pub(crate) fn bar_moved(_app: &AppHandle, x: i32, y: i32) {
    let mut st = lock();
    st.settings.bar_pos = Some([x, y]);
    write_json("netspeed.json", &st.settings);
}

/// The speed bar was dragged along the taskbar: remembered (off the bar's thread).
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn bar_docked(dock: dock::Dock) {
    let mut st = lock();
    st.settings.bar_dock = Some(dock);
    write_json("netspeed.json", &st.settings);
}

/// The speed bar back where it starts: beside the clock (docked on the
/// taskbar), or just above it (floating; moved there at once).
pub fn bar_reset(app: &AppHandle) {
    {
        let mut st = lock();
        st.settings.bar_dock = None;
        st.settings.bar_pos = None;
        st.dirty = true;
        write_json("netspeed.json", &st.settings);
    }
    bar::reset(app);
}

/// A double-click on the taskbar's speed bar (off the bar's thread).
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn open_tab() {
    let app = APP.get().cloned();
    if let Some(app) = app {
        show_tab(&app);
    }
}

/// Read only: this computer's taskbar, its buttons, and where the bar would go.
#[cfg(all(test, windows))]
pub(crate) fn taskbar_probe() -> serde_json::Value {
    taskbar::probe()
}

/// The real taskbar bar, shown for a few seconds with this text.
#[cfg(all(test, windows))]
pub(crate) fn taskbar_show_for(secs: u64, up: &str, down: &str, light: bool) -> serde_json::Value {
    taskbar::show_for(secs, taskbar::View { up: up.into(), down: down.into(), show: "both".into(), light, bits: false })
}

/// The probe: the saved settings read in as `start` reads them (each probe
/// call is a process of its own, with no thread and no app).
#[cfg(test)]
pub(crate) fn load_for_probe() {
    lock().settings = read_json::<Settings>("netspeed.json").checked();
}

/// Read only, for the probe: the speed as the app works it out (the
/// connection in use, as "auto" picks it), once a second for `secs` seconds,
/// with the text the tab, the menu bar and the Windows tray icon would show
/// for it, and what moved through that connection meanwhile: to hold against
/// the system's own count and a download of known size.
#[cfg(test)]
pub(crate) fn watch_for_probe(secs: u64) -> serde_json::Value {
    let start = Instant::now();
    let first = read();
    let (mut prev_at, mut prev, mut last) = (start, counters(&first, false), first.clone());
    let mut samples = Vec::new();
    for i in 1..=secs {
        if let Some(wait) = (start + Duration::from_secs(i)).checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let at = Instant::now();
        let now = read();
        let (rates, _) = rates_between(&prev, &now, at.duration_since(prev_at).as_secs_f64());
        let (d, u, source) = source_rate("auto", &now, &rates);
        samples.push(serde_json::json!({
            "t": at.duration_since(start).as_secs_f64(),
            "down": d,
            "up": u,
            "source": source,
            "shown": format::full(d, false),
            "shown_bits": format::full(d, true),
            "icon": format::compact(d, false),
        }));
        (prev_at, prev, last) = (at, counters(&now, false), now);
    }
    let used = first.iter().find(|i| i.in_use);
    let end = used.and_then(|a| last.iter().find(|b| b.id == a.id));
    serde_json::json!({
        "adapter": used.map(|i| serde_json::json!({ "id": i.id, "name": i.name, "detail": i.detail, "kind": i.kind })),
        "seconds": prev_at.duration_since(start).as_secs_f64(),
        "moved": used.zip(end).map(|(a, b)| [b.rx.saturating_sub(a.rx), b.tx.saturating_sub(a.tx)]),
        "counters_at_end": end.map(|b| [b.rx, b.tx]),
        "samples": samples,
    })
}

/// Everything the tab shows. Asking keeps the numbers fresh for a few seconds.
pub fn snapshot() -> Snapshot {
    let mut st = lock();
    st.watch_until = Instant::now() + Duration::from_millis(st.settings.interval_ms) + Duration::from_secs(3);
    // The Wi-Fi's details: asked for again now and then, never waited for.
    let stale = st.wifi_at.is_none_or(|t| t.elapsed() > Duration::from_secs(if cfg!(target_os = "macos") { 30 } else { 10 }));
    let has_wifi = st.ifaces.iter().any(|i| i.kind == Kind::Wifi && i.up);
    if stale && has_wifi && !st.wifi_busy {
        st.wifi_busy = true;
        std::thread::spawn(|| {
            let w = wifi_now();
            let mut st = lock();
            st.wifi = w;
            st.wifi_at = Some(Instant::now());
            st.wifi_busy = false;
        });
    }
    let (down, up, source_name) = source_rate(&st.settings.source, &st.ifaces, &st.rates);
    let connection = st.ifaces.iter().find(|i| i.in_use).map(|i| view(i, &st.rates));
    let wifi = connection.as_ref().filter(|c| c.kind == Kind::Wifi).and(st.wifi.clone());
    let interfaces = st.ifaces.iter().filter(|i| physical(i) || (i.up && i.rx + i.tx > 0)).map(|i| view(i, &st.rates)).collect();
    let (day, month) = (today(), chrono::Local::now().format("%Y-%m").to_string());
    Snapshot {
        platform: platform(),
        down,
        up,
        source_name,
        connection,
        wifi,
        history: st.history.iter().copied().collect(),
        interfaces,
        usage: UsageView { today: st.ledger.day(&day), month: st.ledger.month(&month), session: st.session, since: st.since },
        settings: st.settings.clone(),
        // Read from the system each time (Windows' Startup apps can turn it off
        // meanwhile), with what failed the last time it was set.
        startup: {
            let now = autostart::status(st.settings.start_at_login);
            autostart::StartupView { error: st.startup_error.clone().or(now.error.clone()), ..now }
        },
    }
}

pub fn bar_text() -> BarText {
    let st = lock();
    let (d, u, _) = source_rate(&st.settings.source, &st.ifaces, &st.rates);
    let bits = st.settings.bits();
    BarText { down: format::full(d, bits), up: format::full(u, bits), show: st.settings.show.clone(), light: cfg!(windows) && light_taskbar() }
}

/// The system's settings page that shows what is hidden: the taskbar's
/// icons (Windows) or location access (for the Wi-Fi's name).
pub fn open_os_settings(which: &str) -> Result<(), AppError> {
    let target = match (platform(), which) {
        ("windows", "taskbar") => "ms-settings:taskbar",
        ("windows", "location") => "ms-settings:privacy-location",
        ("windows", "startupapps") => "ms-settings:startupapps",
        ("macos", "location") => "x-apple.systempreferences:com.apple.preference.security?Privacy_LocationServices",
        ("macos", "loginitems") => "x-apple.systempreferences:com.apple.LoginItems-Settings.extension",
        _ => return Err(AppError::Config("There is no such setting here.".into())),
    };
    let program = if cfg!(windows) { "explorer.exe" } else { "open" };
    crate::spawn::command(program).arg(target).spawn().map(|_| ()).map_err(|e| AppError::Command(e.to_string()))
}

/// The speed bar asks for the tab: bring the window forward on it.
pub fn show_tab(app: &AppHandle) {
    crate::tray::show_main(app);
    let _ = app.emit("navigate", "/netspeed");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(id: &str, kind: Kind, up: bool, in_use: bool, rx: u64, tx: u64) -> Iface {
        Iface { id: id.into(), name: id.to_uppercase(), detail: None, kind, up, rx, tx, link_bps: None, in_use }
    }

    #[test]
    fn rates_come_from_the_counters_and_a_reset_is_not_a_spike() {
        let prev: HashMap<String, [u64; 2]> = [("wifi".to_string(), [1_000, 500]), ("vpn".to_string(), [0, 0]), ("eth".to_string(), [9_000_000, 9_000])].into();
        let now = vec![
            iface("wifi", Kind::Wifi, true, true, 3_001_000, 100_500),
            iface("vpn", Kind::Virtual, true, false, 2_000_000, 50_000),
            iface("eth", Kind::Ethernet, true, false, 10, 10),
            iface("new", Kind::Ethernet, true, false, 5_000, 5_000),
        ];
        let (rates, moved) = rates_between(&prev, &now, 2.0);
        assert_eq!(rates["wifi"], [1_500_000.0, 50_000.0]);
        assert_eq!(rates["eth"], [0.0, 0.0], "a counter that went back (reset)");
        assert_eq!(rates["new"], [0.0, 0.0], "a new adapter starts from now");
        assert_eq!(moved, [3_000_000, 100_000], "a VPN's traffic is not counted twice");
        let (huge, _) = rates_between(&[("wifi".to_string(), [0, 0])].into(), &[iface("wifi", Kind::Wifi, true, true, u64::MAX / 2, 0)], 1.0);
        assert_eq!(huge["wifi"][0], 0.0, "not a believable speed");
    }

    #[test]
    fn the_source_is_the_connection_in_use_all_of_them_or_one() {
        let ifaces = vec![iface("wifi", Kind::Wifi, true, true, 0, 0), iface("eth", Kind::Ethernet, true, false, 0, 0), iface("vpn", Kind::Virtual, true, false, 0, 0)];
        let rates: HashMap<String, [f64; 2]> = [("wifi".to_string(), [10.0, 1.0]), ("eth".to_string(), [5.0, 2.0]), ("vpn".to_string(), [100.0, 100.0])].into();
        assert_eq!(source_rate("auto", &ifaces, &rates), (10.0, 1.0, "WIFI".into()));
        assert_eq!(source_rate("all", &ifaces, &rates), (15.0, 3.0, "All connections".into()), "the computer's own connections");
        assert_eq!(source_rate("eth", &ifaces, &rates), (5.0, 2.0, "ETH".into()));
        assert_eq!(source_rate("gone", &ifaces, &rates).2, "Not connected");
        let offline = vec![iface("wifi", Kind::Wifi, false, false, 0, 0)];
        assert_eq!(source_rate("auto", &offline, &rates).2, "Not connected");
    }

    #[test]
    fn settings_keep_to_what_they_can_be() {
        let s = Settings { style: "neon".into(), bar_place: "sideways".into(), show: "x".into(), unit: "furlongs".into(), interval_ms: 7, source: " ".into(), ..Settings::default() }.checked();
        assert_eq!(s, Settings::default());
        assert_eq!(Settings::default().bar_on_taskbar(), cfg!(windows), "Windows: docked on the taskbar unless chosen otherwise");
        assert!(!Settings { bar_place: "anywhere".into(), ..Settings::default() }.bar_on_taskbar(), "anywhere: the floating one");
        let parsed: Settings = serde_json::from_str(r#"{"tray":true}"#).unwrap();
        assert!(parsed.tray && parsed.interval_ms == 1000 && parsed.show == "both", "missing fields take their defaults");
        let on = Settings { tray: true, ..Settings::default() };
        assert!(on.wants_tray_text());
        if cfg!(windows) {
            assert!(!on.wants_bar());
            assert!(Settings { style: "both".into(), ..on.clone() }.wants_bar());
            assert!(!Settings { style: "bar".into(), ..on.clone() }.wants_tray_text());
        } else {
            assert!(!on.wants_bar() && Settings { bar: true, ..Settings::default() }.wants_bar());
        }
        // Starting at sign-in only while the speed is shown somewhere.
        assert!(!Settings { start_at_login: true, ..Settings::default() }.checked().start_at_login, "nothing shown: it doesn't start");
        assert!(Settings { start_at_login: true, tray: true, ..Settings::default() }.checked().start_at_login);
        let only_bar = if cfg!(windows) { Settings { tray: true, style: "bar".into(), ..Settings::default() } } else { Settings { bar: true, ..Settings::default() } };
        assert!(Settings { start_at_login: true, ..only_bar }.checked().start_at_login, "the speed bar alone counts");
        assert!(!serde_json::from_str::<Settings>(r#"{"tray":true}"#).unwrap().start_at_login, "settings from before it existed: off");
        let t = title(&Settings::default(), 12_400_000.0, 300_000.0);
        assert!(t.starts_with('↓') && t.contains("12.4 MB/s") && t.contains("↑") && t.contains("300 KB/s"), "{t}");
        assert!(title(&Settings { show: "up".into(), unit: "bits".into(), ..Settings::default() }, 0.0, 1_000_000.0).contains("8.0 Mbps"));
    }
}
