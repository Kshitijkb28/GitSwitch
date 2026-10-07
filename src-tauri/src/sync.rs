//! "Take the latest, keep my commits on top" — for any repository, submodules
//! included.
//!
//! A plain rebase pull either refuses (dirty tree, submodule pointer moved on
//! both sides) or leaves the submodules stale. This module does what a careful
//! person does by hand: fetch, assess, back up, rebase the superproject with
//! autostash, resolve each submodule-pointer stop by rebasing that submodule's
//! own commits onto the commit upstream recorded, fast-forward the rest,
//! verify — and refuse, by name, every state it cannot handle safely.
//!
//! Every decision is a pure function of measured facts (`plan`), every git
//! command is pinned (`GitCmd::pinned`) so a user's config cannot change what
//! it means, and progress lives in a state file inside the repository's git
//! directory so a paused sync survives page switches and app restarts.
//! Facts behind the choices are recorded in `scripts/verify/sync-facts.md`.

use crate::error::AppError;
use crate::git_exec::{GitCmd, LOCAL_TIMEOUT, NET_TIMEOUT};
use crate::git_ops::{check, Intent, OpResult, Refusal, StatusFacts};
use crate::git_status::{self, RepoStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const BACKUP_PREFIX: &str = "gitswitch-before-sync-";
const STATE_FILE: &str = "gitswitch-sync.json";
const STATE_VERSION: u32 = 1;
const MAX_LISTED: usize = 30;

// ---------------------------------------------------------------------------
// Facts — measured, then handed to the pure planner
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CommitRef {
    pub oid: String,
    pub short: String,
    pub subject: String,
    /// Submodule paths whose pointer this commit moves.
    #[serde(default)]
    pub touches: Vec<String>,
    #[serde(default)]
    pub author_name: String,
    #[serde(default)]
    pub author_email: String,
    /// The author date, ISO 8601 — stable across the rebases an update performs.
    #[serde(default)]
    pub date: String,
    /// The commit changes nothing but submodule pointers (a CI bump).
    #[serde(default)]
    pub only_pointers: bool,
    /// Upstream already carries this change (`git cherry`): the rebase drops it.
    #[serde(default)]
    pub already_upstream: bool,
    /// In an outcome: left out because nothing of it remained once its
    /// conflict was resolved (the resolution matched what was already there).
    #[serde(default)]
    pub resolved_away: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SuperFacts {
    pub branch: String,
    pub upstream: String,
    pub upstream_ref: String,
    pub remote: String,
    pub head: String,
    pub upstream_oid: String,
    pub base_oid: String,
    pub ahead: usize,
    pub behind: usize,
    pub own_commits: Vec<CommitRef>,
    pub incoming: Vec<CommitRef>,
    pub file_overlap: Vec<String>,
    pub dirty_tracked: Vec<String>,
    pub untracked: usize,
    pub dirty_overlap: Vec<String>,
    pub untracked_overlap: Vec<String>,
    pub is_submodule_itself: bool,
    /// Merge commits among my own: a rebase replays their changes in a line
    /// and does not keep the merges themselves.
    #[serde(default)]
    pub own_merges: usize,
    /// Commits of mine upstream already carries that are older than the
    /// listed ones: the preview counts them, the result does not call them
    /// unforeseen.
    #[serde(default)]
    pub unlisted_already_upstream: Vec<String>,
    /// git here is set to commit as someone other than the folder's profile.
    /// Filled in by sync_plan / sync_run from the repository status.
    #[serde(default)]
    pub identity_mismatch: bool,
    #[serde(default)]
    pub identity_email: String,
    #[serde(default)]
    pub profile_email: Option<String>,
    /// Submodules upstream's tree adds that its .gitmodules declares: the run
    /// downloads them after the rebase, as `git submodule update --init` would.
    #[serde(default)]
    pub new_submodules: Vec<String>,
    /// Gitlinks upstream adds without a .gitmodules entry: nothing can fetch them.
    #[serde(default)]
    pub new_unmapped: Vec<String>,
}

/// A submodule's address as upstream's `.gitmodules` now declares it (`to`),
/// when that is not the one it fetches from (`from`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UrlChange {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SubFacts {
    pub path: String,
    pub name: Option<String>,
    pub mapped: bool,
    pub populated: bool,
    pub gitdir_valid: bool,
    pub head: Option<String>,
    pub recorded: String,
    /// What upstream's superproject records. `None` when upstream removed it
    /// — or never had it: `base_link` tells the two apart.
    pub target: Option<String>,
    pub base_link: Option<String>,
    pub target_present: bool,
    pub head_is_ancestor_of_target: bool,
    pub target_is_ancestor_of_head: bool,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub remote: Option<String>,
    /// The branch `refs/remotes/<remote>/HEAD` points at, when it exists.
    pub remote_head_branch: Option<String>,
    pub tip_ahead_of_target: bool,
    pub own_commits_moving_gitlink: usize,
    pub my_commits_move_gitlink: bool,
    pub upstream_moves_gitlink: bool,
    /// Several commits of mine move this pointer, and every one of them points
    /// at a plain commit on its checked-out line, past where we parted from
    /// upstream — so each can record the rewritten copy of what it pointed at.
    #[serde(default)]
    pub own_pointers_on_line: bool,
    /// My branch records a commit that is not ahead of the one we both
    /// started from: the pointer was moved back, or onto another line.
    #[serde(default)]
    pub pointer_rewound: bool,
    /// What the index holds for it. Different from `recorded` (the commit's)
    /// when a pointer change is staged and not committed.
    #[serde(default)]
    pub index_pointer: String,
    pub dirty_tracked: usize,
    pub untracked: usize,
    pub dirty_overlap: Vec<String>,
    /// Untracked files inside it that the commits it would be moved onto add:
    /// git refuses to overwrite them, so the move would fail half-way.
    #[serde(default)]
    pub untracked_overlap: Vec<String>,
    pub conflicted: usize,
    pub operation: Option<String>,
    pub fetch_error: Option<String>,
    pub has_nested: bool,
    /// Upstream's .gitmodules declares another address for it than the one it
    /// fetches from, and that declaration changed since we parted: it was
    /// fetched from the new one, and the run re-points it.
    #[serde(default)]
    pub url_change: Option<UrlChange>,
    /// It fetches from another address than .gitmodules declares, by this
    /// checkout's own choice (upstream did not move it): left as it is.
    #[serde(default)]
    pub url_differs: Option<UrlChange>,
    /// Commits inside it that the update brings in (`head..target`) and that
    /// are its own (`target..head`), each capped at MAX_LISTED.
    #[serde(default)]
    pub incoming_commits: Vec<CommitRef>,
    #[serde(default)]
    pub own_commits: Vec<CommitRef>,
    /// How many commits of its own it carries past the target — not capped.
    #[serde(default)]
    pub own_total: usize,
    /// Read only for a submodule that would be rebased: its commits are re-written.
    #[serde(default)]
    pub identity_mismatch: bool,
    #[serde(default)]
    pub identity_email: String,
    #[serde(default)]
    pub profile_email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skipped {
    pub path: String,
    /// "unmapped" | "not-initialised"
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SyncFacts {
    pub superproject: SuperFacts,
    pub submodules: Vec<SubFacts>,
    pub skipped: Vec<Skipped>,
}

// ---------------------------------------------------------------------------
// The plan — what will happen, or why it cannot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubPlan {
    pub path: String,
    pub name: Option<String>,
    /// none | fast-forward | rebase | ahead | blocked
    pub action: String,
    pub reason: String,
    pub head: Option<String>,
    pub recorded: String,
    pub target: Option<String>,
    pub rebase_onto: Option<String>,
    pub predicted_gitlink_conflict: bool,
    pub branch: Option<String>,
    pub detached: bool,
    /// Detached only: the branch it is re-attached to after its rebase when
    /// that branch stood where the checkout was (named from the remote's HEAD).
    pub reattach_to: Option<String>,
    pub dirty_tracked: usize,
    pub untracked: usize,
    pub fetch_error: Option<String>,
    pub notes: Vec<String>,
    /// A stable code for the blocker's remedy, when blocked: reinit |
    /// finish-operation | resolve-conflicts | removed-upstream | fetch-failed |
    /// unreachable | record-pointers | squash-own | stash-inside. The UI keys
    /// its buttons on this, never on the sentence.
    #[serde(default)]
    pub remedy: Option<String>,
    /// What the update brings into this submodule, and what is its own.
    #[serde(default)]
    pub incoming_commits: Vec<CommitRef>,
    #[serde(default)]
    pub own_commits: Vec<CommitRef>,
    /// It has submodules of its own; the update does not go inside them.
    #[serde(default)]
    pub has_nested: bool,
    /// Upstream moved it to another address (shown with any token masked);
    /// the update re-points it there before aligning it.
    #[serde(default)]
    pub url_change: Option<UrlChange>,
    /// The remote its branch's upstream lives on.
    #[serde(default)]
    pub remote: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SuperPlan {
    pub branch: String,
    pub upstream: String,
    /// The remote the update fetches from.
    #[serde(default)]
    pub remote: String,
    pub head: String,
    pub upstream_oid: String,
    pub ahead: usize,
    pub behind: usize,
    pub own_commits: Vec<CommitRef>,
    pub incoming: Vec<CommitRef>,
    pub file_overlap: Vec<String>,
    pub dirty_count: usize,
    pub untracked: usize,
    pub will_stash: bool,
    pub notes: Vec<String>,
    /// Stable codes for the superproject's blockers, in their order:
    /// turn-on-stash | stash-collides | untracked-in-the-way.
    #[serde(default)]
    pub remedies: Vec<String>,
    /// Where you and upstream last agreed, shortened.
    #[serde(default)]
    pub base_short: String,
    /// The first MAX_LISTED files with uncommitted changes.
    #[serde(default)]
    pub dirty_paths: Vec<String>,
    /// Commits beyond the MAX_LISTED shown on each side.
    #[serde(default)]
    pub more_incoming: usize,
    #[serde(default)]
    pub more_own: usize,
    /// Of the commits of mine beyond the listed ones, how many upstream
    /// already carries (and which): they are dropped too.
    #[serde(default)]
    pub more_own_dropped: usize,
    #[serde(default)]
    pub unlisted_dropped: Vec<String>,
    /// Submodules upstream added that the run downloads after the rebase, and
    /// gitlinks it added without a .gitmodules entry (which nothing can fetch).
    #[serde(default)]
    pub new_submodules: Vec<String>,
    #[serde(default)]
    pub new_unmapped: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SyncPlan {
    pub can_run: bool,
    pub nothing_to_do: bool,
    pub refusal: Option<Refusal>,
    pub blockers: Vec<String>,
    pub superproject: SuperPlan,
    pub submodules: Vec<SubPlan>,
    pub skipped: Vec<Skipped>,
    /// Oids the run re-checks before touching anything: "" for the
    /// superproject's upstream, then one per submodule path (its target).
    pub fingerprint: Vec<(String, String)>,
    pub summary: String,
    /// The superproject HEAD the plan was made for; the page drops a plan
    /// for any other.
    #[serde(default)]
    pub head_oid: String,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {}", one)
    } else {
        format!("{} {}", n, many)
    }
}

/// Pure: facts in, plan out. Every ambiguous state is a named blocker.
pub fn plan(facts: &SyncFacts, stash: bool) -> SyncPlan {
    let s = &facts.superproject;
    let mut blockers: Vec<String> = Vec::new();
    let mut remedies: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    let dirty = !s.dirty_tracked.is_empty();
    // Level with upstream already (only submodules need aligning): the
    // superproject's own tree is not touched, so its edits need no stash.
    if dirty && !stash && s.behind > 0 {
        blockers.push(format!(
            "You have {} with uncommitted changes. Turn on stashing for the update, or commit or stash them yourself first.",
            plural(s.dirty_tracked.len(), "file", "files")
        ));
        remedies.push("turn-on-stash".into());
    }
    if dirty && stash && !s.dirty_overlap.is_empty() {
        blockers.push(format!(
            "Your uncommitted changes to {} collide with incoming commits; a stash could not be re-applied cleanly. Commit or stash them yourself first.",
            s.dirty_overlap.join(", ")
        ));
        remedies.push("stash-collides".into());
    }
    if !s.untracked_overlap.is_empty() {
        blockers.push(format!(
            "Untracked {} would be overwritten by incoming commits: {}. Move or remove them first.",
            if s.untracked_overlap.len() == 1 { "file" } else { "files" },
            some_of(&s.untracked_overlap)
        ));
        remedies.push("untracked-in-the-way".into());
    }
    if !s.file_overlap.is_empty() {
        notes.push(format!(
            "Your commits and the incoming ones both touch {}; the rebase may stop there for you to resolve.",
            s.file_overlap.join(", ")
        ));
    }
    if s.is_submodule_itself {
        notes.push("This repository is itself a submodule; its parent will need a pointer update afterwards.".into());
    }
    if s.own_merges > 0 && s.behind > 0 {
        notes.push(format!(
            "{} of your commits {}: the update replays what they changed in a straight line, so the merge {} not kept.",
            s.own_merges,
            if s.own_merges == 1 { "is a merge" } else { "are merges" },
            if s.own_merges == 1 { "commit itself is" } else { "commits themselves are" }
        ));
    }
    if !s.new_submodules.is_empty() {
        notes.push(format!(
            "upstream added {}: {} — downloaded after the rebase, at the commit it records (git submodule update --init).",
            plural(s.new_submodules.len(), "submodule", "submodules"),
            s.new_submodules.join(", ")
        ));
    }
    if !s.new_unmapped.is_empty() {
        notes.push(format!(
            "upstream added {} without an entry in .gitmodules: {} — git cannot fetch {}, so {} empty.",
            plural(s.new_unmapped.len(), "submodule", "submodules"),
            s.new_unmapped.join(", "),
            if s.new_unmapped.len() == 1 { "it" } else { "them" },
            if s.new_unmapped.len() == 1 { "it stays" } else { "they stay" }
        ));
    }

    let mut subs = Vec::new();
    for f in &facts.submodules {
        let mut p = SubPlan {
            path: f.path.clone(),
            name: f.name.clone(),
            action: "none".into(),
            reason: String::new(),
            head: f.head.clone(),
            recorded: f.recorded.clone(),
            target: f.target.clone(),
            rebase_onto: None,
            predicted_gitlink_conflict: false,
            branch: f.branch.clone(),
            detached: f.populated && f.branch.is_none(),
            reattach_to: if f.populated && f.branch.is_none() { f.remote_head_branch.clone() } else { None },
            dirty_tracked: f.dirty_tracked,
            untracked: f.untracked,
            fetch_error: f.fetch_error.clone(),
            notes: Vec::new(),
            remedy: None,
            incoming_commits: f.incoming_commits.clone(),
            own_commits: f.own_commits.clone(),
            has_nested: f.has_nested,
            url_change: f.url_change.as_ref().map(|u| UrlChange { from: crate::repo_scan::mask_token(&u.from), to: crate::repo_scan::mask_token(&u.to) }),
            remote: f.remote.clone(),
        };
        if let Some(u) = &p.url_change {
            p.notes.push(format!(
                "upstream moved it: it now lives at {} (it fetched from {} until now); the update re-points it there, as `git submodule sync` does, before aligning it",
                u.to, u.from
            ));
        }
        if let Some(u) = &f.url_differs {
            p.notes.push(format!(
                "it fetches from {} while .gitmodules declares {}; upstream did not move it, so the update leaves that as it is",
                crate::repo_scan::mask_token(&u.from),
                crate::repo_scan::mask_token(&u.to)
            ));
        }
        let block = |p: &mut SubPlan, code: &str, why: String| {
            p.action = "blocked".into();
            p.remedy = Some(code.into());
            p.reason = why;
        };
        let head = f.head.clone().unwrap_or_default();
        let pointer_moves = f.my_commits_move_gitlink || f.upstream_moves_gitlink;
        if !f.gitdir_valid {
            block(&mut p, "reinit", "its .git points at a directory that no longer exists; re-initialise it (git submodule update --init) first".into());
        } else if let Some(op) = &f.operation {
            block(&mut p, "finish-operation", format!("a {} is in progress inside it; finish or abort that first", op));
        } else if f.conflicted > 0 {
            block(&mut p, "resolve-conflicts", format!("{} inside it still {} conflicts", plural(f.conflicted, "file", "files"), if f.conflicted == 1 { "has" } else { "have" }));
        } else if !f.index_pointer.is_empty() && !same_oid(&f.index_pointer, &f.recorded) {
            // A pointer change that is staged and not committed is work in
            // progress the update would walk over: the rebase refuses a dirty
            // index, or stashes it and brings it back in the way.
            block(&mut p, "commit-first", format!(
                "its pointer is staged at {} but this branch records {}; commit it or unstage it before updating",
                short(&f.index_pointer), short(&f.recorded)
            ));
        } else if f.target.is_none() && f.base_link.is_none() && !s.base_oid.is_empty() {
            // Neither upstream nor the commit we both started from has it:
            // it was added here. There is nothing to move it to and nothing
            // upstream could conflict with.
            if f.own_commits_moving_gitlink > 0 && f.populated && !head.is_empty() && !same_oid(&head, &f.recorded) {
                // Not where my commit records it: the run would move it there
                // (or rebase it) although nothing here was planned for it.
                block(&mut p, "record-pointers", format!(
                    "it is checked out at {} but this branch records {}; commit that pointer (Record submodule pointers) or move it back before updating",
                    short(&head), short(&f.recorded)
                ));
            } else if f.own_commits_moving_gitlink > 0 {
                p.action = "none".into();
                p.reason = "added by your commits; upstream does not have it".into();
            } else {
                block(&mut p, "commit-first", "it was added here but is not committed yet; commit it before updating".into());
            }
        } else if f.target.is_none() {
            block(&mut p, "removed-upstream", "upstream removed this submodule; decide by hand what to do with your copy".into());
        } else if !f.target_present {
            let stale = match &f.url_differs {
                Some(u) => format!(" — it fetches from {} while .gitmodules declares {}; `git submodule sync` re-points it", crate::repo_scan::mask_token(&u.from), crate::repo_scan::mask_token(&u.to)),
                None => String::new(),
            };
            block(&mut p, if f.fetch_error.is_some() { "fetch-failed" } else { "unreachable" }, match &f.fetch_error {
                Some(e) => format!("the commit upstream records ({}) is not available here and fetching failed: {}{}", short(f.target.as_deref().unwrap_or("")), e, stale),
                None => format!("the commit upstream records ({}) is not reachable from its remote's branches{}", short(f.target.as_deref().unwrap_or("")), stale),
            });
        } else if !same_oid(&head, &f.recorded) && pointer_moves {
            block(&mut p, "record-pointers", format!(
                "it is checked out at {} but this branch records {}; commit that pointer (Record submodule pointers) or move it back before updating",
                short(&head), short(&f.recorded)
            ));
        } else if f.own_commits_moving_gitlink > 1 && f.upstream_moves_gitlink && !f.own_pointers_on_line {
            // Only when upstream moved the pointer too: otherwise every one
            // of my pointer commits replays onto the pointer it started from
            // and the rebase cannot stop on it. With all of them along the
            // checked-out line the update keeps them one by one (below).
            block(&mut p, "squash-own", format!(
                "{} of your commits move this submodule's pointer and not every one points at a commit on its checked-out line; squash them, or do this one by hand",
                f.own_commits_moving_gitlink
            ));
        } else {
            let target = f.target.clone().unwrap_or_default();
            if let Some(e) = &f.fetch_error {
                p.notes.push(format!("its remote could not be fetched ({}); the recorded commit is available locally", e));
            }
            if f.tip_ahead_of_target {
                p.notes.push("its remote has newer commits than upstream's superproject records; the update targets what is recorded".into());
            }
            if same_oid(&head, &target) {
                p.action = "none".into();
                p.reason = "already at the commit upstream records".into();
            } else if f.base_link.is_none() && f.own_commits_moving_gitlink > 0 {
                // Both sides added it, at different commits: git cannot pick
                // one and stops there. Which commit it records is not the
                // update's to decide — it pauses and hands the pointer over.
                p.action = "none".into();
                p.predicted_gitlink_conflict = true;
                p.reason = format!(
                    "added by your commits at {} and by upstream at {}; the update pauses there for you to choose which it records",
                    short(&f.recorded), short(&target)
                );
            } else if f.my_commits_move_gitlink && f.own_commits_moving_gitlink > 0 && f.upstream_moves_gitlink && f.pointer_rewound && f.head_is_ancestor_of_target {
                // My commit moves the pointer back while upstream moved it
                // forward: git cannot keep both and stops on it. Moving the
                // submodule forward would quietly undo what my commit says, so
                // the update pauses and hands the pointer over instead.
                p.action = "none".into();
                p.predicted_gitlink_conflict = true;
                p.reason = format!(
                    "your commit moves its pointer back to {} while upstream moved it forward to {}; the update pauses there for you to choose which it records",
                    short(&f.recorded), short(&target)
                );
            } else if f.my_commits_move_gitlink && f.own_commits_moving_gitlink > 0 && !f.upstream_moves_gitlink && !f.target_is_ancestor_of_head {
                // Only my commits move this pointer: the rebase keeps where I
                // put it, and the checkout is already there.
                p.action = "none".into();
                p.reason = format!("your commits put its pointer at {}; upstream did not move it, so it stays there", short(&head));
            } else if f.head_is_ancestor_of_target {
                if !f.dirty_overlap.is_empty() {
                    // Said now, not at align time: a fast-forward over an
                    // edited file would fail there and leave a half-done update.
                    block(&mut p, "stash-inside", format!(
                        "its uncommitted changes to {} collide with the commits it will be moved onto; commit or stash them inside it first",
                        f.dirty_overlap.join(", ")
                    ));
                } else if !f.untracked_overlap.is_empty() {
                    // The same, for files git does not track yet: it will not
                    // overwrite them, and says so only when it gets there.
                    block(&mut p, "untracked-inside", format!(
                        "untracked {} inside it would be overwritten by the commits it will be moved onto: {}; move or remove them first",
                        if f.untracked_overlap.len() == 1 { "file" } else { "files" },
                        some_of(&f.untracked_overlap)
                    ));
                } else {
                    p.action = "fast-forward".into();
                    p.reason = format!("moves forward to {} (no commits of yours)", short(&target));
                }
            } else if f.target_is_ancestor_of_head {
                if f.my_commits_move_gitlink && f.own_commits_moving_gitlink > 0 {
                    p.action = "none".into();
                    p.reason = format!("already contains {}; your commit's pointer will be kept", short(&target));
                } else {
                    p.action = "ahead".into();
                    p.reason = format!("your commits sit on top of {} already; the superproject does not record them yet", short(&target));
                }
            } else {
                // Diverged: rebase this submodule's own commits onto the recorded target.
                if !f.dirty_overlap.is_empty() {
                    block(&mut p, "stash-inside", format!(
                        "its uncommitted changes to {} collide with the commits it will be rebased onto; commit or stash them inside it first",
                        f.dirty_overlap.join(", ")
                    ));
                } else if !f.untracked_overlap.is_empty() {
                    block(&mut p, "untracked-inside", format!(
                        "untracked {} inside it would be overwritten by the commits it will be rebased onto: {}; move or remove them first",
                        if f.untracked_overlap.len() == 1 { "file" } else { "files" },
                        some_of(&f.untracked_overlap)
                    ));
                } else if f.identity_mismatch {
                    block(&mut p, "identity", format!(
                        "rebasing would re-write its commits as {} but this folder should commit as {}; fix it in Doctor first",
                        f.identity_email,
                        f.profile_email.clone().unwrap_or_default()
                    ));
                } else {
                    p.action = "rebase".into();
                    p.rebase_onto = Some(target.clone());
                    p.predicted_gitlink_conflict = f.my_commits_move_gitlink && f.upstream_moves_gitlink;
                    p.reason = format!(
                        "your {} rebased onto {}{}",
                        plural(f.own_total.max(f.own_commits.len()).max(1), "commit is", "commits are"),
                        short(&target),
                        if p.predicted_gitlink_conflict { " when the superproject rebase stops on its pointer" } else { " after the superproject rebase" }
                    );
                    if p.detached {
                        p.notes.push(match &f.remote_head_branch {
                            Some(b) => format!("it is detached; it is re-attached to '{}' afterwards when that branch was where it stood", b),
                            None => "it is detached and stays detached (no default branch is known for its remote)".into(),
                        });
                    }
                    if f.dirty_tracked > 0 {
                        p.notes.push(format!("its {} stashed and re-applied around the rebase", plural(f.dirty_tracked, "uncommitted change is", "uncommitted changes are")));
                    }
                    if f.own_commits_moving_gitlink > 1 {
                        p.notes.push(format!(
                            "{} commits of yours move its pointer; each will record the rewritten copy of the commit it pointed at",
                            f.own_commits_moving_gitlink
                        ));
                    }
                }
            }
        }
        if p.action == "blocked" {
            blockers.push(format!("{}: {}", f.path, p.reason));
        }
        subs.push(p);
    }

    // Rebasing re-commits your work under whatever identity git has here —
    // the same reason a commit or a rebase pull is refused on a mismatch. It
    // fires only when something of yours here would be re-committed: with
    // nothing of yours to replay the update fast-forwards, and level with
    // upstream it does not rebase the superproject at all. A submodule that
    // would be rebased is checked on its own identity, above.
    if s.identity_mismatch && s.ahead > 0 && s.behind > 0 {
        blockers.insert(
            remedies.len(),
            format!(
                "This folder should commit as {} but git is set to {}. The update would re-write your commits under that identity — fix it in Doctor first.",
                s.profile_email.clone().unwrap_or_default(),
                s.identity_email
            ),
        );
        remedies.push("identity".into());
    }
    let submodule_work = subs.iter().any(|p| matches!(p.action.as_str(), "fast-forward" | "rebase"));
    let nothing_to_do = s.behind == 0 && !submodule_work && blockers.is_empty();
    let can_run = blockers.is_empty() && !nothing_to_do;
    let mut fingerprint = vec![(String::new(), s.upstream_oid.clone())];
    for f in &facts.submodules {
        if let Some(t) = &f.target {
            fingerprint.push((f.path.clone(), t.clone()));
        }
    }
    let summary = if nothing_to_do {
        format!("Nothing to update — you are level with {} and every submodule is where it should be.", s.upstream)
    } else if !blockers.is_empty() {
        format!("Cannot update yet: {} attention first.", plural(blockers.len(), "thing needs", "things need"))
    } else if s.behind == 0 {
        "Only submodules need aligning.".into()
    } else {
        format!(
            "Your {} replayed on top of {} from {}{}.",
            plural(s.ahead, "commit is", "commits are"),
            plural(s.behind, "new commit", "new commits"),
            s.upstream,
            if subs.iter().any(|p| p.action == "rebase") { ", rebasing the submodules that carry commits of yours" } else { "" }
        )
    };
    SyncPlan {
        can_run,
        nothing_to_do,
        refusal: None,
        blockers,
        superproject: SuperPlan {
            branch: s.branch.clone(),
            upstream: s.upstream.clone(),
            remote: s.remote.clone(),
            head: s.head.clone(),
            upstream_oid: s.upstream_oid.clone(),
            ahead: s.ahead,
            behind: s.behind,
            own_commits: s.own_commits.clone(),
            incoming: s.incoming.clone(),
            file_overlap: s.file_overlap.clone(),
            dirty_count: s.dirty_tracked.len(),
            untracked: s.untracked,
            will_stash: dirty && stash && s.behind > 0,
            notes,
            remedies,
            base_short: if s.base_oid.is_empty() { String::new() } else { short(&s.base_oid) },
            dirty_paths: s.dirty_tracked.iter().take(MAX_LISTED).cloned().collect(),
            more_incoming: s.behind.saturating_sub(s.incoming.len()),
            more_own: s.ahead.saturating_sub(s.own_commits.len()),
            more_own_dropped: s.unlisted_already_upstream.len(),
            unlisted_dropped: s.unlisted_already_upstream.clone(),
            new_submodules: s.new_submodules.clone(),
            new_unmapped: s.new_unmapped.clone(),
        },
        submodules: subs,
        skipped: facts.skipped.clone(),
        fingerprint,
        summary,
        head_oid: s.head.clone(),
    }
}

pub(crate) fn short(oid: &str) -> String {
    oid.chars().take(7).collect()
}

fn same_oid(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

// ---------------------------------------------------------------------------
// State file — the sync survives page switches and app restarts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubState {
    pub path: String,
    pub branch_before: Option<String>,
    #[serde(default)]
    pub reattach_to: Option<String>,
    pub head_before: String,
    pub backup: Option<String>,
    pub target: String,
    pub rebased: bool,
    pub head_after: Option<String>,
    pub aligned: bool,
    pub note: String,
    /// The top of its stash just before the update rebased it ("" for none):
    /// an "autostash" entry found afterwards is this run's only if the top moved.
    #[serde(default)]
    pub stash_before: Option<String>,
    /// A branch the update moved onto the rebased tip to re-attach a detached
    /// checkout, and where that branch stood before — so an abort puts it back.
    #[serde(default)]
    pub reattached: Option<(String, String)>,
    /// The address upstream's .gitmodules now declares for it, when that is
    /// not the one it fetched from; `url_synced` once the run re-pointed it.
    #[serde(default)]
    pub url_change: Option<UrlChange>,
    #[serde(default)]
    pub url_synced: bool,
    #[serde(default)]
    pub url_note: Option<String>,
    /// The remote its branch's upstream lives on ("origin" when unknown).
    #[serde(default)]
    pub remote: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NeedsUser {
    /// superproject | submodule:<path> | submodule-autostash:<path> | autostash-conflict
    pub where_: String,
    pub paths: Vec<String>,
    pub hint: String,
    /// The commit of yours being replayed when the superproject's rebase stopped.
    #[serde(default)]
    pub at_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncState {
    pub version: u32,
    pub nonce: String,
    pub started_at: String,
    /// preparing | rebasing | aligning | done | aborted
    pub phase: String,
    pub repo: String,
    pub upstream_ref: String,
    pub upstream_oid: String,
    pub head_before: String,
    pub stash: bool,
    pub backup: Option<String>,
    pub captures: BTreeMap<String, Vec<String>>,
    pub subs: Vec<SubState>,
    pub needs_user: Option<NeedsUser>,
    pub plan: SyncPlan,
    pub bundles: Vec<String>,
    /// Commits of yours replayed with help — a conflict you resolved, or a
    /// submodule pointer the update rewrote — so their patch differs from the
    /// original and `git cherry` cannot vouch for them afterwards.
    #[serde(default)]
    pub resolved: Vec<String>,
    /// Commits git left out while replaying because "patch contents already
    /// upstream" — read from what `rebase` printed, so a drop the preview did
    /// not foresee is still told apart from a loss afterwards.
    #[serde(default)]
    pub dropped_by_git: Vec<String>,
    /// Commits of mine whose stop was resolved into nothing: when the rebase
    /// was continued, nothing was staged against HEAD, so git left the commit
    /// out without a word. Measured at that moment, not inferred afterwards.
    #[serde(default)]
    pub emptied: Vec<String>,
    /// The top of the superproject's stash just before its rebase ("" for
    /// none). `None` in a state an older build wrote.
    #[serde(default)]
    pub stash_before: Option<String>,
    /// What git last printed while rebasing here — shown when it stops
    /// without a conflict (a hook, a signing failure), so the reason is on
    /// the page and not only in the command log.
    #[serde(default)]
    pub git_said: String,
    /// Submodule pointers the update handed over for the person to place.
    #[serde(default)]
    pub handed_over: Vec<String>,
    /// Stops whose pointer turned out to be upstream's own (what my commit
    /// pointed at was dropped by the submodule's rebase as a twin): when
    /// nothing else of the commit remains, it was already upstream.
    #[serde(default)]
    pub pointer_upstreams: Vec<String>,
    /// The repositories of this update that use Git LFS — "" for the
    /// superproject, otherwise the submodule's path — each with whether every
    /// one of its large files was present before the update.
    #[serde(default)]
    pub lfs: BTreeMap<String, bool>,
    /// Large files are written as pointer stubs while the update rewrites the
    /// working trees, and downloaded afterwards on their own clock — as Pull
    /// does. Left to itself git downloads them INSIDE the rebase, through the
    /// smudge filter, where a slow or unreachable LFS server fails the whole
    /// update. Off when one of your uncommitted changes is itself a large
    /// file: the autostash would put it back as a stub. `false` in a state an
    /// older build wrote, which then behaves as before.
    #[serde(default)]
    pub lfs_skip: bool,
}

/// `rebase` announces each commit it leaves out as empty: "dropping <oid>
/// <subject> -- patch contents already upstream". Keep the ids — and what it
/// said last, for a stop that is not a conflict.
fn note_drops(st: &mut SyncState, stderr: &str) {
    // git writes its progress with carriage returns — "Rebasing (1/3)\r" and
    // then the message on the same line — and, on a terminal it thinks is
    // smart, an erase-line sequence in front. Split on both, strip those.
    let lines: Vec<String> = stderr
        .split(['\n', '\r'])
        .map(|l| l.replace("\u{1b}[K", "").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let said: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|l| !l.starts_with("hint:") && !l.starts_with("dropping ") && !l.starts_with("Rebasing ("))
        .collect();
    let tail = said[said.len().saturating_sub(4)..].join(" / ");
    st.git_said = if tail.chars().count() > 600 { format!("{}…", tail.chars().take(600).collect::<String>()) } else { tail };
    for l in &lines {
        if !l.ends_with("-- patch contents already upstream") {
            continue;
        }
        if let Some(rest) = l.strip_prefix("dropping ") {
            let oid: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            if oid.len() >= 7 && !st.dropped_by_git.contains(&oid) {
                st.dropped_by_git.push(oid);
            }
        }
    }
}

/// Where the state lives: per worktree, dies with the repo, invisible to git.
async fn state_path(repo: &Path) -> Result<PathBuf, AppError> {
    let p = GitCmd::at(repo)
        .args(["rev-parse", "--path-format=absolute", "--git-path", STATE_FILE])
        .text()
        .await?;
    Ok(PathBuf::from(p.trim()))
}

pub async fn read_state(repo: &Path) -> Option<SyncState> {
    let p = state_path(repo).await.ok()?;
    let text = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_state_at(path: &Path, st: &SyncState) -> Result<(), AppError> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(st)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A summary for `RepoStatus`, also true for a submodule whose superproject
/// is mid-sync.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SyncSummary {
    pub phase: String,
    pub started_at: String,
    pub needs_user: Option<NeedsUser>,
    /// The repository the sync belongs to (the superproject).
    pub root: String,
    /// False when this repository is a submodule of the repository being synced.
    pub is_root: bool,
    /// Conflicted files still unmerged in the repository the pause names —
    /// 0 is when "Continue update" can be pressed.
    pub unresolved: usize,
    /// The superproject's upstream ("origin/main"): what "theirs" means in
    /// every rebase the update runs.
    pub upstream: String,
    /// For a submodule's own status: its path inside the superproject, and the
    /// commit the update rebases it onto (what upstream's superproject records).
    pub sub_path: Option<String>,
    pub sub_target: Option<String>,
    /// The pause is a submodule pointer the update could not place itself and
    /// it is still unmerged here: what is left to do is stage it in this
    /// repository, not resolve anything inside the submodule.
    pub handed_over: bool,
}

pub async fn summary_for(repo: &Path) -> Option<SyncSummary> {
    if let Some(st) = read_state(repo).await {
        // One extra `git status`, only while paused: what the person still
        // has to resolve before the update can go on.
        let mut pointer_left = false;
        let unresolved = match st.needs_user.as_ref().map(|n| n.where_.as_str()) {
            Some(w) if w.starts_with("submodule") => match w.split(':').nth(1).filter(|p| !p.is_empty()) {
                Some(sub) => {
                    let dir = repo.join(sub);
                    // An unpopulated gitlink directory is not a repository of
                    // its own: git asked there would answer for this one.
                    let populated = dir.join(".git").exists();
                    let inside = if populated { Box::pin(unmerged(&dir)).await.len() } else { 0 };
                    // Nothing is being rebased inside it: the pause is a
                    // hand-over ("stage it, then Continue"), and what is left
                    // to do is the pointer, unmerged in this repository.
                    let handed_over = w.starts_with("submodule:") && !(populated && rebase_in_progress(&dir).await);
                    let pointer = if handed_over { Box::pin(unmerged(repo)).await.iter().filter(|c| c.path == sub).count() } else { 0 };
                    pointer_left = pointer > 0;
                    inside + pointer
                }
                None => 0,
            },
            Some(_) => Box::pin(unmerged(repo)).await.len(),
            None => 0,
        };
        let upstream = st.plan.superproject.upstream.clone();
        return Some(SyncSummary {
            phase: st.phase,
            started_at: st.started_at,
            needs_user: st.needs_user,
            root: st.repo,
            is_root: true,
            unresolved,
            upstream,
            sub_path: None,
            sub_target: None,
            handed_over: pointer_left,
        });
    }
    let parent = GitCmd::at(repo)
        .args(["rev-parse", "--show-superproject-working-tree"])
        .ok_text()
        .await
        .filter(|s| !s.is_empty())?;
    let st = read_state(Path::new(&parent)).await?;
    // Which of the superproject's submodules this repository is: the one
    // whose folder is this folder.
    let here = std::fs::canonicalize(repo).ok();
    let mine = st
        .subs
        .iter()
        .find(|s| here.is_some() && std::fs::canonicalize(Path::new(&parent).join(&s.path)).ok() == here);
    let (sub_path, sub_target) = match mine {
        Some(s) => (Some(s.path.clone()), Some(s.target.clone())),
        None => (None, None),
    };
    let unresolved = Box::pin(unmerged(repo)).await.len();
    let upstream = st.plan.superproject.upstream.clone();
    Some(SyncSummary {
        phase: st.phase,
        started_at: st.started_at,
        needs_user: st.needs_user,
        root: st.repo,
        is_root: false,
        unresolved,
        upstream,
        sub_path,
        sub_target,
        handed_over: false,
    })
}

// ---------------------------------------------------------------------------
// Gathering facts
// ---------------------------------------------------------------------------

/// How many remotes are fetched at once while assessing.
const FETCH_FANOUT: usize = 6;

fn g(repo: &Path) -> GitCmd {
    GitCmd::at(repo).pinned()
}

/// A command that rewrites the working tree: with the update's large files
/// written as pointer stubs when `skip_smudge` (see `SyncState::lfs_skip`).
fn rw(repo: &Path, skip_smudge: bool) -> GitCmd {
    let cmd = g(repo);
    if skip_smudge {
        cmd.env("GIT_LFS_SKIP_SMUDGE", "1")
    } else {
        cmd
    }
}

/// Whether any of the repository's uncommitted changes to tracked files is a
/// file Git LFS manages — what an autostash would carry. When git cannot say,
/// the answer is yes: then nothing is written as a stub.
async fn dirty_large_file(repo: &Path) -> bool {
    let Some(changed) = g(repo).args(["diff", "--name-only", "-z", "HEAD"]).ok_text().await else { return false };
    let paths: Vec<&str> = changed.split('\0').filter(|p| !p.is_empty()).collect();
    if paths.is_empty() {
        return false;
    }
    let input = format!("{}\0", paths.join("\0")).into_bytes();
    let Ok(out) = g(repo).args(["check-attr", "-z", "--stdin", "filter"]).stdin_bytes(input).run().await else { return true };
    if !out.ok() {
        return true;
    }
    // `-z`: path NUL attribute NUL value NUL, repeated.
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let fields: Vec<&str> = text.split('\0').collect();
    fields.chunks(3).any(|c| c.len() == 3 && c[2] == "lfs")
}

/// Which repositories of the update use Git LFS, whether each was complete,
/// and whether the update may write pointer stubs (no large file among the
/// uncommitted changes of any of them).
async fn lfs_plan(repo: &Path, subs: &[SubState]) -> (BTreeMap<String, bool>, bool) {
    let mut lfs = BTreeMap::new();
    let mut dirty_large = false;
    let mut repos: Vec<(String, PathBuf)> = vec![(String::new(), repo.to_path_buf())];
    repos.extend(subs.iter().map(|s| (s.path.clone(), repo.join(&s.path))));
    for (key, dir) in repos {
        let Ok(s) = crate::lfs::lfs_status(&dir.to_string_lossy()).await else { continue };
        if s.uses_lfs && s.installed {
            lfs.insert(key, s.pointers == 0);
            if dirty_large_file(&dir).await {
                dirty_large = true;
            }
        }
    }
    let skip = !lfs.is_empty() && !dirty_large;
    (lfs, skip)
}

/// After an update that wrote pointer stubs: where every large file was
/// present before, download what is now missing, so that stays true; where
/// some were left out on purpose, leave them out and say how many are stubs.
/// Sentences with their leading space, one per repository that has news.
async fn lfs_after_update(repo: &Path, st: &SyncState) -> String {
    if !st.lfs_skip {
        return String::new();
    }
    let mut out = String::new();
    for (path, complete_before) in &st.lfs {
        let dir = if path.is_empty() { repo.to_path_buf() } else { repo.join(path) };
        if !dir.is_dir() {
            continue;
        }
        let place = if path.is_empty() { "here".to_string() } else { format!("in {}", path) };
        let dir_s = dir.to_string_lossy().to_string();
        if *complete_before {
            match crate::git_ops::fetch_lfs_content(&dir_s).await {
                Ok(f) => match f.error {
                    Some(a) => out.push_str(&format!(" The large files {} didn't download: {} The LFS card can fetch them.", place, a.headline)),
                    None if f.fetched > 0 => out.push_str(&format!(" Downloaded {} {}.", plural(f.fetched, "LFS file", "LFS files"), place)),
                    None => {}
                },
                Err(e) => out.push_str(&format!(" The large files {} didn't download: {} The LFS card can fetch them.", place, e)),
            }
        } else if let Ok(s) = crate::lfs::lfs_status(&dir_s).await {
            if s.pointers > 0 {
                out.push_str(&format!(
                    " {} {} {} a pointer stub; the LFS card downloads them.",
                    plural(s.pointers, "LFS file", "LFS files"),
                    place,
                    if s.pointers == 1 { "is" } else { "are" }
                ));
            }
        }
    }
    out
}

pub(crate) async fn oid(repo: &Path, rev: &str) -> Option<String> {
    g(repo).args(["rev-parse", "--verify", "--quiet", rev]).ok_text().await.filter(|s| !s.is_empty())
}

pub(crate) async fn is_ancestor(repo: &Path, a: &str, b: &str) -> bool {
    g(repo).args(["merge-base", "--is-ancestor", a, b]).run().await.map(|o| o.ok()).unwrap_or(false)
}

async fn commit_list(repo: &Path, range: &str) -> Vec<CommitRef> {
    g(repo)
        .args(["log", &format!("-n{}", MAX_LISTED), "--format=%H%x1f%h%x1f%s%x1f%an%x1f%ae%x1f%aI", range])
        .ok_text()
        .await
        .map(|out| {
            out.lines()
                .filter_map(|l| {
                    let mut it = l.split('\x1f');
                    Some(CommitRef {
                        oid: it.next()?.to_string(),
                        short: it.next()?.to_string(),
                        subject: it.next().unwrap_or("").to_string(),
                        touches: Vec::new(),
                        author_name: it.next().unwrap_or("").to_string(),
                        author_email: it.next().unwrap_or("").to_string(),
                        date: it.next().unwrap_or("").to_string(),
                        only_pointers: false,
                        already_upstream: false,
                        resolved_away: false,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The files each commit in `range` touches — one git call for the whole
/// range, so a commit can be told apart from a submodule-pointer bump.
async fn files_per_commit(repo: &Path, range: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out = BTreeMap::new();
    // -z and --ignore-submodules=none, for the reasons the history page has:
    // names as they are, and a pointer change listed whatever the config says.
    let Some(text) = g(repo)
        .args(["log", "--no-show-signature", "--no-renames", &format!("-n{}", MAX_LISTED), "--format=%x1e%H", "--name-only", "-z", "--ignore-submodules=none", range])
        .ok_text()
        .await
    else {
        return out;
    };
    for (oid, files) in crate::git_history::parse_name_only(&text) {
        out.insert(oid, files.into_iter().collect());
    }
    out
}

/// The commit the superproject's rebase stopped on (the merge backend writes it).
async fn stopped_sha(repo: &Path) -> Option<String> {
    let p = GitCmd::at(repo).args(["rev-parse", "--path-format=absolute", "--git-path", "rebase-merge/stopped-sha"]).ok_text().await?;
    std::fs::read_to_string(p.trim()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The paths a `diff --name-only` / `log --name-only` names, as they are:
/// -z, so a non-ASCII name is not quoted into something no path equals.
async fn name_only(repo: &Path, args: &[&str]) -> BTreeSet<String> {
    // -z straight after the subcommand, where nothing can mistake it for a path.
    let (sub, rest) = args.split_first().unwrap_or((&"diff", &[]));
    match g(repo).arg(*sub).arg("-z").args(rest.iter().copied()).run().await {
        Ok(out) if out.ok() => out
            .stdout
            .split(|b| *b == 0)
            .map(|p| String::from_utf8_lossy(p).trim_start_matches('\n').to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        _ => BTreeSet::new(),
    }
}

/// The commit a tree records for a submodule path — only when the entry
/// there IS a submodule pointer. A folder or a file at that path (it became
/// a submodule later, or stopped being one) is not a pointer, and its tree
/// or blob id must not be taken for one.
pub(crate) async fn gitlink_at(repo: &Path, rev: &str, path: &str) -> Option<String> {
    let out = g(repo).top_flag("--literal-pathspecs").args(["ls-tree", "-z", rev, "--", path]).ok_text().await?;
    let rec = out.split('\0').next()?;
    let (meta, _) = rec.split_once('\t')?;
    let mut f = meta.split(' ');
    match (f.next(), f.next(), f.next()) {
        (Some("160000"), Some("commit"), Some(id)) => Some(id.to_string()),
        _ => None,
    }
}

/// Which untracked files (not ignored) are in the way of `paths` arriving.
/// In the way means: something untracked at an incoming path, or at a path
/// an incoming path needs as a folder, or an untracked nested repository
/// already holding a file an incoming path names. Not in the way: an
/// untracked nested repository where `target` brings a submodule and that
/// already sits on the very commit the target records (git adopts it).
///
/// Asked of the disk, path by path: whether something exists there is the
/// disk's own answer, with its own idea of which names are the same (a
/// case-insensitive disk folds case — in every alphabet, which git's
/// `:(icase)` does not — and macOS folds Unicode normalisation too), and it
/// is exactly the test git makes before it writes. Only what exists is then
/// asked of git, two hundred paths a question: untracked and not ignored,
/// under its name as the disk spells it. A status listing is capped, and so
/// is never consulted. The answer stops at `MAX_HITS` names.
async fn untracked_among(repo: &Path, paths: &BTreeSet<String>, target: &str) -> Vec<String> {
    let mut hits: Vec<String> = Vec::new();
    let push = |hits: &mut Vec<String>, name: String| {
        if !hits.contains(&name) {
            hits.push(name);
        }
    };
    let mut listings = Listings::new();
    // Every folder an incoming path needs, looked at once: a folder, nothing,
    // or something else — a file or a link, which is in the way of everything
    // under it (its name as the disk spells it, when untracked).
    enum Ancestor {
        Folder,
        Missing,
        InTheWay(Option<String>),
    }
    let mut ancestors: BTreeMap<String, Ancestor> = BTreeMap::new();
    // What exists at an incoming path, spelled as the disk spells it.
    let mut present: Vec<String> = Vec::new();
    'paths: for p in paths {
        let comps: Vec<&str> = p.split('/').collect();
        let mut prefix = String::new();
        for c in &comps[..comps.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(c);
            if !ancestors.contains_key(&prefix) {
                let state = match repo.join(&prefix).symlink_metadata() {
                    Err(_) => Ancestor::Missing,
                    Ok(m) if m.is_dir() => Ancestor::Folder,
                    Ok(_) => Ancestor::InTheWay(untracked_at(repo, &prefix, &mut listings).await),
                };
                ancestors.insert(prefix.clone(), state);
            }
            match &ancestors[&prefix] {
                Ancestor::Missing => continue 'paths,
                Ancestor::Folder => {}
                Ancestor::InTheWay(name) => {
                    if let Some(n) = name {
                        push(&mut hits, n.clone());
                    }
                    continue 'paths;
                }
            }
        }
        let Ok(meta) = repo.join(p).symlink_metadata() else { continue };
        if meta.is_dir() && repo.join(p).join(".git").exists() {
            if let Some(t) = gitlink_at(repo, target, p).await {
                if oid(&repo.join(p), "HEAD").await.is_some_and(|h| same_oid(&h, &t)) {
                    continue;
                }
            }
        }
        present.push(on_disk_name(repo, p, &mut listings));
    }
    for chunk in present.chunks(200) {
        if hits.len() >= MAX_HITS {
            return hits;
        }
        for name in untracked_entries(repo, chunk).await {
            if hits.len() >= MAX_HITS {
                break;
            }
            push(&mut hits, name);
        }
    }
    // What git did not list may sit inside an untracked nested repository,
    // where git does not look: such a file is judged by the repository
    // holding it (asked once per repository).
    let listed: BTreeSet<String> = hits.iter().map(|h| h.trim_end_matches('/').to_string()).collect();
    let mut nested: BTreeMap<String, bool> = BTreeMap::new();
    for name in &present {
        if hits.len() >= MAX_HITS {
            break;
        }
        if listed.contains(name) {
            continue;
        }
        let mut cut = name.as_str();
        while let Some((dir, _)) = cut.rsplit_once('/') {
            if repo.join(dir).join(".git").exists() {
                if !nested.contains_key(dir) {
                    let untracked = !untracked_entries(repo, &[dir.to_string()]).await.is_empty();
                    nested.insert(dir.to_string(), untracked);
                }
                if nested[dir] {
                    push(&mut hits, name.clone());
                }
                break;
            }
            cut = dir;
        }
    }
    hits
}

/// The most names `untracked_among` collects: enough to say what is in the
/// way, few enough to stay quick when thousands are.
const MAX_HITS: usize = 50;

/// What sits at `rel` when it is untracked and not ignored — named as the
/// disk spells it (a folder as `name/`); `None` when it is tracked, ignored,
/// or an empty folder, none of which is in anything's way. Inside an
/// untracked nested repository git does not look, so a file there is judged
/// by the repository that holds it.
async fn untracked_at(repo: &Path, rel: &str, listings: &mut Listings) -> Option<String> {
    let actual = on_disk_name(repo, rel, listings);
    if let Some(name) = untracked_entries(repo, std::slice::from_ref(&actual)).await.into_iter().next() {
        return Some(name);
    }
    let mut cut = actual.as_str();
    while let Some((dir, _)) = cut.rsplit_once('/') {
        if repo.join(dir).join(".git").exists() {
            return (!untracked_entries(repo, &[dir.to_string()]).await.is_empty()).then_some(actual.clone());
        }
        cut = dir;
    }
    None
}

/// Of `specs` (paths as the disk spells them), those that are untracked and
/// not ignored, as git names them: a file by its path, a folder as `name/`.
async fn untracked_entries(repo: &Path, specs: &[String]) -> Vec<String> {
    let mut cmd = GitCmd::at(repo)
        .top_flag("--literal-pathspecs")
        .args(["ls-files", "--others", "--exclude-standard", "--directory", "--no-empty-directory", "-z", "--"]);
    for s in specs {
        cmd = cmd.arg(s.as_str());
    }
    match cmd.run().await {
        Ok(out) if out.ok() => out.stdout.split(|b| *b == 0).filter(|e| !e.is_empty()).map(|e| String::from_utf8_lossy(e).to_string()).collect(),
        _ => Vec::new(),
    }
}

/// One folder's entries as read once: the name, and what identifies the
/// file itself so a name the disk treats as the same can be found.
struct Entry {
    name: String,
    #[cfg(unix)]
    id: Option<(u64, u64)>,
}
type Listings = BTreeMap<PathBuf, Vec<Entry>>;

fn listing<'a>(listings: &'a mut Listings, dir: &Path) -> &'a Vec<Entry> {
    listings.entry(dir.to_path_buf()).or_insert_with(|| {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| Entry {
                        name: e.file_name().to_string_lossy().to_string(),
                        #[cfg(unix)]
                        id: e.metadata().ok().map(|m| {
                            use std::os::unix::fs::MetadataExt;
                            (m.dev(), m.ino())
                        }),
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// `rel`, spelled as the disk spells it: each part is looked up in its folder
/// exactly, then without regard to case, then — the disk said something is
/// there — as the very same file by identity (a name that differs only in
/// Unicode normalisation, say).
fn on_disk_name(repo: &Path, rel: &str, listings: &mut Listings) -> String {
    let mut spelled = String::new();
    let mut dir = repo.to_path_buf();
    for part in rel.split('/') {
        let entries = listing(listings, &dir);
        let found = entries
            .iter()
            .find(|e| e.name == part)
            .or_else(|| entries.iter().find(|e| e.name.to_lowercase() == part.to_lowercase()))
            .map(|e| e.name.clone());
        #[cfg(unix)]
        let found = found.or_else(|| {
            use std::os::unix::fs::MetadataExt;
            let m = dir.join(part).symlink_metadata().ok()?;
            let id = Some((m.dev(), m.ino()));
            entries.iter().find(|e| e.id == id).map(|e| e.name.clone())
        });
        let name = found.unwrap_or_else(|| part.to_string());
        if !spelled.is_empty() {
            spelled.push('/');
        }
        spelled.push_str(&name);
        dir = dir.join(&name);
    }
    spelled
}

/// The first few of many names, for a sentence. A list cut at `MAX_HITS`
/// does not know how many more there are.
fn some_of(names: &[String]) -> String {
    const SHOWN: usize = 10;
    if names.len() <= SHOWN {
        names.join(", ")
    } else if names.len() >= MAX_HITS {
        format!("{}, and at least {} more", names[..SHOWN].join(", "), names.len() - SHOWN)
    } else {
        format!("{}, and {} more", names[..SHOWN].join(", "), names.len() - SHOWN)
    }
}

/// Tracked paths with uncommitted changes, and untracked paths.
async fn dirty_sets(repo: &Path) -> (Vec<String>, Vec<String>) {
    let out = GitCmd::at(repo)
        .top_flag("--no-optional-locks")
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=all", "--ignore-submodules=none"])
        .run()
        .await;
    let Ok(out) = out else { return (Vec::new(), Vec::new()) };
    let parsed = git_status::parse_porcelain_v2(&out.stdout);
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    for e in parsed.entries {
        if e.kind == "untracked" {
            untracked.push(e.path);
        } else if !e.is_submodule {
            tracked.push(e.path);
        }
    }
    (tracked, untracked)
}

/// A `.gitmodules` URL as git resolves it: `./x` and `../x` are relative to
/// the superproject's remote — each `../` drops one path component of it (or
/// what follows the colon of a `host:path` address) — and anything else is
/// itself. Without a remote address a relative one is returned as written.
pub(crate) fn resolve_submodule_url(base: Option<&str>, url: &str) -> String {
    if !(url.starts_with("./") || url.starts_with("../")) {
        return url.to_string();
    }
    let Some(base) = base else { return url.to_string() };
    let mut remote = base.trim_end_matches(['/', '\\']).to_string();
    let mut rest = url;
    let mut colon = false;
    loop {
        if let Some(r) = rest.strip_prefix("../") {
            rest = r;
            if let Some(i) = remote.rfind(['/', '\\']) {
                remote.truncate(i);
            } else if let Some(i) = remote.rfind(':') {
                remote.truncate(i);
                colon = true;
            } else {
                remote = ".".into();
            }
        } else if let Some(r) = rest.strip_prefix("./") {
            rest = r;
        } else {
            break;
        }
    }
    let mut out = format!("{}{}{}", remote, if colon { ":" } else { "/" }, rest);
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// What `.gitmodules` at `rev` declares as the address of submodule `name`.
async fn declared_url(repo: &Path, rev: &str, name: &str) -> Option<String> {
    g(repo)
        .args(["config", "--blob", &format!("{}:.gitmodules", rev), "--get", &format!("submodule.{}.url", name)])
        .ok_text()
        .await
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Drops what a preview parked under refs/gitswitch-moved/ for this remote.
async fn clear_parked(sub: &Path, remote: &str) {
    let refs = g(sub).args(["for-each-ref", "--format=%(refname)", &format!("refs/gitswitch-moved/{}/", remote)]).ok_text().await.unwrap_or_default();
    for r in refs.lines().map(str::trim).filter(|r| !r.is_empty()) {
        let _ = g(sub).args(["update-ref", "-d", r]).run().await;
    }
}

/// Among `added` (paths upstream's tree adds), the gitlinks — split into the
/// ones upstream's .gitmodules declares and the ones it does not.
async fn new_gitlinks(repo: &Path, upstream_ref: &str, added: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    if added.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut args: Vec<String> = vec!["ls-tree".into(), "-z".into(), upstream_ref.to_string(), "--".into()];
    args.extend(added.iter().cloned());
    let Ok(out) = g(repo).top_flag("--literal-pathspecs").args(args.iter().map(String::as_str)).run().await else {
        return (Vec::new(), Vec::new());
    };
    let gitlinks: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter_map(|rec| {
            let rec = String::from_utf8_lossy(rec);
            let (meta, path) = rec.split_once('\t')?;
            meta.starts_with("160000 ").then(|| path.to_string())
        })
        .collect();
    if gitlinks.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let declared: BTreeSet<String> = g(repo)
        .args(["config", "--blob", &format!("{}:.gitmodules", upstream_ref), "--get-regexp", r"^submodule\..*\.path$"])
        .ok_text()
        .await
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once(' ').map(|(_, v)| v.trim().to_string()))
        .collect();
    gitlinks.into_iter().partition(|p| declared.contains(p))
}

/// The same address, give or take a trailing slash.
fn same_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

async fn fetch_remote(repo: &Path, remote: &str, extra: &[&str]) -> Result<(), String> {
    // `--progress`: stopped only for silence, never for the size of the fetch.
    // Kept as a pack, because git reports nothing while it unpacks a small one.
    let mut args = vec!["fetch", "--progress", "--no-recurse-submodules", "--", remote];
    args.extend_from_slice(extra);
    let out = g(repo).cfg("fetch.unpackLimit", "1").args(args).stall(crate::git_exec::net_stall()).run().await.map_err(|e| e.to_string())?;
    if out.ok() {
        Ok(())
    } else {
        Err(out.stderr.trim().lines().last().unwrap_or("fetch failed").to_string())
    }
}

/// Everything the planner needs. `fetch` is the only network step; the run
/// gathers again — fetching too — and compares oids with the plan.
pub async fn gather_facts(repo_path: &str, fetch: bool) -> Result<SyncFacts, AppError> {
    let repo = PathBuf::from(repo_path);
    let branch = g(&repo).args(["symbolic-ref", "-q", "--short", "HEAD"]).ok_text().await.unwrap_or_default();
    let upstream_ref = g(&repo)
        .args(["rev-parse", "--symbolic-full-name", "@{u}"])
        .ok_text()
        .await
        .filter(|s| s.starts_with("refs/remotes/"))
        .ok_or_else(|| AppError::Command("This branch has no upstream to update from.".into()))?;
    let upstream = upstream_ref.trim_start_matches("refs/remotes/").to_string();
    let remote = upstream.split('/').next().unwrap_or("origin").to_string();
    crate::git_exec::validate_remote_name(&remote)?;

    // Fetches run concurrently — a slow submodule remote should not
    // serialise six timeouts — but a few at a time: thirty submodules on one
    // host, all at once, is more connections than an ssh server lets in, and
    // the ones it turns away would show up as fetch failures.
    let subs_listed = crate::submodules::list_submodules(repo_path).await?;
    let mut fetch_errors: BTreeMap<String, String> = BTreeMap::new();
    if fetch {
        if let Err(e) = fetch_remote(&repo, &remote, &[]).await {
            return Err(AppError::Command(format!("Fetching {} failed: {}", remote, e)));
        }
    }
    // Where upstream now says each submodule lives. One whose address changed
    // in upstream's .gitmodules is fetched from the new one, parked under
    // refs/gitswitch-moved/ so a preview leaves its remote-tracking branches
    // alone; the run re-points it (git submodule sync), fetches for real and
    // clears the parking.
    let super_url = g(&repo).args(["config", "--get", &format!("remote.{}.url", remote)]).ok_text().await.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let base_for_urls = g(&repo).args(["merge-base", "HEAD", &upstream_ref]).ok_text().await.map(|s| s.trim().to_string()).unwrap_or_default();
    let mut url_changes: BTreeMap<String, UrlChange> = BTreeMap::new();
    let mut url_differs: BTreeMap<String, UrlChange> = BTreeMap::new();
    for s in subs_listed.iter().filter(|s| s.listed && s.initialised && s.gitdir_valid) {
        let Some(name) = s.name.as_deref() else { continue };
        let sub_remote = s.upstream_remote.clone().unwrap_or_else(|| "origin".into());
        let Some(declared) = declared_url(&repo, &upstream_ref, name).await else { continue };
        let Some(configured) = g(&repo.join(&s.path)).args(["config", "--get", &format!("remote.{}.url", sub_remote)]).ok_text().await.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else { continue };
        let to = resolve_submodule_url(super_url.as_deref(), &declared);
        if same_url(&to, &configured) {
            continue;
        }
        // Upstream moved it only when its declaration differs from where we
        // parted; otherwise the address here is this checkout's own choice (a
        // mirror, ssh for https), which the update leaves alone.
        let base_declared = if base_for_urls.is_empty() { None } else { declared_url(&repo, &base_for_urls, name).await.map(|d| resolve_submodule_url(super_url.as_deref(), &d)) };
        let moved = match &base_declared {
            Some(b) => !same_url(b, &to),
            None => true,
        };
        if moved {
            url_changes.insert(s.path.clone(), UrlChange { from: configured, to });
        } else {
            url_differs.insert(s.path.clone(), UrlChange { from: configured, to });
        }
    }
    if fetch {
        let jobs: Vec<(String, PathBuf, String, Option<String>)> = subs_listed
            .iter()
            .filter(|s| s.listed && s.initialised && s.gitdir_valid)
            .map(|s| (s.path.clone(), repo.join(&s.path), s.upstream_remote.clone().unwrap_or_else(|| "origin".into()), url_changes.get(&s.path).map(|u| u.to.clone())))
            .collect();
        for batch in jobs.chunks(FETCH_FANOUT) {
            let tasks: Vec<_> = batch
                .iter()
                .cloned()
                .map(|(path, dir, rem, moved_to)| {
                    tokio::spawn(async move {
                        let r = match moved_to {
                            Some(url) => {
                                let spec = format!("+refs/heads/*:refs/gitswitch-moved/{}/*", rem);
                                fetch_remote(&dir, &url, &[spec.as_str()]).await
                            }
                            None => fetch_remote(&dir, &rem, &[]).await,
                        };
                        (path, r)
                    })
                })
                .collect();
            for t in tasks {
                if let Ok((path, Err(e))) = t.await {
                    fetch_errors.insert(path, e);
                }
            }
        }
    }

    let head = oid(&repo, "HEAD").await.ok_or_else(|| AppError::Command("No commits yet".into()))?;
    let upstream_oid = oid(&repo, &upstream_ref).await.ok_or_else(|| AppError::Command(format!("{} does not exist locally", upstream)))?;
    let base_oid = g(&repo).args(["merge-base", "HEAD", &upstream_ref]).ok_text().await.unwrap_or_default();
    let counts = g(&repo).args(["rev-list", "--left-right", "--count", &format!("HEAD...{}", upstream_ref)]).ok_text().await.unwrap_or_default();
    let mut it = counts.split_whitespace();
    let ahead: usize = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let behind: usize = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let mut own_commits = commit_list(&repo, &format!("{}..HEAD", upstream_ref)).await;
    let mut incoming = commit_list(&repo, &format!("HEAD..{}", upstream_ref)).await;
    let own_merges: usize = g(&repo)
        .args(["rev-list", "--count", "--merges", &format!("{}..HEAD", upstream_ref)])
        .ok_text()
        .await
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0);
    // Which of my commits upstream already carries, patch for patch: the
    // rebase drops those, and the picture says so beforehand.
    let cherry = g(&repo).args(["cherry", &upstream_ref, "HEAD"]).ok_text().await.unwrap_or_default();
    let equivalent: BTreeSet<String> = cherry.lines().filter_map(|l| l.strip_prefix("- ")).map(|o| o.trim().to_string()).collect();
    for c in own_commits.iter_mut() {
        c.already_upstream = equivalent.contains(&c.oid);
    }
    let unlisted_already_upstream: Vec<String> = equivalent.iter().filter(|o| !own_commits.iter().any(|c| &c.oid == *o)).take(1000).cloned().collect();
    let own_files = files_per_commit(&repo, &format!("{}..HEAD", upstream_ref)).await;
    let in_files = files_per_commit(&repo, &format!("HEAD..{}", upstream_ref)).await;
    // git also drops a commit whose change is already in place when it is
    // replayed — the same lines arrived by another commit — even though its
    // patch is not identical. Predicted when every file it touches already
    // has its content in upstream, and no other commit of mine touches those
    // files (a later one would replay onto them and not be empty).
    for i in 0..own_commits.len() {
        if own_commits[i].already_upstream {
            continue;
        }
        let Some(files) = own_files.get(&own_commits[i].oid) else { continue };
        if files.is_empty() {
            continue;
        }
        let shared = own_commits.iter().enumerate().any(|(j, o)| j != i && own_files.get(&o.oid).is_some_and(|f| f.iter().any(|p| files.contains(p))));
        if shared {
            continue;
        }
        let mut args: Vec<String> = vec!["diff".into(), "--quiet".into(), "--ignore-submodules=none".into(), own_commits[i].oid.clone(), upstream_ref.clone(), "--".into()];
        args.extend(files.iter().cloned());
        let same = g(&repo).top_flag("--literal-pathspecs").args(args.iter().map(String::as_str)).run().await.map(|o| o.ok()).unwrap_or(false);
        if same {
            own_commits[i].already_upstream = true;
        }
    }
    // A commit that changes nothing but submodule pointers is a bump, not work.
    let gitlinks: BTreeSet<String> = subs_listed.iter().map(|s| s.path.clone()).collect();
    if !gitlinks.is_empty() {
        for (list, files) in [(&mut own_commits, &own_files), (&mut incoming, &in_files)] {
            for c in list.iter_mut() {
                if let Some(f) = files.get(&c.oid) {
                    c.only_pointers = !f.is_empty() && f.iter().all(|p| gitlinks.contains(p));
                }
            }
        }
    }
    let upstream_changed = name_only(&repo, &["diff", "--name-only", "--ignore-submodules=none", &format!("HEAD...{}", upstream_ref)]).await;
    // --no-renames: a file upstream renamed is an addition at its new path
    // all the same, and an untracked file sitting there is in its way.
    let upstream_added = name_only(&repo, &["diff", "--name-only", "--diff-filter=A", "--no-renames", "--ignore-submodules=none", &format!("HEAD...{}", upstream_ref)]).await;
    // Submodules upstream added: the run downloads the ones its .gitmodules
    // declares; one without an entry cannot be fetched by anything.
    let (new_submodules, new_unmapped) = new_gitlinks(&repo, &upstream_ref, &upstream_added).await;
    let mine_changed = name_only(&repo, &["log", "--name-only", "--format=", "--ignore-submodules=none", &format!("{}..HEAD", upstream_ref)]).await;
    let (dirty_tracked, untracked) = dirty_sets(&repo).await;
    let file_overlap: Vec<String> = mine_changed.intersection(&upstream_changed).cloned().collect();
    let dirty_overlap: Vec<String> = dirty_tracked.iter().filter(|p| upstream_changed.contains(*p)).cloned().collect();
    let untracked_overlap: Vec<String> = untracked_among(&repo, &upstream_added, &upstream_ref).await;
    let is_submodule_itself = g(&repo).args(["rev-parse", "--show-superproject-working-tree"]).ok_text().await.is_some_and(|s| !s.is_empty());

    let mut submodules = Vec::new();
    let mut skipped = Vec::new();
    for s in &subs_listed {
        if !s.listed {
            skipped.push(Skipped { path: s.path.clone(), why: "unmapped".into() });
            continue;
        }
        if !s.initialised {
            skipped.push(Skipped { path: s.path.clone(), why: "not-initialised".into() });
            continue;
        }
        let sub = repo.join(&s.path);
        // What the branch records — the commit, not the index: a pointer that
        // is only staged is not something "my commits" did, and it is the
        // committed one the run aligns to. (Not committed at all yet: the index.)
        let recorded = gitlink_at(&repo, "HEAD", &s.path).await.unwrap_or_else(|| s.recorded.clone());
        let target = gitlink_at(&repo, &upstream_ref, &s.path).await;
        let base_link = if base_oid.is_empty() { None } else { gitlink_at(&repo, &base_oid, &s.path).await };
        let moving: Vec<String> = g(&repo)
            .top_flag("--literal-pathspecs")
            .args(["log", "--format=%H", &format!("{}..HEAD", upstream_ref), "--", &s.path])
            .ok_text()
            .await
            .map(|o| o.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default();
        for c in own_commits.iter_mut() {
            if moving.iter().any(|m| m == &c.oid) {
                c.touches.push(s.path.clone());
            }
        }
        let moving_in: Vec<String> = g(&repo)
            .top_flag("--literal-pathspecs")
            .args(["log", "--format=%H", &format!("HEAD..{}", upstream_ref), "--", &s.path])
            .ok_text()
            .await
            .map(|o| o.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default();
        for c in incoming.iter_mut() {
            if moving_in.iter().any(|m| m == &c.oid) {
                c.touches.push(s.path.clone());
            }
        }
        let mut f = SubFacts {
            path: s.path.clone(),
            name: s.name.clone(),
            mapped: true,
            populated: true,
            gitdir_valid: s.gitdir_valid,
            head: s.actual.clone(),
            recorded: recorded.clone(),
            target: target.clone(),
            base_link: base_link.clone(),
            target_present: false,
            head_is_ancestor_of_target: false,
            target_is_ancestor_of_head: false,
            branch: s.own_branch.clone(),
            upstream: s.own_upstream.clone(),
            remote: s.upstream_remote.clone().or_else(|| Some("origin".into())),
            remote_head_branch: None,
            tip_ahead_of_target: false,
            // A commit upstream already carries patch for patch is left out
            // before anything is replayed: it cannot stop on this pointer.
            own_commits_moving_gitlink: moving.iter().filter(|m| !own_commits.iter().any(|c| &c.oid == *m && c.already_upstream)).count(),
            my_commits_move_gitlink: base_link.as_deref().is_some_and(|b| !same_oid(b, &recorded)),
            upstream_moves_gitlink: match (&base_link, &target) {
                (Some(b), Some(t)) => !same_oid(b, t),
                (None, Some(_)) | (Some(_), None) => true,
                _ => false,
            },
            own_pointers_on_line: false,
            pointer_rewound: false,
            index_pointer: s.recorded.clone(),
            dirty_tracked: s.dirty_tracked,
            untracked: s.dirty_untracked,
            dirty_overlap: Vec::new(),
            untracked_overlap: Vec::new(),
            conflicted: s.conflicted,
            operation: s.operation.clone(),
            fetch_error: fetch_errors.get(&s.path).cloned(),
            has_nested: sub.join(".gitmodules").exists(),
            url_change: url_changes.get(&s.path).cloned(),
            url_differs: url_differs.get(&s.path).cloned(),
            incoming_commits: Vec::new(),
            own_commits: Vec::new(),
            own_total: 0,
            identity_mismatch: false,
            identity_email: String::new(),
            profile_email: None,
        };
        if !s.gitdir_valid {
            submodules.push(f);
            continue;
        }
        if let Some(b) = &base_link {
            f.pointer_rewound = !same_oid(b, &recorded) && !is_ancestor(&sub, b, &recorded).await;
        }
        if let Some(t) = &target {
            let rem = f.remote.clone().unwrap_or_else(|| "origin".into());
            let mut present = g(&sub).args(["cat-file", "-e", &format!("{}^{{commit}}", t)]).run().await.map(|o| o.ok()).unwrap_or(false);
            if !present && fetch && f.fetch_error.is_none() {
                // What `git submodule update` does: fetch the exact commit.
                if fetch_remote(&sub, &rem, &[t]).await.is_ok() {
                    present = g(&sub).args(["cat-file", "-e", &format!("{}^{{commit}}", t)]).run().await.map(|o| o.ok()).unwrap_or(false);
                }
            }
            f.target_present = present;
            if present {
                if let Some(h) = &f.head {
                    f.head_is_ancestor_of_target = is_ancestor(&sub, h, t).await;
                    f.target_is_ancestor_of_head = is_ancestor(&sub, t, h).await;
                    let (dirty, _) = dirty_sets(&sub).await;
                    let changed = name_only(&sub, &["diff", "--name-only", "--ignore-submodules=none", &format!("{}...{}", h, t)]).await;
                    f.dirty_overlap = dirty.into_iter().filter(|p| changed.contains(p)).collect();
                    if !same_oid(h, t) {
                        // What moving it onto the target would write that is
                        // not tracked here yet: git will not overwrite those.
                        let added = name_only(&sub, &["diff", "--name-only", "--diff-filter=A", "--no-renames", "--ignore-submodules=none", &format!("{}...{}", h, t)]).await;
                        f.untracked_overlap = untracked_among(&sub, &added, t).await;
                    }
                    if !same_oid(h, t) {
                        f.incoming_commits = commit_list(&sub, &format!("{}..{}", h, t)).await;
                        f.own_commits = commit_list(&sub, &format!("{}..{}", t, h)).await;
                        f.own_total = g(&sub)
                            .args(["rev-list", "--count", &format!("{}..{}", t, h)])
                            .ok_text()
                            .await
                            .and_then(|n| n.trim().parse().ok())
                            .unwrap_or(f.own_commits.len());
                    }
                    // Several commits of mine move this pointer: the update
                    // can keep them one by one only when each points at a
                    // plain commit along the checked-out line, past where we
                    // parted from upstream.
                    if f.own_commits_moving_gitlink > 1 && f.upstream_moves_gitlink {
                        let mut on_line = true;
                        for m in moving.iter().filter(|m| !own_commits.iter().any(|c| &c.oid == *m && c.already_upstream)) {
                            let Some(ptr) = gitlink_at(&repo, m, &s.path).await else {
                                on_line = false;
                                break;
                            };
                            let merge = g(&sub).args(["rev-parse", "--verify", "--quiet", &format!("{}^2", ptr)]).ok_text().await.is_some();
                            if merge || !is_ancestor(&sub, &ptr, h).await || is_ancestor(&sub, &ptr, t).await {
                                on_line = false;
                                break;
                            }
                        }
                        f.own_pointers_on_line = on_line;
                    }
                    // Diverged: its commits would be re-written, so the
                    // identity git has there matters — as it does for a commit.
                    if !f.head_is_ancestor_of_target && !f.target_is_ancestor_of_head {
                        if let Ok(st) = Box::pin(git_status::repo_status(&sub.to_string_lossy())).await {
                            f.identity_mismatch = !st.identity.matches_profile;
                            f.identity_email = st.identity.email.clone();
                            f.profile_email = st.identity.profile_email.clone();
                        }
                    }
                }
                let remote_head = g(&sub).args(["symbolic-ref", "-q", "--short", &format!("refs/remotes/{}/HEAD", rem)]).ok_text().await;
                f.remote_head_branch = remote_head.as_deref().and_then(|r| r.split_once('/')).map(|(_, b)| b.to_string());
                let tip = match &f.upstream {
                    Some(u) => oid(&sub, &format!("refs/remotes/{}", u)).await,
                    None => match &remote_head {
                        Some(r) => oid(&sub, &format!("refs/remotes/{}", r)).await,
                        None => None,
                    },
                };
                if let Some(tip) = tip {
                    // Not for a moved submodule: its remote-tracking tip is the old address's.
                    f.tip_ahead_of_target = f.url_change.is_none() && !same_oid(&tip, t) && is_ancestor(&sub, t, &tip).await;
                }
            }
        }
        submodules.push(f);
    }

    // A pointer bump upstream has since moved past: replaying it, git merges
    // the two pointers, takes upstream's newer commit, finds my commit empty
    // and leaves it out ("patch contents already upstream"). Foreseen when
    // the commit moves nothing but pointers, each from the commit we both
    // started from to one upstream's pointer already contains, and no other
    // commit of mine moves the same pointer.
    for i in 0..own_commits.len() {
        if own_commits[i].already_upstream || !own_commits[i].only_pointers || own_commits[i].touches.is_empty() {
            continue;
        }
        // Every file it touches must be a pointer accounted for below.
        if own_files.get(&own_commits[i].oid).map(|f| f.len()) != Some(own_commits[i].touches.len()) {
            continue;
        }
        let mut superseded = true;
        for path in own_commits[i].touches.clone() {
            let shared = own_commits.iter().enumerate().any(|(j, o)| j != i && o.touches.contains(&path));
            let fact = submodules.iter().find(|f| f.path == path);
            let (Some(f), false) = (fact, shared) else {
                superseded = false;
                break;
            };
            let (Some(t), Some(b), true) = (&f.target, &f.base_link, f.target_present) else {
                superseded = false;
                break;
            };
            let Some(mine) = gitlink_at(&repo, &own_commits[i].oid, &path).await else {
                superseded = false;
                break;
            };
            let sub = repo.join(&path);
            let forward = same_oid(b, &mine) || is_ancestor(&sub, b, &mine).await;
            let contained = same_oid(&mine, t) || is_ancestor(&sub, &mine, t).await;
            if !(forward && contained) {
                superseded = false;
                break;
            }
        }
        if superseded {
            own_commits[i].already_upstream = true;
        }
    }

    Ok(SyncFacts {
        superproject: SuperFacts {
            branch,
            upstream,
            upstream_ref,
            remote,
            head,
            upstream_oid,
            base_oid,
            ahead,
            behind,
            own_commits,
            incoming,
            file_overlap,
            dirty_tracked,
            untracked: untracked.len(),
            dirty_overlap,
            untracked_overlap,
            is_submodule_itself,
            own_merges,
            unlisted_already_upstream,
            identity_mismatch: false,
            identity_email: String::new(),
            profile_email: None,
            new_submodules,
            new_unmapped,
        },
        submodules,
        skipped,
    })
}

/// The identity git has in the superproject, from the status the caller
/// already read: a rebase re-commits under it, so the planner must know.
fn note_identity(facts: &mut SyncFacts, f: &StatusFacts) {
    facts.superproject.identity_mismatch = !f.identity_matches;
    facts.superproject.identity_email = f.identity_email.clone();
    facts.superproject.profile_email = f.profile_email.clone();
}

// ---------------------------------------------------------------------------
// Assess
// ---------------------------------------------------------------------------

async fn facts_for_check(repo_path: &str) -> Result<(RepoStatus, StatusFacts), AppError> {
    let status = Box::pin(git_status::repo_status(repo_path)).await?;
    let facts = StatusFacts::from(&status);
    Ok((status, facts))
}

/// Fetch, measure, decide — nothing is written.
/// `fetch` is false only when re-planning with a different `stash` choice
/// right after an assessment: the answer is then local and instant.
pub async fn sync_plan(repo_path: &str, stash: bool, fetch: bool) -> Result<SyncPlan, AppError> {
    let (_status, f) = facts_for_check(repo_path).await?;
    if let Some(r) = check(&Intent::Sync, &f) {
        return Ok(SyncPlan { refusal: Some(r.clone()), summary: r.message, ..Default::default() });
    }
    let mut facts = gather_facts(repo_path, fetch).await?;
    note_identity(&mut facts, &f);
    Ok(plan(&facts, stash))
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SubOutcome {
    pub path: String,
    pub action: String,
    pub head_before: Option<String>,
    pub head_after: Option<String>,
    pub recorded_after: Option<String>,
    pub ok: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SyncOutcome {
    /// done | needs-user | aborted | failed
    pub phase: String,
    pub needs_user: Option<NeedsUser>,
    pub backups: Vec<BackupRef>,
    pub bundles: Vec<String>,
    pub head_before: String,
    pub head_after: String,
    pub incoming: usize,
    pub own_kept: Vec<CommitRef>,
    /// How many commits of mine are on top — all of them; `own_kept` lists
    /// only the newest.
    #[serde(default)]
    pub own_kept_total: usize,
    pub own_dropped: Vec<CommitRef>,
    pub stashed: bool,
    pub stash_left: bool,
    pub submodules: Vec<SubOutcome>,
    pub verified: Verified,
    pub unrecorded_pointers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BackupRef {
    pub repo: String,
    pub branch: String,
    pub oid: String,
    pub recovery: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Verified {
    pub behind_zero: bool,
    pub own_on_top: bool,
    pub dirty_paths_unchanged: bool,
    pub untracked_unchanged: bool,
    pub submodules_aligned: bool,
    pub no_conflicts_left: bool,
}

pub struct RunOptions {
    pub stash: bool,
    pub bundles: bool,
    /// From the plan; empty skips the "upstream moved since you assessed" check (tests only).
    pub fingerprint: Vec<(String, String)>,
}

pub(crate) fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%d%H%M%S").to_string()
}

pub(crate) async fn make_backup(repo: &Path, name: &str) -> Result<String, AppError> {
    let out = g(repo).args(["branch", "--", name]).timeout(LOCAL_TIMEOUT).run().await?;
    if !out.ok() {
        return Err(AppError::Command(format!(
            "Could not create the backup branch {} in {}: {}. Nothing was changed.",
            name,
            repo.display(),
            out.stderr.trim()
        )));
    }
    oid(repo, "HEAD").await.ok_or_else(|| AppError::Command("no HEAD".into()))
}

async fn rebase_in_progress(repo: &Path) -> bool {
    match GitCmd::at(repo).args(["rev-parse", "--path-format=absolute", "--git-path", "rebase-merge"]).ok_text().await {
        Some(p) => Path::new(p.trim()).is_dir(),
        None => false,
    }
}

pub(crate) async fn unmerged(repo: &Path) -> Vec<git_status::ChangeEntry> {
    let Ok(out) = GitCmd::at(repo)
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=no", "--ignore-submodules=none"])
        .run()
        .await
    else {
        return Vec::new();
    };
    git_status::parse_porcelain_v2(&out.stdout)
        .entries
        .into_iter()
        .filter(|e| e.kind == "conflicted")
        .collect()
}

/// The reflog subject a rebase writes when it cannot re-apply its autostash
/// (`git stash store -m autostash`): exactly this word, no branch prefix. A
/// hand-made stash whose message mentions the word is "On main: …autostash…"
/// and is not one.
pub(crate) const AUTOSTASH_SUBJECT: &str = "autostash";

pub(crate) fn is_autostash_subject(subject: &str) -> bool {
    subject.trim() == AUTOSTASH_SUBJECT
}

/// Any entry in the stash list that a rebase's failed autostash pop stored.
pub(crate) async fn has_autostash_entry(repo: &Path) -> bool {
    g(repo)
        .args(["stash", "list", "--format=%gs"])
        .ok_text()
        .await
        .is_some_and(|s| s.lines().any(is_autostash_subject))
}

/// The top of the stash right now ("" when there is none) — read before a
/// rebase that may autostash, so an entry found afterwards can be told from
/// one that was already there.
async fn stash_top(repo: &Path) -> String {
    oid(repo, "refs/stash").await.unwrap_or_default()
}

/// Did the rebase that just ran leave its autostash behind? `before` is the
/// stash top read before it: the stash must have grown and its newest entry
/// be an autostash. An entry named "autostash" from an earlier rebase (the
/// person reset instead of popping it) is theirs and says nothing about this
/// run. Without a `before` — a state an older build wrote — any such entry
/// counts, as it used to.
async fn autostash_left(repo: &Path, before: Option<&str>) -> bool {
    match before {
        Some(b) => stash_top(repo).await != b && autostash_on_top(repo).await,
        None => has_autostash_entry(repo).await,
    }
}

/// The newest stash entry is one a rebase's failed autostash pop stored — the
/// entry conflicts left behind with no operation in progress came from.
pub(crate) async fn autostash_on_top(repo: &Path) -> bool {
    g(repo)
        .args(["stash", "list", "-n1", "--format=%gs"])
        .ok_text()
        .await
        .is_some_and(|s| is_autostash_subject(&s))
}

/// The state machine's single step: called after the superproject rebase
/// started or resumed, and again by `sync_continue`. Returns when the sync is
/// done, needs the user, or failed.
async fn drive(repo: &Path, st: &mut SyncState, state_file: &Path) -> Result<(), AppError> {
    loop {
        if !rebase_in_progress(repo).await {
            break;
        }
        let conflicts = unmerged(repo).await;
        if conflicts.is_empty() {
            // Stopped with nothing unmerged: an untracked file in the way, a
            // hook, an LFS smudge failure. Hand over with git's words.
            let hint = if st.git_said.is_empty() {
                "The rebase stopped without a conflict to resolve. Read what git said in the command log, fix it, then Continue update — or Abort update.".to_string()
            } else {
                format!("The rebase stopped without a conflict to resolve. git said: {} — fix that, then Continue update, or Abort update.", st.git_said)
            };
            st.needs_user = Some(NeedsUser {
                where_: "superproject".into(),
                paths: Vec::new(),
                hint,
                at_commit: stopped_sha(repo).await,
            });
            st.phase = "rebasing".into();
            write_state_at(state_file, st)?;
            return Ok(());
        }
        let files: Vec<String> = conflicts.iter().filter(|c| !c.is_submodule).map(|c| c.path.clone()).collect();
        if !files.is_empty() {
            st.needs_user = Some(NeedsUser {
                where_: "superproject".into(),
                paths: files.clone(),
                hint: format!("Resolve {} on the Changes page (edit, then stage), then Continue update.", files.join(", ")),
                at_commit: stopped_sha(repo).await,
            });
            st.phase = "rebasing".into();
            write_state_at(state_file, st)?;
            return Ok(());
        }
        for c in conflicts.iter().filter(|c| c.is_submodule) {
            resolve_gitlink_stop(repo, st, state_file, c).await?;
            if st.needs_user.is_some() {
                return Ok(());
            }
        }
        // This commit is replayed with a rewritten pointer: its patch will not
        // match the original, so the check afterwards must not count it dropped.
        remember_resolved(repo, st).await;
        crate::git_ops::commit_resolved_pick(repo).await;
        let out = rw(repo, st.lfs_skip).args(["rebase", "--continue"]).timeout(NET_TIMEOUT).run().await?;
        note_drops(st, &out.stderr);
        if !out.ok() && !rebase_in_progress(repo).await {
            return Err(AppError::Command(format!("rebase --continue failed: {}", out.stderr.trim())));
        }
    }
    // The rebase finished. A failed autostash pop leaves unmerged paths and
    // an "autostash" stash entry with no operation marker.
    let leftovers = unmerged(repo).await;
    if !leftovers.is_empty() || autostash_left(repo, st.stash_before.as_deref()).await {
        st.needs_user = Some(NeedsUser {
            where_: "autostash-conflict".into(),
            paths: leftovers.iter().map(|c| c.path.clone()).collect(),
            hint: "The rebase finished, but your stashed changes did not re-apply cleanly. Resolve and stage them, then Continue update — or run `git reset --hard` (your changes are still in stash@{0}) and `git stash pop` by hand.".into(),
            at_commit: None,
        });
        st.phase = "aligning".into();
        write_state_at(state_file, st)?;
        return Ok(());
    }
    st.phase = "aligning".into();
    write_state_at(state_file, st)?;
    align_submodules(repo, st, state_file).await
}

/// Note the commit the superproject's rebase is replaying right now, just
/// before it is continued. With something staged it is committed with a
/// patch of its own (resolved by hand, or a rewritten pointer). With nothing
/// staged against HEAD, git leaves the commit out and says nothing — so that
/// is measured here, where it can be, instead of guessed at afterwards.
async fn remember_resolved(repo: &Path, st: &mut SyncState) {
    let Some(sha) = stopped_sha(repo).await else { return };
    // A stop committed already — by hand, or by the app on a try git then
    // refused for unstaged edits — has its commit; nothing staged is not
    // "nothing of it remained" then.
    let nothing_staged = crate::git_ops::stop_is_open(repo).await
        && g(repo)
            .args(["diff", "--cached", "--quiet", "--ignore-submodules=none", "HEAD", "--"])
            .run()
            .await
            .map(|o| o.code == 0)
            .unwrap_or(false);
    st.resolved.retain(|o| o != &sha);
    st.emptied.retain(|o| o != &sha);
    if nothing_staged {
        // Nothing left of it. When that is because its pointer was upstream's
        // own, upstream already contained it; otherwise its resolution
        // emptied it.
        if st.pointer_upstreams.contains(&sha) {
            if !st.dropped_by_git.contains(&sha) {
                st.dropped_by_git.push(sha);
            }
        } else {
            st.emptied.push(sha);
        }
    } else {
        // Something of it stays (files it changed, say): on top, whatever
        // became of its pointer.
        st.dropped_by_git.retain(|o| o != &sha);
        st.resolved.push(sha);
    }
}

/// A submodule-pointer stop: rebase that submodule's own commits onto the
/// commit upstream recorded, then record the result. Neither `--ours` nor
/// `--theirs` is ever right here.
async fn resolve_gitlink_stop(repo: &Path, st: &mut SyncState, state_file: &Path, c: &git_status::ChangeEntry) -> Result<(), AppError> {
    let path = c.path.clone();
    let at_commit = stopped_sha(repo).await;
    let hand_over = |st: &mut SyncState, hint: String| {
        st.needs_user = Some(NeedsUser { where_: format!("submodule:{}", path), paths: vec![path.clone()], hint, at_commit: at_commit.clone() });
        if !st.handed_over.contains(&path) {
            st.handed_over.push(path.clone());
        }
    };
    if c.stage_oids.len() != 3 || c.stage_oids.iter().any(|o| o.is_empty() || o == "0000000000000000000000000000000000000000") || c.stage_modes.get(1) != Some(&"160000".to_string()) || c.stage_modes.get(2) != Some(&"160000".to_string()) {
        hand_over(st, format!("The submodule {} was added or removed on one side; decide by hand, stage it, then Continue update.", path));
        return write_state_at(state_file, st);
    }
    let ours = c.stage_oids[1].clone(); // upstream's recorded commit (during a rebase, "ours" is the onto side)
    let theirs = c.stage_oids[2].clone(); // my commit's recorded commit
    let sub = repo.join(&path);
    let head = oid(&sub, "HEAD").await.unwrap_or_default();
    let ss = st.subs.iter_mut().find(|s| s.path == path);
    let Some(ss) = ss else {
        hand_over(st, format!("The submodule {} was not part of the plan; stage it by hand, then Continue update.", path));
        return write_state_at(state_file, st);
    };
    let already_rebased = ss.rebased && ss.head_after.as_deref() == Some(head.as_str());
    let old_tip = ss.head_before.clone();
    let target_for_stop = ss.target.clone();
    // My commit may point at an earlier commit of the checked-out line (one of
    // several pointer commits of mine): then the line is rebased once and this
    // stop records the rewritten copy of what it pointed at.
    let pointer_on_line = same_oid(&head, &theirs) || is_ancestor(&sub, &theirs, &head).await;
    if !pointer_on_line && !already_rebased {
        hand_over(st, format!(
            "{} is checked out at {} but your commit records {}. Put it where you want it (with your commits on top of {}), stage it, then Continue update.",
            path, short(&head), short(&theirs), short(&ours)
        ));
        return write_state_at(state_file, st);
    }
    if !already_rebased && (same_oid(&theirs, &ours) || is_ancestor(&sub, &theirs, &ours).await) {
        // My commit records a commit upstream's choice already contains: it
        // moved the pointer back. "Rebasing" the submodule onto upstream's
        // would be a plain fast-forward that undoes what the commit says,
        // and the commit would then vanish as empty. Which pointer stays is
        // the person's call.
        hand_over(st, format!(
            "Your commit records {} at {}, behind the {} upstream moved it to. Leave it there to keep your commit's pointer, or move it to {} to take upstream's — then stage it and Continue update.",
            path, short(&theirs), short(&ours), short(&ours)
        ));
        return write_state_at(state_file, st);
    }
    // My commit's choice already contains upstream's: there is nothing to
    // rebase, and recording mine keeps both.
    let needs_rebase = !already_rebased && !is_ancestor(&sub, &ours, &theirs).await;
    if needs_rebase && ss.backup.is_none() {
        // Not planned as a rebase: no backup branch was made for it and none
        // of a rebase's checks (identity, files in the way) were run. Its
        // commits are not re-written on the strength of a stop the preview
        // did not expect — the pointer is handed over instead.
        hand_over(st, format!(
            "The update stopped on {}: your commit records it at {} and upstream at {}. Put it where you want it, stage it, then Continue update.",
            path, short(&theirs), short(&ours)
        ));
        return write_state_at(state_file, st);
    }
    if needs_rebase {
        // Detached: rebase detached, re-attach afterwards when the branch was where it stood.
        ss.stash_before = Some(stash_top(&sub).await);
        let out = rw(&sub, st.lfs_skip).args(["rebase", "--merge", "--autostash", &ours]).timeout(NET_TIMEOUT).run().await?;
        if !out.ok() {
            ss.rebased = true;
            ss.note = "rebase stopped on a conflict inside it".into();
            hand_over(st, format!(
                "Rebasing {} onto {} stopped on a conflict inside it. Open it as its own repository on the Changes page, resolve and stage, then Continue update.",
                path, short(&ours)
            ));
            return write_state_at(state_file, st);
        }
        if !unmerged(&sub).await.is_empty() || autostash_left(&sub, ss.stash_before.as_deref()).await {
            ss.rebased = true;
            hand_over(st, format!(
                "{} was rebased, but its uncommitted changes did not re-apply cleanly. Resolve and stage them inside it, then Continue update.",
                path
            ));
            st.needs_user.as_mut().unwrap().where_ = format!("submodule-autostash:{}", path);
            return write_state_at(state_file, st);
        }
        let new_head = oid(&sub, "HEAD").await.unwrap_or_default();
        reattach(&sub, ss).await;
        ss.rebased = true;
        ss.head_after = Some(new_head.clone());
        ss.note = format!("rebased onto {}", short(&ours));
    }
    // What this commit records for the pointer: with mine already containing
    // upstream's, exactly what it pointed at; after a rebase, the rewritten
    // copy of that.
    let new_head = oid(&sub, "HEAD").await.unwrap_or_default();
    // The line was rebased onto the plan's target; at a later stop `ours` is
    // what the previous pick recorded (one of my own copies), so the copies
    // are looked for from the target on.
    let onto = if target_for_stop.is_empty() { ours.clone() } else { target_for_stop.clone() };
    let record = if !needs_rebase && !already_rebased {
        theirs.clone()
    } else {
        match rewritten_pointer(&sub, &theirs, &onto, &old_tip, &new_head).await {
            Ok(Some(copy)) => copy,
            Ok(None) => {
                // What it pointed at is upstream's own now. Whether the whole
                // commit is then already upstream is measured when it is
                // continued: nothing else of it may remain. The pointer never
                // moves back for it: when what the previous commit recorded
                // already contains upstream's, that stays.
                if let Some(sha) = &at_commit {
                    if !st.pointer_upstreams.contains(sha) {
                        st.pointer_upstreams.push(sha.clone());
                    }
                }
                if same_oid(&onto, &ours) || is_ancestor(&sub, &onto, &ours).await { ours.clone() } else { onto.clone() }
            }
            Err(why) => {
                hand_over(st, why);
                return write_state_at(state_file, st);
            }
        }
    };
    if let Err(e) = stage_pointer(repo, &path, &record, &new_head).await {
        // What was done to the submodule is on record either way, so an
        // abort can still put it back.
        write_state_at(state_file, st)?;
        return Err(e);
    }
    write_state_at(state_file, st)
}

/// The commits in `range` with this author, time and subject.
async fn commits_like(repo: &Path, key: &str, range: &str) -> Vec<String> {
    g(repo)
        .args(["log", "--format=%H%x1f%an%x1f%at%x1f%s", range])
        .ok_text()
        .await
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\x1f').filter(|(_, k)| *k == key).map(|(h, _)| h.to_string()))
        .collect()
}

/// Stage `record` as the pointer of `path` — through `git add` when it is
/// the submodule's checkout, exactly by id otherwise (a stop that is not the
/// last of several pointer commits records an earlier commit of the line).
async fn stage_pointer(repo: &Path, path: &str, record: &str, sub_head: &str) -> Result<(), AppError> {
    let out = if same_oid(record, sub_head) {
        g(repo).top_flag("--literal-pathspecs").args(["add", "--", path]).timeout(LOCAL_TIMEOUT).run().await?
    } else {
        g(repo).args(["update-index", "--cacheinfo", &format!("160000,{},{}", record, path)]).timeout(LOCAL_TIMEOUT).run().await?
    };
    if !out.ok() {
        return Err(AppError::Command(format!("Could not record {}: {}", path, out.stderr.trim())));
    }
    Ok(())
}

/// The commit a pointer stop records once the submodule's line has been
/// rebased onto `onto` (the plan's target): the copy of `theirs` — what my
/// commit pointed at — on the rewritten line, found by its author, time and
/// subject (a rebase keeps those). `Ok(None)` when that commit was left out
/// as upstream's own (its twin sits on upstream's side of the line): the
/// pointer is upstream's then. An error carries the hint for a hand-over:
/// no copy, or more than one.
async fn rewritten_pointer(sub: &Path, theirs: &str, onto: &str, old_tip: &str, new_head: &str) -> Result<Option<String>, String> {
    let key = g(sub)
        .args(["log", "-1", "--format=%an%x1f%at%x1f%s", theirs])
        .ok_text()
        .await
        .ok_or_else(|| format!("Your commit records this submodule at {}, which cannot be read here. Put the pointer where you want it, stage it, then Continue update.", short(theirs)))?;
    let copies = commits_like(sub, &key, &format!("{}..{}", onto, new_head)).await;
    match copies.len() {
        1 => return Ok(Some(copies[0].clone())),
        0 => {}
        _ => {
            return Err(format!(
                "Your commit records this submodule at {} ({}), and its rebased line carries more than one commit like it. Put the pointer where you want it, stage it, then Continue update.",
                short(theirs),
                key.split('\x1f').nth(2).unwrap_or("")
            ))
        }
    }
    // Left out of the rebased line: upstream's own copy of it, if there is one.
    if let Some(mb) = g(sub).args(["merge-base", onto, old_tip]).ok_text().await.filter(|m| !m.is_empty()) {
        if commits_like(sub, &key, &format!("{}..{}", mb, onto)).await.len() == 1 {
            return Ok(None);
        }
    }
    Err(format!(
        "Your commit records this submodule at {} ({}), which has no counterpart on its rebased line. Put the pointer where you want it, stage it, then Continue update.",
        short(theirs),
        key.split('\x1f').nth(2).unwrap_or("")
    ))
}

/// After a detached rebase: the branch the submodule was on before (or, when
/// it was detached, the branch its remote's HEAD names) is moved to the new
/// HEAD and checked out — but only when that branch stood exactly where the
/// checkout was, or is already an ancestor of the new HEAD. A branch pointing
/// anywhere else is somebody's work and is left alone.
async fn reattach(sub: &Path, ss: &mut SubState) {
    let on_branch = g(sub).args(["symbolic-ref", "-q", "HEAD"]).ok_text().await.is_some();
    if on_branch {
        return;
    }
    let candidates: Vec<String> = ss.branch_before.iter().chain(ss.reattach_to.iter()).cloned().collect();
    for b in candidates {
        let Some(at) = oid(sub, &format!("refs/heads/{}", b)).await else { continue };
        if same_oid(&at, &ss.head_before) || is_ancestor(sub, &b, "HEAD").await {
            let moved = g(sub).args(["branch", "-f", &b, "HEAD"]).run().await.map(|o| o.ok()).unwrap_or(false);
            let _ = g(sub).args(["switch", "--quiet", &b]).run().await;
            if moved && ss.reattached.is_none() {
                // Where the branch stood, so an abort can put it back.
                ss.reattached = Some((b.clone(), at.clone()));
            }
            ss.note = format!("{} (re-attached to '{}')", ss.note, b);
            return;
        }
    }
}

/// Step 5: bring every mapped, populated submodule to the commit the
/// superproject now records.
async fn align_submodules(repo: &Path, st: &mut SyncState, state_file: &Path) -> Result<(), AppError> {
    for i in 0..st.subs.len() {
        // A continue after a pause comes back through here: what was already
        // aligned keeps its note and is not touched again.
        if st.subs[i].aligned {
            continue;
        }
        let path = st.subs[i].path.clone();
        let sub = repo.join(&path);
        if let Some(u) = st.subs[i].url_change.clone().filter(|_| st.subs[i].url_note.is_none()) {
            let to = crate::repo_scan::mask_token(&u.to);
            let remote = st.subs[i].remote.clone().unwrap_or_else(|| "origin".into());
            let dead = st.plan.submodules.iter().find(|p| p.path == path).and_then(|p| p.fetch_error.clone());
            if let Some(e) = dead {
                // The new address did not answer at preview time: the working
                // one stays, and the result says so.
                st.subs[i].url_note = Some(format!("not re-pointed at {}: fetching from there failed at preview time ({}); it still fetches from {}", to, e, crate::repo_scan::mask_token(&u.from)));
            } else {
                // .gitmodules is the rebased branch's now: `git submodule sync`
                // points the submodule (and the superproject's record of it) at
                // the address it declares; then its branches are fetched from
                // there. The note names the address actually left in place.
                let synced = g(repo).args(["submodule", "sync", "--", &path]).timeout(LOCAL_TIMEOUT).run().await.map(|o| o.ok()).unwrap_or(false);
                st.subs[i].url_synced = synced;
                let now = g(&sub).args(["config", "--get", &format!("remote.{}.url", remote)]).ok_text().await.map(|t| crate::repo_scan::mask_token(t.trim())).unwrap_or_default();
                st.subs[i].url_note = Some(if !synced {
                    format!("could not be re-pointed at {} (git submodule sync failed)", to)
                } else {
                    match fetch_remote(&sub, &remote, &[]).await {
                        Ok(()) => format!("now fetches from {}", now),
                        Err(e) => format!("re-pointed at {}, but fetching from there failed: {}", now, e),
                    }
                });
                clear_parked(&sub, &remote).await;
            }
            write_state_at(state_file, st)?;
        }
        let Some(recorded) = gitlink_at(repo, "HEAD", &path).await else { continue };
        let Some(head) = oid(&sub, "HEAD").await else { continue };
        st.subs[i].target = recorded.clone();
        if same_oid(&head, &recorded) {
            st.subs[i].aligned = true;
            if st.subs[i].note.is_empty() {
                st.subs[i].note = "already at the recorded commit".into();
            }
            continue;
        }
        if is_ancestor(&sub, &head, &recorded).await {
            let on_branch = g(&sub).args(["symbolic-ref", "-q", "HEAD"]).ok_text().await.is_some();
            let out = if on_branch {
                rw(&sub, st.lfs_skip).args(["merge", "--ff-only", "--no-edit", "--no-stat", "--", &recorded]).timeout(LOCAL_TIMEOUT).run().await?
            } else {
                rw(&sub, st.lfs_skip).args(["checkout", "--quiet", "--detach", &recorded]).timeout(LOCAL_TIMEOUT).run().await?
            };
            if out.ok() {
                st.subs[i].aligned = true;
                st.subs[i].head_after = Some(recorded.clone());
                st.subs[i].note = format!("fast-forwarded to {}{}", short(&recorded), if on_branch { "" } else { " (detached)" });
            } else {
                st.subs[i].note = format!("could not fast-forward: {}", out.stderr.trim());
            }
            continue;
        }
        if is_ancestor(&sub, &recorded, &head).await {
            st.subs[i].aligned = true;
            st.subs[i].head_after = Some(head.clone());
            st.subs[i].note = format!("your commits sit on top of {}; the superproject does not record them yet", short(&recorded));
            continue;
        }
        // Diverged and untouched by my commits: rebase its own commits onto the recorded target.
        if st.subs[i].backup.is_none() {
            // …but only one the plan meant to rebase, with its backup made and
            // its checks run. Anything else is said, not re-written.
            st.subs[i].note = format!(
                "could not be aligned: it has diverged from {} and was not planned to be rebased; align it by hand",
                short(&recorded)
            );
            continue;
        }
        st.subs[i].stash_before = Some(stash_top(&sub).await);
        let out = rw(&sub, st.lfs_skip).args(["rebase", "--merge", "--autostash", &recorded]).timeout(NET_TIMEOUT).run().await?;
        st.subs[i].rebased = true;
        if !out.ok() {
            st.needs_user = Some(NeedsUser {
                where_: format!("submodule:{}", path),
                paths: vec![path.clone()],
                hint: format!("Rebasing {} onto {} stopped on a conflict inside it. Open it as its own repository, resolve and stage, then Continue update.", path, short(&recorded)),
                at_commit: None,
            });
            return write_state_at(state_file, st);
        }
        if !unmerged(&sub).await.is_empty() || autostash_left(&sub, st.subs[i].stash_before.as_deref()).await {
            st.needs_user = Some(NeedsUser {
                where_: format!("submodule-autostash:{}", path),
                paths: vec![path.clone()],
                hint: format!("{} was rebased, but its uncommitted changes did not re-apply cleanly. Resolve and stage them inside it, then Continue update.", path),
                at_commit: None,
            });
            return write_state_at(state_file, st);
        }
        let new_head = oid(&sub, "HEAD").await.unwrap_or_default();
        let mut ss = st.subs[i].clone();
        reattach(&sub, &mut ss).await;
        st.subs[i] = ss;
        st.subs[i].aligned = true;
        st.subs[i].head_after = Some(new_head);
        st.subs[i].note = format!("your commits rebased onto {}; the superproject does not record them yet", short(&recorded));
    }
    // Submodules upstream added: downloaded at the commit the rebased branch
    // records, as `git submodule update --init` would — one at a time, so one
    // that fails does not stop the next; each is named in the result. An
    // abort never undoes a download (there was nothing there before).
    let fresh: Vec<String> = st.plan.superproject.new_submodules.iter().filter(|p| !st.subs.iter().any(|s| &s.path == *p)).cloned().collect();
    for path in fresh {
        let sub = repo.join(&path);
        let recorded = gitlink_at(repo, "HEAD", &path).await.unwrap_or_default();
        let (ok, note) = if sub.join(".git").exists() {
            (true, "new upstream; already downloaded".to_string())
        } else if recorded.is_empty() {
            (false, "new upstream; the rebased branch does not record it after all".to_string())
        } else {
            let url = g(repo).args(["config", "--get", &format!("remote.{}.url", st.plan.superproject.remote)]).ok_text().await.unwrap_or_default();
            let repo_s = repo.to_string_lossy().to_string();
            let report = crate::sparse::update_submodules(repo, None, None, url.trim(), Some(std::slice::from_ref(&path)), Some(repo_s.as_str())).await;
            match (&report.error, report.downloaded) {
                (None, n) if n > 0 => (true, format!("new upstream; downloaded at {}", short(&recorded))),
                (None, _) => (false, "new upstream; could not be downloaded — Download it from the Submodules tab".to_string()),
                (Some(e), _) => (false, format!("new upstream; could not be downloaded: {} — Download it from the Submodules tab", e.trim())),
            }
        };
        // Under a blocked or locked repository the new submodule gets the
        // same guard-rail at once (the lock's own layer is Repair's).
        let mut note = note;
        if ok {
            if let Ok(ps) = crate::git_status::push_state_for(&repo.to_string_lossy()).await {
                if ps.repo_blocked || ps.lock.locked {
                    let notes = crate::push_guard::cover_submodules(&repo.to_string_lossy(), Some(std::slice::from_ref(&path))).await;
                    note = if !notes.is_empty() {
                        format!("{}; the push block could not be extended to it: {}", note, notes.join("; "))
                    } else if ps.lock.locked {
                        format!("{}; given the repository's guard-rail — the lock's own layer for it needs Repair (administrator) under Push access", note)
                    } else {
                        format!("{}; given the repository's push block", note)
                    };
                }
            }
        }
        st.subs.push(SubState {
            path: path.clone(),
            branch_before: None,
            reattach_to: None,
            head_before: String::new(),
            backup: None,
            target: recorded.clone(),
            rebased: false,
            head_after: oid(&sub, "HEAD").await,
            aligned: ok,
            note,
            stash_before: None,
            reattached: None,
            url_change: None,
            url_synced: false,
            url_note: None,
            remote: None,
        });
        write_state_at(state_file, st)?;
    }
    st.phase = "done".into();
    write_state_at(state_file, st)
}

/// Where a sub's rebase is mid-flight.
async fn sub_rebase_in_progress(repo: &Path, st: &SyncState) -> Option<String> {
    for s in &st.subs {
        if rebase_in_progress(&repo.join(&s.path)).await {
            return Some(s.path.clone());
        }
    }
    None
}

fn recovery(repo: &Path, branch: &str) -> String {
    format!("git -C {} reset --hard {}", shell_quote(&repo.to_string_lossy()), branch)
}

pub(crate) fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':')) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

async fn finish(repo_path: &str, st: &SyncState, state_file: &Path) -> Result<OpResult, AppError> {
    let repo = PathBuf::from(repo_path);
    // Done or aborted: the large files the update wrote as stubs come now, on
    // their own clock. A pause leaves them until the update is through.
    let large_files = if st.phase == "done" || st.phase == "aborted" { lfs_after_update(&repo, st).await } else { String::new() };
    let outcome = build_outcome(&repo, st).await;
    let status = Box::pin(git_status::repo_status(repo_path)).await?;
    let mut op = if st.phase == "done" {
        std::fs::remove_file(state_file).ok();
        let status = Box::pin(git_status::repo_status(repo_path)).await?;
        let v = &outcome.verified;
        let verified = v.behind_zero && v.own_on_top && v.no_conflicts_left && v.submodules_aligned;
        let unaligned: Vec<&SubOutcome> = outcome.submodules.iter().filter(|s| !s.ok).collect();
        let headline = if !v.submodules_aligned && v.behind_zero && v.own_on_top && v.no_conflicts_left {
            format!(
                "Update finished, but {} could not be aligned — read the details.",
                unaligned.iter().map(|s| s.path.as_str()).collect::<Vec<_>>().join(", ")
            )
        } else if !verified {
            "Update finished, but a check did not pass — read the details.".to_string()
        } else if outcome.incoming == 0 {
            // Level with upstream: the branch was not touched, only submodules moved.
            let moved: Vec<&str> = outcome
                .submodules
                .iter()
                .filter(|s| matches!(s.action.as_str(), "fast-forwarded" | "rebased"))
                .map(|s| s.path.as_str())
                .collect();
            if moved.is_empty() {
                format!("Nothing to update — you are level with {}.", st.plan.superproject.upstream)
            } else {
                format!("Submodules aligned: {}. Your branch was already level with {}.", moved.join(", "), st.plan.superproject.upstream)
            }
        } else if outcome.own_kept_total == 0 {
            format!(
                "Updated: fast-forwarded to {} from {}.",
                plural(outcome.incoming, "new commit", "new commits"),
                st.plan.superproject.upstream
            )
        } else {
            format!(
                "Updated: {} on top of {} from {}.",
                plural(outcome.own_kept_total, "commit of yours", "commits of yours"),
                plural(outcome.incoming, "new commit", "new commits"),
                st.plan.superproject.upstream
            )
        };

        let mut detail = String::new();
        let subjects = |cs: &[&CommitRef]| cs.iter().map(|c| c.subject.clone()).collect::<Vec<_>>().join("; ");
        let away: Vec<&CommitRef> = outcome.own_dropped.iter().filter(|c| c.resolved_away).collect();
        let (expected, lost): (Vec<&CommitRef>, Vec<&CommitRef>) = outcome.own_dropped.iter().filter(|c| !c.resolved_away).partition(|c| c.already_upstream);
        if !expected.is_empty() {
            detail.push_str(&format!(
                " {} of yours {} dropped because upstream already contained {}: {}.",
                plural(expected.len(), "commit", "commits"),
                if expected.len() == 1 { "was" } else { "were" },
                if expected.len() == 1 { "it" } else { "them" },
                subjects(&expected)
            ));
        }
        if !away.is_empty() {
            detail.push_str(&format!(
                " {} of yours {} left out: nothing of {} remained once {} resolved — {}.",
                plural(away.len(), "commit", "commits"),
                if away.len() == 1 { "was" } else { "were" },
                if away.len() == 1 { "it" } else { "them" },
                if away.len() == 1 { "its conflict was" } else { "their conflicts were" },
                subjects(&away)
            ));
        }
        if !lost.is_empty() {
            detail.push_str(&format!(
                " {} of yours {} missing after the replay and not found upstream: {}. The originals are on the backup branch — see Backups.",
                plural(lost.len(), "commit", "commits"),
                if lost.len() == 1 { "is" } else { "are" },
                subjects(&lost)
            ));
        }
        // Every drop was one the preview foresaw and every other commit of
        // yours is on top: the result is the preview, nothing more. A drop git
        // found while replaying is still fine — but it was not in the preview,
        // and the result says so instead.
        let foreseen = |c: &CommitRef| {
            st.plan.superproject.own_commits.iter().any(|p| p.oid == c.oid && p.already_upstream) || st.plan.superproject.unlisted_dropped.contains(&c.oid)
        };
        let unforeseen: Vec<&CommitRef> = expected.iter().copied().filter(|c| !foreseen(c)).collect();
        // The other way round: the preview said git would leave it out, and git kept it.
        let kept_after_all: Vec<&CommitRef> = st
            .plan
            .superproject
            .own_commits
            .iter()
            .filter(|p| p.already_upstream && !outcome.own_dropped.iter().any(|d| d.oid == p.oid))
            .collect();
        if !kept_after_all.is_empty() {
            detail.push_str(&format!(
                " Not as previewed: {} of yours the preview expected git to leave out {} kept — {}.",
                plural(kept_after_all.len(), "commit", "commits"),
                if kept_after_all.len() == 1 { "was" } else { "were" },
                subjects(&kept_after_all)
            ));
        }
        // A pointer the update had to hand over without having said so
        // beforehand is not "as previewed" either.
        let surprise_hand_over = st.handed_over.iter().any(|p| !st.plan.submodules.iter().any(|s| &s.path == p && s.predicted_gitlink_conflict));
        if verified && unforeseen.is_empty() && away.is_empty() && kept_after_all.is_empty() && outcome.incoming > 0 && !surprise_hand_over {
            detail.push_str(&format!(
                " As previewed: {} of yours on top of {} incoming.",
                plural(outcome.own_kept_total, "commit", "commits"),
                outcome.incoming
            ));
        } else if verified && !unforeseen.is_empty() {
            detail.push_str(&format!(
                " Not in the preview: git left out {} of yours whose changes upstream already had — {}.",
                plural(unforeseen.len(), "commit", "commits"),
                subjects(&unforeseen)
            ));
        }
        for s in &unaligned {
            detail.push_str(&format!(" {}: {}.", s.path, s.note.trim_end_matches('.')));
        }
        if !outcome.unrecorded_pointers.is_empty() {
            detail.push_str(&format!(" Submodule pointers not yet recorded by this branch: {}. Use \"Record submodule pointers\" when you want a commit for them.", outcome.unrecorded_pointers.join(", ")));
        }
        if outcome.stashed {
            detail.push_str(" Your uncommitted changes were stashed and re-applied (staged changes come back unstaged).");
        }
        if !outcome.verified.dirty_paths_unchanged {
            detail.push_str(" The set of files with uncommitted changes differs from before the update — check the file list.");
        }
        detail.push_str(&large_files);
        let mut r = OpResult::done(headline, detail.trim().to_string(), status);
        r.ok = verified;
        r
    } else if st.phase == "aborted" {
        std::fs::remove_file(state_file).ok();
        OpResult::done("Update aborted.".to_string(), format!("Everything is back where it was, except what is named below.{}", large_files), status)
    } else {
        let nu = st.needs_user.clone().unwrap_or(NeedsUser { where_: "superproject".into(), paths: vec![], hint: String::new(), at_commit: None });
        let mut r = OpResult::done(
            match nu.where_.as_str() {
                "autostash-conflict" => "Update paused: your stashed changes did not re-apply cleanly.".to_string(),
                w if w.starts_with("submodule") => format!("Update paused inside {}.", w.split_once(':').map(|x| x.1).unwrap_or("a submodule")),
                _ => "Update paused: conflicts to resolve.".to_string(),
            },
            nu.hint.clone(),
            status,
        );
        r.ok = false;
        r
    };
    op.sync = Some(outcome);
    Ok(op)
}

/// What the run actually did to a submodule — from its state, not from what
/// the plan said would happen: fast-forwarded | rebased | left-ahead |
/// unchanged | could-not-fast-forward | blocked.
fn outcome_action(s: &SubState, planned: Option<&str>, phase: &str) -> String {
    if s.note.starts_with("put back") {
        // An abort undid what the update had done to it.
        "put-back"
    } else if s.note.starts_with("left as") || s.note.starts_with("left at") || s.note.starts_with("could not be put back") {
        "left-as-moved"
    } else if s.note.starts_with("could not be aligned") {
        "could-not-align"
    } else if s.note.starts_with("new upstream; downloaded") || s.note.starts_with("new upstream; already downloaded") {
        "downloaded"
    } else if s.note.starts_with("new upstream") {
        "could-not-download"
    } else if s.rebased {
        "rebased"
    } else if s.note.starts_with("fast-forwarded") {
        "fast-forwarded"
    } else if s.note.starts_with("could not fast-forward") {
        "could-not-fast-forward"
    } else if s.note.starts_with("your commits sit on top") {
        "left-ahead"
    } else if s.note.starts_with("already at") {
        "unchanged"
    } else {
        // Never touched. A finished run did what was planned; one that was
        // aborted left it alone; one that is paused has not reached it yet —
        // the plan's verb would claim work that did not happen.
        match (phase, planned) {
            (_, Some("blocked")) => "blocked",
            ("done", Some("none")) | ("done", None) => "unchanged",
            ("done", Some(other)) => other,
            ("aborted", _) => "unchanged",
            (_, Some("none")) | (_, None) => "unchanged",
            _ => "pending",
        }
    }
    .to_string()
}

/// One commit as the plan lists them, for one the plan did not list.
async fn commit_ref(repo: &Path, oid: &str) -> Option<CommitRef> {
    commit_list(repo, &format!("{}^!", oid)).await.into_iter().next()
}

/// A commit as a replay keeps it. A rebase gives a commit a new id and a new
/// committer; who wrote it, when, and its subject it leaves alone — so those
/// tell which commit on top is the copy of which original far more surely
/// than the subject alone ("wip" twice is two different moments).
struct Replayed {
    oid: String,
    /// Author name, address, time and the subject.
    key: String,
    subject: String,
}

/// Every non-merge commit in `range`, newest first — read untrimmed and
/// NUL-separated, so an empty subject is a subject and not a missing line.
/// `None` when the range cannot be read (a branch that is gone).
async fn replayed(repo: &Path, range: &str) -> Option<Vec<Replayed>> {
    let out = g(repo).args(["log", "--no-merges", "-z", "--format=%H%x1f%an%x1f%ae%x1f%at%x1f%s", range]).run().await.ok()?;
    if !out.ok() {
        return None;
    }
    Some(parse_replayed(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_replayed(text: &str) -> Vec<Replayed> {
    text.split('\0')
        .filter_map(|rec| {
            let (oid, key) = rec.split_once('\x1f')?;
            let subject = key.splitn(4, '\x1f').nth(3).unwrap_or("");
            Some(Replayed { oid: oid.trim().to_string(), key: key.to_string(), subject: subject.to_string() })
        })
        .collect()
}

/// The commits on top, each able to vouch for one original: by author, time
/// and subject first, by subject alone as a second chance (a copy whose
/// author date was reset by hand).
struct Copies {
    by_key: BTreeMap<String, Vec<usize>>,
    by_subject: BTreeMap<String, Vec<usize>>,
    claimed: Vec<bool>,
}

impl Copies {
    fn new(copies: &[Replayed]) -> Self {
        let mut by_key: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut by_subject: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, c) in copies.iter().enumerate() {
            by_key.entry(c.key.clone()).or_default().push(i);
            by_subject.entry(c.subject.clone()).or_default().push(i);
        }
        Copies { by_key, by_subject, claimed: vec![false; copies.len()] }
    }
    fn free(&self, key: &str) -> usize {
        self.by_key.get(key).map(|v| v.iter().filter(|i| !self.claimed[**i]).count()).unwrap_or(0)
    }
    fn take(list: Option<&Vec<usize>>, claimed: &mut [bool]) -> bool {
        match list.and_then(|v| v.iter().find(|i| !claimed[**i])) {
            Some(i) => {
                claimed[*i] = true;
                true
            }
            None => false,
        }
    }
    fn claim(&mut self, key: &str) -> bool {
        Self::take(self.by_key.get(key), &mut self.claimed)
    }
    fn claim_subject(&mut self, subject: &str) -> bool {
        Self::take(self.by_subject.get(subject), &mut self.claimed)
    }
}

/// Which originals have no copy on top. `originals` are the commits not
/// already known to have been left out, each with: was it committed by the
/// app after a resolution (`certain`), and does HEAD already hold everything
/// it did (`present`, asked only when it matters). Where a key has fewer
/// copies than originals, the ones surest to be on top take them — so a
/// commit that is really gone is never excused by a namesake.
fn unmatched<'a>(originals: &'a [Replayed], copies: &mut Copies, certain: &dyn Fn(&str) -> bool, present: &BTreeMap<String, bool>) -> Vec<&'a Replayed> {
    let mut groups: BTreeMap<&str, Vec<&Replayed>> = BTreeMap::new();
    for o in originals {
        groups.entry(o.key.as_str()).or_default().push(o);
    }
    let mut missing: Vec<&Replayed> = Vec::new();
    for (key, mut members) in groups {
        if copies.free(key) < members.len() {
            members.sort_by_key(|m| (!certain(&m.oid), !present.get(&m.oid).copied().unwrap_or(false)));
        }
        for m in members {
            if !copies.claim(key) {
                missing.push(m);
            }
        }
    }
    // A second chance by subject alone, in the same order of sureness.
    missing.sort_by_key(|m| (!certain(&m.oid), !present.get(&m.oid).copied().unwrap_or(false)));
    missing.into_iter().filter(|m| !copies.claim_subject(&m.subject)).collect()
}

/// Would replaying this commit onto HEAD change nothing — is everything it
/// did already there? Asked of git itself: a merge of the commit onto HEAD
/// with its parent as the base, compared with HEAD's own tree. A conflict, a
/// root commit, or a git too old to answer (before 2.40) all mean "cannot
/// say", which is no.
async fn adds_nothing(repo: &Path, commit: &str) -> bool {
    let Ok(out) = g(repo).args(["merge-tree", "--write-tree", &format!("--merge-base={}^", commit), "HEAD", commit]).run().await else { return false };
    if !out.ok() {
        return false;
    }
    let tree = out.text().lines().next().unwrap_or("").trim().to_string();
    !tree.is_empty() && oid(repo, "HEAD^{tree}").await.as_deref() == Some(tree.as_str())
}

/// Which of the commits the plan listed are no longer on top by subject, each
/// subject on top vouching for one commit. On top means every missing one
/// was foreseen as already upstream.
fn subject_fallback(own: &[CommitRef], mut subjects: BTreeMap<String, usize>) -> (bool, Vec<CommitRef>) {
    let mut dropped = Vec::new();
    for c in own {
        match subjects.get_mut(c.subject.as_str()) {
            Some(n) if *n > 0 => *n -= 1,
            _ => dropped.push(c.clone()),
        }
    }
    let on_top = dropped.iter().all(|c| c.already_upstream);
    (on_top, dropped)
}

async fn build_outcome(repo: &Path, st: &SyncState) -> SyncOutcome {
    let head_after = oid(repo, "HEAD").await.unwrap_or_default();
    let own_after = commit_list(repo, &format!("{}..HEAD", st.upstream_ref)).await;
    // Measured, not assumed. Every commit of mine the backup holds is either
    // known to have been left out — upstream already had it (a twin by `git
    // cherry`, foreseen by the plan, or announced by git as "patch contents
    // already upstream"), or its stop was resolved into nothing — or it must
    // have a copy on top: a commit with its author, its time and its
    // subject. One with no copy was left out by a rebase the app did not
    // watch (finished in a terminal): fine when HEAD already holds everything
    // it did, a loss otherwise.
    let (own_on_top, own_dropped) = match st.backup.as_deref() {
        Some(backup) if st.phase == "done" => match replayed(repo, &format!("HEAD..{}", backup)).await {
            Some(originals) => {
                let twins: BTreeSet<String> = g(repo)
                    .args(["cherry", &st.upstream_ref, backup])
                    .ok_text()
                    .await
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|l| l.strip_prefix("- "))
                    .map(|o| o.trim().to_string())
                    .collect();
                let on_top_now = replayed(repo, &format!("{}..HEAD", st.upstream_ref)).await.unwrap_or_default();
                let mut copies = Copies::new(&on_top_now);
                let listed = |oid: &str| st.plan.superproject.own_commits.iter().find(|c| c.oid == oid).cloned();
                let mut dropped: Vec<CommitRef> = Vec::new();
                let mut pending: Vec<Replayed> = Vec::new();
                // Foreseen as already upstream by the plan's predictor, but
                // neither a twin nor announced by git: a forecast, checked
                // against the copies on top once the certain ones have theirs.
                let mut forecast: Vec<Replayed> = Vec::new();
                for o in originals {
                    let foreseen = listed(&o.oid).is_some_and(|c| c.already_upstream) || st.plan.superproject.unlisted_dropped.contains(&o.oid);
                    let away = st.emptied.contains(&o.oid);
                    let certain = away || twins.contains(&o.oid) || st.dropped_by_git.contains(&o.oid);
                    if !certain && !foreseen {
                        pending.push(o);
                        continue;
                    }
                    if !certain {
                        forecast.push(o);
                        continue;
                    }
                    let Some(mut c) = (match listed(&o.oid) {
                        Some(c) => Some(c),
                        None => commit_ref(repo, &o.oid).await,
                    }) else {
                        continue;
                    };
                    c.resolved_away = away;
                    c.already_upstream = !away;
                    dropped.push(c);
                }
                // Does HEAD already hold everything it did? Asked only where
                // it can matter: for a commit whose key has fewer copies
                // than originals.
                let mut wanted: BTreeMap<&str, usize> = BTreeMap::new();
                for o in &pending {
                    *wanted.entry(o.key.as_str()).or_default() += 1;
                }
                let mut present: BTreeMap<String, bool> = BTreeMap::new();
                for o in &pending {
                    if copies.free(&o.key) < wanted[o.key.as_str()] {
                        present.insert(o.oid.clone(), adds_nothing(repo, &o.oid).await);
                    }
                }
                let certain = |oid: &str| st.resolved.iter().any(|r| r == oid);
                let mut on_top = true;
                for m in unmatched(&pending, &mut copies, &certain, &present) {
                    let Some(mut c) = (match listed(&m.oid) {
                        Some(c) => Some(c),
                        None => commit_ref(repo, &m.oid).await,
                    }) else {
                        continue;
                    };
                    if present.get(&m.oid).copied().unwrap_or(false) {
                        // Everything it did is there: left out as already
                        // upstream by a rebase finished outside the app.
                        c.already_upstream = true;
                    } else {
                        c.already_upstream = false;
                        on_top = false;
                    }
                    dropped.push(c);
                }
                for o in forecast {
                    if copies.claim(&o.key) {
                        // Kept after all: git did not find it empty.
                        continue;
                    }
                    let Some(mut c) = (match listed(&o.oid) {
                        Some(c) => Some(c),
                        None => commit_ref(repo, &o.oid).await,
                    }) else {
                        continue;
                    };
                    c.already_upstream = true;
                    dropped.push(c);
                }
                (on_top, dropped)
            }
            // The backup is gone (deleted by hand): nothing to measure against.
            // Subjects decide, and a missing one is a drop.
            None => {
                let mut subjects: BTreeMap<String, usize> = BTreeMap::new();
                for c in replayed(repo, &format!("{}..HEAD", st.upstream_ref)).await.unwrap_or_default() {
                    *subjects.entry(c.subject).or_default() += 1;
                }
                subject_fallback(&st.plan.superproject.own_commits, subjects)
            }
        },
        // Paused or aborted: nothing has been judged yet.
        _ => (true, Vec::new()),
    };
    let counts = g(repo).args(["rev-list", "--left-right", "--count", &format!("HEAD...{}", st.upstream_ref)]).ok_text().await.unwrap_or_default();
    let behind: usize = counts.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
    let own_kept_total: usize = counts.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(own_after.len());
    let (dirty_now, untracked_now) = dirty_sets(repo).await;
    let dirty_before = st.captures.get("").cloned().unwrap_or_default();
    let dirty_set_now: BTreeSet<String> = dirty_now.iter().cloned().collect();
    let dirty_set_before: BTreeSet<String> = dirty_before.iter().cloned().collect();
    // An untracked nested repository that the update adopted as a submodule
    // (upstream added one at its path) is not an untracked file that vanished.
    let untracked_before = match st.captures.get("untracked") {
        Some(v) => {
            let mut n = v.len();
            for p in v {
                if let Some(dir) = p.strip_suffix('/') {
                    if gitlink_at(repo, "HEAD", dir).await.is_some() {
                        n -= 1;
                    }
                }
            }
            n
        }
        None => untracked_now.len(),
    };
    let mut backups = Vec::new();
    if let Some(b) = &st.backup {
        backups.push(BackupRef { repo: repo.to_string_lossy().to_string(), branch: b.clone(), oid: st.head_before.clone(), recovery: recovery(repo, b) });
    }
    let mut subs = Vec::new();
    let mut aligned_all = true;
    let mut unrecorded = Vec::new();
    for s in &st.subs {
        let sub = repo.join(&s.path);
        if let Some(b) = &s.backup {
            backups.push(BackupRef { repo: sub.to_string_lossy().to_string(), branch: b.clone(), oid: s.head_before.clone(), recovery: recovery(&sub, b) });
        }
        let recorded = gitlink_at(repo, "HEAD", &s.path).await;
        let head_now = oid(&sub, "HEAD").await;
        let ok = match (&recorded, &head_now) {
            (Some(r), Some(h)) => same_oid(r, h) || is_ancestor(&sub, r, h).await,
            _ => false,
        };
        if !ok {
            aligned_all = false;
        }
        if let (Some(r), Some(h)) = (&recorded, &head_now) {
            if !same_oid(r, h) && is_ancestor(&sub, r, h).await {
                unrecorded.push(s.path.clone());
            }
        }
        subs.push(SubOutcome {
            path: s.path.clone(),
            action: outcome_action(s, st.plan.submodules.iter().find(|p| p.path == s.path).map(|p| p.action.as_str()), &st.phase),
            head_before: Some(s.head_before.clone()),
            head_after: head_now,
            recorded_after: recorded,
            ok,
            note: match &s.url_note {
                Some(u) if s.note.is_empty() => u.clone(),
                Some(u) => format!("{}; {}", s.note, u),
                None => s.note.clone(),
            },
        });
    }
    SyncOutcome {
        phase: match st.phase.as_str() {
            "done" => "done".into(),
            "aborted" => "aborted".into(),
            _ => "needs-user".into(),
        },
        needs_user: st.needs_user.clone(),
        backups,
        bundles: st.bundles.clone(),
        head_before: st.head_before.clone(),
        head_after,
        incoming: st.plan.superproject.behind,
        own_kept: own_after,
        own_kept_total,
        own_dropped,
        stashed: st.stash && !dirty_before.is_empty(),
        stash_left: autostash_left(repo, st.stash_before.as_deref()).await,
        submodules: subs,
        verified: Verified {
            behind_zero: behind == 0,
            own_on_top,
            dirty_paths_unchanged: dirty_set_now == dirty_set_before,
            untracked_unchanged: untracked_now.len() == untracked_before,
            submodules_aligned: aligned_all,
            no_conflicts_left: unmerged(repo).await.is_empty() && !rebase_in_progress(repo).await,
        },
        unrecorded_pointers: unrecorded,
    }
}

async fn write_bundle(repo: &Path, dir: &Path, name: &str) -> Result<String, AppError> {
    std::fs::create_dir_all(dir)?;
    let final_path = dir.join(format!("{}.bundle", name));
    let tmp = dir.join(format!("{}.bundle.tmp", name));
    let out = g(repo).args(["bundle", "create", &tmp.to_string_lossy(), "--all"]).timeout(NET_TIMEOUT).run().await?;
    if !out.ok() {
        let _ = std::fs::remove_file(&tmp);
        return Err(AppError::Command(format!("bundle failed: {}", out.stderr.trim())));
    }
    std::fs::rename(&tmp, &final_path)?;
    Ok(final_path.to_string_lossy().to_string())
}

/// The whole operation, from a plan the user approved.
pub async fn sync_run(repo_path: &str, opts: RunOptions) -> Result<OpResult, AppError> {
    let (status, f) = facts_for_check(repo_path).await?;
    if let Some(r) = check(&Intent::Sync, &f) {
        return Ok(OpResult::refused(r, status));
    }
    let repo = PathBuf::from(repo_path);
    // Fetch — the drawer promises it — then measure again and compare with
    // what was approved: upstream having moved is a refusal (assess-again),
    // never a silent change of plan.
    let mut facts = gather_facts(repo_path, true).await?;
    note_identity(&mut facts, &f);
    if !opts.fingerprint.is_empty() {
        let mut now = vec![(String::new(), facts.superproject.upstream_oid.clone())];
        for s in &facts.submodules {
            if let Some(t) = &s.target {
                now.push((s.path.clone(), t.clone()));
            }
        }
        if now != opts.fingerprint {
            return Ok(OpResult::refused(
                Refusal { code: "assess-again".into(), message: "Upstream moved since you assessed. Assess again, then update.".into() },
                status,
            ));
        }
    }
    let plan = plan(&facts, opts.stash);
    if !plan.can_run {
        let msg = if plan.nothing_to_do { plan.summary.clone() } else { plan.blockers.join(" ") };
        return Ok(OpResult::refused(Refusal { code: if plan.nothing_to_do { "nothing-to-sync".into() } else { "sync-blocked".into() }, message: msg }, status));
    }

    let state_file = state_path(&repo).await?;
    let id = stamp();
    let head_before = facts.superproject.head.clone();
    let (dirty, untracked) = dirty_sets(&repo).await;
    let mut captures = BTreeMap::new();
    captures.insert(String::new(), dirty.clone());
    captures.insert("untracked".into(), untracked);
    let mut st = SyncState {
        version: STATE_VERSION,
        nonce: uuid::Uuid::new_v4().to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        phase: "preparing".into(),
        repo: repo_path.to_string(),
        upstream_ref: facts.superproject.upstream_ref.clone(),
        upstream_oid: facts.superproject.upstream_oid.clone(),
        head_before: head_before.clone(),
        // Level with upstream there is no superproject rebase, so nothing is stashed.
        stash: opts.stash && !dirty.is_empty() && facts.superproject.behind > 0,
        backup: None,
        captures,
        subs: plan
            .submodules
            .iter()
            .filter(|p| p.action != "blocked")
            .map(|p| SubState {
                path: p.path.clone(),
                branch_before: p.branch.clone(),
                reattach_to: p.reattach_to.clone(),
                head_before: p.head.clone().unwrap_or_default(),
                backup: None,
                target: p.target.clone().unwrap_or_default(),
                rebased: false,
                head_after: None,
                aligned: false,
                note: String::new(),
                stash_before: None,
                reattached: None,
                // Unmasked, from the facts: the state compares it with the
                // configuration when an abort puts the address back.
                url_change: facts.submodules.iter().find(|f| f.path == p.path).and_then(|f| f.url_change.clone()),
                url_synced: false,
                url_note: None,
                remote: p.remote.clone(),
            })
            .collect(),
        needs_user: None,
        plan: plan.clone(),
        bundles: Vec::new(),
        resolved: Vec::new(),
        dropped_by_git: Vec::new(),
        emptied: Vec::new(),
        stash_before: None,
        git_said: String::new(),
        handed_over: Vec::new(),
        pointer_upstreams: Vec::new(),
        lfs: BTreeMap::new(),
        lfs_skip: false,
    };
    let (lfs, lfs_skip) = lfs_plan(&repo, &st.subs).await;
    st.lfs = lfs;
    st.lfs_skip = lfs_skip;
    write_state_at(&state_file, &st)?;

    // Backups first; a name collision fails closed.
    let backup = format!("{}{}", BACKUP_PREFIX, id);
    if let Err(e) = make_backup(&repo, &backup).await {
        let _ = std::fs::remove_file(&state_file);
        return Err(e);
    }
    st.backup = Some(backup.clone());
    for i in 0..st.subs.len() {
        let action = plan.submodules.iter().find(|p| p.path == st.subs[i].path).map(|p| p.action.clone()).unwrap_or_default();
        if action == "rebase" {
            let sub = repo.join(&st.subs[i].path);
            match make_backup(&sub, &backup).await {
                Ok(_) => st.subs[i].backup = Some(backup.clone()),
                Err(e) => {
                    let _ = g(&repo).args(["branch", "-D", &backup]).run().await;
                    let _ = std::fs::remove_file(&state_file);
                    return Err(e);
                }
            }
        }
    }
    if opts.bundles {
        if let Some(dir) = dirs::data_dir().map(|d| d.join("com.gitswitch.app").join("sync-backups").join(format!("{}-{}", crate::paths::base_name(repo_path), id))) {
            if let Ok(p) = write_bundle(&repo, &dir, "superproject").await {
                st.bundles.push(p);
            }
            for s in &st.subs {
                if s.backup.is_some() {
                    if let Ok(p) = write_bundle(&repo.join(&s.path), &dir, &s.path.replace('/', "__")).await {
                        st.bundles.push(p);
                    }
                }
            }
        }
    }
    st.stash_before = Some(stash_top(&repo).await);
    st.phase = "rebasing".into();
    write_state_at(&state_file, &st)?;

    // Level with upstream already — only submodules need aligning: there is
    // nothing to replay onto, and a rebase could only re-write history that
    // needs no change (it would flatten a merge of yours for nothing).
    if facts.superproject.behind > 0 {
        let upstream_ref = st.upstream_ref.clone();
        let mut args = vec!["rebase", "--merge"];
        args.push(if st.stash { "--autostash" } else { "--no-autostash" });
        args.push(&upstream_ref);
        let out = rw(&repo, st.lfs_skip).args(args).timeout(NET_TIMEOUT).run().await?;
        note_drops(&mut st, &out.stderr);
        if !out.ok() && !rebase_in_progress(&repo).await {
            // Refused before starting (a hook, an untracked file): nothing to undo but the backup branches.
            st.phase = "aborted".into();
            write_state_at(&state_file, &st)?;
            let after = Box::pin(git_status::repo_status(repo_path)).await?;
            let a = crate::git_advice::explain(crate::git_advice::GitOp::Rebase, &format!("{}\n{}", out.text(), out.stderr), &crate::git_ops::advice_ctx(&after));
            let _ = std::fs::remove_file(&state_file);
            let mut r = OpResult::failed(a, after);
            r.sync = Some(build_outcome(&repo, &st).await);
            return Ok(r);
        }
    }
    Box::pin(drive(&repo, &mut st, &state_file)).await?;
    Box::pin(finish(repo_path, &st, &state_file)).await
}

/// After the user resolved and staged: continue the innermost rebase, then
/// resume the state machine.
pub async fn sync_continue(repo_path: &str, allow_markers: bool) -> Result<OpResult, AppError> {
    let (status, f) = facts_for_check(repo_path).await?;
    if let Some(r) = check(&Intent::SyncContinue, &f) {
        return Ok(OpResult::refused(r, status));
    }
    let repo = PathBuf::from(repo_path);
    let state_file = state_path(&repo).await?;
    let Some(mut st) = read_state(&repo).await else {
        return Ok(OpResult::refused(Refusal { code: "no-sync".into(), message: "No update is paused here.".into() }, status));
    };
    // A file staged with its conflict markers still in it would be committed
    // that way by `rebase --continue`; refuse by name unless told otherwise.
    if let Some(path) = sub_rebase_in_progress(&repo, &st).await {
        let sub = repo.join(&path);
        if !unmerged(&sub).await.is_empty() {
            let status = Box::pin(git_status::repo_status(repo_path)).await?;
            return Ok(OpResult::refused(Refusal { code: "unmerged-paths".into(), message: format!("{} still has conflicted files. Resolve and stage them inside it first.", path) }, status));
        }
        if !allow_markers {
            if let Some((file, lines)) = crate::git_ops::staged_marker_offence(&sub).await {
                let status = Box::pin(git_status::repo_status(repo_path)).await?;
                return Ok(OpResult::refused(crate::git_ops::markers_refusal(&format!("{}/{}", path, file), &lines), status));
            }
        }
        crate::git_ops::commit_resolved_pick(&sub).await;
        let out = rw(&sub, st.lfs_skip).args(["rebase", "--continue"]).timeout(NET_TIMEOUT).run().await?;
        if !out.ok() && rebase_in_progress(&sub).await {
            let status = Box::pin(git_status::repo_status(repo_path)).await?;
            return Ok(OpResult::refused(Refusal { code: "sub-rebase-stopped".into(), message: format!("Rebasing {} stopped again: {}", path, out.stderr.trim()) }, status));
        }
        // Its rebase finished, but its own stashed changes may not have come
        // back cleanly: that is another pause, not something to add silently.
        let sub_stash_before = st.subs.iter().find(|s| s.path == path).and_then(|s| s.stash_before.clone());
        if !unmerged(&sub).await.is_empty() || autostash_left(&sub, sub_stash_before.as_deref()).await {
            if let Some(ss) = st.subs.iter_mut().find(|s| s.path == path) {
                ss.rebased = true;
            }
            st.needs_user = Some(NeedsUser {
                where_: format!("submodule-autostash:{}", path),
                paths: vec![path.clone()],
                hint: format!("{} was rebased, but its uncommitted changes did not re-apply cleanly. Resolve and stage them inside it, then Continue update.", path),
                at_commit: None,
            });
            write_state_at(&state_file, &st)?;
            return finish(repo_path, &st, &state_file).await;
        }
        if let Some(ss) = st.subs.iter_mut().find(|s| s.path == path) {
            let new_head = oid(&sub, "HEAD").await.unwrap_or_default();
            reattach(&sub, ss).await;
            ss.head_after = Some(new_head);
            ss.rebased = true;
            if ss.note.is_empty() || ss.note.contains("stopped") {
                ss.note = "rebased (conflicts resolved by you)".into();
            }
        }
        st.needs_user = None;
        return Box::pin(resume_after_submodule(repo_path, &repo, &mut st, &state_file, &path)).await;
    }
    if let Some(path) = st.needs_user.as_ref().and_then(|n| n.where_.strip_prefix("submodule-autostash:")).map(|p| p.to_string()) {
        let sub = repo.join(&path);
        if !unmerged(&sub).await.is_empty() {
            let status = Box::pin(git_status::repo_status(repo_path)).await?;
            return Ok(OpResult::refused(Refusal { code: "unmerged-paths".into(), message: format!("{} still has conflicted files. Resolve and stage them inside it first.", path) }, status));
        }
        // The stash entry it left is theirs to drop; the rebase itself is done.
        if let Some(ss) = st.subs.iter_mut().find(|s| s.path == path) {
            let new_head = oid(&sub, "HEAD").await.unwrap_or_default();
            reattach(&sub, ss).await;
            ss.head_after = Some(new_head);
            ss.rebased = true;
            if ss.note.is_empty() {
                ss.note = "rebased (its stashed changes re-applied by you)".into();
            }
        }
        st.needs_user = None;
        return Box::pin(resume_after_submodule(repo_path, &repo, &mut st, &state_file, &path)).await;
    }
    if st.needs_user.as_ref().map(|n| n.where_.as_str()) == Some("autostash-conflict") {
        if !unmerged(&repo).await.is_empty() {
            let status = Box::pin(git_status::repo_status(repo_path)).await?;
            return Ok(OpResult::refused(Refusal { code: "unmerged-paths".into(), message: "Resolve and stage the conflicted files first.".into() }, status));
        }
        // The user staged the resolution of the stash. The leftover stash entry is theirs to drop; carry on.
        st.needs_user = None;
        align_submodules(&repo, &mut st, &state_file).await?;
        return finish(repo_path, &st, &state_file).await;
    }
    // A pointer the update handed over ("put it where you want it, stage it,
    // then Continue"): until it is staged there is nothing to continue with —
    // git would refuse, and the same pause would come straight back.
    if let Some(path) = st.needs_user.as_ref().and_then(|n| n.where_.strip_prefix("submodule:")).map(|p| p.to_string()) {
        if unmerged(&repo).await.iter().any(|c| c.path == path) {
            let status = Box::pin(git_status::repo_status(repo_path)).await?;
            return Ok(OpResult::refused(
                Refusal { code: "unmerged-paths".into(), message: format!("{} is still unmerged here. Put it where you want it and stage it, then Continue update.", path) },
                status,
            ));
        }
    }
    if rebase_in_progress(&repo).await {
        if !unmerged(&repo).await.iter().any(|c| !c.is_submodule) {
            if !allow_markers {
                if let Some((file, lines)) = crate::git_ops::staged_marker_offence(&repo).await {
                    let status = Box::pin(git_status::repo_status(repo_path)).await?;
                    return Ok(OpResult::refused(crate::git_ops::markers_refusal(&file, &lines), status));
                }
            }
            // Resolved by hand: its patch is now its own, and the check
            // afterwards must not call it dropped.
            remember_resolved(&repo, &mut st).await;
            crate::git_ops::commit_resolved_pick(&repo).await;
            let out = rw(&repo, st.lfs_skip).args(["rebase", "--continue"]).timeout(NET_TIMEOUT).run().await?;
            note_drops(&mut st, &out.stderr);
            if !out.ok() && !rebase_in_progress(&repo).await {
                return Err(AppError::Command(format!("rebase --continue failed: {}", out.stderr.trim())));
            }
            st.needs_user = None;
            drive(&repo, &mut st, &state_file).await?;
            return finish(repo_path, &st, &state_file).await;
        }
        let status = Box::pin(git_status::repo_status(repo_path)).await?;
        return Ok(OpResult::refused(Refusal { code: "unmerged-paths".into(), message: "Resolve and stage every conflicted file first.".into() }, status));
    }
    // Finished or aborted outside the app.
    st.needs_user = None;
    // Aborted outside: the branch is back where it started — whether its
    // rebase was aborted or the branch reset by hand after it. (Not when the
    // run was level with upstream: there never was a rebase, and HEAD has not
    // moved for that reason alone.)
    if st.plan.superproject.behind > 0 && oid(&repo, "HEAD").await.as_deref() == Some(st.head_before.as_str()) {
        // What the update had already moved goes back too, as Abort would.
        let mut notes = put_back_submodules(&repo, &mut st).await;
        notes.extend(put_back_urls(&repo, &mut st).await);
        st.phase = "aborted".into();
        let mut r = finish(repo_path, &st, &state_file).await?;
        if !notes.is_empty() {
            r.detail = notes.join(" ");
        }
        return Ok(r);
    }
    align_submodules(&repo, &mut st, &state_file).await?;
    finish(repo_path, &st, &state_file).await
}

/// A submodule's pause is over: record it in the superproject's stop and carry
/// on there, or — when the superproject's rebase had already finished —
/// align whatever is left.
async fn resume_after_submodule(repo_path: &str, repo: &Path, st: &mut SyncState, state_file: &Path, path: &str) -> Result<OpResult, AppError> {
    if rebase_in_progress(repo).await {
        // The stop this pause belongs to: what my commit pointed at (stage 3)
        // and what upstream records (stage 2), so the rewritten copy of the
        // former is what gets recorded.
        let sub = repo.join(path);
        let new_head = oid(&sub, "HEAD").await.unwrap_or_default();
        let stop = unmerged(repo).await.into_iter().find(|c| c.path == path && c.stage_oids.len() == 3);
        let (old_tip, target) = st
            .subs
            .iter()
            .find(|s| s.path == path)
            .map(|s| (s.head_before.clone(), s.target.clone()))
            .unwrap_or_default();
        let record = match &stop {
            Some(c) => {
                let onto = if target.is_empty() { c.stage_oids[1].clone() } else { target.clone() };
                match rewritten_pointer(&sub, &c.stage_oids[2], &onto, &old_tip, &new_head).await {
                    Ok(Some(copy)) => copy,
                    Ok(None) => {
                        if let Some(sha) = stopped_sha(repo).await {
                            if !st.pointer_upstreams.contains(&sha) {
                                st.pointer_upstreams.push(sha);
                            }
                        }
                        let ours = c.stage_oids[1].clone();
                        if same_oid(&onto, &ours) || is_ancestor(&sub, &onto, &ours).await { ours } else { onto }
                    }
                    Err(why) => {
                        st.needs_user = Some(NeedsUser { where_: format!("submodule:{}", path), paths: vec![path.to_string()], hint: why, at_commit: stopped_sha(repo).await });
                        if !st.handed_over.contains(&path.to_string()) {
                            st.handed_over.push(path.to_string());
                        }
                        write_state_at(state_file, st)?;
                        return finish(repo_path, st, state_file).await;
                    }
                }
            }
            None => new_head.clone(),
        };
        stage_pointer(repo, path, &record, &new_head).await?;
        // The commit being replayed now records a rewritten pointer.
        remember_resolved(repo, st).await;
        crate::git_ops::commit_resolved_pick(repo).await;
        let out = rw(repo, st.lfs_skip).args(["rebase", "--continue"]).timeout(NET_TIMEOUT).run().await?;
        note_drops(st, &out.stderr);
        if !out.ok() && !rebase_in_progress(repo).await {
            return Err(AppError::Command(format!("rebase --continue failed: {}", out.stderr.trim())));
        }
        drive(repo, st, state_file).await?;
    } else {
        align_submodules(repo, st, state_file).await?;
    }
    finish(repo_path, st, state_file).await
}

/// Some remote branch already has this commit: moving a checkout or a branch
/// off it loses nothing.
async fn published(repo: &Path, commit: &str) -> bool {
    g(repo).args(["branch", "-r", "--contains", commit]).ok_text().await.is_some_and(|t| !t.trim().is_empty())
}

/// Every submodule the update moved goes back where it stood — a rebase in
/// progress is aborted, a finished rebase or a fast-forward undone, a branch
/// moved to re-attach a detached checkout restored — only when clean, never
/// at the cost of uncommitted work. Returns one note per submodule touched.
/// A submodule the update re-pointed is pointed back. With the superproject
/// at its pre-update commit, `.gitmodules` declares the address it had and
/// `git submodule sync` restores it; when the superproject could not go back
/// (uncommitted changes), the address stays, and the note says so.
async fn put_back_urls(repo: &Path, st: &mut SyncState) -> Vec<String> {
    let mut notes = Vec::new();
    for s in st.subs.iter_mut() {
        let Some(u) = s.url_change.clone().filter(|_| s.url_synced) else { continue };
        let sub = repo.join(&s.path);
        let remote = s.remote.clone().unwrap_or_else(|| "origin".into());
        let ok = g(repo).args(["submodule", "sync", "--", &s.path]).timeout(LOCAL_TIMEOUT).run().await.map(|o| o.ok()).unwrap_or(false);
        clear_parked(&sub, &remote).await;
        let now = g(&sub).args(["config", "--get", &format!("remote.{}.url", remote)]).ok_text().await.map(|t| t.trim().to_string()).unwrap_or_default();
        let (from, now_shown) = (crate::repo_scan::mask_token(&u.from), crate::repo_scan::mask_token(&now));
        if ok && same_url(&now, &u.from) {
            s.url_synced = false;
            s.url_note = Some(format!("fetches from {} again", from));
            notes.push(format!("{}: its address was put back to {}", s.path, from));
        } else {
            s.url_note = Some(format!("still points at {}", now_shown));
            notes.push(format!("{}: still points at {}; `git submodule sync -- {}` puts it back once .gitmodules declares {} again", s.path, now_shown, s.path, from));
        }
    }
    notes
}

async fn put_back_submodules(repo: &Path, st: &mut SyncState) -> Vec<String> {
    let skip_smudge = st.lfs_skip;
    let mut notes: Vec<String> = Vec::new();
    let handed_over = st.handed_over.clone();
    for s in st.subs.iter_mut() {
        let sub = repo.join(&s.path);
        // Downloaded by the update because upstream added it: there was nothing
        // there before, and nothing is taken away.
        if s.head_before.is_empty() {
            continue;
        }
        if rebase_in_progress(&sub).await {
            let _ = rw(&sub, skip_smudge).args(["rebase", "--abort"]).timeout(LOCAL_TIMEOUT).run().await;
        }
        // Where it really is now: aborting its rebase may already have put
        // it back (and re-applied what it had stashed — that is not "moved").
        let now = oid(&sub, "HEAD").await.unwrap_or_default();
        let on = g(&sub).args(["symbolic-ref", "-q", "--short", "HEAD"]).ok_text().await.filter(|b| !b.is_empty());
        let displaced = !same_oid(&now, &s.head_before) || on != s.branch_before;
        // Everything the update moved goes back: a rebase, and a fast-forward
        // too — left where upstream points it, the restored branch would
        // record one commit and the checkout sit at another.
        let moved = s.rebased || s.head_after.as_deref().is_some_and(|h| !same_oid(h, &s.head_before));
        // A pointer that was handed over may have been moved by the person, on
        // the hand-over's own advice ("move it to upstream's, then stage it").
        let by_hand = !moved && displaced && handed_over.contains(&s.path) && !now.is_empty();
        if !moved && !by_hand {
            continue;
        }
        if !displaced && s.reattached.is_none() {
            s.note = format!("put back at {}", short(&s.head_before));
            notes.push(format!("{}: {}", s.path, s.note));
            continue;
        }
        // Going back must strand nothing: neither the commit the checkout sits
        // on nor the tip of the branch that is moved back may be one that only
        // they hold. Safe is: where it stood, what the update itself made
        // (`head_after`), or a commit some remote branch already has.
        let safe_now = same_oid(&now, &s.head_before) || s.head_after.as_deref().is_some_and(|h| same_oid(h, &now)) || published(&sub, &now).await;
        let tip = match &s.branch_before {
            Some(b) => oid(&sub, &format!("refs/heads/{}", b)).await,
            None => None,
        };
        let safe_tip = match &tip {
            Some(t) if !same_oid(t, &now) => {
                same_oid(t, &s.head_before) || s.head_after.as_deref().is_some_and(|h| same_oid(h, t)) || is_ancestor(&sub, t, &s.head_before).await || published(&sub, t).await
            }
            _ => true,
        };
        if !safe_now || !safe_tip {
            let held = if safe_now { tip.clone().unwrap_or_default() } else { now.clone() };
            s.note = if by_hand {
                format!("left at {}, where you put it; {} holds commits nothing else has, and it stood at {} before the update", short(&now), short(&held), short(&s.head_before))
            } else {
                format!("left as moved: {} holds commits made since the update, which nothing else has; it stood at {} before the update", short(&held), short(&s.head_before))
            };
            notes.push(format!("{}: {}", s.path, s.note));
            continue;
        }
        // Where its pre-update tip can still be found, for the notes below.
        let kept = match &s.backup {
            Some(b) => format!("its pre-update tip is branch {}", b),
            None => format!("it stood at {} before the update", short(&s.head_before)),
        };
        let (dirty, _) = dirty_sets(&sub).await;
        if dirty.is_empty() && unmerged(&sub).await.is_empty() {
            let target = s.head_before.clone();
            let ok = match &s.branch_before {
                Some(b) => rw(&sub, skip_smudge).args(["switch", "--quiet", "-C", b, &target]).run().await.map(|o| o.ok()).unwrap_or(false),
                None => rw(&sub, skip_smudge).args(["checkout", "--quiet", "--detach", &target]).run().await.map(|o| o.ok()).unwrap_or(false),
            };
            // A branch the update moved to re-attach a detached checkout
            // goes back to where it stood, too.
            if ok {
                if let Some((b, at)) = s.reattached.take() {
                    if s.branch_before.as_deref() != Some(b.as_str()) {
                        let _ = g(&sub).args(["branch", "-f", &b, &at]).run().await;
                    }
                }
            }
            s.note = if ok { format!("put back at {}", short(&target)) } else { format!("could not be put back; {}", kept) };
        } else {
            s.note = format!("left as moved because it has uncommitted changes; {}", kept);
        }
        notes.push(format!("{}: {}", s.path, s.note));
    }
    notes
}

/// Undo: inner rebase first, then the superproject's (git restores the
/// autostash), then every submodule the sync rebased goes back to its
/// pre-sync tip — only when clean, never at the cost of uncommitted work.
pub async fn sync_abort(repo_path: &str) -> Result<OpResult, AppError> {
    let (status, f) = facts_for_check(repo_path).await?;
    if let Some(r) = check(&Intent::SyncAbort, &f) {
        return Ok(OpResult::refused(r, status));
    }
    let repo = PathBuf::from(repo_path);
    let state_file = state_path(&repo).await?;
    let Some(mut st) = read_state(&repo).await else {
        return Ok(OpResult::refused(Refusal { code: "no-sync".into(), message: "No update is paused here.".into() }, status));
    };
    let mut notes: Vec<String> = put_back_submodules(&repo, &mut st).await;
    if rebase_in_progress(&repo).await {
        let out = rw(&repo, st.lfs_skip).args(["rebase", "--abort"]).timeout(LOCAL_TIMEOUT).run().await?;
        if !out.ok() {
            notes.push(format!("the superproject rebase could not be aborted: {}", out.stderr.trim()));
        }
    } else if oid(&repo, "HEAD").await.as_deref() != Some(st.head_before.as_str()) {
        let (dirty, _) = dirty_sets(&repo).await;
        if dirty.is_empty() && unmerged(&repo).await.is_empty() && !autostash_left(&repo, st.stash_before.as_deref()).await {
            let _ = rw(&repo, st.lfs_skip).args(["reset", "--hard", &st.head_before]).timeout(LOCAL_TIMEOUT).run().await;
            notes.push(format!("the superproject was put back at {}", short(&st.head_before)));
        } else {
            notes.push(format!("the superproject was left as rebased because it has uncommitted changes; its pre-update tip is branch {}", st.backup.clone().unwrap_or_default()));
        }
    }
    // After the superproject, whose .gitmodules decides where they point.
    notes.extend(put_back_urls(&repo, &mut st).await);
    st.phase = "aborted".into();
    st.needs_user = None;
    let mut r = finish(repo_path, &st, &state_file).await?;
    if !notes.is_empty() {
        // The notes name what was put back; what finish said about the large files stays.
        r.detail = r.detail.replacen("Everything is back where it was, except what is named below.", &notes.join(" "), 1);
    }
    Ok(r)
}

/// Why one named submodule's pointer cannot be recorded. Each is a stable
/// code with a sentence that names the path and says what it found there.
async fn pointer_refusal(repo: &Path, subs: &[crate::submodules::SubmoduleInfo], path: &str) -> Option<Refusal> {
    if crate::git_ops::validate_repo_path(path).is_err() {
        return Some(Refusal { code: "outside-repository".into(), message: format!("{} isn't a path inside this repository; nothing to record.", path) });
    }
    let Some(s) = subs.iter().find(|s| s.path == path) else {
        return Some(Refusal { code: "not-a-submodule".into(), message: format!("{} is not a submodule of this repository; nothing to record.", path) });
    };
    if !s.listed {
        return Some(Refusal { code: "not-a-submodule".into(), message: format!("{} has no entry in .gitmodules, so git does not treat it as a submodule; nothing to record.", path) });
    }
    let Some(actual) = s.actual.as_deref().filter(|_| s.initialised) else {
        return Some(Refusal { code: "not-populated".into(), message: format!("{} is not initialised, so nothing is checked out there to record.", path) });
    };
    let (a, r) = (short(actual), short(&s.recorded));
    if same_oid(actual, &s.recorded) {
        // `recorded` is the index's; a pointer staged by hand is not the
        // branch's yet, and the sentence must not say it is.
        let committed = gitlink_at(repo, "HEAD", path).await;
        return Some(match committed {
            Some(c) if !same_oid(&c, &s.recorded) => Refusal {
                code: "not-ahead".into(),
                message: format!("{} is checked out at {}, which is already staged as its pointer; this branch's last commit still records {} — commit the staged change instead; nothing more to record.", path, a, short(&c)),
            },
            _ => Refusal { code: "not-ahead".into(), message: format!("{} is checked out at {}, the commit this branch records already; nothing to record.", path, r) },
        });
    }
    if s.recorded_missing {
        // Neither side can be compared until the submodule has the recorded commit.
        return Some(Refusal { code: "recorded-missing".into(), message: format!("{} is checked out at {}, but the {} this branch records is not in the submodule yet, so the two cannot be compared; fetch inside it first.", path, a, r) });
    }
    let sub = repo.join(path);
    if is_ancestor(&sub, &s.recorded, actual).await {
        return None;
    }
    let how = if is_ancestor(&sub, actual, &s.recorded).await { "it is behind it" } else { "the two have diverged" };
    Some(Refusal { code: "not-ahead".into(), message: format!("{} is checked out at {} which is not ahead of the {} this branch records ({}); nothing to record.", path, a, r, how) })
}

/// One commit recording submodule pointers whose checkout sits ahead of what
/// the branch records — offered, never automatic. `None` records every such
/// submodule; `Some(paths)` records exactly those, and refuses the whole
/// request when one of them is not a populated, listed submodule or is not
/// strictly ahead (equal, behind or diverged), naming the path and why.
pub async fn record_pointers(repo_path: &str, only: Option<&[String]>) -> Result<OpResult, AppError> {
    let (status, f) = facts_for_check(repo_path).await?;
    // Nothing is staged yet, so the commit pre-check is run as if the pointers
    // it is about to stage already were: every other rule (paused sync,
    // conflicts, identity, an operation in progress) applies unchanged.
    let repo = PathBuf::from(repo_path);
    let subs = crate::submodules::list_submodules(repo_path).await?;
    let mut paths: Vec<String> = Vec::new();
    let mut refusal: Option<Refusal> = None;
    match only {
        None => {
            for s in subs.iter().filter(|s| s.initialised && s.listed) {
                if let Some(a) = &s.actual {
                    if !same_oid(a, &s.recorded) && is_ancestor(&repo.join(&s.path), &s.recorded, a).await {
                        paths.push(s.path.clone());
                    }
                }
            }
        }
        Some(wanted) => {
            // The first refusal stands for the request: nothing is recorded
            // until every named path can be.
            for p in wanted {
                let p = p.trim_end_matches(['/', '\\']);
                if paths.iter().any(|x| x == p) {
                    continue;
                }
                match pointer_refusal(&repo, &subs, p).await {
                    Some(r) => {
                        refusal = Some(r);
                        break;
                    }
                    None => paths.push(p.to_string()),
                }
            }
        }
    }
    let mut as_if_staged = f.clone();
    as_if_staged.staged += paths.len().max(1); // permission first, content second
    if let Some(r) = check(&Intent::Commit { amend: false, message_empty: false }, &as_if_staged) {
        return Ok(OpResult::refused(r, status));
    }
    if let Some(r) = refusal {
        return Ok(OpResult::refused(r, status));
    }
    if paths.is_empty() {
        let message = if only.is_some() { "No submodule was named; nothing to record." } else { "Every submodule is at the commit this branch records." };
        return Ok(OpResult::refused(Refusal { code: "nothing-to-record".into(), message: message.into() }, status));
    }
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    g(&repo).top_flag("--literal-pathspecs").args(args).timeout(LOCAL_TIMEOUT).text().await?;
    let msg = format!("Record submodule pointers: {}", paths.join(", "));
    g(&repo).args(["commit", "--quiet", "--only", "-m", &msg, "--"]).args(paths.iter().map(String::as_str)).timeout(LOCAL_TIMEOUT).text().await?;
    let after = Box::pin(git_status::repo_status(repo_path)).await?;
    Ok(OpResult::done(format!("Recorded {} submodule pointer(s).", paths.len()), msg, after))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sup() -> SuperFacts {
        SuperFacts {
            branch: "main".into(),
            upstream: "origin/main".into(),
            upstream_ref: "refs/remotes/origin/main".into(),
            remote: "origin".into(),
            head: "h".repeat(40),
            upstream_oid: "u".repeat(40),
            base_oid: "b".repeat(40),
            ahead: 2,
            behind: 3,
            own_commits: vec![CommitRef { oid: "1".repeat(40), short: "1111111".into(), subject: "seal work".into(), touches: vec!["A".into()], ..Default::default() }],
            incoming: vec![],
            ..Default::default()
        }
    }
    #[test]
    fn gitmodules_urls_resolve_the_way_git_resolves_them() {
        let r = |base: Option<&str>, url: &str| resolve_submodule_url(base, url);
        assert_eq!(r(Some("git@github.com:org/repo.git"), "https://x.test/y.git"), "https://x.test/y.git");
        assert_eq!(r(Some("git@github.com:org/repo.git"), "../lib.git"), "git@github.com:org/lib.git");
        assert_eq!(r(Some("git@github.com:repo.git"), "../lib.git"), "git@github.com:lib.git");
        assert_eq!(r(Some("https://github.com/org/repo.git/"), "../lib.git"), "https://github.com/org/lib.git");
        assert_eq!(r(Some("https://github.com/org/repo.git"), "../../other/lib.git"), "https://github.com/other/lib.git");
        assert_eq!(r(Some("https://github.com/org/repo.git"), "./lib"), "https://github.com/org/repo.git/lib");
        assert_eq!(r(Some("/srv/git/repo.git"), "../lib.git"), "/srv/git/lib.git");
        assert_eq!(r(None, "../lib.git"), "../lib.git");
        assert!(same_url("https://x.test/y.git/", "https://x.test/y.git"));
        assert!(!same_url("https://x.test/y.git", "https://x.test/y"));
    }

    fn sub(path: &str) -> SubFacts {
        SubFacts {
            path: path.into(),
            mapped: true,
            populated: true,
            gitdir_valid: true,
            head: Some("r".repeat(40)),
            recorded: "r".repeat(40),
            target: Some("t".repeat(40)),
            base_link: Some("o".repeat(40)),
            target_present: true,
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            remote: Some("origin".into()),
            remote_head_branch: Some("main".into()),
            my_commits_move_gitlink: true,
            upstream_moves_gitlink: true,
            own_commits_moving_gitlink: 1,
            ..Default::default()
        }
    }
    fn facts(subs: Vec<SubFacts>) -> SyncFacts {
        SyncFacts { superproject: sup(), submodules: subs, skipped: vec![] }
    }

    #[test]
    fn a_diverged_submodule_is_rebased_onto_the_recorded_target_at_the_stop() {
        let p = plan(&facts(vec![sub("A")]), true);
        assert!(p.can_run, "{:?}", p.blockers);
        let a = &p.submodules[0];
        assert_eq!(a.action, "rebase");
        assert_eq!(a.rebase_onto.as_deref(), Some("t".repeat(40).as_str()));
        assert!(a.predicted_gitlink_conflict);
        assert!(p.summary.contains("rebasing the submodules"));
        assert_eq!(p.fingerprint[0].1, "u".repeat(40));
        assert_eq!(p.fingerprint[1], ("A".to_string(), "t".repeat(40)));
    }

    #[test]
    fn every_ambiguous_state_is_a_named_blocker() {
        // Checked out at an unrecorded commit while the pointer moves.
        let mut a = sub("A");
        a.head = Some("x".repeat(40));
        let p = plan(&facts(vec![a]), true);
        assert!(!p.can_run);
        assert!(p.blockers[0].contains("checked out at xxxxxxx but this branch records"));
        // Two own commits moving the pointer.
        let mut a = sub("A");
        a.own_commits_moving_gitlink = 2;
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("2 of your commits move"));
        // Upstream removed it.
        let mut a = sub("A");
        a.target = None;
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("upstream removed"));
        // Target unreachable after a failed fetch.
        let mut a = sub("A");
        a.target_present = false;
        a.fetch_error = Some("could not read".into());
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("fetching failed"));
        // An operation inside it.
        let mut a = sub("A");
        a.operation = Some("rebase".into());
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("in progress inside"));
        // Dirty files inside it that the rebase would touch.
        let mut a = sub("A");
        a.dirty_overlap = vec!["A.txt".into()];
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("collide"));
        // A dead gitfile.
        let mut a = sub("A");
        a.gitdir_valid = false;
        assert!(plan(&facts(vec![a]), true).blockers[0].contains("no longer exists"));
    }

    #[test]
    fn a_fast_forward_over_an_edited_file_is_a_blocker_not_a_late_failure() {
        let mut b = sub("B");
        b.my_commits_move_gitlink = false;
        b.own_commits_moving_gitlink = 0;
        b.head_is_ancestor_of_target = true;
        b.dirty_overlap = vec!["B.txt".into()];
        let p = plan(&facts(vec![b]), true);
        assert!(!p.can_run);
        assert_eq!(p.submodules[0].remedy.as_deref(), Some("stash-inside"));
        assert!(p.blockers[0].contains("collide with the commits it will be moved onto"));
    }

    #[test]
    fn a_submodule_only_my_commits_added_is_not_called_removed() {
        // Neither upstream nor the commit we both started from has it.
        let mut a = sub("lib");
        a.target = None;
        a.base_link = None;
        a.my_commits_move_gitlink = false;
        a.upstream_moves_gitlink = false;
        let p = plan(&facts(vec![a.clone()]), true);
        assert!(p.can_run, "{:?}", p.blockers);
        assert_eq!(p.submodules[0].action, "none");
        assert!(p.submodules[0].reason.contains("added by your commits"));
        // Added twice over (add, then bump): still nothing upstream could conflict with.
        a.own_commits_moving_gitlink = 3;
        assert!(plan(&facts(vec![a.clone()]), true).can_run);
        // Staged but not committed: said plainly, with its own code.
        a.own_commits_moving_gitlink = 0;
        let p = plan(&facts(vec![a.clone()]), true);
        assert_eq!(p.submodules[0].remedy.as_deref(), Some("commit-first"));
        assert!(p.blockers[0].contains("not committed yet"));
        // The commit we started from had it and upstream does not: a real removal.
        a.base_link = Some("o".repeat(40));
        a.own_commits_moving_gitlink = 1;
        assert_eq!(plan(&facts(vec![a]), true).submodules[0].remedy.as_deref(), Some("removed-upstream"));
    }

    #[test]
    fn a_pointer_the_update_cannot_place_is_a_predicted_pause_not_a_silent_move() {
        // My commit moves the pointer back; upstream moved it forward.
        let mut a = sub("A");
        a.pointer_rewound = true;
        a.head_is_ancestor_of_target = true;
        let p = plan(&facts(vec![a.clone()]), true);
        assert!(p.can_run, "{:?}", p.blockers);
        assert_eq!(p.submodules[0].action, "none", "the submodule is not moved forward over what my commit says");
        assert!(p.submodules[0].predicted_gitlink_conflict);
        assert!(p.submodules[0].reason.contains("moves its pointer back to rrrrrrr while upstream moved it forward to ttttttt"), "{}", p.submodules[0].reason);
        // Moved forward by me and further by upstream: git settles that itself.
        a.pointer_rewound = false;
        assert_eq!(plan(&facts(vec![a]), true).submodules[0].action, "fast-forward");
        // Added on both sides at different commits.
        let mut x = sub("X");
        x.base_link = None;
        x.my_commits_move_gitlink = false;
        x.head_is_ancestor_of_target = true;
        let p = plan(&facts(vec![x.clone()]), true);
        assert_eq!(p.submodules[0].action, "none");
        assert!(p.submodules[0].predicted_gitlink_conflict);
        assert!(p.submodules[0].reason.contains("added by your commits at rrrrrrr and by upstream at ttttttt"));
        // …at the same commit: nothing to choose.
        x.target = Some("r".repeat(40));
        let p = plan(&facts(vec![x]), true);
        assert!(!p.submodules[0].predicted_gitlink_conflict);
        assert!(p.submodules[0].reason.contains("already at the commit upstream records"));
        // Only I added it, and its checkout is not where my commit records it.
        let mut l = sub("lib");
        l.target = None;
        l.base_link = None;
        l.my_commits_move_gitlink = false;
        l.upstream_moves_gitlink = false;
        l.head = Some("x".repeat(40));
        let p = plan(&facts(vec![l]), true);
        assert_eq!(p.submodules[0].remedy.as_deref(), Some("record-pointers"));
        // A pointer that is only staged is not something my commits did…
        let mut s = sub("S");
        s.upstream_moves_gitlink = false;
        s.own_commits_moving_gitlink = 0;
        s.head_is_ancestor_of_target = true;
        assert_eq!(plan(&facts(vec![s.clone()]), true).submodules[0].action, "fast-forward");
        // …and while it is staged the update does not walk over it.
        s.index_pointer = "i".repeat(40);
        let p = plan(&facts(vec![s]), true);
        assert_eq!(p.submodules[0].remedy.as_deref(), Some("commit-first"));
        assert!(p.blockers[0].contains("its pointer is staged at iiiiiii but this branch records rrrrrrr"), "{:?}", p.blockers);
    }

    #[test]
    fn untracked_files_inside_a_submodule_block_the_move_before_it_starts() {
        // A fast-forward over a file git does not track yet.
        let mut b = sub("B");
        b.my_commits_move_gitlink = false;
        b.own_commits_moving_gitlink = 0;
        b.head_is_ancestor_of_target = true;
        b.untracked_overlap = vec!["notes.txt".into()];
        let p = plan(&facts(vec![b.clone()]), true);
        assert!(!p.can_run);
        assert_eq!(p.submodules[0].remedy.as_deref(), Some("untracked-inside"));
        assert!(p.blockers[0].contains("B: untracked file inside it would be overwritten"), "{:?}", p.blockers);
        assert!(p.blockers[0].contains("notes.txt"));
        // The same file, edited and tracked, is the other blocker and comes first.
        b.dirty_overlap = vec!["notes.txt".into()];
        assert_eq!(plan(&facts(vec![b]), true).submodules[0].remedy.as_deref(), Some("stash-inside"));
        // A rebase does not carry untracked files out of the way either.
        let mut a = sub("A");
        a.untracked_overlap = vec!["x.txt".into(), "y.txt".into()];
        let p = plan(&facts(vec![a]), true);
        assert_eq!(p.submodules[0].remedy.as_deref(), Some("untracked-inside"));
        assert!(p.blockers[0].contains("untracked files inside it"));
    }

    #[test]
    fn several_own_pointer_commits_block_only_when_upstream_moved_the_pointer_too() {
        // Upstream moved it as well, and my pointers do not all sit along the
        // checked-out line: nothing to record them against — a blocker.
        let mut a = sub("A");
        a.own_commits_moving_gitlink = 2;
        assert_eq!(plan(&facts(vec![a.clone()]), true).submodules[0].remedy.as_deref(), Some("squash-own"));
        // Along the line: the submodule is rebased once and each pointer
        // commit records its rewritten copy — said in the plan.
        a.own_pointers_on_line = true;
        let p = plan(&facts(vec![a.clone()]), true);
        assert!(p.can_run, "{:?}", p.blockers);
        assert_eq!(p.submodules[0].action, "rebase");
        assert!(p.submodules[0].predicted_gitlink_conflict);
        assert!(p.submodules[0].notes.iter().any(|n| n.contains("2 commits of yours move its pointer")), "{:?}", p.submodules[0].notes);
        a.own_pointers_on_line = false;
        // Upstream left it alone: every pointer commit replays onto the pointer
        // it started from, and the checkout is already where the last one put it.
        a.upstream_moves_gitlink = false;
        a.target_is_ancestor_of_head = true;
        let p = plan(&facts(vec![a.clone()]), true);
        assert!(p.can_run, "{:?}", p.blockers);
        assert_eq!(p.submodules[0].action, "none");
        assert!(p.submodules[0].reason.contains("your commit's pointer will be kept"));
        // …also when my commits moved it sideways or back: nothing to align.
        a.target_is_ancestor_of_head = false;
        a.head_is_ancestor_of_target = true;
        let p = plan(&facts(vec![a.clone()]), true);
        assert_eq!(p.submodules[0].action, "none");
        assert!(p.submodules[0].reason.contains("upstream did not move it"), "{}", p.submodules[0].reason);
        a.head_is_ancestor_of_target = false;
        assert_eq!(plan(&facts(vec![a]), true).submodules[0].action, "none");
    }

    #[test]
    fn a_pointer_kept_only_while_a_commit_of_mine_still_moves_it() {
        // My one pointer commit is already upstream (a twin), and upstream then
        // moved the pointer back: nothing of mine is replayed, so nothing keeps
        // the checkout's pointer — the branch will record upstream's, and the
        // checkout sits ahead of it.
        let mut a = sub("A");
        a.target_is_ancestor_of_head = true;
        a.own_commits_moving_gitlink = 0;
        let p = plan(&facts(vec![a.clone()]), true);
        assert_eq!(p.submodules[0].action, "ahead", "{}", p.submodules[0].reason);
        assert!(p.submodules[0].reason.contains("the superproject does not record them yet"));
        // With a commit of mine still to replay, its pointer is kept.
        a.own_commits_moving_gitlink = 1;
        let p = plan(&facts(vec![a]), true);
        assert_eq!(p.submodules[0].action, "none");
        assert!(p.submodules[0].reason.contains("your commit's pointer will be kept"));
    }

    #[test]
    fn the_rebase_reason_counts_the_submodules_own_commits() {
        let mut a = sub("A");
        a.own_total = 5;
        let p = plan(&facts(vec![a.clone()]), true);
        assert!(p.submodules[0].reason.starts_with("your 5 commits are rebased onto"), "{}", p.submodules[0].reason);
        a.own_total = 1;
        assert!(plan(&facts(vec![a]), true).submodules[0].reason.starts_with("your 1 commit is rebased onto"));
    }

    #[test]
    fn level_with_upstream_the_superproject_is_left_alone() {
        // Only a submodule needs moving: no stash to turn on, no identity to fix.
        let mut s = sup();
        s.behind = 0;
        s.dirty_tracked = vec!["README.md".into()];
        s.identity_mismatch = true;
        s.own_merges = 1;
        let mut b = sub("B");
        b.my_commits_move_gitlink = false;
        b.upstream_moves_gitlink = false;
        b.own_commits_moving_gitlink = 0;
        b.head_is_ancestor_of_target = true;
        let p = plan(&SyncFacts { superproject: s.clone(), submodules: vec![b], skipped: vec![] }, false);
        assert!(p.can_run, "{:?}", p.blockers);
        assert!(p.superproject.remedies.is_empty());
        assert!(!p.superproject.will_stash);
        assert_eq!(p.summary, "Only submodules need aligning.");
        assert!(p.superproject.notes.iter().all(|n| !n.contains("merge")), "no rebase, so no merge is flattened");
        // Behind as well: the same tree needs the stash, the identity and the note.
        s.behind = 3;
        let p = plan(&SyncFacts { superproject: s, submodules: vec![], skipped: vec![] }, false);
        assert_eq!(p.superproject.remedies, vec!["turn-on-stash".to_string(), "identity".to_string()]);
        assert!(p.superproject.notes.iter().any(|n| n.contains("1 of your commits is a merge")), "{:?}", p.superproject.notes);
        assert_eq!(p.summary, "Cannot update yet: 2 things need attention first.");
    }

    fn rp(oid: &str, when: u32, subject: &str) -> Replayed {
        Replayed { oid: oid.into(), key: format!("Ann\x1fann@x.test\x1f{}\x1f{}", when, subject), subject: subject.into() }
    }

    #[test]
    fn replayed_commits_are_read_with_their_author_time_and_subject() {
        let got = parse_replayed("aaa\x1fAnn\x1fann@x.test\x1f100\x1fwip\0bbb\x1fAnn\x1fann@x.test\x1f200\x1f\0ccc\x1fAnn\x1fann@x.test\x1f300\x1fa\x1fb\0");
        assert_eq!(got.len(), 3);
        assert_eq!((got[0].oid.as_str(), got[0].subject.as_str()), ("aaa", "wip"));
        // An empty subject is a subject; one holding the separator is kept whole.
        assert_eq!((got[1].subject.as_str(), got[1].key.as_str()), ("", "Ann\x1fann@x.test\x1f200\x1f"));
        assert_eq!(got[2].subject, "a\x1fb");
    }

    #[test]
    fn a_commit_that_is_gone_is_not_excused_by_a_namesake() {
        let never = |_: &str| false;
        let none = BTreeMap::new();
        // Two commits called "wip", written at different moments; one copy on top.
        let originals = vec![rp("a", 100, "wip"), rp("b", 200, "wip")];
        let on_top = vec![rp("a2", 100, "wip")];
        let missing: Vec<String> = unmatched(&originals, &mut Copies::new(&on_top), &never, &none).iter().map(|m| m.oid.clone()).collect();
        assert_eq!(missing, vec!["b"], "the time tells them apart: the copy is a's");
        // Everything on top: nothing is missing, whatever the subjects.
        let on_top = vec![rp("b2", 200, "wip"), rp("a2", 100, "wip")];
        assert!(unmatched(&originals, &mut Copies::new(&on_top), &never, &none).is_empty());
        // An empty subject is matched like any other.
        let originals = vec![rp("e", 100, "")];
        assert!(unmatched(&originals, &mut Copies::new(&[rp("e2", 100, "")]), &never, &none).is_empty());
        assert_eq!(unmatched(&originals, &mut Copies::new(&[]), &never, &none).len(), 1);
    }

    #[test]
    fn with_one_copy_for_two_identical_keys_the_surer_one_takes_it() {
        let never = |_: &str| false;
        // Same author, same second, same subject: only what they did can tell.
        let originals = vec![rp("a", 100, "wip"), rp("b", 100, "wip")];
        let on_top = vec![rp("x", 100, "wip")];
        let present: BTreeMap<String, bool> = [("a".to_string(), false), ("b".to_string(), true)].into_iter().collect();
        let missing: Vec<String> = unmatched(&originals, &mut Copies::new(&on_top), &never, &present).iter().map(|m| m.oid.clone()).collect();
        assert_eq!(missing, vec!["a"], "the one whose changes are not in HEAD is the one that is gone");
        // A commit the app itself committed after a resolution is surer still.
        let resolved = |oid: &str| oid == "a";
        let missing: Vec<String> = unmatched(&originals, &mut Copies::new(&on_top), &resolved, &present).iter().map(|m| m.oid.clone()).collect();
        assert_eq!(missing, vec!["b"]);
    }

    #[test]
    fn a_copy_whose_author_date_was_reset_is_still_found_by_its_subject() {
        let never = |_: &str| false;
        let present: BTreeMap<String, bool> = [("a".to_string(), false)].into_iter().collect();
        let originals = vec![rp("a", 100, "fix the parser"), rp("b", 200, "add tests")];
        let on_top = vec![rp("b2", 200, "add tests"), rp("a2", 999, "fix the parser")];
        assert!(unmatched(&originals, &mut Copies::new(&on_top), &never, &present).is_empty());
        // …but a copy somebody else already owns does not vouch twice.
        let originals = vec![rp("a", 100, "wip"), rp("b", 200, "wip")];
        let on_top = vec![rp("b2", 200, "wip")];
        let missing: Vec<String> = unmatched(&originals, &mut Copies::new(&on_top), &never, &present).iter().map(|m| m.oid.clone()).collect();
        assert_eq!(missing, vec!["a"]);
    }

    #[test]
    fn what_git_printed_is_kept_as_drops_and_as_its_last_words() {
        let mut st = SyncState {
            version: STATE_VERSION,
            nonce: String::new(),
            started_at: String::new(),
            phase: "rebasing".into(),
            repo: String::new(),
            upstream_ref: String::new(),
            upstream_oid: String::new(),
            head_before: String::new(),
            stash: false,
            backup: None,
            captures: BTreeMap::new(),
            subs: vec![],
            needs_user: None,
            plan: SyncPlan::default(),
            bundles: vec![],
            resolved: vec![],
            dropped_by_git: vec![],
            emptied: vec![],
            stash_before: None,
            git_said: String::new(),
            handed_over: vec![],
            pointer_upstreams: vec![],
            lfs: BTreeMap::new(),
            lfs_skip: false,
        };
        let a = "a".repeat(40);
        let stderr = format!(
            "Rebasing (1/3)\rdropping {} mine: line 1 -- patch contents already upstream\nRebasing (2/3)\r\u{1b}[Kerror: commit-msg hook declined\nhint: fix it and run git rebase --continue\nCould not apply 1234567... mine 2\n",
            a
        );
        note_drops(&mut st, &stderr);
        assert_eq!(st.dropped_by_git, vec![a.clone()]);
        assert_eq!(st.git_said, "error: commit-msg hook declined / Could not apply 1234567... mine 2");
        // The same line twice is one drop; a later call replaces the last words.
        note_drops(&mut st, &format!("dropping {} mine: line 1 -- patch contents already upstream\nSuccessfully rebased and updated refs/heads/main.\n", a));
        assert_eq!(st.dropped_by_git.len(), 1);
        assert_eq!(st.git_said, "Successfully rebased and updated refs/heads/main.");
    }

    #[test]
    fn identity_blocks_only_what_would_be_recommitted() {
        // The superproject re-commits your work: a mismatch blocks.
        let mut s = sup();
        s.identity_mismatch = true;
        s.identity_email = "wrong@example.test".into();
        s.profile_email = Some("right@example.test".into());
        let p = plan(&SyncFacts { superproject: s.clone(), submodules: vec![], skipped: vec![] }, true);
        assert!(!p.can_run);
        assert_eq!(p.superproject.remedies, vec!["identity".to_string()]);
        assert!(p.blockers[0].contains("should commit as right@example.test but git is set to wrong@example.test"));
        // Nothing of yours to replay and only a fast-forward: nothing is re-committed.
        s.ahead = 0;
        s.own_commits = vec![];
        let mut b = sub("B");
        b.my_commits_move_gitlink = false;
        b.own_commits_moving_gitlink = 0;
        b.head_is_ancestor_of_target = true;
        let p = plan(&SyncFacts { superproject: s.clone(), submodules: vec![b], skipped: vec![] }, true);
        assert!(p.can_run, "{:?}", p.blockers);
        // A submodule rebase re-commits inside it: its own identity matters.
        let mut a = sub("A");
        a.identity_mismatch = true;
        a.identity_email = "wrong@example.test".into();
        a.profile_email = Some("right@example.test".into());
        let p = plan(&facts(vec![a]), true);
        assert_eq!(p.submodules[0].remedy.as_deref(), Some("identity"));
        assert!(p.blockers[0].contains("re-write its commits as wrong@example.test"));
    }

    #[test]
    fn the_plan_carries_what_the_picture_needs() {
        let mut s = sup();
        s.base_oid = "b".repeat(40);
        s.dirty_tracked = vec!["README.md".into()];
        s.behind = 40;
        s.ahead = 2;
        let mut a = sub("A");
        a.has_nested = true;
        a.own_commits = vec![CommitRef { oid: "1".repeat(40), short: "1111111".into(), subject: "A: my commit".into(), ..Default::default() }];
        let p = plan(&SyncFacts { superproject: s, submodules: vec![a], skipped: vec![] }, true);
        assert_eq!(p.superproject.base_short, "bbbbbbb");
        assert_eq!(p.superproject.dirty_paths, vec!["README.md".to_string()]);
        assert_eq!(p.superproject.more_incoming, 40);
        assert!(p.submodules[0].has_nested);
        assert_eq!(p.submodules[0].own_commits[0].subject, "A: my commit");
    }

    #[test]
    fn remedies_are_stable_codes_beside_the_sentences() {
        let remedy = |a: SubFacts| plan(&facts(vec![a]), true).submodules[0].remedy.clone();
        let mut a = sub("A");
        a.head = Some("x".repeat(40));
        assert_eq!(remedy(a).as_deref(), Some("record-pointers"));
        let mut a = sub("A");
        a.own_commits_moving_gitlink = 2;
        assert_eq!(remedy(a).as_deref(), Some("squash-own"));
        let mut a = sub("A");
        a.target = None;
        assert_eq!(remedy(a).as_deref(), Some("removed-upstream"));
        let mut a = sub("A");
        a.target_present = false;
        a.fetch_error = Some("could not read".into());
        assert_eq!(remedy(a).as_deref(), Some("fetch-failed"));
        let mut a = sub("A");
        a.target_present = false;
        assert_eq!(remedy(a).as_deref(), Some("unreachable"));
        let mut a = sub("A");
        a.operation = Some("rebase".into());
        assert_eq!(remedy(a).as_deref(), Some("finish-operation"));
        let mut a = sub("A");
        a.conflicted = 1;
        assert_eq!(remedy(a).as_deref(), Some("resolve-conflicts"));
        let mut a = sub("A");
        a.dirty_overlap = vec!["A.txt".into()];
        assert_eq!(remedy(a).as_deref(), Some("stash-inside"));
        let mut a = sub("A");
        a.gitdir_valid = false;
        assert_eq!(remedy(a).as_deref(), Some("reinit"));
        // Nothing blocked: no remedy.
        assert_eq!(remedy(sub("A")), None);
        // The superproject's own blockers carry their codes in order.
        let mut s = sup();
        s.dirty_tracked = vec!["README.md".into()];
        let p = plan(&SyncFacts { superproject: s.clone(), submodules: vec![], skipped: vec![] }, false);
        assert_eq!(p.superproject.remedies, vec!["turn-on-stash".to_string()]);
        s.dirty_overlap = vec!["README.md".into()];
        s.untracked_overlap = vec!["new.txt".into()];
        let p = plan(&SyncFacts { superproject: s, submodules: vec![], skipped: vec![] }, true);
        assert_eq!(p.superproject.remedies, vec!["stash-collides".to_string(), "untracked-in-the-way".to_string()]);
        assert_eq!(p.blockers.len(), p.superproject.remedies.len());
    }

    #[test]
    fn the_outcome_reports_what_align_did_not_what_was_planned() {
        let st = |note: &str, rebased: bool| SubState {
            path: "A".into(),
            branch_before: None,
            reattach_to: None,
            head_before: String::new(),
            backup: None,
            target: String::new(),
            rebased,
            head_after: None,
            aligned: true,
            note: note.into(),
            stash_before: None,
            reattached: None,
            url_change: None,
            url_synced: false,
            url_note: None,
            remote: None,
        };
        assert_eq!(outcome_action(&st("fast-forwarded to abc1234", false), Some("fast-forward"), "done"), "fast-forwarded");
        assert_eq!(outcome_action(&st("rebased (conflicts resolved by you)", true), Some("rebase"), "done"), "rebased");
        assert_eq!(outcome_action(&st("could not fast-forward: x", false), Some("fast-forward"), "done"), "could-not-fast-forward");
        assert_eq!(outcome_action(&st("your commits sit on top of abc1234; the superproject does not record them yet", false), Some("ahead"), "done"), "left-ahead");
        assert_eq!(outcome_action(&st("already at the recorded commit", false), Some("none"), "done"), "unchanged");
        assert_eq!(outcome_action(&st("", false), Some("blocked"), "done"), "blocked");
        // A run that never reached it must not claim the plan's verb.
        assert_eq!(outcome_action(&st("", false), Some("rebase"), "rebasing"), "pending");
        assert_eq!(outcome_action(&st("", false), Some("fast-forward"), "aborted"), "unchanged");
        assert_eq!(outcome_action(&st("", false), Some("none"), "rebasing"), "unchanged");
        // What an abort undid, or could not.
        assert_eq!(outcome_action(&st("put back at abc1234", true), Some("rebase"), "aborted"), "put-back");
        assert_eq!(outcome_action(&st("put back at abc1234", false), Some("fast-forward"), "aborted"), "put-back");
        assert_eq!(outcome_action(&st("left as moved because it has uncommitted changes; it stood at abc1234 before the update", false), Some("fast-forward"), "aborted"), "left-as-moved");
    }

    #[test]
    fn stashing_is_the_users_choice_and_overlaps_block_it() {
        let mut s = sup();
        s.dirty_tracked = vec!["README.md".into()];
        let f = SyncFacts { superproject: s.clone(), submodules: vec![], skipped: vec![] };
        let p = plan(&f, false);
        assert!(p.blockers[0].contains("Turn on stashing"));
        let p = plan(&f, true);
        assert!(p.can_run);
        assert!(p.superproject.will_stash);
        s.dirty_overlap = vec!["README.md".into()];
        let p = plan(&SyncFacts { superproject: s.clone(), submodules: vec![], skipped: vec![] }, true);
        assert!(p.blockers[0].contains("collide with incoming"));
        s.dirty_overlap.clear();
        s.untracked_overlap = vec!["new.txt".into()];
        let p = plan(&SyncFacts { superproject: s, submodules: vec![], skipped: vec![] }, true);
        assert!(p.blockers[0].contains("Untracked file would be overwritten"));
    }

    #[test]
    fn fast_forward_ahead_and_none_are_told_apart() {
        let mut b = sub("B");
        b.my_commits_move_gitlink = false;
        b.own_commits_moving_gitlink = 0;
        b.head_is_ancestor_of_target = true;
        let p = plan(&facts(vec![b.clone()]), true);
        assert_eq!(p.submodules[0].action, "fast-forward");
        let mut c = b.clone();
        c.head_is_ancestor_of_target = false;
        c.target_is_ancestor_of_head = true;
        let p = plan(&facts(vec![c]), true);
        assert_eq!(p.submodules[0].action, "ahead");
        assert!(p.submodules[0].reason.contains("does not record them yet"));
        let mut d = b.clone();
        d.head = d.target.clone();
        d.recorded = d.target.clone().unwrap();
        d.head_is_ancestor_of_target = false;
        let p = plan(&facts(vec![d]), true);
        assert_eq!(p.submodules[0].action, "none");
    }

    #[test]
    fn nothing_to_do_is_said_plainly_and_the_remote_tip_note_is_informational() {
        let mut s = sup();
        s.behind = 0;
        s.ahead = 0;
        let mut a = sub("A");
        a.head = a.target.clone();
        a.recorded = a.target.clone().unwrap();
        a.my_commits_move_gitlink = false;
        a.upstream_moves_gitlink = false;
        a.own_commits_moving_gitlink = 0;
        a.tip_ahead_of_target = true;
        let p = plan(&SyncFacts { superproject: s, submodules: vec![a], skipped: vec![Skipped { path: "forge/x".into(), why: "unmapped".into() }] }, true);
        assert!(p.nothing_to_do);
        assert!(!p.can_run);
        assert!(p.summary.starts_with("Nothing to update"));
        assert!(p.submodules[0].notes.iter().any(|n| n.contains("newer commits than upstream's superproject records")));
        assert_eq!(p.skipped[0].why, "unmapped");
    }

    #[test]
    fn state_round_trips() {
        let st = SyncState {
            version: STATE_VERSION,
            nonce: "n".into(),
            started_at: "t".into(),
            phase: "rebasing".into(),
            repo: "/r".into(),
            upstream_ref: "refs/remotes/origin/main".into(),
            upstream_oid: "u".into(),
            head_before: "h".into(),
            stash: true,
            backup: Some("gitswitch-before-sync-1".into()),
            captures: BTreeMap::new(),
            subs: vec![],
            needs_user: Some(NeedsUser { where_: "submodule:A".into(), paths: vec!["A".into()], hint: "x".into(), at_commit: None }),
            plan: SyncPlan::default(),
            resolved: vec![],
            dropped_by_git: vec!["d".repeat(40)],
            emptied: vec![],
            stash_before: Some(String::new()),
            git_said: "error: hook declined".into(),
            handed_over: vec!["A".into()],
            pointer_upstreams: vec![],
            bundles: vec![],
            lfs: BTreeMap::from([(String::new(), true), ("A".to_string(), false)]),
            lfs_skip: true,
        };
        let json = serde_json::to_string(&st).unwrap();
        let back: SyncState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, st);
        assert_eq!(shell_quote("/Users/me/with space"), "'/Users/me/with space'");
        assert_eq!(shell_quote("/Users/me/repo"), "/Users/me/repo");
    }
}
