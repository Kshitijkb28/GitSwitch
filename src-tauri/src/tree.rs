//! Tree management: branches, undo, reset, detach, revert, cherry-pick,
//! conflict sides, discard-all and commit lookup.
//!
//! Every operation follows the four phases of `git_ops` (snapshot → pure
//! `check` → one explicit git command → verify by re-reading). Anything that
//! moves HEAD or a branch away from commits makes a backup first and reports
//! the exact command that undoes it. Nothing here ever touches a remote.

use crate::error::AppError;
use crate::git_advice::{self, GitOp};
use crate::git_exec::{validate_branch_name, GitCmd, LOCAL_TIMEOUT, NET_TIMEOUT};
use crate::git_ops::{advice_ctx, busy, check, pathspec_stdin, refuse, snapshot, Intent, OpResult, Refusal, StatusFacts};
use crate::git_status::RepoStatus;
use crate::stash::StashRef;
use crate::sync::{self, BackupRef};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const RESET_BACKUP_PREFIX: &str = "gitswitch-before-reset-";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

impl ResetMode {
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s {
            "soft" => Ok(ResetMode::Soft),
            "mixed" => Ok(ResetMode::Mixed),
            "hard" => Ok(ResetMode::Hard),
            other => Err(AppError::Command(format!("unknown reset mode: {}", other))),
        }
    }
    pub fn flag(&self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            ResetMode::Soft => "soft",
            ResetMode::Mixed => "mixed",
            ResetMode::Hard => "hard",
        }
    }
}

/// Which side of a conflict the user means — in their words, never git's
/// (`--ours`/`--theirs` swap meaning during a rebase).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Mine,
    Theirs,
}

impl Side {
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s {
            "mine" => Ok(Side::Mine),
            "theirs" => Ok(Side::Theirs),
            other => Err(AppError::Command(format!("unknown side: {}", other))),
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Side::Mine => "Keep mine",
            Side::Theirs => "Take theirs",
        }
    }
}

/// What a tree operation is asking for. Facts that need an extra git read
/// (target validity, ancestry, merge-ness) are computed by the operation and
/// carried here, so `check_tree` stays pure and unit-testable.
#[derive(Debug, Clone, PartialEq)]
pub enum TreeIntent {
    Switch { detached_unique: usize },
    CreateBranch { switch_to: bool, from_valid: bool, detached_unique: usize },
    DeleteBranch { is_current: bool, exists: bool, unique_commits: usize, force: bool },
    RenameBranch { exists: bool },
    UndoCommit { head_is_root: bool, head_is_merge: bool },
    Reset { mode: ResetMode, target_valid: bool, target_is_head: bool, drops_pushed: bool, stash_first: bool },
    Detach { target_valid: bool, detached_unique: usize },
    Revert { target_valid: bool, is_merge: bool, mainline_given: bool },
    CherryPick { target_valid: bool, is_merge: bool, already_contained: bool },
    ResolveSide { has_paths: bool, submodule_selected: bool, not_conflicted_selected: bool },
    DiscardAll { include_untracked: bool, stash_first: bool },
}

impl TreeIntent {
    /// Resolution work is allowed while a sync is paused; everything else
    /// would fight it.
    pub fn allowed_during_sync(&self) -> bool {
        matches!(self, TreeIntent::ResolveSide { .. })
    }

    fn verb(&self) -> &'static str {
        match self {
            TreeIntent::Switch { .. } => "switch branches",
            TreeIntent::CreateBranch { .. } => "create a branch",
            TreeIntent::DeleteBranch { .. } => "delete a branch",
            TreeIntent::RenameBranch { .. } => "rename a branch",
            TreeIntent::UndoCommit { .. } => "undo a commit",
            TreeIntent::Reset { .. } => "reset",
            TreeIntent::Detach { .. } => "check out a commit",
            TreeIntent::Revert { .. } => "revert",
            TreeIntent::CherryPick { .. } => "cherry-pick",
            TreeIntent::ResolveSide { .. } => "resolve",
            TreeIntent::DiscardAll { .. } => "discard everything",
        }
    }

    /// The word for what the operation makes, in identity refusals.
    fn makes_a_commit(&self) -> &'static str {
        match self {
            TreeIntent::Revert { .. } => "revert",
            TreeIntent::CherryPick { .. } => "cherry-pick",
            _ => "commit",
        }
    }
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

fn conflicts_left(n: usize) -> String {
    format!(
        "{} file{} still {} conflicts. Resolve or discard {} first.",
        n,
        plural(n, "", "s"),
        plural(n, "has", "have"),
        plural(n, "it", "them")
    )
}

fn detached_commits(n: usize) -> Refusal {
    refuse(
        "detached-commits",
        format!(
            "You're not on a branch and have {} commit(s) here that no branch holds. Start a branch here first, or they'd be left behind.",
            n
        ),
    )
}

fn identity_mismatch(f: &StatusFacts, makes: &str) -> Refusal {
    let expected = f.profile_email.clone().unwrap_or_default();
    let where_from = if f.email_scope == "local" {
        " This repo's own .git/config overrides the profile."
    } else {
        ""
    };
    refuse(
        "identity-mismatch",
        format!(
            "This folder should commit as {} but git is set to {}.{} Fix the identity first — a {} makes a commit.",
            expected, f.identity_email, where_from, makes
        ),
    )
}

/// The sentence for `unmerged-branch`; the operation knows the name, the pure
/// check only the count.
pub fn unmerged_branch_message(name: &str, n: usize) -> String {
    format!(
        "Deleting `{}` would drop {} commit(s) that no other branch holds. Confirm to delete anyway — the remote branch, if any, is never touched.",
        name, n
    )
}

/// The pure gate for every tree operation. Order: busy → head state → target
/// validity → data-loss rules → tree state → names. Names (`invalid-name`,
/// `no-such-branch`, `current-branch`) come last so a typo never hides a more
/// fundamental blocker; the operations validate them after this returns None.
pub fn check_tree(intent: &TreeIntent, f: &StatusFacts) -> Option<Refusal> {
    use TreeIntent::*;

    // Conflict resolution is exactly the work an operation in progress needs.
    if let ResolveSide { has_paths, submodule_selected, not_conflicted_selected } = intent {
        if !*has_paths {
            return Some(refuse("no-paths", "Nothing selected. Pick the conflicted files to resolve."));
        }
        if *submodule_selected {
            return Some(refuse(
                "submodule-conflict",
                "Submodule pointer conflicts are resolved by bringing the submodule to the right commit — Sync does that, or do it inside the submodule.",
            ));
        }
        if *not_conflicted_selected {
            return Some(refuse("not-conflicted", "That file isn't in conflict — the list may be out of date."));
        }
        return None;
    }

    // --- busy ---
    match intent {
        // `git branch <name> <commit>` writes one ref and touches nothing a
        // rebase is using.
        CreateBranch { switch_to: false, .. } => {}
        DiscardAll { .. } => {
            if let Some(kind) = &f.operation {
                return Some(refuse(
                    "operation-in-progress",
                    format!(
                        "There's a {} in progress. Abort the {} instead — that undoes all of it ({}).",
                        kind,
                        kind,
                        f.abort_command.clone().unwrap_or_default()
                    ),
                ));
            }
        }
        _ => {
            if let Some(r) = busy(f, intent.verb()) {
                return Some(r);
            }
        }
    }

    // --- head state ---
    if f.unborn {
        let msg = match intent {
            CreateBranch { from_valid: false, .. } => Some("There are no commits yet — a branch needs a commit to start from."),
            UndoCommit { .. } => Some("There are no commits yet — nothing to undo."),
            Reset { .. } => Some("There are no commits yet — nothing to reset."),
            Revert { .. } => Some("There are no commits yet — nothing to revert."),
            CherryPick { .. } => Some("There are no commits yet. Make the first commit before picking others onto it."),
            _ => None,
        };
        if let Some(m) = msg {
            return Some(refuse("unborn-head", m));
        }
    }
    if f.detached {
        match intent {
            Revert { .. } => return Some(refuse("detached-head", "Start a branch here first — a revert makes a commit.")),
            CherryPick { .. } => return Some(refuse("detached-head", "Start a branch here first — a cherry-pick makes a commit.")),
            _ => {}
        }
    }

    // --- target validity ---
    let target_valid = match intent {
        CreateBranch { from_valid, .. } => *from_valid,
        Reset { target_valid, .. } | Detach { target_valid, .. } | Revert { target_valid, .. } | CherryPick { target_valid, .. } => *target_valid,
        _ => true,
    };
    if !target_valid {
        return Some(refuse("not-a-commit", "That isn't a commit in this repository."));
    }
    if let DeleteBranch { exists: false, .. } | RenameBranch { exists: false } = intent {
        // A branch that doesn't exist has nothing below to check.
        return Some(refuse("no-such-branch", "That branch doesn't exist here."));
    }
    if let DeleteBranch { is_current: true, .. } = intent {
        return Some(refuse("current-branch", "That's the branch you're on. Switch to another one first."));
    }

    // --- data-loss rules ---
    match intent {
        UndoCommit { head_is_root, head_is_merge } => {
            // can_amend is false exactly when HEAD is on a remote (busy and
            // unborn were excluded above).
            if !f.can_amend {
                return Some(refuse(
                    "already-pushed",
                    "That commit is already on the remote. Undoing it would need a force-push, which GitSwitch never does — Revert it instead.",
                ));
            }
            if *head_is_root {
                return Some(refuse("root-commit", "This is the first commit; there's nothing before it to go back to."));
            }
            if *head_is_merge {
                return Some(refuse(
                    "merge-commit",
                    "That's a merge commit. Undoing it would turn the whole merge into staged changes. Reset the branch to its first parent instead.",
                ));
            }
        }
        Reset { drops_pushed: true, .. } => {
            return Some(refuse(
                "would-drop-pushed",
                // The pure gate does not know which remote branch holds them;
                // reset_to replaces this with the branch names it measured.
                "Commits already on a remote branch would be dropped, and GitSwitch never force-pushes. Look at that commit (detached), start a branch there, or Revert instead.".to_string(),
            ));
        }
        DeleteBranch { unique_commits, force: false, .. } if *unique_commits > 0 => {
            return Some(refuse("unmerged-branch", unmerged_branch_message("this branch", *unique_commits)));
        }
        Switch { detached_unique } | Detach { detached_unique, .. } if f.detached && *detached_unique > 0 => {
            return Some(detached_commits(*detached_unique));
        }
        CreateBranch { switch_to: true, detached_unique, .. } if f.detached && *detached_unique > 0 => {
            return Some(detached_commits(*detached_unique));
        }
        CherryPick { already_contained: true, .. } => {
            return Some(refuse("already-contained", "That commit is already part of this branch."));
        }
        CherryPick { is_merge: true, .. } => {
            return Some(refuse(
                "merge-commit",
                "Cherry-picking a merge commit needs a mainline and is rarely what you mean — pick the commits it brought in instead.",
            ));
        }
        _ => {}
    }

    // --- tree state ---
    if f.conflicted > 0 {
        match intent {
            Switch { .. } | CreateBranch { switch_to: true, .. } | Detach { .. } => {
                return Some(refuse("unmerged-paths", "Resolve or discard the conflicted files before switching."));
            }
            UndoCommit { .. } | Reset { .. } | Revert { .. } | CherryPick { .. } => {
                return Some(refuse("unmerged-paths", conflicts_left(f.conflicted)));
            }
            _ => {}
        }
    }
    // A hard reset on a dirty tree without `stash_first` is not refused: the
    // caller's dialog has already said the changes are thrown away, and that
    // explicit choice is honoured — exactly as "Discard everything" is.
    match intent {
        Reset { mode, target_is_head: true, .. } if *mode != ResetMode::Hard || f.staged + f.unstaged == 0 => {
            return Some(refuse("nothing-to-do", "The branch is already at that commit."));
        }
        Revert { .. } | CherryPick { .. } if f.staged > 0 => {
            return Some(refuse(
                "staged-changes",
                format!(
                    "You have staged changes. {} needs the staging area to match the last commit — commit or unstage them first.",
                    if matches!(intent, Revert { .. }) { "Revert" } else { "Cherry-pick" }
                ),
            ));
        }
        Revert { .. } | CherryPick { .. } if !f.identity_matches => {
            return Some(identity_mismatch(f, intent.makes_a_commit()));
        }
        Revert { is_merge: true, mainline_given: false, .. } => {
            return Some(refuse("merge-commit-needs-mainline", "That's a merge commit. Choose which side to keep."));
        }
        DiscardAll { include_untracked, .. } => {
            let untracked = if *include_untracked { f.untracked } else { 0 };
            if f.staged + f.unstaged + f.conflicted + untracked == 0 {
                return Some(refuse("nothing-to-discard", "There's nothing to discard — the working tree is clean."));
            }
        }
        _ => {}
    }
    None
}

/// Which git flag takes the side the user means. During a rebase (and `am`,
/// and a paused sync, which is a rebase) git replays *your* commit onto the
/// upstream, so "ours" is the upstream and "theirs" is your work.
pub fn side_flag(op_kind: Option<&str>, side: Side) -> &'static str {
    let swapped = matches!(op_kind, Some("rebase") | Some("rebase-interactive") | Some("am"));
    match (side, swapped) {
        (Side::Mine, false) | (Side::Theirs, true) => "--ours",
        (Side::Mine, true) | (Side::Theirs, false) => "--theirs",
    }
}

fn mapping_note(op_kind: Option<&str>, side: Side, flag: &str) -> String {
    match op_kind {
        Some(k) if side_flag(op_kind, Side::Mine) == "--theirs" => format!(
            "During a {} git calls your commit 'theirs', so {} ran `git checkout {}`.",
            if k == "am" { "git am" } else { "rebase" },
            side.label(),
            flag
        ),
        _ => format!("{} ran `git checkout {}`.", side.label(), flag),
    }
}

/// A commit id, branch, tag or expression the user typed: short enough, never
/// an option, never several words.
pub fn validate_rev_text(text: &str) -> Result<(), AppError> {
    if text.is_empty() {
        return Err(AppError::Command("Type a commit id, branch or tag.".into()));
    }
    if text.chars().count() > 256 {
        return Err(AppError::Command("That's too long to be a commit id, branch or tag.".into()));
    }
    if text.starts_with('-') {
        return Err(AppError::Command(format!("'{}' can't be a commit: it starts with '-'.", text)));
    }
    if text.chars().any(|c| c.is_whitespace() || c == '\0') {
        return Err(AppError::Command(format!("'{}' isn't a commit id, branch or tag (no spaces).", text)));
    }
    Ok(())
}

/// The command that puts the branch back after a reset. With a backup branch
/// the same mode is used again against it: `--soft` after a soft reset leaves
/// what was staged staged, `--mixed` after a mixed one puts the index back
/// (a `--soft` there would leave the index at the target's tree), `--hard`
/// after a hard one restores the files.
pub fn reset_recovery(mode: ResetMode, repo_path: &str, backup: Option<&str>, head_before: &str) -> String {
    match backup {
        Some(b) => format!("git -C {} reset {} {}", sync::shell_quote(repo_path), mode.flag(), b),
        None => format!("git reset {} {}", mode.flag(), head_before),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct CommitRef {
    pub oid: String,
    pub short: String,
    pub subject: String,
}

impl CommitRef {
    fn label(&self) -> String {
        format!("{} {}", self.short, self.subject)
    }
}

/// What a tree operation did, in facts the result card can show.
#[derive(Debug, Serialize, Clone, Default)]
pub struct TreeOutcome {
    /// "switch" | "create-branch" | "delete-branch" | "rename-branch" | "undo-commit" | "reset" | "detach" | "revert" | "cherry-pick" | "resolve-side" | "discard-all"
    pub op: String,
    pub head_before: Option<String>,
    pub head_after: Option<String>,
    pub branch_before: Option<String>,
    pub branch_after: Option<String>,
    /// The commit acted on: undone, reverted, picked, or the reset target.
    pub commit: Option<CommitRef>,
    /// A merge commit's parents, when a mainline has to be chosen.
    pub parents: Vec<CommitRef>,
    /// Commits that left the branch (first 10) and how many there were.
    pub dropped: Vec<CommitRef>,
    pub dropped_total: usize,
    pub backup: Option<crate::sync::BackupRef>,
    /// The safety stash made first, if any.
    pub stash: Option<crate::stash::StashRef>,
    /// The exact command that undoes this operation. For revert and
    /// cherry-pick it is `git reset --soft <head before>`: that drops the new
    /// commit and leaves its changes staged, so nothing uncommitted is lost.
    pub recovery: Option<String>,
    /// Gitlinks now checked out at a commit other than what HEAD records.
    pub submodule_mismatch: Vec<String>,
    pub blocking_files: Vec<String>,
    pub conflicts: Vec<String>,
    /// How "mine"/"theirs" mapped to git's --ours/--theirs this time.
    pub mapping_note: Option<String>,
    pub deleted_untracked: usize,
    pub restored_tracked: usize,
}

/// A commit the user typed or picked, with everything the "go back" dialog
/// needs to preview the consequences before anything runs.
#[derive(Debug, Serialize, Clone)]
pub struct CommitTarget {
    pub input: String,
    pub oid: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub is_merge: bool,
    pub parents: Vec<CommitRef>,
    pub is_head: bool,
    /// The commit is an ancestor of HEAD (a reset to it moves backwards).
    pub contained_in_head: bool,
    /// How many commits would leave the branch on a reset to it.
    pub dropped_if_reset: usize,
    /// Some of those commits are already on a remote branch (the upstream or
    /// any other) — a reset is refused, since taking them back would need a
    /// force-push. The same definition `can_amend` / `already-pushed` use.
    pub would_drop_pushed: bool,
    /// The commit itself is on some remote branch.
    pub on_remote: bool,
}

// ---------------------------------------------------------------------------
// Git reads shared by the operations
// ---------------------------------------------------------------------------

fn g(repo: &str) -> GitCmd {
    GitCmd::at(repo).pinned()
}

fn outcome(op: &str, before: &RepoStatus, after: &RepoStatus) -> TreeOutcome {
    TreeOutcome {
        op: op.to_string(),
        head_before: before.head_oid.clone(),
        head_after: after.head_oid.clone(),
        branch_before: before.branch.clone(),
        branch_after: after.branch.clone(),
        ..Default::default()
    }
}

fn refused_with(r: Refusal, status: RepoStatus, tree: TreeOutcome) -> OpResult {
    OpResult { tree: Some(tree), ..OpResult::refused(r, status) }
}

fn failed_with(op: GitOp, said: &str, status: RepoStatus, tree: TreeOutcome) -> OpResult {
    let a = git_advice::explain(op, said, &advice_ctx(&status));
    OpResult { tree: Some(tree), ..OpResult::failed(a, status) }
}

fn said(out: &crate::git_exec::GitOutput) -> String {
    let text = out.text();
    if text.is_empty() {
        out.stderr.clone()
    } else if out.stderr.is_empty() {
        text
    } else {
        format!("{}\n{}", text, out.stderr)
    }
}

/// `text^{commit}` → full oid, or None when git doesn't know it. Invalid text
/// is an error, not a miss: the UI shows it as such.
async fn resolve_oid(repo: &str, text: &str) -> Result<Option<String>, AppError> {
    validate_rev_text(text)?;
    Ok(g(repo)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{}^{{commit}}", text)])
        .ok_text()
        .await
        .filter(|s| !s.is_empty()))
}

/// `not-a-commit` finished with what the user typed.
fn name_target(r: Refusal, text: &str) -> Refusal {
    if r.code == "not-a-commit" {
        refuse("not-a-commit", format!("`{}` isn't a commit in this repository.", text))
    } else {
        r
    }
}

fn name_branch(r: Refusal, name: &str, valid: bool) -> Refusal {
    match r.code.as_str() {
        "no-such-branch" if !valid => invalid_name(name),
        "no-such-branch" => refuse("no-such-branch", format!("There's no branch called `{}` here.", name)),
        _ => r,
    }
}

fn invalid_name(name: &str) -> Refusal {
    refuse(
        "invalid-name",
        validate_branch_name(name).err().map(|e| e.to_string()).unwrap_or_else(|| format!("'{}' is not a valid branch name", name)),
    )
}

async fn count(repo: &str, args: &[&str]) -> usize {
    let mut all = vec!["rev-list", "--count"];
    all.extend_from_slice(args);
    g(repo).args(all).ok_text().await.and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// Commits reachable from HEAD that no branch, remote-tracking ref or tag
/// holds — what a detached HEAD would leave behind.
async fn detached_unique(repo: &str) -> usize {
    count(repo, &["HEAD", "--not", "--branches", "--remotes", "--tags"]).await
}

async fn commit_refs(repo: &str, oids: &[String]) -> Vec<CommitRef> {
    if oids.is_empty() {
        return Vec::new();
    }
    let mut args = vec!["log".to_string(), "--no-walk=unsorted".into(), "--format=%H%x1f%h%x1f%s".into()];
    args.extend(oids.iter().cloned());
    parse_refs(&g(repo).args(args).ok_text().await.unwrap_or_default())
}

async fn commit_ref(repo: &str, oid: &str) -> CommitRef {
    commit_refs(repo, &[oid.to_string()]).await.into_iter().next().unwrap_or_else(|| CommitRef {
        oid: oid.to_string(),
        short: sync::short(oid),
        subject: String::new(),
    })
}

fn parse_refs(out: &str) -> Vec<CommitRef> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\x1f').collect();
            CommitRef {
                oid: f.first().copied().unwrap_or_default().to_string(),
                short: f.get(1).copied().unwrap_or_default().to_string(),
                subject: f.get(2).copied().unwrap_or_default().to_string(),
            }
        })
        .collect()
}

/// The parents of one commit, in order.
async fn parents_of(repo: &str, oid: &str) -> Vec<String> {
    g(repo)
        .args(["rev-list", "--parents", "-n1", oid])
        .ok_text()
        .await
        .map(|s| s.split_whitespace().skip(1).map(|p| p.to_string()).collect())
        .unwrap_or_default()
}

/// Commits in `oid..HEAD` that some remote-tracking branch already holds, and
/// which branches those are. "Pushed" means the same thing everywhere in this
/// app — on *any* remote branch, not only the upstream — because a commit a
/// remote has can only be taken back with a force-push, which GitSwitch never
/// does. `can_amend` / `already-pushed` use the same definition (any ref under
/// `refs/remotes` containing HEAD). Measured exactly, so a branch that has
/// diverged from, or fallen behind, its upstream is judged right too.
#[derive(Debug, Default)]
struct PushedDropped {
    count: usize,
    /// The remote branches holding at least one of those commits (short names).
    on: Vec<String>,
}

async fn pushed_dropped(repo: &str, oid: &str) -> PushedDropped {
    let total = count(repo, &[&format!("{}..HEAD", oid)]).await;
    if total == 0 {
        return PushedDropped::default();
    }
    let unpushed = count(repo, &["HEAD", &format!("^{}", oid), "--not", "--remotes"]).await;
    let pushed = total.saturating_sub(unpushed);
    if pushed == 0 {
        return PushedDropped::default();
    }
    // Name the branches: the newest dropped commit that is not among the
    // unpushed ones is on some remote; ask which. Bounded lists — a reset
    // dropping more than 200 commits is named by its newest ones.
    let dropped = g(repo).args(["rev-list", "-n200", &format!("{}..HEAD", oid)]).ok_text().await.unwrap_or_default();
    let unpushed_list = g(repo)
        .args(["rev-list", "-n200", "HEAD", &format!("^{}", oid), "--not", "--remotes"])
        .ok_text()
        .await
        .unwrap_or_default();
    let unpushed_set: std::collections::HashSet<&str> = unpushed_list.lines().map(str::trim).collect();
    let on = match dropped.lines().map(str::trim).find(|c| !c.is_empty() && !unpushed_set.contains(c)) {
        Some(c) => remote_branches_containing(repo, c).await,
        None => Vec::new(),
    };
    PushedDropped { count: pushed, on }
}

/// Short names of the remote-tracking branches that contain `commit`
/// (`origin/main`), skipping each remote's symbolic `HEAD`.
async fn remote_branches_containing(repo: &str, commit: &str) -> Vec<String> {
    g(repo)
        .args(["for-each-ref", &format!("--contains={}", commit), "--format=%(refname)", "refs/remotes"])
        .ok_text()
        .await
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|r| !r.is_empty() && !r.ends_with("/HEAD"))
                .map(|r| r.trim_start_matches("refs/remotes/").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `would-drop-pushed` sentence with the branches that actually hold the
/// commits; the pure check only knows the upstream.
fn would_drop_pushed_message(on: &[String], upstream: Option<&str>) -> String {
    let where_ = match (on.is_empty(), upstream) {
        (false, _) => on.iter().map(|b| format!("`{}`", b)).collect::<Vec<_>>().join(", "),
        (true, Some(u)) => format!("`{}`", u),
        (true, None) => "the remote".to_string(),
    };
    format!(
        "Commits already on {} would be dropped, and GitSwitch never force-pushes. Look at that commit (detached), start a branch there, or Revert instead.",
        where_
    )
}

fn conflicted_paths(s: &RepoStatus) -> Vec<String> {
    s.entries.iter().filter(|e| e.kind == "conflicted").map(|e| e.path.clone()).collect()
}

fn submodule_mismatch(s: &RepoStatus) -> Vec<String> {
    s.entries.iter().filter(|e| e.is_submodule && e.sub_commit_changed).map(|e| e.path.clone()).collect()
}

/// Tracked, non-submodule entries with staged or unstaged changes. A gitlink
/// that a reset left pointing elsewhere is reported as `submodule_mismatch`,
/// not counted as dirt.
fn dirty_files(s: &RepoStatus) -> usize {
    s.entries.iter().filter(|e| e.kind == "tracked" && !e.is_submodule && (e.is_staged() || e.is_unstaged())).count()
}

fn switch_back(before: &RepoStatus) -> Option<String> {
    match (&before.branch, &before.head_oid) {
        (Some(b), _) => Some(format!("git switch {}", b)),
        (None, Some(h)) => Some(format!("git switch --detach {}", h)),
        _ => None,
    }
}

fn submodule_note(paths: &[String]) -> String {
    if paths.is_empty() {
        String::new()
    } else {
        format!(
            " {} submodule(s) are checked out at a different commit than this one records: {}. Update submodules to align them; their own changes are untouched.",
            paths.len(),
            paths.join(", ")
        )
    }
}

/// A branch name nothing else uses yet: two resets within the same second
/// must not fail the second one.
async fn free_backup_name(repo: &str, prefix: &str) -> String {
    let base = format!("{}{}", prefix, sync::stamp());
    let mut name = base.clone();
    let mut n = 1;
    while sync::oid(Path::new(repo), &format!("refs/heads/{}", name)).await.is_some() {
        n += 1;
        name = format!("{}-{}", base, n);
    }
    name
}

/// `git stash push` for a safety copy, verified by the stash count growing.
/// Untracked files go in only when `include_untracked` — the caller is about
/// to delete them; otherwise they stay on disk, where neither a hard reset nor
/// a discard of tracked changes touches them.
async fn safety_stash(repo: &str, before: &RepoStatus, message: &str, include_untracked: bool) -> Result<Result<StashRef, String>, AppError> {
    let mut args = vec!["stash", "push", "--quiet"];
    if include_untracked {
        args.push("--include-untracked");
    }
    args.extend(["-m", message]);
    let out = g(repo).args(args).timeout(LOCAL_TIMEOUT).run().await?;
    if !out.ok() {
        return Ok(Err(said(&out)));
    }
    let after = snapshot(repo).await?;
    if after.stash_count <= before.stash_count {
        return Ok(Err(format!("git stash push exited 0 but the stash list did not grow ({} before, {} after)", before.stash_count, after.stash_count)));
    }
    let oid = sync::oid(Path::new(repo), "stash@{0}").await.unwrap_or_default();
    Ok(Ok(StashRef { ref_: "stash@{0}".into(), oid, message: message.to_string() }))
}

// ---------------------------------------------------------------------------
// Branches
// ---------------------------------------------------------------------------

/// `git switch --no-guess -- <name>`. A dirty tree is allowed: git refuses
/// by itself when a changed file would be overwritten, and that refusal is
/// reported with the files named.
pub async fn switch_branch(repo_path: &str, name: &str) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let unique = if before.detached { detached_unique(repo_path).await } else { 0 };
    let facts = StatusFacts::from(&before);
    if let Some(r) = check(&Intent::Tree(TreeIntent::Switch { detached_unique: unique }), &facts) {
        return Ok(OpResult::refused(r, before));
    }
    if validate_branch_name(name).is_err() {
        return Ok(OpResult::refused(invalid_name(name), before));
    }

    let out = g(repo_path)
        .args(["switch", "--no-guess", "--", name])
        .timeout(NET_TIMEOUT)
        .run()
        .await?;
    let after = snapshot(repo_path).await?;
    let mut tree = outcome("switch", &before, &after);
    if !out.ok() {
        tree.blocking_files = git_advice::blocking_files(&out.stderr);
        return Ok(failed_with(GitOp::Checkout, &said(&out), after, tree));
    }
    if after.branch.as_deref() != Some(name) {
        return Ok(failed_with(
            GitOp::Checkout,
            &format!("git switch exited 0 but HEAD is on {} — {}", after.branch.clone().unwrap_or_else(|| "no branch".into()), out.stderr),
            after,
            tree,
        ));
    }
    tree.submodule_mismatch = submodule_mismatch(&after);
    tree.recovery = switch_back(&before);
    let detail = format!(
        "Was {}.{}",
        before.branch.as_ref().map(|b| format!("on {}", b)).unwrap_or_else(|| format!("detached at {}", sync::short(before.head_oid.as_deref().unwrap_or("")))),
        submodule_note(&tree.submodule_mismatch)
    );
    Ok(OpResult { tree: Some(tree), ..OpResult::done(format!("Switched to {}.", name), detail, after) })
}

/// Is `text` a remote-tracking branch as the user would type it (`origin/x`)?
/// Then git gets the ref itself, so the new branch tracks it.
async fn remote_tracking_ref(repo: &str, text: &str) -> Option<String> {
    let full = if text.starts_with("refs/remotes/") { text.to_string() } else { format!("refs/remotes/{}", text) };
    sync::oid(Path::new(repo), &full).await.map(|_| full)
}

/// `from` None starts at HEAD. `switch_to` runs `git switch -c`; otherwise
/// `git branch`, which leaves HEAD alone and is allowed during an operation.
pub async fn create_branch(repo_path: &str, name: &str, from: Option<&str>, switch_to: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let from = from.map(str::trim).filter(|s| !s.is_empty());
    let (from_oid, from_text) = match from {
        None => (before.head_oid.clone(), "HEAD".to_string()),
        Some(t) => (resolve_oid(repo_path, t).await?, t.to_string()),
    };
    // Commits only HEAD holds are kept when the new branch starts at HEAD.
    let unique = if switch_to && before.detached && from_oid.is_some() && from_oid != before.head_oid {
        detached_unique(repo_path).await
    } else {
        0
    };
    let intent = TreeIntent::CreateBranch { switch_to, from_valid: from_oid.is_some(), detached_unique: unique };
    if let Some(r) = check(&Intent::Tree(intent), &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(name_target(r, &from_text), before));
    }
    if validate_branch_name(name).is_err() {
        return Ok(OpResult::refused(invalid_name(name), before));
    }
    let from_oid = from_oid.unwrap_or_default();

    let tracking = match from {
        Some(t) => remote_tracking_ref(repo_path, t).await,
        None => None,
    };
    let start = tracking.clone().unwrap_or_else(|| from_oid.clone());
    let cmd = g(repo_path).cfg("branch.autoSetupMerge", "true");
    let out = if switch_to {
        cmd.args(["switch", "--no-guess", "-c", name, &start]).timeout(NET_TIMEOUT).run().await?
    } else {
        cmd.args(["branch", "--", name, &start]).timeout(LOCAL_TIMEOUT).run().await?
    };
    let after = snapshot(repo_path).await?;
    let mut tree = outcome("create-branch", &before, &after);
    if !out.ok() {
        tree.blocking_files = git_advice::blocking_files(&out.stderr);
        return Ok(failed_with(GitOp::Checkout, &said(&out), after, tree));
    }
    let created = sync::oid(Path::new(repo_path), &format!("refs/heads/{}", name)).await;
    if created.as_deref() != Some(from_oid.as_str()) || (switch_to && after.branch.as_deref() != Some(name)) {
        return Ok(failed_with(
            GitOp::Branch,
            &format!("git exited 0 but {} is at {} (expected {}) and HEAD is on {}", name, created.unwrap_or_default(), from_oid, after.branch.clone().unwrap_or_default()),
            after,
            tree,
        ));
    }
    tree.commit = Some(commit_ref(repo_path, &from_oid).await);
    tree.branch_after = Some(name.to_string());
    tree.submodule_mismatch = if switch_to { submodule_mismatch(&after) } else { Vec::new() };
    tree.recovery = Some(match (switch_to, switch_back(&before)) {
        (true, Some(back)) => format!("{} && git branch -D {}", back, name),
        _ => format!("git branch -D {}", name),
    });
    let headline = if switch_to { format!("Created and switched to {}.", name) } else { format!("Created {}.", name) };
    let detail = format!(
        "Starts at {}{}.{}",
        tree.commit.as_ref().map(|c| c.label()).unwrap_or_default(),
        tracking.as_ref().map(|t| format!(", tracking {}", t.trim_start_matches("refs/remotes/"))).unwrap_or_default(),
        submodule_note(&tree.submodule_mismatch)
    );
    Ok(OpResult { tree: Some(tree), ..OpResult::done(headline, detail, after) })
}

/// `git branch -D`, once the app's own gate has passed: no commit is lost when
/// every commit of the branch is held by some other ref (branch, remote-tracking
/// branch, tag or stash), or the user has confirmed the count of commits only
/// this branch holds. Git's `-d` judges "merged into HEAD or the upstream",
/// which refuses a branch merged only into another branch — so it is not used.
/// The local ref only; a remote branch is never touched.
pub async fn delete_branch(repo_path: &str, name: &str, force: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let valid = validate_branch_name(name).is_ok();
    let full = format!("refs/heads/{}", name);
    let tip = if valid { sync::oid(Path::new(repo_path), &full).await } else { None };
    let unique = if tip.is_some() { count(repo_path, &[&full, "--not", &format!("--exclude={}", full), "--all"]).await } else { 0 };
    let intent = TreeIntent::DeleteBranch {
        is_current: before.branch.as_deref() == Some(name),
        exists: tip.is_some(),
        unique_commits: unique,
        force,
    };
    if let Some(r) = check(&Intent::Tree(intent), &StatusFacts::from(&before)) {
        let r = if r.code == "unmerged-branch" { refuse("unmerged-branch", unmerged_branch_message(name, unique)) } else { name_branch(r, name, valid) };
        return Ok(OpResult::refused(r, before));
    }
    let tip = tip.unwrap_or_default();

    let out = g(repo_path).args(["branch", "-D", "--", name]).timeout(LOCAL_TIMEOUT).run().await?;
    let after = snapshot(repo_path).await?;
    let mut tree = outcome("delete-branch", &before, &after);
    tree.commit = Some(commit_ref(repo_path, &tip).await);
    if !out.ok() {
        return Ok(failed_with(GitOp::Branch, &said(&out), after, tree));
    }
    if sync::oid(Path::new(repo_path), &full).await.is_some() {
        return Ok(failed_with(GitOp::Branch, &format!("git branch exited 0 but {} still exists", name), after, tree));
    }
    tree.recovery = Some(format!("git branch {} {}", name, tip));
    let mut detail = format!("It pointed at {}. Recreate it with `git branch {} {}`.", tree.commit.as_ref().map(|c| c.label()).unwrap_or_default(), name, tip);
    if unique > 0 {
        detail.push_str(&format!(" {} commit(s) no other branch held are now reachable only by that id.", unique));
    }
    Ok(OpResult { tree: Some(tree), ..OpResult::done(format!("Deleted {}.", name), detail, after) })
}

/// `git branch -m -- <old> <new>`; the upstream and reflog move with it.
pub async fn rename_branch(repo_path: &str, old: &str, new: &str) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let old_valid = validate_branch_name(old).is_ok();
    let exists = old_valid && sync::oid(Path::new(repo_path), &format!("refs/heads/{}", old)).await.is_some();
    if let Some(r) = check(&Intent::Tree(TreeIntent::RenameBranch { exists }), &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(name_branch(r, old, old_valid), before));
    }
    if validate_branch_name(new).is_err() {
        return Ok(OpResult::refused(invalid_name(new), before));
    }

    let out = g(repo_path).args(["branch", "-m", "--", old, new]).timeout(LOCAL_TIMEOUT).run().await?;
    let after = snapshot(repo_path).await?;
    let mut tree = outcome("rename-branch", &before, &after);
    tree.branch_before = Some(old.to_string());
    tree.branch_after = Some(new.to_string());
    if !out.ok() {
        return Ok(failed_with(GitOp::Branch, &said(&out), after, tree));
    }
    let repo = Path::new(repo_path);
    if sync::oid(repo, &format!("refs/heads/{}", new)).await.is_none() || sync::oid(repo, &format!("refs/heads/{}", old)).await.is_some() {
        return Ok(failed_with(GitOp::Branch, &format!("git branch -m exited 0 but the rename of {} to {} did not take", old, new), after, tree));
    }
    tree.recovery = Some(format!("git branch -m {} {}", new, old));
    Ok(OpResult {
        tree: Some(tree),
        ..OpResult::done(format!("Renamed {} to {}.", old, new), "Its commits, upstream and history are unchanged.".to_string(), after)
    })
}

// ---------------------------------------------------------------------------
// Undo, reset
// ---------------------------------------------------------------------------

/// `git reset --soft HEAD~1`: the last commit's changes come back staged.
pub async fn undo_commit(repo_path: &str) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let parents = if before.unborn { Vec::new() } else { parents_of(repo_path, "HEAD").await };
    let intent = TreeIntent::UndoCommit { head_is_root: !before.unborn && parents.is_empty(), head_is_merge: parents.len() > 1 };
    if let Some(r) = check(&Intent::Tree(intent), &StatusFacts::from(&before)) {
        let r = if r.code == "merge-commit" {
            refuse(
                "merge-commit",
                format!(
                    "That's a merge commit. Undoing it would turn the whole merge into staged changes. Reset the branch to `{}` instead.",
                    sync::short(&parents[0])
                ),
            )
        } else {
            r
        };
        return Ok(OpResult::refused(r, before));
    }
    let head_before = before.head_oid.clone().unwrap_or_default();
    let undone = commit_ref(repo_path, &head_before).await;

    let out = g(repo_path).args(["reset", "--soft", "--quiet", "HEAD~1"]).timeout(LOCAL_TIMEOUT).run().await?;
    let after = snapshot(repo_path).await?;
    let mut tree = outcome("undo-commit", &before, &after);
    tree.commit = Some(undone.clone());
    if !out.ok() {
        return Ok(failed_with(GitOp::Reset, &said(&out), after, tree));
    }
    if after.head_oid.as_deref() != Some(parents[0].as_str()) {
        return Ok(failed_with(GitOp::Reset, &format!("git reset exited 0 but HEAD is {} (expected {})", after.head_oid.clone().unwrap_or_default(), parents[0]), after, tree));
    }
    let files = g(repo_path)
        .args(["diff", "--cached", "--name-only"])
        .ok_text()
        .await
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(after.staged_count);
    tree.recovery = Some(format!("git reset --soft {}", head_before));
    Ok(OpResult {
        tree: Some(tree),
        ..OpResult::done(
            "Undid the last commit.".to_string(),
            format!("`{}` is undone; its {} file(s) are staged again. Redo with `git reset --soft {}`.", undone.label(), files, head_before),
            after,
        )
    })
}

/// Move the branch to `target`. Commits that would leave it are kept on a
/// backup branch first; commits any remote branch already has are never
/// dropped. `stash_first` stashes the staged and unstaged changes before the
/// move (untracked files are left on disk: no reset touches them).
pub async fn reset_to(repo_path: &str, target: &str, mode: ResetMode, stash_first: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let target = target.trim();
    let oid = resolve_oid(repo_path, target).await?;
    let target_is_head = oid.is_some() && oid == before.head_oid;
    let (dropped_total, pushed) = match &oid {
        Some(o) if !before.unborn => (count(repo_path, &[&format!("{}..HEAD", o)]).await, pushed_dropped(repo_path, o).await),
        _ => (0, PushedDropped::default()),
    };
    let intent = TreeIntent::Reset { mode, target_valid: oid.is_some(), target_is_head, drops_pushed: pushed.count > 0, stash_first };
    if let Some(r) = check(&Intent::Tree(intent), &StatusFacts::from(&before)) {
        let r = if r.code == "would-drop-pushed" {
            refuse("would-drop-pushed", would_drop_pushed_message(&pushed.on, before.upstream.as_deref()))
        } else {
            name_target(r, target)
        };
        return Ok(OpResult::refused(r, before));
    }
    let oid = oid.unwrap_or_default();
    let head_before = before.head_oid.clone().unwrap_or_default();
    let target_ref = commit_ref(repo_path, &oid).await;
    let mut tree = outcome("reset", &before, &before);
    tree.commit = Some(target_ref.clone());
    tree.dropped_total = dropped_total;

    // Backup first: the only step that can leave nothing behind if it fails.
    if dropped_total > 0 {
        let name = free_backup_name(repo_path, RESET_BACKUP_PREFIX).await;
        let boid = sync::make_backup(Path::new(repo_path), &name).await?;
        tree.backup = Some(BackupRef { repo: repo_path.to_string(), branch: name.clone(), oid: boid, recovery: reset_recovery(mode, repo_path, Some(&name), &head_before) });
        tree.dropped = parse_refs(
            &g(repo_path)
                .args(["log", "-n10", "--format=%H%x1f%h%x1f%s", &format!("{}..{}", oid, head_before)])
                .ok_text()
                .await
                .unwrap_or_default(),
        );
    }

    // Untracked files are not stashed: a reset of any mode leaves them alone.
    if stash_first && before.staged_count + before.unstaged_count > 0 {
        match safety_stash(repo_path, &before, &format!("gitswitch before reset to {}", target_ref.short), false).await? {
            Ok(s) => tree.stash = Some(s),
            Err(said) => {
                let after = snapshot(repo_path).await?;
                let note = tree.backup.as_ref().map(|b| format!("\nThe backup branch {} was created and is harmless.", b.branch)).unwrap_or_default();
                return Ok(failed_with(GitOp::Stash, &format!("{}{}\nThe branch was not moved.", said, note), after, tree));
            }
        }
    }

    let out = g(repo_path).args(["reset", mode.flag(), "--quiet", &oid]).timeout(LOCAL_TIMEOUT).run().await?;
    let after = snapshot(repo_path).await?;
    tree.head_after = after.head_oid.clone();
    tree.branch_after = after.branch.clone();
    if !out.ok() {
        return Ok(failed_with(GitOp::Reset, &said(&out), after, tree));
    }
    if after.head_oid.as_deref() != Some(oid.as_str()) {
        return Ok(failed_with(GitOp::Reset, &format!("git reset exited 0 but HEAD is {} (expected {})", after.head_oid.clone().unwrap_or_default(), oid), after, tree));
    }
    if mode == ResetMode::Hard && dirty_files(&after) > 0 {
        return Ok(failed_with(GitOp::Reset, &format!("git reset --hard exited 0 but {} file(s) still show changes", dirty_files(&after)), after, tree));
    }
    tree.submodule_mismatch = submodule_mismatch(&after);
    tree.recovery = Some(reset_recovery(mode, repo_path, tree.backup.as_ref().map(|b| b.branch.as_str()), &head_before));

    let mut detail = match mode {
        ResetMode::Soft => "The working tree and staging area are untouched; the commits' changes are staged.".to_string(),
        ResetMode::Mixed => "The working tree is untouched; the commits' changes are unstaged.".to_string(),
        ResetMode::Hard => "The working tree now matches that commit.".to_string(),
    };
    if let Some(b) = &tree.backup {
        detail.push_str(&format!(" {} commit(s) left the branch; `{}` still holds them.", dropped_total, b.branch));
    }
    if tree.stash.is_some() {
        detail.push_str(" Your uncommitted changes are in stash@{0}.");
    }
    detail.push_str(&format!(" Undo with `{}`.", tree.recovery.clone().unwrap_or_default()));
    detail.push_str(&submodule_note(&tree.submodule_mismatch));
    Ok(OpResult { tree: Some(tree), ..OpResult::done(format!("Reset to {} ({}).", target_ref.short, mode.label()), detail, after) })
}

// ---------------------------------------------------------------------------
// Detach, revert, cherry-pick
// ---------------------------------------------------------------------------

/// `git switch --detach <oid>`: look at a commit without moving any branch.
pub async fn detach(repo_path: &str, target: &str) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let target = target.trim();
    let oid = resolve_oid(repo_path, target).await?;
    let unique = if before.detached && oid.is_some() && oid != before.head_oid { detached_unique(repo_path).await } else { 0 };
    let intent = TreeIntent::Detach { target_valid: oid.is_some(), detached_unique: unique };
    if let Some(r) = check(&Intent::Tree(intent), &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(name_target(r, target), before));
    }
    let oid = oid.unwrap_or_default();

    let out = g(repo_path).args(["switch", "--detach", &oid]).timeout(NET_TIMEOUT).run().await?;
    let after = snapshot(repo_path).await?;
    let mut tree = outcome("detach", &before, &after);
    tree.commit = Some(commit_ref(repo_path, &oid).await);
    if !out.ok() {
        tree.blocking_files = git_advice::blocking_files(&out.stderr);
        return Ok(failed_with(GitOp::Checkout, &said(&out), after, tree));
    }
    if !after.detached || after.head_oid.as_deref() != Some(oid.as_str()) {
        return Ok(failed_with(GitOp::Checkout, &format!("git switch --detach exited 0 but HEAD is {} on {}", after.head_oid.clone().unwrap_or_default(), after.branch.clone().unwrap_or_else(|| "no branch".into())), after, tree));
    }
    tree.submodule_mismatch = submodule_mismatch(&after);
    tree.recovery = switch_back(&before);
    let detail = format!(
        "You're not on a branch: commits made here belong to no branch until you start one. Go back with `{}`.{}",
        tree.recovery.clone().unwrap_or_default(),
        submodule_note(&tree.submodule_mismatch)
    );
    let headline = format!("Looking at {}.", tree.commit.as_ref().map(|c| c.label()).unwrap_or_default());
    Ok(OpResult { tree: Some(tree), ..OpResult::done(headline, detail, after) })
}

/// The mainline sentence, with both parents named.
fn mainline_message(parents: &[CommitRef]) -> String {
    let p = |i: usize| parents.get(i).map(|c| c.label()).unwrap_or_default();
    format!(
        "That's a merge commit. Choose which side to keep: 1 keeps `{}` and undoes what came in from `{}`.",
        p(0),
        p(1)
    )
}

/// The new commit's parent must be the old HEAD, or the pick/revert did not
/// land where the result claims.
async fn landed_on(repo: &str, after: &RepoStatus, head_before: &str) -> bool {
    match &after.head_oid {
        Some(h) if h != head_before => parents_of(repo, h).await.first().map(|p| p == head_before).unwrap_or(false),
        _ => false,
    }
}

fn empty_pick(said: &str) -> bool {
    said.contains("is now empty") || said.contains("nothing to commit")
}

/// `git revert --no-edit [-m n] <oid>`. A merge commit needs a mainline; the
/// refusal carries both parents so the UI can offer the choice.
pub async fn revert(repo_path: &str, target: &str, mainline: Option<u32>) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let target = target.trim();
    let oid = resolve_oid(repo_path, target).await?;
    let parents = match &oid {
        Some(o) => parents_of(repo_path, o).await,
        None => Vec::new(),
    };
    let is_merge = parents.len() > 1;
    let intent = TreeIntent::Revert { target_valid: oid.is_some(), is_merge, mainline_given: mainline.is_some() };
    if let Some(r) = check(&Intent::Tree(intent), &StatusFacts::from(&before)) {
        if r.code == "merge-commit-needs-mainline" {
            let mut tree = outcome("revert", &before, &before);
            tree.commit = Some(commit_ref(repo_path, oid.as_deref().unwrap_or_default()).await);
            tree.parents = commit_refs(repo_path, &parents).await;
            let r = refuse("merge-commit-needs-mainline", mainline_message(&tree.parents));
            return Ok(refused_with(r, before, tree));
        }
        return Ok(OpResult::refused(name_target(r, target), before));
    }
    let oid = oid.unwrap_or_default();
    if let Some(m) = mainline {
        if is_merge && !(1..=parents.len() as u32).contains(&m) {
            return Ok(OpResult::refused(
                refuse("invalid-mainline", format!("That merge has {} parents; the mainline must be one of them (1 to {}).", parents.len(), parents.len())),
                before,
            ));
        }
    }
    let head_before = before.head_oid.clone().unwrap_or_default();
    let reverted = commit_ref(repo_path, &oid).await;

    let mut args: Vec<String> = vec!["revert".into(), "--no-edit".into()];
    if let (true, Some(m)) = (is_merge, mainline) {
        args.push("-m".into());
        args.push(m.to_string());
    }
    args.push(oid.clone());
    let out = g(repo_path).args(args).timeout(LOCAL_TIMEOUT).run().await?;
    let after = snapshot(repo_path).await?;
    let mut tree = outcome("revert", &before, &after);
    tree.commit = Some(reverted.clone());
    if !out.ok() {
        tree.conflicts = conflicted_paths(&after);
        // An empty revert is cleaned up by git itself: no REVERT_HEAD, HEAD unmoved.
        if after.operation.is_none() && after.head_oid.as_deref() == Some(head_before.as_str()) && after.conflicted_count == 0 && empty_pick(&said(&out)) {
            return Ok(OpResult {
                tree: Some(tree),
                ..OpResult::done(
                    format!("Nothing to revert: this branch's content already lacks {}'s changes.", reverted.short),
                    "No commit was made.".to_string(),
                    after,
                )
            });
        }
        return Ok(failed_with(GitOp::Revert, &said(&out), after, tree));
    }
    if !landed_on(repo_path, &after, &head_before).await {
        return Ok(failed_with(GitOp::Revert, &format!("git revert exited 0 but HEAD is {} (was {})", after.head_oid.clone().unwrap_or_default(), head_before), after, tree));
    }
    let new = commit_ref(repo_path, after.head_oid.as_deref().unwrap_or_default()).await;
    tree.recovery = Some(format!("git reset --soft {}", head_before));
    Ok(OpResult {
        tree: Some(tree),
        ..OpResult::done(
            format!("Reverted {}.", reverted.short),
            format!("New commit `{}` undoes `{}`. Undo the revert with `git reset --soft {}` (keeps its changes staged).", new.label(), reverted.label(), head_before),
            after,
        )
    })
}

/// `git cherry-pick <oid>`. A pick that turns out empty is skipped and said so;
/// conflicts are left for the user, exactly as git left them.
pub async fn cherry_pick(repo_path: &str, target: &str) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let target = target.trim();
    let oid = resolve_oid(repo_path, target).await?;
    let (is_merge, contained) = match &oid {
        Some(o) => (parents_of(repo_path, o).await.len() > 1, !before.unborn && sync::is_ancestor(Path::new(repo_path), o, "HEAD").await),
        None => (false, false),
    };
    let intent = TreeIntent::CherryPick { target_valid: oid.is_some(), is_merge, already_contained: contained };
    if let Some(r) = check(&Intent::Tree(intent), &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(name_target(r, target), before));
    }
    let oid = oid.unwrap_or_default();
    let head_before = before.head_oid.clone().unwrap_or_default();
    let picked = commit_ref(repo_path, &oid).await;

    let out = g(repo_path).args(["cherry-pick", &oid]).timeout(LOCAL_TIMEOUT).run().await?;
    let mut after = snapshot(repo_path).await?;
    let mut tree = outcome("cherry-pick", &before, &after);
    tree.commit = Some(picked.clone());
    if !out.ok() {
        let words = said(&out);
        let picking = after.operation.as_ref().map(|o| o.kind == "cherry-pick").unwrap_or(false);
        if picking && after.conflicted_count == 0 && empty_pick(&words) {
            // Git stopped on a pick with nothing left to apply. Skipping it is
            // the only way on; --continue refuses.
            let skip = g(repo_path).args(["cherry-pick", "--skip"]).timeout(LOCAL_TIMEOUT).run().await?;
            after = snapshot(repo_path).await?;
            tree.head_after = after.head_oid.clone();
            if !skip.ok() || after.operation.is_some() {
                return Ok(failed_with(GitOp::CherryPick, &format!("{}\n{}", words, said(&skip)), after, tree));
            }
            return Ok(OpResult {
                tree: Some(tree),
                ..OpResult::done(format!("Nothing to apply: {} is already in this branch's content.", picked.short), "No commit was made.".to_string(), after)
            });
        }
        tree.conflicts = conflicted_paths(&after);
        return Ok(failed_with(GitOp::CherryPick, &words, after, tree));
    }
    if !landed_on(repo_path, &after, &head_before).await {
        return Ok(failed_with(GitOp::CherryPick, &format!("git cherry-pick exited 0 but HEAD is {} (was {})", after.head_oid.clone().unwrap_or_default(), head_before), after, tree));
    }
    let new = commit_ref(repo_path, after.head_oid.as_deref().unwrap_or_default()).await;
    tree.recovery = Some(format!("git reset --soft {}", head_before));
    Ok(OpResult {
        tree: Some(tree),
        ..OpResult::done(
            format!("Picked {}.", picked.short),
            format!("New commit `{}` carries `{}`. Undo with `git reset --soft {}` (keeps its changes staged).", new.label(), picked.label(), head_before),
            after,
        )
    })
}

// ---------------------------------------------------------------------------
// Conflict sides, discard everything
// ---------------------------------------------------------------------------

/// Resolve conflicted paths by taking one side, in the user's words. A path
/// whose chosen side has no version (deleted on that side) is removed.
pub async fn resolve_side(repo_path: &str, paths: Vec<String>, side: Side) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let selected: Vec<&crate::git_status::ChangeEntry> = before.entries.iter().filter(|e| e.kind == "conflicted" && paths.contains(&e.path)).collect();
    let not_conflicted = paths.iter().find(|p| !selected.iter().any(|e| &e.path == *p)).cloned();
    let intent = TreeIntent::ResolveSide {
        has_paths: !paths.is_empty(),
        submodule_selected: selected.iter().any(|e| e.is_submodule || e.stage_modes.iter().any(|m| m == "160000")),
        not_conflicted_selected: not_conflicted.is_some(),
    };
    if let Some(r) = check(&Intent::Tree(intent), &StatusFacts::from(&before)) {
        let r = match (&r.code[..], &not_conflicted) {
            ("not-conflicted", Some(p)) => refuse("not-conflicted", format!("{} isn't in conflict — the list may be out of date.", p)),
            _ => r,
        };
        return Ok(OpResult::refused(r, before));
    }

    let op_kind = before.operation.as_ref().map(|o| o.kind.clone());
    let flag = side_flag(op_kind.as_deref(), side);
    let stage = if flag == "--ours" { 1 } else { 2 };
    let (keep, remove): (Vec<String>, Vec<String>) = {
        let mut keep = Vec::new();
        let mut remove = Vec::new();
        for e in &selected {
            let present = e.stage_modes.get(stage).map(|m| m != "000000").unwrap_or(false);
            if present {
                keep.push(e.path.clone());
            } else {
                remove.push(e.path.clone());
            }
        }
        (keep, remove)
    };

    let mut errors = Vec::new();
    if !keep.is_empty() {
        let out = g(repo_path)
            .args(["checkout", flag, "--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin_bytes(pathspec_stdin(&keep)?)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !out.ok() {
            errors.push(said(&out));
        } else {
            let add = g(repo_path)
                .args(["add", "--pathspec-from-file=-", "--pathspec-file-nul"])
                .stdin_bytes(pathspec_stdin(&keep)?)
                .timeout(LOCAL_TIMEOUT)
                .run()
                .await?;
            if !add.ok() {
                errors.push(said(&add));
            }
        }
    }
    if !remove.is_empty() {
        let out = g(repo_path)
            .args(["rm", "--quiet", "--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin_bytes(pathspec_stdin(&remove)?)
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !out.ok() {
            errors.push(said(&out));
        }
    }

    let after = snapshot(repo_path).await?;
    let mut tree = outcome("resolve-side", &before, &after);
    tree.mapping_note = Some(mapping_note(op_kind.as_deref(), side, flag));
    tree.conflicts = conflicted_paths(&after);
    let still: Vec<&String> = paths.iter().filter(|p| tree.conflicts.contains(p)).collect();
    if !errors.is_empty() || !still.is_empty() {
        let mut words = errors.join("\n");
        if !still.is_empty() {
            words.push_str(&format!("\n{} still conflicted after the resolution: {}", still.len(), still.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
        }
        return Ok(failed_with(GitOp::Merge, words.trim(), after, tree));
    }
    let mut detail = format!(
        "{} file(s) now hold {} side and are staged{}. {}",
        keep.len(),
        if side == Side::Mine { "your" } else { "the other" },
        if remove.is_empty() { String::new() } else { format!("; {} removed because that side deleted {}", remove.len(), plural(remove.len(), "it", "them")) },
        tree.mapping_note.clone().unwrap_or_default()
    );
    if !tree.conflicts.is_empty() {
        detail.push_str(&format!(" {} other file(s) are still in conflict.", tree.conflicts.len()));
    }
    Ok(OpResult { tree: Some(tree), ..OpResult::done(format!("{}: {} file(s) resolved.", side.label(), paths.len()), detail, after) })
}

/// Everything back to HEAD: tracked changes restored, new files left untracked
/// unless asked to delete them, ignored files never touched (no `-x`). The
/// safety stash takes the new files only when they are about to be deleted;
/// otherwise they stay exactly where they are.
pub async fn discard_all(repo_path: &str, include_untracked: bool, stash_first: bool) -> Result<OpResult, AppError> {
    let before = snapshot(repo_path).await?;
    let intent = TreeIntent::DiscardAll { include_untracked, stash_first };
    if let Some(r) = check(&Intent::Tree(intent), &StatusFacts::from(&before)) {
        return Ok(OpResult::refused(r, before));
    }
    let restored_tracked = before.entries.iter().filter(|e| e.kind != "untracked" && !e.is_submodule).count();
    let staged_added = before.entries.iter().filter(|e| e.kind == "tracked" && e.staged == "A").count();
    let mut tree = outcome("discard-all", &before, &before);
    tree.restored_tracked = restored_tracked;

    if stash_first {
        match safety_stash(repo_path, &before, "gitswitch before discard", include_untracked).await? {
            Ok(s) => tree.stash = Some(s),
            Err(said) => {
                let after = snapshot(repo_path).await?;
                return Ok(failed_with(GitOp::Stash, &format!("{}\nNothing was discarded.", said), after, tree));
            }
        }
    }

    let mut errors = Vec::new();
    // Index first, then the worktree: in one step a file that is staged but
    // not in HEAD would be deleted from disk, not merely unstaged.
    for scope in ["--staged", "--worktree"] {
        let out = g(repo_path)
            .args(["restore", scope, "--source=HEAD", "--", ":/"])
            .timeout(LOCAL_TIMEOUT)
            .run()
            .await?;
        if !out.ok() {
            errors.push(said(&out));
        }
    }
    if include_untracked && errors.is_empty() {
        // One --force, no -x (ignored files stay), no -ff (nested repositories stay).
        let out = g(repo_path).args(["clean", "-d", "--force"]).timeout(LOCAL_TIMEOUT).run().await?;
        if !out.ok() {
            errors.push(said(&out));
        }
    }

    let after = snapshot(repo_path).await?;
    tree.head_after = after.head_oid.clone();
    tree.branch_after = after.branch.clone();
    if !errors.is_empty() {
        return Ok(failed_with(GitOp::Checkout, &errors.join("\n"), after, tree));
    }
    let left = dirty_files(&after) + after.conflicted_count;
    if left > 0 || (include_untracked && after.untracked_count > 0) {
        return Ok(failed_with(
            GitOp::Checkout,
            &format!("git exited 0 but {} tracked file(s) and {} untracked file(s) still show changes", left, after.untracked_count),
            after,
            tree,
        ));
    }
    tree.deleted_untracked = if include_untracked { (before.untracked_count + staged_added).saturating_sub(after.untracked_count) } else { 0 };
    tree.submodule_mismatch = submodule_mismatch(&after);
    // Nothing here recurses: a submodule's own edits, untracked files and
    // moved pointer are exactly as they were.
    let dirty_subs: Vec<String> = after.entries.iter().filter(|e| e.is_submodule && (e.sub_tracked_changes || e.sub_untracked || e.sub_commit_changed)).map(|e| e.path.clone()).collect();

    let mut detail = format!("{} tracked file(s) restored to the last commit.", restored_tracked);
    if include_untracked {
        detail.push_str(&format!(" {} untracked file(s) deleted; ignored files were left alone.", tree.deleted_untracked));
    } else if after.untracked_count > 0 {
        detail.push_str(&format!(" {} untracked file(s) were left in place{}.", after.untracked_count, if staged_added > 0 { format!(" ({} of them newly added, now untracked)", staged_added) } else { String::new() }));
    }
    if let Some(s) = &tree.stash {
        detail.push_str(&format!(" Everything is kept in {} — apply it to get it back.", s.ref_));
    }
    if !dirty_subs.is_empty() {
        detail.push_str(&format!(" Changes inside submodules were not touched: {}.", dirty_subs.join(", ")));
    }
    Ok(OpResult { tree: Some(tree), ..OpResult::done("Discarded everything.".to_string(), detail, after) })
}

// ---------------------------------------------------------------------------
// Commit lookup
// ---------------------------------------------------------------------------

/// What the user typed, resolved, with what a reset to it would do. `None`
/// when git knows no such commit; malformed text is an error.
pub async fn resolve_commit(repo_path: &str, text: &str) -> Result<Option<CommitTarget>, AppError> {
    let text = text.trim();
    let Some(oid) = resolve_oid(repo_path, text).await? else {
        return Ok(None);
    };
    let line = g(repo_path)
        .args(["log", "-1", "--format=%H%x1f%h%x1f%s%x1f%an%x1f%cI%x1f%P", &oid])
        .ok_text()
        .await
        .unwrap_or_default();
    let f: Vec<&str> = line.split('\x1f').collect();
    let parent_oids: Vec<String> = f.get(5).copied().unwrap_or_default().split_whitespace().map(|s| s.to_string()).collect();
    let parents = commit_refs(repo_path, &parent_oids).await;
    let repo = Path::new(repo_path);
    let head = sync::oid(repo, "HEAD").await;
    let is_head = head.as_deref() == Some(oid.as_str());
    let contained_in_head = head.is_some() && sync::is_ancestor(repo, &oid, "HEAD").await;
    let dropped_if_reset = if head.is_some() { count(repo_path, &[&format!("{}..HEAD", oid)]).await } else { 0 };
    let would_drop_pushed = head.is_some() && pushed_dropped(repo_path, &oid).await.count > 0;
    let on_remote = !remote_branches_containing(repo_path, &oid).await.is_empty();
    Ok(Some(CommitTarget {
        input: text.to_string(),
        oid: oid.clone(),
        short: f.get(1).copied().unwrap_or_default().to_string(),
        subject: f.get(2).copied().unwrap_or_default().to_string(),
        author: f.get(3).copied().unwrap_or_default().to_string(),
        date: f.get(4).copied().unwrap_or_default().to_string(),
        is_merge: parent_oids.len() > 1,
        parents,
        is_head,
        contained_in_head,
        dropped_if_reset,
        would_drop_pushed,
        on_remote,
    }))
}

// ---------------------------------------------------------------------------
// Probe driver
// ---------------------------------------------------------------------------

/// Shell-suite driver for the `tree_*` probe ops (see probe.rs). Args are the
/// comma-split PROBE_ARGS.
#[cfg(test)]
pub(crate) async fn probe(op: &str, repo: &str, args: &[String]) {
    use crate::probe::tests::{emit, emit_err, show};
    let a = |i: usize| args.get(i).map(|s| s.as_str()).unwrap_or("");
    let has = |word: &str| args.iter().skip(1).any(|s| s == word);
    match op {
        "tree_branch_switch" => show(switch_branch(repo, a(0)).await),
        // <name>[,<from>][,switch]
        "tree_branch_create" => {
            let from = args.get(1).filter(|s| s.as_str() != "switch").map(|s| s.as_str());
            show(create_branch(repo, a(0), from, has("switch")).await)
        }
        "tree_branch_delete" => show(delete_branch(repo, a(0), has("force")).await),
        "tree_branch_rename" => show(rename_branch(repo, a(0), a(1)).await),
        "tree_undo_commit" => show(undo_commit(repo).await),
        // <target>,<soft|mixed|hard>[,stash]
        "tree_reset" => match ResetMode::parse(a(1)) {
            Ok(mode) => show(reset_to(repo, a(0), mode, has("stash")).await),
            Err(e) => emit_err(e),
        },
        "tree_detach" => show(detach(repo, a(0)).await),
        // <target>[,<mainline>]
        "tree_revert" => show(revert(repo, a(0), args.get(1).and_then(|s| s.parse().ok())).await),
        "tree_cherry_pick" => show(cherry_pick(repo, a(0)).await),
        // <mine|theirs>,<path>[,<path>…]
        "tree_resolve_side" => match Side::parse(a(0)) {
            Ok(side) => show(resolve_side(repo, args.iter().skip(1).cloned().collect(), side).await),
            Err(e) => emit_err(e),
        },
        // [untracked][,stash]
        "tree_discard_all" => {
            let untracked = args.iter().any(|s| s == "untracked");
            let stash = args.iter().any(|s| s == "stash");
            show(discard_all(repo, untracked, stash).await)
        }
        "tree_resolve" => match resolve_commit(repo, a(0)).await {
            Ok(t) => emit(&t),
            Err(e) => emit_err(e),
        },
        other => emit_err(format!("unknown tree op {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TreeIntent::*;

    fn clean() -> StatusFacts {
        StatusFacts { identity_matches: true, upstream: Some("origin/main".into()), can_amend: true, ..Default::default() }
    }

    fn code(intent: TreeIntent, f: &StatusFacts) -> String {
        check(&Intent::Tree(intent), f).map(|r| r.code).unwrap_or_else(|| "ok".into())
    }

    fn reset(mode: ResetMode, drops_pushed: bool, stash_first: bool) -> TreeIntent {
        Reset { mode, target_valid: true, target_is_head: false, drops_pushed, stash_first }
    }

    #[test]
    fn a_reset_refuses_pushed_commits_and_honours_a_hard_reset_of_a_dirty_tree() {
        let dirty = StatusFacts { unstaged: 2, staged: 1, ..clean() };
        // would-drop-pushed beats everything else about the tree.
        assert_eq!(code(reset(ResetMode::Hard, true, false), &dirty), "would-drop-pushed");
        assert_eq!(code(reset(ResetMode::Hard, true, true), &dirty), "would-drop-pushed");
        let r = check(&Intent::Tree(reset(ResetMode::Hard, true, false)), &dirty).unwrap();
        assert!(r.message.contains("a remote branch") && r.message.contains("never force-pushes"));
        // A hard reset without a stash on a dirty tree is the user's explicit
        // choice (the dialog said the changes are thrown away): not refused.
        assert_eq!(code(reset(ResetMode::Hard, false, false), &dirty), "ok");
        assert_eq!(code(reset(ResetMode::Hard, false, true), &dirty), "ok");
        assert_eq!(code(reset(ResetMode::Soft, false, false), &dirty), "ok");
        assert_eq!(code(reset(ResetMode::Mixed, false, false), &dirty), "ok");
        // The operation names the remote branches that hold the commits; the
        // pure check only knows the upstream.
        assert!(would_drop_pushed_message(&["origin/backup".into()], Some("origin/main")).contains("`origin/backup`"));
        assert!(would_drop_pushed_message(&[], Some("origin/main")).contains("`origin/main`"));
        assert!(would_drop_pushed_message(&[], None).contains("the remote"));
        // Unknown targets and empty repositories are named first.
        assert_eq!(code(Reset { mode: ResetMode::Soft, target_valid: false, target_is_head: false, drops_pushed: false, stash_first: false }, &clean()), "not-a-commit");
        assert_eq!(code(reset(ResetMode::Soft, false, false), &StatusFacts { unborn: true, ..clean() }), "unborn-head");
        assert_eq!(code(reset(ResetMode::Soft, false, false), &StatusFacts { conflicted: 1, ..clean() }), "unmerged-paths");
    }

    #[test]
    fn a_reset_to_head_is_nothing_to_do_unless_it_is_a_hard_reset_of_a_dirty_tree() {
        let at_head = |mode| Reset { mode, target_valid: true, target_is_head: true, drops_pushed: false, stash_first: false };
        assert_eq!(code(at_head(ResetMode::Soft), &clean()), "nothing-to-do");
        assert_eq!(code(at_head(ResetMode::Hard), &clean()), "nothing-to-do");
        let dirty = StatusFacts { unstaged: 1, ..clean() };
        assert_eq!(code(at_head(ResetMode::Mixed), &dirty), "nothing-to-do");
        // Hard at HEAD on a dirty tree is "throw my changes away": allowed, stash or not.
        assert_eq!(code(at_head(ResetMode::Hard), &dirty), "ok");
        assert_eq!(code(Reset { mode: ResetMode::Hard, target_valid: true, target_is_head: true, drops_pushed: false, stash_first: true }, &dirty), "ok");
    }

    #[test]
    fn undo_refuses_pushed_root_and_merge_commits_in_that_order() {
        let plain = UndoCommit { head_is_root: false, head_is_merge: false };
        assert_eq!(code(plain.clone(), &clean()), "ok");
        let pushed = StatusFacts { can_amend: false, ..clean() };
        let r = check(&Intent::Tree(plain.clone()), &pushed).unwrap();
        assert_eq!(r.code, "already-pushed");
        assert!(r.message.contains("Revert it instead"));
        // Pushed outranks root and merge: the fix is the same either way.
        assert_eq!(code(UndoCommit { head_is_root: true, head_is_merge: false }, &pushed), "already-pushed");
        assert_eq!(code(UndoCommit { head_is_root: true, head_is_merge: false }, &clean()), "root-commit");
        let r = check(&Intent::Tree(UndoCommit { head_is_root: false, head_is_merge: true }), &clean()).unwrap();
        assert_eq!(r.code, "merge-commit");
        assert!(r.message.contains("first parent"));
        assert_eq!(code(plain.clone(), &StatusFacts { unborn: true, ..clean() }), "unborn-head");
        assert_eq!(code(plain.clone(), &StatusFacts { conflicted: 2, ..clean() }), "unmerged-paths");
        // Detached is fine: undo works on HEAD, whatever it is.
        assert_eq!(code(plain, &StatusFacts { detached: true, ..clean() }), "ok");
    }

    #[test]
    fn switching_away_from_a_detached_head_with_its_own_commits_is_refused() {
        let detached = StatusFacts { detached: true, ..clean() };
        let r = check(&Intent::Tree(Switch { detached_unique: 2 }), &detached).unwrap();
        assert_eq!(r.code, "detached-commits");
        assert!(r.message.contains("2 commit(s)"));
        assert_eq!(code(Switch { detached_unique: 0 }, &detached), "ok");
        assert_eq!(code(Detach { target_valid: true, detached_unique: 1 }, &detached), "detached-commits");
        assert_eq!(code(CreateBranch { switch_to: true, from_valid: true, detached_unique: 1 }, &detached), "detached-commits");
        // Starting a branch here without switching is the way out, so it is allowed.
        assert_eq!(code(CreateBranch { switch_to: false, from_valid: true, detached_unique: 1 }, &detached), "ok");
        // A dirty tree is git's call (it refuses only when a file collides).
        assert_eq!(code(Switch { detached_unique: 0 }, &StatusFacts { unstaged: 3, ..clean() }), "ok");
        assert_eq!(code(Switch { detached_unique: 0 }, &StatusFacts { conflicted: 1, ..clean() }), "unmerged-paths");
        assert_eq!(code(Detach { target_valid: false, detached_unique: 0 }, &clean()), "not-a-commit");
    }

    #[test]
    fn deleting_a_branch_needs_it_to_exist_not_be_current_and_hold_nothing_unique() {
        let del = |is_current, exists, unique_commits, force| DeleteBranch { is_current, exists, unique_commits, force };
        assert_eq!(code(del(false, false, 0, false), &clean()), "no-such-branch");
        assert_eq!(code(del(true, true, 0, false), &clean()), "current-branch");
        // Current wins over unmerged: force would not help.
        assert_eq!(code(del(true, true, 3, true), &clean()), "current-branch");
        let r = check(&Intent::Tree(del(false, true, 1, false)), &clean()).unwrap();
        assert_eq!(r.code, "unmerged-branch");
        assert!(r.message.contains("1 commit") && r.message.contains("never touched"));
        assert_eq!(code(del(false, true, 1, true), &clean()), "ok");
        assert_eq!(code(del(false, true, 0, false), &clean()), "ok");
        assert_eq!(unmerged_branch_message("feat", 4), "Deleting `feat` would drop 4 commit(s) that no other branch holds. Confirm to delete anyway — the remote branch, if any, is never touched.");
        assert_eq!(code(RenameBranch { exists: false }, &clean()), "no-such-branch");
        assert_eq!(code(RenameBranch { exists: true }, &clean()), "ok");
    }

    #[test]
    fn revert_needs_a_branch_a_clean_index_the_right_identity_and_a_mainline_for_merges() {
        let plain = Revert { target_valid: true, is_merge: false, mainline_given: false };
        assert_eq!(code(plain.clone(), &clean()), "ok");
        assert_eq!(code(plain.clone(), &StatusFacts { detached: true, ..clean() }), "detached-head");
        assert_eq!(code(plain.clone(), &StatusFacts { unborn: true, ..clean() }), "unborn-head");
        assert_eq!(code(Revert { target_valid: false, is_merge: false, mainline_given: false }, &clean()), "not-a-commit");
        let r = check(&Intent::Tree(plain.clone()), &StatusFacts { staged: 1, ..clean() }).unwrap();
        assert_eq!(r.code, "staged-changes");
        assert!(r.message.contains("Revert needs the staging area"));
        // Unstaged edits are allowed: git refuses only when a file collides.
        assert_eq!(code(plain.clone(), &StatusFacts { unstaged: 2, ..clean() }), "ok");
        let r = check(&Intent::Tree(plain.clone()), &StatusFacts { identity_matches: false, identity_email: "me@home".into(), profile_email: Some("me@work".into()), email_scope: "local".into(), ..clean() }).unwrap();
        assert_eq!(r.code, "identity-mismatch");
        assert!(r.message.contains("me@work") && r.message.contains(".git/config") && r.message.contains("revert"));
        assert_eq!(code(Revert { is_merge: true, mainline_given: false, target_valid: true }, &clean()), "merge-commit-needs-mainline");
        assert_eq!(code(Revert { is_merge: true, mainline_given: true, target_valid: true }, &clean()), "ok");
        assert_eq!(code(plain, &StatusFacts { conflicted: 1, ..clean() }), "unmerged-paths");
        let msg = mainline_message(&[CommitRef { oid: "a".into(), short: "aaaa".into(), subject: "mine".into() }, CommitRef { oid: "b".into(), short: "bbbb".into(), subject: "theirs".into() }]);
        assert_eq!(msg, "That's a merge commit. Choose which side to keep: 1 keeps `aaaa mine` and undoes what came in from `bbbb theirs`.");
    }

    #[test]
    fn cherry_pick_refuses_contained_and_merge_commits() {
        let pick = |is_merge, already_contained| CherryPick { target_valid: true, is_merge, already_contained };
        assert_eq!(code(pick(false, false), &clean()), "ok");
        assert_eq!(code(pick(false, true), &clean()), "already-contained");
        let r = check(&Intent::Tree(pick(true, false)), &clean()).unwrap();
        assert_eq!(r.code, "merge-commit");
        assert!(r.message.contains("mainline"));
        assert_eq!(code(pick(false, false), &StatusFacts { detached: true, ..clean() }), "detached-head");
        assert_eq!(code(pick(false, false), &StatusFacts { staged: 1, ..clean() }), "staged-changes");
        assert_eq!(code(CherryPick { target_valid: false, is_merge: false, already_contained: false }, &clean()), "not-a-commit");
    }

    #[test]
    fn every_tree_operation_but_resolution_waits_for_an_operation_in_progress() {
        let busy = StatusFacts { operation: Some("merge".into()), abort_command: Some("git merge --abort".into()), conflicted: 1, ..clean() };
        for intent in [
            Switch { detached_unique: 0 },
            CreateBranch { switch_to: true, from_valid: true, detached_unique: 0 },
            DeleteBranch { is_current: false, exists: true, unique_commits: 0, force: false },
            RenameBranch { exists: true },
            UndoCommit { head_is_root: false, head_is_merge: false },
            reset(ResetMode::Soft, false, false),
            Detach { target_valid: true, detached_unique: 0 },
            Revert { target_valid: true, is_merge: false, mainline_given: false },
            CherryPick { target_valid: true, is_merge: false, already_contained: false },
            DiscardAll { include_untracked: true, stash_first: false },
        ] {
            let r = check(&Intent::Tree(intent.clone()), &busy).unwrap();
            assert_eq!(r.code, "operation-in-progress", "{:?}", intent);
        }
        // Discard everything points at Abort, which undoes the whole merge.
        let r = check(&Intent::Tree(DiscardAll { include_untracked: false, stash_first: false }), &busy).unwrap();
        assert!(r.message.contains("Abort the merge instead"));
        // Creating a branch without switching only writes a ref.
        assert_eq!(code(CreateBranch { switch_to: false, from_valid: true, detached_unique: 0 }, &busy), "ok");
        assert_eq!(code(ResolveSide { has_paths: true, submodule_selected: false, not_conflicted_selected: false }, &busy), "ok");
    }

    #[test]
    fn resolving_a_side_refuses_nothing_submodules_and_stale_lists() {
        let f = StatusFacts { conflicted: 2, operation: Some("rebase".into()), ..clean() };
        assert_eq!(code(ResolveSide { has_paths: false, submodule_selected: false, not_conflicted_selected: false }, &f), "no-paths");
        assert_eq!(code(ResolveSide { has_paths: true, submodule_selected: true, not_conflicted_selected: false }, &f), "submodule-conflict");
        assert_eq!(code(ResolveSide { has_paths: true, submodule_selected: false, not_conflicted_selected: true }, &f), "not-conflicted");
        // Allowed while a sync is paused — that is exactly when it is needed.
        assert_eq!(code(ResolveSide { has_paths: true, submodule_selected: false, not_conflicted_selected: false }, &StatusFacts { sync_in_progress: true, ..f }), "ok");
    }

    #[test]
    fn discard_all_refuses_a_clean_tree_but_counts_untracked_only_when_asked() {
        assert_eq!(code(DiscardAll { include_untracked: false, stash_first: false }, &clean()), "nothing-to-discard");
        let only_untracked = StatusFacts { untracked: 2, ..clean() };
        assert_eq!(code(DiscardAll { include_untracked: false, stash_first: false }, &only_untracked), "nothing-to-discard");
        assert_eq!(code(DiscardAll { include_untracked: true, stash_first: false }, &only_untracked), "ok");
        // Leftover stash conflicts with no operation can be discarded.
        assert_eq!(code(DiscardAll { include_untracked: false, stash_first: false }, &StatusFacts { conflicted: 1, ..clean() }), "ok");
    }

    #[test]
    fn mine_and_theirs_swap_flags_during_a_rebase() {
        assert_eq!(side_flag(None, Side::Mine), "--ours");
        assert_eq!(side_flag(None, Side::Theirs), "--theirs");
        assert_eq!(side_flag(Some("merge"), Side::Mine), "--ours");
        assert_eq!(side_flag(Some("cherry-pick"), Side::Theirs), "--theirs");
        for k in ["rebase", "rebase-interactive", "am"] {
            assert_eq!(side_flag(Some(k), Side::Mine), "--theirs", "{}", k);
            assert_eq!(side_flag(Some(k), Side::Theirs), "--ours", "{}", k);
        }
        let note = mapping_note(Some("rebase"), Side::Mine, "--theirs");
        assert!(note.contains("During a rebase") && note.contains("Keep mine ran `git checkout --theirs`"));
        let plain = mapping_note(Some("merge"), Side::Theirs, "--theirs");
        assert_eq!(plain, "Take theirs ran `git checkout --theirs`.");
    }

    #[test]
    fn rev_text_is_bounded_and_never_an_option() {
        for good in ["HEAD~1", "abc1234", "origin/main", "@{u}", "v1.2.0", "main^2", "HEAD@{yesterday}"] {
            assert!(validate_rev_text(good).is_ok(), "{}", good);
        }
        for bad in ["", "-x", "--output=/tmp/x", "two words", "tab\there", "a\0b", &"a".repeat(257)] {
            assert!(validate_rev_text(bad).is_err(), "{:?}", bad);
        }
        assert!(validate_rev_text(&"a".repeat(256)).is_ok());
    }

    #[test]
    fn recovery_uses_the_same_mode_against_the_backup_or_the_old_head() {
        let backup = "gitswitch-before-reset-20260924010203";
        assert_eq!(reset_recovery(ResetMode::Hard, "/r/x", Some(backup), "old"), format!("git -C /r/x reset --hard {}", backup));
        assert_eq!(reset_recovery(ResetMode::Soft, "/r/x", Some(backup), "old"), format!("git -C /r/x reset --soft {}", backup));
        assert_eq!(reset_recovery(ResetMode::Mixed, "/r/x", Some(backup), "old"), format!("git -C /r/x reset --mixed {}", backup));
        // Paths with spaces are quoted for the shell.
        assert_eq!(reset_recovery(ResetMode::Hard, "/r/my repo", Some(backup), "old"), format!("git -C '/r/my repo' reset --hard {}", backup));
        // Without a backup nothing left the branch: the old HEAD is enough.
        assert_eq!(reset_recovery(ResetMode::Hard, "/r/x", None, "old"), "git reset --hard old");
        assert_eq!(reset_recovery(ResetMode::Mixed, "/r/x", None, "old"), "git reset --mixed old");
    }

    #[test]
    fn modes_and_sides_parse_from_exactly_their_words() {
        assert_eq!(ResetMode::parse("hard").unwrap(), ResetMode::Hard);
        assert!(ResetMode::parse("--hard").is_err());
        assert_eq!(Side::parse("mine").unwrap(), Side::Mine);
        assert!(Side::parse("ours").is_err(), "git's words are never accepted from the UI");
        assert_eq!(serde_json::to_string(&ResetMode::Mixed).unwrap(), "\"mixed\"");
    }

    #[test]
    fn log_records_are_split_on_the_unit_separator() {
        let refs = parse_refs("aaaa\x1fa\x1fsubject one\nbbbb\x1fb\x1fsubject: with\x1fseparator\n");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].subject, "subject one");
        assert_eq!(refs[1].short, "b");
        assert_eq!(refs[0].label(), "a subject one");
    }
}
