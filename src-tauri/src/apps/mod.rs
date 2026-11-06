//! The Uninstall tab: the apps installed on this computer, uninstalling one,
//! and then only that app's leftovers, to the Trash.
//!
//! The rules that keep it from offering anything that isn't the removed
//! app's (see matcher.rs for the matching itself):
//!
//! - What identifies the app is recorded before it goes (a session, kept on
//!   disk), so a scan after a restart still knows it.
//! - Leftovers are looked for only directly inside a fixed list of places
//!   per platform — never a search of the disk — and links are never
//!   followed: an item is refused when any folder between its place and it
//!   is a link.
//! - The scan runs only when the app is really gone, and everything is
//!   checked again just before deleting: the app still gone, nothing else
//!   installed claiming the name, the item the very file or folder the scan
//!   saw (device and inode, owner, type; size and time for a file).
//! - The page sends ids, never paths; the backend holds what each id means.
//! - Deleting moves to the Trash. What needs the administrator is listed
//!   with a command, never deleted here.

pub mod matcher;
mod linux;
mod macos;
mod windows;

use crate::error::AppError;
use matcher::{Allow, Confidence, Evidence, Match, Others};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One installed app (or, on Ubuntu's "All packages" tab, one package).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub location: Option<String>,
    /// app | app-store | homebrew | msi | exe | store | deb | snap | flatpak | appimage | package
    pub source: String,
    pub size: Option<u64>,
    /// Seconds since 1970, when known.
    pub date: Option<i64>,
    pub notes: Vec<String>,
    /// Why GitSwitch won't remove it.
    pub blocked: Option<String>,
    /// GitSwitch itself: it has its own uninstall.
    pub is_self: bool,
    /// A system app, and why: it came with Windows or the PC, runs its
    /// hardware, or other apps use it ("Your apps" holds the rest).
    #[serde(default)]
    pub system: Option<String>,
}

/// What the backend needs to remove it — never sent by the page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Record {
    MacBundle { bundle: PathBuf, bundle_id: String, cask: Option<String>, vendor_uninstaller: Option<PathBuf>, app_store: bool },
    WinEntry {
        view: String,
        key: String,
        uninstall: String,
        msi: Option<String>,
        install_dir: Option<PathBuf>,
        icon: Option<String>,
        /// Every folder it is installed in, recorded before it goes (its
        /// InstallLocation, or where its program, uninstaller and shortcuts point).
        #[serde(default)]
        dirs: Vec<PathBuf>,
        /// Its programs' names (`DiskDrill` for DiskDrill.exe).
        #[serde(default)]
        exes: Vec<String>,
        /// What starts it when Windows starts: Run values pointing into its folders.
        #[serde(default)]
        startup: Vec<Startup>,
        /// Installed by a setup bundle (WiX Burn): the bundle registered more than this entry.
        #[serde(default)]
        bundle: bool,
    },
    WinAppx { full_name: String, family: String, name: String, install_dir: Option<PathBuf> },
    Deb { package: String, desktop: Vec<PathBuf> },
    Snap { name: String, classic: bool },
    Flatpak { id: String, installation: String, branch: Option<String> },
    AppImage { file: PathBuf, desktop: Vec<PathBuf> },
}

/// A Run value that starts a program when Windows starts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Startup {
    /// HKCU | HKLM64 | HKLM32
    pub view: String,
    pub name: String,
    pub data: String,
}

/// A platform's listing: the apps with what removing each needs, and notes.
pub(crate) type Listing = (Vec<(InstalledApp, Record)>, Vec<String>);

#[derive(Debug, Clone, Serialize)]
pub struct AppList {
    pub platform: String,
    pub apps: Vec<InstalledApp>,
    pub notes: Vec<String>,
}

/// One uninstall, from the confirm step to the cleaned leftovers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub app: InstalledApp,
    pub record: Record,
    pub evidence: Evidence,
    pub facts: Vec<String>,
    pub started: i64,
    /// prepared | uninstalling | gone | still-installed | failed | cleaned
    pub state: String,
    pub message: Option<String>,
    /// Ubuntu: exactly the packages the person approved.
    pub packages: Vec<String>,
    /// Windows: the uninstaller's process id and creation time while it runs.
    pub process: Option<(u32, u64)>,
}

/// The confirm step: what will happen, and what is known about the app.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Prepared {
    pub session: String,
    pub app: InstalledApp,
    /// One sentence: how it is removed.
    pub method: String,
    /// The exact command that runs, when one does.
    pub command: Option<String>,
    pub facts: Vec<String>,
    /// Ubuntu: the other packages apt would remove with it.
    pub also_removed: Vec<String>,
    pub blocked: Option<String>,
    pub running: bool,
    pub warnings: Vec<String>,
    /// macOS: the app's own uninstaller, to use instead.
    pub vendor_uninstaller: Option<String>,
    /// Windows: deleting its files directly instead of running its uninstaller.
    pub force: Option<ForcePlan>,
    /// Windows: why its files can't be deleted directly.
    pub force_unavailable: Option<String>,
}

/// Windows: what deleting a program's files directly would remove.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ForcePlan {
    pub folders: Vec<ForceFolder>,
    /// Its entry in Installed apps.
    pub entry: String,
    pub entry_admin: bool,
    /// Its start-up entries, as "name (where)".
    pub startup: Vec<String>,
    /// Windows asks for permission for some of it.
    pub admin: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ForceFolder {
    pub path: String,
    pub size: Option<u64>,
    pub size_complete: bool,
    pub admin: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Progress {
    /// uninstalling | gone | still-installed | failed
    pub state: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Leftover {
    pub id: String,
    pub path: String,
    /// The place it is in, as the page groups them.
    pub place: String,
    /// file | folder | registry | launch-agent | package-settings | link
    pub kind: String,
    pub size: Option<u64>,
    pub size_complete: bool,
    pub confidence: Confidence,
    pub reason: String,
    /// Needs the administrator.
    pub admin: bool,
    /// What the administrator would run, where GitSwitch can't do it itself.
    pub command: Option<String>,
    /// GitSwitch deletes it after Windows' permission prompt (Windows).
    #[serde(default)]
    pub elevate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeftoverScan {
    pub scan: String,
    pub items: Vec<Leftover>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deleted {
    pub id: String,
    /// trashed | removed | gone | changed | refused | needs-admin | protected | failed
    pub outcome: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session: String,
    pub name: String,
    pub state: String,
    pub started: i64,
}

/// What a leftover is done with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    /// To the Trash.
    Trash,
    /// macOS: `launchctl bootout` the label, then to the Trash.
    LaunchAgent { label: String },
    /// Windows: a key under HKCU, exported first.
    #[cfg_attr(not(windows), allow(dead_code))]
    Registry { key: String },
    /// Ubuntu: `dpkg --purge` of a package in "rc" state.
    Purge { package: String },
    /// A command link that pointed into the removed app: still that link, to the Trash.
    Link { target: PathBuf },
    /// Windows: a Run value that started it, exported first.
    #[cfg_attr(not(windows), allow(dead_code))]
    RunValue { view: String, name: String, data: String },
    /// Listed only.
    None,
}

/// A leftover a platform's scan found.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub path: PathBuf,
    /// The place it was found in: nothing between it and the item may be a link.
    pub root: PathBuf,
    pub place: String,
    pub kind: &'static str,
    pub m: Match,
    pub admin: bool,
    pub command: Option<String>,
    pub action: Action,
    /// Its size is worth counting (not inside macOS's protected containers).
    pub sized: bool,
    /// The name it matched by and how — matched again before deleting.
    pub entry: Option<(String, Allow)>,
}

/// A place leftovers are looked for in, directly inside.
#[derive(Debug, Clone)]
pub(crate) struct Place {
    pub path: PathBuf,
    pub label: String,
    pub allow: Allow,
    pub admin: bool,
    pub sized: bool,
    /// Also `<vendor>/<name>` (never the vendor folder itself).
    pub vendor_depth: bool,
    /// The home folder: only `.name` entries, matched without their dot.
    pub dot_names: bool,
    /// Windows: a publisher's folder is also its name without the legal
    /// form, and the app's exact name inside it is sure (two things agree).
    pub vendor_sure: bool,
}

impl Place {
    pub fn new(path: PathBuf, label: &str, allow: Allow) -> Place {
        Place { path, label: label.to_string(), allow, admin: false, sized: true, vendor_depth: false, dot_names: false, vendor_sure: false }
    }
    pub fn sure(mut self) -> Place {
        self.vendor_sure = true;
        self
    }
    pub fn admin(mut self) -> Place {
        self.admin = true;
        self
    }
    pub fn no_size(mut self) -> Place {
        self.sized = false;
        self
    }
    pub fn vendor(mut self) -> Place {
        self.vendor_depth = true;
        self
    }
    pub fn dots(mut self) -> Place {
        self.dot_names = true;
        self
    }
}

// ---------------------------------------------------------------------------
// Where things are: the real system, or (debug builds) a sandbox.

/// A system path — re-rooted under GITSWITCH_APPS_ROOT in debug builds, so
/// the suites never read or touch the real /Applications, /usr/share or
/// registry-free stand-ins.
pub(crate) fn sys(path: &str) -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(root) = std::env::var_os("GITSWITCH_APPS_ROOT").filter(|r| !r.is_empty()) {
        let rel = path.trim_start_matches(['/', '\\']);
        let rel = rel.split_once(":\\").map(|(_, r)| r).unwrap_or(rel);
        return PathBuf::from(root).join(rel);
    }
    PathBuf::from(path)
}

/// Debug builds: the suites run without the real system's package
/// managers, Spotlight and process lists.
pub(crate) fn sandboxed() -> bool {
    cfg!(debug_assertions) && std::env::var_os("GITSWITCH_APPS_ROOT").is_some_and(|r| !r.is_empty())
}

/// A path under GITSWITCH_APPS_ROOT back as the system names it (what
/// dpkg's lists say), or the path itself.
pub(crate) fn unsys(path: &Path) -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(root) = std::env::var_os("GITSWITCH_APPS_ROOT").filter(|r| !r.is_empty()) {
        if let Ok(rel) = path.strip_prefix(PathBuf::from(root)) {
            return Path::new("/").join(rel);
        }
    }
    path.to_path_buf()
}

pub(crate) fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

fn data_dir() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("com.gitswitch.app"))
}

pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ---------------------------------------------------------------------------
// What the page's ids mean.

struct State {
    list: HashMap<String, (InstalledApp, Record)>,
    sessions: HashMap<String, Session>,
    scans: HashMap<String, Scan>,
}

struct Scan {
    session: String,
    items: HashMap<String, ScanItem>,
}

#[derive(Debug, Clone)]
struct ScanItem {
    path: PathBuf,
    root: PathBuf,
    ident: Option<Ident>,
    action: Action,
    admin: bool,
    /// Done after Windows' permission prompt.
    elevate: bool,
    entry: Option<(String, Allow)>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { list: HashMap::new(), sessions: HashMap::new(), scans: HashMap::new() }))
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

fn sessions_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("uninstall-sessions"))
}

fn save_session(s: &Session) {
    if let Some(dir) = sessions_dir() {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_vec_pretty(s) {
            let tmp = dir.join(format!("{}.json.tmp", s.id));
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, dir.join(format!("{}.json", s.id)));
            }
        }
    }
    lock().sessions.insert(s.id.clone(), s.clone());
}

fn load_session(id: &str) -> Result<Session, AppError> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || id.is_empty() {
        return Err(AppError::Config("Not an uninstall session.".into()));
    }
    if let Some(s) = lock().sessions.get(id) {
        return Ok(s.clone());
    }
    let path = sessions_dir().map(|d| d.join(format!("{}.json", id))).ok_or_else(|| AppError::Config("No data folder.".into()))?;
    let text = std::fs::read_to_string(&path).map_err(|_| AppError::Config("This uninstall is no longer known — list the apps again.".into()))?;
    let s: Session = serde_json::from_str(&text).map_err(|e| AppError::Config(format!("The uninstall record is damaged: {}", e)))?;
    lock().sessions.insert(s.id.clone(), s.clone());
    Ok(s)
}

/// Every uninstall and every deletion, one JSON line each.
pub(crate) fn log(event: serde_json::Value) {
    use std::io::Write;
    if let Some(dir) = data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("uninstall-log.jsonl")) {
            let mut line = event;
            line["at"] = serde_json::json!(now());
            let _ = writeln!(f, "{}", line);
        }
    }
}

// ---------------------------------------------------------------------------
// The file or folder the scan saw, and nothing else.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Ident {
    dev: u64,
    ino: u64,
    owner: u32,
    dir: bool,
    /// For a file: size and modification time. A folder's time changes with
    /// what is inside, so it is not compared.
    len: Option<u64>,
    #[serde(with = "i128_text")]
    mtime: Option<i128>,
}

/// An i128 travels as text: JSON numbers stop at 64 bits.
mod i128_text {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<i128>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(n) => s.serialize_some(&n.to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i128>, D::Error> {
        let t: Option<String> = Option::deserialize(d)?;
        t.map(|x| x.parse().map_err(serde::de::Error::custom)).transpose()
    }
}

/// None for a link (or junction), or what can't be read.
pub(crate) fn ident(path: &Path) -> Option<Ident> {
    let md = std::fs::symlink_metadata(path).ok()?;
    if is_link(&md) {
        return None;
    }
    let dir = md.is_dir();
    let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i128);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(Ident { dev: md.dev(), ino: md.ino(), owner: md.uid(), dir, len: (!dir).then_some(md.len()), mtime: if dir { None } else { mtime } })
    }
    #[cfg(windows)]
    {
        let (dev, ino) = windows::file_id(path)?;
        Some(Ident { dev, ino, owner: 0, dir, len: (!dir).then_some(md.len()), mtime: if dir { None } else { mtime } })
    }
}

pub(crate) fn is_link(md: &std::fs::Metadata) -> bool {
    if md.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// The place itself is a link, or any folder between it and the item is.
pub(crate) fn link_on_the_way(root: &Path, item: &Path) -> bool {
    let Ok(rel) = item.strip_prefix(root) else { return true };
    if std::fs::symlink_metadata(root).map(|m| is_link(&m)).unwrap_or(true) {
        return true;
    }
    let mut p = root.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    for c in parts.iter().take(parts.len().saturating_sub(1)) {
        match c {
            std::path::Component::Normal(n) => p.push(n),
            _ => return true,
        }
        if std::fs::symlink_metadata(&p).map(|m| is_link(&m)).unwrap_or(true) {
            return true;
        }
    }
    false
}

/// Bytes in a file or folder: links not followed, one filesystem, within a
/// time budget (false when it ran out).
pub(crate) fn size_of(path: &Path, budget: Duration) -> (u64, bool) {
    let deadline = Instant::now() + budget;
    let Ok(top) = std::fs::symlink_metadata(path) else { return (0, true) };
    if !top.is_dir() {
        return (top.len(), true);
    }
    #[cfg(unix)]
    let dev = {
        use std::os::unix::fs::MetadataExt;
        top.dev()
    };
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    let mut n = 0u32;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            n += 1;
            if n.is_multiple_of(512) && Instant::now() > deadline {
                return (total, false);
            }
            let Ok(md) = e.path().symlink_metadata() else { continue };
            if is_link(&md) {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if md.dev() != dev {
                    continue;
                }
            }
            if md.is_dir() {
                stack.push(e.path());
            } else {
                total += md.len();
            }
        }
    }
    (total, true)
}

// ---------------------------------------------------------------------------
// The Trash.

#[derive(Debug, PartialEq)]
pub(crate) enum TrashOutcome {
    Trashed,
    Gone,
    NeedsAdmin(String),
    Protected(String),
    Failed(String),
}

#[cfg(unix)]
pub(crate) fn writable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
    // SAFETY: a valid NUL-terminated path; access() only reads it.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Move a file or folder to the Trash (the Recycle Bin on Windows).
pub(crate) fn to_trash(path: &Path) -> TrashOutcome {
    let Ok(md) = std::fs::symlink_metadata(path) else { return TrashOutcome::Gone };
    // Debug builds: the suites' "Trash" is a sandbox folder.
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("GITSWITCH_TRASH_DIR").filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut dest = dir.join(&name);
        let mut i = 1;
        while dest.exists() {
            i += 1;
            dest = dir.join(format!("{} {}", name, i));
        }
        return match std::fs::rename(path, &dest) {
            Ok(()) => TrashOutcome::Trashed,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => TrashOutcome::NeedsAdmin(e.to_string()),
            Err(e) => TrashOutcome::Failed(e.to_string()),
        };
    }
    #[cfg(unix)]
    {
        let parent = path.parent().unwrap_or(Path::new("/"));
        // Moving needs the folder it is in writable — and a folder itself
        // writable too, for its ".." entry.
        if !writable(parent) || (md.is_dir() && !writable(path)) {
            return TrashOutcome::NeedsAdmin("It belongs to another user (the administrator).".into());
        }
    }
    #[cfg(not(unix))]
    let _ = md;
    #[cfg(target_os = "macos")]
    let result = {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::default();
        // Finder is the crate's default: it asks for permission to control Finder.
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path)
    };
    #[cfg(not(target_os = "macos"))]
    let result = trash::delete(path);
    match result {
        Ok(()) => TrashOutcome::Trashed,
        Err(e) => {
            let text = format!("{:?}", e);
            if std::fs::symlink_metadata(path).is_err() {
                // Some implementations report an error after the move.
                return TrashOutcome::Trashed;
            }
            let lower = text.to_lowercase();
            if lower.contains("not permitted") || lower.contains("code=513") {
                TrashOutcome::Protected(text)
            } else if lower.contains("permission denied") || lower.contains("access is denied") || lower.contains("code: 13") || lower.contains("code: 5,") || lower.contains("-2147024891") || lower.contains("0x80070005") {
                TrashOutcome::NeedsAdmin(text)
            } else {
                TrashOutcome::Failed(text)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Looking in the places.

/// Every entry directly inside `places` the removed app owns, by the
/// matcher's rules. Links are skipped; a place that is itself a link is not
/// searched (a note says so).
pub(crate) fn scan_places(places: &[Place], ev: &Evidence, others: &Others) -> (Vec<Candidate>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for place in places {
        let Ok(md) = std::fs::symlink_metadata(&place.path) else { continue };
        if is_link(&md) {
            notes.push(format!("{} is a link to another folder, so it was not searched.", place.path.display()));
            continue;
        }
        if !md.is_dir() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&place.path) else {
            notes.push(format!("{} could not be read.", place.path.display()));
            continue;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if let Ok(md) = e.path().symlink_metadata() {
                if is_link(&md) {
                    continue;
                }
            }
            let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
            let key = if place.dot_names {
                match name.strip_prefix('.') {
                    Some(k) if !k.is_empty() => k.to_string(),
                    _ => continue,
                }
            } else {
                name.clone()
            };
            if let Some(m) = matcher::match_entry(&key, ev, others, place.allow) {
                // Where the administrator is needed, only the sure ones.
                if place.admin && m.confidence != Confidence::High {
                    continue;
                }
                out.push(Candidate {
                    path: e.path(),
                    root: place.path.clone(),
                    place: place.label.clone(),
                    kind: if ft.is_dir() { "folder" } else { "file" },
                    m,
                    admin: place.admin,
                    command: None,
                    action: if place.admin { Action::None } else { Action::Trash },
                    sized: place.sized,
                    entry: Some((key, place.allow)),
                });
                continue;
            }
            // `<vendor>/<name>`: the vendor folder is never the app's, a
            // folder named exactly like the app inside it may be.
            let vendor = ev.vendor.as_deref().map(|v| matcher::norm(v, others.fold_case()));
            let vendor_folder = if place.vendor_sure { matcher::vendor_matches(&key, ev.vendor.as_deref()) } else { vendor.is_some_and(|v| v == matcher::norm(&key, others.fold_case())) };
            // Sure only under a publisher's own name: "Microsoft" or "Google" hold many apps' folders.
            let sure = place.vendor_sure && matcher::usable_name(&key);
            if place.vendor_depth && ft.is_dir() && vendor_folder {
                let names_only = Evidence { ids: vec![], names: ev.names.clone(), groups: vec![], vendor: None };
                let Ok(inner) = std::fs::read_dir(e.path()) else { continue };
                for c in inner.flatten() {
                    let Ok(cft) = c.file_type() else { continue };
                    if cft.is_symlink() || !cft.is_dir() {
                        continue;
                    }
                    let Some(cname) = c.file_name().to_str().map(str::to_string) else { continue };
                    if let Some(m) = matcher::match_entry(&cname, &names_only, others, Allow::IdsAndNames) {
                        if place.admin {
                            continue;
                        }
                        out.push(Candidate {
                            path: c.path(),
                            root: place.path.clone(),
                            place: place.label.clone(),
                            kind: "folder",
                            m: Match { confidence: if sure { Confidence::High } else { m.confidence }, reason: format!("in {}: {}", name, m.reason) },
                            admin: false,
                            command: None,
                            action: Action::Trash,
                            sized: place.sized,
                            entry: None,
                        });
                    }
                }
            }
        }
    }
    (out, notes)
}

// ---------------------------------------------------------------------------
// The commands.

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// The installed apps — `kind` "packages" for Ubuntu's every-package list.
pub async fn list(kind: String) -> Result<AppList, AppError> {
    let (rows, notes) = tokio::task::spawn_blocking(move || -> Result<Listing, AppError> {
        if cfg!(target_os = "macos") {
            macos::list()
        } else if cfg!(windows) {
            windows::list()
        } else {
            linux::list(&kind)
        }
    })
    .await
    .map_err(|e| AppError::Command(e.to_string()))??;
    let mut st = lock();
    st.list.clear();
    let mut apps = Vec::with_capacity(rows.len());
    for (app, rec) in rows {
        st.list.insert(app.id.clone(), (app.clone(), rec));
        apps.push(app);
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    Ok(AppList { platform: platform().into(), apps, notes })
}

/// How big an app is, counted within a few seconds.
pub async fn size(id: String) -> Result<(u64, bool), AppError> {
    let path = {
        let st = lock();
        let (_, rec) = st.list.get(&id).ok_or_else(|| AppError::Config("List the apps again.".into()))?;
        match rec {
            Record::MacBundle { bundle, .. } => Some(bundle.clone()),
            Record::AppImage { file, .. } => Some(file.clone()),
            Record::WinEntry { install_dir, .. } => install_dir.clone().filter(|d| windows::safe_install_dir(d)),
            _ => None,
        }
    };
    let Some(path) = path else { return Ok((0, false)) };
    tokio::task::spawn_blocking(move || size_of(&path, Duration::from_secs(4))).await.map_err(|e| AppError::Command(e.to_string()))
}

/// A platform's confirm step: what to show, what identifies the app, the
/// packages approved (Ubuntu).
type Prepped = (Prepared, Evidence, Vec<String>);

/// The confirm step: record what identifies the app, say how it will go.
pub async fn prepare(id: String) -> Result<Prepared, AppError> {
    if !lock().list.contains_key(&id) {
        // The list is read again (after a restart of the backend, say).
        list(if id.starts_with("pkg:") { "packages".into() } else { "apps".into() }).await?;
    }
    let (app, rec) = lock().list.get(&id).cloned().ok_or_else(|| AppError::Config("That app is no longer installed — list the apps again.".into()))?;
    if app.is_self {
        return Err(AppError::Config("GitSwitch removes itself from Settings › Uninstall.".into()));
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    let (rec2, app2) = (rec.clone(), app.clone());
    let (mut p, rec) = tokio::task::spawn_blocking(move || -> Result<(Prepped, Record), AppError> {
        if cfg!(target_os = "macos") {
            macos::prepare(&app2, &rec2).map(|p| (p, rec2))
        } else if cfg!(windows) {
            // What only the installed program can tell: its folders, programs, start-up entries.
            let rec2 = windows::enrich(&app2, &rec2);
            windows::prepare(&app2, &rec2).map(|p| (p, rec2))
        } else {
            linux::prepare(&app2, &rec2).map(|p| (p, rec2))
        }
    })
    .await
    .map_err(|e| AppError::Command(e.to_string()))??;
    if p.0.blocked.is_none() {
        p.0.blocked = app.blocked.clone();
    }
    // A system app can go, but something may need it.
    if let Some(why) = &app.system {
        let os = if cfg!(target_os = "macos") { "macOS" } else { "Windows" };
        p.0.warnings.push(format!("{}: {}, your hardware or other apps may need it.", why, os));
    }
    let session = Session {
        id: session_id.clone(),
        app: app.clone(),
        record: rec,
        evidence: p.1,
        facts: p.0.facts.clone(),
        started: now(),
        state: "prepared".into(),
        message: None,
        packages: p.2,
        process: None,
    };
    save_session(&session);
    p.0.session = session_id;
    p.0.app = app;
    Ok(p.0)
}

/// Uninstall the app of a prepared session. Returns at once on Windows and
/// for packages; `status` follows it.
pub async fn uninstall(session: String) -> Result<Progress, AppError> {
    let mut s = load_session(&session)?;
    if s.state != "prepared" && s.state != "failed" && s.state != "still-installed" {
        return Err(AppError::Config(format!("This uninstall is already {}.", s.state)));
    }
    s.state = "uninstalling".into();
    s.message = None;
    save_session(&s);
    log(serde_json::json!({ "event": "uninstall", "app": s.app.name, "record": s.record }));
    let result = Box::pin(async {
        if cfg!(target_os = "macos") {
            macos::uninstall(&mut s).await
        } else if cfg!(windows) {
            windows::uninstall(&mut s).await
        } else {
            linux::uninstall(&mut s).await
        }
    })
    .await;
    if let Err(e) = result {
        s.state = "failed".into();
        s.message = Some(e.to_string());
    }
    save_session(&s);
    Ok(Progress { state: s.state.clone(), message: s.message.clone() })
}

/// Where an uninstall is: still running, gone, or still installed.
pub async fn status(session: String) -> Result<Progress, AppError> {
    let mut s = load_session(&session)?;
    if s.state == "uninstalling" || s.state == "still-installed" || s.state == "gone" {
        let checked = Box::pin(async {
            if cfg!(target_os = "macos") {
                macos::status(&mut s).await
            } else if cfg!(windows) {
                windows::status(&mut s).await
            } else {
                linux::status(&mut s).await
            }
        })
        .await;
        if let Err(e) = checked {
            s.message = Some(e.to_string());
        }
        save_session(&s);
    }
    Ok(Progress { state: s.state.clone(), message: s.message.clone() })
}

/// The person says the uninstaller has finished: check whether the app is gone.
pub async fn check_gone(session: String) -> Result<Progress, AppError> {
    let mut s = load_session(&session)?;
    let s2 = s.clone();
    let gone = tokio::task::spawn_blocking(move || platform_gone(&s2)).await.map_err(|e| AppError::Command(e.to_string()))?;
    match gone {
        Ok(()) => {
            s.state = "gone".into();
            s.message = None;
        }
        Err(why) => {
            s.state = "still-installed".into();
            s.message = Some(why);
        }
    }
    s.process = None;
    save_session(&s);
    Ok(Progress { state: s.state.clone(), message: s.message.clone() })
}

fn platform_gone(s: &Session) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        macos::gone(s)
    } else if cfg!(windows) {
        windows::gone(s)
    } else {
        linux::gone(s)
    }
}

fn platform_others(s: &Session) -> Others {
    if cfg!(target_os = "macos") {
        macos::others(s)
    } else if cfg!(windows) {
        windows::others(s)
    } else {
        linux::others(s)
    }
}

/// The app's leftovers — only once it is gone.
pub async fn scan(session: String, registry: bool) -> Result<LeftoverScan, AppError> {
    let s = load_session(&session)?;
    tokio::task::spawn_blocking(move || scan_blocking(s, registry)).await.map_err(|e| AppError::Command(e.to_string()))?
}

fn scan_blocking(mut s: Session, registry: bool) -> Result<LeftoverScan, AppError> {
    if let Err(why) = platform_gone(&s) {
        s.state = "still-installed".into();
        s.message = Some(why.clone());
        save_session(&s);
        return Err(AppError::Config(format!("{} is still installed, so its files are still its own: {}", s.app.name, why)));
    }
    if s.state != "cleaned" {
        s.state = "gone".into();
        save_session(&s);
    }
    let others = platform_others(&s);
    let (mut cands, mut notes) = if cfg!(target_os = "macos") {
        macos::leftovers(&s, &others)
    } else if cfg!(windows) {
        windows::leftovers(&s, &others, registry)
    } else {
        linux::leftovers(&s, &others)
    };
    // One item once, the surer reason kept.
    cands.sort_by(|a, b| a.path.cmp(&b.path).then(b.m.confidence.cmp(&a.m.confidence)));
    cands.dedup_by(|a, b| a.path == b.path);
    // Nothing inside something else that is offered.
    let paths: Vec<PathBuf> = cands.iter().filter(|c| file_backed(c.kind)).map(|c| c.path.clone()).collect();
    cands.retain(|c| !file_backed(c.kind) || !paths.iter().any(|p| p != &c.path && c.path.starts_with(p)));
    let scan_id = uuid::Uuid::new_v4().to_string();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut items = Vec::new();
    let mut held = HashMap::new();
    for (i, c) in cands.into_iter().enumerate() {
        let file_backed = file_backed(c.kind);
        if file_backed && link_on_the_way(&c.root, &c.path) {
            notes.push(format!("{} was skipped: a folder on the way to it is a link.", c.path.display()));
            continue;
        }
        let id = format!("l{}", i);
        let (size, complete) = if file_backed && c.sized && Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(1500));
            let (b, done) = size_of(&c.path, left);
            (Some(b), done)
        } else {
            (None, false)
        };
        // Windows: GitSwitch does what needs the administrator after its prompt.
        let elevate = c.admin && cfg!(windows) && c.action != Action::None;
        held.insert(
            id.clone(),
            ScanItem { path: c.path.clone(), root: c.root.clone(), ident: if file_backed { ident(&c.path) } else { None }, action: c.action.clone(), admin: c.admin, elevate, entry: c.entry.clone() },
        );
        items.push(Leftover {
            id,
            path: c.path.to_string_lossy().to_string(),
            place: c.place,
            kind: c.kind.to_string(),
            size,
            size_complete: complete,
            confidence: c.m.confidence,
            reason: c.m.reason,
            admin: c.admin,
            command: c.command,
            elevate,
        });
    }
    let mut st = lock();
    st.scans.retain(|_, sc| sc.session != s.id);
    st.scans.insert(scan_id.clone(), Scan { session: s.id.clone(), items: held });
    Ok(LeftoverScan { scan: scan_id, items, notes })
}

/// Move the leftovers the person ticked to the Trash — each one checked
/// again first; on Windows, what needs the administrator after one prompt.
pub async fn delete(scan: String, ids: Vec<String>) -> Result<Vec<Deleted>, AppError> {
    let (session, mut out, batch) = tokio::task::spawn_blocking(move || delete_blocking(scan, ids)).await.map_err(|e| AppError::Command(e.to_string()))??;
    if !batch.is_empty() {
        let app = load_session(&session).map(|s| s.app.name).unwrap_or_default();
        match Box::pin(elevated_run(batch.clone(), false)).await {
            Ok(done) => {
                for d in done {
                    log(serde_json::json!({ "event": "leftover", "app": app, "id": d.id, "outcome": d.outcome, "message": d.message, "elevated": true }));
                    out.push(d);
                }
            }
            Err(why) => {
                for b in batch {
                    out.push(Deleted { id: b.id, outcome: "needs-admin".into(), message: why.clone() });
                }
            }
        }
    }
    if out.iter().all(|d| d.outcome == "trashed" || d.outcome == "removed" || d.outcome == "gone") {
        if let Ok(mut s) = load_session(&session) {
            s.state = "cleaned".into();
            save_session(&s);
        }
    }
    Ok(out)
}

/// What a leftover came to: done here, or to do after the permission prompt.
enum Step {
    Done(&'static str, String),
    Elevate(JobWhat),
}

/// (session, what was done here, what needs the administrator)
type Phase = (String, Vec<Deleted>, Vec<JobItem>);

fn delete_blocking(scan: String, ids: Vec<String>) -> Result<Phase, AppError> {
    let (session, items) = {
        let st = lock();
        let sc = st.scans.get(&scan).ok_or_else(|| AppError::Config("Scan for leftovers again — this list is no longer current.".into()))?;
        let mut picked = Vec::new();
        for id in &ids {
            picked.push((id.clone(), sc.items.get(id).cloned()));
        }
        (sc.session.clone(), picked)
    };
    let s = load_session(&session)?;
    // Reinstalled since the scan? Then nothing of it is a leftover.
    if let Err(why) = platform_gone(&s) {
        return Err(AppError::Config(format!("{} is installed again, so nothing was deleted: {}", s.app.name, why)));
    }
    let others = platform_others(&s);
    let mut out = Vec::new();
    let mut batch = Vec::new();
    for (id, item) in items {
        let Some(item) = item else {
            out.push(Deleted { id, outcome: "refused".into(), message: "Not part of this scan.".into() });
            continue;
        };
        match delete_one(&s, &others, &item) {
            Step::Done(o, m) => {
                log(serde_json::json!({ "event": "leftover", "app": s.app.name, "path": item.path, "outcome": o, "message": m }));
                out.push(Deleted { id, outcome: o.into(), message: m });
            }
            Step::Elevate(what) => batch.push(JobItem { id, what }),
        }
    }
    Ok((session, out, batch))
}

fn delete_one(s: &Session, others: &Others, item: &ScanItem) -> Step {
    use Step::Done;
    if item.action == Action::None || (item.admin && !item.elevate) {
        return Done("needs-admin", "This one needs the administrator; GitSwitch doesn't delete it.".into());
    }
    if let Some((entry, allow)) = &item.entry {
        if matcher::match_entry(entry, &s.evidence, others, *allow).is_none() {
            return Done("refused", "Something installed now claims this name, so it was kept.".into());
        }
    }
    match &item.action {
        Action::Registry { key } => {
            let (o, m) = windows::delete_registry(key);
            return Done(o, m);
        }
        Action::Purge { package } => {
            let (o, m) = linux::purge(package);
            return Done(o, m);
        }
        Action::RunValue { view, name, data } => {
            if item.elevate {
                return Step::Elevate(JobWhat::RunValue { view: view.clone(), name: name.clone(), data: data.clone() });
            }
            let (o, m) = windows::delete_run_value(view, name, data, &backups_dir());
            return Done(o, m);
        }
        _ => {}
    }
    if let Action::Link { target } = &item.action {
        let still = std::fs::symlink_metadata(&item.path).is_ok_and(|m| m.file_type().is_symlink())
            && std::fs::read_link(&item.path).ok().map(|t| if t.is_absolute() { t } else { item.root.join(t) }).as_ref() == Some(target);
        if !still {
            return Done("changed", "It is no longer the same link, so it was kept.".into());
        }
        return match to_trash(&item.path) {
            TrashOutcome::Trashed | TrashOutcome::Gone => Done("trashed", "Moved to the Trash.".into()),
            TrashOutcome::NeedsAdmin(m) => Done("needs-admin", m),
            TrashOutcome::Protected(m) | TrashOutcome::Failed(m) => Done("failed", m),
        };
    }
    if std::fs::symlink_metadata(&item.path).is_err() {
        return Done("gone", "Already gone.".into());
    }
    if !item.path.starts_with(&item.root) || link_on_the_way(&item.root, &item.path) {
        return Done("refused", "A folder on the way to it is now a link, so it was kept.".into());
    }
    match (&item.ident, ident(&item.path)) {
        (Some(a), Some(b)) if a == &b => {}
        _ => return Done("changed", "It changed since the scan (replaced, or now a link), so it was kept. Scan again.".into()),
    }
    // Windows would delete for good what the Recycle Bin can't take.
    #[cfg(windows)]
    if let Some(why) = windows::recycle_refusal(&item.path) {
        return Done("refused", why);
    }
    let elevated = || Step::Elevate(JobWhat::Trash { path: item.path.clone(), root: item.root.clone(), ident: item.ident.clone() });
    if item.elevate {
        return elevated();
    }
    if let Action::LaunchAgent { label } = &item.action {
        macos::bootout(label);
    }
    match to_trash(&item.path) {
        TrashOutcome::Trashed => Done("trashed", format!("Moved to the {}.", trash_word())),
        TrashOutcome::Gone => Done("gone", "Already gone.".into()),
        // Windows: refused here after all — the administrator's step tries it.
        TrashOutcome::NeedsAdmin(_) if cfg!(windows) => elevated(),
        TrashOutcome::NeedsAdmin(m) => Done("needs-admin", format!("It needs the administrator: {}", m)),
        TrashOutcome::Protected(m) => Done(
            "protected",
            format!("The system protects it ({}). Allow GitSwitch in System Settings › Privacy & Security, or move it to the Trash yourself.", m),
        ),
        TrashOutcome::Failed(m) => Done("failed", format!("Couldn't move it to the {}: {}", trash_word(), m)),
    }
}

fn trash_word() -> &'static str {
    if cfg!(windows) {
        "Recycle Bin"
    } else {
        "Trash"
    }
}

/// Leftover kinds that are files or folders (not registry keys, package
/// settings or start-up values).
pub(crate) fn file_backed(kind: &str) -> bool {
    !matches!(kind, "registry" | "package-settings" | "startup")
}

// ---------------------------------------------------------------------------
// Windows: what needs the administrator, done after one permission prompt —
// GitSwitch itself, started elevated, carries out a job file whose SHA-256
// it was given, checking every item again.

/// The argument GitSwitch is started with, elevated, to carry out a job.
pub const ELEVATED_FLAG: &str = "--gitswitch-elevated-cleanup";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct JobItem {
    pub id: String,
    pub what: JobWhat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum JobWhat {
    /// A file or folder to the Recycle Bin: the very one that was looked at.
    Trash { path: PathBuf, root: PathBuf, ident: Option<Ident> },
    /// A program's entry in Installed apps, exported first.
    UninstallEntry { view: String, key: String, name: String },
    /// A Run value, exported first.
    RunValue { view: String, name: String, data: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct Job {
    version: u32,
    items: Vec<JobItem>,
    /// Deleting files directly: the entry and start-up values go only once
    /// every folder went.
    registry_needs_files: bool,
    backups: PathBuf,
}

fn backups_dir() -> PathBuf {
    data_dir().unwrap_or_else(std::env::temp_dir).join("registry-backups")
}

fn result_path_of(job: &Path) -> PathBuf {
    let mut p = job.as_os_str().to_owned();
    p.push(".result.json");
    PathBuf::from(p)
}

/// One file or folder to the Recycle Bin, checked again first. `elevated`:
/// the administrator's run, which refuses anything outside the places
/// programs live in.
fn trash_checked(path: &Path, root: &Path, want: Option<&Ident>, elevated: bool) -> (&'static str, String) {
    if std::fs::symlink_metadata(path).is_err() {
        return ("gone", "Already gone.".into());
    }
    if !path.starts_with(root) || link_on_the_way(root, path) {
        return ("refused", "A folder on the way to it is now a link, so it was kept.".into());
    }
    match (want, ident(path)) {
        (Some(a), Some(b)) if a == &b => {}
        _ => return ("changed", "It changed since it was looked at (replaced, or now a link), so it was kept. Look again.".into()),
    }
    #[cfg(windows)]
    {
        if elevated {
            if let Err(why) = windows::elevated_path_ok(path) {
                return ("refused", format!("{} It was kept.", why));
            }
        }
        if let Some(why) = windows::recycle_refusal(path) {
            return ("refused", why);
        }
    }
    #[cfg(not(windows))]
    let _ = elevated;
    match to_trash(path) {
        TrashOutcome::Trashed => ("trashed", format!("Moved to the {}.", trash_word())),
        TrashOutcome::Gone => ("gone", "Already gone.".into()),
        TrashOutcome::NeedsAdmin(m) => ("needs-admin", format!("It needs the administrator: {}", m)),
        TrashOutcome::Protected(m) => ("protected", m),
        TrashOutcome::Failed(m) => ("failed", format!("Couldn't move it to the {}: {}", trash_word(), m)),
    }
}

/// Carry out cleanup items in order, each checked again first.
pub(crate) fn run_items(items: &[JobItem], registry_needs_files: bool, backups: &Path, elevated: bool) -> Vec<Deleted> {
    let mut out = Vec::new();
    let mut files_ok = true;
    for it in items {
        let (o, m) = match &it.what {
            // Deleting files directly stops at the first folder that can't go.
            JobWhat::Trash { .. } if registry_needs_files && !files_ok => ("refused", "Kept: an earlier folder could not be moved, so this one was left as it was.".into()),
            JobWhat::Trash { path, root, ident: want } => {
                let r = trash_checked(path, root, want.as_ref(), elevated);
                if r.0 != "trashed" && r.0 != "gone" {
                    files_ok = false;
                }
                r
            }
            JobWhat::UninstallEntry { .. } | JobWhat::RunValue { .. } if registry_needs_files && !files_ok => {
                ("refused", "Kept: its folder could not be moved, so the rest was left as it was.".into())
            }
            JobWhat::UninstallEntry { view, key, name } => windows::delete_uninstall_entry(view, key, name, backups),
            JobWhat::RunValue { view, name, data } => windows::delete_run_value(view, name, data, backups),
        };
        out.push(Deleted { id: it.id.clone(), outcome: o.into(), message: m });
    }
    out
}

/// No folder from the drive down to `p` is a link (or junction): what the
/// administrator writes lands where it says.
fn no_link_on_the_path(p: &Path) -> bool {
    p.ancestors().all(|a| std::fs::symlink_metadata(a).map(|m| !is_link(&m)).unwrap_or(true))
}

/// The elevated side: check the job against the hash it was approved with,
/// carry it out, write what happened next to it — never through a link.
pub(crate) fn run_job_file(job: &Path, sha: &str) -> i32 {
    let result = (|| -> Result<Vec<Deleted>, String> {
        if !no_link_on_the_path(job.parent().unwrap_or(job)) {
            return Err("GitSwitch's data folder is reached through a link, so the administrator's step did nothing.".into());
        }
        let bytes = std::fs::read(job).map_err(|e| format!("The job could not be read: {}", e))?;
        if !gitswitch_lock_core::sha256_hex(&bytes).eq_ignore_ascii_case(sha.trim()) {
            return Err("The job does not match what was approved, so nothing was done.".into());
        }
        let j: Job = serde_json::from_slice(&bytes).map_err(|e| format!("The job is damaged: {}", e))?;
        if j.version != 1 {
            return Err("The job was written by another version of GitSwitch, so nothing was done.".into());
        }
        if !no_link_on_the_path(&j.backups) {
            return Err("The backup folder is reached through a link, so the administrator's step did nothing.".into());
        }
        Ok(run_items(&j.items, j.registry_needs_files, &j.backups, true))
    })();
    let (code, body) = match result {
        Ok(d) => (0, serde_json::json!({ "ok": true, "results": d })),
        Err(e) => (2, serde_json::json!({ "ok": false, "error": e })),
    };
    // A new file only: never one (or a link) that is already there.
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create_new(true).open(result_path_of(job)) {
        use std::io::Write;
        let _ = f.write_all(&serde_json::to_vec(&body).unwrap_or_default());
    }
    code
}

/// GitSwitch started elevated for a job: carry it out. None when this is
/// an ordinary start; with the flag, always Some — never the app itself.
pub fn elevated_cleanup_from_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == ELEVATED_FLAG)?;
    let job = args.get(i + 1);
    let sha = args.iter().position(|a| a == "--sha256").and_then(|j| args.get(j + 1));
    Some(match (job, sha) {
        (Some(job), Some(sha)) => run_job_file(Path::new(job), sha),
        _ => 2,
    })
}

async fn launch_elevated(job: &Path, sha: &str) -> crate::elevate::Elevated {
    use crate::elevate::Elevated;
    if let Some(mode) = crate::elevate::test_mode() {
        return match mode.as_str() {
            "direct" => Elevated::Ran { code: run_job_file(job, sha), stdout: String::new(), stderr: String::new() },
            "cancelled" => Elevated::Cancelled,
            "failed" => Elevated::Failed("simulated failure".into()),
            other => Elevated::Failed(format!("unknown GITSWITCH_ELEVATE mode {}", other)),
        };
    }
    #[cfg(windows)]
    {
        match std::env::current_exe() {
            Ok(exe) => crate::elevate::windows_runas(&exe, format!("{} \"{}\" --sha256 {}", ELEVATED_FLAG, job.display(), sha), "GitSwitch's cleanup").await,
            Err(e) => Elevated::Failed(format!("GitSwitch could not find itself: {}", e)),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (job, sha);
        Elevated::Unsupported("Only on Windows.".into())
    }
}

/// Write the job, ask Windows for permission once, read what happened.
/// Err: nothing was done (the prompt was declined, or it could not run).
async fn elevated_run(items: Vec<JobItem>, registry_needs_files: bool) -> Result<Vec<Deleted>, String> {
    use crate::elevate::Elevated;
    let dir = data_dir().ok_or("GitSwitch has no data folder to write the job in.")?.join("uninstall-jobs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("The job could not be written: {}", e))?;
    let job_path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    let job = Job { version: 1, items, registry_needs_files, backups: backups_dir() };
    let bytes = serde_json::to_vec_pretty(&job).map_err(|e| e.to_string())?;
    std::fs::write(&job_path, &bytes).map_err(|e| format!("The job could not be written: {}", e))?;
    let sha = gitswitch_lock_core::sha256_hex(&bytes);
    let result_path = result_path_of(&job_path);
    // One prompt at a time — a test run (no prompt at all) doesn't hold the guard.
    let testing = crate::elevate::test_mode().is_some();
    let guard = if testing { None } else { crate::elevate::begin() };
    let outcome = if testing || guard.is_some() { Some(launch_elevated(&job_path, &sha).await) } else { None };
    drop(guard);
    let r = match outcome {
        None => Err("Another administrator prompt is open — finish it, then try again.".to_string()),
        Some(Elevated::Ran { .. }) => match std::fs::read(&result_path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
            Some(v) if v["ok"].as_bool() == Some(true) => serde_json::from_value::<Vec<Deleted>>(v["results"].clone()).map_err(|e| e.to_string()),
            Some(v) => Err(v["error"].as_str().unwrap_or("The administrator's step failed.").to_string()),
            None => Err("The administrator's step ended without saying what it did.".into()),
        },
        Some(Elevated::Cancelled) | Some(Elevated::Denied) => Err("Windows' permission prompt was declined, so these were kept.".into()),
        Some(Elevated::Failed(m)) | Some(Elevated::Unsupported(m)) => Err(m),
        Some(_) => Err("The administrator's step could not run.".into()),
    };
    let _ = std::fs::remove_file(&job_path);
    let _ = std::fs::remove_file(&result_path);
    r
}

/// Windows: delete a program's files directly instead of running its
/// uninstaller — its folders to the Recycle Bin, then its entry and
/// start-up values (each exported first); the administrator's part after
/// one prompt. Nothing is half done: the entry goes only once the folders did.
pub async fn force_remove(session: String) -> Result<Progress, AppError> {
    let mut s = load_session(&session)?;
    if !matches!(s.state.as_str(), "prepared" | "still-installed" | "failed") {
        return Err(AppError::Config(format!("This uninstall is already {}.", s.state)));
    }
    let s2 = s.clone();
    let items = tokio::task::spawn_blocking(move || windows::force_items(&s2)).await.map_err(|e| AppError::Command(e.to_string()))?.map_err(AppError::Config)?;
    log(serde_json::json!({ "event": "force-remove", "app": s.app.name, "record": s.record }));
    let admin = items.iter().any(|(_, a)| *a);
    let backups = backups_dir();
    let results = if admin {
        // One prompt: every folder and what only the administrator may
        // remove. Your own entries are removed here, afterwards, in your
        // own registry — whoever approved the prompt.
        let (elevated, later): (Vec<_>, Vec<_>) = items.into_iter().partition(|(j, a)| *a || matches!(j.what, JobWhat::Trash { .. }));
        let first = Box::pin(elevated_run(elevated.into_iter().map(|(j, _)| j).collect(), true)).await.map_err(AppError::Config)?;
        let files_ok = first.iter().filter(|d| d.id.starts_with("folder")).all(|d| d.outcome == "trashed" || d.outcome == "gone");
        let later: Vec<JobItem> = later.into_iter().map(|(j, _)| j).collect();
        let rest = if files_ok {
            tokio::task::spawn_blocking(move || run_items(&later, true, &backups, false)).await.map_err(|e| AppError::Command(e.to_string()))?
        } else {
            later.into_iter().map(|j| Deleted { id: j.id, outcome: "refused".into(), message: "Kept: its folder could not be moved, so the rest was left as it was.".into() }).collect()
        };
        first.into_iter().chain(rest).collect()
    } else {
        let jobs: Vec<JobItem> = items.into_iter().map(|(j, _)| j).collect();
        tokio::task::spawn_blocking(move || run_items(&jobs, true, &backups, false)).await.map_err(|e| AppError::Command(e.to_string()))?
    };
    for r in &results {
        log(serde_json::json!({ "event": "force-remove-item", "app": s.app.name, "id": r.id, "outcome": r.outcome, "message": r.message }));
    }
    let problems: Vec<String> = results.iter().filter(|r| !matches!(r.outcome.as_str(), "trashed" | "removed" | "gone")).map(|r| r.message.clone()).collect();
    let s3 = s.clone();
    let gone = tokio::task::spawn_blocking(move || platform_gone(&s3)).await.map_err(|e| AppError::Command(e.to_string()))?;
    s.process = None;
    match gone {
        Ok(()) => {
            s.state = "gone".into();
            s.message = Some(if problems.is_empty() {
                format!("{}'s files are in the Recycle Bin and it is no longer in Installed apps.", s.app.name)
            } else {
                format!("{} is no longer in Installed apps, but: {}", s.app.name, problems.join(" "))
            });
        }
        Err(why) => {
            s.state = "still-installed".into();
            s.message = Some(if problems.is_empty() { why } else { problems.join(" ") });
        }
    }
    save_session(&s);
    Ok(Progress { state: s.state.clone(), message: s.message.clone() })
}

/// Uninstalls that were started and not cleaned up — after a restart, say.
pub fn sessions() -> Vec<SessionSummary> {
    let mut out = Vec::new();
    let Some(dir) = sessions_dir() else { return out };
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    let month_ago = now() - 30 * 24 * 3600;
    for e in rd.flatten() {
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let Ok(s) = serde_json::from_str::<Session>(&text) else { continue };
        if s.started < month_ago || s.state == "cleaned" || s.state == "prepared" {
            continue;
        }
        out.push(SessionSummary { session: s.id, name: s.app.name, state: s.state, started: s.started });
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.started));
    out
}

/// Forget an unfinished uninstall.
pub fn forget(session: String) -> Result<(), AppError> {
    let s = load_session(&session)?;
    lock().sessions.remove(&s.id);
    if let Some(dir) = sessions_dir() {
        let _ = std::fs::remove_file(dir.join(format!("{}.json", s.id)));
    }
    Ok(())
}

/// macOS: open the app's own uninstaller.
pub fn open_vendor_uninstaller(session: String) -> Result<(), AppError> {
    let s = load_session(&session)?;
    match &s.record {
        Record::MacBundle { vendor_uninstaller: Some(u), .. } if u.exists() => {
            crate::spawn::command("open").arg(u).spawn().map(|_| ()).map_err(|e| AppError::Command(e.to_string()))
        }
        _ => Err(AppError::Config("This app has no uninstaller of its own here.".into())),
    }
}

/// The probe: several commands in one process, as the app runs them —
/// "$session" and "$scan" stand for the last ones returned, "$ids:high" and
/// "$ids:all" for the last scan's items (the sure ones not needing the
/// administrator, or every one).
#[cfg(test)]
pub(crate) async fn script(steps: Vec<serde_json::Value>) -> Result<Vec<serde_json::Value>, AppError> {
    let mut out = Vec::new();
    let (mut session, mut scan_id, mut items): (String, String, Vec<Leftover>) = (String::new(), String::new(), Vec::new());
    for step in steps {
        let cmd = step["cmd"].as_str().unwrap_or_default().to_string();
        let mut args = step["args"].clone();
        if let Some(obj) = args.as_object_mut() {
            for v in obj.values_mut() {
                match v.as_str() {
                    Some("$session") => *v = serde_json::json!(session),
                    Some("$scan") => *v = serde_json::json!(scan_id),
                    Some("$ids:high") => *v = serde_json::json!(items.iter().filter(|i| i.confidence == Confidence::High && !i.admin).map(|i| i.id.clone()).collect::<Vec<_>>()),
                    Some("$ids:all") => *v = serde_json::json!(items.iter().map(|i| i.id.clone()).collect::<Vec<_>>()),
                    _ => {}
                }
            }
        }
        let s = |k: &str| args[k].as_str().unwrap_or_default().to_string();
        let r: Result<serde_json::Value, AppError> = match cmd.as_str() {
            "list" => list(s("kind")).await.map(|v| serde_json::json!(v)),
            "prepare" => prepare(s("id")).await.map(|p| {
                session = p.session.clone();
                serde_json::json!(p)
            }),
            "uninstall" => uninstall(s("session")).await.map(|v| serde_json::json!(v)),
            "status" => status(s("session")).await.map(|v| serde_json::json!(v)),
            "check_gone" => check_gone(s("session")).await.map(|v| serde_json::json!(v)),
            "scan" => scan(s("session"), args["registry"].as_bool().unwrap_or(false)).await.map(|v| {
                scan_id = v.scan.clone();
                items = v.items.clone();
                serde_json::json!(v)
            }),
            "delete" => delete(s("scan"), serde_json::from_value(args["ids"].clone()).unwrap_or_default()).await.map(|v| serde_json::json!(v)),
            "sessions" => Ok(serde_json::json!(sessions())),
            // Change the world between two steps (a swapped folder, a link).
            "sh" => std::process::Command::new("sh").arg("-c").arg(s("run")).status().map(|st| serde_json::json!(st.success())).map_err(|e| AppError::Command(e.to_string())),
            other => Err(AppError::Config(format!("unknown step {}", other))),
        };
        out.push(match r {
            Ok(v) => serde_json::json!({ "ok": true, "value": v }),
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
        });
    }
    Ok(out)
}

/// The probe's look at what a scan WOULD offer for an app that is still
/// installed — nothing is uninstalled, nothing is deleted, the gone check is
/// skipped. For reviewing the rules against a real computer.
#[cfg(test)]
pub(crate) async fn dry_scan(id: String) -> Result<Vec<(String, String, String, bool)>, AppError> {
    if !lock().list.contains_key(&id) {
        list("apps".into()).await?;
    }
    let (app, rec) = lock().list.get(&id).cloned().ok_or_else(|| AppError::Config("No such app.".into()))?;
    let rec = if cfg!(windows) { windows::enrich(&app, &rec) } else { rec };
    let (_, ev, _) = if cfg!(target_os = "macos") {
        macos::prepare(&app, &rec)?
    } else if cfg!(windows) {
        windows::prepare(&app, &rec)?
    } else {
        linux::prepare(&app, &rec)?
    };
    let s = Session { id: "dry".into(), app, record: rec, evidence: ev, facts: vec![], started: 0, state: "dry".into(), message: None, packages: vec![], process: None };
    let others = platform_others(&s);
    let (c, _) = if cfg!(target_os = "macos") {
        macos::leftovers(&s, &others)
    } else if cfg!(windows) {
        windows::leftovers(&s, &others, true)
    } else {
        linux::leftovers(&s, &others)
    };
    Ok(c.into_iter().map(|c| (c.path.to_string_lossy().to_string(), format!("{:?}", c.m.confidence), c.m.reason, c.admin)).collect())
}

/// The probe's look at what a scan of a saved session (a session file,
/// read and never written) WOULD offer — nothing deleted, the gone check
/// skipped. For reviewing the rules against what an uninstall really left.
#[cfg(test)]
pub(crate) async fn dry_session(path: String) -> Result<Vec<serde_json::Value>, AppError> {
    let text = std::fs::read_to_string(&path).map_err(|e| AppError::Config(e.to_string()))?;
    let s: Session = serde_json::from_str(&text).map_err(|e| AppError::Config(e.to_string()))?;
    tokio::task::spawn_blocking(move || {
        let others = platform_others(&s);
        let (c, notes) = if cfg!(target_os = "macos") {
            macos::leftovers(&s, &others)
        } else if cfg!(windows) {
            windows::leftovers(&s, &others, true)
        } else {
            linux::leftovers(&s, &others)
        };
        let mut out: Vec<serde_json::Value> = c
            .into_iter()
            .map(|c| serde_json::json!({ "path": c.path, "kind": c.kind, "place": c.place, "confidence": c.m.confidence, "reason": c.m.reason, "admin": c.admin }))
            .collect();
        out.push(serde_json::json!({ "notes": notes }));
        Ok(out)
    })
    .await
    .map_err(|e| AppError::Command(e.to_string()))?
}

/// The probe's look at what deleting an installed program's files directly
/// would remove — read only: the record GitSwitch would keep, and the plan.
#[cfg(test)]
pub(crate) async fn dry_force(id: String) -> Result<serde_json::Value, AppError> {
    if !lock().list.contains_key(&id) {
        list("apps".into()).await?;
    }
    let (app, rec) = lock().list.get(&id).cloned().ok_or_else(|| AppError::Config("No such app.".into()))?;
    tokio::task::spawn_blocking(move || {
        let rec = windows::enrich(&app, &rec);
        let (plan, why) = windows::force_plan(&app, &rec);
        Ok(serde_json::json!({ "app": app, "record": rec, "plan": plan, "unavailable": why }))
    })
    .await
    .map_err(|e| AppError::Command(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gitswitch-apps-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    #[test]
    fn the_scan_finds_only_entries_directly_inside_and_never_through_a_link() {
        let root = tmp("scan");
        let caches = root.join("Caches");
        std::fs::create_dir_all(caches.join("com.foo.app")).unwrap();
        std::fs::create_dir_all(caches.join("Foo App")).unwrap();
        std::fs::create_dir_all(caches.join("deeper").join("com.foo.app")).unwrap();
        std::fs::create_dir_all(root.join("precious")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("precious"), caches.join("FooApp")).unwrap();
        let vendor = root.join("Support");
        std::fs::create_dir_all(vendor.join("Foo").join("Foo App")).unwrap();
        std::fs::create_dir_all(vendor.join("Foo").join("Other")).unwrap();
        let ev = Evidence { ids: vec!["com.foo.app".into()], names: vec!["Foo App".into()], groups: vec![], vendor: Some("Foo".into()) };
        let places = vec![Place::new(caches.clone(), "Caches", Allow::IdsAndNames), Place::new(vendor.clone(), "Support", Allow::IdsAndNames).vendor()];
        let (found, notes) = scan_places(&places, &ev, &Others::new(true));
        let mut got: Vec<String> = found.iter().map(|c| c.path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        got.sort();
        assert_eq!(got, vec!["Caches/Foo App", "Caches/com.foo.app", "Support/Foo/Foo App"], "{notes:?}");
        assert!(found.iter().all(|c| c.path.file_name().unwrap() != "FooApp"), "a link is never offered");
        assert!(!found.iter().any(|c| c.path == vendor.join("Foo")), "the vendor folder itself never");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_on_the_way_or_a_swapped_item_is_refused() {
        let root = tmp("links");
        let place = root.join("Caches");
        std::fs::create_dir_all(place.join("com.foo.app")).unwrap();
        assert!(!link_on_the_way(&place, &place.join("com.foo.app")));
        let seen = ident(&place.join("com.foo.app")).unwrap();
        // Swapped for another folder with the same name.
        std::fs::rename(place.join("com.foo.app"), root.join("moved")).unwrap();
        std::fs::create_dir_all(place.join("com.foo.app")).unwrap();
        assert_ne!(Some(seen), ident(&place.join("com.foo.app")), "another folder is not the one the scan saw");
        // Replaced by a link.
        std::fs::remove_dir_all(place.join("com.foo.app")).unwrap();
        std::os::unix::fs::symlink(root.join("moved"), place.join("com.foo.app")).unwrap();
        assert_eq!(ident(&place.join("com.foo.app")), None, "a link has no identity to delete");
        // The place reached through a link.
        std::os::unix::fs::symlink(&place, root.join("CachesLink")).unwrap();
        assert!(link_on_the_way(&root.join("CachesLink"), &root.join("CachesLink").join("x")));
        std::fs::create_dir_all(place.join("a")).unwrap();
        std::os::unix::fs::symlink(root.join("moved"), place.join("a").join("b")).unwrap();
        assert!(link_on_the_way(&place, &place.join("a").join("b").join("c")), "a link between the place and the item");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_publishers_folder_and_the_apps_name_inside_it_are_sure_on_windows_places_only() {
        let root = tmp("vendor-sure");
        let place = root.join("ProgramData");
        std::fs::create_dir_all(place.join("CleverFiles").join("Disk Drill")).unwrap();
        std::fs::create_dir_all(place.join("BitTorrent").join("uTorrent")).unwrap();
        let ev = Evidence { ids: vec![], names: vec!["Disk Drill".into(), "uTorrent".into()], groups: vec![], vendor: Some("CleverFiles".into()) };
        let o = Others::new(true);
        let (plain, _) = scan_places(&[Place::new(place.clone(), "P", Allow::IdsAndNames).vendor()], &ev, &o);
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].m.confidence, Confidence::Medium, "elsewhere (macOS) a name alone stays a maybe");
        let (sure, _) = scan_places(&[Place::new(place.clone(), "P", Allow::IdsAndNames).vendor().sure()], &ev, &o);
        assert_eq!(sure.len(), 1);
        assert_eq!(sure[0].m.confidence, Confidence::High, "publisher folder + exact name");
        let bt = Evidence { vendor: Some("BitTorrent Limited".into()), ..ev.clone() };
        let (found, _) = scan_places(&[Place::new(place.clone(), "P", Allow::IdsAndNames).vendor().sure()], &bt, &o);
        assert_eq!(found.iter().map(|c| c.path.file_name().unwrap().to_string_lossy().to_string()).collect::<Vec<_>>(), vec!["uTorrent"], "the publisher without its legal form");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn a_publishers_folder_holding_only_the_app_is_offered_instead_unless_another_app_has_that_publisher() {
        // As Windows names folders (canonicalize would add \\?\).
        let root = PathBuf::from(tmp("promote").to_string_lossy().trim_start_matches(r"\\?\"));
        let place = root.join("Program Files");
        let inner = place.join("CleverFiles").join("Disk Drill");
        std::fs::create_dir_all(&inner).unwrap();
        let ev = Evidence { ids: vec![], names: vec!["Disk Drill".into()], groups: vec![], vendor: Some("CleverFiles".into()) };
        let cand = || Candidate {
            path: inner.clone(),
            root: place.clone(),
            place: "P".into(),
            kind: "folder",
            m: Match { confidence: Confidence::High, reason: "x".into() },
            admin: false,
            command: None,
            action: Action::Trash,
            sized: true,
            entry: None,
        };
        // The temp folder is a few levels deep, like C:\Program Files\X.
        let mut v = vec![cand()];
        windows::promote_vendor_folders(&mut v, &ev, &Others::new(true), std::slice::from_ref(&place));
        assert_eq!(v[0].path, place.join("CleverFiles"), "{:?}", v[0].m.reason);
        assert!(v[0].path.starts_with(&v[0].root));
        let mut o = Others::new(true);
        o.add_app(&[], &["Another Tool".to_string()], Some("CleverFiles"), &[]);
        let mut v = vec![cand()];
        windows::promote_vendor_folders(&mut v, &ev, &o, std::slice::from_ref(&place));
        assert_eq!(v[0].path, inner, "another installed app has that publisher");
        std::fs::create_dir_all(place.join("CleverFiles").join("Other Product")).unwrap();
        let mut v = vec![cand()];
        windows::promote_vendor_folders(&mut v, &ev, &Others::new(true), std::slice::from_ref(&place));
        assert_eq!(v[0].path, inner, "the publisher's folder holds something else too");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_administrators_job_is_refused_unless_it_is_exactly_what_was_approved() {
        let root = tmp("job");
        let job = root.join("job.json");
        let j = Job { version: 1, items: vec![], registry_needs_files: false, backups: root.join("b") };
        let bytes = serde_json::to_vec(&j).unwrap();
        std::fs::write(&job, &bytes).unwrap();
        assert_eq!(run_job_file(&job, "00"), 2, "another hash");
        let r: serde_json::Value = serde_json::from_slice(&std::fs::read(result_path_of(&job)).unwrap()).unwrap();
        assert!(r["error"].as_str().unwrap().contains("does not match"));
        assert_eq!(run_job_file(&job, &gitswitch_lock_core::sha256_hex(&bytes)), 0);
        let r: serde_json::Value = serde_json::from_slice(&std::fs::read(result_path_of(&job)).unwrap()).unwrap();
        assert!(r["error"].as_str().unwrap().contains("does not match"), "a result already there is never written over");
        let _ = std::fs::remove_file(result_path_of(&job));
        assert_eq!(run_job_file(&job, &gitswitch_lock_core::sha256_hex(&bytes)), 0);
        let r: serde_json::Value = serde_json::from_slice(&std::fs::read(result_path_of(&job)).unwrap()).unwrap();
        assert_eq!(r["ok"], serde_json::json!(true));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Tests that point GitSwitch's folders somewhere else take turns.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
        L.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A temp folder as Windows names it (canonicalize adds \\?\ there).
    fn plain_tmp(name: &str) -> PathBuf {
        PathBuf::from(tmp(name).to_string_lossy().trim_start_matches(r"\\?\"))
    }

    #[test]
    fn the_administrators_part_runs_after_the_prompt_and_nothing_when_it_is_declined() {
        let _g = env_lock();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let root = plain_tmp("elevated");
        std::env::set_var("GITSWITCH_DATA_DIR", root.join("data"));
        std::env::set_var("GITSWITCH_TRASH_DIR", root.join("trash"));
        let place = root.join("place");
        std::fs::create_dir_all(place.join("Kept")).unwrap();
        std::fs::create_dir_all(place.join("Moved")).unwrap();
        let item = |n: &str| JobItem { id: n.into(), what: JobWhat::Trash { path: place.join(n), root: place.clone(), ident: ident(&place.join(n)) } };
        std::env::set_var("GITSWITCH_ELEVATE", "cancelled");
        let declined = rt.block_on(elevated_run(vec![item("Kept")], false));
        assert!(declined.as_ref().unwrap_err().contains("declined"), "{declined:?}");
        assert!(place.join("Kept").exists(), "declined: nothing moved");
        std::env::set_var("GITSWITCH_ELEVATE", "direct");
        let done = rt.block_on(elevated_run(vec![item("Moved")], false)).unwrap();
        assert_eq!(done[0].outcome, "trashed", "{done:?}");
        assert!(!place.join("Moved").exists() && root.join("trash").join("Moved").exists());
        let jobs = root.join("data").join("com.gitswitch.app").join("uninstall-jobs");
        assert_eq!(std::fs::read_dir(&jobs).unwrap().count(), 0, "the job and its result are cleaned up");
        for v in ["GITSWITCH_ELEVATE", "GITSWITCH_DATA_DIR", "GITSWITCH_TRASH_DIR"] {
            std::env::remove_var(v);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cleanup_item_is_moved_only_while_it_is_the_one_that_was_looked_at() {
        let _g = env_lock();
        let root = tmp("items");
        let trash = root.join("trash");
        std::env::set_var("GITSWITCH_TRASH_DIR", &trash);
        let place = root.join("place");
        std::fs::create_dir_all(place.join("A")).unwrap();
        std::fs::create_dir_all(place.join("B")).unwrap();
        let seen_a = ident(&place.join("A"));
        let seen_b = ident(&place.join("B"));
        // B is swapped for another folder of the same name.
        std::fs::rename(place.join("B"), root.join("B-moved")).unwrap();
        std::fs::create_dir_all(place.join("B")).unwrap();
        let items = vec![
            JobItem { id: "a".into(), what: JobWhat::Trash { path: place.join("A"), root: place.clone(), ident: seen_a } },
            JobItem { id: "b".into(), what: JobWhat::Trash { path: place.join("B"), root: place.clone(), ident: seen_b } },
            JobItem { id: "c".into(), what: JobWhat::Trash { path: place.join("C"), root: place.clone(), ident: None } },
        ];
        let out = run_items(&items, false, &root.join("b"), false);
        let o: Vec<&str> = out.iter().map(|d| d.outcome.as_str()).collect();
        assert_eq!(o, vec!["trashed", "changed", "gone"]);
        assert!(trash.join("A").exists() && place.join("B").exists());
        // Deleting files directly: the rest waits for every folder.
        let items = vec![
            JobItem { id: "f".into(), what: JobWhat::Trash { path: place.join("B"), root: place.clone(), ident: None } },
            JobItem { id: "e".into(), what: JobWhat::UninstallEntry { view: "HKCU".into(), key: "Nothing".into(), name: "Nothing".into() } },
        ];
        let out = run_items(&items, true, &root.join("b"), false);
        assert_eq!(out[0].outcome, "changed");
        assert_eq!(out[1].outcome, "refused", "its entry stays when its folder could not be moved");
        // Deleting files directly stops at the first folder that can't go.
        std::fs::create_dir_all(place.join("D")).unwrap();
        let items = vec![
            JobItem { id: "f0".into(), what: JobWhat::Trash { path: place.join("B"), root: place.clone(), ident: None } },
            JobItem { id: "f1".into(), what: JobWhat::Trash { path: place.join("D"), root: place.clone(), ident: ident(&place.join("D")) } },
        ];
        let out = run_items(&items, true, &root.join("b"), false);
        assert_eq!(out.iter().map(|d| d.outcome.as_str()).collect::<Vec<_>>(), vec!["changed", "refused"]);
        assert!(place.join("D").exists(), "the second folder was not touched");
        std::env::remove_var("GITSWITCH_TRASH_DIR");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn jobs_and_results_survive_the_trip_to_the_administrators_run() {
        let item = JobItem { id: "x".into(), what: JobWhat::Trash { path: PathBuf::from("p"), root: PathBuf::from("r"), ident: Some(Ident { dev: 1, ino: 2, owner: 0, dir: true, len: None, mtime: Some(1_700_000_000_123_456_789) }) } };
        let back: JobItem = serde_json::from_str(&serde_json::to_string(&item).unwrap()).unwrap();
        assert_eq!(back, item);
        let d: Vec<Deleted> = serde_json::from_value(serde_json::json!([{ "id": "x", "outcome": "trashed", "message": "m" }])).unwrap();
        assert_eq!(d[0].outcome, "trashed");
    }

    #[test]
    fn sizes_stop_at_links_and_report_running_out_of_time() {
        let root = tmp("size");
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("d").join("a"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("big"), vec![0u8; 5000]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("big"), root.join("d").join("link")).unwrap();
        assert_eq!(size_of(&root.join("d"), Duration::from_secs(5)), (1000, true));
        let _ = std::fs::remove_dir_all(&root);
    }
}
