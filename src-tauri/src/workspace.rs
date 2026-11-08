//! A workspace: one checkout of a project, told apart from every other
//! checkout of the same project on this machine.
//!
//! Two copies of one repository collide through what they share — a Compose
//! project name (the folder's), a port a dev server always takes — and an
//! agent working in copy 2 then finds copy 1's server "already running". Each
//! checkout gets an identity of its own: a short id kept in its own
//! `.git/config` (so a rename or move keeps it, and nothing is ever pushed),
//! a port offset, and for every copy but the first a Compose project name of
//! its own. The terminal drawer exports them into the shell, so every agent
//! and server started from it inherits them; the Repository page shows the
//! other copies, and warns when both are on one branch with changes.
//!
//! Deliberately not done: switching a branch, editing a port in the project,
//! renaming containers already running. Those are the person's decisions.

use crate::error::AppError;
use crate::git_exec::{GitCmd, READ_TIMEOUT};
use crate::profiles::{self, Profile};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

const KEY_ID: &str = "gitswitch.workspace";
const KEY_OFFSET: &str = "gitswitch.portOffset";
const KEY_COMPOSE: &str = "gitswitch.composeProject";
const OFFSET_STEP: u32 = 100;
pub const OFFSET_MAX: u32 = 60000;

/// The sentence every page repeats about what this does not do.
pub const NOT_AUTOMATED: &str = "GitSwitch only tells you about the other copy — it never switches a branch, edits a port in your project, or renames containers that are already running. Those are your decisions.";

// ---------------------------------------------------------------------------
// What the page gets
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WorkspaceCopy {
    pub path: String,
    /// The folder name, with a parent prepended while two copies share one.
    pub label: String,
    pub id: String,
    pub offset: u32,
    /// clone | worktree
    pub kind: String,
    pub is_this: bool,
    pub profile_name: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub unborn: bool,
    /// Tracked files changed (untracked left out); None when unreadable.
    pub changed: Option<usize>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WorkspaceNote {
    /// amber
    pub level: String,
    pub text: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct WorkspaceInfo {
    /// The workspace's top folder (a submodule resolves to its superproject's).
    pub path: String,
    pub name: String,
    pub id: String,
    /// clone | worktree
    pub kind: String,
    /// What was asked about, as given.
    pub asked_path: String,
    pub offset: u32,
    /// Set for every copy but the first; the first keeps Compose's own name.
    pub compose_project: Option<String>,
    /// "owner/name" for GitHub, else the remote's address; None without a remote.
    pub remote: Option<String>,
    /// Every copy of the same remote among the profile folders, this one included; by offset, then path.
    pub copies: Vec<WorkspaceCopy>,
    pub note: Option<WorkspaceNote>,
    /// What the drawer exports, as (name, value).
    pub env: Vec<(String, String)>,
    pub banner: String,
    pub warnings: Vec<String>,
    pub not_automated: String,
}

/// One remote's copies, for the Ports page's "copy 2 of 2".
#[derive(Serialize, Clone, Debug)]
pub struct Group {
    pub key: String,
    pub label: String,
    pub members: Vec<GroupMember>,
}

#[derive(Serialize, Clone, Debug)]
pub struct GroupMember {
    pub path: String,
    pub label: String,
    pub id: String,
    pub offset: u32,
    pub kind: String,
    /// 1-based, by offset.
    pub rank: usize,
}

// ---------------------------------------------------------------------------
// Pure pieces — unit-tested
// ---------------------------------------------------------------------------

/// Eight hex of SHA-256 over the normalised path: the lock's construction,
/// shortened for container names.
pub fn short_id(path: &str) -> String {
    gitswitch_lock_core::sha256_hex(crate::paths::norm(path).as_bytes())[..8].to_string()
}

fn valid_id(s: &str) -> bool {
    s.len() == 8 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// A Compose project name from a folder name: lowercase, `[a-z0-9_-]` only,
/// starting with a letter or digit, at most 40 characters, never empty.
pub fn compose_name(folder: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in folder.to_lowercase().chars() {
        let keep = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        let c = if keep { c } else { '-' };
        if c == '-' {
            if last_dash || out.is_empty() {
                continue;
            }
            last_dash = true;
        } else {
            last_dash = false;
        }
        if out.is_empty() && !(c.is_ascii_lowercase() || c.is_ascii_digit()) {
            continue;
        }
        out.push(c);
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "workspace".into()
    } else {
        out
    }
}

/// Two copies are of one project when their remotes name the same thing:
/// GitHub's owner/name whatever the address form, else the address itself.
pub fn remote_key(url: &str, owner: &str, name: &str) -> Option<String> {
    if !owner.is_empty() && !name.is_empty() {
        return Some(format!("github:{}/{}", owner.to_ascii_lowercase(), name.to_ascii_lowercase()));
    }
    let u = url.trim().to_ascii_lowercase();
    if u.is_empty() {
        return None;
    }
    let u = u.trim_end_matches('/');
    let u = u.strip_suffix(".git").unwrap_or(u);
    Some(format!("url:{}", u))
}

pub fn remote_label(url: &str, owner: &str, name: &str) -> Option<String> {
    if !owner.is_empty() && !name.is_empty() {
        return Some(format!("{}/{}", owner, name));
    }
    let u = url.trim();
    (!u.is_empty()).then(|| u.to_string())
}

/// One copy, as the assignment sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub path: String,
    pub stored_id: Option<String>,
    pub stored_offset: Option<u32>,
    /// When the checkout was made (its reflog's first line), for "the older copy keeps 0".
    pub first_seen: Option<i64>,
    /// A linked worktree: its id is derived from the main checkout's, never stored.
    pub worktree: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub path: String,
    pub id: String,
    pub offset: u32,
    /// The id must be written (it was missing, or a copy's duplicate).
    pub write_id: bool,
    pub write_offset: bool,
}

/// Which copy of one remote gets which offset. Stored values are kept; the
/// rest take the lowest free multiple of 100, oldest checkout first — so when
/// the app first meets an existing pair, the older copy gets 0 and keeps
/// its Compose name and data. A folder copied with its `.git` carries the
/// same stored id as its original: the copy whose path still hashes to that
/// id keeps it (it is where the id was minted), the other gets a fresh one.
pub fn assign(members: &[Member]) -> (Vec<Assignment>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut out: Vec<Assignment> = Vec::new();
    let mut by_id: HashMap<&str, Vec<&Member>> = HashMap::new();
    for m in members {
        if let Some(id) = &m.stored_id {
            by_id.entry(id.as_str()).or_default().push(m);
        }
    }
    let mut keeps_id: HashSet<&str> = HashSet::new();
    let mut ids: Vec<&&str> = by_id.keys().collect();
    ids.sort();
    for id in ids {
        let ms = &by_id[*id];
        if ms.len() == 1 {
            keeps_id.insert(ms[0].path.as_str());
            continue;
        }
        let keeper = ms
            .iter()
            .find(|m| short_id(&m.path) == **id)
            .or_else(|| ms.iter().min_by(|a, b| a.path.cmp(&b.path)))
            .map(|m| m.path.as_str())
            .unwrap_or_default();
        keeps_id.insert(keeper);
        for m in ms.iter().filter(|m| m.path != keeper) {
            warnings.push(format!(
                "{} carried the same workspace id as {} (a folder copied with its .git); it was given its own.",
                crate::paths::base_name(&m.path),
                crate::paths::base_name(keeper)
            ));
        }
    }
    let keeps = |m: &Member| m.stored_id.is_some() && keeps_id.contains(m.path.as_str());
    let mut taken: HashSet<u32> = HashSet::new();
    let mut pending: Vec<&Member> = Vec::new();
    for m in members {
        match (keeps(m), m.stored_offset) {
            (true, Some(off)) if !taken.contains(&off) => {
                taken.insert(off);
                out.push(Assignment { path: m.path.clone(), id: m.stored_id.clone().unwrap_or_default(), offset: off, write_id: false, write_offset: false });
            }
            _ => pending.push(m),
        }
    }
    // Oldest first; unknown ages last; ties by path so the result is stable.
    pending.sort_by(|a, b| match (a.first_seen, b.first_seen) {
        (Some(x), Some(y)) => x.cmp(&y).then(a.path.cmp(&b.path)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.path.cmp(&b.path),
    });
    for m in pending {
        let mut off = 0;
        while taken.contains(&off) {
            off += OFFSET_STEP;
        }
        taken.insert(off);
        let kept = keeps(m);
        let id = if kept { m.stored_id.clone().unwrap_or_default() } else { short_id(&m.path) };
        out.push(Assignment { path: m.path.clone(), id, offset: off, write_id: !kept && !m.worktree, write_offset: m.stored_offset != Some(off) });
    }
    (out, warnings)
}

/// Folder names, with one more parent prepended while two collide.
pub fn distinct_labels(paths: &[String]) -> Vec<String> {
    let parts: Vec<Vec<String>> = paths.iter().map(|p| crate::paths::norm(p).split('/').filter(|s| !s.is_empty()).map(String::from).collect()).collect();
    let mut depth = 1;
    loop {
        let labels: Vec<String> = parts.iter().map(|p| p[p.len().saturating_sub(depth)..].join("/")).collect();
        let distinct = labels.iter().collect::<HashSet<_>>().len() == labels.len();
        let exhausted = parts.iter().all(|p| p.len() <= depth);
        if distinct || exhausted {
            return labels;
        }
        depth += 1;
    }
}

/// The amber sentence: only when two or more copies have the same branch
/// checked out and each has tracked changes.
pub fn note(copies: &[WorkspaceCopy]) -> Option<WorkspaceNote> {
    let mut by_branch: BTreeMap<&str, Vec<&WorkspaceCopy>> = BTreeMap::new();
    for c in copies {
        if c.detached || c.unborn {
            continue;
        }
        if let (Some(b), Some(n)) = (c.branch.as_deref(), c.changed) {
            if n > 0 {
                by_branch.entry(b).or_default().push(c);
            }
        }
    }
    let (branch, cs) = by_branch.into_iter().find(|(_, cs)| cs.len() >= 2)?;
    let who = cs
        .iter()
        .map(|c| format!("{}: {} file{}", c.label, c.changed.unwrap_or(0), if c.changed == Some(1) { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(", ");
    let lead = if cs.len() == 2 { "Both copies".to_string() } else { format!("{} copies", cs.len()) };
    Some(WorkspaceNote {
        level: "amber".into(),
        text: format!(
            "{} have {} checked out with uncommitted changes ({}). Commits made in both will diverge and the second push will be refused — work on a branch per copy.",
            lead, branch, who
        ),
    })
}

/// The drawer's one dim line at the shell's start.
pub fn banner(id: &str, name: &str, offset: u32, compose: Option<&str>) -> String {
    let mut s = format!("GitSwitch workspace #{} · {} · port offset {} ($GITSWITCH_PORT_OFFSET)", id, name, offset);
    if let Some(c) = compose {
        s.push_str(&format!(" · Compose project {} (this copy's own)", c));
    }
    s
}

/// The unix time in the first line of a reflog: `<old> <new> <who> <ts> <tz>\t<msg>`.
pub fn parse_reflog_time(first_line: &str) -> Option<i64> {
    let head = first_line.split('\t').next()?;
    let toks: Vec<&str> = head.split_whitespace().collect();
    // The timestamp is the second-to-last token; the name may hold spaces.
    toks.get(toks.len().checked_sub(2)?)?.parse().ok()
}

fn env_pairs(id: &str, top: &str, name: &str, offset: u32, compose: Option<&str>) -> Vec<(String, String)> {
    let mut v = vec![
        ("GITSWITCH_WORKSPACE".to_string(), id.to_string()),
        ("GITSWITCH_WORKSPACE_PATH".to_string(), top.to_string()),
        ("GITSWITCH_WORKSPACE_NAME".to_string(), name.to_string()),
        ("GITSWITCH_PORT_OFFSET".to_string(), offset.to_string()),
    ];
    if let Some(c) = compose {
        v.push(("COMPOSE_PROJECT_NAME".to_string(), c.to_string()));
    }
    v
}

/// "copy 2 of 2" for a folder, from the groups: (rank, of, the other copies' labels).
pub fn copy_of(path: &str, groups: &[Group]) -> Option<(usize, usize, Vec<String>)> {
    let want = crate::paths::real_form(path).unwrap_or_else(|| crate::paths::norm(path));
    for g in groups {
        if let Some(me) = g.members.iter().find(|m| m.path == want) {
            let others = g.members.iter().filter(|m| m.path != want).map(|m| m.label.clone()).collect();
            return Some((me.rank, g.members.len(), others));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

/// What a folder is, in git's eyes.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub top: String,
    pub gitdir: String,
    pub commondir: String,
    pub worktree: bool,
    /// The worktree's name (its folder under `worktrees/`), for a linked worktree.
    pub wt_name: Option<String>,
}

fn sync_git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = crate::spawn::command("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn real(p: &str) -> String {
    crate::paths::real_form(p).unwrap_or_else(|| crate::paths::norm(p))
}

/// One `rev-parse` answers everything: the top folder, the git dir, the
/// common dir (differs for a linked worktree) and the superproject when the
/// folder is a submodule checkout — which resolves to its superproject.
pub fn resolve(path: &str) -> Result<Resolved, AppError> {
    let mut dir = PathBuf::from(path);
    if !dir.is_dir() {
        return Err(AppError::NotFound(format!("{} no longer exists on disk", path)));
    }
    for _ in 0..8 {
        let Some(out) = sync_git(&dir, &["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-dir", "--git-common-dir", "--show-superproject-working-tree"]) else {
            let bare = sync_git(&dir, &["rev-parse", "--is-bare-repository"]).is_some_and(|s| s.trim() == "true");
            return Err(AppError::NotFound(if bare {
                format!("{} is a bare repository: nothing runs from it, so it has no workspace.", path)
            } else {
                format!("{} is not inside a git repository.", path)
            }));
        };
        let lines: Vec<String> = out.lines().map(|l| crate::paths::norm(l.trim())).collect();
        if lines.len() < 3 || lines[0].is_empty() {
            return Err(AppError::NotFound(format!("{} is not inside a git working tree.", path)));
        }
        if let Some(sup) = lines.get(3).filter(|s| !s.is_empty()) {
            dir = PathBuf::from(sup);
            continue;
        }
        let top = real(&lines[0]);
        let gitdir = real(&lines[1]);
        let commondir = real(&lines[2]);
        let worktree = gitdir != commondir;
        let wt_name = worktree.then(|| crate::paths::base_name(&gitdir));
        return Ok(Resolved { top, gitdir, commondir, worktree, wt_name });
    }
    Err(AppError::NotFound(format!("{}: submodules nested deeper than eight levels.", path)))
}

/// Every `gitswitch.*` key in a repository's local config, keys lowercased
/// (git prints section and key names in lowercase; a worktree's name keeps
/// its case, and is compared lowercased too).
fn read_keys(top: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(text) = sync_git(top, &["config", "--local", "-z", "--get-regexp", r"^gitswitch\."]) {
        for rec in text.split('\0').filter(|r| !r.is_empty()) {
            let (k, v) = rec.split_once('\n').unwrap_or((rec, ""));
            out.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    out
}

fn write_key(top: &Path, key: &str, value: &str) -> Result<(), String> {
    let out = crate::spawn::command("git").arg("-C").arg(top).args(["config", "--local", key, value]).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn unset_key(top: &Path, key: &str) {
    let _ = crate::spawn::command("git").arg("-C").arg(top).args(["config", "--local", "--unset", key]).output();
}

/// The keys of one checkout: (id, offset, compose). A worktree's offset and
/// Compose name live in the common config under its own name; its id is the
/// main checkout's, from which its own is derived.
fn keys_for(r: &Resolved) -> (String, String, String) {
    match &r.wt_name {
        Some(n) => (KEY_ID.into(), format!("gitswitch.worktree.{}.portOffset", n), format!("gitswitch.worktree.{}.composeProject", n)),
        None => (KEY_ID.into(), KEY_OFFSET.into(), KEY_COMPOSE.into()),
    }
}

fn first_seen(gitdir: &str) -> Option<i64> {
    let text = std::fs::read_to_string(Path::new(gitdir).join("logs").join("HEAD")).ok()?;
    parse_reflog_time(text.lines().next()?)
}

/// A worktree's id: the main checkout's id plus the worktree's name.
fn worktree_id(r: &Resolved, main_id: &str) -> String {
    short_id(&format!("{}/worktrees/{}", main_id, r.wt_name.as_deref().unwrap_or("")))
}

/// What is stored for one checkout: (id, offset, compose). The id is this
/// checkout's own — derived for a worktree.
fn stored(r: &Resolved) -> (Option<String>, Option<u32>, Option<String>) {
    let keys = read_keys(Path::new(&r.top));
    let (kid, koff, kcomp) = keys_for(r);
    let main_id = keys.get(&kid.to_ascii_lowercase()).cloned().filter(|v| valid_id(v));
    let id = match (main_id, r.worktree) {
        (Some(m), true) => Some(worktree_id(r, &m)),
        (m, _) => m,
    };
    let off = keys.get(&koff.to_ascii_lowercase()).and_then(|v| v.parse::<u32>().ok()).filter(|o| *o <= OFFSET_MAX);
    let comp = keys.get(&kcomp.to_ascii_lowercase()).cloned().filter(|v| !v.is_empty());
    (id, off, comp)
}

/// The origin of a checkout: (url, owner, name), the url with any token masked.
fn origin_of(top: &Path) -> Option<(String, String, String)> {
    let url = crate::repo_scan::get_origin(top)?;
    let (owner, name) = crate::repo_scan::parse_github_owner_name(&url).unwrap_or_default();
    Some((crate::repo_scan::mask_token(&url), owner, name))
}

// ---------------------------------------------------------------------------
// The whole picture for one checkout
// ---------------------------------------------------------------------------

struct Settled {
    id: String,
    offset: u32,
    compose: Option<String>,
    warnings: Vec<String>,
}

/// Decide and store this checkout's identity among the copies of its remote.
/// Writes only to repositories of the same remote, and only what is missing.
fn settle(r: &Resolved, siblings: &[String]) -> Settled {
    let mut warnings = Vec::new();
    // This checkout and every sibling, each resolved, one entry per working tree.
    let mut members: Vec<(Resolved, Member)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for p in std::iter::once(r.top.clone()).chain(siblings.iter().cloned()) {
        let Ok(res) = resolve(&p) else { continue };
        if !seen.insert(res.top.clone()) {
            continue;
        }
        let (sid, soff, _) = stored(&res);
        let fs = first_seen(&res.gitdir);
        members.push((res.clone(), Member { path: res.top.clone(), stored_id: sid, stored_offset: soff, first_seen: fs, worktree: res.worktree }));
    }
    let (assignments, warns) = assign(&members.iter().map(|(_, m)| m.clone()).collect::<Vec<_>>());
    warnings.extend(warns);
    let assigned = |path: &str| assignments.iter().find(|a| a.path == path);
    let mut mine = Settled { id: short_id(&r.top), offset: 0, compose: None, warnings: Vec::new() };
    for (res, _) in &members {
        let Some(a) = assigned(&res.top) else { continue };
        let top = Path::new(&res.top);
        let (kid, koff, kcomp) = keys_for(res);
        // A worktree's id follows its main checkout's — the one assigned in
        // this same pass when the main is among the copies.
        let id = if res.worktree {
            let main_id = members
                .iter()
                .find(|(m, _)| !m.worktree && m.gitdir == res.commondir)
                .and_then(|(m, _)| assigned(&m.top))
                .map(|a| a.id.clone())
                .or_else(|| read_keys(top).get(&kid.to_ascii_lowercase()).cloned().filter(|v| valid_id(v)))
                .unwrap_or_else(|| short_id(&res.commondir));
            worktree_id(res, &main_id)
        } else {
            a.id.clone()
        };
        if a.write_id && !res.worktree {
            if let Err(e) = write_key(top, &kid, &a.id) {
                warnings.push(format!("Could not write {}/.git/config ({}); this copy's workspace id follows its folder until it can be saved.", res.top, e));
            }
        }
        if a.write_offset {
            if let Err(e) = write_key(top, &koff, &a.offset.to_string()) {
                warnings.push(format!("Could not store the port offset of {}: {}", res.top, e));
            }
        }
        // The Compose name: stored for every copy but the first, removed for
        // the first. A copy that was just given its own id drops the name it
        // carried over from its original — that name is the original's.
        let (_, _, scomp) = stored(res);
        let compose = if a.offset > 0 {
            match scomp {
                Some(c) if !a.write_id => Some(c),
                _ => {
                    let c = format!("{}-{}", compose_name(&crate::paths::base_name(&res.top)), id);
                    if let Err(e) = write_key(top, &kcomp, &c) {
                        warnings.push(format!("Could not store the Compose project name of {}: {}", res.top, e));
                    }
                    Some(c)
                }
            }
        } else {
            if scomp.is_some() {
                unset_key(top, &kcomp);
            }
            None
        };
        if res.top == r.top {
            mine = Settled { id, offset: a.offset, compose, warnings: Vec::new() };
        }
    }
    mine.warnings = warnings;
    mine
}

/// The sibling checkouts of a remote among the profile folders: (key, label, paths).
async fn siblings_of(top: &str, profs: &[Profile], fresh: bool) -> (Option<String>, Option<String>, Vec<String>) {
    let Some((url, owner, name)) = origin_of(Path::new(top)) else { return (None, None, Vec::new()) };
    let key = remote_key(&url, &owner, &name);
    let label = remote_label(&url, &owner, &name);
    let Some(key) = key else { return (None, label, Vec::new()) };
    let idx = crate::remote_repos::local_index(profs, fresh).await;
    let mut sibs: Vec<String> = idx.by_remote.get(&key).cloned().unwrap_or_default();
    let me = crate::paths::norm(top);
    sibs.retain(|p| real(p) != me);
    (Some(key), label, sibs)
}

fn id_of(res: &Resolved) -> String {
    stored(res).0.unwrap_or_else(|| short_id(&res.top))
}

/// This checkout's identity, its copies, and the amber note — the Repository
/// page's read. The profile folders are scanned afresh only when this
/// checkout has no keys yet (a new copy must find its siblings to take the
/// next offset); otherwise the minute-old index serves.
pub async fn info(path: &str) -> Result<WorkspaceInfo, AppError> {
    let r = resolve(path)?;
    let store = profiles::load_profiles().map(|s| s.profiles).unwrap_or_default();
    let (sid, soff, _) = stored(&r);
    let (_, remote, sibs) = siblings_of(&r.top, &store, sid.is_none() || soff.is_none()).await;
    let settled = {
        let r = r.clone();
        let sibs = sibs.clone();
        tokio::task::spawn_blocking(move || settle(&r, &sibs)).await.map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?
    };
    // Every copy, read lock-free.
    let mut copies = Vec::new();
    let mut resolved: Vec<Resolved> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for p in std::iter::once(r.top.clone()).chain(sibs.iter().cloned()) {
        let Ok(res) = resolve(&p) else { continue };
        if seen.insert(res.top.clone()) {
            resolved.push(res);
        }
    }
    resolved.sort_by(|a, b| a.top.cmp(&b.top));
    let labels = distinct_labels(&resolved.iter().map(|x| x.top.clone()).collect::<Vec<_>>());
    for (res, label) in resolved.iter().zip(labels) {
        let (_, soff, _) = stored(res);
        let st = GitCmd::at(&res.top)
            .top_flag("--no-optional-locks")
            .args(["status", "--porcelain=v2", "--branch", "--untracked-files=no", "--ignore-submodules=all", "-z"])
            .timeout(READ_TIMEOUT)
            .run()
            .await
            .ok()
            .filter(|o| o.ok())
            .map(|o| crate::git_status::parse_porcelain_v2(&o.stdout));
        let profile_name = crate::commit_audit::owning_profile(&res.top, &store).map(|p| p.name.clone());
        copies.push(WorkspaceCopy {
            path: res.top.clone(),
            label,
            id: id_of(res),
            offset: soff.unwrap_or(0),
            kind: if res.worktree { "worktree".into() } else { "clone".into() },
            is_this: res.top == r.top,
            profile_name,
            branch: st.as_ref().and_then(|s| s.branch.clone()),
            detached: st.as_ref().is_some_and(|s| s.detached),
            unborn: st.as_ref().is_some_and(|s| s.unborn),
            changed: st.as_ref().map(|s| s.entries.iter().filter(|e| e.kind == "tracked" || e.kind == "conflicted").count()),
        });
    }
    copies.sort_by(|a, b| a.offset.cmp(&b.offset).then(a.path.cmp(&b.path)));
    let name = crate::paths::base_name(&r.top);
    let env = env_pairs(&settled.id, &r.top, &name, settled.offset, settled.compose.as_deref());
    let note = note(&copies);
    Ok(WorkspaceInfo {
        path: r.top.clone(),
        name: name.clone(),
        id: settled.id.clone(),
        kind: if r.worktree { "worktree".into() } else { "clone".into() },
        asked_path: path.to_string(),
        offset: settled.offset,
        compose_project: settled.compose.clone(),
        remote,
        copies,
        note,
        env,
        banner: banner(&settled.id, &name, settled.offset, settled.compose.as_deref()),
        warnings: settled.warnings,
        not_automated: NOT_AUTOMATED.into(),
    })
}

/// What the drawer exports, and the banner.
#[derive(Serialize, Clone, Debug)]
pub struct WorkspaceEnv {
    pub env: Vec<(String, String)>,
    pub banner: String,
}

/// What the drawer exports — never fails: a folder that is no repository
/// still gets a shell, with an id from its path and nothing else. One config
/// read, no scan.
pub fn env_for(path: &str) -> WorkspaceEnv {
    let fallback = |p: &str| {
        let id = short_id(p);
        let name = crate::paths::base_name(p);
        WorkspaceEnv { env: env_pairs(&id, &crate::paths::norm(p), &name, 0, None), banner: banner(&id, &name, 0, None) }
    };
    let Ok(r) = resolve(path) else { return fallback(path) };
    let (_, soff, scomp) = stored(&r);
    let id = id_of(&r);
    let offset = soff.unwrap_or(0);
    let compose = if offset > 0 { scomp } else { None };
    let name = crate::paths::base_name(&r.top);
    WorkspaceEnv { env: env_pairs(&id, &r.top, &name, offset, compose.as_deref()), banner: banner(&id, &name, offset, compose.as_deref()) }
}

/// Before a shell starts: give the checkout its keys if it has none yet
/// (one config read; the scan only when something is missing).
pub async fn ensure_assigned(path: &str) {
    let Ok(r) = resolve(path) else { return };
    let (sid, soff, _) = stored(&r);
    if sid.is_some() && soff.is_some() {
        return;
    }
    let _ = info(path).await;
}

/// The person chose an offset for this checkout.
pub async fn set_offset(path: &str, offset: u32) -> Result<WorkspaceInfo, AppError> {
    if offset > OFFSET_MAX {
        return Err(AppError::Config(format!("A port offset is a whole number from 0 to {}, usually a multiple of 100.", OFFSET_MAX)));
    }
    let r = resolve(path)?;
    let store = profiles::load_profiles().map(|s| s.profiles).unwrap_or_default();
    let (_, _, sibs) = siblings_of(&r.top, &store, true).await;
    for s in &sibs {
        let Ok(res) = resolve(s) else { continue };
        if res.top == r.top {
            continue;
        }
        let (_, soff, _) = stored(&res);
        if soff == Some(offset) {
            return Err(AppError::Config(format!(
                "{} already uses port offset {}. Pick another, or change it there first.",
                crate::paths::base_name(&res.top),
                offset
            )));
        }
    }
    let top = Path::new(&r.top);
    let (kid, koff, kcomp) = keys_for(&r);
    let (sid, _, _) = stored(&r);
    let id = match sid {
        Some(s) => s,
        None if r.worktree => id_of(&r),
        None => {
            let fresh = short_id(&r.top);
            write_key(top, &kid, &fresh).map_err(|e| AppError::Command(format!("Could not write {}/.git/config: {}", r.top, e)))?;
            fresh
        }
    };
    write_key(top, &koff, &offset.to_string()).map_err(|e| AppError::Command(format!("Could not store the port offset: {}", e)))?;
    if offset > 0 {
        let c = format!("{}-{}", compose_name(&crate::paths::base_name(&r.top)), id);
        write_key(top, &kcomp, &c).map_err(|e| AppError::Command(format!("Could not store the Compose project name: {}", e)))?;
    } else {
        unset_key(top, &kcomp);
    }
    info(path).await
}

/// Every remote with two or more copies among the profile folders, for the
/// Ports page. Served from the index as it stands (refreshed in the
/// background when older than a minute): a port listing must not wait for a
/// scan.
pub async fn groups() -> Vec<Group> {
    let store = profiles::load_profiles().map(|s| s.profiles).unwrap_or_default();
    let idx = crate::remote_repos::local_index_stale_ok(&store).await;
    let mut out = Vec::new();
    for (key, paths) in &idx.by_remote {
        if paths.len() < 2 {
            continue;
        }
        let mut members: Vec<GroupMember> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut resolved: Vec<Resolved> = Vec::new();
        for p in paths {
            let Ok(res) = resolve(p) else { continue };
            if seen.insert(res.top.clone()) {
                resolved.push(res);
            }
        }
        if resolved.len() < 2 {
            continue;
        }
        let labels = distinct_labels(&resolved.iter().map(|x| x.top.clone()).collect::<Vec<_>>());
        for (res, label) in resolved.iter().zip(labels) {
            let (_, soff, _) = stored(res);
            members.push(GroupMember { path: res.top.clone(), label, id: id_of(res), offset: soff.unwrap_or(0), kind: if res.worktree { "worktree".into() } else { "clone".into() }, rank: 0 });
        }
        members.sort_by(|a, b| a.offset.cmp(&b.offset).then(a.path.cmp(&b.path)));
        for (i, m) in members.iter_mut().enumerate() {
            m.rank = i + 1;
        }
        let label = key.strip_prefix("github:").or_else(|| key.strip_prefix("url:")).unwrap_or(key).to_string();
        out.push(Group { key: key.clone(), label, members });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_short_and_ignore_a_trailing_slash() {
        let a = short_id("/Users/me/work/argos");
        assert_eq!(a.len(), 8);
        assert!(valid_id(&a));
        assert_eq!(a, short_id("/Users/me/work/argos/"));
        assert_ne!(a, short_id("/Users/me/work/argos2"));
        assert!(!valid_id("ABCDEF01"), "git stores what it is given; only our own lowercase form is trusted");
    }

    #[test]
    fn compose_names_are_what_compose_accepts() {
        assert_eq!(compose_name("Argos_08-10-26"), "argos_08-10-26");
        assert_eq!(compose_name("My App (v2)"), "my-app-v2");
        assert_eq!(compose_name(".hidden"), "hidden");
        assert_eq!(compose_name("日本語"), "workspace");
        assert_eq!(compose_name("---"), "workspace");
        assert!(compose_name(&"x".repeat(100)).len() <= 40);
    }

    #[test]
    fn remote_keys_group_the_same_project_whatever_the_address() {
        assert_eq!(remote_key("git@github.com:Acme/Argos.git", "Acme", "Argos").as_deref(), Some("github:acme/argos"));
        assert_eq!(remote_key("https://github.com/acme/argos", "acme", "argos").as_deref(), Some("github:acme/argos"));
        assert_eq!(remote_key("https://gitlab.example/x/y.git/", "", "").as_deref(), Some("url:https://gitlab.example/x/y"));
        assert_eq!(remote_key("", "", ""), None);
        assert_eq!(remote_label("git@github.com:Acme/Argos.git", "Acme", "Argos").as_deref(), Some("Acme/Argos"));
    }

    fn m(path: &str, id: Option<&str>, off: Option<u32>, seen: Option<i64>) -> Member {
        Member { path: path.into(), stored_id: id.map(String::from), stored_offset: off, first_seen: seen, worktree: false }
    }

    #[test]
    fn the_older_copy_keeps_zero_and_the_rest_take_the_next_hundreds() {
        let (a, w) = assign(&[m("/w/newer", None, None, Some(200)), m("/w/older", None, None, Some(100)), m("/w/unknown", None, None, None)]);
        assert!(w.is_empty());
        let off = |p: &str| a.iter().find(|x| x.path == p).unwrap().offset;
        assert_eq!(off("/w/older"), 0);
        assert_eq!(off("/w/newer"), 100);
        assert_eq!(off("/w/unknown"), 200);
        assert!(a.iter().all(|x| x.write_id && x.write_offset));
    }

    #[test]
    fn stored_values_are_kept_and_gaps_are_filled() {
        let (a, _) = assign(&[m("/w/a", Some("aaaaaaaa"), Some(100), Some(1)), m("/w/b", Some("bbbbbbbb"), Some(300), Some(2)), m("/w/c", None, None, Some(3))]);
        let get = |p: &str| a.iter().find(|x| x.path == p).unwrap();
        assert_eq!((get("/w/a").offset, get("/w/a").write_id, get("/w/a").write_offset), (100, false, false));
        assert_eq!(get("/w/b").offset, 300);
        assert_eq!(get("/w/c").offset, 0, "the lowest free multiple of 100");
    }

    #[test]
    fn a_folder_copied_with_its_git_gets_its_own_identity() {
        let orig = "/w/argos";
        let id = short_id(orig);
        let (a, w) = assign(&[m("/w/argos-copy", Some(&id), Some(0), Some(5)), m(orig, Some(&id), Some(0), Some(1))]);
        let get = |p: &str| a.iter().find(|x| x.path == p).unwrap();
        assert_eq!((get(orig).id.as_str(), get(orig).offset, get(orig).write_id), (id.as_str(), 0, false));
        assert_ne!(get("/w/argos-copy").id, id);
        assert_eq!(get("/w/argos-copy").offset, 100);
        assert!(get("/w/argos-copy").write_id);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("argos-copy carried the same workspace id as argos"), "{}", w[0]);
    }

    #[test]
    fn a_worktree_is_never_written_an_id() {
        let (a, _) = assign(&[m("/w/main", Some("aaaaaaaa"), Some(0), Some(1)), Member { path: "/w/wt".into(), stored_id: Some("cccccccc".into()), stored_offset: None, first_seen: Some(2), worktree: true }]);
        let wt = a.iter().find(|x| x.path == "/w/wt").unwrap();
        assert_eq!((wt.offset, wt.write_id, wt.write_offset), (100, false, true));
    }

    #[test]
    fn labels_grow_a_parent_only_while_they_collide() {
        assert_eq!(distinct_labels(&["/a/x/argos".into(), "/b/y/argos".into()]), vec!["x/argos", "y/argos"]);
        assert_eq!(distinct_labels(&["/a/argos".into(), "/a/argos2".into()]), vec!["argos", "argos2"]);
        assert_eq!(distinct_labels(&["C:/w/argos".into(), "D:/w/argos".into()]), vec!["C:/w/argos", "D:/w/argos"]);
    }

    fn copy(label: &str, branch: Option<&str>, changed: Option<usize>, detached: bool) -> WorkspaceCopy {
        WorkspaceCopy { path: format!("/w/{}", label), label: label.into(), id: "x".into(), offset: 0, kind: "clone".into(), is_this: false, profile_name: None, branch: branch.map(String::from), detached, unborn: false, changed }
    }

    #[test]
    fn the_amber_note_needs_one_branch_in_two_copies_with_changes_in_both() {
        let n = note(&[copy("A", Some("main"), Some(3), false), copy("B", Some("main"), Some(1), false)]).unwrap();
        assert_eq!(n.level, "amber");
        assert_eq!(n.text, "Both copies have main checked out with uncommitted changes (A: 3 files, B: 1 file). Commits made in both will diverge and the second push will be refused — work on a branch per copy.");
        assert!(note(&[copy("A", Some("main"), Some(3), false), copy("B", Some("feature"), Some(1), false)]).is_none(), "different branches");
        assert!(note(&[copy("A", Some("main"), Some(3), false), copy("B", Some("main"), Some(0), false)]).is_none(), "one is clean");
        assert!(note(&[copy("A", Some("main"), Some(3), true), copy("B", Some("main"), Some(1), false)]).is_none(), "detached does not count");
        assert!(note(&[copy("A", Some("main"), Some(1), false), copy("B", Some("main"), Some(1), false), copy("C", Some("main"), Some(2), false)]).unwrap().text.starts_with("3 copies have main"));
    }

    #[test]
    fn the_banner_says_what_the_shell_got() {
        assert_eq!(banner("2f9c1a7b", "Argos_08-10-26", 100, Some("argos_08-10-26-2f9c1a7b")), "GitSwitch workspace #2f9c1a7b · Argos_08-10-26 · port offset 100 ($GITSWITCH_PORT_OFFSET) · Compose project argos_08-10-26-2f9c1a7b (this copy's own)");
        assert_eq!(banner("a1b2c3d4", "Argos_06-10-26_V1", 0, None), "GitSwitch workspace #a1b2c3d4 · Argos_06-10-26_V1 · port offset 0 ($GITSWITCH_PORT_OFFSET)");
    }

    #[test]
    fn the_env_names_the_compose_project_only_for_a_later_copy() {
        let first = env_pairs("a1b2c3d4", "/w/argos", "argos", 0, None);
        assert!(!first.iter().any(|(k, _)| k == "COMPOSE_PROJECT_NAME"));
        assert_eq!(first.iter().find(|(k, _)| k == "GITSWITCH_PORT_OFFSET").map(|(_, v)| v.as_str()), Some("0"));
        let second = env_pairs("2f9c1a7b", "/w/argos2", "argos2", 100, Some("argos2-2f9c1a7b"));
        assert_eq!(second.iter().find(|(k, _)| k == "COMPOSE_PROJECT_NAME").map(|(_, v)| v.as_str()), Some("argos2-2f9c1a7b"));
    }

    #[test]
    fn a_reflogs_first_line_tells_when_the_clone_was_made() {
        assert_eq!(parse_reflog_time("0000000000000000000000000000000000000000 9750aa0d Kshitij B <k@example.test> 1760053200 +0530\tclone: from git@github.com:acme/argos.git"), Some(1760053200));
        assert_eq!(parse_reflog_time("garbage"), None);
    }

    #[test]
    fn copy_of_ranks_a_folder_among_its_group() {
        let groups = vec![Group {
            key: "github:acme/argos".into(),
            label: "acme/argos".into(),
            members: vec![
                GroupMember { path: "/w/a".into(), label: "a".into(), id: "1".into(), offset: 0, kind: "clone".into(), rank: 1 },
                GroupMember { path: "/w/b".into(), label: "b".into(), id: "2".into(), offset: 100, kind: "clone".into(), rank: 2 },
            ],
        }];
        // Paths that are not on disk fall back to their normalised form.
        assert_eq!(copy_of("/w/b/", &groups), Some((2, 2, vec!["a".to_string()])));
        assert_eq!(copy_of("/w/zzz", &groups), None);
    }
}
