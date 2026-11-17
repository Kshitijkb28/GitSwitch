//! The everyday git loop: stage, unstage, discard, commit, pull, push,
//! submodules, abort.
//!
//! Every operation follows the same four phases:
//!
//! 1. **Snapshot** the repo (`git_status::repo_status`).
//! 2. **Check** preconditions against that snapshot — a pure function, so every
//!    refusal is unit-testable and nothing runs when we already know it fails.
//! 3. **Mutate** with one explicit git command. Never `--force`, never
//!    `--no-verify`, never a pathless `clean`, never an implicit pull strategy.
//! 4. **Verify** by re-reading the repo and reporting what *actually* changed —
//!    the same "proof, not a promise" idea as `git_history::fetch_repo`.

use crate::error::AppError;
use crate::git_advice::{self, Advice, AdviceCtx, GitOp};
use crate::git_exec::{GitCmd, LFS_TIMEOUT, LOCAL_TIMEOUT, NET_TIMEOUT};
use crate::git_status::{self, RepoStatus};
use crate::sparse::SubmoduleReport;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A refusal carries a stable `code` (for the UI and for tests) and a sentence
/// the user can act on.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

pub(crate) fn refuse(code: &str, message: impl Into<String>) -> Refusal {
    Refusal {
        code: code.to_string(),
        message: message.into(),
    }
}

/// What the caller is asking for — enough for `check` to decide, and nothing more.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Stage,
    Unstage { conflicted_selected: bool },
    Discard { has_paths: bool, submodule_selected: bool, conflicted_selected: bool },
    Commit { amend: bool, message_empty: bool },
    Push,
    /// `autostash` lets a rebase pull run on a dirty tree (git stashes and re-applies).
    Pull { mode: PullMode, autostash: bool },
    Submodule,
    Abort,
    /// Finish the operation in progress once its conflicts are staged.
    Continue,
    /// Assess or run a sync (sync.rs): fetch, rebase with my commits on top.
    Sync,
    SyncContinue,
    SyncAbort,
    /// Branches, undo, reset, detach, revert, cherry-pick, conflict sides,
    /// discard-all (tree.rs). The variant carries the facts its check needs.
    Tree(crate::tree::TreeIntent),
    /// Stash push / apply / drop / restore one file (stash.rs).
    Stash(crate::stash::StashIntent),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PullMode {
    FfOnly,
    Merge,
    Rebase,
}

impl PullMode {
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s {
            "ff-only" => Ok(PullMode::FfOnly),
            "merge" => Ok(PullMode::Merge),
            "rebase" => Ok(PullMode::Rebase),
            other => Err(AppError::Command(format!(
                "'{}' is not a pull mode — expected ff-only, merge or rebase",
                other
            ))),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            PullMode::FfOnly => "fast-forward only",
            PullMode::Merge => "merge",
            PullMode::Rebase => "rebase",
        }
    }
}

/// The snapshot `check` is allowed to see. Flat and owned so the preconditions
/// can be tested without a repository.
#[derive(Debug, Clone, Default)]
pub struct StatusFacts {
    pub staged: usize,
    pub unstaged: usize,
    pub conflicted: usize,
    pub operation: Option<String>,
    pub abort_command: Option<String>,
    pub continue_command: Option<String>,
    /// A sync is paused here (or in this repository's superproject).
    pub sync_in_progress: bool,
    pub detached: bool,
    pub unborn: bool,
    pub upstream: Option<String>,
    pub push_blocked: bool,
    pub push_locked: bool,
    pub push_reason: String,
    pub identity_matches: bool,
    pub identity_email: String,
    pub profile_email: Option<String>,
    pub email_scope: String,
    pub can_amend: bool,
    pub untracked: usize,
}

impl From<&RepoStatus> for StatusFacts {
    fn from(s: &RepoStatus) -> Self {
        StatusFacts {
            staged: s.staged_count,
            unstaged: s.unstaged_count,
            conflicted: s.conflicted_count,
            operation: s.operation.as_ref().map(|o| o.kind.clone()),
            abort_command: s.operation.as_ref().map(|o| o.abort_command.clone()),
            continue_command: s.operation.as_ref().and_then(|o| o.continue_command.clone()),
            sync_in_progress: s.sync.is_some(),
            detached: s.detached,
            unborn: s.unborn,
            upstream: s.upstream.clone(),
            push_blocked: s.push.blocked,
            push_locked: s.push.lock.locked,
            push_reason: s.push.reason.clone(),
            identity_matches: s.identity.matches_profile,
            identity_email: s.identity.email.clone(),
            profile_email: s.identity.profile_email.clone(),
            email_scope: s.identity.email_scope.clone(),
            can_amend: s.can_amend,
            untracked: s.untracked_count,
        }
    }
}

/// "There's a rebase in progress…" — the refusal every operation that would
/// fight an in-progress merge/rebase/cherry-pick shares.
pub(crate) fn busy(f: &StatusFacts, verb: &str) -> Option<Refusal> {
    f.operation.as_ref().map(|kind| {
        refuse(
            "operation-in-progress",
            format!(
                "There's a {} in progress. Finish or abort it before you {} ({}).",
                kind,
                verb,
                f.abort_command.clone().unwrap_or_default()
            ),
        )
    })
}

/// Everything that must be true before git runs. Pure.
pub fn check(intent: &Intent, f: &StatusFacts) -> Option<Refusal> {
    let busy = |verb: &str| -> Option<Refusal> { busy(f, verb) };

    // While a sync is paused, only the sync's own buttons and the resolution
    // work (stage, unstage, diff) make sense; everything else would fight it.
    let allowed_during_sync = match intent {
        Intent::Stage | Intent::Unstage { .. } | Intent::Discard { .. } | Intent::SyncContinue | Intent::SyncAbort => true,
        Intent::Tree(t) => t.allowed_during_sync(),
        Intent::Stash(s) => s.allowed_during_sync(),
        _ => false,
    };
    if f.sync_in_progress && !allowed_during_sync {
        return Some(refuse(
            "sync-in-progress",
            "An update is paused here. Continue or abort it first (the Changes tab shows both).",
        ));
    }

    match intent {
        Intent::Tree(t) => crate::tree::check_tree(t, f),
        Intent::Stash(s) => crate::stash::check_stash(s, f),

        Intent::Stage => None,

        Intent::Unstage { conflicted_selected } => {
            if *conflicted_selected {
                // `git reset HEAD -- <conflicted>` throws away the stage 1/2/3
                // entries and leaves the file looking resolved-to-HEAD, quietly
                // destroying the merge information.
                return Some(refuse(
                    "unmerged-paths",
                    "That file is part of a conflict. Unstaging it would throw away the merge information — resolve it instead.",
                ));
            }
            None
        }

        Intent::Discard {
            has_paths,
            submodule_selected,
            conflicted_selected,
        } => {
            if !has_paths {
                return Some(refuse(
                    "no-paths",
                    "Nothing selected. Discard only ever acts on files you pick.",
                ));
            }
            if *submodule_selected {
                return Some(refuse(
                    "submodule-selected",
                    "Those changes live inside a submodule. Open it as its own repository to discard them.",
                ));
            }
            // Inside a merge/rebase the stage-1/2/3 entries are the conflict
            // itself; outside one (a stash that failed to re-apply) restoring
            // the file to HEAD is exactly the way out, and the stash still
            // holds the change.
            if *conflicted_selected && f.operation.is_some() {
                return Some(refuse(
                    "unmerged-paths",
                    "Conflicted files can't be discarded here. Resolve the conflict, or abort the merge to undo all of it.",
                ));
            }
            None
        }

        Intent::Commit {
            amend,
            message_empty,
        } => {
            if f.conflicted > 0 {
                return Some(refuse(
                    "unmerged-paths",
                    format!(
                        "{} file(s) still have conflicts. Resolve and stage them first.",
                        f.conflicted
                    ),
                ));
            }
            if *amend {
                if !f.can_amend {
                    return Some(refuse(
                        "already-pushed",
                        "That commit is already on the remote. Amending it would need a force-push, which GitSwitch never does — make a new commit instead.",
                    ));
                }
                if let Some(r) = busy("amend") {
                    return Some(r);
                }
            } else if f.staged == 0 && f.operation.as_deref() != Some("merge") {
                // A merge is finished by exactly one commit, and that commit is
                // worth making even when nothing is staged: it records the second
                // parent. (Resolving every conflict in favour of our side leaves
                // the index identical to HEAD, which is exactly this case.)
                return Some(refuse(
                    "nothing-staged",
                    "Nothing is staged. Stage the files you want in this commit first.",
                ));
            }
            // Checked after "nothing staged" so the more fundamental blocker is
            // the one reported on an untouched repo.
            if *message_empty {
                return Some(refuse("empty-message", "A commit needs a message."));
            }
            if !f.identity_matches {
                let expected = f.profile_email.clone().unwrap_or_default();
                let where_from = if f.email_scope == "local" {
                    " This repo's own .git/config overrides the profile."
                } else {
                    ""
                };
                return Some(refuse(
                    "identity-mismatch",
                    format!(
                        "This folder should commit as {} but git is set to {}.{} Fix the identity before committing.",
                        expected, f.identity_email, where_from
                    ),
                ));
            }
            None
        }

        Intent::Push => {
            if f.push_locked {
                return Some(refuse("push-locked", f.push_reason.clone()));
            }
            if f.push_blocked {
                return Some(refuse("push-blocked", f.push_reason.clone()));
            }
            if f.unborn {
                return Some(refuse(
                    "unborn-head",
                    "There are no commits yet — nothing to push.",
                ));
            }
            if f.detached {
                return Some(refuse(
                    "detached-head",
                    "You're not on a branch, so there's no branch to push. Switch to one first.",
                ));
            }
            busy("push")
        }

        Intent::Pull { mode, autostash } => {
            if f.unborn {
                return Some(refuse(
                    "unborn-head",
                    "There are no commits yet. Pull needs a branch with history.",
                ));
            }
            if f.detached {
                return Some(refuse(
                    "detached-head",
                    "You're not on a branch. Switch to one before pulling.",
                ));
            }
            // A merge writes a new commit, and a rebase re-commits yours, both
            // under whatever identity git has here — the same reason a commit
            // is refused on a mismatch. A fast-forward writes nothing.
            if *mode != PullMode::FfOnly && !f.identity_matches {
                let expected = f.profile_email.clone().unwrap_or_default();
                return Some(refuse(
                    "identity-mismatch",
                    format!(
                        "This folder should commit as {} but git is set to {}. A {} would record commits under that identity — fix it first, or pull fast-forward only.",
                        expected,
                        f.identity_email,
                        if *mode == PullMode::Rebase { "rebase" } else { "merge" }
                    ),
                ));
            }
            if f.upstream.is_none() {
                return Some(refuse(
                    "no-upstream",
                    "This branch isn't tracking a remote branch yet, so there's nothing to pull from.",
                ));
            }
            if let Some(r) = busy("pull") {
                return Some(r);
            }
            if *mode == PullMode::Rebase && !*autostash && (f.staged > 0 || f.unstaged > 0) {
                // --no-autostash is the default on purpose: a silent
                // stash/unstash cycle can fail halfway and leave work in a
                // stash nobody asked for. Autostash is an explicit choice, and
                // a failed re-apply is then reported, never hidden.
                return Some(refuse(
                    "dirty-tree",
                    "Rebase needs a clean working tree. Commit or stash your changes first, turn on autostash, or pull with merge instead.",
                ));
            }
            None
        }

        Intent::Submodule => busy("update submodules"),

        Intent::Abort => {
            if f.operation.is_none() {
                Some(refuse("nothing-to-abort", "There's no operation in progress."))
            } else {
                None
            }
        }

        Intent::Sync => {
            if f.unborn {
                return Some(refuse("unborn-head", "There are no commits yet — nothing to update."));
            }
            if f.detached {
                return Some(refuse("detached-head", "You're not on a branch. Check one out first."));
            }
            if f.upstream.is_none() {
                return Some(refuse("no-upstream", "This branch isn't tracking a remote branch, so there is nothing to update from."));
            }
            busy("sync")
        }

        Intent::SyncContinue | Intent::SyncAbort => {
            if f.sync_in_progress {
                None
            } else {
                Some(refuse("no-sync", "No update is paused here."))
            }
        }

        Intent::Continue => {
            if f.operation.is_none() {
                return Some(refuse("nothing-to-continue", "There's no operation in progress."));
            }
            if f.continue_command.is_none() {
                return Some(refuse(
                    "cannot-continue",
                    format!("A {} can't be continued from here; finish it in a terminal or abort it.", f.operation.clone().unwrap_or_default()),
                ));
            }
            if f.conflicted > 0 {
                return Some(refuse(
                    "unmerged-paths",
                    format!(
                        "{} file{} still {} conflicts. Resolve and stage {} first.",
                        f.conflicted,
                        if f.conflicted == 1 { "" } else { "s" },
                        if f.conflicted == 1 { "has" } else { "have" },
                        if f.conflicted == 1 { "it" } else { "them" }
                    ),
                ));
            }
            None
        }
    }
}

/// NUL-delimited pathspecs for `--pathspec-from-file=-`, each marked
/// `:(literal)` so nothing in a filename is read as an option, a glob or
/// pathspec magic.
pub fn pathspec_stdin(paths: &[String]) -> Result<Vec<u8>, AppError> {
    if paths.is_empty() {
        return Err(AppError::Command(
            "No files selected — this only ever acts on files you pick.".into(),
        ));
    }
    let mut out = Vec::new();
    for p in paths {
        validate_repo_path(p)?;
        out.extend_from_slice(b":(literal)");
        out.extend_from_slice(p.as_bytes());
        out.push(0);
    }
    Ok(out)
}

/// The old names of the staged renames among `paths`: status reports a rename
/// as one entry under its new name, but the index holds it as two changes, and
/// an action on the entry has to act on both. A copy is left out: its source
/// was not changed by it, and may carry edits of its own nobody selected.
fn renamed_from(entries: &[git_status::ChangeEntry], paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in entries.iter().filter(|e| e.staged == "R" && paths.contains(&e.path)) {
        if let Some(o) = e.orig_path.as_ref().filter(|o| !out.contains(o)) {
            out.push(o.clone());
        }
    }
    out
}

/// A path must stay inside the worktree. Paths come from our own status output,
/// so this is a guard against a malformed request, not a user typo.
/// The absolute, canonical path of `path` inside `repo_path` — or an error
/// when it is not inside the repository (a `..`, an absolute path, a symlink
/// that leaves it) or does not exist. Every command that hands a path to the
/// operating system goes through this, so the JavaScript side never holds a
/// general "open any path" capability.
pub fn within_repo(repo_path: &str, path: &str) -> Result<PathBuf, AppError> {
    validate_repo_path(path)?;
    let base = std::fs::canonicalize(repo_path).map_err(|e| AppError::NotFound(format!("{}: {}", repo_path, e)))?;
    let full = std::fs::canonicalize(base.join(path))
        .map_err(|e| AppError::NotFound(format!("{} does not exist in this repository ({})", path, e)))?;
    if !full.starts_with(&base) {
        return Err(AppError::Command(format!("'{}' isn't a path inside this repository", path)));
    }
    Ok(full)
}

/// Would the system's "open" run this path rather than show it? An
/// executable bit, or a bundle / launcher type by extension. Such a path is
/// revealed in the file manager, never opened.
pub fn opens_by_running(path: &std::path::Path) -> bool {
    // What "open" runs everywhere: bundles, installers, shell scripts, launchers.
    const LAUNCHERS: &[&str] = &[
        "app", "command", "tool", "terminal", "pkg", "dmg", "scpt", "workflow", "action", "exe", "bat", "cmd", "ps1", "msi", "jar", "sh", "bash",
        "zsh", "fish", "ksh", "csh", "desktop", "run", "appimage",
    ];
    // What Windows runs on a double-click — the script host, the shell, the
    // registry editor, the Python launcher. There is no execute bit to ask.
    #[cfg(windows)]
    const PLATFORM: &[&str] = &[
        "js", "jse", "vbs", "vbe", "wsf", "wsh", "hta", "scr", "pif", "com", "lnk", "msc", "cpl", "reg", "inf", "scf", "py", "pyw", "psm1", "psd1",
        "msp", "mst", "application", "gadget", "iso", "vhd", "vhdx", "appref-ms", "settingcontent-ms", "library-ms",
    ];
    #[cfg(not(windows))]
    const PLATFORM: &[&str] = &[];
    let by_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            LAUNCHERS.contains(&e.as_str()) || PLATFORM.contains(&e.as_str())
        })
        .unwrap_or(false);
    if by_ext {
        return true;
    }
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if meta.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

/// A canonical path the way the operating system's "open" wants it. On
/// Windows `canonicalize` gives the verbatim form (`\\?\C:\…`), which the
/// shell does not take; the prefix is dropped for it.
pub fn shell_path(p: &std::path::Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{}", rest));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    p.to_path_buf()
}

/// Up to `max` bytes of a file, and whether there was more. The marker guard
/// and the conflict view never need a whole file to decide.
pub(crate) fn read_head(path: &std::path::Path, max: usize) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)?.take(max as u64 + 1).read_to_end(&mut buf)?;
    let more = buf.len() > max;
    buf.truncate(max);
    Ok((buf, more))
}

/// How much of a file the marker guard reads.
pub(crate) const MARKER_SCAN_MAX: usize = 16 * 1024 * 1024;

pub fn validate_repo_path(p: &str) -> Result<(), AppError> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.starts_with('\\')
        || p.starts_with("~")
        || p.split(['/', '\\']).any(|seg| seg == "..")
        || p.contains('\0')
        // A Windows drive-absolute path.
        || (p.len() > 1 && p.as_bytes()[1] == b':');
    if bad {
        return Err(AppError::Command(format!(
            "'{}' isn't a path inside this repository",
            p
        )));
    }
    Ok(())
}

/// The argv for a push. Pure, so a test can prove what it can never contain.
pub fn build_push_args(remote: &str, local_branch: &str, remote_ref: &str, set_upstream: bool) -> Vec<String> {
    // `--progress`: the counters keep coming while bytes move, so the push is
    // stopped only for silence (`stall`), never for the size of what it sends.
    let mut a: Vec<String> = vec!["push".into(), "--porcelain".into(), "--progress".into()];
    if set_upstream {
        a.push("--set-upstream".into());
    }
    // `--` ends option parsing; both sides of the refspec are fully qualified so
    // nothing depends on push.default, the source can never be empty (which
    // would be a deletion), and there is no leading '+' (which would be a force).
    a.push("--".into());
    a.push(remote.to_string());
    a.push(format!("refs/heads/{}:{}", local_branch, remote_ref));
    a
}

/// The argv that integrates fetched work. One command per mode, named by the
/// same word the button uses.
pub fn build_integrate_args(mode: PullMode, upstream_ref: &str, autostash: bool) -> Vec<String> {
    match mode {
        PullMode::FfOnly => vec![
            "merge".into(),
            "--ff-only".into(),
            "--no-edit".into(),
            "--no-stat".into(),
            upstream_ref.to_string(),
        ],
        PullMode::Merge => vec![
            "merge".into(),
            "--no-edit".into(),
            "--no-stat".into(),
            upstream_ref.to_string(),
        ],
        PullMode::Rebase => vec![
            "rebase".into(),
            if autostash { "--autostash".into() } else { "--no-autostash".into() },
            upstream_ref.to_string(),
        ],
    }
}

fn signature_label(code: &str) -> &'static str {
    match code {
        "G" => "good",
        "U" => "good, untrusted",
        "X" => "expired",
        "Y" => "expired key",
        "R" => "revoked key",
        "B" => "bad",
        "E" => "unverifiable",
        _ => "unsigned",
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct CommitInfo {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    pub signature: String,
    pub signed: bool,
    pub amended: bool,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct PushedRef {
    pub flag: String,
    pub summary: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct PushOutcome {
    pub refs: Vec<PushedRef>,
    pub up_to_date: bool,
    pub new_upstream: Option<String>,
    pub remote: String,
}

/// A submodule whose recorded commit this push advertises while no remote of
/// the submodule has it — what `git push --recurse-submodules=check` looks
/// for — or where that could not be checked. `state`: not-on-remote |
/// not-here (the commit is not in the submodule's checkout) | not-populated.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SubmodulePush {
    pub path: String,
    pub recorded: String,
    pub recorded_short: String,
    pub state: String,
    /// Its checked-out branch; `None` when detached, where a push from it
    /// needs a branch first.
    pub branch: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct PullOutcome {
    pub mode: String,
    pub head_before: String,
    pub head_after: String,
    pub fast_forward: bool,
    pub commits_pulled: usize,
    pub files_changed: usize,
    pub conflicts: Vec<String>,
    /// The literal command that puts the branch back where it was.
    pub recovery: Option<String>,
    /// The rebase finished but git could not re-apply the autostash cleanly:
    /// the conflicted files are in the tree and the stash entry is kept.
    pub autostash_conflict: bool,
    pub autostash_left: Option<String>,
    /// Submodule pointers the pull moved: the repository now records other
    /// commits for them than are checked out. Update aligns them.
    pub submodules_moved: usize,
}

/// What every operation returns: whether it worked, what to tell the user, and
/// the freshly re-read repository state.
#[derive(Debug, Serialize, Clone)]
pub struct OpResult {
    pub ok: bool,
    pub headline: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advice: Option<Advice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<RepoStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push: Option<PushOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull: Option<PullOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submodules: Option<SubmoduleReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lfs: Option<crate::lfs::LfsStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<crate::sync::SyncOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree: Option<crate::tree::TreeOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stash: Option<crate::stash::StashOutcome>,
    /// Boxed: a debug build gives every OpResult temporary in every
    /// operation's frames a stack slot of its own, and inline, this dozen
    /// strings pushed `changes_sync_run` past a 2 MiB thread stack (see
    /// every_heavy_command_future_stays_far_below_a_threads_stack).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Box<crate::states::StateOutcome>>,
    /// Submodules a push would leave a clone unable to check out (their
    /// recorded commit is on no remote of theirs), or could not check.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub unpushed_submodules: Vec<SubmodulePush>,
}

impl OpResult {
    pub(crate) fn blank() -> Self {
        OpResult {
            ok: true,
            headline: String::new(),
            detail: String::new(),
            advice: None,
            refusal: None,
            status: None,
            commit: None,
            push: None,
            pull: None,
            submodules: None,
            lfs: None,
            sync: None,
            tree: None,
            stash: None,
            state: None,
            unpushed_submodules: Vec::new(),
        }
    }

    /// A refusal decided before the repo could be read at all.
    pub(crate) fn refused_alone(r: Refusal) -> Self {
        OpResult {
            ok: false,
            headline: r.message.clone(),
            refusal: Some(r),
            ..OpResult::blank()
        }
    }

    pub(crate) fn refused(r: Refusal, status: RepoStatus) -> Self {
        OpResult {
            ok: false,
            headline: r.message.clone(),
            refusal: Some(r),
            status: Some(status),
            ..OpResult::blank()
        }
    }

    pub(crate) fn done(headline: impl Into<String>, detail: impl Into<String>, status: RepoStatus) -> Self {
        OpResult {
            ok: true,
            headline: headline.into(),
            detail: detail.into(),
            status: Some(status),
            ..OpResult::blank()
        }
    }

    pub(crate) fn failed(advice: Advice, status: RepoStatus) -> Self {
        OpResult {
            ok: false,
            headline: advice.headline.clone(),
            detail: advice.guidance.clone(),
            advice: Some(advice),
            status: Some(status),
            ..OpResult::blank()
        }
    }
}

pub(crate) async fn snapshot(repo_path: &str) -> Result<RepoStatus, AppError> {
    git_status::repo_status(repo_path).await
}

pub(crate) fn advice_ctx<'a>(status: &'a RepoStatus) -> AdviceCtx<'a> {
    AdviceCtx {
        key_path: None,
        url: status.push.remotes.first().map(|r| r.fetch_url.as_str()),
        branch: status.branch.as_deref(),
        remote: status.push.remotes.first().map(|r| r.name.as_str()),
        login: None,
    }
}

/// A push or fetch that failed, explained. When GitHub answered "Repository
/// not found" (its words both for a repository that doesn't exist and for
/// one the account can't see), with the key used and the account it signs in
/// as, asked only after such a failure and then cached: under that account's
/// own name it means the repository doesn't exist. And when the address may
/// be wrong while nothing was ever fetched from or pushed to that remote (so
/// nothing depends on it yet), "Change the address…".
pub(crate) async fn explain_transport_failure(repo_path: &str, op: GitOp, stderr: &str, remote: &str, after: &RepoStatus) -> Advice {
    let id = if stderr.contains("Repository not found") {
        Box::pin(crate::push_identity::identity_of_remote(repo_path, remote)).await
    } else {
        None
    };
    let mut ctx = advice_ctx(after);
    ctx.remote = Some(remote);
    if let Some(id) = &id {
        ctx.key_path = id.key_path.as_deref();
        ctx.login = id.login.as_deref();
        if id.url.is_some() {
            ctx.url = id.url.as_deref();
        }
    }
    let mut a = git_advice::explain(op, stderr, &ctx);
    if crate::sparse::address_may_be_wrong(stderr) && never_reached(repo_path, remote).await {
        a.action = Some("change-remote".to_string());
        a.remote = Some(remote.to_string());
    }
    a
}

/// Nothing was ever fetched from `remote` or pushed to it: it has no
/// remote-tracking branch. Nothing depends on such a remote yet.
pub(crate) async fn never_reached(repo_path: &str, remote: &str) -> bool {
    if crate::git_exec::validate_remote_name(remote).is_err() {
        return false;
    }
    GitCmd::at(repo_path)
        .args(["for-each-ref", "--count=1", "--format=%(refname)", &format!("refs/remotes/{}/", remote)])
        .timeout(LOCAL_TIMEOUT)
        .ok_text()
        .await
        .is_some_and(|s| s.is_empty())
}

// --- Staging -------------------------------------------------------------

/// Lines (1-based) that still carry a conflict marker: every `<<<<<<<` that
/// opens a hunk, finished or not, and a `>>>>>>>` with no hunk open — the
/// shapes a half-done edit leaves behind. A bare `=======` on its own is not
/// one (a Markdown heading underline looks exactly like it). `size` is the
/// width git wrote: 7 unless the `conflict-marker-size` attribute says so.
/// The same rule draws the conflict view, so the two never disagree.
pub fn conflict_marker_lines(text: &str, size: usize) -> Vec<usize> {
    let mut lines = Vec::new();
    let mut open = false;
    for (i, line) in text.lines().enumerate() {
        match crate::git_status::marker_kind(line, size) {
            Some('<') => {
                open = true;
                lines.push(i + 1);
            }
            Some('>') => {
                if !open {
                    lines.push(i + 1);
                }
                open = false;
            }
            _ => {}
        }
    }
    lines
}

/// The marker width git writes for each path: 7, or what the
/// `conflict-marker-size` attribute sets for it. One `check-attr` for all,
/// the paths on its standard input so their number does not matter.
pub(crate) async fn marker_sizes(repo: &std::path::Path, paths: &[String]) -> std::collections::BTreeMap<String, usize> {
    let mut sizes = std::collections::BTreeMap::new();
    if paths.is_empty() {
        return sizes;
    }
    let mut stdin: Vec<u8> = Vec::new();
    for p in paths {
        stdin.extend_from_slice(p.as_bytes());
        stdin.push(0);
    }
    let Ok(out) = GitCmd::at(repo).args(["check-attr", "--stdin", "-z", "conflict-marker-size"]).stdin_bytes(stdin).run().await else { return sizes };
    if !out.ok() {
        return sizes;
    }
    let fields: Vec<String> = out.stdout.split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).to_string()).collect();
    for rec in fields.chunks(3) {
        if let [path, _attr, value] = rec {
            if let Ok(n) = value.trim().parse::<usize>() {
                if (3..=200).contains(&n) {
                    sizes.insert(path.clone(), n);
                }
            }
        }
    }
    sizes
}

pub(crate) fn marker_size_for(sizes: &std::collections::BTreeMap<String, usize>, path: &str) -> usize {
    sizes.get(path).copied().unwrap_or(7)
}

/// The refusal for a file that still carries conflict markers.
pub(crate) fn markers_refusal(path: &str, lines: &[usize]) -> Refusal {
    let shown: Vec<String> = lines.iter().take(12).map(|n| n.to_string()).collect();
    let more = lines.len().saturating_sub(shown.len());
    refuse(
        "conflict-markers",
        format!(
            "{} still contains conflict markers (line{} {}{}). Finish the edit, or Keep mine / Take theirs.",
            path,
            if lines.len() == 1 { "" } else { "s" },
            shown.join(", "),
            if more > 0 { format!(" and {} more", more) } else { String::new() }
        ),
    )
}

/// What one read of staged blobs may hold, and what the whole check may read.
const MARKER_BATCH_BYTES: usize = 8 * 1024 * 1024;
const MARKER_SCAN_TOTAL: usize = 256 * 1024 * 1024;

/// `git diff --cached --raw -z` records: the path and blob of every regular
/// file the index adds or changes. Gitlinks and symlinks have no lines.
fn parse_raw_blobs(raw: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut it = raw.split(|b| *b == 0).filter(|t| !t.is_empty());
    while let Some(meta) = it.next() {
        if meta.first() != Some(&b':') {
            continue;
        }
        let Some(path) = it.next() else { break };
        let meta = String::from_utf8_lossy(meta);
        let f: Vec<&str> = meta.split(' ').collect();
        // ":<mode before> <mode after> <blob before> <blob after> <status>"
        if f.len() >= 5 && f[1].starts_with("100") {
            out.push((String::from_utf8_lossy(path).to_string(), f[3].to_string()));
        }
    }
    out
}

/// `git cat-file --batch` output for the blobs asked for, in order: a header
/// "<oid> blob <size>", the bytes, a newline — or "<oid> missing".
fn first_marker_offence(batch_out: &[u8], batch: &[(String, String)], sizes: &std::collections::BTreeMap<String, usize>) -> Option<(String, Vec<usize>)> {
    let mut rest = batch_out;
    for (path, _) in batch {
        let nl = rest.iter().position(|b| *b == b'\n')?;
        let header = String::from_utf8_lossy(&rest[..nl]).to_string();
        rest = &rest[nl + 1..];
        let mut f = header.split(' ');
        let (_, kind, size) = (f.next(), f.next(), f.next());
        if kind != Some("blob") {
            continue; // missing, or not a file's content
        }
        let size: usize = size?.parse().ok()?;
        if rest.len() < size {
            return None;
        }
        let content = &rest[..size];
        rest = &rest[(size + 1).min(rest.len())..];
        if content.iter().take(8000).any(|b| *b == 0) {
            continue;
        }
        let lines = conflict_marker_lines(&String::from_utf8_lossy(content), marker_size_for(sizes, path));
        if !lines.is_empty() {
            return Some((path.clone(), lines));
        }
    }
    None
}

/// The first staged text file that still carries conflict markers, read from
/// the index — what `rebase --continue` or a commit would record. The diff
/// names the staged blobs itself (no rename pairing, so a file moved while it
/// was edited is still seen); they are read a batch at a time through
/// `cat-file`, and judged by the same rule as "Mark resolved" and the
/// conflict view. Nothing here depends on the person's colour, grep or diff
/// settings, or on what an attribute calls binary.
pub(crate) async fn staged_marker_offence(repo: &std::path::Path) -> Option<(String, Vec<usize>)> {
    let out = GitCmd::at(repo)
        .args(["diff", "--cached", "--raw", "-z", "--no-abbrev", "--no-renames", "--no-color", "--diff-filter=AMT"])
        .run()
        .await
        .ok()?;
    if !out.ok() {
        return None;
    }
    let staged = parse_raw_blobs(&out.stdout);
    if staged.is_empty() {
        return None;
    }
    let paths: Vec<String> = staged.iter().map(|(p, _)| p.clone()).collect();
    let sizes = marker_sizes(repo, &paths).await;
    // How large each blob is, before any of them is read.
    let ids = |list: &[(String, String)]| -> Vec<u8> { list.iter().flat_map(|(_, oid)| format!("{}\n", oid).into_bytes()).collect() };
    let check = GitCmd::at(repo).args(["cat-file", "--batch-check"]).stdin_bytes(ids(&staged)).run().await.ok()?;
    if !check.ok() {
        return None;
    }
    let blob_sizes: Vec<Option<usize>> = String::from_utf8_lossy(&check.stdout)
        .lines()
        .map(|l| {
            let mut f = l.split(' ');
            match (f.next(), f.next(), f.next()) {
                (_, Some("blob"), Some(n)) => n.parse().ok(),
                _ => None,
            }
        })
        .collect();
    let mut budget = MARKER_SCAN_TOTAL;
    let mut batch: Vec<(String, String)> = Vec::new();
    let mut batch_bytes = 0usize;
    for (i, entry) in staged.iter().enumerate() {
        let Some(size) = blob_sizes.get(i).copied().flatten() else { continue };
        if size == 0 || size > MARKER_SCAN_MAX {
            continue;
        }
        if size > budget {
            break;
        }
        budget -= size;
        batch_bytes += size;
        batch.push(entry.clone());
        if batch_bytes >= MARKER_BATCH_BYTES {
            let read = GitCmd::at(repo).args(["cat-file", "--batch"]).stdin_bytes(ids(&batch)).private_output().run().await.ok()?;
            if let Some(hit) = first_marker_offence(&read.stdout, &batch, &sizes) {
                return Some(hit);
            }
            batch.clear();
            batch_bytes = 0;
        }
    }
    if batch.is_empty() {
        return None;
    }
    let read = GitCmd::at(repo).args(["cat-file", "--batch"]).stdin_bytes(ids(&batch)).private_output().run().await.ok()?;
    first_marker_offence(&read.stdout, &batch, &sizes)
}

pub async fn stage(repo_path: &str, paths: Vec<String>, allow_markers: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Stage, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let conflicted_before: Vec<String> = before
        .entries
        .iter()
        .filter(|e| e.kind == "conflicted" && paths.contains(&e.path))
        .map(|e| e.path.clone())
        .collect();
    // "Mark resolved" on a file whose markers are still in it would stage the
    // markers as the resolution. Refuse by name; the page offers "anyway".
    if !allow_markers && !conflicted_before.is_empty() {
        let sizes = marker_sizes(std::path::Path::new(repo_path), &conflicted_before).await;
        for p in &conflicted_before {
            let Ok((bytes, _)) = read_head(&std::path::Path::new(repo_path).join(p), MARKER_SCAN_MAX) else { continue };
            if bytes.iter().take(8000).any(|b| *b == 0) {
                continue;
            }
            let lines = conflict_marker_lines(&String::from_utf8_lossy(&bytes), marker_size_for(&sizes, p));
            if !lines.is_empty() {
                return Ok(OpResult::refused(markers_refusal(p, &lines), before));
            }
        }
    }

    let stdin = pathspec_stdin(&paths)?;
    let out = GitCmd::at(repo_path)
        .args(["add", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin_bytes(stdin)
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(GitOp::Commit, &out.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }

    let staged_now = paths
        .iter()
        .filter(|p| {
            after
                .entries
                .iter()
                .any(|e| &&e.path == p && e.is_staged())
        })
        .count();
    let mut detail = format!("{} of {} selected file(s) are now staged.", staged_now, paths.len());
    if !conflicted_before.is_empty() {
        detail.push_str(&format!(
            " {} conflict(s) marked resolved.",
            conflicted_before.len()
        ));
    }
    Ok(OpResult::done("Staged.", detail, after))
}

/// Explicitly separate from `stage`, so "stage everything" can never happen by
/// an empty path list falling through.
pub async fn stage_all(repo_path: &str) -> Result<OpResult, AppError> {
    let out = GitCmd::at(repo_path)
        .args(["add", "--all", "--"])
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;
    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(GitOp::Commit, &out.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }
    Ok(OpResult::done(
        "Staged everything.",
        format!("{} file(s) staged.", after.staged_count),
        after,
    ))
}

pub async fn unstage(repo_path: &str, paths: Vec<String>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let conflicted_selected = before
        .entries
        .iter()
        .any(|e| e.kind == "conflicted" && paths.contains(&e.path));
    if let Some(r) = check(
        &Intent::Unstage {
            conflicted_selected,
        },
        &StatusFacts::from(&before),
    ) {
        return Ok(OpResult::refused(r, before));
    }

    // A staged rename is one entry under its new name, but two index entries:
    // unstaging only the new name would leave the old one's deletion staged.
    let mut pathspec = paths.clone();
    pathspec.extend(renamed_from(&before.entries, &paths));
    let stdin = pathspec_stdin(&pathspec)?;
    // With no commits yet there is no HEAD to reset against.
    let out = if before.unborn {
        GitCmd::at(repo_path)
            .args([
                "rm",
                "--cached",
                "--quiet",
                "-r",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ])
            .stdin_bytes(stdin)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?
    } else {
        GitCmd::at(repo_path)
            .args([
                "reset",
                "--quiet",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
                "HEAD",
            ])
            .stdin_bytes(stdin)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?
    };

    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(GitOp::Commit, &out.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }
    let still_staged = paths
        .iter()
        .filter(|p| after.entries.iter().any(|e| &&e.path == p && e.is_staged()))
        .count();
    Ok(OpResult::done(
        "Unstaged.",
        format!(
            "{} file(s) moved out of the staging area.",
            paths.len() - still_staged
        ),
        after,
    ))
}

// --- Discard (destructive) ----------------------------------------------

/// Throw away changes to the named files. Tracked files go back to HEAD;
/// untracked files are deleted. Never pathless, never `-x` (which would take
/// .env and node_modules with it).
pub async fn discard(repo_path: &str, paths: Vec<String>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let selected: Vec<&git_status::ChangeEntry> = before
        .entries
        .iter()
        .filter(|e| paths.contains(&e.path))
        .collect();

    let intent = Intent::Discard {
        has_paths: !paths.is_empty(),
        submodule_selected: selected.iter().any(|e| e.is_submodule),
        conflicted_selected: selected.iter().any(|e| e.kind == "conflicted"),
    };
    if let Some(r) = check(&intent, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }

    // A conflicted entry with no operation in progress (a stash that failed
    // to re-apply) is discarded like any tracked file: back to HEAD, and the
    // change is still in the stash. `check` refused it when an operation is
    // in progress, so reaching here means it is safe.
    let tracked: Vec<String> = selected
        .iter()
        .filter(|e| e.kind == "tracked" || e.kind == "conflicted")
        .map(|e| e.path.clone())
        .collect();
    let untracked: Vec<String> = selected
        .iter()
        .filter(|e| e.kind == "untracked")
        .map(|e| e.path.clone())
        .collect();
    // A staged rename is one entry under its new name. Restoring only that
    // name deletes it (HEAD has no such file) and leaves the old name deleted
    // too: the file would be gone under both. Its old name comes back from
    // HEAD as well — in the index only when something else now sits at that
    // path on disk, which a restore of the work tree would overwrite.
    let (old_names, old_names_in_use): (Vec<String>, Vec<String>) = renamed_from(&before.entries, &paths)
        .into_iter()
        .partition(|p| std::fs::symlink_metadata(std::path::Path::new(repo_path).join(p)).is_err());
    // A rename that changes only the case, on a disk that ignores case: the
    // old name "exists" because it is the renamed file itself. Restoring the
    // new name deletes that one physical file, so the old name has to come
    // back after it, in a call of its own (in the same call git writes the
    // old name first and then deletes it as the new one: measured on NTFS).
    let ignore_case = !old_names_in_use.is_empty()
        && GitCmd::at(repo_path).args(["config", "--bool", "--get", "core.ignorecase"]).ok_text().await.is_some_and(|s| s.trim() == "true");
    let (case_only, old_names_in_use): (Vec<String>, Vec<String>) = old_names_in_use.into_iter().partition(|o| {
        ignore_case
            && before.entries.iter().any(|e| {
                e.staged == "R" && e.orig_path.as_deref() == Some(o.as_str()) && paths.contains(&e.path) && e.path != *o && e.path.to_lowercase() == o.to_lowercase()
            })
    });
    // A staged-added file has no HEAD version, nor has the new name of a
    // rename or a copy: restoring one deletes it. The UI needs to word that
    // differently, so it is counted separately.
    let will_be_deleted: Vec<String> = selected
        .iter()
        .filter(|e| e.kind == "untracked" || matches!(e.staged.as_str(), "A" | "R" | "C"))
        .map(|e| e.path.clone())
        .collect();

    if tracked.is_empty() && untracked.is_empty() {
        return Ok(OpResult::refused(
            refuse(
                "no-paths",
                "Those files have no changes to discard — the list may be out of date. Refresh and try again.",
            ),
            before,
        ));
    }

    let mut errors: Vec<String> = Vec::new();

    if !tracked.is_empty() {
        let mut both = tracked.clone();
        both.extend(old_names.iter().cloned());
        let out = GitCmd::at(repo_path)
            .args([
                "restore",
                "--staged",
                "--worktree",
                "--source=HEAD",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ])
            .stdin_bytes(pathspec_stdin(&both)?)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !out.ok() {
            errors.push(out.stderr.clone());
        }
    }
    if !old_names_in_use.is_empty() {
        let out = GitCmd::at(repo_path)
            .args(["restore", "--staged", "--source=HEAD", "--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin_bytes(pathspec_stdin(&old_names_in_use)?)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !out.ok() {
            errors.push(out.stderr.clone());
        }
    }
    if !case_only.is_empty() {
        let out = GitCmd::at(repo_path)
            .args(["restore", "--staged", "--worktree", "--source=HEAD", "--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin_bytes(pathspec_stdin(&case_only)?)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !out.ok() {
            errors.push(out.stderr.clone());
        }
    }

    if !untracked.is_empty() {
        // `git clean` has no --pathspec-from-file, so these go in argv — still
        // `:(literal)`-marked, and still never with -x.
        let mut cmd = GitCmd::at(repo_path).args(["clean", "--force", "-d", "--"]);
        for p in &untracked {
            validate_repo_path(p)?;
            cmd = cmd.arg(format!(":(literal){}", p));
        }
        let out = cmd.timeout(LOCAL_TIMEOUT).run().await?;
        if !out.ok() {
            errors.push(out.stderr.clone());
        }
    }

    let after = snapshot(repo_path).await?;
    // clean exits 0 even when it matched nothing, so verify by re-reading.
    let not_discarded: Vec<String> = paths
        .iter()
        .filter(|p| after.entries.iter().any(|e| &&e.path == p))
        .cloned()
        .collect();

    if !errors.is_empty() {
        let a = git_advice::explain(GitOp::Checkout, &errors.join("\n"), &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }

    let mut detail = format!(
        "{} file(s) discarded{}.",
        paths.len() - not_discarded.len(),
        if will_be_deleted.is_empty() {
            String::new()
        } else {
            format!(", {} of them deleted", will_be_deleted.len())
        }
    );
    if !not_discarded.is_empty() {
        detail.push_str(&format!(
            " {} still show(s) changes: {}.",
            not_discarded.len(),
            not_discarded.join(", ")
        ));
    }
    Ok(OpResult::done("Discarded.", detail, after))
}

// --- Commit --------------------------------------------------------------

pub async fn commit(repo_path: &str, message: &str, amend: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let trimmed = message.trim();
    let intent = Intent::Commit {
        amend,
        message_empty: trimmed.is_empty(),
    };
    if let Some(r) = check(&intent, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }

    let mut cmd = GitCmd::at(repo_path).args(["commit", "--cleanup=whitespace"]);
    if amend {
        cmd = cmd.arg("--amend");
    }
    // The message goes on stdin: it can start with '-', span any length, and
    // never becomes an argument. No --no-verify: the identity guard must run.
    // No -S/--no-gpg-sign: signing is the profile's decision, already in config.
    let out = cmd
        .arg("--file=-")
        .stdin_bytes(trimmed.as_bytes().to_vec())
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let stderr = if out.stderr.is_empty() {
            out.text()
        } else {
            out.stderr.clone()
        };
        let a = git_advice::explain(GitOp::Commit, &stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }

    // Report the identity and signature that actually landed.
    let line = GitCmd::at(repo_path)
        .args(["log", "-1", "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%G?%x1f%s"])
        .ok_text()
        .await
        .unwrap_or_default();
    let f: Vec<&str> = line.split('\x1f').collect();
    let shortstat = GitCmd::at(repo_path)
        .args(["show", "--shortstat", "--format=", "HEAD"])
        .ok_text()
        .await
        .unwrap_or_default();
    let (files_changed, insertions, deletions) = parse_shortstat(&shortstat);
    let sig_code = f.get(4).copied().unwrap_or("N");

    let info = CommitInfo {
        hash: f.first().copied().unwrap_or_default().to_string(),
        short: f.get(1).copied().unwrap_or_default().to_string(),
        author_name: f.get(2).copied().unwrap_or_default().to_string(),
        author_email: f.get(3).copied().unwrap_or_default().to_string(),
        signature: signature_label(sig_code).to_string(),
        signed: sig_code != "N",
        subject: f.get(5).copied().unwrap_or_default().to_string(),
        amended: amend,
        files_changed,
        insertions,
        deletions,
    };

    let headline = if amend {
        format!("Amended {}.", info.short)
    } else {
        format!("Committed {}.", info.short)
    };
    let detail = format!(
        "as {} <{}> · {} file(s), +{} −{}{}",
        info.author_name,
        info.author_email,
        info.files_changed,
        info.insertions,
        info.deletions,
        if info.signed {
            format!(" · signature {}", info.signature)
        } else {
            String::new()
        }
    );

    Ok(OpResult {
        commit: Some(info),
        ..OpResult::done(headline, detail, after)
    })
}

pub fn parse_shortstat(s: &str) -> (usize, usize, usize) {
    let num = |needle: &str| -> usize {
        s.split(',')
            .find(|p| p.contains(needle))
            .and_then(|p| p.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    (num("file"), num("insertion"), num("deletion"))
}

// --- Push ----------------------------------------------------------------

/// What `push()` knows before choosing where the branch goes.
#[derive(Debug, Default, Clone)]
pub struct PushInputs {
    pub branch: String,
    /// The upstream as `git status` shows it (`origin/main`, or `dev` for a
    /// branch that tracks a local one).
    pub upstream: Option<String>,
    pub branch_remote: Option<String>,
    pub merge_ref: Option<String>,
    pub push_remote: Option<String>,
    pub push_default: Option<String>,
    /// Every remote's name.
    pub remotes: Vec<String>,
    /// The caller asked for `--set-upstream`: the "Publish" button.
    pub set_upstream: bool,
}

#[derive(Debug, PartialEq)]
pub struct PushTarget {
    pub remote: String,
    pub remote_ref: String,
    pub set_upstream: bool,
}

/// Why `url` is not an address to publish to, or None when it is: https,
/// ssh (`ssh://…` or the scp form `git@github.com:owner/repo.git`), or the
/// full path of a folder. Nothing git could read as an option, no transport
/// helper (`ext::…`, any `<x>::`), no plain http, and no user name, password
/// or token written into an https address.
pub(crate) fn remote_url_problem(url: &str) -> Option<String> {
    let u = url.trim();
    if u.is_empty() {
        return Some("Paste the repository's address.".into());
    }
    if u.len() > 4096 || u.chars().any(char::is_control) {
        return Some("The address has an invisible character in it. Copy it again from GitHub (the green Code button).".into());
    }
    if u.starts_with('-') {
        return Some("That isn't a repository address.".into());
    }
    if u.contains("::") {
        return Some("GitSwitch doesn't publish through git transport helpers (an address with \"::\"). Use the HTTPS or SSH address.".into());
    }
    // The full path of a folder: C:\…, C:/…, \\server\share\…, /…
    let b = u.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    if drive || u.starts_with("\\\\") || u.starts_with('/') {
        return None;
    }
    if u.chars().any(char::is_whitespace) {
        return Some("The address has a space in it. Copy it again from GitHub (the green Code button).".into());
    }
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("http://") {
        return Some("Use the https:// address: plain http sends everything unencrypted.".into());
    }
    for scheme in ["https://", "ssh://"] {
        if let Some(rest) = lower.strip_prefix(scheme) {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            if scheme == "https://" && authority.contains('@') {
                return Some("Leave the user name, password or token out of the address: GitSwitch signs in with this folder's profile key, or with the sign-in git already has.".into());
            }
            let host = authority.rsplit('@').next().unwrap_or("");
            if host.is_empty() || host.starts_with('-') || path.trim_matches('/').is_empty() || path.starts_with('-') {
                return Some(format!(
                    "The address needs a server and a repository, like {}github.com/you/repo.git.",
                    if scheme == "ssh://" { "ssh://git@" } else { "https://" }
                ));
            }
            return None;
        }
    }
    if let Some(path) = lower.strip_prefix("file://") {
        return (path.trim_matches('/').is_empty()).then(|| "The address needs the path of a folder after file://.".to_string());
    }
    // The scp form: [user@]host:path. One letter before the colon is a drive
    // to git on Windows, not a host.
    if let Some((left, path)) = u.split_once(':') {
        let host = left.rsplit('@').next().unwrap_or("");
        if host.len() > 1 && !host.starts_with('-') && !left.contains('/') && !left.contains('\\') && !path.is_empty() && !path.starts_with('-') {
            return None;
        }
    }
    Some("That doesn't look like a repository address. Use the HTTPS or SSH address from GitHub's green Code button, like git@github.com:you/repo.git.".into())
}

/// What Publish's address check found, for "Add and publish" and "Change
/// and publish".
#[derive(Debug, Serialize, Clone, Default, PartialEq)]
pub struct RemoteCheck {
    /// The address is saved: origin added, or the remote's address changed.
    pub saved: bool,
    /// Why it wasn't, in words. Empty when it was.
    pub problem: String,
    /// GitHub's new-repository form, filled in, when the repository doesn't
    /// exist yet under the account the folder's key signs in as.
    pub create_url: Option<String>,
    /// "owner/name" of that missing repository: what the form that creates
    /// it starts from.
    pub repo: Option<String>,
    /// The branches a repository that isn't empty already has, when this
    /// branch isn't one of them: saved only once the person says to publish
    /// there anyway.
    pub existing_branches: Vec<String>,
}

/// How long the address check may take: a dialog is waiting for it.
const ADDRESS_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// The names of this repository's remotes.
async fn remote_names(repo_path: &str) -> Result<Vec<String>, AppError> {
    let listed = GitCmd::at(repo_path).args(["remote"]).timeout(LOCAL_TIMEOUT).run().await?;
    if !listed.ok() {
        return Err(AppError::Command(format!("GitSwitch couldn't read this repository's remotes: {}", listed.stderr.trim())));
    }
    Ok(listed.text().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

/// Before an address is saved: is there a repository at it that this
/// folder's key or sign-in can reach, and is it empty? `git ls-remote` from
/// the folder, so the profile's key is the one asked; it writes nothing.
/// None: go ahead.
async fn check_address(repo_path: &str, url: &str, allow_existing: bool) -> Result<Option<RemoteCheck>, AppError> {
    let ls = GitCmd::at(repo_path).args(["ls-remote", "--heads", "--", url]).timeout(ADDRESS_CHECK_TIMEOUT).run().await?;
    if ls.ok() {
        let branches: Vec<String> = ls
            .text()
            .lines()
            .filter_map(|l| l.split('\t').nth(1))
            .filter_map(|r| r.trim().strip_prefix("refs/heads/"))
            .map(String::from)
            .collect();
        if branches.is_empty() || allow_existing {
            return Ok(None);
        }
        let mine = GitCmd::at(repo_path).args(["symbolic-ref", "-q", "--short", "HEAD"]).timeout(LOCAL_TIMEOUT).ok_text().await.unwrap_or_default();
        return Ok(Some(not_empty(&branches, &mine)));
    }
    let said = ls.stderr.trim().to_string();
    // "Repository not found": the key, and the account it signs in as, tell
    // a repository that doesn't exist from one out of this account's reach.
    let id = if said.contains("Repository not found") {
        Box::pin(crate::push_identity::identity_for_url(repo_path, None, url)).await.ok()
    } else {
        None
    };
    let login = id.as_ref().and_then(|i| i.login.as_deref());
    let key = id.as_ref().and_then(|i| i.key_path.as_deref());
    if crate::sparse::missing_own_repository(&said, url, login) {
        let full = crate::repo_scan::parse_github_owner_name(url).map(|(o, n)| format!("{}/{}", o, n)).unwrap_or_default();
        let the_key = key.map(|k| format!("the key {}", crate::paths::base_name(k))).unwrap_or_else(|| "this folder's key".to_string());
        return Ok(Some(RemoteCheck {
            problem: format!(
                "There's no repository {} on GitHub yet: {} signs in as {}, who would see it even if it were private. Create it there (empty: no README, licence or .gitignore), then publish again.",
                full,
                the_key,
                login.unwrap_or_default()
            ),
            create_url: crate::sparse::github_create_url(url),
            repo: Some(full),
            ..Default::default()
        }));
    }
    let explained = crate::sparse::explain_transport_error_as(&said, key, url, false, login);
    let hint = explained.split("\n\ngit said:").next().unwrap_or(&explained).trim().to_string();
    let problem = if explained.trim() == said || hint.is_empty() {
        format!("GitSwitch couldn't reach a repository at that address. git said: {}", said)
    } else {
        hint
    };
    Ok(Some(RemoteCheck { problem, ..Default::default() }))
}

/// The words for a repository that already has branches. With this branch
/// among them a publish would be refused, so nothing is offered.
fn not_empty(branches: &[String], mine: &str) -> RemoteCheck {
    if !mine.is_empty() && branches.iter().any(|b| b == mine) {
        let others = match branches.len() - 1 {
            0 => String::new(),
            1 => " (and one more)".to_string(),
            n => format!(" (and {} more)", n),
        };
        return RemoteCheck {
            problem: format!(
                "That repository isn't empty: it already has a branch {}{} with commits of its own, so publishing {} there would be refused. Use an empty repository instead (no README, licence or .gitignore).",
                mine, others, mine
            ),
            ..Default::default()
        };
    }
    let shown = if branches.len() <= 4 { branches.join(", ") } else { format!("{}, and {} more", branches[..3].join(", "), branches.len() - 3) };
    let has = if branches.len() == 1 { format!("a branch ({})", shown) } else { format!("{} branches ({})", branches.len(), shown) };
    let this = if mine.is_empty() { "this branch" } else { mine };
    RemoteCheck {
        problem: format!(
            "That repository isn't empty: it already has {}. Publishing puts {} there beside {}, with a history of its own. Use an empty repository instead (no README, licence or .gitignore), or publish there anyway.",
            has,
            this,
            if branches.len() == 1 { "it" } else { "them" }
        ),
        existing_branches: branches.to_vec(),
        ..Default::default()
    }
}

/// Connect a repository that has no remote yet to `url`, as `origin`: what
/// Publish needs, without the terminal. Only for a repository with no remote
/// at all; one that has some is never changed here. The address is checked
/// first, and nothing is saved unless a repository is there that this
/// folder's key or sign-in can reach (and, when it has branches already,
/// only on the person's word: `allow_existing`).
pub async fn add_origin(repo_path: &str, url: &str, allow_existing: bool) -> Result<RemoteCheck, AppError> {
    if let Some(problem) = remote_url_problem(url) {
        return Err(AppError::Command(problem));
    }
    let url = url.trim();
    add_origin_guard(repo_path).await?;
    if let Some(refused) = Box::pin(check_address(repo_path, url, allow_existing)).await? {
        return Ok(refused);
    }
    save_origin(repo_path, url).await?;
    Ok(RemoteCheck { saved: true, ..Default::default() })
}

/// What `add_origin` needs first: a repository with no remote at all.
pub(crate) async fn add_origin_guard(repo_path: &str) -> Result<(), AppError> {
    let remotes = remote_names(repo_path).await?;
    if let Some(first) = remotes.first() {
        return Err(AppError::Command(format!(
            "This repository already has a remote ({}), so Publish uses it. If its address is wrong, change it in the terminal: `git remote set-url {} <url>`.",
            remotes.join(", "),
            first
        )));
    }
    Ok(())
}

/// Save `url` as origin; the caller has checked the address and the guard.
pub(crate) async fn save_origin(repo_path: &str, url: &str) -> Result<(), AppError> {
    let out = GitCmd::at(repo_path).args(["remote", "add", "--", "origin", url]).timeout(LOCAL_TIMEOUT).run().await?;
    if !out.ok() {
        return Err(AppError::Command(format!("git remote add failed: {}", out.stderr.trim())));
    }
    Ok(())
}

/// A new address for `remote`: Publish's "Change the address…", after a push
/// found nothing at the old one. Only while the remote still has the address
/// `from` that the person saw, has no separate push address, and was never
/// fetched from or pushed to: a remote that something depends on is never
/// changed here. The new address is checked first, as `add_origin` does.
pub async fn change_remote_address(repo_path: &str, remote: &str, from: &str, url: &str, allow_existing: bool) -> Result<RemoteCheck, AppError> {
    if let Some(problem) = remote_url_problem(url) {
        return Err(AppError::Command(problem));
    }
    let url = url.trim();
    change_remote_guard(repo_path, remote, from).await?;
    if let Some(refused) = Box::pin(check_address(repo_path, url, allow_existing)).await? {
        return Ok(refused);
    }
    save_remote_url(repo_path, remote, url).await?;
    Ok(RemoteCheck { saved: true, ..Default::default() })
}

/// What `change_remote_address` needs first: `remote` still has the address
/// `from` that the person saw, no separate push address, and nothing was ever
/// fetched from or pushed to it. Returns its current address.
pub(crate) async fn change_remote_guard(repo_path: &str, remote: &str, from: &str) -> Result<String, AppError> {
    let current = remote_as_seen(repo_path, remote, from).await?;
    if !never_reached(repo_path, remote).await {
        return Err(AppError::Command(format!(
            "{} has been fetched from or pushed to, so GitSwitch leaves its address alone. To change it anyway, in the terminal: `git remote set-url {} <url>`.",
            remote, remote
        )));
    }
    Ok(current)
}

/// What making a repository again needs first (Publish, after GitHub lost
/// the one this folder pushed to): `remote` still has the address `from`
/// that the person saw, no separate push address, and that address names
/// exactly `owner/name` on GitHub — as configured, or as git uses it once an
/// insteadOf rule has rewritten it. The repository made again sits where the
/// address already points, so the address never changes, and the remote may
/// have been fetched from and pushed to: nothing that depends on it moves.
/// Returns the address as configured (as git uses it when none is set).
pub(crate) async fn again_guard(repo_path: &str, remote: &str, from: &str, owner: &str, name: &str) -> Result<String, AppError> {
    let current = remote_as_seen(repo_path, remote, from).await?;
    let configured = GitCmd::at(repo_path)
        .args(["config", "--get", &format!("remote.{}.url", remote)])
        .timeout(LOCAL_TIMEOUT)
        .ok_text()
        .await
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| current.clone());
    let mut named = None;
    for url in [configured.as_str(), current.as_str()] {
        if let Some((o, n)) = crate::peek::github_slug_resolved(url).await {
            if o.eq_ignore_ascii_case(owner) && n.eq_ignore_ascii_case(name) {
                return Ok(configured.clone());
            }
            named.get_or_insert((o, n));
        }
    }
    match named {
        Some((o, n)) => Err(AppError::Command(format!(
            "This folder's address names {}/{}, not {}/{}: GitSwitch makes a repository again only under the name its address already has.",
            o, n, owner, name
        ))),
        None => Err(AppError::Command(format!(
            "The address of {} isn't a GitHub repository's, so GitSwitch can't make it again on GitHub.",
            remote
        ))),
    }
}

/// `remote` still has the address `from` that the person saw (tokens
/// masked as the status shows them) and no separate push address. Returns
/// its current address.
async fn remote_as_seen(repo_path: &str, remote: &str, from: &str) -> Result<String, AppError> {
    crate::git_exec::validate_remote_name(remote)?;
    if !remote_names(repo_path).await?.iter().any(|r| r == remote) {
        return Err(AppError::Command(format!("This repository has no remote named {} any more.", remote)));
    }
    let current = GitCmd::at(repo_path).args(["remote", "get-url", remote]).timeout(LOCAL_TIMEOUT).ok_text().await.unwrap_or_default();
    let current_push = GitCmd::at(repo_path).args(["remote", "get-url", "--push", remote]).timeout(LOCAL_TIMEOUT).ok_text().await.unwrap_or_default();
    // What the person saw is the address as the status shows it: tokens masked.
    let from = from.trim();
    if current != from && crate::repo_scan::mask_token(&current) != from {
        return Err(AppError::Command(format!(
            "The address of {} has changed meanwhile (it is now {}). Close this and try again.",
            remote,
            crate::repo_scan::mask_token(&current)
        )));
    }
    if current_push != current {
        return Err(AppError::Command(format!(
            "{} has a separate address for pushes, so GitSwitch leaves it alone. Change it in the terminal: `git remote set-url --push {} <url>`.",
            remote, remote
        )));
    }
    Ok(current)
}

/// Save `url` as `remote`'s address; the caller has checked both.
pub(crate) async fn save_remote_url(repo_path: &str, remote: &str, url: &str) -> Result<(), AppError> {
    let out = GitCmd::at(repo_path).args(["remote", "set-url", "--", remote, url]).timeout(LOCAL_TIMEOUT).run().await?;
    if !out.ok() {
        return Err(AppError::Command(format!("git remote set-url failed: {}", out.stderr.trim())));
    }
    Ok(())
}

/// `current`'s form (user, host alias, port) pointed at `owner/name`: a new
/// address for a remote keeps how it reached GitHub (`org-123@github.com:`,
/// `git@github.com-work:`, `ssh://git@ssh.github.com:443/`); an https one
/// loses any user name or token. None for anything but an SSH or https form.
pub(crate) fn retarget_url(current: &str, owner: &str, name: &str) -> Option<String> {
    let c = current.trim();
    if let Some(rest) = c.strip_prefix("ssh://") {
        let (authority, _) = rest.split_once('/')?;
        return Some(format!("ssh://{}/{}/{}.git", authority, owner, name));
    }
    if let Some(rest) = c.strip_prefix("https://") {
        let (authority, _) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        return Some(format!("https://{}/{}/{}.git", host, owner, name));
    }
    // The scp form, [user@]host:path. One letter before the colon is a drive.
    let (left, _) = c.split_once(':')?;
    (left.len() > 1 && !left.contains('/') && !left.contains('\\')).then(|| format!("{}:{}/{}.git", left, owner, name))
}

/// One look, as the folder itself reaches it (its own identity): does git
/// reach `url` now?
pub(crate) async fn reachable_now(repo_path: &str, url: &str) -> bool {
    matches!(GitCmd::at(repo_path).args(["ls-remote", "--heads", "--", url]).timeout(ADDRESS_CHECK_TIMEOUT).run().await, Ok(ls) if ls.ok())
}

/// A repository just created can take a moment to answer over git: up to
/// three looks, two seconds apart, looking again only while GitHub says it
/// isn't there. Err: why git still can't reach it, in words.
pub(crate) async fn wait_reachable(repo_path: &str, url: &str) -> Result<(), String> {
    let mut said = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        match GitCmd::at(repo_path).args(["ls-remote", "--heads", "--", url]).timeout(ADDRESS_CHECK_TIMEOUT).run().await {
            Ok(ls) if ls.ok() => return Ok(()),
            Ok(ls) => said = ls.stderr.trim().to_string(),
            Err(e) => return Err(e.to_string()),
        }
        if !said.contains("Repository not found") {
            break;
        }
    }
    let id = if said.contains("Repository not found") {
        Box::pin(crate::push_identity::identity_for_url(repo_path, None, url)).await.ok()
    } else {
        None
    };
    let login = id.as_ref().and_then(|i| i.login.as_deref());
    let key = id.as_ref().and_then(|i| i.key_path.as_deref());
    if crate::sparse::missing_own_repository(&said, url, login) {
        return Err("GitHub hasn't shown it to git yet; wait a moment.".to_string());
    }
    let explained = crate::sparse::explain_transport_error_as(&said, key, url, false, login);
    let hint = explained.split("\n\ngit said:").next().unwrap_or(&explained).trim().to_string();
    Err(if hint.is_empty() || explained.trim() == said { format!("git said: {}", said) } else { hint })
}

/// What `resolve_push_target` reads, from git's own records
/// (`branch.<b>.remote` / `.merge`), not parsed out of the upstream's
/// display name, which a remote named with a slash or a branch tracking a
/// LOCAL branch (`remote = .`) would both defeat.
pub(crate) async fn push_inputs(repo_path: &str, status: &RepoStatus, branch: &str, set_upstream: bool) -> PushInputs {
    let cfg = |key: String| GitCmd::at(repo_path).args(["config", "--get", key.as_str()]).ok_text();
    PushInputs {
        branch: branch.to_string(),
        upstream: status.upstream.clone(),
        branch_remote: cfg(format!("branch.{}.remote", branch)).await,
        merge_ref: cfg(format!("branch.{}.merge", branch)).await,
        push_remote: cfg(format!("branch.{}.pushRemote", branch)).await,
        push_default: cfg("remote.pushDefault".to_string()).await,
        remotes: status.push.remotes.iter().map(|r| r.name.clone()).collect(),
        set_upstream,
    }
}

/// Where a push goes, by the same rules as git's default (`push.default =
/// simple`). An upstream of the SAME name is pushed to. One of a different
/// name is refused: a branch started from `origin/main` with tracking would
/// otherwise send its commits straight into main — git's own default refuses
/// exactly this. A branch that tracks a local branch has no remote branch to
/// go to. With no upstream the remote is the one git would choose
/// (`pushRemote`, `pushDefault`, `branch.remote`, `origin`), or the only one
/// there is; the branch is then published under its own name. "Publish"
/// (`set_upstream`) always means: this name, on that remote, tracked from now
/// on.
pub fn resolve_push_target(i: &PushInputs) -> Result<PushTarget, Refusal> {
    let own = format!("refs/heads/{}", i.branch);
    let value = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let branch_remote = value(&i.branch_remote);
    let tracks_local = branch_remote.as_deref() == Some(".");
    let upstream_remote = branch_remote.clone().filter(|r| r != ".");

    // 1. Which remote — git's own precedence: branch.<b>.pushRemote, then
    //    remote.pushDefault (a fork, in a triangular workflow), then the
    //    upstream's remote, then origin, then the only one there is.
    let picked = value(&i.push_remote).or_else(|| value(&i.push_default)).or_else(|| upstream_remote.clone());
    let remote = match picked {
        Some(r) => r,
        None if i.remotes.iter().any(|r| r == "origin") => "origin".to_string(),
        None if i.remotes.len() == 1 => i.remotes[0].clone(),
        None if i.remotes.is_empty() => {
            return Err(refuse(
                "no-remote",
                "This repository isn't on a server yet: it has no remote to push to. Add one (the address of an empty repository on GitHub or your own server) and publish there.",
            ))
        }
        None => {
            return Err(refuse(
                "ambiguous-remote",
                format!(
                    "This branch has no upstream and there are {} remotes ({}), none called origin. Tell git which one to use — `git config remote.pushDefault <name>` — or push once from the terminal with `git push -u <remote> {}`.",
                    i.remotes.len(),
                    i.remotes.join(", "),
                    i.branch
                ),
            ))
        }
    };

    // 2. A branch tracking a LOCAL branch has no remote branch to go to.
    if tracks_local && !i.set_upstream {
        return Err(refuse(
            "local-upstream",
            format!(
                "This branch tracks the local branch `{}`, not a remote one, so a push has nowhere to go. Publish it as `{}/{}` instead.",
                i.upstream.as_deref().unwrap_or("?"),
                remote,
                i.branch
            ),
        ));
    }

    // 3. Pushing to the upstream's own remote: the branch there must have the
    //    same name (`push.default = simple`), or a branch started from
    //    `origin/main` with tracking would send its commits into main. A
    //    DIFFERENT remote (pushRemote / pushDefault) is a triangular workflow:
    //    git pushes the branch under its own name there and asks nothing.
    if let (Some(_), Some(up_remote)) = (&i.upstream, &upstream_remote) {
        if *up_remote == remote {
            let merge = value(&i.merge_ref).unwrap_or_else(|| own.clone());
            if merge != own && !i.set_upstream {
                let there = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
                return Err(refuse(
                    "upstream-name-differs",
                    format!(
                        "This branch tracks `{remote}/{there}`, which has a different name, so pushing would send your commits into `{there}`. Publish it as `{remote}/{branch}` instead.",
                        branch = i.branch
                    ),
                ));
            }
            let re_pointed = merge != own;
            return Ok(PushTarget { remote, remote_ref: own, set_upstream: i.set_upstream || re_pointed });
        }
        return Ok(PushTarget { remote, remote_ref: own, set_upstream: i.set_upstream });
    }

    // No remote upstream: publish under the branch's own name, tracked from now on.
    Ok(PushTarget { remote, remote_ref: own, set_upstream: true })
}

/// Every submodule pointer the commits being pushed set — `since..HEAD` for
/// the remote's current tip, every commit no remote-tracking branch of that
/// remote has for a first publish — and whether the commit each points at is
/// on a remote-tracking branch of the remote a clone would fetch it from. A
/// clone may check out any of the pushed commits, so an intermediate pointer
/// counts as much as HEAD's (`git push --recurse-submodules=check` walks the
/// same commits). Read with -z, so a path with non-ASCII bytes is itself.
pub(crate) async fn submodules_needing_push(repo_path: &str, remote: &str, since: Option<&str>) -> Vec<SubmodulePush> {
    let repo = std::path::Path::new(repo_path);
    let mut args: Vec<String> = vec![
        "log".into(),
        "-z".into(),
        "--raw".into(),
        "--no-abbrev".into(),
        "--no-renames".into(),
        "--ignore-submodules=none".into(),
        "--diff-filter=AMT".into(),
        "--format=".into(),
    ];
    match since {
        Some(base) => args.push(format!("{}..HEAD", base)),
        None => args.extend(["HEAD".to_string(), "--not".to_string(), format!("--remotes={}", remote)]),
    }
    let Ok(out) = GitCmd::at(repo_path).args(args.iter().map(String::as_str)).run().await else { return Vec::new() };
    if !out.ok() {
        return Vec::new();
    }
    // `:<srcmode> <dstmode> <src> <dst> <status>\0<path>\0`, per change.
    let toks: Vec<&[u8]> = out.stdout.split(|b| *b == 0).collect();
    let mut pointers: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.first() == Some(&b':') {
            let header = String::from_utf8_lossy(t).to_string();
            let f: Vec<&str> = header.split(' ').collect();
            if f.len() >= 5 && f[1] == "160000" {
                if let Some(p) = toks.get(i + 1) {
                    pointers.insert((String::from_utf8_lossy(p).to_string(), f[3].to_string()));
                }
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    if pointers.is_empty() {
        return Vec::new();
    }
    let declared = crate::submodules::parse_gitmodules(&crate::submodules::gitmodules_listing(repo).await);
    let super_url = GitCmd::at(repo_path)
        .args(["config", "--get", &format!("remote.{}.url", remote)])
        .ok_text()
        .await
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut out_list = Vec::new();
    for (path, recorded) in pointers {
        let sub = repo.join(&path);
        let populated = sub.join(".git").exists() && GitCmd::at(&sub).args(["rev-parse", "--git-dir"]).ok_text().await.is_some();
        let branch = if populated {
            GitCmd::at(&sub).args(["symbolic-ref", "-q", "--short", "HEAD"]).ok_text().await.map(|b| b.trim().to_string()).filter(|b| !b.is_empty())
        } else {
            None
        };
        let state = if !populated {
            "not-populated"
        } else if !GitCmd::at(&sub).args(["cat-file", "-e", &format!("{}^{{commit}}", recorded)]).run().await.map(|o| o.ok()).unwrap_or(false) {
            "not-here"
        } else {
            let which = crate::submodules::clone_remote(&sub, declared.get(&path).and_then(|m| m.url.as_deref()), super_url.as_deref()).await;
            if crate::submodules::on_remote(&sub, &recorded, which.as_deref()).await {
                continue;
            }
            "not-on-remote"
        };
        out_list.push(SubmodulePush { recorded_short: recorded.chars().take(7).collect(), recorded, path, state: state.into(), branch });
    }
    out_list
}

/// One sentence per submodule the push check could not clear, for a push
/// that went ahead. "Has" means a remote-tracking branch here has it: what
/// this checkout knows, which a fetch may update.
fn submodule_push_notes(checks: &[SubmodulePush]) -> Vec<String> {
    checks
        .iter()
        .map(|s| match s.state.as_str() {
            "not-on-remote" => format!("Pushed anyway: {} records {}, which none of its remote-tracking branches has here — push {} too.", s.path, s.recorded_short, s.path),
            "not-here" => format!("{} records {}, a commit its checkout here does not have, so whether a remote has it could not be checked.", s.path, s.recorded_short),
            _ => format!("{} is not initialised here, so whether a remote has {} could not be checked.", s.path, s.recorded_short),
        })
        .collect()
}

pub async fn push(repo_path: &str, set_upstream: bool, allow_unpushed_submodules: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Push, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let Some(branch) = before.branch.clone() else {
        return Ok(OpResult::refused(
            refuse("detached-head", "You're not on a branch."),
            before,
        ));
    };

    // Which remote, and which branch there.
    let inputs = push_inputs(repo_path, &before, &branch, set_upstream).await;
    let target = match resolve_push_target(&inputs) {
        Ok(t) => t,
        Err(r) => return Ok(OpResult::refused(r, before)),
    };
    let PushTarget { remote, remote_ref, set_upstream: needs_upstream } = target;
    crate::git_exec::validate_remote_name(&remote)?;
    crate::git_exec::validate_branch_name(&branch)?;
    // What this push advertises: every submodule pointer the remote's tip does
    // not already record must point at a commit some remote of the submodule
    // has, or a clone of the pushed branch cannot check it out — the check
    // `git push --recurse-submodules=check` makes. Refused with the
    // submodules named; pushing anyway is the person's call.
    let remote_tip = GitCmd::at(repo_path)
        .args(["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{}/{}^{{commit}}", remote, remote_ref.trim_start_matches("refs/heads/"))])
        .ok_text()
        .await
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let sub_checks = submodules_needing_push(repo_path, &remote, remote_tip.as_deref()).await;
    let unpushed: Vec<&SubmodulePush> = sub_checks.iter().filter(|s| s.state == "not-on-remote").collect();
    if !unpushed.is_empty() && !allow_unpushed_submodules {
        let msg = if unpushed.len() == 1 {
            let s = unpushed[0];
            let how = if s.state == "not-on-remote" && s.branch.is_none() {
                "(it is detached — start a branch there first)"
            } else {
                "(or fetch it, if that commit was pushed from elsewhere)"
            };
            format!(
                "{} records {}, which none of {}'s remote-tracking branches has — as far as this checkout knows. Anyone fetching this push could not check it out. Push {} first {}, or push anyway.",
                s.path, s.recorded_short, s.path, s.path, how
            )
        } else {
            format!(
                "{} record commits that none of their remote-tracking branches has — as far as this checkout knows. Anyone fetching this push could not check them out. Push them first (or fetch them, if those commits were pushed from elsewhere), or push anyway.",
                unpushed.iter().map(|s| format!("{} ({})", s.path, s.recorded_short)).collect::<Vec<_>>().join(", ")
            )
        };
        let mut r = OpResult::refused(refuse("submodules-unpushed", msg), before);
        r.unpushed_submodules = sub_checks;
        return Ok(r);
    }
    let args = build_push_args(&remote, &branch, &remote_ref, needs_upstream);
    let out = GitCmd::at(repo_path)
        .args(args)
        .stall(crate::git_exec::net_stall())
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = Box::pin(explain_transport_failure(repo_path, GitOp::Push, &out.stderr, &remote, &after)).await;
        return Ok(OpResult::failed(a, after));
    }

    let refs = parse_push_porcelain(&out.text());
    let up_to_date = refs.iter().all(|r| r.flag == "=");
    let outcome = PushOutcome {
        up_to_date,
        new_upstream: if needs_upstream {
            after.upstream.clone()
        } else {
            None
        },
        remote: remote.clone(),
        refs,
    };

    let headline = if up_to_date {
        "Already up to date.".to_string()
    } else {
        format!("Pushed to {}.", remote)
    };
    let mut detail = outcome
        .refs
        .iter()
        .map(|r| format!("{} {}", r.to, r.summary))
        .collect::<Vec<_>>()
        .join(", ");
    let notes = submodule_push_notes(&sub_checks);
    if !notes.is_empty() {
        detail = format!("{} {}", detail, notes.join(" ")).trim().to_string();
    }

    Ok(OpResult {
        push: Some(outcome),
        unpushed_submodules: sub_checks,
        ..OpResult::done(headline, detail, after)
    })
}

/// `--porcelain` output: `<flag>\t<from>:<to>\t<summary> (<reason>)`.
pub fn parse_push_porcelain(out: &str) -> Vec<PushedRef> {
    let mut refs = Vec::new();
    for line in out.lines() {
        if line.starts_with("To ") || line.starts_with("Done") || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 {
            continue;
        }
        let (from, to) = parts[1].split_once(':').unwrap_or(("", parts[1]));
        refs.push(PushedRef {
            flag: parts[0].to_string(),
            from: from.to_string(),
            to: to.to_string(),
            summary: parts[2].to_string(),
        });
    }
    refs
}

// --- Pull ----------------------------------------------------------------

/// `git pull` is never used: its behaviour depends on `pull.rebase`, `pull.ff`
/// and `branch.<n>.rebase`, so the same button could merge for one user and
/// rebase for another. Fetch and integrate are separate, and the mode the user
/// picked is the command that runs.
/// `source`: integrate a named remote-tracking branch ("origin/main") instead
/// of the branch's upstream — every Pull rule still applies, with it standing
/// in for the upstream.
pub async fn pull(repo_path: &str, mode: PullMode, with_lfs: bool, autostash: bool, source: Option<&str>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let mut facts = StatusFacts::from(&before);
    if let Some(src) = source {
        facts.upstream = Some(src.to_string());
    }
    if let Some(r) = check(&Intent::Pull { mode, autostash }, &facts) {
        return Ok(OpResult::refused(r, before));
    }
    if let Some(src) = source {
        return Box::pin(pull_from(repo_path, before, mode, with_lfs, autostash, src)).await;
    }
    let upstream = before.upstream.clone().unwrap_or_default();
    // The remote by git's own record, so a remote named with a slash is read
    // whole and a branch tracking a LOCAL branch is told apart.
    let branch = before.branch.clone().unwrap_or_default();
    let branch_remote = GitCmd::at(repo_path)
        .args(["config", "--get", &format!("branch.{}.remote", branch)])
        .ok_text()
        .await
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if branch_remote.as_deref() == Some(".") {
        return Ok(OpResult::refused(
            refuse(
                "local-upstream",
                format!(
                    "This branch tracks the local branch `{}`, not a remote one, so there is nothing to fetch. To bring its commits in, merge or rebase onto `{}` from the terminal.",
                    upstream, upstream
                ),
            ),
            before,
        ));
    }
    let remote = branch_remote.unwrap_or_else(|| upstream.split('/').next().unwrap_or("origin").to_string());
    Box::pin(pull_integrate(repo_path, before, mode, with_lfs, autostash, remote, upstream)).await
}

/// Pull from a named remote-tracking branch: the configured remote it starts
/// with (the longest, so a remote named with a slash is read whole), the rest
/// its branch.
async fn pull_from(repo_path: &str, before: RepoStatus, mode: PullMode, with_lfs: bool, autostash: bool, src: &str) -> Result<OpResult, AppError> {
    let remotes: Vec<String> = GitCmd::at(repo_path).args(["remote"]).ok_text().await.unwrap_or_default().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let Some(remote) = remotes.iter().filter(|r| src.starts_with(&format!("{}/", r))).max_by_key(|r| r.len()).cloned() else {
        return Ok(OpResult::refused(refuse("no-such-remote", format!("`{}` doesn't start with a remote of this repository.", src)), before));
    };
    crate::git_exec::validate_branch_name(&src[remote.len() + 1..])?;
    Box::pin(pull_integrate(repo_path, before, mode, with_lfs, autostash, remote, src.to_string())).await
}

/// The fetch and the merge or rebase of a pull, `upstream` being the
/// remote-tracking branch integrated (`<remote>/<branch>`).
async fn pull_integrate(repo_path: &str, before: RepoStatus, mode: PullMode, with_lfs: bool, autostash: bool, remote: String, upstream: String) -> Result<OpResult, AppError> {
    let named = before.upstream.as_deref() != Some(upstream.as_str());
    crate::git_exec::validate_remote_name(&remote)?;
    let head_before = before.head_oid.clone().unwrap_or_default();

    // Fetch only the remote's own refs: no refspec, no --prune, no --tags, so
    // nothing local can move in this step. No submodule recursion either —
    // `fetch.recurseSubmodules=on-demand` would fail the whole pull on one
    // unreachable submodule remote. `--progress`, so a big fetch is stopped
    // only when it goes silent, never for taking long — and kept as a pack
    // (`fetch.unpackLimit=1`), because git reports nothing at all while it
    // unpacks a fetch of under a hundred objects, however big they are.
    let fetched = GitCmd::at(repo_path)
        .for_fetch()
        .args(["fetch", "--progress", "--no-recurse-submodules", "--", &remote])
        .run()
        .await?;
    if fetched.ok() {
        crate::git_exec::housekeeping(repo_path).await;
    }
    if !fetched.ok() {
        let after = snapshot(repo_path).await?;
        let a = Box::pin(explain_transport_failure(repo_path, GitOp::Fetch, &fetched.stderr, &remote, &after)).await;
        return Ok(OpResult::failed(a, after));
    }

    // Fully qualified, so a local branch of the same name can't win.
    let upstream_ref = format!("refs/remotes/{}", upstream);
    if named {
        if GitCmd::at(repo_path).args(["rev-parse", "--verify", "--quiet", &upstream_ref]).ok_text().await.is_none() {
            let after = snapshot(repo_path).await?;
            return Ok(OpResult::refused(refuse("no-such-remote-branch", format!("{} has no branch `{}` — nothing to rebase onto.", remote, &upstream[remote.len() + 1..])), after));
        }
        // A rebase re-writes my commits: never one a remote branch already
        // has, which only a force push could replace — GitSwitch never does.
        if mode == PullMode::Rebase {
            let count = |args: Vec<String>| async move { GitCmd::at(repo_path).args(args).ok_text().await.and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(0) };
            let mine = count(vec!["rev-list".into(), "--count".into(), format!("{}..HEAD", upstream_ref)]).await;
            let unpushed = count(vec!["rev-list".into(), "--count".into(), "HEAD".into(), "--not".into(), upstream_ref.clone(), "--remotes".into()]).await;
            if mine > unpushed {
                let after = snapshot(repo_path).await?;
                return Ok(OpResult::refused(
                    refuse(
                        "would-rewrite-pushed",
                        format!("{} of the commits a rebase onto {} would re-write are already on a remote branch, and only a force push could replace them. Pull with merge instead.", mine - unpushed, upstream),
                    ),
                    after,
                ));
            }
        }
    }
    // Large files are downloaded after, by the step below, not inside the
    // merge or rebase on its 180s clock (see LfsAround).
    let lfs = lfs_before(repo_path, &before).await;
    let out = lfs
        .apply(GitCmd::at(repo_path))
        .args(build_integrate_args(mode, &upstream_ref, autostash))
        .timeout(NET_TIMEOUT)
        .run()
        .await?;

    let after = snapshot(repo_path).await?;
    // An autostash that git could not re-apply leaves conflicted files with
    // no rebase in progress, and stores the entry in the stash list (a clean
    // re-apply stores nothing). Git may still exit 0 here, so this is measured
    // — against `before`, so an old entry named "autostash" from an earlier
    // pull cannot make a clean pull look failed.
    let autostash_conflict = mode == PullMode::Rebase
        && autostash
        && after.operation.is_none()
        && (after.conflicted_count > 0 || after.stash_count > before.stash_count);
    let head_after = after.head_oid.clone().unwrap_or_default();
    let conflicts: Vec<String> = after
        .entries
        .iter()
        .filter(|e| e.kind == "conflicted")
        .map(|e| e.path.clone())
        .collect();

    let fast_forward = !head_before.is_empty()
        && GitCmd::at(repo_path)
            .args(["merge-base", "--is-ancestor", &head_before, &head_after])
            .run()
            .await
            .map(|o| o.ok())
            .unwrap_or(false);
    // The upstream commits that came in — not `head_before..head_after`, which
    // would also count the local commits a rebase replayed or the merge commit.
    let commits_pulled = if head_before == head_after {
        0
    } else {
        GitCmd::at(repo_path)
            .args([
                "rev-list",
                "--count",
                &format!("{}..{}", head_before, upstream_ref),
            ])
            .ok_text()
            .await
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    };
    let files_changed = if head_before == head_after {
        0
    } else {
        GitCmd::at(repo_path)
            .args([
                "diff",
                "--name-only",
                &format!("{}..{}", head_before, head_after),
            ])
            .ok_text()
            .await
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    };

    let outcome = PullOutcome {
        mode: mode.label().to_string(),
        head_before: head_before.clone(),
        head_after: head_after.clone(),
        fast_forward,
        commits_pulled,
        files_changed,
        conflicts: conflicts.clone(),
        recovery: if head_before.is_empty() || head_before == head_after {
            None
        } else {
            Some(format!("git reset --hard {}", head_before))
        },
        autostash_conflict,
        autostash_left: if autostash_conflict { Some("stash@{0}".to_string()) } else { None },
        submodules_moved: after.entries.iter().filter(|e| e.is_submodule && e.sub_commit_changed).count(),
    };

    if autostash_conflict {
        let a = git_advice::explain(
            GitOp::Rebase,
            &format!("Applying autostash resulted in conflicts\n{}\n{}", out.text(), out.stderr),
            &advice_ctx(&after),
        );
        return Ok(OpResult {
            pull: Some(outcome),
            ..OpResult::failed(a, after)
        });
    }

    if !out.ok() {
        // Conflicts are left exactly as they are — resolving them is the user's
        // work, and aborting for them would throw it away.
        let a = git_advice::explain(
            match mode {
                PullMode::Rebase => GitOp::Rebase,
                PullMode::Merge => GitOp::Merge,
                // The user pressed Pull, so that is the word the message uses.
                PullMode::FfOnly => GitOp::Pull,
            },
            &format!("{}\n{}", out.text(), out.stderr),
            &advice_ctx(&after),
        );
        return Ok(OpResult {
            pull: Some(outcome),
            ..OpResult::failed(a, after)
        });
    }

    let mut headline = if commits_pulled == 0 {
        "Already up to date.".to_string()
    } else {
        format!(
            "Pulled {} commit(s) with {}.",
            commits_pulled,
            mode.label()
        )
    };
    let mut detail = if commits_pulled == 0 {
        String::new()
    } else {
        format!(
            "{} file(s) changed. {}",
            files_changed,
            if fast_forward {
                "Fast-forwarded."
            } else {
                "History was combined."
            }
        )
    };

    // A plain pull only downloads LFS content for files it touches, and only
    // when the repository's LFS filters are configured. Without them every
    // pulled large file lands as a pointer stub and nothing says so — which is
    // why this step is offered, and why it is named in the result either way.
    let lfs_state;
    if with_lfs {
        let done = fetch_lfs_content(repo_path).await?;
        lfs_state = Some(done.after.clone());
        if let Some(a) = done.error {
            detail.push_str(&format!(" The pull worked, but downloading the LFS files didn't: {}", a.headline));
        } else if done.fetched > 0 {
            headline.push_str(&format!(
                " Downloaded {} LFS file{}.",
                done.fetched,
                if done.fetched == 1 { "" } else { "s" }
            ));
            if done.configured_now {
                detail.push_str(" This repository's LFS filters weren't set up, so that was done first.");
            }
        }
    } else {
        // Not asked for, but say so rather than leaving silent stubs behind.
        let state = crate::lfs::lfs_status(repo_path).await?;
        if state.uses_lfs && state.pointers > 0 {
            detail.push_str(&format!(
                " {} LFS file(s) here are still pointer stubs — use Pull LFS files to download them.",
                state.pointers
            ));
        }
        lfs_state = Some(state);
    }

    Ok(OpResult {
        pull: Some(outcome),
        lfs: lfs_state,
        ..OpResult::done(headline, detail, after)
    })
}

// --- Submodules ----------------------------------------------------------

/// `paths`: only these submodules (the ones never initialised, from the UI);
/// `None` means every listed one — the clone-time behaviour.
pub async fn submodule_update(repo_path: &str, paths: Option<Vec<String>>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Submodule, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    if !before.has_submodules {
        return Ok(OpResult::refused(
            refuse("no-submodules", "This repository has no submodules."),
            before,
        ));
    }
    let url = before
        .push
        .remotes
        .first()
        .map(|r| r.fetch_url.clone())
        .unwrap_or_default();

    // Reuse the clone path's per-submodule loop: a batch `submodule update`
    // aborts on the first gitlink missing from .gitmodules and leaves the rest
    // cloned-but-empty.
    if let Some(p) = &paths {
        for path in p {
            validate_repo_path(path)?;
        }
    }
    // A populated submodule this moves — to the commit recorded for it — must
    // lose nothing: no changes, operation or conflicts inside, no commit only
    // its detached HEAD (or a nested one's: `--recursive` moves those too)
    // holds. One that was never checked out has nothing to lose.
    let mut was_populated: Vec<String> = Vec::new();
    if let Some(p) = &paths {
        let mut blockers: Vec<Refusal> = Vec::new();
        for path in p {
            let dir = PathBuf::from(repo_path).join(path);
            if !(crate::sparse::own_repository(&dir).await && crate::sparse::is_checked_out(&dir).await) {
                continue;
            }
            was_populated.push(path.clone());
            let recorded = crate::tree::recorded_gitlink(repo_path, path).await;
            if let Some(r) = Box::pin(crate::tree::submodule_move_blocker(repo_path, path, recorded.as_deref(), true)).await? {
                blockers.push(r);
            }
        }
        if let Some(first) = blockers.first() {
            let message = blockers.iter().map(|r| r.message.as_str()).collect::<Vec<_>>().join(" ");
            return Ok(OpResult::refused(refuse(&first.code, message), before));
        }
    }
    let guarded = before.push.repo_blocked || before.push.lock.locked;
    // What was really there before — checked out, not an interrupted
    // download's leftovers, which the download clears and fetches again: what
    // this download populates gets the repository's push treatment and its
    // large-file step.
    let mut populated_before = Vec::new();
    for p in crate::push_guard::populated_submodules(std::path::Path::new(repo_path)).await {
        if crate::sparse::is_checked_out(&std::path::Path::new(repo_path).join(&p)).await {
            populated_before.push(p);
        }
    }
    // Progress is recorded under the repository's path, for the page's strip,
    // and removed when the download is over (however it ended).
    let report = {
        let _progress = crate::sparse::ProgressGuard::hold(repo_path);
        let profile = crate::git_status::owning_profile_id(repo_path);
        Box::pin(crate::sparse::update_submodules(&PathBuf::from(repo_path), None, None, &url, paths.as_deref(), Some(repo_path), profile.as_deref())).await
    };
    // A submodule this download populated — nested ones included — under a
    // blocked or locked repository gets the same guard-rail at once; the
    // lock's own layer for it is Repair's.
    let mut inherited = String::new();
    let fresh: Vec<String> = crate::push_guard::populated_submodules(std::path::Path::new(repo_path)).await.into_iter().filter(|p| !populated_before.contains(p)).collect();
    // Their large files are fetched when the repository's own are all here
    // (whoever keeps those keeps these) — and git-lfs is set up in them before
    // any push guard is, so the guard chains its hook instead of git-lfs
    // refusing to install next to the guard's.
    let lfs_requested = !fresh.is_empty() && crate::lfs::lfs_status(repo_path).await.ok().is_some_and(|s| s.uses_lfs && s.installed && s.pointers == 0);
    if lfs_requested {
        for rel in &fresh {
            lfs_setup_before_guard(&PathBuf::from(repo_path).join(rel).to_string_lossy()).await;
        }
    }
    if guarded && !fresh.is_empty() {
        let notes = crate::push_guard::cover_submodules(repo_path, Some(&fresh)).await;
        inherited = if !notes.is_empty() {
            format!(" Could not extend the push block to: {}.", notes.join("; "))
        } else if before.push.lock.locked {
            let out = Box::pin(crate::push_lock::lock_downloaded_submodules(repo_path, &fresh)).await;
            if out.applied() {
                " Pushes are locked here, so the downloaded submodule(s) were locked too, with the repository's lock.".to_string()
            } else {
                format!(
                    " Pushes are locked here; the downloaded submodule(s) got the same guard-rail but not the lock ({}) — Repair (administrator) under Push access adds it.",
                    out.message.trim_end_matches('.')
                )
            }
        } else {
            " Pushes are blocked here, so the downloaded submodule(s) got the same guard-rail.".to_string()
        };
    }
    // The large files of what this download populated: fetched as decided
    // above, said either way — a submodule's stubs are never left silent.
    let mut lfs_notes = String::new();
    if !fresh.is_empty() {
        let _progress = crate::sparse::ProgressGuard::hold(repo_path);
        for s in Box::pin(crate::sparse::lfs_in_submodules(std::path::Path::new(repo_path), Some(&fresh), lfs_requested, Some(repo_path))).await {
            lfs_notes.push_str(&format!(" Git LFS in {}: {}", s.path, s.report.note));
        }
    }
    let after = snapshot(repo_path).await?;
    // `git submodule update` can exit 0 and leave a populated one where it
    // was (`update = none` somewhere git reads it, a merge of an ancestor):
    // measured here, never assumed from the exit code.
    let mut stuck: Vec<String> = Vec::new();
    for path in &was_populated {
        if report.failed_paths.contains(path) {
            continue;
        }
        let dir = PathBuf::from(repo_path).join(path);
        if let (Some(h), Some(r)) = (crate::sync::oid(&dir, "HEAD").await, crate::tree::recorded_gitlink(repo_path, path).await) {
            if h != r {
                stuck.push(format!("{} is at {}, not the recorded {}", path, crate::sync::short(&h), crate::sync::short(&r)));
            }
        }
    }

    let headline = format!("{} of {} submodule(s) up to date.", report.downloaded, report.listed);
    let mut detail = String::new();
    if report.unlisted > 0 {
        detail.push_str(&format!(
            "{} submodule entry(s) have no address in .gitmodules, so git can't fetch them — the same as any clone. ",
            report.unlisted
        ));
    }
    if !report.cleared.is_empty() {
        detail.push_str(&format!("An interrupted download of {} was cleared before downloading again. ", report.cleared.join(", ")));
    }
    if let Some(err) = &report.error {
        detail.push_str(err);
    }
    detail.push_str(&inherited);
    detail.push_str(&lfs_notes);
    let detail = detail.trim().to_string();

    if !stuck.is_empty() {
        let advice = Advice {
            headline: "Not every submodule reached the commit this repository records.".to_string(),
            guidance: format!("Not moved: {}. {}", stuck.join("; "), detail).trim().to_string(),
            action: None,
            link: None,
            remote: None,
            repo: None,
            git_said: String::new(),
        };
        return Ok(OpResult { submodules: Some(report), ..OpResult::failed(advice, after) });
    }
    Ok(OpResult {
        submodules: Some(report),
        ..OpResult::done(headline, detail, after)
    })
}

// --- Git LFS -------------------------------------------------------------

/// Download the real content for every LFS pointer stub in the checkout.
/// `git lfs pull` is fetch + checkout for the current ref only, so it never
/// touches history, the index, or files that aren't LFS pointers.
/// What a download of LFS content actually did. Every number is measured by
/// counting pointer stubs before and after, never inferred from an exit code.
pub struct LfsFetch {
    pub after: crate::lfs::LfsStatus,
    pub fetched: usize,
    /// The repository had no LFS filters, so `git lfs install --local` was run
    /// first — without it `git lfs pull` exits 0 and downloads nothing.
    pub configured_now: bool,
    /// git-lfs failed and files are still missing. The caller decides how
    /// loudly to say so.
    pub error: Option<Advice>,
    /// git-lfs exited with an error, yet no file is missing — an object it
    /// could not fetch whose file is already here, or a stop after the last
    /// file was written. Said, never reported as a failure.
    pub warning: Option<Advice>,
}

impl LfsFetch {
    fn nothing_to_do(status: crate::lfs::LfsStatus) -> Self {
        LfsFetch {
            after: status,
            fetched: 0,
            configured_now: false,
            error: None,
            warning: None,
        }
    }
}

/// Large files around a command that rewrites the worktree: a switch, a hard
/// reset, a revert, a cherry-pick, the merge or rebase of a pull.
///
/// Left to itself, git downloads every large file such a command touches
/// INSIDE the command, through the smudge filter: on a local timeout, with
/// nothing on screen moving, and a kill at the deadline leaves the worktree
/// half written. So the command runs with `GIT_LFS_SKIP_SMUDGE=1` — pointer
/// stubs are written instantly — and the download is a separate step on the
/// large-file clock, with its progress banner. When every large file was
/// present before, the download runs so that stays true; a deliberately
/// partial checkout (one folder downloaded) is left partial, and said.
pub(crate) struct LfsAround {
    skip: bool,
    complete_before: bool,
}

impl LfsAround {
    /// For a command that does not touch the worktree.
    pub(crate) fn none() -> Self {
        LfsAround { skip: false, complete_before: false }
    }

    pub(crate) fn apply(&self, cmd: GitCmd) -> GitCmd {
        if self.skip {
            cmd.env("GIT_LFS_SKIP_SMUDGE", "1")
        } else {
            cmd
        }
    }
}

pub(crate) async fn lfs_before(repo: &str, before: &RepoStatus) -> LfsAround {
    if !before.uses_lfs {
        return LfsAround::none();
    }
    match crate::lfs::lfs_status(repo).await {
        Ok(s) if s.installed => LfsAround { skip: true, complete_before: s.pointers == 0 },
        // Without git-lfs there is no smudge filter to wait on.
        _ => LfsAround::none(),
    }
}

/// After the command succeeded: what became of the large files, as a sentence
/// (with its leading space) for the result's detail — or nothing to add.
pub(crate) async fn lfs_after(repo: &str, plan: &LfsAround) -> String {
    if !plan.skip {
        return String::new();
    }
    if plan.complete_before {
        return match fetch_lfs_content(repo).await {
            Ok(done) => match done.error {
                Some(a) => format!(" The large files didn't download: {} The LFS card can fetch them.", a.headline),
                None if done.fetched > 0 => {
                    format!(" Downloaded {} LFS file{}.", done.fetched, if done.fetched == 1 { "" } else { "s" })
                }
                None => String::new(),
            },
            Err(e) => format!(" The large files didn't download: {}", e),
        };
    }
    match crate::lfs::lfs_status(repo).await {
        Ok(s) if s.pointers > 0 => format!(
            " {} LFS file{} here {} a pointer stub; the LFS card downloads them.",
            s.pointers,
            if s.pointers == 1 { "" } else { "s" },
            if s.pointers == 1 { "is" } else { "are" }
        ),
        _ => String::new(),
    }
}

/// git-lfs's own setup (its filters and hooks) in a repository that uses large
/// files and has none — run BEFORE a push guard adds its pre-push hook: git-lfs
/// refuses to install over a hook that is not its own, while the guard keeps
/// and chains whatever hook it finds. The other way round, every large file
/// would stay a stub.
pub(crate) async fn lfs_setup_before_guard(dir: &str) {
    if let Ok(s) = crate::lfs::lfs_status(dir).await {
        if s.uses_lfs && s.installed && !s.filters_configured {
            let _ = GitCmd::at(dir).args(["lfs", "install", "--local"]).timeout(LOCAL_TIMEOUT).run().await;
        }
    }
}

/// Download the content behind every pointer stub. Safe to call on any repo:
/// one that doesn't use LFS, has no stubs, or has no git-lfs installed simply
/// reports that nothing was done.
pub(crate) async fn fetch_lfs_content(repo_path: &str) -> Result<LfsFetch, AppError> {
    Box::pin(fetch_lfs_content_reporting(repo_path, crate::lfs::progress_path(repo_path))).await
}

/// The same, with git-lfs's progress written to `progress_at` — the file the
/// page polls. A submodule's download after an update reports into the
/// superproject's, which is the one the page is looking at.
pub(crate) async fn fetch_lfs_content_reporting(repo_path: &str, progress_at: std::path::PathBuf) -> Result<LfsFetch, AppError> {
    let before = crate::lfs::lfs_status(repo_path).await?;
    if !before.uses_lfs || !before.installed || before.pointers == 0 {
        return Ok(LfsFetch::nothing_to_do(before));
    }

    // A clone made before git-lfs was installed has no lfs filters in its
    // config. In that state `git lfs pull` prints "Skipping object checkout,
    // Git LFS is not installed for this repository" and exits 0 having done
    // nothing. `git lfs install --local` writes exactly those filter entries,
    // touches nothing else, and is what the LFS docs tell you to run first.
    let mut configured_now = false;
    if !before.filters_configured {
        let setup = GitCmd::at(repo_path)
            .args(["lfs", "install", "--local"])
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !setup.ok() {
            let advice = git_advice::explain(GitOp::Lfs, &setup.stderr, &AdviceCtx::default());
            return Ok(LfsFetch {
                after: before,
                fetched: 0,
                configured_now: false,
                error: Some(advice),
                warning: None,
            });
        }
        configured_now = true;
    }

    let progress = crate::lfs::ProgressFile::start_at(progress_at);
    let out = GitCmd::at(repo_path)
        .args(["lfs", "pull"])
        // Gigabytes of large files are normal; a transfer that is still moving
        // must not be killed on an ordinary fetch's clock. The progress file
        // is what makes the wait visible instead of looking like a hang.
        .timeout(LFS_TIMEOUT)
        .env("GIT_LFS_PROGRESS", progress.0.to_string_lossy().to_string())
        .run()
        .await?;
    drop(progress);

    // Measure, don't assume: count the stubs again. The exit code is not the
    // answer either way — git-lfs also fails on an object whose file is
    // already here, or is stopped after it wrote the last one. Every stub
    // filled, with every file still counted (a scan that failed would list
    // none and read as "nothing missing"), is a warning, not a failure.
    let after = crate::lfs::lfs_status(repo_path).await?;
    let fetched = before.pointers.saturating_sub(after.pointers);
    let complete = after.pointers == 0 && after.tracked >= before.tracked;
    let (error, warning) = if out.ok() {
        (None, None)
    } else {
        let stderr = if out.stderr.is_empty() { out.text() } else { out.stderr.clone() };
        let advice = git_advice::explain(GitOp::Lfs, &stderr, &AdviceCtx::default());
        if complete {
            (None, Some(advice))
        } else {
            (Some(advice), None)
        }
    };

    Ok(LfsFetch { after, fetched, configured_now, error, warning })
}

pub async fn lfs_pull(repo_path: &str) -> Result<OpResult, AppError> {
    // LFS state is read *before* the repo snapshot on purpose: with
    // filter.lfs.required set and git-lfs missing, `git status` itself fails,
    // and the person needs the install hint — not the snapshot's error.
    let lfs_before = crate::lfs::lfs_status(repo_path).await?;
    if !lfs_before.uses_lfs {
        return Ok(OpResult::refused_alone(refuse(
            "no-lfs",
            "This repository doesn't use Git LFS — there is nothing to pull.",
        )));
    }
    if !lfs_before.installed {
        return Ok(OpResult::refused_alone(refuse(
            "lfs-not-installed",
            format!("git-lfs isn't installed on this computer, so the large files can't be downloaded. Install it with {} and try again.", crate::lfs::install_hint()),
        )));
    }
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Submodule, &StatusFacts::from(&before)) {
        // Same rule as submodule update: not in the middle of a merge/rebase,
        // because lfs pull rewrites files in the worktree.
        return Ok(OpResult::refused(
            Refusal {
                code: r.code,
                message: r.message.replace("update submodules", "pull LFS files"),
            },
            before,
        ));
    }
    if lfs_before.pointers == 0 {
        return Ok(OpResult {
            lfs: Some(lfs_before),
            ..OpResult::done(
                "All LFS files are already here.",
                "Nothing needed downloading.",
                before,
            )
        });
    }

    let done = fetch_lfs_content(repo_path).await?;
    let lfs_after = done.after.clone();
    let fetched = done.fetched;
    let configured_now = done.configured_now;
    let after = snapshot(repo_path).await?;

    if let Some(a) = done.error {
        return Ok(OpResult {
            lfs: Some(lfs_after),
            detail: if fetched > 0 {
                format!("{} file(s) did download before it failed.", fetched)
            } else {
                a.guidance.clone()
            },
            ..OpResult::failed(a, after)
        });
    }

    let headline = if lfs_after.pointers == 0 {
        format!("Downloaded {} LFS file{}.", fetched, if fetched == 1 { "" } else { "s" })
    } else {
        format!(
            "Downloaded {} of {} LFS files — {} still missing.",
            fetched, lfs_before.pointers, lfs_after.pointers
        )
    };
    let mut detail = if lfs_after.pointers == 0 {
        "Every pointer stub now holds its real content.".to_string()
    } else if !lfs_after.filters_configured {
        "git lfs pull skipped the checkout because this repository's LFS filters aren't configured — even after setting them up. Run `git lfs install --local` in the folder and try again.".to_string()
    } else {
        "git lfs pull finished without an error but some files are still pointer stubs; their objects may not exist on the server.".to_string()
    };
    if configured_now {
        detail.push_str(" (LFS filters were not set up for this repository, so `git lfs install --local` was run first.)");
    }
    if let Some(w) = &done.warning {
        detail.push_str(&format!(" git-lfs still exited with an error, but no file is missing: {}", w.headline));
    }
    Ok(OpResult {
        lfs: Some(lfs_after),
        // git-lfs's own words stay under "what git said"; the card stays green.
        advice: done.warning.map(|w| Advice { guidance: String::new(), action: None, ..w }),
        ..OpResult::done(headline, detail, after)
    })
}

/// Download only the large files a person picked — one file, one folder, or
/// any set of them.
///
/// Two git-lfs commands rather than one `pull`, because they answer different
/// questions and only the pair is precise: `fetch --include` brings the objects
/// for the selection into the local store, and `checkout` writes the *exact*
/// paths into the working tree. `--include` is a comma-separated pattern list,
/// so a path containing a comma can only be fetched approximately; checkout
/// takes real arguments, which is why it, not the fetch, decides what lands.
pub async fn lfs_pull_paths(repo_path: &str, paths: Vec<String>) -> Result<OpResult, AppError> {
    let before = crate::lfs::lfs_files(repo_path).await?;
    if !before.uses_lfs {
        return Ok(OpResult::refused_alone(refuse(
            "no-lfs",
            "This repository doesn't use Git LFS — there is nothing to download.",
        )));
    }
    if !before.installed {
        return Ok(OpResult::refused_alone(refuse(
            "lfs-not-installed",
            format!("git-lfs isn't installed on this computer, so the large files can't be downloaded. Install it with {} and try again.", crate::lfs::install_hint()),
        )));
    }
    let selected: Vec<String> = paths
        .iter()
        .map(|p| p.trim_end_matches('/').to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if selected.is_empty() {
        return Ok(OpResult::refused_alone(refuse(
            "no-paths",
            "Nothing was selected, so there is nothing to download.",
        )));
    }
    for p in &selected {
        if let Err(msg) = crate::lfs::validate_lfs_path(p) {
            return Ok(OpResult::refused_alone(refuse("invalid-path", msg)));
        }
        if !before.knows(p) {
            return Ok(OpResult::refused_alone(refuse(
                "unknown-path",
                format!(
                    "`{}` isn't a large file or folder in this repository any more — the list may be out of date. Close and reopen it.",
                    p
                ),
            )));
        }
    }

    let snap = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Submodule, &StatusFacts::from(&snap)) {
        return Ok(OpResult::refused(
            Refusal { code: r.code, message: r.message.replace("update submodules", "download LFS files") },
            snap,
        ));
    }

    let wanted: Vec<&crate::lfs::LfsFile> = before
        .files
        .iter()
        .filter(|f| selected.iter().any(|s| crate::lfs::LfsListing::covers(s, &f.path)))
        .collect();
    let missing_before = wanted.iter().filter(|f| !f.present).count();
    let bytes_wanted: u64 = wanted.iter().filter(|f| !f.present).map(|f| f.size).sum();
    if missing_before == 0 && !wanted.is_empty() {
        return Ok(OpResult {
            lfs: Some(before.to_status()),
            ..OpResult::done(
                format!(
                    "{} file{} already here.",
                    wanted.len(),
                    if wanted.len() == 1 { " is" } else { "s are" }
                ),
                "Nothing needed downloading.",
                snap,
            )
        });
    }

    // Filters first: without them `git lfs checkout` writes nothing and still
    // exits 0 — the silent failure this whole feature exists to avoid.
    let mut configured_now = false;
    if !before.filters_configured {
        let setup = GitCmd::at(repo_path)
            .args(["lfs", "install", "--local"])
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !setup.ok() {
            let advice = git_advice::explain(GitOp::Lfs, &setup.stderr, &AdviceCtx::default());
            return Ok(OpResult { lfs: Some(before.to_status()), ..OpResult::failed(advice, snap) });
        }
        configured_now = true;
    }

    // Chunked, so an enormous selection is still exactly what was asked for
    // rather than a fallback that fetches and writes out the whole repository.
    let progress = crate::lfs::ProgressFile::start_at(crate::lfs::progress_path(repo_path));
    let mut failure: Option<String> = None;
    for chunk in crate::lfs::chunk_selection(&selected) {
        let include = crate::lfs::include_patterns(&chunk);
        let fetched_out = GitCmd::at(repo_path)
            .args(["lfs", "fetch"])
            .arg(format!("--include={}", include))
            .timeout(LFS_TIMEOUT)
            .env("GIT_LFS_PROGRESS", progress.0.to_string_lossy().to_string())
            .run()
            .await?;
        if !fetched_out.ok() {
            failure = Some(if fetched_out.stderr.is_empty() { fetched_out.text() } else { fetched_out.stderr });
            break;
        }
        // Write the selection into the working tree. Only content already in
        // the local store is written, a file you have edited is never
        // overwritten, and every pattern is escaped so a name holding a
        // wildcard matches itself and nothing else.
        let mut checkout = GitCmd::at(repo_path).args(["lfs", "checkout", "--"]);
        for p in &chunk {
            checkout = checkout.arg(crate::lfs::literal_pattern(p));
        }
        // Writing the files out is the slow half on a local disk, and it is
        // the half git-lfs reports on there — so it gets the progress file too.
        let checkout_out = checkout
            .timeout(LFS_TIMEOUT)
            .env("GIT_LFS_PROGRESS", progress.0.to_string_lossy().to_string())
            .run()
            .await?;
        if !checkout_out.ok() {
            failure = Some(if checkout_out.stderr.is_empty() { checkout_out.text() } else { checkout_out.stderr });
            break;
        }
    }
    drop(progress);

    let after = crate::lfs::lfs_files(repo_path).await?;
    let still_missing: Vec<&crate::lfs::LfsFile> = after
        .files
        .iter()
        .filter(|f| !f.present && selected.iter().any(|s| crate::lfs::LfsListing::covers(s, &f.path)))
        .collect();
    let arrived = missing_before.saturating_sub(still_missing.len());
    let status = snapshot(repo_path).await?;

    // git-lfs failed, yet everything asked for is here — and the scan still
    // knows every path asked for, so an empty list is not a failed read: a
    // warning on a green result, not a failure.
    let mut warning: Option<Advice> = None;
    if let Some(stderr) = failure.as_deref().filter(|_| still_missing.is_empty() && selected.iter().all(|p| after.knows(p))) {
        warning = Some(git_advice::explain(GitOp::Lfs, stderr, &AdviceCtx::default()));
    }
    if let Some(stderr) = failure.filter(|_| warning.is_none()) {
        let advice = git_advice::explain(GitOp::Lfs, &stderr, &AdviceCtx::default());
        let names: Vec<String> = still_missing.iter().take(5).map(|f| f.path.clone()).collect();
        // Name what is still a stub. Reporting only a count is how a failure
        // ends up looking like a success the next time the list is read.
        let detail = if still_missing.is_empty() {
            format!(
                "git-lfs reported an error, but all {} file{} asked for {} here now.",
                missing_before,
                if missing_before == 1 { "" } else { "s" },
                if missing_before == 1 { "is" } else { "are" }
            )
        } else {
            format!(
                "{} of {} file{} arrived. Still missing: {}{}.",
                arrived,
                missing_before,
                if missing_before == 1 { "" } else { "s" },
                names.join(", "),
                if still_missing.len() > names.len() {
                    format!(" and {} more", still_missing.len() - names.len())
                } else {
                    String::new()
                }
            )
        };
        return Ok(OpResult { lfs: Some(after.to_status()), detail, ..OpResult::failed(advice, status) });
    }

    let headline = if still_missing.is_empty() {
        // An older git-lfs reports no sizes at all; "(0 B)" would be a
        // measurement that was never taken.
        format!(
            "Downloaded {} file{}{}.",
            arrived,
            if arrived == 1 { "" } else { "s" },
            if before.sizes_known { format!(" ({})", crate::lfs::human_bytes(bytes_wanted)) } else { String::new() }
        )
    } else {
        format!(
            "Downloaded {} of {} files — {} still missing.",
            arrived, missing_before, still_missing.len()
        )
    };
    let mut detail = if still_missing.is_empty() {
        format!(
            "{} now hold{} real content.",
            if selected.len() == 1 { selected[0].clone() } else { format!("The {} selected paths", selected.len()) },
            if selected.len() == 1 { "s" } else { "" }
        )
    } else {
        "git-lfs reported no error, but some files are still pointer stubs — their objects may not exist on the server.".to_string()
    };
    if configured_now {
        detail.push_str(" (This repository's LFS filters weren't set up, so that was done first.)");
    }
    if let Some(w) = &warning {
        detail.push_str(&format!(" git-lfs still exited with an error, but no file asked for is missing: {}", w.headline));
    }
    Ok(OpResult {
        lfs: Some(after.to_status()),
        advice: warning.map(|w| Advice { guidance: String::new(), action: None, ..w }),
        ..OpResult::done(headline, detail, status)
    })
}

// --- Abort ---------------------------------------------------------------

pub async fn abort(repo_path: &str) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Abort, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let op = before.operation.clone().unwrap();
    let args: Vec<&str> = op.abort_command.split_whitespace().skip(1).collect();
    let out = GitCmd::at(repo_path)
        .args(args)
        .timeout(LOCAL_TIMEOUT)
        .run()
        .await?;
    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(op_kind_to_gitop(&op.kind), &out.stderr, &advice_ctx(&after));
        return Ok(OpResult::failed(a, after));
    }
    Ok(OpResult::done(
        format!("Aborted the {}.", op.kind),
        "The branch is back where it was before it started.".to_string(),
        after,
    ))
}

fn op_kind_to_gitop(kind: &str) -> GitOp {
    if kind.contains("rebase") || kind == "am" {
        GitOp::Rebase
    } else {
        GitOp::Merge
    }
}

/// Is the pick that stopped now empty — every change resolved to what HEAD
/// already has? Then `--continue` refuses and `--skip` is the only way on.
async fn staged_is_empty(repo_path: &str) -> bool {
    GitCmd::at(repo_path)
        .args(["diff", "--cached", "--quiet", "HEAD"])
        .run()
        .await
        .map(|o| o.ok())
        .unwrap_or(false)
}

// --- Continue ------------------------------------------------------------

/// Finish the operation in progress: `rebase --continue`, `cherry-pick
/// --continue`, `revert --continue`, `am --continue`, or the commit that
/// concludes a merge. A pick that became empty after the resolution is
/// skipped (git would refuse to continue it) and the result says so.
/// A rebase paused on a plain `pick` whose conflict is resolved and staged:
/// commit it here, as the commit it replays — its message letter for letter,
/// its author and date — so `rebase --continue` has nothing left to commit.
/// Left to git, the continue runs the message through the "editor" with
/// cleanup=strip: every line starting with '#' goes. A subject like "#123 fix
/// the parser" vanishes, and a message that is only such a line becomes an
/// empty one git refuses, with no way on but Abort. Nothing staged (git skips
/// the commit, and the caller measures that itself), or a stop that is not a
/// plain pick (an edit or a fixup of somebody's own interactive rebase): left
/// to git. Hooks are skipped, as git's own rebase skips them. Returns whether
/// a commit was made.
pub(crate) async fn commit_resolved_pick(repo: &std::path::Path) -> bool {
    let Some(stopped) = git_file(repo, "rebase-merge/stopped-sha").await.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else { return false };
    // The last line of `done` is the command the rebase stopped on.
    let Some(done) = git_file(repo, "rebase-merge/done").await else { return false };
    let last = done.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    if !(last.starts_with("pick ") || last.starts_with("p ")) || git_file(repo, "rebase-merge/amend").await.is_some() {
        return false;
    }
    // Options of that rebase a plain commit would not honour (dates, signing):
    // git's own continue then, as before.
    for opt in ["rebase-merge/cdate_is_adate", "rebase-merge/ignore_date", "rebase-merge/gpg_sign_opt"] {
        if git_file(repo, opt).await.is_some() {
            return false;
        }
    }
    let signoff = git_file(repo, "rebase-merge/signoff").await.is_some();
    // The stop must still be open — not committed already, by an earlier try
    // or by hand — and the continue must not be about to refuse: git refuses
    // it for unstaged edits to tracked files, and a commit made before that
    // refusal would be committed a second time on the retry.
    if !stop_is_open(repo).await {
        return false;
    }
    let unstaged = GitCmd::at(repo).pinned().args(["diff", "--quiet", "--ignore-submodules", "--"]).run().await.map(|o| o.code != 0).unwrap_or(true);
    if unstaged {
        return false;
    }
    let staged = GitCmd::at(repo).pinned().args(["diff", "--cached", "--quiet", "--ignore-submodules=none", "HEAD", "--"]).run().await.map(|o| o.code == 1).unwrap_or(false);
    if !staged {
        return false;
    }
    let mut args = vec!["commit", "--quiet", "--no-verify", "--cleanup=verbatim", "--allow-empty-message"];
    if signoff {
        args.push("--signoff");
    }
    args.extend(["-C", stopped.as_str()]);
    GitCmd::at(repo).pinned().args(args).timeout(NET_TIMEOUT).run().await.map(|o| o.ok()).unwrap_or(false)
}

/// The pick a rebase stopped on has not been committed yet: git writes the
/// message it would commit with to MERGE_MSG at the stop, and the commit —
/// by `rebase --continue`, by the app, or by hand — removes it.
pub(crate) async fn stop_is_open(repo: &std::path::Path) -> bool {
    git_file(repo, "MERGE_MSG").await.is_some()
}

/// A file of the repository's git directory, read whole; `None` when it is not there.
async fn git_file(repo: &std::path::Path, name: &str) -> Option<String> {
    let p = GitCmd::at(repo).args(["rev-parse", "--path-format=absolute", "--git-path", name]).ok_text().await?;
    std::fs::read_to_string(p.trim()).ok()
}

pub async fn continue_op(repo_path: &str, allow_markers: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    if let Some(r) = check(&Intent::Continue, &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    // What is staged is what the continue records: markers left in a staged
    // file would be committed as the resolution.
    if !allow_markers {
        if let Some((file, lines)) = staged_marker_offence(std::path::Path::new(repo_path)).await {
            return Ok(OpResult::refused(markers_refusal(&file, &lines), before));
        }
    }
    let op = before.operation.clone().unwrap();
    let cmd = op.continue_command.clone().unwrap_or_default();
    let mut args: Vec<String> = cmd.split_whitespace().skip(1).map(|s| s.to_string()).collect();
    let mut skipped = false;
    let picks = matches!(op.kind.as_str(), "rebase" | "rebase-interactive" | "cherry-pick" | "revert");
    // A rebase stop that was committed already (by hand, or by the app on a
    // try git then refused) has nothing left to skip or to commit.
    // (Only the merge backend writes a stop this way; the apply backend has no
    // MERGE_MSG at all, and keeps the old behaviour.)
    let committed_already = op.kind.starts_with("rebase")
        && git_file(std::path::Path::new(repo_path), "rebase-merge/stopped-sha").await.is_some()
        && !stop_is_open(std::path::Path::new(repo_path)).await;
    if picks && !committed_already && staged_is_empty(repo_path).await {
        // `rebase --skip` resets the working tree hard (cherry-pick and revert
        // skip with `reset --merge`, which keeps edits): unstaged edits to
        // tracked files would go with it, where `rebase --continue` refuses
        // over them. So a rebase skips only over a tree git calls clean, and
        // a check git could not answer counts as edits.
        if op.kind.starts_with("rebase") {
            let unstaged = GitCmd::at(repo_path).pinned().args(["diff", "--quiet", "--ignore-submodules", "--"]).run().await.map(|o| o.code != 0).unwrap_or(true);
            if unstaged {
                return Ok(OpResult::refused(
                    refuse(
                        "skip-would-discard",
                        "The stopped commit has nothing left to apply after the resolution, so the rebase can only skip it — and skipping resets the working tree, which would throw away your unstaged changes to tracked files. Nothing was changed. Set them aside first (in a terminal: `git stash`, then `git stash pop` once the rebase is finished), or discard them, then Continue.",
                    ),
                    before,
                ));
            }
        }
        args = vec![args[0].clone(), "--skip".into()];
        skipped = true;
    } else if op.kind.starts_with("rebase") && !committed_already {
        // The resolved commit keeps its message, whatever it starts with.
        commit_resolved_pick(std::path::Path::new(repo_path)).await;
    }
    let out = GitCmd::at(repo_path)
        .pinned()
        .args(args.iter().map(String::as_str))
        .timeout(NET_TIMEOUT)
        .run()
        .await?;
    let after = snapshot(repo_path).await?;
    if !out.ok() {
        let a = git_advice::explain(
            op_kind_to_gitop(&op.kind),
            &format!("{}\n{}", out.text(), out.stderr),
            &advice_ctx(&after),
        );
        return Ok(OpResult::failed(a, after));
    }
    // A rebase started with --autostash pops the stash when it finishes; a pop
    // that conflicts leaves conflicted files behind with no rebase in progress
    // and stores the stash as a new list entry (a clean pop stores nothing).
    if op.kind.contains("rebase") && after.operation.is_none() && (after.conflicted_count > 0 || after.stash_count > before.stash_count) {
        let a = git_advice::explain(
            GitOp::Rebase,
            &format!("Applying autostash resulted in conflicts\n{}\n{}", out.text(), out.stderr),
            &advice_ctx(&after),
        );
        return Ok(OpResult::failed(a, after));
    }
    let (headline, detail) = if after.operation.is_some() {
        (
            format!("Continued the {} — it stopped again.", op.kind),
            if after.conflicted_count > 0 {
                format!("{} file(s) conflict this time. Resolve and stage them, then continue again.", after.conflicted_count)
            } else {
                after.operation.as_ref().map(|o| o.detail.clone()).unwrap_or_default()
            },
        )
    } else {
        (
            format!("Finished the {}.", op.kind),
            if skipped {
                "The stopped commit had nothing left to apply after the resolution, so it was skipped.".to_string()
            } else {
                String::new()
            },
        )
    };
    Ok(OpResult::done(headline, detail, after))
}

// ---------------------------------------------------------------------------
// Making a folder a repository
// ---------------------------------------------------------------------------

/// What `init_repo` made.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct InitOutcome {
    pub path: String,
    /// The first branch, as git now names it.
    pub branch: String,
}

/// Make a folder a git repository (`git init`), but only a folder that is on
/// this computer, holds no repository and is inside none; otherwise nothing
/// is written and the reason is given. The first branch is the person's
/// `init.defaultBranch`, or `main`. No identity is written: a profile whose
/// folder this is applies through its `includeIf "gitdir:"` rule the moment
/// `.git` exists.
pub async fn init_repo(path: &str) -> Result<InitOutcome, AppError> {
    use crate::repo_scan::FolderState;
    match crate::repo_scan::folder_state(path.to_string()).await? {
        FolderState::NoRepo => {}
        FolderState::Missing => return Err(AppError::NotFound(format!("{} is not on this computer.", path))),
        FolderState::Repo => return Err(AppError::Config(format!("{} is already a git repository.", path))),
        FolderState::InsideRepo { top } => {
            return Err(AppError::Config(format!("{} is inside the repository {}, so it can't be a repository of its own.", path, top)));
        }
        FolderState::ContainsRepos { count } => {
            let (n, it) = if count == 1 { ("1 repository".to_string(), "it") } else { (format!("{} repositories", count), "them") };
            return Err(AppError::Config(format!("{} already holds {}; making the folder around {} a repository would nest {} inside another.", path, n, it, it)));
        }
    }
    let configured = GitCmd::at(path).args(["config", "--get", "init.defaultBranch"]).ok_text().await.filter(|b| !b.trim().is_empty());
    let mut args = vec!["init", "--quiet"];
    if configured.is_none() {
        args.extend(["--initial-branch", "main"]);
    }
    let out = GitCmd::at(path).args(args).timeout(LOCAL_TIMEOUT).run().await?;
    if !out.ok() {
        return Err(AppError::Command(format!("git init failed: {}", out.stderr.trim())));
    }
    // Measured, not assumed.
    if !std::path::Path::new(path).join(".git").exists() || GitCmd::at(path).args(["rev-parse", "--git-dir"]).ok_text().await.is_none() {
        return Err(AppError::Command(format!("git init said it was done, but {} is still not a repository.", path)));
    }
    let branch = GitCmd::at(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .ok_text()
        .await
        .unwrap_or_else(|| configured.unwrap_or_else(|| "main".into()));
    Ok(InitOutcome { path: path.to_string(), branch })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_staged_rename_is_acted_on_under_both_names_and_a_copy_under_one() {
        let h = "a".repeat(40);
        let raw = format!(
            "2 R. N... 100644 100644 100644 {h} {h} R100 new.txt\0old.txt\x002 C. N... 100644 100644 100644 {h} {h} C75 copy.txt\0src.txt\x001 .M N... 100644 100644 100644 {h} {h} other.txt\0"
        );
        let entries = git_status::parse_porcelain_v2(raw.as_bytes()).entries;
        let sel = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        assert_eq!(renamed_from(&entries, &sel(&["new.txt"])), vec!["old.txt"]);
        assert_eq!(renamed_from(&entries, &sel(&["new.txt", "old.txt"])), vec!["old.txt"], "named once");
        assert!(renamed_from(&entries, &sel(&["copy.txt"])).is_empty(), "a copy's source was not changed by it");
        assert!(renamed_from(&entries, &sel(&["other.txt"])).is_empty());
        assert!(renamed_from(&entries, &sel(&["old.txt"])).is_empty(), "only a selected rename brings its old name");
    }

    /// A throwaway folder with a project in it, no git anywhere above it.
    fn project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gitswitch-init-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app").join("src")).unwrap();
        std::fs::write(dir.join("README.md"), "a project\n").unwrap();
        dir
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = crate::spawn::command("git").arg("-C").arg(dir).args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn a_folder_with_no_repository_becomes_one_and_nothing_else_does() {
        // Only where git finds no repository above the temporary folder.
        if crate::repo_scan::enclosing_repo(&std::env::temp_dir()).is_some() {
            return;
        }
        let dir = project("make");
        let path = dir.to_string_lossy().to_string();
        let made = init_repo(&path).await.unwrap();
        assert!(dir.join(".git").is_dir());
        assert!(!made.branch.is_empty());
        assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]), made.branch);
        assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "a project\n", "the files stay as they were");

        let again = init_repo(&path).await.unwrap_err().to_string();
        assert!(again.contains("already a git repository"), "{again}");
        let inner = dir.join("app").join("src").to_string_lossy().to_string();
        let inside = init_repo(&inner).await.unwrap_err().to_string();
        assert!(inside.contains("inside the repository"), "{inside}");
        assert!(!dir.join("app").join("src").join(".git").exists(), "nothing written");
        let gone = init_repo(&dir.join("nope").to_string_lossy()).await.unwrap_err().to_string();
        assert!(gone.contains("not on this computer"), "{gone}");

        let parent = project("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();
        git(&child, &["init", "--quiet"]);
        let around = init_repo(&parent.to_string_lossy()).await.unwrap_err().to_string();
        assert!(around.contains("already holds 1 repository"), "{around}");
        assert!(!parent.join(".git").exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&parent);
    }

    fn clean() -> StatusFacts {
        StatusFacts {
            identity_matches: true,
            upstream: Some("origin/main".into()),
            can_amend: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_push_is_structurally_incapable_of_forcing_or_deleting() {
        // Every shape the builder can produce, including hostile-looking input.
        for (remote, branch) in [
            ("origin", "main"),
            ("upstream", "feature/x"),
            ("origin", "--force"),
        ] {
            for set_upstream in [true, false] {
                let args = build_push_args(remote, branch, &format!("refs/heads/{}", branch), set_upstream);
                let joined = args.join(" ");
                for forbidden in [
                    "--force",
                    "--force-with-lease",
                    "--delete",
                    "--mirror",
                    "--all",
                    "--tags",
                    "--prune",
                ] {
                    assert!(
                        !args.iter().any(|a| a == forbidden),
                        "{} must never appear: {}",
                        forbidden,
                        joined
                    );
                }
                // A leading '+' in the refspec would make it a force-push.
                let refspec = args.last().unwrap();
                assert!(!refspec.starts_with('+'), "{}", refspec);
                // A refspec with an empty source deletes the remote branch.
                assert!(refspec.starts_with("refs/heads/"), "{}", refspec);
                assert!(refspec.contains(':') && !refspec.ends_with(':'));
                // Anything that looks like an option sits after `--`.
                let dashdash = args.iter().position(|a| a == "--").unwrap();
                assert!(args.iter().position(|a| a == remote).unwrap() > dashdash);
            }
        }
    }

    /// A command whose async work outgrows a thread's stack (2 MiB) crashes
    /// without a word — an Update's Continue once did, in a debug build, when
    /// its nested steps grew. The heavy nested steps are `Box::pin`ned; this
    /// catches a future that grows grossly. (The overflow itself came from how
    /// deep the debug build's poll frames nest, which a size cannot show: the
    /// live harness, which drives every operation for real, is what finds it.)
    #[test]
    fn every_heavy_command_future_stays_far_below_a_threads_stack() {
        let sizes: Vec<(&str, usize)> = vec![
            ("sync_run", std::mem::size_of_val(&crate::sync::sync_run("/x", crate::sync::RunOptions { stash: true, bundles: false, with_lfs: false, fingerprint: Vec::new() }))),
            ("sync_continue", std::mem::size_of_val(&crate::sync::sync_continue("/x", false))),
            ("sync_abort", std::mem::size_of_val(&crate::sync::sync_abort("/x"))),
            ("sync_plan", std::mem::size_of_val(&crate::sync::sync_plan("/x", true, true))),
            ("submodule_update", std::mem::size_of_val(&submodule_update("/x", None))),
            ("pull", std::mem::size_of_val(&pull("/x", PullMode::Rebase, true, true, None))),
            ("push", std::mem::size_of_val(&push("/x", false, false))),
            ("full_clone", std::mem::size_of_val(&crate::sparse::full_clone("u", "/p", None, None, true, true))),
            ("repo_status", std::mem::size_of_val(&crate::git_status::repo_status("/x"))),
        ];
        for (name, size) in &sizes {
            eprintln!("future size {}: {} bytes", name, size);
            assert!(*size < 64 * 1024, "{} builds a {} byte future: Box::pin its heavy nested steps", name, size);
        }
    }

    #[test]
    fn push_sets_upstream_only_when_asked() {
        let with = build_push_args("origin", "main", "refs/heads/main", true);
        assert!(with.iter().any(|a| a == "--set-upstream"));
        let without = build_push_args("origin", "main", "refs/heads/main", false);
        assert!(!without.iter().any(|a| a == "--set-upstream"));
    }

    #[test]
    fn the_refspec_builder_sends_the_branch_wherever_it_is_told() {
        let args = build_push_args("origin", "local-name", "refs/heads/remote-name", false);
        assert_eq!(args.last().unwrap(), "refs/heads/local-name:refs/heads/remote-name");
    }

    fn tracking(branch: &str, remote: &str, merge: &str) -> PushInputs {
        PushInputs {
            branch: branch.into(),
            upstream: Some(format!("{}/{}", remote, merge)),
            branch_remote: Some(remote.into()),
            merge_ref: Some(format!("refs/heads/{}", merge)),
            remotes: vec![remote.into()],
            ..Default::default()
        }
    }

    #[test]
    fn a_push_goes_to_an_upstream_of_the_same_name() {
        let t = resolve_push_target(&tracking("main", "origin", "main")).unwrap();
        assert_eq!(t, PushTarget { remote: "origin".into(), remote_ref: "refs/heads/main".into(), set_upstream: false });
    }

    #[test]
    fn a_branch_started_from_main_with_tracking_is_not_pushed_into_main() {
        // `git switch -c feature origin/main` leaves feature tracking main.
        // Pushing "to the upstream" would land the feature's commits in main.
        let r = resolve_push_target(&tracking("feature", "origin", "main")).unwrap_err();
        assert_eq!(r.code, "upstream-name-differs");
        assert!(r.message.contains("origin/main") && r.message.contains("origin/feature"), "{}", r.message);
        // Publishing it under its own name is the way out, and re-points the upstream.
        let i = PushInputs { set_upstream: true, ..tracking("feature", "origin", "main") };
        let t = resolve_push_target(&i).unwrap();
        assert_eq!(t, PushTarget { remote: "origin".into(), remote_ref: "refs/heads/feature".into(), set_upstream: true });
    }

    #[test]
    fn a_remote_named_with_a_slash_is_read_whole_from_the_config() {
        let mut i = tracking("main", "team/gh", "main");
        i.upstream = Some("team/gh/main".into());
        let t = resolve_push_target(&i).unwrap();
        assert_eq!(t.remote, "team/gh");
    }

    #[test]
    fn a_branch_tracking_a_local_branch_is_refused_and_can_be_published() {
        let i = PushInputs {
            branch: "feature".into(),
            upstream: Some("dev".into()),
            branch_remote: Some(".".into()),
            merge_ref: Some("refs/heads/dev".into()),
            remotes: vec!["origin".into()],
            ..Default::default()
        };
        let r = resolve_push_target(&i).unwrap_err();
        assert_eq!(r.code, "local-upstream");
        assert!(r.message.contains("`dev`") && r.message.contains("origin/feature"), "{}", r.message);
        let t = resolve_push_target(&PushInputs { set_upstream: true, ..i }).unwrap();
        assert_eq!(t, PushTarget { remote: "origin".into(), remote_ref: "refs/heads/feature".into(), set_upstream: true });
    }

    #[test]
    fn without_an_upstream_the_remote_is_the_one_git_would_pick() {
        let base = PushInputs { branch: "b".into(), remotes: vec!["fork".into(), "upstream".into()], ..Default::default() };
        // pushRemote beats everything…
        let i = PushInputs { push_remote: Some("fork".into()), push_default: Some("upstream".into()), ..base.clone() };
        assert_eq!(resolve_push_target(&i).unwrap().remote, "fork");
        // …then remote.pushDefault…
        let i = PushInputs { push_default: Some("upstream".into()), ..base.clone() };
        assert_eq!(resolve_push_target(&i).unwrap().remote, "upstream");
        // …then branch.<b>.remote…
        let i = PushInputs { branch_remote: Some("fork".into()), ..base.clone() };
        assert_eq!(resolve_push_target(&i).unwrap().remote, "fork");
        // …then origin, if there is one…
        let i = PushInputs { remotes: vec!["zeta".into(), "origin".into()], ..base.clone() };
        assert_eq!(resolve_push_target(&i).unwrap().remote, "origin");
        // …then the only remote. Alphabetical order never decides.
        let i = PushInputs { remotes: vec!["zeta".into()], ..base.clone() };
        assert_eq!(resolve_push_target(&i).unwrap().remote, "zeta");
        let r = resolve_push_target(&base).unwrap_err();
        assert_eq!(r.code, "ambiguous-remote");
        assert!(r.message.contains("fork, upstream"), "{}", r.message);
        let r = resolve_push_target(&PushInputs { remotes: vec![], ..base }).unwrap_err();
        assert_eq!(r.code, "no-remote");
        // And publishing always sets the upstream.
        let i = PushInputs { branch: "b".into(), remotes: vec!["origin".into()], ..Default::default() };
        assert!(resolve_push_target(&i).unwrap().set_upstream);
    }

    #[test]
    fn push_default_or_push_remote_wins_over_the_upstreams_remote_and_needs_no_same_name() {
        // Triangular: track upstream's main, push to the fork. git pushes
        // `main -> fork/main` and `feature -> fork/feature` with no complaint.
        let mut i = tracking("main", "origin", "main");
        i.remotes = vec!["origin".into(), "fork".into()];
        i.push_default = Some("fork".into());
        let t = resolve_push_target(&i).unwrap();
        assert_eq!(t, PushTarget { remote: "fork".into(), remote_ref: "refs/heads/main".into(), set_upstream: false });
        let mut f = tracking("feature", "origin", "main");
        f.remotes = vec!["origin".into(), "fork".into()];
        f.push_default = Some("fork".into());
        let t = resolve_push_target(&f).unwrap();
        assert_eq!(t.remote, "fork");
        assert_eq!(t.remote_ref, "refs/heads/feature");
        // branch.<b>.pushRemote beats remote.pushDefault.
        f.push_remote = Some("origin".into());
        let r = resolve_push_target(&f).unwrap_err();
        assert_eq!(r.code, "upstream-name-differs", "back on the upstream's remote, the same-name rule applies again");
    }

    #[test]
    fn a_merge_or_rebase_pull_with_the_wrong_identity_is_refused_but_a_fast_forward_is_not() {
        let f = StatusFacts { identity_matches: false, identity_email: "me@home".into(), profile_email: Some("me@work".into()), ..clean() };
        for mode in [PullMode::Merge, PullMode::Rebase] {
            let r = check(&Intent::Pull { mode, autostash: false }, &f).unwrap();
            assert_eq!(r.code, "identity-mismatch");
            assert!(r.message.contains("me@work") && r.message.contains("me@home"), "{}", r.message);
        }
        assert!(check(&Intent::Pull { mode: PullMode::FfOnly, autostash: false }, &f).is_none());
    }

    #[test]
    fn each_pull_mode_runs_the_command_its_name_promises() {
        let ff = build_integrate_args(PullMode::FfOnly, "refs/remotes/origin/main", false);
        assert_eq!(ff[0], "merge");
        assert!(ff.iter().any(|a| a == "--ff-only"));
        assert!(!ff.iter().any(|a| a == "--rebase"));

        let merge = build_integrate_args(PullMode::Merge, "refs/remotes/origin/main", false);
        assert_eq!(merge[0], "merge");
        assert!(merge.iter().any(|a| a == "--no-edit"));
        assert!(!merge.iter().any(|a| a == "--ff-only"));

        let rebase = build_integrate_args(PullMode::Rebase, "refs/remotes/origin/main", false);
        assert_eq!(rebase[0], "rebase");
        // Autostash would silently move the user's work and can fail on the way back.
        assert!(rebase.iter().any(|a| a == "--no-autostash"));
        // …unless the user asked for it, in which case the failure is reported, never hidden.
        let auto = build_integrate_args(PullMode::Rebase, "refs/remotes/origin/main", true);
        assert!(auto.iter().any(|a| a == "--autostash"));
        assert!(!auto.iter().any(|a| a == "--no-autostash"));

        // All three point at the fully-qualified remote ref.
        for args in [ff, merge, rebase] {
            assert_eq!(args.last().unwrap(), "refs/remotes/origin/main");
        }
    }

    #[test]
    fn pull_modes_parse_from_exactly_three_words() {
        assert_eq!(PullMode::parse("ff-only").unwrap(), PullMode::FfOnly);
        assert_eq!(PullMode::parse("merge").unwrap(), PullMode::Merge);
        assert_eq!(PullMode::parse("rebase").unwrap(), PullMode::Rebase);
        assert!(PullMode::parse("").is_err());
        assert!(PullMode::parse("--hard").is_err());
    }

    #[test]
    fn pathspecs_are_literal_nul_terminated_and_never_empty() {
        let out = pathspec_stdin(&["src/a b.ts".into(), "-weird.txt".into()]).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert_eq!(text, ":(literal)src/a b.ts\0:(literal)-weird.txt\0");
        // A glob must stay a literal filename.
        let star = pathspec_stdin(&["*.md".into()]).unwrap();
        assert_eq!(String::from_utf8(star).unwrap(), ":(literal)*.md\0");
        // Empty selection is an error, never "everything".
        assert!(pathspec_stdin(&[]).is_err());
    }

    #[test]
    fn a_filename_with_a_newline_survives_the_pathspec_encoding() {
        let out = pathspec_stdin(&["weird\nname.txt".into()]).unwrap();
        assert_eq!(out.iter().filter(|b| **b == 0).count(), 1);
        assert!(String::from_utf8(out).unwrap().contains("weird\nname.txt"));
    }

    #[test]
    fn paths_outside_the_worktree_are_rejected() {
        for bad in [
            "/etc/passwd",
            "../outside.txt",
            "a/../../b",
            "~/secret",
            "C:/Windows/x",
            "",
        ] {
            assert!(validate_repo_path(bad).is_err(), "{} should be rejected", bad);
        }
        for good in ["src/app.ts", "a b/c.md", "deep/nested/path.rs", "..dotfile"] {
            assert!(validate_repo_path(good).is_ok(), "{} should be allowed", good);
        }
    }

    #[test]
    fn commit_refuses_with_nothing_staged_and_with_an_empty_message() {
        let f = clean();
        let r = check(
            &Intent::Commit {
                amend: false,
                message_empty: false,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "nothing-staged");

        let staged = StatusFacts { staged: 2, ..clean() };
        let r = check(
            &Intent::Commit {
                amend: false,
                message_empty: true,
            },
            &staged,
        )
        .unwrap();
        assert_eq!(r.code, "empty-message");
    }

    #[test]
    fn a_merge_can_still_be_concluded_by_a_commit() {
        // The case a naive "refuse if any operation is in progress" rule would
        // make impossible.
        let f = StatusFacts {
            staged: 3,
            conflicted: 0,
            operation: Some("merge".into()),
            abort_command: Some("git merge --abort".into()),
            ..clean()
        };
        assert!(check(
            &Intent::Commit {
                amend: false,
                message_empty: false
            },
            &f
        )
        .is_none());

        // And with nothing staged: resolving every conflict in favour of our
        // side leaves the index identical to HEAD, but the merge commit still
        // has to be made — it is what records the second parent.
        let resolved_to_ours = StatusFacts { staged: 0, ..f };
        assert!(check(
            &Intent::Commit {
                amend: false,
                message_empty: false
            },
            &resolved_to_ours
        )
        .is_none());

        // Outside a merge, nothing staged is still a refusal.
        let idle = StatusFacts {
            staged: 0,
            operation: None,
            abort_command: None,
            ..clean()
        };
        assert_eq!(
            check(
                &Intent::Commit {
                    amend: false,
                    message_empty: false
                },
                &idle
            )
            .unwrap()
            .code,
            "nothing-staged"
        );
    }

    #[test]
    fn commit_refuses_while_conflicts_remain() {
        let f = StatusFacts {
            staged: 1,
            conflicted: 2,
            ..clean()
        };
        let r = check(
            &Intent::Commit {
                amend: false,
                message_empty: false,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "unmerged-paths");
    }

    #[test]
    fn commit_refuses_when_the_identity_is_wrong_and_says_where_it_came_from() {
        let f = StatusFacts {
            staged: 1,
            identity_matches: false,
            identity_email: "personal@me.com".into(),
            profile_email: Some("work@corp.com".into()),
            email_scope: "local".into(),
            ..clean()
        };
        let r = check(
            &Intent::Commit {
                amend: false,
                message_empty: false,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "identity-mismatch");
        assert!(r.message.contains("work@corp.com"));
        assert!(r.message.contains("personal@me.com"));
        assert!(r.message.contains(".git/config"));
    }

    #[test]
    fn amending_a_published_commit_is_refused() {
        let f = StatusFacts {
            staged: 1,
            can_amend: false,
            ..clean()
        };
        let r = check(
            &Intent::Commit {
                amend: true,
                message_empty: false,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "already-pushed");
        assert!(r.message.contains("force-push"));
        // Amend with nothing staged is fine — that's how you fix a message.
        let f2 = StatusFacts { staged: 0, ..clean() };
        assert!(check(
            &Intent::Commit {
                amend: true,
                message_empty: false
            },
            &f2
        )
        .is_none());
    }

    #[test]
    fn a_locked_push_is_refused_by_its_own_code() {
        let r = check(
            &Intent::Push,
            &StatusFacts {
                push_blocked: true,
                push_locked: true,
                push_reason: "Locked for this repository. Unlocking needs the administrator password.".into(),
                ..Default::default()
            },
        )
        .expect("refused");
        assert_eq!(r.code, "push-locked");
        assert!(r.message.contains("administrator"));
    }

    #[test]
    fn a_blocked_push_never_reaches_git() {
        let f = StatusFacts {
            push_blocked: true,
            push_reason: "Blocked by profile 'Work'.".into(),
            ..clean()
        };
        let r = check(&Intent::Push, &f).unwrap();
        assert_eq!(r.code, "push-blocked");
        assert!(r.message.contains("Work"));
    }

    #[test]
    fn push_refuses_on_a_detached_or_unborn_head() {
        let d = StatusFacts { detached: true, ..clean() };
        assert_eq!(check(&Intent::Push, &d).unwrap().code, "detached-head");
        let u = StatusFacts { unborn: true, ..clean() };
        assert_eq!(check(&Intent::Push, &u).unwrap().code, "unborn-head");
    }

    #[test]
    fn rebase_needs_a_clean_tree_but_fast_forward_does_not() {
        let dirty = StatusFacts { unstaged: 2, ..clean() };
        assert_eq!(
            check(&Intent::Pull { mode: PullMode::Rebase, autostash: false }, &dirty).unwrap().code,
            "dirty-tree"
        );
        // Let git decide for the other two — it refuses only if files collide.
        assert!(check(&Intent::Pull { mode: PullMode::FfOnly, autostash: false }, &dirty).is_none());
        assert!(check(&Intent::Pull { mode: PullMode::Merge, autostash: false }, &dirty).is_none());
    }

    #[test]
    fn pull_refuses_without_an_upstream() {
        let f = StatusFacts { upstream: None, ..clean() };
        assert_eq!(
            check(&Intent::Pull { mode: PullMode::Merge, autostash: false }, &f).unwrap().code,
            "no-upstream"
        );
    }

    #[test]
    fn every_operation_stops_while_a_rebase_is_in_progress() {
        let f = StatusFacts {
            operation: Some("rebase".into()),
            abort_command: Some("git rebase --abort".into()),
            staged: 1,
            ..clean()
        };
        for intent in [
            Intent::Push,
            Intent::Pull { mode: PullMode::Merge, autostash: false },
            Intent::Submodule,
            Intent::Sync,
            Intent::Tree(crate::tree::TreeIntent::UndoCommit { head_is_root: false, head_is_merge: false }),
            Intent::Tree(crate::tree::TreeIntent::Switch { detached_unique: 0 }),
            Intent::Tree(crate::tree::TreeIntent::DiscardAll { include_untracked: false, stash_first: false }),
            Intent::Stash(crate::stash::StashIntent::Push { include_untracked: false, has_paths: false }),
            Intent::Stash(crate::stash::StashIntent::Apply { exists: true, pop: true }),
        ] {
            let r = check(&intent, &f).unwrap();
            assert_eq!(r.code, "operation-in-progress", "{:?}", intent);
            assert!(r.message.contains("git rebase --abort"));
        }
        // Resolving a conflict is exactly the work an operation in progress needs.
        assert!(check(&Intent::Tree(crate::tree::TreeIntent::ResolveSide { has_paths: true, not_conflicted_selected: false }), &f).is_none());
    }

    #[test]
    fn autostash_lets_a_dirty_tree_rebase_and_nothing_else_changes() {
        let dirty = StatusFacts { unstaged: 1, ..clean() };
        assert_eq!(check(&Intent::Pull { mode: PullMode::Rebase, autostash: false }, &dirty).unwrap().code, "dirty-tree");
        assert!(check(&Intent::Pull { mode: PullMode::Rebase, autostash: true }, &dirty).is_none());
        assert_eq!(check(&Intent::Pull { mode: PullMode::Rebase, autostash: true }, &StatusFacts { upstream: None, ..dirty.clone() }).unwrap().code, "no-upstream");
    }

    #[test]
    fn a_paused_sync_blocks_everything_but_resolution_and_its_own_buttons() {
        let f = StatusFacts { sync_in_progress: true, operation: Some("rebase".into()), abort_command: Some("git rebase --abort".into()), continue_command: Some("git rebase --continue".into()), ..clean() };
        for intent in [
            Intent::Push,
            Intent::Pull { mode: PullMode::Merge, autostash: false },
            Intent::Commit { amend: false, message_empty: false },
            Intent::Abort,
            Intent::Continue,
            Intent::Sync,
            Intent::Submodule,
            Intent::Tree(crate::tree::TreeIntent::UndoCommit { head_is_root: false, head_is_merge: false }),
            Intent::Tree(crate::tree::TreeIntent::Reset { mode: crate::tree::ResetMode::Hard, target_valid: true, target_is_head: false, drops_pushed: false, stash_first: false }),
            Intent::Tree(crate::tree::TreeIntent::DiscardAll { include_untracked: false, stash_first: false }),
            Intent::Stash(crate::stash::StashIntent::Push { include_untracked: false, has_paths: false }),
            Intent::Stash(crate::stash::StashIntent::Drop { exists: true }),
        ] {
            assert_eq!(check(&intent, &f).unwrap().code, "sync-in-progress", "{:?}", intent);
        }
        assert!(check(&Intent::Stage, &f).is_none());
        // Resolution work is allowed: taking a side, and recovering a file from the autostash.
        assert!(check(&Intent::Tree(crate::tree::TreeIntent::ResolveSide { has_paths: true, not_conflicted_selected: false }), &f).is_none());
        assert!(check(&Intent::Stash(crate::stash::StashIntent::RestoreFile { exists: true, in_stash: true }), &f).is_none());
        assert!(check(&Intent::SyncContinue, &f).is_none());
        assert!(check(&Intent::SyncAbort, &f).is_none());
        assert_eq!(check(&Intent::SyncContinue, &clean()).unwrap().code, "no-sync");
        // Sync itself needs a branch with an upstream and no operation, but not a clean tree.
        assert!(check(&Intent::Sync, &StatusFacts { staged: 3, unstaged: 2, ..clean() }).is_none());
        assert_eq!(check(&Intent::Sync, &StatusFacts { upstream: None, ..clean() }).unwrap().code, "no-upstream");
        assert_eq!(check(&Intent::Sync, &StatusFacts { detached: true, ..clean() }).unwrap().code, "detached-head");
    }

    #[test]
    fn continue_needs_an_operation_a_continue_command_and_no_conflicts() {
        assert_eq!(check(&Intent::Continue, &clean()).unwrap().code, "nothing-to-continue");
    }

    #[test]
    fn staged_blobs_are_read_from_the_raw_diff_and_the_batch() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let z = "0".repeat(40);
        // Two files, a gitlink and a symlink: only the files have lines.
        let raw = format!(
            ":100644 100644 {z} {a} M\0src/one.txt\0:000000 100755 {z} {b} A\0with space.sh\0:160000 160000 {z} {a} M\0sub\0:000000 120000 {z} {b} A\0link\0"
        );
        let staged = parse_raw_blobs(raw.as_bytes());
        assert_eq!(staged, vec![("src/one.txt".to_string(), a.clone()), ("with space.sh".to_string(), b.clone())]);
        // A path that stops being a symlink and becomes a file has lines to read; the reverse has none.
        let types = format!(":120000 100644 {z} {a} T\0was-a-link\0:100644 120000 {z} {b} T\0now-a-link\0");
        assert_eq!(parse_raw_blobs(types.as_bytes()), vec![("was-a-link".to_string(), a.clone())]);
        // The batch: a clean file, a missing object, then one with a marker — at a width its attribute sets.
        let clean = "fine\n";
        let marked = "x\n<<<<< HEAD\ny\n";
        let list = vec![("clean.txt".to_string(), a.clone()), ("gone.txt".to_string(), z.clone()), ("narrow.txt".to_string(), b.clone())];
        let out = format!("{a} blob {}\n{clean}\n{z} missing\n{b} blob {}\n{marked}\n", clean.len(), marked.len());
        let mut sizes = std::collections::BTreeMap::new();
        assert_eq!(first_marker_offence(out.as_bytes(), &list, &sizes), None, "five is not a marker at the default width");
        sizes.insert("narrow.txt".to_string(), 5);
        assert_eq!(first_marker_offence(out.as_bytes(), &list, &sizes), Some(("narrow.txt".to_string(), vec![2])));
        // A bare CRLF marker is one; a binary blob is skipped; a cut-off batch is no answer.
        let crlf = "<<<<<<<\r\nmine\r\n=======\r\n";
        let bin = "\0\0<<<<<<< HEAD\n";
        let list = vec![("bin.dat".to_string(), a.clone()), ("crlf.txt".to_string(), b.clone())];
        let out = format!("{a} blob {}\n{bin}\n{b} blob {}\n{crlf}\n", bin.len(), crlf.len());
        assert_eq!(first_marker_offence(out.as_bytes(), &list, &std::collections::BTreeMap::new()), Some(("crlf.txt".to_string(), vec![1])));
        assert_eq!(first_marker_offence(&out.as_bytes()[..60], &list, &std::collections::BTreeMap::new()), None);
    }

    #[test]
    fn windows_double_click_types_count_as_programs_only_there() {
        let p = std::path::Path::new("tools/run.js");
        assert_eq!(opens_by_running(p), cfg!(windows));
        assert!(opens_by_running(std::path::Path::new("Build.app")), "a bundle is a program even as a directory");
        assert!(opens_by_running(std::path::Path::new("setup.Command")));
        assert!(!opens_by_running(std::path::Path::new("README.md")));
        // What "open" runs on every platform, in any case.
        for name in ["a.terminal", "a.action", "a.ksh", "a.csh", "a.AppImage", "A.APP"] {
            assert!(opens_by_running(std::path::Path::new(name)), "{name}");
        }
        // What Windows runs on a double-click, and nothing else does.
        for name in ["a.vbs", "a.hta", "a.lnk", "a.reg", "a.py", "a.scr", "a.appref-ms", "A.JS"] {
            assert_eq!(opens_by_running(std::path::Path::new(name)), cfg!(windows), "{name}");
        }
    }

    #[test]
    fn a_path_goes_to_the_shell_without_the_verbatim_prefix() {
        // An ordinary path is handed over as it is, on every platform.
        assert_eq!(shell_path(std::path::Path::new("/repos/r/a.txt")), PathBuf::from("/repos/r/a.txt"));
        #[cfg(windows)]
        {
            assert_eq!(shell_path(std::path::Path::new(r"\\?\C:\r\a.txt")), PathBuf::from(r"C:\r\a.txt"));
            assert_eq!(shell_path(std::path::Path::new(r"\\?\UNC\srv\share\a.txt")), PathBuf::from(r"\\srv\share\a.txt"));
            assert_eq!(shell_path(std::path::Path::new(r"C:\r\a.txt")), PathBuf::from(r"C:\r\a.txt"));
        }
    }

    #[test]
    fn programs_are_revealed_never_opened() {
        let dir = std::env::temp_dir().join(format!("gitswitch-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("notes.txt");
        std::fs::write(&text, "hello").unwrap();
        assert!(!opens_by_running(&text));
        assert!(opens_by_running(&dir.join("Installer.pkg")));
        assert!(opens_by_running(&dir.join("run.command")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let exe = dir.join("tool");
            std::fs::write(&exe, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(opens_by_running(&exe));
        }
        assert!(!opens_by_running(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conflict_markers_are_found_by_opener_and_orphan_close() {
        let text = "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> theirs\nb\n<<<<<<< ours\nunfinished\n";
        assert_eq!(conflict_marker_lines(text, 7), vec![2, 8], "a half-done hunk is still a marker");
        assert!(conflict_marker_lines("=======\n<<<<<<<<\nnot markers\n", 7).is_empty(), "a bare separator is a heading; eight is not a marker");
        assert_eq!(conflict_marker_lines(">>>>>>> left over\n", 7), vec![1], "an orphan close is a leftover");
        assert_eq!(conflict_marker_lines("<<<<<<<<<< HEAD\nx\n>>>>>>>>>> theirs\n", 10), vec![1], "the width follows conflict-marker-size");
        assert!(conflict_marker_lines("<<<<<<<<<< HEAD\n", 7).is_empty());
        assert!(conflict_marker_lines("clean\n", 7).is_empty());
        let r = markers_refusal("shared.txt", &[3, 12]);
        assert_eq!(r.code, "conflict-markers");
        assert!(r.message.contains("shared.txt still contains conflict markers (lines 3, 12)"));
        let many: Vec<usize> = (1..=20).collect();
        assert!(markers_refusal("big.txt", &many).message.contains("12 and 8 more"), "{}", markers_refusal("big.txt", &many).message);
        let bisect = StatusFacts {
            operation: Some("bisect".into()),
            abort_command: Some("git bisect reset".into()),
            continue_command: None,
            ..clean()
        };
        assert_eq!(check(&Intent::Continue, &bisect).unwrap().code, "cannot-continue");
        let conflicted = StatusFacts {
            operation: Some("rebase".into()),
            abort_command: Some("git rebase --abort".into()),
            continue_command: Some("git rebase --continue".into()),
            conflicted: 2,
            ..clean()
        };
        let r = check(&Intent::Continue, &conflicted).unwrap();
        assert_eq!(r.code, "unmerged-paths");
        assert!(r.message.contains("2 files"));
        let ready = StatusFacts { conflicted: 0, ..conflicted };
        assert!(check(&Intent::Continue, &ready).is_none(), "a rebase with everything staged may continue");
    }

    #[test]
    fn unstaging_a_conflicted_file_is_refused() {
        let f = StatusFacts { conflicted: 1, ..clean() };
        let r = check(
            &Intent::Unstage {
                conflicted_selected: true,
            },
            &f,
        )
        .unwrap();
        assert_eq!(r.code, "unmerged-paths");
        assert!(check(
            &Intent::Unstage {
                conflicted_selected: false
            },
            &f
        )
        .is_none());
    }

    #[test]
    fn discard_refuses_without_paths_and_inside_submodules() {
        let f = clean();
        assert_eq!(
            check(
                &Intent::Discard {
                    has_paths: false,
                    submodule_selected: false,
                    conflicted_selected: false
                },
                &f
            )
            .unwrap()
            .code,
            "no-paths"
        );
        assert_eq!(
            check(
                &Intent::Discard {
                    has_paths: true,
                    submodule_selected: true,
                    conflicted_selected: false
                },
                &f
            )
            .unwrap()
            .code,
            "submodule-selected"
        );
        // Inside a merge the stage entries are the conflict itself…
        let merging = StatusFacts { operation: Some("merge".into()), abort_command: Some("git merge --abort".into()), ..f.clone() };
        assert_eq!(
            check(
                &Intent::Discard {
                    has_paths: true,
                    submodule_selected: false,
                    conflicted_selected: true
                },
                &merging
            )
            .unwrap()
            .code,
            "unmerged-paths"
        );
        // …but a stash that failed to re-apply leaves conflicts with no
        // operation, and restoring the file to HEAD is the way out.
        assert!(check(
            &Intent::Discard { has_paths: true, submodule_selected: false, conflicted_selected: true },
            &f
        )
        .is_none());
    }

    #[test]
    fn abort_is_refused_when_nothing_is_in_progress() {
        assert_eq!(
            check(&Intent::Abort, &clean()).unwrap().code,
            "nothing-to-abort"
        );
        let f = StatusFacts {
            operation: Some("merge".into()),
            abort_command: Some("git merge --abort".into()),
            ..clean()
        };
        assert!(check(&Intent::Abort, &f).is_none());
    }

    #[test]
    fn push_porcelain_is_parsed_into_refs() {
        let out = "To github.com:o/r.git\n \trefs/heads/main:refs/heads/main\t1a2b3c..4d5e6f\nDone";
        let refs = parse_push_porcelain(out);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].flag, " ");
        assert_eq!(refs[0].to, "refs/heads/main");
        assert_eq!(refs[0].summary, "1a2b3c..4d5e6f");

        let new_branch = "To github.com:o/r.git\n*\trefs/heads/feat:refs/heads/feat\t[new branch]\nDone";
        assert_eq!(parse_push_porcelain(new_branch)[0].flag, "*");

        let up_to_date = "To github.com:o/r.git\n=\trefs/heads/main:refs/heads/main\t[up to date]\nDone";
        let r = parse_push_porcelain(up_to_date);
        assert!(r.iter().all(|x| x.flag == "="));
    }

    #[test]
    fn shortstat_numbers_are_read_back() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 45 insertions(+), 2 deletions(-)"),
            (3, 45, 2)
        );
        assert_eq!(parse_shortstat(" 1 file changed, 1 insertion(+)"), (1, 1, 0));
        assert_eq!(parse_shortstat(""), (0, 0, 0));
    }

    #[test]
    fn signature_codes_map_to_words() {
        assert_eq!(signature_label("G"), "good");
        assert_eq!(signature_label("B"), "bad");
        assert_eq!(signature_label("N"), "unsigned");
        assert_eq!(signature_label(""), "unsigned");
    }
}

#[cfg(test)]
mod remote_url_tests {
    use super::remote_url_problem;

    #[test]
    fn a_publish_address_is_https_ssh_or_a_full_path_and_nothing_git_could_misread() {
        for ok in [
            "git@github.com:you/subtitlemaker.git",
            "https://github.com/you/subtitlemaker.git",
            "https://github.com/you/subtitlemaker",
            "ssh://git@github.com/you/subtitlemaker.git",
            "ssh://git@example.com:2222/srv/repo.git",
            "org-302118749@github.com:Acme/app.git",
            "C:\\Users\\me\\remotes\\app.git",
            "D:/Shared Repos/app.git",
            "\\\\server\\share\\app.git",
            "/srv/git/app.git",
            "file:///srv/git/app.git",
            "  git@github.com:you/app.git  ",
        ] {
            assert_eq!(remote_url_problem(ok), None, "{ok} should be accepted");
        }
        for (bad, says) in [
            ("", "Paste"),
            ("ext::sh -c touch% /tmp/pwned", "transport helpers"),
            ("--upload-pack=touch x", "isn't a repository address"),
            ("-oProxyCommand=calc:x", "isn't a repository address"),
            ("git@-oProxyCommand=calc:x", "doesn't look like"),
            ("ssh://-oProxyCommand=calc/x", "server and a repository"),
            ("ssh://git@github.com/-x", "server and a repository"),
            ("http://github.com/you/app.git", "https://"),
            ("https://user:ghp_secret@github.com/you/app.git", "token"),
            ("https://github.com", "server and a repository"),
            ("file://", "path of a folder"),
            ("github.com/you/app", "doesn't look like"),
            ("app.git", "doesn't look like"),
            ("C:app.git", "doesn't look like"),
            ("git@github.com:you/app\n.git", "invisible"),
            ("https://github.com/you/my app.git", "space"),
        ] {
            let p = remote_url_problem(bad).unwrap_or_else(|| panic!("{bad:?} should be refused"));
            assert!(p.contains(says), "{bad:?}: {p}");
        }
    }
}

#[cfg(test)]
mod remote_check_tests {
    use super::{not_empty, retarget_url};

    #[test]
    fn a_new_address_keeps_how_the_remote_reached_github() {
        assert_eq!(retarget_url("git@github.com:me/old.git", "me", "new").as_deref(), Some("git@github.com:me/new.git"));
        assert_eq!(retarget_url("org-123@github.com:Acme/old.git", "Acme", "new").as_deref(), Some("org-123@github.com:Acme/new.git"));
        assert_eq!(retarget_url("git@github.com-work:me/old.git", "me", "new").as_deref(), Some("git@github.com-work:me/new.git"));
        assert_eq!(retarget_url("ssh://git@ssh.github.com:443/me/old.git", "me", "new").as_deref(), Some("ssh://git@ssh.github.com:443/me/new.git"));
        assert_eq!(retarget_url("https://tok@github.com/me/old.git", "me", "new").as_deref(), Some("https://github.com/me/new.git"));
        assert_eq!(retarget_url("C:/repos/old.git", "me", "new"), None);
        assert_eq!(retarget_url("/srv/old.git", "me", "new"), None);
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_repository_with_other_branches_is_offered_only_on_the_persons_word() {
        let r = not_empty(&names(&["dev"]), "main");
        assert!(!r.saved && r.existing_branches == names(&["dev"]), "{r:?}");
        assert!(r.problem.contains("a branch (dev)") && r.problem.contains("puts main there beside it") && r.problem.contains("publish there anyway"), "{}", r.problem);
        let many = not_empty(&names(&["a", "b", "c", "d", "e"]), "main");
        assert!(many.problem.contains("5 branches (a, b, c, and 2 more)") && many.problem.contains("beside them"), "{}", many.problem);
    }

    #[test]
    fn a_repository_that_has_this_branch_already_is_refused_with_nothing_to_offer() {
        let r = not_empty(&names(&["main"]), "main");
        assert!(r.existing_branches.is_empty() && r.problem.contains("a branch main with commits of its own") && r.problem.contains("would be refused"), "{r:?}");
        let r = not_empty(&names(&["dev", "main", "x"]), "main");
        assert!(r.existing_branches.is_empty() && r.problem.contains("a branch main (and 2 more)"), "{}", r.problem);
    }
}
