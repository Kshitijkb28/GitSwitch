//! Find repositories: the clones on this computer that no profile folder
//! holds, which the Repository page therefore does not list — and, for each,
//! what GitHub says the signed-in accounts may do with it.
//!
//! The walk starts from the places the person chose, the folders that hold
//! their profile folders outside the home folder, and the home folder itself.
//! It reads folder names only: git is asked about a folder once it is known
//! to be a repository the page does not list. It never follows a link — a
//! symbolic link or a Windows junction near the home folder is offered as a
//! place to look instead — so a loop, or a second road to the same disk,
//! cannot make it long. Folders that hold no clones (media libraries,
//! application data, package caches, hidden folders) are passed over, and so
//! are network filesystems mounted under the home folder.

use crate::error::AppError;
use crate::profiles::Profile;
use crate::repo_scan::ScannedRepo;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How far and how long one search may go.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Folders below a place, the place itself being 0 — the depth the
    /// Repository page scans a profile folder to.
    pub depth: u32,
    /// Folders looked at under one place before it is cut short.
    pub per_place: usize,
    /// The whole walk, every place together.
    pub time: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { depth: 5, per_place: 100_000, time: Duration::from_secs(30) }
    }
}

/// Direct children of the home folder that hold no clones, in any letter
/// case: the media libraries and application data of macOS, Windows and
/// Linux desktops, and two package managers' own trees (scoop's buckets and
/// snap's are git repositories nobody works in).
const HOME_SKIP: [&str; 12] = [
    "library",
    "applications",
    "pictures",
    "music",
    "movies",
    "videos",
    "public",
    "appdata",
    "application data",
    "local settings",
    "scoop",
    "snap",
];

/// Linked folders offered as places, at most.
const MAX_LINKED: usize = 10;

// ---------------------------------------------------------------------------
// What a search reports
// ---------------------------------------------------------------------------

/// Where a repository's commits get their email from today.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct CommitsAs {
    pub profile_id: Option<String>,
    /// "folder" (a profile folder holds it, deeper than the page looks),
    /// "rule" (a profile's remote rule), "gitdir" (a worktree or a separate
    /// git folder: it follows where its git folder is), "default" (the default
    /// profile, as for every folder no rule names), "local" (the repository
    /// sets its own email), "none" (no email at all).
    pub via: String,
}

/// A repository the Repository page does not list.
#[derive(Debug, Serialize)]
pub struct FoundRepo {
    #[serde(flatten)]
    pub repo: ScannedRepo,
    /// The GitHub repository pushes go to: the push URL's when one is set,
    /// otherwise the origin's. Empty when pushes go somewhere that is not
    /// GitHub.
    pub push_owner: String,
    pub push_name: String,
    /// `.git` is a file: a worktree, or a clone made with a separate git
    /// folder. Its identity follows that git folder, not this one.
    pub gitdir_elsewhere: bool,
    pub commits_as: CommitsAs,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Cut {
    pub place: String,
    pub why: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Rejected {
    pub place: String,
    pub why: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Linked {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, Default)]
pub struct FoundClones {
    /// The home folder, for showing paths under it as `~/…`.
    pub home: Option<String>,
    /// The places searched, in order.
    pub roots: Vec<String>,
    pub repos: Vec<FoundRepo>,
    /// Repositories found that the Repository page lists already.
    pub already_listed: usize,
    /// Places cut short, or not reached, and why.
    pub truncated: Vec<Cut>,
    /// Folders at the top of a place that could not be read — on macOS, a
    /// privacy refusal for Desktop, Documents or Downloads looks like this.
    pub unreadable: Vec<String>,
    /// Places asked for that could not be searched, and why.
    pub rejected: Vec<Rejected>,
    /// Linked folders near the home folder that lead somewhere not searched.
    pub linked: Vec<Linked>,
    /// Stopped before it ended.
    pub cancelled: bool,
}

/// The folders a search found, before git is asked about any of them.
#[derive(Debug, Default)]
pub(crate) struct Search {
    pub roots: Vec<PathBuf>,
    /// Repositories the page does not list, with whether `.git` is a file.
    pub candidates: Vec<(PathBuf, bool)>,
    pub already_listed: usize,
    pub truncated: Vec<Cut>,
    pub unreadable: Vec<String>,
    pub rejected: Vec<Rejected>,
    pub linked: Vec<Linked>,
    pub cancelled: bool,
}

// ---------------------------------------------------------------------------
// Progress and cancel, by search id
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Clone, Default)]
pub struct FindProgress {
    /// "looking" while folders are walked, "checking" while GitHub is asked.
    pub phase: String,
    pub folders: usize,
    pub found: usize,
    pub checked: usize,
    pub total: usize,
    /// The place being walked.
    pub place: String,
}

pub(crate) struct ScanState {
    cancel: AtomicBool,
    progress: Mutex<FindProgress>,
}

impl ScanState {
    pub(crate) fn new() -> Self {
        ScanState { cancel: AtomicBool::new(false), progress: Mutex::new(FindProgress::default()) }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn set(&self, f: impl FnOnce(&mut FindProgress)) {
        if let Ok(mut p) = self.progress.lock() {
            f(&mut p);
        }
    }
}

type Registry = Mutex<Vec<(String, Arc<ScanState>)>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

/// The search with this id, made on first use. A Stop that arrives before
/// the search itself makes it, cancelled, so the search ends as it starts.
/// The sixteen most recent are kept.
fn scan(id: &str) -> Arc<ScanState> {
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, s)) = r.iter().find(|(k, _)| k == id) {
        return s.clone();
    }
    let s = Arc::new(ScanState::new());
    r.push((id.to_string(), s.clone()));
    if r.len() > 16 {
        r.remove(0);
    }
    s
}

pub fn progress(id: &str) -> Option<FindProgress> {
    let r = registry().lock().ok()?;
    let (_, s) = r.iter().find(|(k, _)| k == id)?;
    s.progress.lock().ok().map(|p| p.clone())
}

pub fn cancel(id: &str) {
    scan(id).cancel.store(true, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Places
// ---------------------------------------------------------------------------

fn shown(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn norm(p: &Path) -> String {
    crate::paths::norm(&p.to_string_lossy())
}

/// `~`, `~/…` and `~\…` against the home folder; anything else as given.
fn expand_home(place: &str, home: Option<&Path>) -> Option<PathBuf> {
    if place == "~" {
        return home.map(Path::to_path_buf);
    }
    match place.strip_prefix("~/").or_else(|| place.strip_prefix("~\\")) {
        Some(rest) => home.map(|h| h.join(rest)),
        None => Some(PathBuf::from(place)),
    }
}

/// Is `path` the folder `dir`, or inside it — as written, or once links
/// are resolved?
fn within_either(path: &str, dir: &Path) -> bool {
    let d = norm(dir);
    if crate::paths::is_within(path, &d) {
        return true;
    }
    match (crate::paths::real_form(path), crate::paths::real_form(&shown(dir))) {
        (Some(p), Some(d)) => crate::paths::is_within(&p, &d),
        _ => false,
    }
}

struct Root {
    path: PathBuf,
    home: bool,
}

fn add_root(out: &mut Vec<Root>, seen: &mut Vec<crate::paths::FolderId>, path: PathBuf, home: bool) {
    let id = crate::paths::FolderId::of(&shown(&path));
    if seen.iter().any(|s| s.same(&id)) {
        return;
    }
    seen.push(id);
    out.push(Root { path, home });
}

/// The places to search, in order: the ones asked for, then the folders that
/// hold profile folders outside the home folder (other clones tend to sit
/// beside them), then the home folder.
fn places(extra: &[String], profs: &[Profile], home: Option<&Path>, rejected: &mut Vec<Rejected>) -> Vec<Root> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for raw in extra {
        let place = raw.trim();
        if place.is_empty() {
            continue;
        }
        let reject = |why: &str| Rejected { place: place.to_string(), why: why.to_string() };
        let Some(path) = expand_home(place, home) else {
            rejected.push(reject("the home folder is not known"));
            continue;
        };
        if !path.is_absolute() {
            rejected.push(reject("not a full path"));
            continue;
        }
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => add_root(&mut out, &mut seen, path, false),
            Ok(_) => rejected.push(reject("not a folder")),
            Err(_) => rejected.push(reject("not found")),
        }
    }
    for p in profs {
        for d in &p.directories {
            let dir = Path::new(d);
            if home.is_some_and(|h| within_either(&norm(dir), h)) {
                continue;
            }
            let Some(parent) = dir.parent() else { continue };
            // Never a drive or the filesystem's root: that is the whole disk.
            if parent.parent().is_none() || !parent.is_dir() {
                continue;
            }
            add_root(&mut out, &mut seen, parent.to_path_buf(), false);
        }
    }
    if let Some(h) = home.filter(|h| h.is_dir()) {
        add_root(&mut out, &mut seen, h.to_path_buf(), true);
    }
    out
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

fn home_skip(name: &str) -> bool {
    let lower = name.to_lowercase();
    HOME_SKIP.contains(&lower.as_str())
}

/// Windows: a folder both hidden and system (the profile's own plumbing), or
/// one whose contents are in the cloud only (OneDrive's "online-only"):
/// listing it would download it.
#[cfg(windows)]
fn windows_skip(entry: &std::fs::DirEntry, path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    let Ok(m) = entry.metadata() else { return false };
    let a = m.file_attributes();
    if a & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0 {
        return true;
    }
    a & HIDDEN != 0 && a & SYSTEM != 0 && !path.join(".git").exists()
}

/// The device a folder is on (unix), to notice a mount point.
#[cfg(unix)]
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device_of(_: &Path) -> Option<u64> {
    None
}

#[cfg(unix)]
fn entry_device(entry: &std::fs::DirEntry) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    entry.metadata().ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn entry_device(_: &std::fs::DirEntry) -> Option<u64> {
    None
}

/// A network or FUSE filesystem: walking one under the home folder could
/// wait on a server, or list a whole remote share.
#[cfg(target_os = "macos")]
fn remote_filesystem(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: a zeroed statfs is a valid out-buffer; the path is NUL-terminated.
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return false;
    }
    s.f_flags & (libc::MNT_LOCAL as u32) == 0
}

#[cfg(target_os = "linux")]
fn remote_filesystem(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    // NFS, SMB, CIFS, SMB2, FUSE (sshfs, rclone…), AFS, Coda, 9P, Ceph, NCP.
    const REMOTE: [u32; 10] = [
        0x6969,
        0x517B,
        0xFF53_4D42,
        0xFE53_4D42,
        0x6573_5546,
        0x5346_414F,
        0x7375_7245,
        0x0102_1997,
        0x00C3_6400,
        0x564C,
    ];
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: a zeroed statfs is a valid out-buffer; the path is NUL-terminated.
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return false;
    }
    // f_type's integer type differs between architectures; the magic is 32 bits.
    #[allow(clippy::unnecessary_cast)]
    let kind = s.f_type as u32;
    REMOTE.contains(&kind)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn remote_filesystem(_: &Path) -> bool {
    false
}

struct Walker<'a> {
    limits: Limits,
    started: Instant,
    scan: &'a ScanState,
    /// The home folder as written and with links resolved, and its last name.
    home: Option<(String, Option<String>, String)>,
    folders: usize,
    cut: Vec<Cut>,
    unreadable: Vec<String>,
    linked: Vec<Linked>,
    cancelled: bool,
    timed_out: bool,
}

impl<'a> Walker<'a> {
    fn new(home: Option<&Path>, scan: &'a ScanState, limits: Limits) -> Self {
        let home = home.map(|h| {
            let written = norm(h);
            let real = crate::paths::real_form(&shown(h));
            let name = crate::paths::base_name(&written).to_lowercase();
            (written, real, name)
        });
        Walker {
            limits,
            started: Instant::now(),
            scan,
            home,
            folders: 0,
            cut: Vec::new(),
            unreadable: Vec::new(),
            linked: Vec::new(),
            cancelled: false,
            timed_out: false,
        }
    }

    /// Is this folder the home folder — reached from any place, through any
    /// spelling? Links are resolved only for a folder with the home folder's
    /// own name, so this costs nothing for the others.
    fn is_home(&self, dir: &Path) -> bool {
        let Some((written, real, name)) = &self.home else { return false };
        let d = norm(dir);
        if &d == written {
            return true;
        }
        if crate::paths::base_name(&d).to_lowercase() != *name {
            return false;
        }
        match (real, crate::paths::real_form(&shown(dir))) {
            (Some(r), Some(dr)) => *r == dr,
            _ => false,
        }
    }

    fn under_home(&self, path: &Path) -> bool {
        let Some((written, real, _)) = &self.home else { return false };
        let p = norm(path);
        crate::paths::is_within(&p, written) || real.as_ref().is_some_and(|r| crate::paths::is_within(&p, r))
    }

    /// A linked folder near the home folder, offered as a place when it leads
    /// out of the home folder (inside it, the walk gets there anyway).
    fn offer_link(&mut self, link: &Path) {
        if self.linked.len() >= MAX_LINKED {
            return;
        }
        let Some(to) = crate::paths::real_form(&shown(link)) else { return };
        if !Path::new(&to).is_dir() || self.under_home(Path::new(&to)) || self.linked.iter().any(|l| l.to == to) {
            return;
        }
        self.linked.push(Linked { from: shown(link), to });
    }

    /// One place, breadth-first: the shallow folders first, so a place cut
    /// short has given what is nearest to its top.
    fn walk(&mut self, root: &Root, found: &mut Vec<(PathBuf, bool)>) {
        let label = shown(&root.path);
        self.scan.set(|p| {
            p.phase = "looking".into();
            p.place = label.clone();
        });
        let mut queue: VecDeque<(PathBuf, u32, Option<u64>)> = VecDeque::new();
        queue.push_back((root.path.clone(), 0, device_of(&root.path)));
        let mut here = 0usize;
        while let Some((dir, depth, device)) = queue.pop_front() {
            if self.scan.cancelled() {
                self.cancelled = true;
                return;
            }
            if self.started.elapsed() >= self.limits.time {
                self.timed_out = true;
                self.cut.push(Cut { place: label, why: format!("stopped after {} seconds", self.limits.time.as_secs()) });
                return;
            }
            if here >= self.limits.per_place {
                self.cut.push(Cut { place: label, why: format!("stopped after {} folders", self.limits.per_place) });
                return;
            }
            here += 1;
            self.folders += 1;
            if self.folders.is_multiple_of(256) {
                let (n, f) = (self.folders, found.len());
                self.scan.set(|p| {
                    p.folders = n;
                    p.found = f;
                });
            }
            // A repository ends the descent — except the home folder kept
            // in git (dotfiles), however it is reached: every clone is below it.
            let at_home = self.is_home(&dir);
            if dir.join(".git").exists() && !at_home {
                found.push((dir, false));
                continue;
            }
            if depth >= self.limits.depth {
                continue;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => {
                    if depth <= 1 {
                        self.unreadable.push(shown(&dir));
                    }
                    continue;
                }
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else { continue };
                let path = entry.path();
                // Not followed: on unix a link is its own kind of entry, and
                // on Windows both symbolic links and junctions are.
                if kind.is_symlink() {
                    if root.home && depth <= 1 {
                        self.offer_link(&path);
                    }
                    continue;
                }
                if !kind.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if (crate::repo_scan::noise_folder(&name) || (at_home && home_skip(&name))) && !path.join(".git").exists() {
                    continue;
                }
                #[cfg(windows)]
                if windows_skip(&entry, &path) {
                    continue;
                }
                let child_device = entry_device(&entry).or(device);
                if child_device != device && self.under_home(&path) && remote_filesystem(&path) {
                    continue;
                }
                queue.push_back((path, depth + 1, child_device));
            }
        }
    }
}

/// What `.git` is in a folder the walk took for a repository.
#[derive(Debug, PartialEq)]
enum DotGit {
    Dir,
    /// A file naming a git folder elsewhere: a worktree, a separate git dir.
    Elsewhere,
    /// A submodule's checkout: its superproject's concern, not a clone.
    Submodule,
}

fn dot_git(dir: &Path) -> DotGit {
    let p = dir.join(".git");
    match std::fs::symlink_metadata(&p) {
        Ok(m) if m.is_file() => {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let target = text
                .lines()
                .find_map(|l| l.trim().strip_prefix("gitdir:"))
                .map(|t| crate::paths::norm(t.trim()))
                .unwrap_or_default();
            if target.contains(".git/modules/") || target.ends_with(".git/modules") {
                DotGit::Submodule
            } else {
                DotGit::Elsewhere
            }
        }
        _ => DotGit::Dir,
    }
}

/// What the Repository page lists: its own walk of every profile folder
/// (`repo_scan::walk_repo_dirs`), as paths and with links resolved.
fn listed_set(profs: &[Profile]) -> (HashSet<String>, HashSet<String>) {
    let mut paths = HashSet::new();
    let mut reals = HashSet::new();
    for p in profs {
        for d in &p.directories {
            let dir = Path::new(d);
            if !dir.is_dir() {
                continue;
            }
            for r in crate::repo_scan::walk_repo_dirs(dir) {
                let s = shown(&r);
                if let Some(real) = crate::paths::real_form(&s) {
                    reals.insert(real);
                }
                paths.insert(crate::paths::norm(&s));
            }
        }
    }
    (paths, reals)
}

/// Walk the places and keep the repositories the Repository page does not
/// list. No git is run: that is for the ones kept.
pub(crate) fn search(home: Option<&Path>, extra: &[String], profs: &[Profile], scan: &ScanState, limits: Limits) -> Search {
    let mut out = Search::default();
    let roots = places(extra, profs, home, &mut out.rejected);
    let mut walker = Walker::new(home, scan, limits);
    let mut found: Vec<(PathBuf, bool)> = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        walker.walk(root, &mut found);
        if walker.cancelled {
            break;
        }
        if walker.timed_out {
            for r in &roots[i + 1..] {
                walker.cut.push(Cut { place: shown(&r.path), why: "not searched: the time limit was reached".into() });
            }
            break;
        }
    }
    out.cancelled = walker.cancelled;
    out.truncated = walker.cut;
    out.unreadable = walker.unreadable;
    out.linked = walker.linked.into_iter().filter(|l| !roots.iter().any(|r| within_either(&l.to, &r.path))).collect();
    out.roots = roots.into_iter().map(|r| r.path).collect();
    if out.cancelled {
        return out;
    }
    let (listed, listed_real) = listed_set(profs);
    let mut seen: HashSet<String> = HashSet::new();
    for (dir, _) in found {
        let s = shown(&dir);
        let written = crate::paths::norm(&s);
        let real = crate::paths::real_form(&s);
        // One folder reached from two places is one repository.
        if !seen.insert(real.clone().unwrap_or_else(|| written.clone())) {
            continue;
        }
        if listed.contains(&written) || real.as_ref().is_some_and(|r| listed_real.contains(r)) {
            out.already_listed += 1;
            continue;
        }
        let elsewhere = match dot_git(&dir) {
            DotGit::Submodule => continue,
            DotGit::Elsewhere => true,
            DotGit::Dir => false,
        };
        // A clone still writing the folder is not a repository yet.
        if crate::sparse::clone_running_at(&s) {
            continue;
        }
        out.candidates.push((dir, elsewhere));
    }
    out.candidates.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------------------
// Each repository kept: its remote, and whose email it commits with
// ---------------------------------------------------------------------------

/// The deepest profile folder that holds this path, as written or with links
/// resolved — git matches a folder rule both ways.
pub(crate) fn folder_owner<'a>(path: &str, profs: &'a [Profile]) -> Option<&'a Profile> {
    crate::commit_audit::owning_profile(path, profs).or_else(|| {
        let real = crate::paths::real_form(path)?;
        let mut best: Option<&Profile> = None;
        let mut best_len = 0usize;
        for p in profs {
            for d in &p.directories {
                if let Some(dr) = crate::paths::real_form(d) {
                    if crate::paths::is_within(&real, &dr) && dr.len() >= best_len {
                        best_len = dr.len();
                        best = Some(p);
                    }
                }
            }
        }
        best
    })
}

/// Which config file gives this repository its email, and in what scope
/// ("local", "worktree", "global", …). `--show-scope` is git 2.26; an older
/// git still says the file.
fn email_origin(dir: &Path) -> Option<(String, String)> {
    let said = crate::repo_scan::git_text(dir, &["config", "--show-scope", "--show-origin", "--get", "user.email"]);
    if let Some(line) = said.as_deref().and_then(|s| s.lines().next()) {
        let mut parts = line.splitn(3, '\t');
        let scope = parts.next()?.trim().to_string();
        let origin = parts.next()?.trim().trim_start_matches("file:").to_string();
        return Some((scope, origin));
    }
    let line = crate::repo_scan::git_text(dir, &["config", "--show-origin", "--get", "user.email"])?;
    let origin = line.split('\t').next()?.trim().trim_start_matches("file:").to_string();
    let n = crate::paths::norm(&origin);
    let scope = if !Path::new(&origin).is_absolute() || n.contains("/.git/") { "local" } else { "global" };
    Some((scope.to_string(), origin))
}

fn commits_as(dir: &Path, profs: &[Profile], elsewhere: bool) -> CommitsAs {
    let as_ = |p: Option<&Profile>, via: &str| CommitsAs { profile_id: p.map(|p| p.id.clone()), via: via.to_string() };
    if let Some(p) = folder_owner(&shown(dir), profs) {
        return as_(Some(p), "folder");
    }
    let default = profs.iter().find(|p| p.is_default);
    let fallback = || as_(default, if default.is_some() { "default" } else { "none" });
    match email_origin(dir) {
        Some((scope, _)) if scope == "local" || scope == "worktree" => as_(None, "local"),
        Some((_, file)) => match crate::profiles::profile_for_include_origin(&file, profs) {
            Some(p) if p.is_default => as_(Some(p), "default"),
            Some(p) => as_(Some(p), if elsewhere { "gitdir" } else { "rule" }),
            None => fallback(),
        },
        None => as_(None, "none"),
    }
}

fn describe(dir: &Path, elsewhere: bool, profs: &[Profile]) -> FoundRepo {
    let repo = crate::repo_scan::scanned_repo(dir);
    let (push_owner, push_name) = match repo.push_url.as_deref() {
        Some(url) => crate::repo_scan::parse_github_owner_name(url).unwrap_or_default(),
        None => (repo.owner.clone(), repo.name.clone()),
    };
    let commits_as = commits_as(dir, profs, elsewhere);
    FoundRepo { repo, push_owner, push_name, gitdir_elsewhere: elsewhere, commits_as }
}

/// The search, then git's word on each repository kept.
pub(crate) fn find_in(home: Option<&Path>, extra: &[String], profs: &[Profile], scan: &ScanState, limits: Limits) -> FoundClones {
    let s = search(home, extra, profs, scan, limits);
    let mut out = FoundClones {
        home: home.map(shown),
        roots: s.roots.iter().map(|r| shown(r)).collect(),
        repos: Vec::new(),
        already_listed: s.already_listed,
        truncated: s.truncated,
        unreadable: s.unreadable,
        rejected: s.rejected,
        linked: s.linked,
        cancelled: s.cancelled,
    };
    for (dir, elsewhere) in &s.candidates {
        if scan.cancelled() {
            out.cancelled = true;
            break;
        }
        out.repos.push(describe(dir, *elsewhere, profs));
        let n = out.repos.len();
        scan.set(|p| p.found = n);
    }
    out
}

pub async fn find_unlisted(scan_id: String, extra_roots: Vec<String>) -> Result<FoundClones, AppError> {
    let profs = crate::profiles::load_profiles()?.profiles;
    let state = scan(&scan_id);
    tokio::task::spawn_blocking(move || {
        let home = dirs::home_dir();
        find_in(home.as_deref(), &extra_roots, &profs, &state, Limits::default())
    })
    .await
    .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))
}

// ---------------------------------------------------------------------------
// What GitHub says each signed-in account may do with each repository
// ---------------------------------------------------------------------------

/// One account and one repository: "visible" (with what it may do and
/// whether only those it was shared with can read it), "invisible", or
/// "unknown" (why: SSO, rate limit, a token that cannot see private ones).
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AccessCell {
    pub state: String,
    pub push: bool,
    pub admin: bool,
    pub private: Option<bool>,
    pub why: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AccessAccount {
    /// As GitHub spells it.
    pub login: String,
    pub source: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AccountProblem {
    pub account: String,
    pub why: String,
}

#[derive(Debug, Serialize, Default)]
pub struct AccessMatrix {
    pub accounts: Vec<AccessAccount>,
    /// Accounts whose token could not be read or is not accepted.
    pub problems: Vec<AccountProblem>,
    /// "owner/name" (lowercase) -> login (lowercase) -> the answer.
    pub access: HashMap<String, HashMap<String, AccessCell>>,
    pub cancelled: bool,
}

/// One answer: the repository's key, the account's login, what GitHub said.
type Answer = (String, String, Result<(crate::github::RepoAccess, Option<bool>), AppError>);

fn cell(answer: Result<(crate::github::RepoAccess, Option<bool>), AppError>) -> AccessCell {
    use crate::github::RepoAccess;
    let unknown = |why: String| AccessCell { state: "unknown".into(), push: false, admin: false, private: None, why: Some(why) };
    match answer {
        Ok((RepoAccess::Visible(p), private)) => AccessCell { state: "visible".into(), push: p.push || p.admin, admin: p.admin, private, why: None },
        Ok((RepoAccess::Invisible, _)) => AccessCell { state: "invisible".into(), push: false, admin: false, private: None, why: None },
        Ok((RepoAccess::Unknown(why), _)) => unknown(why),
        Err(e) => unknown(e.to_string()),
    }
}

/// Every account GitSwitch can use (GitHub CLI logins and its own GitHub
/// sign-ins), each verified once, asked about every repository — four
/// questions at a time. The tokens never leave this process.
pub async fn access_matrix(scan_id: String, repos: Vec<String>) -> Result<AccessMatrix, AppError> {
    let state = scan(&scan_id);
    let mut out = AccessMatrix::default();
    let mut tokens: Vec<(String, String)> = Vec::new();
    for a in crate::remote_repos::accounts().await {
        if state.cancelled() {
            out.cancelled = true;
            return Ok(out);
        }
        let token = match crate::remote_repos::token_for(&a.key).await {
            Ok(t) => t,
            Err(e) => {
                out.problems.push(AccountProblem { account: a.label.clone(), why: e.to_string() });
                continue;
            }
        };
        match crate::github::verify_token(&token).await {
            Ok(user) => {
                let login = user.login.to_lowercase();
                if tokens.iter().any(|(l, _)| *l == login) {
                    continue;
                }
                tokens.push((login, token));
                out.accounts.push(AccessAccount { login: user.login, source: a.source.clone() });
            }
            Err(e) => out.problems.push(AccountProblem { account: a.label.clone(), why: e.to_string() }),
        }
    }
    let mut keys: Vec<(String, String, String)> = Vec::new();
    for r in repos {
        let Some((owner, name)) = r.split_once('/') else { continue };
        let key = r.to_lowercase();
        if owner.is_empty() || name.is_empty() || keys.iter().any(|(k, _, _)| *k == key) {
            continue;
        }
        keys.push((key, owner.to_string(), name.to_string()));
    }
    let mut jobs = Vec::new();
    for (key, owner, name) in &keys {
        for (login, token) in &tokens {
            jobs.push((key.clone(), owner.clone(), name.clone(), login.clone(), token.clone()));
        }
    }
    let total = jobs.len();
    state.set(|p| {
        p.phase = "checking".into();
        p.checked = 0;
        p.total = total;
    });
    let mut set: tokio::task::JoinSet<Answer> = tokio::task::JoinSet::new();
    let mut pending = jobs.into_iter();
    let start = |set: &mut tokio::task::JoinSet<Answer>, job: (String, String, String, String, String)| {
        set.spawn(async move {
            let (key, owner, name, login, token) = job;
            let answer = crate::github::repo_access_detail(&token, &owner, &name).await;
            (key, login, answer)
        });
    };
    for _ in 0..4 {
        if let Some(job) = pending.next() {
            start(&mut set, job);
        }
    }
    let mut done = 0usize;
    while let Some(joined) = set.join_next().await {
        done += 1;
        if let Ok((key, login, answer)) = joined {
            out.access.entry(key).or_default().insert(login, cell(answer));
        }
        state.set(|p| p.checked = done);
        if state.cancelled() {
            set.abort_all();
            out.cancelled = true;
            break;
        }
        if let Some(job) = pending.next() {
            start(&mut set, job);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Adding the folders found to their profiles, and taking them off again
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct FolderChange {
    pub profile_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct FolderAdded {
    pub profile_id: String,
    /// As the profile stores it.
    pub path: String,
    /// The email the repository's identity guard expected before, when it
    /// has a guard: Undo puts it back.
    pub guard_was: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct FolderFailed {
    pub profile_id: String,
    pub path: String,
    pub why: String,
}

#[derive(Debug, Serialize, Default)]
pub struct FoldersAdded {
    pub added: Vec<FolderAdded>,
    pub failed: Vec<FolderFailed>,
}

/// The email a repository's identity guard expects, when it has one.
pub(crate) fn guard_expects(repo: &str) -> Option<String> {
    let hooks = crate::commit_audit::hooks_dir(Path::new(repo)).ok()?;
    crate::commit_audit::guard_email_in(&hooks.dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gitswitch-find-{}-{}", tag, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        PathBuf::from(crate::paths::real_form(&shown(&d)).unwrap())
    }

    /// A folder git would take for a repository (the walk only looks for `.git`).
    fn repo(path: &Path) {
        std::fs::create_dir_all(path.join(".git")).unwrap();
    }

    fn prof(id: &str, dirs: &[&Path], default: bool) -> Profile {
        Profile {
            id: id.into(),
            name: id.into(),
            git_name: id.into(),
            git_email: format!("{}@example.test", id),
            ssh_key_path: None,
            is_default: default,
            allow_push: true,
            signing_enabled: false,
            directories: dirs.iter().map(|d| shown(d)).collect(),
            remote_patterns: vec![],
            github_login: None,
            push_over_ssh: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn names(s: &Search) -> Vec<String> {
        s.candidates.iter().map(|(p, _)| crate::paths::base_name(&shown(p))).collect()
    }

    fn run(home: &Path, extra: &[String], profs: &[Profile]) -> Search {
        search(Some(home), extra, profs, &ScanState::new(), Limits::default())
    }

    #[test]
    fn hidden_media_and_package_folders_are_passed_over_and_a_repository_ends_the_descent() {
        let t = tmp("skips");
        let home = t.join("home");
        repo(&home.join("code/app"));
        repo(&home.join("code/app/vendored/inner"));
        repo(&home.join(".hidden/dot"));
        repo(&home.join("code/node_modules/dep"));
        repo(&home.join("Movies/film"));
        repo(&home.join("MUSIC/song"));
        repo(&home.join("appdata/cache"));
        repo(&home.join("code/Pictures/kept"));
        // A noise-named folder that is itself a repository is still one.
        repo(&home.join("code/build"));
        let s = run(&home, &[], &[]);
        // In path order: `Pictures` sorts before `app`.
        assert_eq!(names(&s), vec!["kept", "app", "build"]);
        assert!(s.truncated.is_empty() && !s.cancelled);
    }

    #[test]
    fn five_levels_below_a_place_are_searched_and_no_deeper() {
        let t = tmp("depth");
        let home = t.join("home");
        repo(&home.join("a/b/c/d/five"));
        repo(&home.join("a/b/c/d/e/six"));
        assert_eq!(names(&run(&home, &[], &[])), vec!["five"]);
    }

    #[test]
    fn a_home_folder_kept_in_git_is_walked_and_not_offered() {
        let t = tmp("dotfiles");
        let home = t.join("home");
        repo(&home);
        repo(&home.join("code/app"));
        assert_eq!(names(&run(&home, &[], &[])), vec!["app"]);
    }

    #[test]
    fn a_place_that_is_a_repository_is_offered_and_not_entered() {
        let t = tmp("place-repo");
        let home = t.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let clone = t.join("elsewhere/clone");
        repo(&clone);
        repo(&clone.join("nested"));
        let s = run(&home, &[shown(&clone)], &[]);
        assert_eq!(names(&s), vec!["clone"]);
    }

    #[test]
    fn places_are_expanded_and_the_unusable_ones_say_why() {
        let t = tmp("places");
        let home = t.join("home");
        repo(&home.join("code/app"));
        let file = t.join("a-file");
        std::fs::write(&file, "x").unwrap();
        let extra = vec!["~/code".to_string(), "relative/x".into(), shown(&t.join("missing")), shown(&file), "  ".into()];
        let s = run(&home, &extra, &[]);
        // Found once, though both ~/code and the home folder hold it.
        assert_eq!(names(&s), vec!["app"]);
        assert_eq!(s.roots, vec![home.join("code"), home.clone()]);
        let why: Vec<(String, String)> = s.rejected.iter().map(|r| (r.place.clone(), r.why.clone())).collect();
        assert_eq!(
            why,
            vec![
                ("relative/x".to_string(), "not a full path".to_string()),
                (shown(&t.join("missing")), "not found".to_string()),
                (shown(&file), "not a folder".to_string()),
            ]
        );
    }

    #[test]
    fn the_folders_holding_profile_folders_outside_home_are_searched_but_never_a_disk_root() {
        let t = tmp("beside");
        let home = t.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let work = t.join("work");
        repo(&work.join("listed"));
        repo(&work.join("beside"));
        let profs = vec![prof("w", &[&work.join("listed")], false), prof("r", &[Path::new("/")], false)];
        let s = run(&home, &[], &profs);
        assert_eq!(s.roots, vec![work.clone(), home.clone()]);
        assert_eq!(names(&s), vec!["beside"]);
        assert_eq!(s.already_listed, 1);
    }

    #[test]
    fn what_the_repository_page_lists_is_left_out_by_the_same_walk() {
        let t = tmp("listed");
        let home = t.join("home");
        let mine = home.join("mine");
        repo(&mine.join("a"));
        repo(&mine.join("deep/x/y/z/w/v")); // six below the profile folder: the page does not list it
        repo(&home.join("other"));
        let profs = vec![prof("m", &[&mine], true)];
        // The page's own walk and this one agree on what is listed.
        let page: Vec<String> = crate::repo_scan::walk_repo_dirs(&mine).iter().map(|p| crate::paths::base_name(&shown(p))).collect();
        assert_eq!(page, vec!["a"]);
        let s = run(&home, &[shown(&mine.join("deep"))], &profs);
        assert_eq!(names(&s), vec!["v", "other"]);
        assert_eq!(s.already_listed, 1);
        // Inside a profile folder, the folder decides whose it is.
        let v = s.candidates.iter().find(|(p, _)| p.ends_with("v")).unwrap();
        assert_eq!(folder_owner(&shown(&v.0), &profs).map(|p| p.id.as_str()), Some("m"));
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed_but_offered_and_a_linked_profile_folder_still_counts_as_listed() {
        let t = tmp("links");
        let home = t.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let disk = t.join("disk/code");
        repo(&disk.join("far"));
        std::os::unix::fs::symlink(&disk, home.join("code")).unwrap();
        // A loop: never walked twice.
        std::os::unix::fs::symlink(&home, home.join("loop")).unwrap();
        let s = run(&home, &[], &[]);
        assert!(names(&s).is_empty());
        assert_eq!(s.linked, vec![Linked { from: shown(&home.join("code")), to: shown(&disk) }]);
        // A profile folder named through a link lists what is under its target.
        repo(&home.join("projects/near"));
        let link = t.join("via-link");
        std::os::unix::fs::symlink(home.join("projects"), &link).unwrap();
        let profs = vec![prof("p", &[&link], true)];
        let s = run(&home, &[], &profs);
        assert!(names(&s).is_empty(), "{:?}", names(&s));
        assert_eq!(s.already_listed, 1);
        // Searched as a place, the link's target is offered no more.
        let s = run(&home, &[shown(&disk)], &[]);
        assert_eq!(names(&s), vec!["far", "near"]);
        assert!(s.linked.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_is_not_followed_and_hidden_system_folders_are_passed_over() {
        let t = tmp("junction");
        let home = t.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let disk = t.join("disk");
        repo(&disk.join("far"));
        // cmd reads a `/` as a switch: backslashes for its built-ins.
        let win = |p: &Path| shown(p).replace('/', "\\");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(win(&home.join("code")))
            .arg(win(&disk))
            .output()
            .unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        let plumbing = home.join("plumbing");
        repo(&plumbing.join("inner"));
        let shown_only = home.join("shy");
        repo(&shown_only.join("seen"));
        for (dir, flags) in [(&plumbing, &["+h", "+s"][..]), (&shown_only, &["+h"][..])] {
            let st = std::process::Command::new("attrib").args(flags).arg(win(dir)).status().unwrap();
            assert!(st.success());
        }
        let s = run(&home, &[], &[]);
        assert_eq!(names(&s), vec!["seen"]);
        assert_eq!(s.linked.len(), 1);
        assert_eq!(crate::paths::norm(&s.linked[0].to), crate::paths::norm(&shown(&disk)));
    }

    #[test]
    fn worktrees_are_flagged_and_submodule_checkouts_left_out() {
        let t = tmp("dotgit");
        let home = t.join("home");
        let wt = home.join("code/feature");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: /somewhere/main/.git/worktrees/feature\n").unwrap();
        let sub = home.join("code/vendor-sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(".git"), "gitdir: ../../.git/modules/vendor-sub\n").unwrap();
        let s = run(&home, &[], &[]);
        assert_eq!(s.candidates, vec![(wt, true)]);
    }

    #[test]
    fn a_place_is_cut_short_at_its_folder_limit_and_the_time_limit_skips_the_rest() {
        let t = tmp("limits");
        let home = t.join("home");
        for i in 0..20 {
            std::fs::create_dir_all(home.join(format!("d{:02}", i))).unwrap();
        }
        repo(&home.join("d19/r"));
        let few = Limits { per_place: 5, ..Limits::default() };
        let s = search(Some(&home), &[], &[], &ScanState::new(), few);
        assert_eq!(s.truncated, vec![Cut { place: shown(&home), why: "stopped after 5 folders".into() }]);
        let place = t.join("first");
        std::fs::create_dir_all(&place).unwrap();
        let none = Limits { time: Duration::from_secs(0), ..Limits::default() };
        let s = search(Some(&home), &[shown(&place)], &[], &ScanState::new(), none);
        assert_eq!(
            s.truncated,
            vec![
                Cut { place: shown(&place), why: "stopped after 0 seconds".into() },
                Cut { place: shown(&home), why: "not searched: the time limit was reached".into() },
            ]
        );
        assert!(s.candidates.is_empty());
    }

    #[test]
    fn a_stopped_search_reports_nothing_and_says_so() {
        let t = tmp("cancel");
        let home = t.join("home");
        repo(&home.join("code/app"));
        let state = ScanState::new();
        state.stop();
        let s = search(Some(&home), &[], &[], &state, Limits::default());
        assert!(s.cancelled && s.candidates.is_empty());
    }

    #[test]
    fn a_stop_that_comes_first_still_stops_the_search_it_names() {
        cancel("early-stop");
        assert!(scan("early-stop").cancelled());
        assert!(!scan("another-search").cancelled());
    }
}
