//! Repository states: saved snapshots of where you are — the commit, the
//! branch, what is staged and every changed or new file — and a way back to
//! any of them. Each repository keeps as many as its person sets (four to
//! begin with).
//!
//! A state lives inside the repository, shaped like a stash entry: a commit
//! whose tree is the working tree (tracked files as they are on disk, plus new
//! files that aren't ignored), whose first parent is HEAD and whose second
//! parent records the index. Its name, branch and time are in its message.
//! Each sits under `refs/gitswitch/states/<worktree>/<slot>`: it survives
//! restarts, `gc` keeps it from any worktree, nothing pushes it, and History
//! never shows it.
//!
//! Repositories inside — submodules that are checked out, and nested
//! repositories that aren't ignored — are saved with it, each in its own
//! object store under `…/super/<slot>`, and come back with it, at any depth.
//!
//! Going back first saves what is there now as "Before going back" (inner
//! repositories too), and commits that leave a branch keep a backup branch
//! (`gitswitch-before-state-…`), so nothing is ever lost. Then the branch
//! moves back and the files become exactly what was saved: the same changes,
//! the same staging, new files back, files made since removed. Only files
//! "Before going back" holds are ever removed or overwritten: an ignored file
//! in the way stops it before anything changes. A branch whose later commits
//! are pushed moves back only when the person says so; otherwise the state
//! goes onto a new branch (inner repositories are put back at the saved
//! commit, their branch kept).
//!
//! Every operation follows `git_ops`' four phases: snapshot → pure check →
//! git → verify by re-reading.

use crate::error::AppError;
use crate::git_advice::{self, GitOp};
use crate::git_exec::{is_object_id, validate_branch_name, GitCmd, LFS_TIMEOUT, LOCAL_TIMEOUT};
use crate::git_ops::{advice_ctx, busy, refuse, snapshot, OpResult, Refusal, StatusFacts};
use crate::git_status::RepoStatus;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;

/// How many states a repository keeps until its person sets another
/// number. About four things fit in working memory (Cowan 2001); names keep
/// more of them apart.
pub const DEFAULT_SLOTS: usize = 4;
/// The most a repository can be set to keep: only a guard against a typo.
pub const MAX_SLOTS: usize = 99;
/// Where a repository keeps its number: its own (local) git config.
const SLOTS_KEY: &str = "gitswitch.stateSlots";
/// The automatic one, saved before every Go back (not one of the four).
pub const BEFORE: &str = "before";
const ROOT: &str = "refs/gitswitch/states/";
/// The backup branch commits keep when going back takes them off a branch.
pub const STATE_BACKUP_PREFIX: &str = "gitswitch-before-state-";
const FORMAT: u32 = 1;
/// New files a state would copy into `.git`: past this, commit or ignore them.
pub const MAX_NEW_FILES: usize = 50_000;
pub const MAX_NEW_BYTES: u64 = 500 * 1024 * 1024;
const NAME_MAX: usize = 60;
const AUTHOR: &str = "GitSwitch";
const EMAIL: &str = "states@gitswitch.invalid";
/// Paths named in an "in the way" refusal.
const SHOWN_PATHS: usize = 5;

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ---- what is stored -------------------------------------------------------

/// A slot: "1" to "4", or "before".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot(String);

impl Slot {
    /// `max`: the highest slot accepted (the repository's number when saving;
    /// any slot a state can be in when going back or deleting).
    pub fn parse(s: &str, allow_before: bool, max: usize) -> Result<Slot, AppError> {
        let s = s.trim();
        if allow_before && s == BEFORE {
            return Ok(Slot(BEFORE.into()));
        }
        match s.parse::<usize>() {
            Ok(n) if (1..=max).contains(&n) => Ok(Slot(n.to_string())),
            _ => Err(AppError::Command(format!("'{}' is not a state slot (1 to {})", s, max))),
        }
    }
    fn before() -> Slot {
        Slot(BEFORE.into())
    }
    /// This worktree's ref for the slot.
    fn top_ref(&self, wt: &str) -> String {
        format!("{}{}/{}", ROOT, wt, self.0)
    }
    /// An inner repository's part of its outer repository's state in this slot.
    fn nested_ref(&self, wt: &str) -> String {
        format!("{}{}/super/{}", ROOT, wt, self.0)
    }
    fn is_before(&self) -> bool {
        self.0 == BEFORE
    }
    /// "state 2" / "“Before going back”".
    fn label(&self) -> String {
        if self.is_before() {
            "“Before going back”".into()
        } else {
            format!("state {}", self.0)
        }
    }
}

/// A ref component for a worktree: "main" for the main one, "wt-<name>"
/// for a linked one (its directory under `.git/worktrees`).
pub fn wt_component(git_dir: &str, common_dir: &str) -> String {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
    if norm(git_dir) == norm(common_dir) {
        return "main".into();
    }
    let base = git_dir.replace('\\', "/").trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
    let safe: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    format!("wt-{}", if safe.is_empty() { "x".into() } else { safe })
}

/// What a state's message carries, as one JSON line.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Meta {
    #[serde(rename = "gitswitch-state")]
    pub format: u32,
    pub name: String,
    /// Short branch name; none when HEAD was detached.
    pub branch: Option<String>,
    /// Seconds since 1970.
    pub saved: i64,
    /// For "Before going back": the state that was gone back to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_of: Option<String>,
    /// Repositories inside it, saved with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subs: Vec<SubState>,
}

/// An inner repository's part of a state: where it is, and its own state
/// commit (none: it had no commit yet when saved).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SubState {
    pub path: String,
    pub oid: Option<String>,
}

pub fn message(m: &Meta) -> String {
    format!("GitSwitch state: {}\n\n{}\n", m.name, serde_json::to_string(m).unwrap_or_default())
}

pub fn parse_message(text: &str) -> Option<Meta> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with("{\"gitswitch-state\""))
        .and_then(|l| serde_json::from_str(l).ok())
}

/// `git cat-file commit`: (tree, parents, message).
pub fn parse_commit(raw: &str) -> Option<(String, Vec<String>, String)> {
    let (head, msg) = raw.split_once("\n\n").unwrap_or((raw, ""));
    let mut tree = None;
    let mut parents = Vec::new();
    for l in head.lines() {
        if let Some(t) = l.strip_prefix("tree ") {
            tree = Some(t.trim().to_string());
        } else if let Some(p) = l.strip_prefix("parent ") {
            parents.push(p.trim().to_string());
        }
    }
    Some((tree?, parents, msg.to_string()))
}

/// The name kept: one line, at most 60 characters; "State N" when empty.
pub fn clean_name(name: Option<&str>, slot: &Slot) -> String {
    let n: String = name.unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ").chars().take(NAME_MAX).collect();
    if !n.is_empty() {
        n
    } else if slot.is_before() {
        "Before going back".into()
    } else {
        format!("State {}", slot.0)
    }
}

/// `git ls-tree -r -z <tree>`: (mode, path) per entry.
pub fn parse_ls_tree(raw: &[u8]) -> Vec<(String, String)> {
    raw.split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .filter_map(|rec| {
            let tab = rec.iter().position(|b| *b == b'\t')?;
            let meta = String::from_utf8_lossy(&rec[..tab]);
            let mode = meta.split_whitespace().next()?.to_string();
            Some((mode, String::from_utf8_lossy(&rec[tab + 1..]).to_string()))
        })
        .collect()
}

/// A Git LFS pointer's object id: 64 ASCII hex digits, nothing else. The
/// caller slices it by bytes into the store's folder names, which a
/// multi-byte character there would make panic midway through a restore.
pub fn lfs_pointer_oid(content: &str) -> Option<String> {
    if !content.starts_with("version https://git-lfs.github.com/spec/") {
        return None;
    }
    content.lines().find_map(|l| l.strip_prefix("oid sha256:")).map(|o| o.trim().to_string()).filter(|o| o.len() == 64 && o.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A saved state, read back from its commit.
#[derive(Debug, Clone)]
struct Saved {
    oid: String,
    head: String,
    index_tree: String,
    worktree_tree: String,
    meta: Meta,
}

// ---- what the page sees -------------------------------------------------

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct StateSub {
    pub path: String,
    /// Checked out now (its part can come back).
    pub downloaded: bool,
    pub changed: usize,
    pub branch: Option<String>,
    /// Commits on its saved branch since that are already on a remote: that
    /// branch is kept and the repository put back at the saved commit instead.
    pub pushed_since: usize,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct RepoState {
    pub slot: String,
    pub oid: String,
    pub name: String,
    pub branch: Option<String>,
    pub head: String,
    pub head_short: String,
    pub head_subject: String,
    pub saved: i64,
    /// Files that differ from its commit, staged or not, new ones included.
    pub changed: usize,
    pub staged: usize,
    pub new_files: usize,
    pub branch_gone: bool,
    /// Commits its branch gained since (they leave it on Go back, kept on a backup branch).
    pub commits_since: usize,
    /// Of those, already on a remote.
    pub pushed_since: usize,
    pub upstream: Option<String>,
    pub before_of: Option<String>,
    pub subs: Vec<StateSub>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct StatesList {
    /// The saved states, by slot.
    pub states: Vec<RepoState>,
    pub before: Option<RepoState>,
    /// How many this repository keeps (its own setting; 4 until changed).
    pub max: usize,
    /// The slot a new state goes into; none when every slot is in use.
    pub next_free: Option<String>,
    /// The most it can be set to.
    pub limit: usize,
}

/// Why a repository can't be set to keep `max` states, given the slots in
/// use: a number out of range, or saved states above it (nothing is ever
/// dropped silently). Pure.
pub fn check_max(max: usize, used: &[usize]) -> Option<String> {
    if !(1..=MAX_SLOTS).contains(&max) {
        return Some(format!("Choose a number from 1 to {}.", MAX_SLOTS));
    }
    let mut over: Vec<usize> = used.iter().copied().filter(|n| *n > max).collect();
    over.sort_unstable();
    let highest = *over.last()?;
    let one = over.len() == 1;
    Some(format!(
        "State{} {} {} saved above {}. Delete {} first, or keep at least {}.",
        if one { "" } else { "s" },
        over.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", "),
        if one { "is" } else { "are" },
        max,
        if one { "it" } else { "them" },
        highest
    ))
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct StateOutcome {
    /// save | restore | delete
    pub action: String,
    pub slot: String,
    pub name: String,
    pub oid: Option<String>,
    pub branch_before: Option<String>,
    pub branch_after: Option<String>,
    pub head_before: Option<String>,
    pub head_after: Option<String>,
    /// Going back: the "Before going back" state it saved first.
    pub before_saved: Option<String>,
    pub new_branch: Option<String>,
    /// The branch keeping the commits that left (going back).
    pub backup: Option<String>,
    /// What undoes it, in words.
    pub recovery: Option<String>,
    /// About inner repositories and files (put back detached, came after…).
    pub notes: Vec<String>,
}

// ---- the pure check -----------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum StateIntent {
    Save {
        /// The slot holds a state and the caller didn't name it.
        taken: bool,
        /// The caller named a state that is no longer there.
        stale: bool,
        sparse: bool,
        index_locked: bool,
        new_files: usize,
        new_bytes: u64,
    },
    Restore {
        found: bool,
        sparse: bool,
        index_locked: bool,
        new_branch: bool,
        new_branch_exists: bool,
        /// The branch is checked out in another worktree (its path).
        elsewhere: Option<String>,
        pushed_since: usize,
        allow_pushed: bool,
        new_files: usize,
        new_bytes: u64,
    },
    Delete {
        found: bool,
    },
}

pub const STALE: &str = "That state isn't there any more — the list was out of date. It has been read again; nothing was changed.";

fn too_big(files: usize, bytes: u64) -> Option<Refusal> {
    (files > MAX_NEW_FILES || bytes > MAX_NEW_BYTES).then(|| {
        refuse(
            "too-many-new-files",
            format!(
                "There are {} new files ({} MB) that aren't ignored. A state copies every new file into the repository, so commit them or add them to .gitignore first.",
                files,
                bytes / (1024 * 1024)
            ),
        )
    })
}

/// Everything that must be true before git runs. Pure.
pub fn check_state(intent: &StateIntent, f: &StatusFacts) -> Option<Refusal> {
    let (verb, sparse, locked) = match intent {
        StateIntent::Delete { found } => return (!found).then(|| refuse("no-state", STALE)),
        StateIntent::Save { sparse, index_locked, .. } => ("save a state", *sparse, *index_locked),
        StateIntent::Restore { sparse, index_locked, .. } => ("go back to a state", *sparse, *index_locked),
    };
    if f.sync_in_progress {
        return Some(refuse("sync-in-progress", "An update is paused here. Continue or abort it first (the Changes tab shows both)."));
    }
    if let Some(r) = busy(f, verb) {
        return Some(r);
    }
    if f.conflicted > 0 {
        return Some(refuse("conflicts", format!("Resolve the {} conflicted file{} first.", f.conflicted, if f.conflicted == 1 { "" } else { "s" })));
    }
    if f.unborn {
        return Some(refuse("unborn", "There's no commit here yet. Make the first commit, then states can be saved and gone back to."));
    }
    if sparse {
        return Some(refuse("sparse-checkout", "States aren't available in a partial (sparse) checkout yet."));
    }
    if locked {
        return Some(refuse("index-locked", "Another git command is working here (.git/index.lock exists). Wait for it to finish, or delete that file if nothing is running."));
    }
    match intent {
        StateIntent::Save { taken, stale, new_files, new_bytes, .. } => {
            if *stale {
                return Some(refuse("no-state", STALE));
            }
            if *taken {
                return Some(refuse("slot-taken", "That slot holds a state. Replace it, or save into an empty slot."));
            }
            too_big(*new_files, *new_bytes)
        }
        StateIntent::Restore { found, new_branch, new_branch_exists, elsewhere, pushed_since, allow_pushed, new_files, new_bytes, .. } => {
            if !found {
                return Some(refuse("no-state", STALE));
            }
            if *new_branch && *new_branch_exists {
                return Some(refuse("branch-exists", "A branch with that name exists. Choose another name."));
            }
            if let (Some(w), false) = (elsewhere, new_branch) {
                return Some(refuse("branch-elsewhere", format!("That branch is checked out in another worktree ({}). Go back on a new branch instead.", w)));
            }
            if *pushed_since > 0 && !allow_pushed && !new_branch {
                return Some(refuse(
                    "state-drops-pushed",
                    format!(
                        "{} commit{} made since {} already pushed. Go back on a new branch, or move the branch back anyway.",
                        pushed_since,
                        if *pushed_since == 1 { "" } else { "s" },
                        if *pushed_since == 1 { "is" } else { "are" }
                    ),
                ));
            }
            // Going back saves what is there now first.
            too_big(*new_files, *new_bytes)
        }
        StateIntent::Delete { .. } => None,
    }
}

/// The paths a restore would create that something not in "Before going
/// back" — an ignored file or folder, a nested repository — already holds:
/// writing there would destroy it. `target`: (mode, path) of the saved
/// working tree; `known`: every path "Before going back" holds. On a
/// case-insensitive file system paths compare case-insensitively.
pub fn in_the_way(repo: &Path, target: &[(String, String)], known: &HashSet<String>, ignore_case: bool) -> Vec<String> {
    let norm = |p: &str| if ignore_case { p.to_lowercase() } else { p.to_string() };
    let known: HashSet<String> = known.iter().map(|p| norm(p)).collect();
    let mut known_dirs: HashSet<String> = HashSet::new();
    for p in &known {
        let mut cut = p.as_str();
        while let Some(i) = cut.rfind('/') {
            cut = &cut[..i];
            if !known_dirs.insert(cut.to_string()) {
                break;
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (mode, path) in target {
        if out.len() >= SHOWN_PATHS {
            break;
        }
        if known.contains(&norm(path)) {
            continue;
        }
        let full = repo.join(path);
        if let Ok(m) = std::fs::symlink_metadata(&full) {
            let blocked = if mode == "160000" {
                // A submodule's folder may be there; only a file is in the way.
                !m.is_dir()
            } else if m.is_dir() {
                // A folder where the state has a file: everything in it would go.
                !dir_holds_only(&full, path, &known, &norm)
            } else {
                true
            };
            if blocked && seen.insert(path.clone()) {
                out.push(path.clone());
            }
        }
        // A file where the state needs a folder.
        let mut cut = path.as_str();
        while let Some(i) = cut.rfind('/') {
            cut = &cut[..i];
            if known_dirs.contains(&norm(cut)) || known.contains(&norm(cut)) {
                break;
            }
            if let Ok(m) = std::fs::symlink_metadata(repo.join(cut)) {
                if !m.is_dir() && seen.insert(cut.to_string()) {
                    out.push(cut.to_string());
                }
                break;
            }
        }
    }
    out
}

/// Every file under `dir` is one "Before going back" holds, and no
/// repository is inside.
fn dir_holds_only(dir: &Path, rel: &str, known: &HashSet<String>, norm: &dyn Fn(&str) -> String) -> bool {
    let mut stack = vec![(dir.to_path_buf(), rel.to_string())];
    let mut seen = 0usize;
    while let Some((d, r)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { return false };
        for e in entries.flatten() {
            seen += 1;
            if seen > 20_000 {
                return false;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".git" {
                return false;
            }
            let child = format!("{}/{}", r, name);
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push((e.path(), child)),
                Ok(_) => {
                    if !known.contains(&norm(&child)) {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        }
    }
    true
}

// ---- git -------------------------------------------------------------------

/// `-c submodule.recurse=false` and the other pins: nothing here recurses on
/// its own; inner repositories are handled one by one.
fn g(repo: &Path) -> GitCmd {
    GitCmd::at(repo).pinned().timeout(LOCAL_TIMEOUT)
}

/// Run; stdout trimmed, or what git said.
async fn run(cmd: GitCmd) -> Result<String, String> {
    match cmd.run().await {
        Ok(o) if o.ok() => Ok(o.trimmed()),
        Ok(o) => {
            let said = o.stderr.trim().to_string();
            Err(if said.is_empty() { o.text().trim().to_string() } else { said })
        }
        Err(e) => Err(e.to_string()),
    }
}

async fn read_ref(repo: &Path, r: &str) -> Option<String> {
    run(g(repo).args(["rev-parse", "-q", "--verify", "--end-of-options", &format!("{}^{{commit}}", r)])).await.ok().filter(|s| !s.is_empty())
}

async fn head_commit(repo: &Path) -> Option<String> {
    read_ref(repo, "HEAD").await
}

async fn current_branch(repo: &Path) -> Option<String> {
    run(g(repo).args(["symbolic-ref", "-q", "--short", "HEAD"])).await.ok().filter(|s| !s.is_empty())
}

/// This worktree's component in the state refs.
async fn wt(repo: &Path) -> Result<String, String> {
    let out = run(g(repo).args(["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"])).await?;
    let mut lines = out.lines();
    let (Some(gd), Some(cd)) = (lines.next(), lines.next()) else {
        return Err("not a repository".into());
    };
    Ok(wt_component(gd, cd))
}

async fn git_path(repo: &Path, name: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(run(g(repo).args(["rev-parse", "--path-format=absolute", "--git-path", name])).await?))
}

/// A sparse checkout, or any skip-worktree entry.
async fn is_sparse(repo: &Path) -> bool {
    if run(g(repo).args(["config", "--bool", "core.sparseCheckout"])).await.is_ok_and(|s| s == "true") {
        return true;
    }
    g(repo).args(["ls-files", "-z", "-t"]).run().await.is_ok_and(|o| o.stdout.split(|b| *b == 0).any(|r| r.starts_with(b"S ")))
}

async fn index_locked(repo: &Path) -> bool {
    git_path(repo, "index.lock").await.is_ok_and(|p| p.exists())
}

async fn ignore_case(repo: &Path) -> bool {
    run(g(repo).args(["config", "--bool", "core.ignorecase"])).await.is_ok_and(|s| s == "true")
}

/// `git rev-list --count <opts> --end-of-options <revs>`: a revision is never
/// read as an option, whatever text a saved state put there.
async fn count(repo: &Path, opts: &[&str], revs: &[&str]) -> usize {
    let mut a = vec!["rev-list", "--count"];
    a.extend_from_slice(opts);
    a.push("--end-of-options");
    a.extend_from_slice(revs);
    run(g(repo).args(a)).await.ok().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Commits on `tip` that aren't in `head`, and how many of them are on a remote.
async fn since(repo: &Path, head: &str, tip: &str) -> (usize, usize) {
    let not_head = format!("^{}", head);
    let total = count(repo, &[], &[tip, &not_head]).await;
    if total == 0 {
        return (0, 0);
    }
    // `tip ^head --not --remotes`, with the options first: the first `--not`
    // makes `--remotes` exclusions, the second turns that off again for the
    // revisions after `--end-of-options`.
    let unpushed = count(repo, &["--not", "--remotes", "--not"], &[tip, &not_head]).await;
    (total, total.saturating_sub(unpushed))
}

/// A scratch index in the git directory, removed when dropped.
struct TempIndex(PathBuf);

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A scratch index: a copy of the real one (`copy`) or empty.
async fn temp_index(repo: &Path, copy: bool) -> Result<TempIndex, String> {
    let index = git_path(repo, "index").await?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = index.with_file_name(format!("gitswitch-state-{}-{}.index", std::process::id(), nanos));
    if copy && index.exists() {
        std::fs::copy(&index, &tmp).map_err(|e| format!("could not copy the index: {}", e))?;
    }
    Ok(TempIndex(tmp))
}

/// A command on a scratch index; no split index, no line-ending refusal.
fn on(repo: &Path, t: &TempIndex) -> GitCmd {
    g(repo).cfg("core.splitIndex", "false").cfg("core.safecrlf", "false").env("GIT_INDEX_FILE", t.0.to_string_lossy().to_string())
}

fn nul_join(paths: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in paths {
        out.extend_from_slice(p);
        out.push(0);
    }
    out
}

/// The working tree as a tree object, the way `git stash -u` sees it: tracked
/// files as they are on disk (assume-unchanged ones too) plus new files that
/// aren't ignored; and the repositories inside (submodules checked out, and
/// nested repositories that aren't ignored), listed, not added.
async fn worktree_tree(repo: &Path) -> Result<(String, Vec<String>), String> {
    let t = temp_index(repo, true).await?;
    let listing = g(repo).args(["ls-files", "-z", "-v"]).run().await.map_err(|e| e.to_string())?;
    let assumed: Vec<Vec<u8>> = listing
        .stdout
        .split(|b| *b == 0)
        .filter(|r| r.len() > 2 && r[0].is_ascii_lowercase() && r[1] == b' ')
        .map(|r| r[2..].to_vec())
        .collect();
    if !assumed.is_empty() {
        run(on(repo, &t).args(["update-index", "-z", "--no-assume-unchanged", "--stdin"]).stdin_bytes(nul_join(&assumed))).await?;
    }
    run(on(repo, &t).args(["add", "-u"])).await?;
    let others = g(repo).args(["ls-files", "-z", "-o", "--exclude-standard"]).run().await.map_err(|e| e.to_string())?;
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut inner: Vec<String> = Vec::new();
    for rec in others.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        if rec.ends_with(b"/") {
            // A repository of its own (git doesn't look inside).
            inner.push(String::from_utf8_lossy(&rec[..rec.len() - 1]).to_string());
        } else {
            files.push(rec.to_vec());
        }
    }
    if !files.is_empty() {
        run(on(repo, &t).args(["update-index", "-z", "--add", "--stdin"]).stdin_bytes(nul_join(&files))).await?;
    }
    let tree = run(on(repo, &t).args(["write-tree"])).await?;
    let links = on(repo, &t).args(["ls-files", "-s", "-z"]).run().await.map_err(|e| e.to_string())?;
    for (_, path) in crate::submodules::parse_gitlinks(&links.stdout) {
        if !inner.contains(&path) {
            inner.push(path);
        }
    }
    inner.retain(|p| repo.join(p).join(".git").exists());
    Ok((tree, inner))
}

/// New files that aren't ignored, here and in every repository inside: how
/// many, and how big — what a state would copy into `.git`.
fn measure_new<'a>(repo: &'a Path) -> BoxFut<'a, (usize, u64)> {
    Box::pin(async move {
        let mut files = 0usize;
        let mut bytes = 0u64;
        if let Ok(o) = g(repo).args(["ls-files", "--others", "--exclude-standard", "-z"]).run().await {
            for rec in o.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
                let rel = String::from_utf8_lossy(rec).to_string();
                let p = repo.join(&rel);
                if rel.ends_with('/') {
                    let (f, b) = measure_new(&p).await;
                    files += f;
                    bytes += b;
                } else {
                    files += 1;
                    bytes += std::fs::symlink_metadata(&p).map(|m| m.len()).unwrap_or(0);
                }
            }
        }
        let raw = crate::submodules::index_listing(repo).await;
        for (_, path) in crate::submodules::parse_gitlinks(&raw) {
            let dir = repo.join(&path);
            if dir.join(".git").exists() {
                let (f, b) = measure_new(&dir).await;
                files += f;
                bytes += b;
            }
        }
        (files, bytes)
    })
}

/// Why an operation stopped.
enum Stop {
    Refused(Refusal),
    Failed(String),
}

impl From<String> for Stop {
    fn from(s: String) -> Self {
        Stop::Failed(s)
    }
}

/// What an inner repository's own state forbids saving it now.
async fn inner_blocked(repo: &Path, label: &str) -> Option<Refusal> {
    let git_dir = PathBuf::from(run(g(repo).args(["rev-parse", "--path-format=absolute", "--git-dir"])).await.ok()?);
    for (file, what) in [
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
    ] {
        if git_dir.join(file).exists() {
            return Some(refuse("operation-in-progress", format!("{} has a {} in progress. Finish or abort it there first.", label, what)));
        }
    }
    if git_dir.join("index.lock").exists() {
        return Some(refuse("index-locked", format!("Another git command is working in {} (index.lock exists).", label)));
    }
    if is_sparse(repo).await {
        return Some(refuse("sparse-checkout", format!("{} is a partial (sparse) checkout; states aren't available there yet.", label)));
    }
    let unmerged = g(repo).args(["ls-files", "-u", "-z"]).run().await.map(|o| !o.stdout.is_empty()).unwrap_or(false);
    unmerged.then(|| refuse("conflicts", format!("{} has conflicted files. Resolve them there first.", label)))
}

/// A repository saved as it is now: its state commit, its working tree (as a
/// tree), the refs its inner repositories' parts go under (written only once
/// the whole save is sure to go ahead), and those parts by path.
struct Captured {
    oid: String,
    worktree_tree: String,
    refs: Vec<(PathBuf, String, String)>,
    inner: HashMap<String, Captured>,
}

/// Save `repo` as it is now, and every repository inside it (their parts go
/// under the `slot`'s nested ref there).
fn capture<'a>(repo: &'a Path, meta: Meta, slot: &'a Slot, shown: String) -> BoxFut<'a, Result<Captured, Stop>> {
    Box::pin(async move {
        let Some(head) = head_commit(repo).await else {
            return Err(Stop::Failed(format!("{} has no commit yet", shown)));
        };
        let index_tree = run(g(repo).args(["write-tree"])).await?;
        let (tree, inner_paths) = worktree_tree(repo).await?;
        let mut subs = Vec::new();
        let mut refs = Vec::new();
        let mut inner = HashMap::new();
        for path in inner_paths {
            let dir = repo.join(&path);
            let label = if shown.is_empty() { path.clone() } else { format!("{}/{}", shown, path) };
            if head_commit(&dir).await.is_none() {
                subs.push(SubState { path, oid: None });
                continue;
            }
            if let Some(r) = inner_blocked(&dir, &label).await {
                return Err(Stop::Refused(r));
            }
            let m = Meta { format: FORMAT, name: meta.name.clone(), branch: current_branch(&dir).await, saved: meta.saved, before_of: None, subs: Vec::new() };
            let c = capture(&dir, m, slot, label).await?;
            let wtc = wt(&dir).await?;
            refs.push((dir.clone(), slot.nested_ref(&wtc), c.oid.clone()));
            refs.extend(c.refs.iter().cloned());
            subs.push(SubState { path: path.clone(), oid: Some(c.oid.clone()) });
            inner.insert(path, c);
        }
        let meta = Meta { subs, ..meta };
        let ident = |c: GitCmd| {
            c.env("GIT_AUTHOR_NAME", AUTHOR).env("GIT_AUTHOR_EMAIL", EMAIL).env("GIT_COMMITTER_NAME", AUTHOR).env("GIT_COMMITTER_EMAIL", EMAIL)
        };
        let index_commit = run(ident(g(repo).args(["commit-tree", "--no-gpg-sign", "-p", &head, "-m", "GitSwitch state: index", &index_tree]))).await?;
        let oid = run(ident(g(repo).args(["commit-tree", "--no-gpg-sign", "-p", &head, "-p", &index_commit, "-F", "-", &tree])).stdin_bytes(message(&meta).into_bytes())).await?;
        Ok(Captured { oid, worktree_tree: tree, refs, inner })
    })
}

/// The inner repositories' refs, once the save goes ahead.
async fn write_inner_refs(c: &Captured) -> Result<(), String> {
    for (dir, r, oid) in &c.refs {
        run(g(dir).args(["update-ref", "-m", "gitswitch: save the outer repository's state", r, oid])).await?;
    }
    Ok(())
}

/// A state commit's tree and parents go to git as revisions (the counts, the
/// Changes tab's list, going back). `git cat-file` prints a commit's header
/// lines as they are stored, and a commit written by hand can say anything
/// there — `parent --output=<file>` made `git log` overwrite a file when the
/// Changes tab listed the states. Only object ids pass.
fn check_state_ids(tree: &str, parents: &[String]) -> Result<(), String> {
    if is_object_id(tree) && parents.iter().all(|p| is_object_id(p)) {
        Ok(())
    } else {
        Err("this isn't a state GitSwitch saved: its commit names something other than a commit id where git expects one, so it was left alone".into())
    }
}

/// Read a state commit back.
async fn read_saved(repo: &Path, oid: &str) -> Result<Saved, String> {
    // From a ref, or from an outer state's message (an inner repository's part).
    if !is_object_id(oid) {
        return Err("this isn't a state GitSwitch saved: it names something other than a commit id, so it was left alone".into());
    }
    let raw = run(g(repo).args(["cat-file", "commit", oid])).await?;
    let (tree, parents, msg) = parse_commit(&raw).ok_or("not a state commit")?;
    check_state_ids(&tree, &parents)?;
    let meta = parse_message(&msg).ok_or("not a GitSwitch state")?;
    if meta.format > FORMAT {
        return Err("saved by a newer GitSwitch".into());
    }
    let (Some(head), Some(index_commit)) = (parents.first(), parents.get(1)) else {
        return Err("not a GitSwitch state".into());
    };
    let index_tree = run(g(repo).args(["rev-parse", "--verify", "--end-of-options", &format!("{}^{{tree}}", index_commit)])).await?;
    if !is_object_id(&index_tree) {
        return Err("not a GitSwitch state".into());
    }
    Ok(Saved { oid: oid.to_string(), head: head.clone(), index_tree, worktree_tree: tree, meta })
}

/// changed, staged, new files — never changing for a given state.
type Counts = (usize, usize, usize);
static COUNTS: Mutex<Option<HashMap<String, Counts>>> = Mutex::new(None);

async fn names(repo: &Path, a: &str, b: &str, filter: Option<&str>) -> usize {
    let mut args = vec!["diff-tree".to_string(), "-r".into(), "-z".into(), "--name-only".into(), "--no-renames".into()];
    if let Some(f) = filter {
        args.push(format!("--diff-filter={}", f));
    }
    args.push("--end-of-options".into());
    args.push(a.to_string());
    args.push(b.to_string());
    g(repo).args(args).run().await.map(|o| o.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()).count()).unwrap_or(0)
}

async fn counts(repo: &Path, s: &Saved) -> Counts {
    if let Some(c) = COUNTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&s.oid)) {
        return *c;
    }
    let c = (
        names(repo, &s.head, &s.worktree_tree, None).await,
        names(repo, &s.head, &s.index_tree, None).await,
        names(repo, &s.index_tree, &s.worktree_tree, Some("A")).await,
    );
    COUNTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(s.oid.clone(), c);
    c
}

async fn branch_tip(repo: &Path, branch: &str) -> Option<String> {
    read_ref(repo, &format!("refs/heads/{}", branch)).await
}

/// The worktree other than this one that has `branch` checked out.
///
/// The first block `git worktree list` prints is always the main worktree,
/// and for a submodule its path is the module's git directory, not the
/// folder it is checked out in — so the main one is told apart by identity,
/// and only a linked one by its path.
async fn checked_out_elsewhere(repo: &Path, branch: &str) -> Option<String> {
    let dirs = run(g(repo).args(["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"])).await.ok()?;
    let mut lines = dirs.lines();
    let main_here = wt_component(lines.next()?, lines.next()?) == "main";
    let here = run(g(repo).args(["rev-parse", "--show-toplevel"])).await.ok()?;
    let canon = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    let here = canon(&here);
    let list = run(g(repo).args(["worktree", "list", "--porcelain"])).await.ok()?;
    let want = format!("branch refs/heads/{}", branch);
    for (i, block) in list.split("\n\n").filter(|b| !b.trim().is_empty()).enumerate() {
        if !block.lines().any(|l| l.trim() == want) {
            continue;
        }
        let path = block.lines().find_map(|l| l.strip_prefix("worktree ")).unwrap_or_default().to_string();
        let is_here = if i == 0 { main_here } else { !main_here && canon(&path) == here };
        if !is_here {
            return Some(path);
        }
    }
    None
}

async fn tree_entries(repo: &Path, tree: &str) -> Result<Vec<(String, String)>, String> {
    let o = g(repo).args(["ls-tree", "-r", "-z", tree]).run().await.map_err(|e| e.to_string())?;
    if !o.ok() {
        return Err(o.stderr.trim().to_string());
    }
    Ok(parse_ls_tree(&o.stdout))
}

/// Files a restore from `from` to `to` would write that are Git LFS
/// pointers whose content isn't in this repository's LFS store.
async fn lfs_missing(repo: &Path, from: &str, to: &str) -> Vec<String> {
    let Ok(o) = g(repo).args(["diff-tree", "-r", "-z", "--no-renames", "--diff-filter=AMT", "--end-of-options", from, to]).run().await else { return Vec::new() };
    // ":<mode> <mode> <sha> <sha> <status>\0<path>\0"
    let parts: Vec<&[u8]> = o.stdout.split(|b| *b == 0).collect();
    let mut blobs: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i + 1 < parts.len() {
        let meta = String::from_utf8_lossy(parts[i]).to_string();
        let path = String::from_utf8_lossy(parts[i + 1]).to_string();
        i += 2;
        let f: Vec<&str> = meta.trim_start_matches(':').split_whitespace().collect();
        if f.len() >= 4 && (f[1] == "100644" || f[1] == "100755") {
            blobs.push((f[3].to_string(), path));
        }
    }
    if blobs.is_empty() {
        return Vec::new();
    }
    let ids: String = blobs.iter().map(|(s, _)| format!("{}\n", s)).collect();
    let Ok(sizes) = g(repo).args(["cat-file", "--batch-check=%(objectname) %(objectsize)"]).stdin_bytes(ids.into_bytes()).run().await else {
        return Vec::new();
    };
    let small: HashSet<String> = sizes
        .text()
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(_, n)| n.trim().parse::<u64>().is_ok_and(|n| n < 1024))
        .map(|(s, _)| s.to_string())
        .collect();
    if small.is_empty() {
        return Vec::new();
    }
    let common = match run(g(repo).args(["rev-parse", "--path-format=absolute", "--git-common-dir"])).await {
        Ok(c) => PathBuf::from(c),
        Err(_) => return Vec::new(),
    };
    let store = match run(g(repo).args(["config", "lfs.storage"])).await {
        Ok(s) if !s.is_empty() => {
            let p = PathBuf::from(&s);
            if p.is_absolute() {
                p
            } else {
                common.join(p)
            }
        }
        _ => common.join("lfs"),
    };
    let mut missing = Vec::new();
    for (sha, path) in blobs.iter().filter(|(s, _)| small.contains(s)) {
        let Ok(content) = run(g(repo).args(["cat-file", "blob", sha])).await else { continue };
        if let Some(oid) = lfs_pointer_oid(&content) {
            if !store.join("objects").join(&oid[..2]).join(&oid[2..4]).join(&oid).exists() {
                missing.push(path.clone());
                if missing.len() >= SHOWN_PATHS {
                    break;
                }
            }
        }
    }
    missing
}

// ---- going back, in one repository ------------------------------------------

/// How HEAD comes back.
enum Move {
    /// The saved branch, moved back (tip: where it is now; none: recreated).
    Branch { name: String, tip: Option<String> },
    /// A new branch at the saved commit.
    NewBranch(String),
    /// Detached at the saved commit.
    Detached,
}

impl Move {
    fn branch(&self) -> Option<String> {
        match self {
            Move::Branch { name, .. } | Move::NewBranch(name) => Some(name.clone()),
            Move::Detached => None,
        }
    }
}

/// HEAD as `s` saved it; commits that leave the branch keep a backup branch
/// first (returned).
async fn move_head(repo: &Path, s: &Saved, mv: &Move, reason: &str) -> Result<Option<String>, String> {
    let mut backup = None;
    match mv {
        Move::Branch { name, tip } => {
            let r = format!("refs/heads/{}", name);
            if let Some(t) = tip.as_deref().filter(|t| *t != s.head) {
                if count(repo, &[], &[t, &format!("^{}", s.head)]).await > 0 {
                    let b = crate::tree::free_backup_name(&repo.to_string_lossy(), STATE_BACKUP_PREFIX).await;
                    run(g(repo).args(["update-ref", "-m", reason, &format!("refs/heads/{}", b), t, ""])).await?;
                    backup = Some(b);
                }
            }
            run(g(repo).args(["update-ref", "-m", reason, &r, &s.head, tip.as_deref().unwrap_or("")])).await?;
            if current_branch(repo).await.as_deref() != Some(name.as_str()) {
                run(g(repo).args(["symbolic-ref", "-m", reason, "HEAD", &r])).await?;
            }
        }
        Move::NewBranch(name) => {
            let r = format!("refs/heads/{}", name);
            run(g(repo).args(["update-ref", "-m", reason, &r, &s.head, ""])).await?;
            run(g(repo).args(["symbolic-ref", "-m", reason, "HEAD", &r])).await?;
        }
        Move::Detached => {
            run(g(repo).args(["update-ref", "--no-deref", "-m", reason, "HEAD", &s.head])).await?;
        }
    }
    Ok(backup)
}

/// The files as `s` saved them: from `from` (the working tree as "Before
/// going back" holds it) to the saved working tree — only paths those two
/// hold are written or removed — then the saved staging.
async fn put_files(repo: &Path, s: &Saved, from: &str) -> Result<(), String> {
    let t = temp_index(repo, false).await?;
    run(on(repo, &t).args(["read-tree", from])).await?;
    run(on(repo, &t).args(["read-tree", "--reset", "-u", &s.worktree_tree]).timeout(LFS_TIMEOUT)).await?;
    run(g(repo).args(["read-tree", "--reset", &s.index_tree])).await?;
    // Exit 1 only says files differ from the index — which they may.
    let _ = g(repo).args(["update-index", "-q", "--refresh"]).run().await;
    Ok(())
}

/// How `repo` compares with `s`: exact; or the only differences are new
/// files that "Before going back" didn't hold (ignored then, not now) —
/// returned; or something else differs (Err).
async fn compare(repo: &Path, s: &Saved, branch: Option<&str>, before_tree: &str) -> Result<Vec<String>, String> {
    if head_commit(repo).await.as_deref() != Some(s.head.as_str()) || current_branch(repo).await.as_deref() != branch {
        return Err("HEAD isn't where the state was".into());
    }
    if run(g(repo).args(["write-tree"])).await? != s.index_tree {
        return Err("the staged files aren't as saved".into());
    }
    let (now, _) = worktree_tree(repo).await?;
    if now == s.worktree_tree {
        return Ok(Vec::new());
    }
    let o = g(repo).args(["diff-tree", "-r", "-z", "--no-renames", "--name-status", "--end-of-options", &s.worktree_tree, &now]).run().await.map_err(|e| e.to_string())?;
    let parts: Vec<String> = o.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()).map(|r| String::from_utf8_lossy(r).to_string()).collect();
    let known: HashSet<String> = tree_entries(repo, before_tree).await?.into_iter().map(|(_, p)| p).collect();
    let mut extra = Vec::new();
    for pair in parts.chunks(2) {
        let [status, path] = pair else { return Err("the files aren't as saved".into()) };
        if status == "A" && !known.contains(path) {
            extra.push(path.clone());
        } else {
            return Err(format!("{} isn't as saved", path));
        }
    }
    Ok(extra)
}

/// The repositories inside, back as saved, at any depth. A repository that
/// can't come back safely is left as it is and said so.
fn restore_inner<'a>(
    repo: &'a Path,
    subs: &'a [SubState],
    now: &'a HashMap<String, Captured>,
    prefix: &'a str,
    allow_pushed: bool,
    notes: &'a mut Vec<String>,
    backups: &'a mut Vec<String>,
) -> BoxFut<'a, Result<(), String>> {
    Box::pin(async move {
        for sub in subs {
            let shown = format!("{}{}", prefix, sub.path);
            // The path is the state message's text, joined onto the repository:
            // one that is not plain folder names could name a folder outside it.
            if !crate::paths::is_plain_relative(&sub.path) {
                notes.push(format!("{}: its part of the state names a folder outside the repository, so it was left alone.", shown));
                continue;
            }
            // Plain names can still lead out through a link (a junction on
            // Windows) to another repository: no name of it that exists may
            // be one (`paths::lies_inside`).
            if !crate::paths::lies_inside(&repo.join(&sub.path), repo) {
                notes.push(format!("{}: its folder leads outside the repository (a link), so it was left alone.", shown));
                continue;
            }
            let Some(oid) = &sub.oid else {
                notes.push(format!("{} had no commit yet when the state was saved; it was left as it is.", shown));
                continue;
            };
            let dir = repo.join(&sub.path);
            if !dir.join(".git").exists() {
                // A submodule removed or never downloaded here: checked out again
                // from what this repository already has (never from the network).
                // `--checkout`: the saved commit, whatever the repository's
                // `submodule.<name>.update` says (an `!command` there would
                // otherwise run).
                let _ = run(g(repo).args(["submodule", "update", "--init", "--no-fetch", "--checkout", "--", &sub.path])).await;
            }
            if !dir.join(".git").exists() {
                notes.push(format!("{} isn't downloaded, so its part couldn't come back. Download it (Submodules), then go back again.", shown));
                continue;
            }
            let s = match read_saved(&dir, oid).await {
                Ok(s) => s,
                Err(_) => {
                    notes.push(format!("{}'s part of the state is gone (its repository was replaced); it was left as it is.", shown));
                    continue;
                }
            };
            // What it holds now: from "Before going back", or (checked out just
            // now) as it is — nothing of it can be lost either way.
            let from = match now.get(&sub.path) {
                Some(c) => c.worktree_tree.clone(),
                None => worktree_tree(&dir).await.map(|(t, _)| t).map_err(|e| format!("{}: {}", shown, e))?,
            };
            let target = tree_entries(&dir, &s.worktree_tree).await.map_err(|e| format!("{}: {}", shown, e))?;
            let known: HashSet<String> = tree_entries(&dir, &from).await.map_err(|e| format!("{}: {}", shown, e))?.into_iter().map(|(_, p)| p).collect();
            let blocked = in_the_way(&dir, &target, &known, ignore_case(&dir).await);
            if !blocked.is_empty() {
                notes.push(format!("{}: {} {} where the state has a file, so it was left as it is.", shown, blocked.join(", "), if blocked.len() == 1 { "is" } else { "are" }));
                continue;
            }
            let missing = lfs_missing(&dir, &from, &s.worktree_tree).await;
            if !missing.is_empty() {
                notes.push(format!("{}: the large files {} aren't downloaded (Git LFS), so it was left as it is.", shown, missing.join(", ")));
                continue;
            }
            let mv = match &s.meta.branch {
                None => Move::Detached,
                Some(b) => match branch_tip(&dir, b).await {
                    None => Move::Branch { name: b.clone(), tip: None },
                    Some(tip) if tip == s.head => Move::Branch { name: b.clone(), tip: Some(tip) },
                    Some(tip) => {
                        let (_, pushed) = since(&dir, &s.head, &tip).await;
                        if checked_out_elsewhere(&dir, b).await.is_some() {
                            notes.push(format!("{}: {} is checked out in another worktree, so it was put back at the saved commit without moving {}.", shown, b, b));
                            Move::Detached
                        } else if pushed > 0 && !allow_pushed {
                            notes.push(format!(
                                "{}: {} has {} pushed commit{} since, so it was put back at the saved commit without moving {} (it keeps them).",
                                shown,
                                b,
                                pushed,
                                if pushed == 1 { "" } else { "s" },
                                b
                            ));
                            Move::Detached
                        } else {
                            Move::Branch { name: b.clone(), tip: Some(tip) }
                        }
                    }
                },
            };
            let reason = "gitswitch: back to the outer repository's state";
            if let Some(b) = move_head(&dir, &s, &mv, reason).await.map_err(|e| format!("{}: {}", shown, e))? {
                backups.push(format!("{}: {}", shown, b));
            }
            put_files(&dir, &s, &from).await.map_err(|e| format!("{}: {}", shown, e))?;
            let empty = HashMap::new();
            let inside = now.get(&sub.path).map(|c| &c.inner).unwrap_or(&empty);
            restore_inner(&dir, &s.meta.subs, inside, &format!("{}/", shown), allow_pushed, notes, backups).await?;
            match compare(&dir, &s, mv.branch().as_deref(), &from).await {
                Ok(extra) if !extra.is_empty() => notes.push(format!("{}: {} ignored before and not now; kept as {}.", shown, extra.join(", "), if extra.len() == 1 { "it was" } else { "they were" })),
                Ok(_) => {}
                Err(e) => return Err(format!("{}: {}", shown, e)),
            }
        }
        Ok(())
    })
}

// ---- the operations ---------------------------------------------------------

fn outcome(action: &str, slot: &Slot, name: &str) -> StateOutcome {
    StateOutcome { action: action.into(), slot: slot.0.clone(), name: name.into(), ..StateOutcome::default() }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

fn short(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

fn failed(status: RepoStatus, said: &str, detail: String, state: StateOutcome) -> OpResult {
    let a = git_advice::explain(GitOp::Reset, said, &advice_ctx(&status));
    OpResult { detail, state: Some(Box::new(state)), ..OpResult::failed(a, status) }
}

fn oid_matches(have: &str, want: &str) -> bool {
    let want = want.trim();
    !want.is_empty() && (have == want || (want.len() >= 7 && have.starts_with(want)))
}

/// Save where the repository is now into `slot`. `replace`: the state the
/// page showed there (a full or 7+ digit oid), to replace it.
pub async fn save(repo_path: &str, slot: &str, name: Option<&str>, replace: Option<&str>) -> Result<OpResult, AppError> {
    let repo = Path::new(repo_path);
    let slot = Slot::parse(slot, false, slots_of(repo).await)?;
    let before = snapshot(repo_path).await?;
    let wtc = wt(repo).await.map_err(AppError::Command)?;
    let current = read_ref(repo, &slot.top_ref(&wtc)).await;
    let named = replace.map(str::trim).filter(|r| !r.is_empty());
    let stale = named.is_some_and(|r| current.as_deref().is_some_and(|c| !oid_matches(c, r)));
    let (new_files, new_bytes) = measure_new(repo).await;
    let intent = StateIntent::Save {
        taken: current.is_some() && named.is_none(),
        stale,
        sparse: is_sparse(repo).await,
        index_locked: index_locked(repo).await,
        new_files,
        new_bytes,
    };
    if let Some(r) = check_state(&intent, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let name = clean_name(name, &slot);
    let meta = Meta {
        format: FORMAT,
        name: name.clone(),
        branch: if before.detached { None } else { before.branch.clone() },
        saved: chrono::Utc::now().timestamp(),
        before_of: None,
        subs: Vec::new(),
    };
    let c = match capture(repo, meta, &slot, String::new()).await {
        Ok(c) => c,
        Err(Stop::Refused(r)) => return Ok(OpResult::refused(r, before)),
        Err(Stop::Failed(e)) => return Ok(failed(before, &e, format!("Nothing was saved: {}", e), outcome("save", &slot, &name))),
    };
    let old = current.clone().unwrap_or_default();
    if let Err(e) = run(g(repo).args(["update-ref", "-m", &format!("gitswitch: save {}", slot.label()), &slot.top_ref(&wtc), &c.oid, &old])).await {
        return Ok(OpResult::refused(refuse("no-state", format!("{} ({})", STALE, e)), before));
    }
    // The replaced state's inner parts go; the new ones take their place.
    if let Some(old) = &current {
        if let Ok(s) = read_saved(repo, old).await {
            forget_inner(repo, &s.meta.subs, &slot).await;
        }
    }
    if let Err(e) = write_inner_refs(&c).await {
        return Ok(failed(before, &e, format!("The state was saved, but a repository inside couldn't keep its part ({}).", e), outcome("save", &slot, &name)));
    }
    let after = snapshot(repo_path).await?;
    let saved = read_saved(repo, &c.oid).await.map_err(AppError::Command)?;
    let (changed, staged, new) = counts(repo, &saved).await;
    let subs = saved.meta.subs.iter().filter(|s| s.oid.is_some()).count();
    let mut detail = format!(
        "“{}”: {} at {} with {}{}{}.",
        name,
        saved.meta.branch.clone().unwrap_or_else(|| "detached HEAD".into()),
        short(&saved.head),
        plural(changed, "changed file", "changed files"),
        if staged > 0 { format!(" ({} staged)", staged) } else { String::new() },
        if new > 0 { format!(", {} of them new", new) } else { String::new() }
    );
    if subs > 0 {
        detail.push_str(&format!(" {} inside {} saved with it.", plural(subs, "repository", "repositories"), if subs == 1 { "was" } else { "were" }));
    }
    detail.push_str(" Go back to it any time from the States card.");
    let mut o = outcome("save", &slot, &name);
    o.oid = Some(c.oid.clone());
    o.recovery = current.map(|c| format!("the state it replaced was {}", c));
    Ok(OpResult { state: Some(Box::new(o)), ..OpResult::done(format!("Saved {}.", slot.label()), detail, after) })
}

/// Go back to the state in `slot` (the page names it by `oid`): what is there
/// now is saved as "Before going back" first. `new_branch`: put it on a new
/// branch instead of moving the saved one; `allow_pushed`: move a branch back
/// past commits that are already pushed (the person said so).
pub async fn restore(repo_path: &str, slot: &str, oid: &str, new_branch: Option<&str>, allow_pushed: bool) -> Result<OpResult, AppError> {
    let slot = Slot::parse(slot, true, MAX_SLOTS)?;
    let repo = Path::new(repo_path);
    let status = snapshot(repo_path).await?;
    let wtc = wt(repo).await.map_err(AppError::Command)?;
    let current = read_ref(repo, &slot.top_ref(&wtc)).await;
    let found = current.as_deref().is_some_and(|c| oid_matches(c, oid));
    let saved = match current.as_deref().filter(|_| found) {
        Some(c) => Some(read_saved(repo, c).await.map_err(AppError::Command)?),
        None => None,
    };
    let new_branch = new_branch.map(str::trim).filter(|b| !b.is_empty()).map(str::to_string);
    if let Some(b) = &new_branch {
        validate_branch_name(b)?;
    }
    let new_branch_exists = match &new_branch {
        Some(b) => branch_tip(repo, b).await.is_some(),
        None => false,
    };
    let target = saved.as_ref().and_then(|s| s.meta.branch.clone());
    let tip = match (&target, &new_branch) {
        (Some(b), None) => branch_tip(repo, b).await,
        _ => None,
    };
    let (commits_since, pushed_since) = match (&saved, &tip) {
        (Some(s), Some(t)) => since(repo, &s.head, t).await,
        _ => (0, 0),
    };
    let elsewhere = match (&target, &new_branch) {
        (Some(b), None) if status.branch.as_deref() != Some(b.as_str()) || status.detached => checked_out_elsewhere(repo, b).await,
        _ => None,
    };
    let (new_files, new_bytes) = measure_new(repo).await;
    let intent = StateIntent::Restore {
        found,
        sparse: is_sparse(repo).await,
        index_locked: index_locked(repo).await,
        new_branch: new_branch.is_some(),
        new_branch_exists,
        elsewhere,
        pushed_since,
        allow_pushed,
        new_files,
        new_bytes,
    };
    if let Some(r) = check_state(&intent, &StatusFacts::from(&status)) {
        return Ok(OpResult::refused(r, status));
    }
    let Some(s) = saved else {
        return Ok(OpResult::refused(refuse("no-state", STALE), status));
    };
    let mut o = outcome("restore", &slot, &s.meta.name);
    o.oid = Some(s.oid.clone());
    o.branch_before = if status.detached { None } else { status.branch.clone() };
    o.head_before = status.head_oid.clone();
    o.new_branch = new_branch.clone();

    // 1. What is there now, saved (no ref written yet).
    let meta = Meta {
        format: FORMAT,
        name: "Before going back".into(),
        branch: if status.detached { None } else { status.branch.clone() },
        saved: chrono::Utc::now().timestamp(),
        before_of: Some(s.meta.name.clone()),
        subs: Vec::new(),
    };
    let before_slot = Slot::before();
    let now = match capture(repo, meta, &before_slot, String::new()).await {
        Ok(c) => c,
        Err(Stop::Refused(r)) => return Ok(OpResult::refused(r, status)),
        Err(Stop::Failed(e)) => return Ok(failed(status, &e, format!("Nothing was changed: what you have now couldn't be saved first ({}).", e), o)),
    };

    // 2. Nothing in the way that "Before going back" doesn't hold; every large file at hand.
    let target_entries = tree_entries(repo, &s.worktree_tree).await.map_err(AppError::Command)?;
    let known: HashSet<String> = tree_entries(repo, &now.worktree_tree).await.map_err(AppError::Command)?.into_iter().map(|(_, p)| p).collect();
    let blocked = in_the_way(repo, &target_entries, &known, ignore_case(repo).await);
    if !blocked.is_empty() {
        let one = blocked.len() == 1;
        return Ok(OpResult::refused(
            refuse(
                "ignored-in-the-way",
                format!(
                    "{} {} where the state has a file. {} ignored, so going back would destroy {}: move or delete {} first. Nothing was changed.",
                    blocked.join(", "),
                    if one { "is" } else { "are" },
                    if one { "It is" } else { "They are" },
                    if one { "it" } else { "them" },
                    if one { "it" } else { "them" }
                ),
            ),
            status,
        ));
    }
    let missing = lfs_missing(repo, &now.worktree_tree, &s.worktree_tree).await;
    if !missing.is_empty() {
        return Ok(OpResult::refused(
            refuse("lfs-content-missing", format!("These large files (Git LFS) aren't downloaded: {}. Download them first (LFS), then go back. Nothing was changed.", missing.join(", "))),
            status,
        ));
    }

    // 3. "Before going back" kept — from here on nothing can be lost.
    let before_ref = before_slot.top_ref(&wtc);
    let before_old = read_ref(repo, &before_ref).await;
    if let Err(e) = run(g(repo).args(["update-ref", "-m", "gitswitch: save before going back", &before_ref, &now.oid, before_old.as_deref().unwrap_or("")])).await {
        return Ok(failed(status, &e, format!("Nothing was changed: “Before going back” couldn't be kept ({}).", e), o));
    }
    if let Some(old) = &before_old {
        if !slot.is_before() {
            if let Ok(prev) = read_saved(repo, old).await {
                forget_inner(repo, &prev.meta.subs, &before_slot).await;
            }
        }
    }
    if let Err(e) = write_inner_refs(&now).await {
        return Ok(failed(status, &e, format!("Nothing was changed: an inner repository's part of “Before going back” couldn't be kept ({}).", e), o));
    }
    o.before_saved = Some(now.oid.clone());
    o.recovery = Some("Go back to “Before going back” on the States card: it holds everything you had.".into());

    // 4. HEAD, the files, then the repositories inside.
    let mv = match (&new_branch, &target) {
        (Some(b), _) => Move::NewBranch(b.clone()),
        (None, Some(b)) => Move::Branch { name: b.clone(), tip: tip.clone() },
        (None, None) => Move::Detached,
    };
    let branch_after = mv.branch();
    let reason = format!("gitswitch: back to {} ({})", slot.label(), s.meta.name);
    let stop_text = |e: &str| format!("Going back stopped part way ({}). Everything you had is saved as “Before going back” — go back to it on the States card.", e);
    match move_head(repo, &s, &mv, &reason).await {
        Ok(b) => o.backup = b,
        Err(e) => {
            let after = snapshot(repo_path).await?;
            return Ok(failed(after, &e, stop_text(&e), o));
        }
    }
    if let Err(e) = put_files(repo, &s, &now.worktree_tree).await {
        let after = snapshot(repo_path).await?;
        return Ok(failed(after, &e, stop_text(&e), o));
    }
    let mut notes = Vec::new();
    let mut backups = Vec::new();
    let inner = restore_inner(repo, &s.meta.subs, &now.inner, "", allow_pushed, &mut notes, &mut backups).await;
    for p in now.inner.keys().filter(|p| !s.meta.subs.iter().any(|x| &x.path == *p)) {
        notes.push(format!("{} came after the state; it was left as it is.", p));
    }
    if let Err(e) = inner {
        o.notes = notes;
        let after = snapshot(repo_path).await?;
        return Ok(failed(after, &e, stop_text(&e), o));
    }

    // 5. Verify by re-reading.
    let checked = compare(repo, &s, branch_after.as_deref(), &now.worktree_tree).await;
    let after = snapshot(repo_path).await?;
    o.branch_after = branch_after.clone();
    o.head_after = after.head_oid.clone();
    if let Ok(extra) = &checked {
        if !extra.is_empty() {
            let one = extra.len() == 1;
            notes.push(format!("{} {} ignored before and not now; kept as {}.", extra.join(", "), if one { "was" } else { "were" }, if one { "it was" } else { "they were" }));
        }
    }
    for b in &backups {
        notes.push(format!("Inside {} (backup branch).", b));
    }
    o.notes = notes.clone();
    if let Err(e) = checked {
        return Ok(failed(after, &e, format!("Going back finished, but {}. Everything you had is saved as “Before going back”.", e), o));
    }
    let (changed, staged, _) = counts(repo, &s).await;
    let mut detail = format!(
        "{} is at {} again and your files are as saved: {}{}.",
        branch_after.clone().unwrap_or_else(|| "HEAD".into()),
        short(&s.head),
        plural(changed, "changed file", "changed files"),
        if staged > 0 { format!(" ({} staged)", staged) } else { String::new() }
    );
    if new_branch.is_some() {
        detail.push_str(&format!(" {} is untouched.", target.clone().unwrap_or_else(|| "The branch you were on".into())));
    } else if let Some(b) = &o.backup {
        detail.push_str(&format!(" The {} made since left the branch; {} keeps them.", plural(commits_since, "commit", "commits"), b));
    }
    if allow_pushed && pushed_since > 0 && new_branch.is_none() {
        detail.push_str(&format!(
            " {} is now behind {}: Update would bring the pushed commits back, and pushing new work would need a force push (GitSwitch never does that).",
            branch_after.clone().unwrap_or_default(),
            status.upstream.clone().unwrap_or_else(|| "its remote".into())
        ));
    }
    for n in &notes {
        detail.push(' ');
        detail.push_str(n);
    }
    detail.push_str(" What you had before is saved as “Before going back”.");
    Ok(OpResult { state: Some(Box::new(o)), ..OpResult::done(format!("Back to {}.", slot.label()), detail, after) })
}

/// Remove the state in `slot` (the page names it by `oid`), and its parts in
/// the repositories inside.
pub async fn delete(repo_path: &str, slot: &str, oid: &str) -> Result<OpResult, AppError> {
    let slot = Slot::parse(slot, true, MAX_SLOTS)?;
    let repo = Path::new(repo_path);
    let status = snapshot(repo_path).await?;
    let wtc = wt(repo).await.map_err(AppError::Command)?;
    let current = read_ref(repo, &slot.top_ref(&wtc)).await;
    let found = current.as_deref().is_some_and(|c| oid_matches(c, oid));
    if let Some(r) = check_state(&StateIntent::Delete { found }, &StatusFacts::from(&status)) {
        return Ok(OpResult::refused(r, status));
    }
    let c = current.unwrap_or_default();
    let saved = read_saved(repo, &c).await.ok();
    if let Err(e) = run(g(repo).args(["update-ref", "-m", "gitswitch: delete state", "-d", &slot.top_ref(&wtc), &c])).await {
        return Ok(OpResult::refused(refuse("no-state", format!("{} ({})", STALE, e)), status));
    }
    if let Some(s) = &saved {
        forget_inner(repo, &s.meta.subs, &slot).await;
    }
    let name = saved.map(|s| s.meta.name).unwrap_or_default();
    let mut o = outcome("delete", &slot, &name);
    o.oid = Some(c.clone());
    o.recovery = Some(format!("its commit was {}", c));
    let after = snapshot(repo_path).await?;
    Ok(OpResult { state: Some(Box::new(o)), ..OpResult::done(format!("Deleted {}.", slot.label()), format!("“{}” is gone from the list.", name), after) })
}

/// The slot's parts in the repositories inside, removed (only where they are
/// still the ones this state recorded).
fn forget_inner<'a>(repo: &'a Path, subs: &'a [SubState], slot: &'a Slot) -> BoxFut<'a, ()> {
    Box::pin(async move {
        for sub in subs {
            // Text from the state's message: never a folder outside the
            // repository, by its names or through a link.
            if !crate::paths::is_plain_relative(&sub.path) || !crate::paths::lies_inside(&repo.join(&sub.path), repo) {
                continue;
            }
            let dir = repo.join(&sub.path);
            let (Some(oid), true) = (&sub.oid, dir.join(".git").exists()) else { continue };
            if let Ok(s) = read_saved(&dir, oid).await {
                forget_inner(&dir, &s.meta.subs, slot).await;
            }
            let Ok(wtc) = wt(&dir).await else { continue };
            let r = slot.nested_ref(&wtc);
            if read_ref(&dir, &r).await.as_deref() == Some(oid.as_str()) {
                let _ = run(g(&dir).args(["update-ref", "-d", &r, oid])).await;
            }
        }
    })
}

/// The four slots and "Before going back".
pub async fn list(repo_path: &str) -> Result<StatesList, AppError> {
    let repo = Path::new(repo_path);
    let wtc = wt(repo).await.map_err(AppError::Command)?;
    let prefix = format!("{}{}/", ROOT, wtc);
    let upstream = run(g(repo).args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])).await.ok();
    let max = slots_of(repo).await;
    let mut out = StatesList { states: Vec::new(), before: None, max, next_free: None, limit: MAX_SLOTS };
    let refs = run(g(repo).args(["for-each-ref", "--format=%(refname) %(objectname)", &prefix])).await.unwrap_or_default();
    for line in refs.lines() {
        let Some((name, oid)) = line.split_once(' ') else { continue };
        let Some(slot) = name.strip_prefix(&prefix) else { continue };
        let Ok(slot) = Slot::parse(slot, true, MAX_SLOTS) else { continue };
        let Ok(s) = read_saved(repo, oid).await else { continue };
        let state = describe(repo, &slot, &s, upstream.clone()).await;
        if slot.is_before() {
            out.before = Some(state);
        } else {
            out.states.push(state);
        }
    }
    out.states.sort_by_key(|s| s.slot.parse::<usize>().unwrap_or(usize::MAX));
    let used: HashSet<usize> = out.states.iter().filter_map(|s| s.slot.parse().ok()).collect();
    out.next_free = (1..=max).find(|n| !used.contains(n)).map(|n| n.to_string());
    Ok(out)
}

/// How many states this repository keeps.
async fn slots_of(repo: &Path) -> usize {
    run(g(repo).args(["config", "--local", "--get", SLOTS_KEY]))
        .await
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map(|n| n.clamp(1, MAX_SLOTS))
        .unwrap_or(DEFAULT_SLOTS)
}

/// Keep up to `max` states in this repository (its own git config). Refused
/// below a slot that holds a state.
pub async fn set_max(repo_path: &str, max: usize) -> Result<StatesList, AppError> {
    let repo = Path::new(repo_path);
    let now = list(repo_path).await?;
    let used: Vec<usize> = now.states.iter().filter_map(|s| s.slot.parse().ok()).collect();
    if let Some(why) = check_max(max, &used) {
        return Err(AppError::Command(why));
    }
    run(g(repo).args(["config", "--local", SLOTS_KEY, &max.to_string()])).await.map_err(AppError::Command)?;
    list(repo_path).await
}

async fn describe(repo: &Path, slot: &Slot, s: &Saved, upstream: Option<String>) -> RepoState {
    let (changed, staged, new_files) = counts(repo, s).await;
    let (head_short, head_subject) = run(g(repo).args(["log", "-1", "--format=%h%x00%s", "--end-of-options", &s.head]))
        .await
        .ok()
        .and_then(|t| t.split_once('\0').map(|(a, b)| (a.to_string(), b.to_string())))
        .unwrap_or_else(|| (short(&s.head).to_string(), String::new()));
    let tip = match &s.meta.branch {
        Some(b) => branch_tip(repo, b).await,
        None => None,
    };
    let (commits_since, pushed_since) = match &tip {
        Some(t) => since(repo, &s.head, t).await,
        None => (0, 0),
    };
    let mut subs = Vec::new();
    describe_inner(repo, &s.meta.subs, "", &mut subs).await;
    RepoState {
        slot: slot.0.clone(),
        oid: s.oid.clone(),
        name: s.meta.name.clone(),
        branch: s.meta.branch.clone(),
        head: s.head.clone(),
        head_short,
        head_subject,
        saved: s.meta.saved,
        changed,
        staged,
        new_files,
        branch_gone: s.meta.branch.is_some() && tip.is_none(),
        commits_since,
        pushed_since,
        upstream,
        before_of: s.meta.before_of.clone(),
        subs,
    }
}

fn describe_inner<'a>(repo: &'a Path, subs: &'a [SubState], prefix: &'a str, out: &'a mut Vec<StateSub>) -> BoxFut<'a, ()> {
    Box::pin(async move {
        for sub in subs {
            // Text from the state's message: never a folder outside the
            // repository, by its names or through a link.
            if !crate::paths::is_plain_relative(&sub.path) || !crate::paths::lies_inside(&repo.join(&sub.path), repo) {
                continue;
            }
            let path = format!("{}{}", prefix, sub.path);
            let dir = repo.join(&sub.path);
            let downloaded = dir.join(".git").exists();
            let saved = match (&sub.oid, downloaded) {
                (Some(oid), true) => read_saved(&dir, oid).await.ok(),
                _ => None,
            };
            let Some(s) = saved else {
                out.push(StateSub { path, downloaded, ..StateSub::default() });
                continue;
            };
            let (changed, _, _) = counts(&dir, &s).await;
            let pushed_since = match &s.meta.branch {
                Some(b) => match branch_tip(&dir, b).await {
                    Some(t) if t != s.head => since(&dir, &s.head, &t).await.1,
                    _ => 0,
                },
                None => 0,
            };
            out.push(StateSub { path: path.clone(), downloaded, changed, branch: s.meta.branch.clone(), pushed_since });
            describe_inner(&dir, &s.meta.subs, &format!("{}/", path), out).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> StatusFacts {
        StatusFacts::default()
    }

    #[test]
    fn slots_go_as_far_as_the_repository_keeps() {
        assert_eq!(Slot::parse("1", false, 4).unwrap().top_ref("main"), "refs/gitswitch/states/main/1");
        assert_eq!(Slot::parse(" 4 ", false, 4).unwrap().nested_ref("main"), "refs/gitswitch/states/main/super/4");
        assert!(Slot::parse("0", false, 4).is_err() && Slot::parse("5", false, 4).is_err() && Slot::parse("x", false, 4).is_err());
        assert!(Slot::parse("12", false, 12).is_ok(), "a repository set to keep 12");
        assert!(Slot::parse("before", false, 4).is_err(), "nobody saves into the automatic one");
        assert!(Slot::parse("before", true, 4).unwrap().is_before());
        assert_eq!(Slot::parse("2", false, 4).unwrap().label(), "state 2");
    }

    #[test]
    fn a_repository_keeps_as_many_as_it_is_set_to_and_never_drops_one() {
        assert_eq!(check_max(10, &[1, 2, 9]), None);
        assert_eq!(check_max(4, &[1, 2]), None);
        let m = check_max(4, &[1, 9, 6]).unwrap();
        assert!(m.contains("States 6, 9 are saved above 4") && m.contains("keep at least 9"), "{m}");
        let one = check_max(5, &[7]).unwrap();
        assert!(one.contains("State 7 is saved above 5. Delete it first"), "{one}");
        assert!(check_max(0, &[]).is_some() && check_max(MAX_SLOTS + 1, &[]).is_some());
        assert_eq!(check_max(MAX_SLOTS, &[]), None);
    }

    #[test]
    fn each_worktree_keeps_its_own_states() {
        assert_eq!(wt_component("D:/r/.git", "D:/r/.git"), "main");
        assert_eq!(wt_component("D:\\r\\.git\\", "d:/r/.git"), "main", "the same directory however it is spelled");
        assert_eq!(wt_component("/r/.git/worktrees/feature", "/r/.git"), "wt-feature");
        assert_eq!(wt_component("/r/.git/worktrees/my wt~1", "/r/.git"), "wt-my-wt-1", "a ref name can't hold those");
    }

    #[test]
    fn a_state_message_carries_its_facts_and_reads_back() {
        let m = Meta {
            format: FORMAT,
            name: "Before the refactor".into(),
            branch: Some("main".into()),
            saved: 1_793_952_000,
            before_of: None,
            subs: vec![SubState { path: "lib".into(), oid: Some("abc".into()) }, SubState { path: "vendor/x y".into(), oid: None }],
        };
        let text = message(&m);
        assert!(text.starts_with("GitSwitch state: Before the refactor\n\n"), "a readable subject for anyone's git log");
        assert_eq!(parse_message(&text), Some(m));
        assert_eq!(parse_message("Just a commit\n\nwith words"), None);
    }

    #[test]
    fn a_state_commit_is_read_as_tree_parents_and_message() {
        let raw = "tree 1111\nparent aaaa\nparent bbbb\nauthor GitSwitch <x> 1 +0000\ncommitter GitSwitch <x> 1 +0000\n\nGitSwitch state: S\n\n{\"gitswitch-state\":1}\n";
        let (tree, parents, msg) = parse_commit(raw).unwrap();
        assert_eq!(tree, "1111");
        assert_eq!(parents, vec!["aaaa", "bbbb"]);
        assert!(msg.contains("gitswitch-state"));
    }

    #[test]
    fn names_are_one_short_line_with_a_default() {
        let s = Slot::parse("3", false, 4).unwrap();
        assert_eq!(clean_name(Some("  fix \n the   header "), &s), "fix the header");
        assert_eq!(clean_name(None, &s), "State 3");
        assert_eq!(clean_name(Some("   "), &s), "State 3");
        assert_eq!(clean_name(Some(&"x".repeat(80)), &s).chars().count(), 60);
    }

    #[test]
    fn tree_listings_and_lfs_pointers_are_read() {
        let raw = b"100644 blob aaa\tsrc/a b.txt\x00160000 commit bbb\tlib\x00";
        assert_eq!(parse_ls_tree(raw), vec![("100644".to_string(), "src/a b.txt".to_string()), ("160000".to_string(), "lib".to_string())]);
        let pointer = format!("version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 12345\n", "a".repeat(64));
        assert_eq!(lfs_pointer_oid(&pointer), Some("a".repeat(64)));
        assert_eq!(lfs_pointer_oid("just text"), None);
    }

    #[test]
    fn an_lfs_pointer_oid_is_64_ascii_hex_digits_or_nothing() {
        let pointer = |oid: &str| format!("version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 1\n", oid);
        // 64 bytes with a two-byte character across the slice at byte 2: this
        // used to panic in lfs_missing (`&oid[..2]`).
        let straddling = format!("a\u{e9}{}", "a".repeat(61));
        assert_eq!(straddling.len(), 64);
        assert_eq!(lfs_pointer_oid(&pointer(&straddling)), None);
        assert_eq!(lfs_pointer_oid(&pointer(&format!("a\u{fffd}{}", "a".repeat(60)))), None, "a byte from_utf8_lossy replaced");
        assert_eq!(lfs_pointer_oid(&pointer(&"g".repeat(64))), None);
        assert_eq!(lfs_pointer_oid(&pointer(&"0123456789abcdef".repeat(4))), Some("0123456789abcdef".repeat(4)));
    }

    #[test]
    fn a_state_names_only_object_ids_for_git_to_read() {
        let (c, t) = ("c".repeat(40), "e".repeat(40));
        assert!(check_state_ids(&t, &[c.clone(), c.clone()]).is_ok());
        assert!(check_state_ids(&"e".repeat(64), &["c".repeat(64), "d".repeat(64)]).is_ok(), "SHA-256 repositories");
        for (tree, parents) in [
            (t.clone(), vec!["--output=canary.txt".to_string(), c.clone()]),
            (t.clone(), vec![c.clone(), "--output=canary.txt".to_string()]),
            ("--output=x".to_string(), vec![c.clone(), c.clone()]),
            (t.clone(), vec![c.clone(), "HEAD".to_string()]),
            (t.clone(), vec![c[..39].to_string(), c.clone()]),
        ] {
            let e = check_state_ids(&tree, &parents).unwrap_err();
            assert!(e.contains("isn't a state GitSwitch saved") && !e.contains("canary"), "{e}");
        }
    }

    fn save(taken: bool, stale: bool, files: usize, bytes: u64) -> StateIntent {
        StateIntent::Save { taken, stale, sparse: false, index_locked: false, new_files: files, new_bytes: bytes }
    }

    #[test]
    fn saving_is_refused_when_it_cant_be_whole() {
        assert_eq!(check_state(&save(false, false, 3, 10), &facts()), None);
        assert_eq!(check_state(&save(true, false, 0, 0), &facts()).unwrap().code, "slot-taken");
        assert_eq!(check_state(&save(false, true, 0, 0), &facts()).unwrap().code, "no-state");
        assert_eq!(check_state(&save(false, false, MAX_NEW_FILES + 1, 0), &facts()).unwrap().code, "too-many-new-files");
        assert_eq!(check_state(&save(false, false, 1, MAX_NEW_BYTES + 1), &facts()).unwrap().code, "too-many-new-files");
        let merging = StatusFacts { operation: Some("merge".into()), abort_command: Some("git merge --abort".into()), ..facts() };
        assert_eq!(check_state(&save(false, false, 0, 0), &merging).unwrap().code, "operation-in-progress");
        assert_eq!(check_state(&save(false, false, 0, 0), &StatusFacts { conflicted: 2, ..facts() }).unwrap().code, "conflicts");
        assert_eq!(check_state(&save(false, false, 0, 0), &StatusFacts { unborn: true, ..facts() }).unwrap().code, "unborn");
        assert_eq!(check_state(&save(false, false, 0, 0), &StatusFacts { sync_in_progress: true, ..facts() }).unwrap().code, "sync-in-progress");
        let sparse = StateIntent::Save { taken: false, stale: false, sparse: true, index_locked: false, new_files: 0, new_bytes: 0 };
        assert_eq!(check_state(&sparse, &facts()).unwrap().code, "sparse-checkout");
        let locked = StateIntent::Save { taken: false, stale: false, sparse: false, index_locked: true, new_files: 0, new_bytes: 0 };
        assert_eq!(check_state(&locked, &facts()).unwrap().code, "index-locked");
    }

    fn restore(found: bool, elsewhere: Option<&str>, pushed: usize, allow: bool, new_branch: bool, exists: bool) -> StateIntent {
        StateIntent::Restore {
            found,
            sparse: false,
            index_locked: false,
            new_branch,
            new_branch_exists: exists,
            elsewhere: elsewhere.map(String::from),
            pushed_since: pushed,
            allow_pushed: allow,
            new_files: 0,
            new_bytes: 0,
        }
    }

    #[test]
    fn going_back_asks_before_moving_a_branch_past_pushed_commits() {
        assert_eq!(check_state(&restore(true, None, 0, false, false, false), &facts()), None, "nothing pushed since: it just goes");
        let r = check_state(&restore(true, None, 2, false, false, false), &facts()).unwrap();
        assert_eq!(r.code, "state-drops-pushed");
        assert!(r.message.contains("2 commits made since are already pushed"), "{}", r.message);
        assert_eq!(check_state(&restore(true, None, 2, true, false, false), &facts()), None, "the person said: move it back anyway");
        assert_eq!(check_state(&restore(true, None, 2, false, true, false), &facts()), None, "or on a new branch");
        assert_eq!(check_state(&restore(true, None, 0, false, true, true), &facts()).unwrap().code, "branch-exists");
        assert_eq!(check_state(&restore(true, Some("D:/w2"), 0, false, false, false), &facts()).unwrap().code, "branch-elsewhere");
        assert_eq!(check_state(&restore(true, Some("D:/w2"), 0, false, true, false), &facts()), None, "a new branch is fine whatever holds the old one");
        assert_eq!(check_state(&restore(false, None, 0, false, false, false), &facts()).unwrap().code, "no-state");
    }

    #[test]
    fn deleting_only_checks_it_is_still_there() {
        assert_eq!(check_state(&StateIntent::Delete { found: true }, &StatusFacts { conflicted: 3, ..facts() }), None);
        assert_eq!(check_state(&StateIntent::Delete { found: false }, &facts()).unwrap().code, "no-state");
    }

    #[test]
    fn the_page_may_name_a_state_by_a_short_id() {
        assert!(oid_matches("1234567890abcdef", "1234567890abcdef"));
        assert!(oid_matches("1234567890abcdef", "1234567"));
        assert!(!oid_matches("1234567890abcdef", "123456"), "too short to be sure");
        assert!(!oid_matches("1234567890abcdef", ""));
        assert!(!oid_matches("1234567890abcdef", "7654321"));
    }

    #[test]
    fn nothing_the_backup_doesnt_hold_is_ever_overwritten() {
        let dir = std::env::temp_dir().join(format!("gitswitch-states-way-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("cache/deep")).unwrap();
        std::fs::create_dir_all(dir.join("held")).unwrap();
        std::fs::create_dir_all(dir.join("lib/.git")).unwrap();
        std::fs::write(dir.join("notes.local"), "mine").unwrap();
        std::fs::write(dir.join("cache/deep/x.bin"), "ignored").unwrap();
        std::fs::write(dir.join("held/a.txt"), "known").unwrap();
        std::fs::write(dir.join("build"), "a file where a folder is wanted").unwrap();
        std::fs::write(dir.join("Readme.md"), "case").unwrap();
        let known: HashSet<String> = ["held/a.txt", "Readme.md", "src/kept.rs"].into_iter().map(String::from).collect();
        let file = |p: &str| ("100644".to_string(), p.to_string());
        let target = vec![
            file("notes.local"),                       // an ignored file where the state has one
            file("cache"),                             // a folder holding ignored files where the state has a file
            file("held"),                              // a folder holding only known files: fine
            file("build/out.txt"),                     // a file where the state needs a folder
            file("README.md"),                         // the same file, spelled differently, on a case-insensitive disk
            file("src/kept.rs"),                       // known: fine
            file("fresh.txt"),                         // nothing there: fine
            ("160000".to_string(), "lib".to_string()), // a submodule's folder: fine
        ];
        let mut got = in_the_way(&dir, &target, &known, true);
        got.sort();
        assert_eq!(got, vec!["build", "cache", "notes.local"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_state_never_names_a_folder_outside_the_repository() {
        // `outside` is a repository next to `repo`; a state's message names it
        // as `../outside`. Nothing may treat it as one of the repository's parts.
        let base = std::env::temp_dir().join(format!("gitswitch-states-outside-{}", std::process::id()));
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(base.join("outside").join(".git")).unwrap();
        // Plain names, but a link (a junction on Windows) to `outside`.
        crate::paths::test_link_dir(&base.join("outside"), &repo.join("linked"));
        let subs = vec![SubState { path: "../outside".into(), oid: None }, SubState { path: "linked".into(), oid: None }, SubState { path: "lib".into(), oid: None }];

        let mut out = Vec::new();
        describe_inner(&repo, &subs, "", &mut out).await;
        assert_eq!(out.iter().map(|s| s.path.as_str()).collect::<Vec<_>>(), vec!["lib"], "only the plain path inside is described");

        let (mut notes, mut backups) = (Vec::new(), Vec::new());
        restore_inner(&repo, &subs, &HashMap::new(), "", false, &mut notes, &mut backups).await.unwrap();
        assert_eq!(notes.len(), 3, "{:?}", notes);
        assert!(notes[0].starts_with("../outside: ") && notes[0].contains("outside the repository"), "{:?}", notes);
        assert!(notes[1].starts_with("linked: ") && notes[1].contains("leads outside the repository"), "{:?}", notes);
        assert!(notes[2].starts_with("lib had no commit yet"), "{:?}", notes);
        assert!(backups.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Text a saved state carries (its HEAD, its trees) that reaches git as
    /// a revision is never an option: each runner puts `--end-of-options`
    /// (git 2.24) right before it, and the file `--output=` names is not
    /// written. Reached here directly: `check_state_ids` refuses such a
    /// state before any of them runs.
    #[tokio::test]
    async fn a_dash_led_revision_from_a_state_is_never_an_option() {
        let dir = std::env::temp_dir().join(format!("gitswitch-states-eoo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(g(&dir).args(["init", "-q", "-b", "main"])).await.unwrap();
        run(g(&dir).args(["-c", "user.name=x", "-c", "user.email=x@x.test", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "--no-verify", "-m", "one"])).await.unwrap();
        std::fs::write(dir.join("canary.txt"), "precious\n").unwrap();
        let rev = "--output=canary.txt";

        assert_eq!(count(&dir, &[], &[rev]).await, 0);
        assert_eq!(names(&dir, rev, "HEAD", None).await, 0);
        assert!(lfs_missing(&dir, rev, "HEAD").await.is_empty());
        let odd = Saved { oid: format!("eoo-{}", std::process::id()), head: rev.into(), index_tree: rev.into(), worktree_tree: rev.into(), meta: Meta::default() };
        let _ = describe(&dir, &Slot::parse("1", false, 4).unwrap(), &odd, None).await;
        // compare's diff-tree runs once HEAD and the index are as saved.
        let at_head = Saved { head: head_commit(&dir).await.unwrap(), index_tree: run(g(&dir).args(["write-tree"])).await.unwrap(), ..odd.clone() };
        assert!(compare(&dir, &at_head, Some("main"), "").await.is_err());

        assert_eq!(std::fs::read_to_string(dir.join("canary.txt")).unwrap(), "precious\n", "nothing was written to the file the text names");
        let ran: Vec<Vec<String>> = crate::git_exec::test_support::ran_under(&dir).into_iter().map(|r| r.args).filter(|a| a.iter().any(|x| x.starts_with(rev))).collect();
        let subs: Vec<&str> = ran.iter().map(|a| a[0].as_str()).collect();
        // count, names, lfs_missing; describe: its counts (three) and its log; compare.
        assert_eq!(subs, vec!["rev-list", "diff-tree", "diff-tree", "diff-tree", "diff-tree", "diff-tree", "log", "diff-tree"], "{:?}", ran);
        for a in &ran {
            let at = a.iter().position(|x| x.starts_with(rev)).unwrap();
            assert_eq!(a[at - 1], "--end-of-options", "{:?}", a);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
