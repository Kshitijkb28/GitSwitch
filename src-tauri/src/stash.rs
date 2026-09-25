//! Stashes: list, show, push, apply/pop, drop, restore one file.
//!
//! Same four phases as `git_ops` (snapshot → pure `check` → one git command →
//! verify by re-reading). Never `stash --all` (ignored files stay ignored),
//! never a silent drop: dropping records the commit id so the entry can be put
//! back with `git stash store`. Every number the result carries is measured
//! from the stash commit itself, never predicted from the tree — a
//! pathspec-limited stash, for one, also records the staged changes that did
//! *not* match, exactly as `git stash show` reports them.

use crate::error::AppError;
use crate::git_advice::{self, GitOp};
use crate::git_exec::{GitCmd, LOCAL_TIMEOUT, NET_TIMEOUT, READ_TIMEOUT};
use crate::git_ops::{self, advice_ctx, busy, pathspec_stdin, refuse, snapshot, validate_repo_path, Intent, OpResult, Refusal, StatusFacts};
use crate::git_status::{self, ChangeEntry, FileDiff, RepoStatus};
use crate::sync::shell_quote;
use serde::{Deserialize, Serialize};

/// `stash_list` returns at most this many entries.
const MAX_LISTED: usize = 50;

const STALE: &str = "That stash isn't there any more — the list may be out of date.";

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct StashRef {
    /// `stash@{n}` at the time of the operation.
    #[serde(rename = "ref")]
    pub ref_: String,
    pub oid: String,
    /// The full reflog subject ("On main: fix header", "WIP on main: 1a2b3c …",
    /// "autostash") — the text `git stash store -m` needs to recreate the entry
    /// exactly, so it is kept whole here and only split for display in
    /// `StashEntry`.
    pub message: String,
}

#[derive(Debug, Serialize, Clone, Default, PartialEq)]
pub struct StashEntry {
    pub index: usize,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub oid: String,
    pub message: String,
    pub branch: Option<String>,
    pub date: String,
    pub tracked_files: usize,
    pub untracked_files: usize,
    /// Stored by a rebase's --autostash (a sync or a pull) when it could not
    /// re-apply the stash: the subject is exactly `autostash`. A hand-made
    /// stash that merely mentions the word is not one.
    pub is_autostash: bool,
}

#[derive(Debug, Serialize, Clone, Default, PartialEq)]
pub struct StashFile {
    pub path: String,
    /// M | A | D | R | T | untracked
    pub status: String,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct StashDetail {
    pub entry: StashEntry,
    pub files: Vec<StashFile>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct StashOutcome {
    /// push | apply | pop | drop | restore
    pub action: String,
    pub entry: Option<StashRef>,
    pub stash_count: usize,
    pub conflicts: Vec<String>,
    /// After a pop that conflicted: the entry is still in the list.
    pub kept: bool,
    /// The exact command that undoes this operation.
    pub recovery: Option<String>,
    /// Dirty submodules a stash never includes (git does not recurse).
    pub not_stashed_submodules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StashIntent {
    Push { include_untracked: bool, has_paths: bool },
    Apply { exists: bool, pop: bool },
    Drop { exists: bool },
    RestoreFile { exists: bool, in_stash: bool },
}

impl StashIntent {
    /// Recovering a file from a stash is exactly what a failed autostash
    /// re-apply during a paused sync needs.
    pub fn allowed_during_sync(&self) -> bool {
        matches!(self, StashIntent::RestoreFile { .. })
    }

    fn verb(&self) -> &'static str {
        match self {
            StashIntent::Push { .. } => "stash",
            StashIntent::Apply { pop: true, .. } => "pop a stash",
            StashIntent::Apply { .. } => "apply a stash",
            StashIntent::Drop { .. } => "drop a stash",
            StashIntent::RestoreFile { .. } => "restore a file from a stash",
        }
    }
}

fn no_stash() -> Refusal {
    refuse("no-stash", "That stash no longer exists — the list may be out of date.")
}

const MOVED: &str = "That stash moved or was dropped — the list is out of date.";

/// The pure gate for every stash operation. Order: busy → the entry exists →
/// conflicts → anything to stash.
pub fn check_stash(intent: &StashIntent, f: &StatusFacts) -> Option<Refusal> {
    // Restoring one file is resolution work: allowed inside a merge or rebase
    // and while a sync is paused, because a stash that failed to re-apply is
    // exactly the moment it is needed.
    if let StashIntent::RestoreFile { exists, in_stash } = intent {
        if !exists {
            return Some(no_stash());
        }
        if !in_stash {
            // The caller names the path; the code is what the UI keys on.
            return Some(refuse("not-in-stash", "That file isn't in this stash."));
        }
        return None;
    }
    if let Some(r) = busy(f, intent.verb()) {
        return Some(r);
    }
    match intent {
        StashIntent::Push { include_untracked, .. } => {
            if f.unborn {
                return Some(refuse(
                    "unborn-head",
                    "There are no commits yet — a stash needs a commit to be based on.",
                ));
            }
            if f.conflicted > 0 {
                return Some(refuse(
                    "unmerged-paths",
                    "Resolve the conflicts first — a stash can't hold half a merge.",
                ));
            }
            if f.staged + f.unstaged == 0 && (!include_untracked || f.untracked == 0) {
                // Same code either way; the words say what is actually there.
                let msg = if f.untracked > 0 {
                    "Only new files here — tick Include untracked files to stash them."
                } else {
                    "Nothing to stash — the working tree is clean."
                };
                return Some(refuse("nothing-to-stash", msg));
            }
            None
        }
        StashIntent::Apply { exists, .. } => {
            if !exists {
                return Some(no_stash());
            }
            if f.conflicted > 0 {
                return Some(refuse("unmerged-paths", "Resolve the conflicts first."));
            }
            None
        }
        StashIntent::Drop { exists } => {
            if !exists {
                return Some(no_stash());
            }
            None
        }
        StashIntent::RestoreFile { .. } => None,
    }
}

// --- Reading -------------------------------------------------------------

/// One record of `git stash list`, before any interpretation.
#[derive(Debug, Clone, PartialEq)]
pub struct StashRecord {
    pub index: usize,
    pub oid: String,
    /// 2 for a plain stash; 3 when `--include-untracked` added a third parent
    /// holding the untracked files.
    pub parents: usize,
    /// Committer date, ISO 8601 (`%cI`).
    pub date: String,
    /// The reflog subject (`%gs`): the stash message as `git stash list` shows it.
    pub subject: String,
}

const LIST_FORMAT: &str = "--format=%gd%x1f%H%x1f%P%x1f%cI%x1f%gs";

fn selector(index: usize) -> String {
    format!("stash@{{{}}}", index)
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn nul_chunks(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    raw.split(|b| *b == 0).filter(|c| !c.is_empty())
}

/// Parse `git stash list -z --format=%gd%x1f%H%x1f%P%x1f%cI%x1f%gs`. Pure.
/// A selector that does not read `stash@{n}` (a `--date` setting could change
/// it) falls back to the record's position, which is the same number.
pub fn parse_stash_list(raw: &[u8]) -> Vec<StashRecord> {
    nul_chunks(raw)
        .enumerate()
        .filter_map(|(position, chunk)| {
            let text = lossy(chunk);
            let f: Vec<&str> = text.splitn(5, '\x1f').collect();
            if f.len() < 5 {
                return None;
            }
            let index = f[0]
                .trim()
                .strip_prefix("stash@{")
                .and_then(|s| s.strip_suffix('}'))
                .and_then(|s| s.parse().ok())
                .unwrap_or(position);
            Some(StashRecord {
                index,
                oid: f[1].trim().to_string(),
                parents: f[2].split_whitespace().count(),
                date: f[3].trim().to_string(),
                subject: f[4].trim_end_matches(['\n', '\r']).to_string(),
            })
        })
        .collect()
}

/// Split a stash subject into (branch, message, is_autostash). Git writes
/// "WIP on <branch>: <short> <subject>" for an unnamed stash and
/// "On <branch>: <message>" for a named one; anything else (a rebase's
/// "autostash", a hand-stored entry) is kept whole as the message. Only the
/// exact subject `autostash` — what `git rebase --autostash` stores when the
/// re-apply fails — counts as an autostash.
pub fn parse_subject(subject: &str) -> (Option<String>, String, bool) {
    let is_autostash = crate::sync::is_autostash_subject(subject);
    let rest = subject
        .strip_prefix("WIP on ")
        .or_else(|| subject.strip_prefix("On "));
    match rest.and_then(|r| r.split_once(": ")) {
        Some((branch, message)) => {
            let branch = if branch.is_empty() || branch == "(no branch)" {
                None
            } else {
                Some(branch.to_string())
            };
            (branch, message.to_string(), is_autostash)
        }
        None => (None, subject.to_string(), is_autostash),
    }
}

/// Parse `git diff --name-status -z`: `<status>\0<path>\0`, or for a rename or
/// copy `<status><score>\0<from>\0<to>\0`. Pure; line counts are filled in
/// from `--numstat` afterwards.
pub fn parse_name_status(raw: &[u8]) -> Vec<StashFile> {
    let chunks: Vec<&[u8]> = nul_chunks(raw).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chunks.len() {
        let status = lossy(chunks[i]);
        i += 1;
        let letter = status.chars().next().unwrap_or('M');
        if matches!(letter, 'R' | 'C') {
            // The old name comes first; the stash's file is the new one.
            i += 1;
        }
        if let Some(path) = chunks.get(i) {
            out.push(StashFile {
                path: lossy(path),
                status: letter.to_string(),
                added: None,
                removed: None,
            });
        }
        i += 1;
    }
    out
}

/// The first `limit` records of the stash list.
async fn list_records(repo: &str, limit: usize) -> Result<Vec<StashRecord>, AppError> {
    let out = GitCmd::at(repo)
        .args(["stash", "list", "-z", &format!("-n{}", limit), LIST_FORMAT])
        .timeout(READ_TIMEOUT)
        .run()
        .await?;
    if !out.ok() {
        return Err(AppError::Command(format!("git stash list failed: {}", out.stderr)));
    }
    Ok(parse_stash_list(&out.stdout))
}

/// The record at `stash@{index}`, or None when the list is shorter.
async fn find_record(repo: &str, index: usize) -> Result<Option<StashRecord>, AppError> {
    Ok(list_records(repo, index + 1).await?.into_iter().find(|r| r.index == index))
}

/// The oid `stash@{index}` resolves to right now, if anything.
async fn oid_at(repo: &str, index: usize) -> Option<String> {
    GitCmd::at(repo)
        .args(["rev-parse", "--verify", "--quiet", &selector(index)])
        .ok_text()
        .await
        .filter(|s| !s.is_empty())
}

/// How many NUL-separated names a plumbing command prints; 0 when it fails.
async fn count_names(repo: &str, args: &[&str]) -> usize {
    match GitCmd::at(repo).args(args.iter().copied()).timeout(READ_TIMEOUT).run().await {
        Ok(o) if o.ok() => nul_chunks(&o.stdout).count(),
        _ => 0,
    }
}

async fn tracked_count(repo: &str, oid: &str) -> usize {
    count_names(repo, &["diff-tree", "-r", "--no-commit-id", "--name-only", "-z", &format!("{}^1", oid), oid]).await
}

async fn untracked_count(repo: &str, rec: &StashRecord) -> usize {
    if rec.parents < 3 {
        return 0;
    }
    count_names(repo, &["ls-tree", "-r", "--name-only", "-z", &format!("{}^3", rec.oid)]).await
}

async fn entry_for(repo: &str, rec: &StashRecord) -> StashEntry {
    let (branch, message, is_autostash) = parse_subject(&rec.subject);
    StashEntry {
        index: rec.index,
        ref_: selector(rec.index),
        oid: rec.oid.clone(),
        message,
        branch,
        date: rec.date.clone(),
        tracked_files: tracked_count(repo, &rec.oid).await,
        untracked_files: untracked_count(repo, rec).await,
        is_autostash,
    }
}

fn stash_ref(rec: &StashRecord) -> StashRef {
    StashRef {
        ref_: selector(rec.index),
        oid: rec.oid.clone(),
        message: rec.subject.clone(),
    }
}

/// The id of the empty tree, computed rather than hard-coded so it is right
/// for SHA-256 repositories too.
async fn empty_tree(repo: &str) -> Result<String, AppError> {
    GitCmd::at(repo)
        .args(["hash-object", "-t", "tree", "--stdin"])
        .stdin_bytes(Vec::new())
        .timeout(READ_TIMEOUT)
        .text()
        .await
}

async fn diff_bytes(repo: &str, args: &[&str]) -> Result<Vec<u8>, AppError> {
    let out = GitCmd::at(repo).args(args.iter().copied()).timeout(READ_TIMEOUT).run().await?;
    if !out.ok() {
        return Err(AppError::Command(format!("git diff failed: {}", out.stderr)));
    }
    Ok(out.stdout)
}

/// Every file a stash holds: the tracked changes (against the stash's base
/// commit) and, when there is a third parent, the untracked files.
async fn stash_files(repo: &str, rec: &StashRecord) -> Result<Vec<StashFile>, AppError> {
    let base = format!("{}^1", rec.oid);
    let raw = diff_bytes(repo, &["diff", "--name-status", "-z", "--no-color", "--no-ext-diff", &base, &rec.oid]).await?;
    let mut files = parse_name_status(&raw);
    let raw = diff_bytes(repo, &["diff", "--numstat", "-z", "--no-color", "--no-ext-diff", &base, &rec.oid]).await?;
    let stats = git_status::parse_numstat(&raw);
    for f in files.iter_mut() {
        if let Some((a, r, _)) = stats.get(&f.path) {
            f.added = *a;
            f.removed = *r;
        }
    }
    if rec.parents >= 3 {
        let third = format!("{}^3", rec.oid);
        let empty = empty_tree(repo).await?;
        let raw = diff_bytes(repo, &["diff", "--numstat", "-z", "--no-color", "--no-ext-diff", &empty, &third]).await?;
        let stats = git_status::parse_numstat(&raw);
        // ls-tree, not the numstat, is the list: an empty new file has no numstat line.
        let out = GitCmd::at(repo)
            .args(["ls-tree", "-r", "--name-only", "-z", &third])
            .timeout(READ_TIMEOUT)
            .run()
            .await?;
        for chunk in nul_chunks(&out.stdout) {
            let path = lossy(chunk);
            let (added, removed) = stats.get(&path).map(|(a, r, _)| (*a, *r)).unwrap_or((None, None));
            files.push(StashFile { path, status: "untracked".into(), added, removed });
        }
    }
    Ok(files)
}

pub async fn stash_list(repo_path: &str) -> Result<Vec<StashEntry>, AppError> {
    let recs = list_records(repo_path, MAX_LISTED).await?;
    let mut out = Vec::with_capacity(recs.len());
    for rec in &recs {
        out.push(entry_for(repo_path, rec).await);
    }
    Ok(out)
}

pub async fn stash_show(repo_path: &str, index: usize) -> Result<StashDetail, AppError> {
    let rec = find_record(repo_path, index)
        .await?
        .ok_or_else(|| AppError::NotFound(STALE.into()))?;
    let entry = entry_for(repo_path, &rec).await;
    let files = stash_files(repo_path, &rec).await?;
    Ok(StashDetail { entry, files })
}

pub async fn stash_file_diff(repo_path: &str, index: usize, path: &str) -> Result<FileDiff, AppError> {
    validate_repo_path(path)?;
    let rec = find_record(repo_path, index)
        .await?
        .ok_or_else(|| AppError::NotFound(STALE.into()))?;
    let files = stash_files(repo_path, &rec).await?;
    let Some(file) = files.iter().find(|f| f.path == path) else {
        return Err(AppError::NotFound(format!("{} isn't in that stash.", path)));
    };
    if file.status != "untracked" {
        return git_status::rev_diff(repo_path, &format!("{}^1", rec.oid), &rec.oid, path).await;
    }
    // A new file has nothing to diff against: show it as added from nothing.
    let empty = empty_tree(repo_path).await?;
    let third = format!("{}^3", rec.oid);
    let literal = format!(":(literal){}", path);
    let out = GitCmd::at(repo_path)
        .args(["diff", "--no-color", "--no-ext-diff", &empty, &third, "--", &literal])
        .timeout(READ_TIMEOUT)
        .run()
        .await?;
    if !out.ok() && out.stdout.is_empty() && !out.stderr.is_empty() {
        return Err(AppError::Command(format!("git diff failed: {}", out.stderr)));
    }
    Ok(git_status::render_diff(out.stdout, path, false, "This stashed file is empty."))
}

// --- Mutations -----------------------------------------------------------

fn sub_is_dirty(e: &ChangeEntry) -> bool {
    e.is_submodule && (e.sub_tracked_changes || e.sub_untracked || e.sub_commit_changed)
}

fn dirty_submodules(status: &RepoStatus) -> Vec<String> {
    status.entries.iter().filter(|e| sub_is_dirty(e)).map(|e| e.path.clone()).collect()
}

fn conflicted_paths(status: &RepoStatus) -> Vec<String> {
    status
        .entries
        .iter()
        .filter(|e| e.kind == "conflicted")
        .map(|e| e.path.clone())
        .collect()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

fn git_said(out: &crate::git_exec::GitOutput) -> String {
    format!("{}\n{}", out.text(), out.stderr).trim().to_string()
}

/// `git stash push` — never `--all`, so ignored files (.env, node_modules) stay
/// exactly where they are. `paths`, when given, limits the stash to those files
/// (NUL-fed, `:(literal)`); an explicitly empty selection is refused rather
/// than widened to everything.
pub async fn stash_push(repo_path: &str, message: Option<&str>, include_untracked: bool, paths: Option<Vec<String>>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let has_paths = paths.as_ref().is_some_and(|p| !p.is_empty());
    let intent = StashIntent::Push { include_untracked, has_paths };
    if let Some(r) = git_ops::check(&Intent::Stash(intent), &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    if paths.is_some() && !has_paths {
        return Ok(OpResult::refused(
            refuse("no-paths", "Nothing selected. A partial stash only ever acts on files you pick."),
            before,
        ));
    }

    let mut cmd = GitCmd::at(repo_path).pinned().args(["stash", "push", "--quiet"]);
    if include_untracked {
        cmd = cmd.arg("--include-untracked");
    }
    if let Some(m) = message.map(str::trim).filter(|m| !m.is_empty()) {
        // One argument, so a message starting with '-' can never be an option.
        cmd = cmd.arg(format!("--message={}", m));
    }
    cmd = match &paths {
        Some(p) => cmd
            .args(["--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin_bytes(pathspec_stdin(p)?),
        None => cmd.arg("--"),
    };
    let out = cmd.timeout(LOCAL_TIMEOUT).run().await?;

    let after = snapshot(repo_path).await?;
    let submodules = dirty_submodules(&after);
    if !out.ok() {
        let a = git_advice::explain(GitOp::Stash, &git_said(&out), &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }
    // `git stash push` exits 0 having stashed nothing when the only dirt is
    // untracked (without -u), inside a submodule, or outside the pathspec.
    if after.stash_count != before.stash_count + 1 {
        let said = format!("No local changes to save\n{}", git_said(&out));
        let a = git_advice::explain(GitOp::Stash, &said, &advice_ctx(&after));
        let mut parts: Vec<String> = Vec::new();
        if has_paths {
            parts.push("None of the selected files had changes to stash.".into());
        }
        if !submodules.is_empty() {
            parts.push(format!(
                "Changes inside {} can't be stashed from here — a stash never includes submodules. Open the submodule as its own repository to stash them.",
                submodules.join(", ")
            ));
        }
        let detail = if parts.is_empty() { a.guidance.clone() } else { parts.join(" ") };
        return Ok(OpResult {
            detail,
            stash: Some(StashOutcome {
                action: "push".into(),
                stash_count: after.stash_count,
                not_stashed_submodules: submodules,
                ..StashOutcome::default()
            }),
            ..OpResult::failed(a, after)
        });
    }

    // Prove what landed by reading the new entry, not by trusting the tree.
    let Some(rec) = find_record(repo_path, 0).await? else {
        return Err(AppError::Command("the stash was created but stash@{0} cannot be read".into()));
    };
    let entry = entry_for(repo_path, &rec).await;
    let total = entry.tracked_files + entry.untracked_files;
    let mut detail = format!(
        "{} now holds {}{}.",
        entry.ref_,
        plural(entry.tracked_files, "tracked file", "tracked files"),
        if entry.untracked_files > 0 {
            format!(" and {}", plural(entry.untracked_files, "new file", "new files"))
        } else {
            String::new()
        }
    );
    // Every status entry is a change; submodules are named separately below.
    let left = after.entries.iter().filter(|e| !e.is_submodule).count();
    if left > 0 {
        detail.push_str(&format!(" {} still in the tree.", plural(left, "change is", "changes are")));
    }
    if !submodules.is_empty() {
        detail.push_str(&format!(
            " Changes inside {} were not stashed — a stash never includes submodules.",
            submodules.join(", ")
        ));
    }
    let outcome = StashOutcome {
        action: "push".into(),
        entry: Some(stash_ref(&rec)),
        stash_count: after.stash_count,
        conflicts: Vec::new(),
        kept: true,
        recovery: Some(format!("git stash pop {}", shell_quote(&entry.ref_))),
        not_stashed_submodules: submodules,
    };
    Ok(OpResult {
        stash: Some(outcome),
        ..OpResult::done(format!("Stashed {}.", plural(total, "change", "changes")), detail, after)
    })
}

/// Where `stash@{index}` stands, for the pure check.
enum Located {
    /// The entry the caller meant is at that index.
    Found(StashRecord),
    /// The list is shorter than that, or the ref does not resolve.
    Missing,
    /// Something is at that index, but not the entry the caller saw: the list
    /// shifted underneath (a pop or drop elsewhere) since it was read.
    Moved,
}

impl Located {
    fn record(&self) -> Option<&StashRecord> {
        match self {
            Located::Found(r) => Some(r),
            _ => None,
        }
    }

    /// `no-stash` says which of the two it was.
    fn explain(&self, r: Refusal) -> Refusal {
        if r.code == "no-stash" && matches!(self, Located::Moved) {
            refuse("no-stash", MOVED)
        } else {
            r
        }
    }
}

/// The caller's oid names the entry; a short form of at least seven hex digits
/// is accepted for the shell suites.
fn oid_matches(rec: &StashRecord, want: &str) -> bool {
    let want = want.trim();
    !want.is_empty() && (rec.oid == want || (want.len() >= 7 && rec.oid.starts_with(want)))
}

/// Facts about `stash@{index}` for the pure check: it exists only when the
/// list is long enough *and* the ref resolves *and*, when the caller says
/// which commit it showed (`expected_oid`), that is what sits there now.
async fn locate(repo_path: &str, index: usize, stash_count: usize, expected_oid: Option<&str>) -> Result<Located, AppError> {
    if index >= stash_count {
        return Ok(Located::Missing);
    }
    let Some(rec) = find_record(repo_path, index).await? else {
        return Ok(Located::Missing);
    };
    if oid_at(repo_path, index).await.as_deref() != Some(rec.oid.as_str()) {
        return Ok(Located::Missing);
    }
    match expected_oid.map(str::trim).filter(|s| !s.is_empty()) {
        Some(want) if !oid_matches(&rec, want) => Ok(Located::Moved),
        _ => Ok(Located::Found(rec)),
    }
}

/// `git stash apply|pop [--index] stash@{n}`. A conflict leaves the conflicted
/// files in the tree and the entry in the list (git does the same); the
/// result says so and names the files. `oid`, when given, must be the entry's
/// commit id as the list showed it — otherwise `no-stash`, so a stale list
/// never applies a neighbour.
pub async fn stash_apply(repo_path: &str, index: usize, pop: bool, restore_index: bool, oid: Option<&str>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let located = locate(repo_path, index, before.stash_count, oid).await?;
    let intent = StashIntent::Apply { exists: located.record().is_some(), pop };
    if let Some(r) = git_ops::check(&Intent::Stash(intent), &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(located.explain(r), before));
    }
    let Located::Found(rec) = located else { unreachable!("checked above") };
    let sel = selector(index);
    // Counted before the command: a pop drops the reflog entry.
    let entry = entry_for(repo_path, &rec).await;
    let action = if pop { "pop" } else { "apply" };

    let mut args: Vec<&str> = vec!["stash", action];
    if restore_index {
        args.push("--index");
    }
    args.push("--quiet");
    args.push(&sel);
    let out = GitCmd::at(repo_path).pinned().args(args).timeout(NET_TIMEOUT).run().await?;

    let after = snapshot(repo_path).await?;
    // Still in the list means the same commit is still at that index.
    let kept = oid_at(repo_path, index).await.as_deref() == Some(rec.oid.as_str());
    let conflicts = conflicted_paths(&after);
    let outcome = StashOutcome {
        action: action.into(),
        entry: Some(stash_ref(&rec)),
        stash_count: after.stash_count,
        conflicts: conflicts.clone(),
        kept,
        recovery: None,
        not_stashed_submodules: Vec::new(),
    };

    if !out.ok() {
        let mut said = git_said(&out);
        // With --quiet a conflicting apply prints nothing at all: the conflict
        // is measured, and git's own sentence about the entry is added when
        // it did not print it itself.
        if !conflicts.is_empty() && after.operation.is_none() && !said.contains("CONFLICT") && !said.contains("The stash entry is kept") {
            said = format!(
                "CONFLICT (content): Merge conflict in {}\nThe stash entry is kept in case you need it again.\n{}",
                conflicts.join(", "),
                said
            )
            .trim()
            .to_string();
        }
        let a = git_advice::explain(GitOp::Stash, &said, &advice_ctx(&after));
        return Ok(OpResult {
            stash: Some(outcome),
            ..OpResult::failed(a, after)
        });
    }

    let headline = if pop { format!("Popped {}.", sel) } else { format!("Applied {}.", sel) };
    let mut detail = format!(
        "{}{} back in the tree{}.",
        plural(entry.tracked_files + entry.untracked_files, "file is", "files are"),
        if entry.untracked_files > 0 {
            format!(" ({} of them new)", entry.untracked_files)
        } else {
            String::new()
        },
        if restore_index { " with the staged/unstaged split restored" } else { "" }
    );
    detail.push_str(match (pop, kept) {
        (false, _) => " The entry is still in the list.",
        (true, false) => " The entry was removed from the list.",
        // git said the pop worked, yet the same commit is still at that index.
        (true, true) => " git reported success, but the entry is still in the list.",
    });
    Ok(OpResult {
        stash: Some(outcome),
        ..OpResult::done(headline, detail, after)
    })
}

/// `git stash drop stash@{n}`, with the way back: the commit object survives
/// until git prunes it, and `git stash store` re-lists it. `oid` as for
/// `stash_apply`.
pub async fn stash_drop(repo_path: &str, index: usize, oid: Option<&str>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let located = locate(repo_path, index, before.stash_count, oid).await?;
    let intent = StashIntent::Drop { exists: located.record().is_some() };
    if let Some(r) = git_ops::check(&Intent::Stash(intent), &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(located.explain(r), before));
    }
    let Located::Found(rec) = located else { unreachable!("checked above") };
    let sel = selector(index);
    let recovery = format!("git stash store -m {} {}", shell_quote(&rec.subject), rec.oid);

    let out = GitCmd::at(repo_path)
        .pinned()
        .args(["stash", "drop", "--quiet", &sel])
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    let kept = oid_at(repo_path, index).await.as_deref() == Some(rec.oid.as_str());
    let outcome = StashOutcome {
        action: "drop".into(),
        entry: Some(stash_ref(&rec)),
        stash_count: after.stash_count,
        conflicts: Vec::new(),
        kept,
        recovery: Some(recovery.clone()),
        not_stashed_submodules: Vec::new(),
    };
    if !out.ok() {
        let a = git_advice::explain(GitOp::Stash, &git_said(&out), &advice_ctx(&after));
        return Ok(OpResult {
            stash: Some(outcome),
            ..OpResult::failed(a, after)
        });
    }
    if kept || after.stash_count + 1 != before.stash_count {
        let a = git_advice::explain(
            GitOp::Stash,
            &format!(
                "git stash drop exited 0 but the list still has {} entries (had {})\n{}",
                after.stash_count,
                before.stash_count,
                git_said(&out)
            ),
            &advice_ctx(&after),
        );
        return Ok(OpResult {
            stash: Some(outcome),
            ..OpResult::failed(a, after)
        });
    }
    Ok(OpResult {
        stash: Some(outcome),
        ..OpResult::done(
            format!("Dropped {}.", sel),
            format!(
                "The commit object survives until git prunes it; the undo command puts the entry back: {}",
                recovery
            ),
            after,
        )
    })
}

/// Put one file back the way a stash has it — index and worktree both, from
/// the stash commit (tracked) or its third parent (untracked). Both sides are
/// always restored: a worktree-only restore is refused on an unmerged path,
/// and an unmerged path left by a failed pop is exactly the recovery case.
/// `oid` as for `stash_apply`.
pub async fn stash_restore_file(repo_path: &str, index: usize, path: &str, oid: Option<&str>) -> Result<OpResult, AppError> {
    validate_repo_path(path)?;
    let before = snapshot(repo_path).await?;
    let located = locate(repo_path, index, before.stash_count, oid).await?;
    let files = match located.record() {
        Some(r) => stash_files(repo_path, r).await?,
        None => Vec::new(),
    };
    let file = files.iter().find(|f| f.path == path).cloned();
    let intent = StashIntent::RestoreFile { exists: located.record().is_some(), in_stash: file.is_some() };
    if let Some(mut r) = git_ops::check(&Intent::Stash(intent), &StatusFacts::from(&before)) {
        if r.code == "not-in-stash" {
            r.message = format!("{} isn't in that stash.", path);
        }
        return Ok(OpResult::refused(located.explain(r), before));
    }
    let Located::Found(rec) = located else { unreachable!("checked above") };
    let file = file.expect("checked above");
    let sel = selector(index);
    let untracked = file.status == "untracked";
    // The commit id, not the selector: exact even if the list shifts underneath.
    let source = if untracked { format!("{}^3", rec.oid) } else { rec.oid.clone() };
    let was_conflicted = before.entries.iter().any(|e| e.path == path && e.kind == "conflicted");

    let out = GitCmd::at(repo_path)
        .pinned()
        .args([
            "restore",
            &format!("--source={}", source),
            "--staged",
            "--worktree",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ])
        .stdin_bytes(pathspec_stdin(&[path.to_string()])?)
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    let outcome = StashOutcome {
        action: "restore".into(),
        entry: Some(stash_ref(&rec)),
        stash_count: after.stash_count,
        conflicts: conflicted_paths(&after),
        kept: oid_at(repo_path, index).await.as_deref() == Some(rec.oid.as_str()),
        recovery: None,
        not_stashed_submodules: Vec::new(),
    };
    if !out.ok() {
        // restore's refusals read like checkout's (overwritten, pathspec, unmerged).
        let a = git_advice::explain(GitOp::Checkout, &git_said(&out), &advice_ctx(&after));
        return Ok(OpResult {
            stash: Some(outcome),
            ..OpResult::failed(a, after)
        });
    }

    // Verify: the index now holds the stash's blob (or nothing, when the stash
    // deleted the file), and the path is no longer conflicted.
    let expected = GitCmd::at(repo_path)
        .args(["rev-parse", "--verify", "--quiet", &format!("{}:{}", source, path)])
        .ok_text()
        .await
        .filter(|s| !s.is_empty());
    let actual = GitCmd::at(repo_path)
        .args(["rev-parse", "--verify", "--quiet", &format!(":0:{}", path)])
        .ok_text()
        .await
        .filter(|s| !s.is_empty());
    let still_conflicted = after.entries.iter().any(|e| e.path == path && e.kind == "conflicted");
    if expected != actual || still_conflicted {
        let a = git_advice::explain(
            GitOp::Checkout,
            &format!(
                "git restore exited 0 but {} does not match the stash's version afterwards{}\n{}",
                path,
                if still_conflicted { " (it is still conflicted)" } else { "" },
                git_said(&out)
            ),
            &advice_ctx(&after),
        );
        return Ok(OpResult {
            stash: Some(outcome),
            ..OpResult::failed(a, after)
        });
    }

    let detail = match file.status.as_str() {
        "untracked" => "The stash's new file is back in the tree and staged.".to_string(),
        "D" => "The stash deleted this file, so it was removed from the tree and the index.".to_string(),
        _ => format!(
            "The file now matches the stash's version and is staged{}",
            if was_conflicted { " — the conflict on it is resolved." } else { "." }
        ),
    };
    Ok(OpResult {
        stash: Some(outcome),
        ..OpResult::done(format!("Restored {} from {}.", path, sel), detail, after)
    })
}

/// Shell-suite driver for the `stash_*` probe ops (see probe.rs).
///
/// - `stash_list`
/// - `stash_show <n>`
/// - `stash_diff <n>,<path>`
/// - `stash_push [<msg>][,untracked]` — an empty first arg means no message
/// - `stash_push_paths <msg>|-,<path>[,<path>…][,untracked]` — `-` means no message
/// - `stash_apply <n>[,pop][,index][,<oid>]`
/// - `stash_drop <n>[,<oid>]`
/// - `stash_restore <n>,<path>[,<oid>]`
///
/// The optional trailing `<oid>` is the commit id the caller saw at that index
/// (as the UI sends it); the operation is refused when the entry moved.
#[cfg(test)]
pub(crate) async fn probe(op: &str, repo: &str, args: &[String]) {
    use crate::probe::tests::{emit, emit_err, show};
    let first = args.first().map(|s| s.as_str()).unwrap_or("");
    let second = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let third = args.get(2).map(|s| s.as_str());
    let untracked = args.iter().any(|s| s == "untracked");
    let index = || first.parse::<usize>();
    match op {
        "stash_list" => match stash_list(repo).await {
            Ok(v) => emit(&v),
            Err(e) => emit_err(e),
        },
        "stash_show" => match index() {
            Ok(n) => match stash_show(repo, n).await {
                Ok(d) => emit(&d),
                Err(e) => emit_err(e),
            },
            Err(_) => emit_err("stash index must be a number"),
        },
        "stash_diff" => match index() {
            Ok(n) => match stash_file_diff(repo, n, second).await {
                Ok(d) => emit(&d),
                Err(e) => emit_err(e),
            },
            Err(_) => emit_err("stash index must be a number"),
        },
        "stash_push" => {
            let msg = args.first().filter(|s| *s != "untracked").map(|s| s.as_str());
            show(stash_push(repo, msg, untracked, None).await)
        }
        "stash_push_paths" => {
            let msg = if first == "-" { None } else { Some(first) };
            let paths: Vec<String> = args.iter().skip(1).filter(|s| *s != "untracked").cloned().collect();
            show(stash_push(repo, msg, untracked, Some(paths)).await)
        }
        "stash_apply" => match index() {
            Ok(n) => {
                let pop = args.iter().skip(1).any(|s| s == "pop");
                let restore_index = args.iter().skip(1).any(|s| s == "index");
                let oid = args.iter().skip(1).find(|s| *s != "pop" && *s != "index").map(|s| s.as_str());
                show(stash_apply(repo, n, pop, restore_index, oid).await)
            }
            Err(_) => emit_err("stash index must be a number"),
        },
        "stash_drop" => match index() {
            Ok(n) => show(stash_drop(repo, n, args.get(1).map(|s| s.as_str())).await),
            Err(_) => emit_err("stash index must be a number"),
        },
        "stash_restore" => match index() {
            Ok(n) => show(stash_restore_file(repo, n, second, third).await),
            Err(_) => emit_err("stash index must be a number"),
        },
        other => emit_err(format!("unknown stash op {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(selector: &str, oid: &str, parents: &str, date: &str, subject: &str) -> Vec<u8> {
        let mut v = Vec::new();
        for (i, f) in [selector, oid, parents, date, subject].iter().enumerate() {
            if i > 0 {
                v.push(0x1f);
            }
            v.extend_from_slice(f.as_bytes());
        }
        v.push(0);
        v
    }

    #[test]
    fn stash_list_records_are_parsed_with_their_parent_count() {
        let mut raw = record(
            "stash@{0}",
            "b3982e7d79c7836a3e53e712c57b36f058a6b9e2",
            "9589a7a34b2446640 e1e8a0eff34d718ea 45644c064526a916e",
            "2026-09-24T01:41:37+05:30",
            "On main: fix: colons in the message",
        );
        raw.extend(record(
            "stash@{1}",
            "0123456789abcdef0123456789abcdef01234567",
            "aaaaaaa bbbbbbb",
            "2026-09-23T10:00:00+00:00",
            "WIP on main: 1a2b3c initial",
        ));
        let recs = parse_stash_list(&raw);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].index, 0);
        assert_eq!(recs[0].oid, "b3982e7d79c7836a3e53e712c57b36f058a6b9e2");
        assert_eq!(recs[0].parents, 3, "three parents: an untracked tree exists");
        assert_eq!(recs[0].date, "2026-09-24T01:41:37+05:30");
        assert_eq!(recs[0].subject, "On main: fix: colons in the message");
        assert_eq!(recs[1].index, 1);
        assert_eq!(recs[1].parents, 2);
        assert_eq!(recs[1].subject, "WIP on main: 1a2b3c initial");
        // Nothing stashed: nothing parsed, no panic.
        assert!(parse_stash_list(b"").is_empty());
        // A selector git printed in another form falls back to the position.
        let odd = record("stash@{2 minutes ago}", "abc", "a b", "d", "On main: x");
        assert_eq!(parse_stash_list(&odd)[0].index, 0);
    }

    #[test]
    fn subjects_split_into_branch_message_and_autostash() {
        assert_eq!(
            parse_subject("WIP on main: 1a2b3c initial commit"),
            (Some("main".into()), "1a2b3c initial commit".into(), false)
        );
        assert_eq!(
            parse_subject("On feature: fix header"),
            (Some("feature".into()), "fix header".into(), false)
        );
        // Colons inside the message stay in the message.
        assert_eq!(
            parse_subject("On main: fix: colons: everywhere"),
            (Some("main".into()), "fix: colons: everywhere".into(), false)
        );
        // A rebase's failed autostash pop stores exactly this subject, no prefix.
        assert_eq!(parse_subject("autostash"), (None, "autostash".into(), true));
        // A stash that merely mentions the word is a person's, not a rebase's.
        assert!(!parse_subject("On main: gitswitch autostash before sync").2);
        assert!(!parse_subject("On main: fix autostash bug").2);
        assert!(!parse_subject("On main: fix header").2);
        // Detached HEAD has no branch to report.
        assert_eq!(parse_subject("WIP on (no branch): 7a38086 other").0, None);
        // A hand-stored entry keeps its whole subject.
        assert_eq!(parse_subject("my own words"), (None, "my own words".into(), false));
    }

    #[test]
    fn name_status_records_include_renames_by_their_new_name() {
        let raw = b"M\0a.txt\0A\0new dir/b.txt\0D\0gone.txt\0R100\0old.txt\0renamed.txt\0";
        let files = parse_name_status(raw);
        let got: Vec<(String, String)> = files.iter().map(|f| (f.status.clone(), f.path.clone())).collect();
        assert_eq!(
            got,
            vec![
                ("M".to_string(), "a.txt".to_string()),
                ("A".to_string(), "new dir/b.txt".to_string()),
                ("D".to_string(), "gone.txt".to_string()),
                ("R".to_string(), "renamed.txt".to_string()),
            ]
        );
        assert!(parse_name_status(b"").is_empty());
    }

    fn clean() -> StatusFacts {
        StatusFacts { ..Default::default() }
    }

    fn merging() -> StatusFacts {
        StatusFacts {
            operation: Some("merge".into()),
            abort_command: Some("git merge --abort".into()),
            conflicted: 1,
            staged: 1,
            ..clean()
        }
    }

    #[test]
    fn every_stash_mutation_but_restore_waits_for_the_operation_in_progress() {
        let f = merging();
        for intent in [
            StashIntent::Push { include_untracked: false, has_paths: false },
            StashIntent::Apply { exists: true, pop: false },
            StashIntent::Apply { exists: true, pop: true },
            StashIntent::Drop { exists: true },
        ] {
            let r = check_stash(&intent, &f).unwrap();
            assert_eq!(r.code, "operation-in-progress", "{:?}", intent);
            assert!(r.message.contains("git merge --abort"));
        }
        // Recovering a file from a stash is resolution work.
        assert!(check_stash(&StashIntent::RestoreFile { exists: true, in_stash: true }, &f).is_none());
        // Busy wins over "does not exist", so the message is about the merge.
        assert_eq!(check_stash(&StashIntent::Drop { exists: false }, &f).unwrap().code, "operation-in-progress");
    }

    #[test]
    fn a_clean_tree_has_nothing_to_stash_unless_untracked_files_are_asked_for() {
        let push = |include_untracked: bool| StashIntent::Push { include_untracked, has_paths: false };
        assert_eq!(check_stash(&push(false), &clean()).unwrap().code, "nothing-to-stash");
        assert_eq!(check_stash(&push(true), &clean()).unwrap().code, "nothing-to-stash");
        let only_untracked = StatusFacts { untracked: 2, ..clean() };
        let r = check_stash(&push(false), &only_untracked).unwrap();
        assert_eq!(r.code, "nothing-to-stash");
        // Same code, but the words name the new files rather than calling the tree clean.
        assert!(r.message.contains("Only new files here"), "{}", r.message);
        assert!(check_stash(&push(false), &clean()).unwrap().message.contains("working tree is clean"));
        assert!(check_stash(&push(true), &only_untracked).is_none());
        assert!(check_stash(&push(false), &StatusFacts { staged: 1, ..clean() }).is_none());
        assert!(check_stash(&push(false), &StatusFacts { unstaged: 1, ..clean() }).is_none());
        // A stash needs a commit to be based on.
        assert_eq!(check_stash(&push(false), &StatusFacts { unborn: true, staged: 1, ..clean() }).unwrap().code, "unborn-head");
        // Conflicts left by a failed pop (no operation) can't be stashed either.
        let r = check_stash(&push(false), &StatusFacts { conflicted: 1, unstaged: 1, ..clean() }).unwrap();
        assert_eq!(r.code, "unmerged-paths");
        assert!(r.message.contains("half a merge"));
    }

    #[test]
    fn a_missing_entry_is_refused_by_name_before_anything_else_about_the_tree() {
        assert_eq!(check_stash(&StashIntent::Apply { exists: false, pop: true }, &clean()).unwrap().code, "no-stash");
        assert_eq!(check_stash(&StashIntent::Drop { exists: false }, &clean()).unwrap().code, "no-stash");
        assert_eq!(check_stash(&StashIntent::RestoreFile { exists: false, in_stash: false }, &clean()).unwrap().code, "no-stash");
        assert_eq!(check_stash(&StashIntent::RestoreFile { exists: true, in_stash: false }, &clean()).unwrap().code, "not-in-stash");
        assert!(check_stash(&StashIntent::RestoreFile { exists: true, in_stash: true }, &clean()).is_none());
        assert!(check_stash(&StashIntent::Apply { exists: true, pop: false }, &clean()).is_none());
        assert!(check_stash(&StashIntent::Drop { exists: true }, &clean()).is_none());
        // Applying onto conflicts would pile a merge on a merge.
        assert_eq!(
            check_stash(&StashIntent::Apply { exists: true, pop: false }, &StatusFacts { conflicted: 2, ..clean() }).unwrap().code,
            "unmerged-paths"
        );
        // Dropping does not touch the tree, so conflicts do not stop it.
        assert!(check_stash(&StashIntent::Drop { exists: true }, &StatusFacts { conflicted: 2, ..clean() }).is_none());
    }

    #[test]
    fn the_callers_oid_must_be_the_entry_at_that_index() {
        let rec = StashRecord { index: 0, oid: "b3982e7d79c7836a3e53e712c57b36f058a6b9e2".into(), parents: 2, date: String::new(), subject: "On main: x".into() };
        assert!(oid_matches(&rec, "b3982e7d79c7836a3e53e712c57b36f058a6b9e2"));
        assert!(oid_matches(&rec, "b3982e7"), "a short id of seven or more digits is accepted");
        assert!(!oid_matches(&rec, "b3982e"), "too short to be an id");
        assert!(!oid_matches(&rec, "0123456789abcdef0123456789abcdef01234567"));
        assert!(!oid_matches(&rec, ""));
        // Only a moved entry gets the "moved" wording; a missing one keeps the plain refusal.
        let r = Located::Moved.explain(no_stash());
        assert_eq!(r.code, "no-stash");
        assert!(r.message.contains("moved or was dropped"));
        assert!(Located::Missing.explain(no_stash()).message.contains("no longer exists"));
        // Other refusals pass through untouched.
        assert_eq!(Located::Moved.explain(refuse("unmerged-paths", "x")).code, "unmerged-paths");
    }

    #[test]
    fn selectors_and_recovery_commands_are_shell_safe() {
        assert_eq!(selector(0), "stash@{0}");
        assert_eq!(selector(12), "stash@{12}");
        assert_eq!(shell_quote("stash@{0}"), "'stash@{0}'");
        assert_eq!(shell_quote("On main: it's here"), "'On main: it'\\''s here'");
    }
}
