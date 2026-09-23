//! Reading a repository's working state: what changed, on which branch, with
//! which identity, and whether pushing is allowed.
//!
//! Everything here is read-only. `git status --porcelain=v2 -z` is the source:
//! v2 because it is the only porcelain format that reports *why* a submodule is
//! dirty, and `-z` because without it a filename containing a quote, a newline
//! or a non-ASCII byte gets C-quoted and the parse silently desynchronises.

use crate::error::AppError;
use crate::git_exec::{GitCmd, READ_TIMEOUT};
use crate::profiles;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A worktree with more untracked files than this is almost always missing a
/// .gitignore. Collect this many, then say so rather than freezing the UI.
const MAX_UNTRACKED: usize = 2000;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ChangeEntry {
    pub path: String,
    /// Where a rename or copy came from. Only ever set for a `2 ` record.
    pub orig_path: Option<String>,
    /// Index vs HEAD: "M" "T" "A" "D" "R" "C" or "." for unchanged.
    pub staged: String,
    /// Worktree vs index: "M" "T" "D" or ".".
    pub unstaged: String,
    /// "tracked" | "untracked" | "conflicted"
    pub kind: String,
    /// Plain words for an unmerged state, e.g. "both modified".
    pub conflict: Option<String>,
    pub rename_score: Option<u8>,
    pub is_submodule: bool,
    /// The submodule points at a different commit than the superproject records.
    pub sub_commit_changed: bool,
    pub sub_tracked_changes: bool,
    pub sub_untracked: bool,
    /// For a gitlink, the commit the superproject records (v2 field `hH`).
    /// Without it nothing downstream can say where a submodule moved *from*.
    pub recorded_oid: Option<String>,
    /// Conflicted entries only: the modes and oids of index stages 1, 2, 3
    /// (base, ours, theirs). A mode of "160000" marks a submodule pointer.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stage_modes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stage_oids: Vec<String>,
    pub staged_added: Option<u64>,
    pub staged_removed: Option<u64>,
    pub unstaged_added: Option<u64>,
    pub unstaged_removed: Option<u64>,
    pub is_binary: bool,
}

impl ChangeEntry {
    pub fn is_staged(&self) -> bool {
        self.staged != "."
    }

    /// True when the worktree differs from the index — an untracked file counts,
    /// since it is entirely absent from the index.
    pub fn is_unstaged(&self) -> bool {
        self.unstaged != "." || self.kind == "untracked"
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct ParsedStatus {
    pub branch: Option<String>,
    pub detached: bool,
    pub unborn: bool,
    pub head_oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub stash_count: usize,
    pub entries: Vec<ChangeEntry>,
    pub untracked_truncated: bool,
}

/// An interrupted merge/rebase/cherry-pick. Detected and shown; never acted on
/// without an explicit request.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct InProgress {
    /// "merge" | "rebase" | "rebase-interactive" | "am" | "cherry-pick" | "revert" | "bisect"
    pub kind: String,
    pub label: String,
    pub detail: String,
    pub abort_command: String,
    /// What finishes it once conflicts are staged; `None` when only a commit
    /// (merge) or nothing (bisect) can.
    pub continue_command: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct RepoIdentity {
    /// What git will actually use — `git var GIT_AUTHOR_IDENT`, not what the
    /// profile intends. Showing the intent would hide the exact bug this app
    /// exists to catch.
    pub name: String,
    pub email: String,
    /// "local" | "global" | "system" | "worktree" | "command" | "unset"
    pub email_scope: String,
    pub email_origin: Option<String>,
    pub profile_id: Option<String>,
    pub profile_name: Option<String>,
    pub profile_email: Option<String>,
    pub matches_profile: bool,
    pub signing_on: bool,
    pub signing_key: Option<String>,
    /// "gitswitch" | "foreign" | "none"
    pub guard: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct RepoStatus {
    pub path: String,
    pub name: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub unborn: bool,
    pub head_oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub last_fetch_secs: Option<u64>,
    pub entries: Vec<ChangeEntry>,
    pub staged_count: usize,
    pub unstaged_count: usize,
    pub untracked_count: usize,
    pub conflicted_count: usize,
    pub submodule_dirty_count: usize,
    pub untracked_truncated: bool,
    pub stash_count: usize,
    pub operation: Option<InProgress>,
    pub identity: RepoIdentity,
    pub push: crate::push_guard::PushState,
    /// The last commit can be amended: it exists and is not on any remote.
    pub can_amend: bool,
    pub head_subject: Option<String>,
    /// Prefilled commit message for a merge being concluded.
    pub merge_message: Option<String>,
    pub has_submodules: bool,
    /// Some tracked .gitattributes routes files through Git LFS.
    pub uses_lfs: bool,
    /// A paused sync (sync.rs) here or in this repository's superproject.
    pub sync: Option<crate::sync::SyncSummary>,
}

/// Split porcelain `-z` output into NUL-delimited chunks. The trailing NUL
/// leaves an empty final chunk, which is dropped.
fn nul_chunks(data: &[u8]) -> Vec<&[u8]> {
    let mut out: Vec<&[u8]> = data.split(|b| *b == 0).collect();
    if let Some(last) = out.last() {
        if last.is_empty() {
            out.pop();
        }
    }
    out
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// Split `s` on spaces into `n` parts, the last of which keeps everything
/// remaining (a pathname may contain spaces).
fn fields(s: &str, n: usize) -> Option<Vec<&str>> {
    let parts: Vec<&str> = s.splitn(n, ' ').collect();
    if parts.len() == n {
        Some(parts)
    } else {
        None
    }
}

fn code_at(xy: &str, idx: usize) -> String {
    xy.chars().nth(idx).map(|c| c.to_string()).unwrap_or_else(|| ".".into())
}

/// Plain-English name for an unmerged `XY` pair.
pub fn classify_conflict(xy: &str) -> String {
    match xy {
        "DD" => "both deleted",
        "AU" => "added by us",
        "UD" => "deleted by them",
        "UA" => "added by them",
        "DU" => "deleted by us",
        "AA" => "both added",
        "UU" => "both modified",
        _ => "unmerged",
    }
    .to_string()
}

/// Submodule field: `N...` when not a submodule, otherwise `S<c><m><u>`.
fn submodule_flags(sub: &str) -> (bool, bool, bool, bool) {
    if !sub.starts_with('S') {
        return (false, false, false, false);
    }
    let c = sub.chars().nth(1) == Some('C');
    let m = sub.chars().nth(2) == Some('M');
    let u = sub.chars().nth(3) == Some('U');
    (true, c, m, u)
}

fn blank_entry(path: String) -> ChangeEntry {
    ChangeEntry {
        path,
        orig_path: None,
        staged: ".".into(),
        unstaged: ".".into(),
        kind: "tracked".into(),
        conflict: None,
        rename_score: None,
        is_submodule: false,
        sub_commit_changed: false,
        sub_tracked_changes: false,
        sub_untracked: false,
        recorded_oid: None,
        stage_modes: Vec::new(),
        stage_oids: Vec::new(),
        staged_added: None,
        staged_removed: None,
        unstaged_added: None,
        unstaged_removed: None,
        is_binary: false,
    }
}

/// Parse `git status --porcelain=v2 -z --branch --show-stash`.
///
/// The one trap in this format: with `-z`, a rename record's original path is a
/// *separate NUL chunk* following the `2 …` record. A naive split-and-map would
/// read it as a malformed record and corrupt everything after it, so this walks
/// the chunks with an index and consumes the extra one.
pub fn parse_porcelain_v2(data: &[u8]) -> ParsedStatus {
    let chunks = nul_chunks(data);
    let mut st = ParsedStatus::default();
    let mut i = 0usize;
    let mut untracked_seen = 0usize;

    while i < chunks.len() {
        let chunk = chunks[i];
        i += 1;
        if chunk.is_empty() {
            continue;
        }
        let line = lossy(chunk);

        if let Some(rest) = line.strip_prefix("# ") {
            let mut it = rest.splitn(2, ' ');
            let key = it.next().unwrap_or("");
            let value = it.next().unwrap_or("").trim();
            match key {
                "branch.oid" => {
                    if value == "(initial)" {
                        st.unborn = true;
                    } else {
                        st.head_oid = Some(value.to_string());
                    }
                }
                "branch.head" => {
                    if value == "(detached)" {
                        st.detached = true;
                    } else {
                        st.branch = Some(value.to_string());
                    }
                }
                "branch.upstream" => st.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            st.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            st.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                "stash" => st.stash_count = value.parse().unwrap_or(0),
                // The format explicitly allows headers we don't know; ignore them.
                _ => {}
            }
            continue;
        }

        match line.chars().next() {
            // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
            Some('1') => {
                let Some(f) = fields(&line, 9) else { continue };
                let (is_sub, c, m, u) = submodule_flags(f[2]);
                let mut e = blank_entry(f[8].to_string());
                e.staged = code_at(f[1], 0);
                e.unstaged = code_at(f[1], 1);
                if is_sub {
                    e.recorded_oid = Some(f[6].to_string());
                }
                e.is_submodule = is_sub;
                e.sub_commit_changed = c;
                e.sub_tracked_changes = m;
                e.sub_untracked = u;
                st.entries.push(e);
            }
            // 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\0<origPath>
            Some('2') => {
                let Some(f) = fields(&line, 10) else { continue };
                let orig = if i < chunks.len() {
                    let o = lossy(chunks[i]);
                    i += 1;
                    Some(o)
                } else {
                    None
                };
                let (is_sub, c, m, u) = submodule_flags(f[2]);
                let mut e = blank_entry(f[9].to_string());
                e.staged = code_at(f[1], 0);
                e.unstaged = code_at(f[1], 1);
                e.orig_path = orig;
                e.rename_score = f[8].get(1..).and_then(|s| s.parse().ok());
                if is_sub {
                    e.recorded_oid = Some(f[6].to_string());
                }
                e.is_submodule = is_sub;
                e.sub_commit_changed = c;
                e.sub_tracked_changes = m;
                e.sub_untracked = u;
                st.entries.push(e);
            }
            // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
            Some('u') => {
                let Some(f) = fields(&line, 11) else { continue };
                let mut e = blank_entry(f[10].to_string());
                e.kind = "conflicted".into();
                e.staged = code_at(f[1], 0);
                e.unstaged = code_at(f[1], 1);
                e.conflict = Some(classify_conflict(f[1]));
                let (is_sub, c, m, u) = submodule_flags(f[2]);
                e.is_submodule = is_sub;
                e.sub_commit_changed = c;
                e.sub_tracked_changes = m;
                e.sub_untracked = u;
                // The three index stages (base, ours, theirs). For a gitlink
                // conflict during a rebase, "ours" is the upstream side and
                // "theirs" the commit being replayed — the sync needs both.
                e.stage_modes = vec![f[3].to_string(), f[4].to_string(), f[5].to_string()];
                e.stage_oids = vec![f[7].to_string(), f[8].to_string(), f[9].to_string()];
                st.entries.push(e);
            }
            Some('?') => {
                untracked_seen += 1;
                if untracked_seen > MAX_UNTRACKED {
                    st.untracked_truncated = true;
                    continue;
                }
                let path = line.get(2..).unwrap_or("").to_string();
                let mut e = blank_entry(path);
                e.kind = "untracked".into();
                e.staged = ".".into();
                e.unstaged = "?".into();
                st.entries.push(e);
            }
            // '!' ignored entries are not requested; anything else is skipped
            // rather than allowed to abort the parse.
            _ => {}
        }
    }

    st
}

/// Parse `git diff --numstat -z`. Normal records are `add\tdel\tpath`; a rename
/// is `add\tdel\t` followed by two more chunks (from, then to).
pub fn parse_numstat(data: &[u8]) -> HashMap<String, (Option<u64>, Option<u64>, bool)> {
    let chunks = nul_chunks(data);
    let mut out = HashMap::new();
    let mut i = 0usize;
    while i < chunks.len() {
        let rec = lossy(chunks[i]);
        i += 1;
        let parts: Vec<&str> = rec.splitn(3, '\t').collect();
        if parts.len() < 3 {
            continue;
        }
        let binary = parts[0] == "-" || parts[1] == "-";
        let added = parts[0].parse::<u64>().ok();
        let removed = parts[1].parse::<u64>().ok();
        let path = if parts[2].is_empty() {
            // Rename: the two pathnames follow as separate chunks.
            let _from = if i < chunks.len() {
                let v = lossy(chunks[i]);
                i += 1;
                v
            } else {
                String::new()
            };
            if i < chunks.len() {
                let to = lossy(chunks[i]);
                i += 1;
                to
            } else {
                continue;
            }
        } else {
            parts[2].to_string()
        };
        out.insert(path, (added, removed, binary));
    }
    out
}

/// Which files git reports as present, for in-progress detection. Split out so
/// the classification is testable without a filesystem.
#[derive(Debug, Default, Clone)]
pub struct OpPresence {
    pub merge_head: bool,
    pub cherry_pick_head: bool,
    pub revert_head: bool,
    pub bisect_log: bool,
    pub rebase_merge: bool,
    pub rebase_apply: bool,
    pub rebase_interactive: bool,
    pub rebase_applying: bool,
    /// "3 of 7" progress, when git wrote it.
    pub step: Option<(usize, usize)>,
    pub head_name: Option<String>,
    pub onto: Option<String>,
}

pub fn parse_in_progress(p: &OpPresence) -> Option<InProgress> {
    let step_text = p
        .step
        .map(|(a, b)| format!("step {} of {}", a, b))
        .unwrap_or_default();
    let branch_text = match (&p.head_name, &p.onto) {
        (Some(h), Some(o)) => format!("{} onto {}", h.trim_start_matches("refs/heads/"), o),
        (Some(h), None) => h.trim_start_matches("refs/heads/").to_string(),
        _ => String::new(),
    };
    let join = |a: &str, b: &str| -> String {
        match (a.is_empty(), b.is_empty()) {
            (false, false) => format!("{}, {}", a, b),
            (false, true) => a.to_string(),
            (true, false) => b.to_string(),
            (true, true) => String::new(),
        }
    };

    if p.merge_head {
        return Some(InProgress {
            kind: "merge".into(),
            label: "Merge in progress".into(),
            detail: "Finish it with one commit, or abort.".into(),
            abort_command: "git merge --abort".into(),
            continue_command: Some("git commit --no-edit".into()),
        });
    }
    if p.rebase_merge || p.rebase_apply {
        let interactive = p.rebase_merge && p.rebase_interactive;
        let am = p.rebase_apply && p.rebase_applying;
        let kind = if am {
            "am"
        } else if interactive {
            "rebase-interactive"
        } else {
            "rebase"
        };
        let label = if am {
            "Applying patches (git am)"
        } else if interactive {
            "Interactive rebase in progress"
        } else {
            "Rebase in progress"
        };
        return Some(InProgress {
            kind: kind.into(),
            label: label.into(),
            detail: join(&step_text, &branch_text),
            abort_command: if am {
                "git am --abort".into()
            } else {
                "git rebase --abort".into()
            },
            continue_command: Some(if am {
                "git am --continue".into()
            } else {
                "git rebase --continue".into()
            }),
        });
    }
    if p.cherry_pick_head {
        return Some(InProgress {
            kind: "cherry-pick".into(),
            label: "Cherry-pick in progress".into(),
            detail: String::new(),
            abort_command: "git cherry-pick --abort".into(),
            continue_command: Some("git cherry-pick --continue".into()),
        });
    }
    if p.revert_head {
        return Some(InProgress {
            kind: "revert".into(),
            label: "Revert in progress".into(),
            detail: String::new(),
            abort_command: "git revert --abort".into(),
            continue_command: Some("git revert --continue".into()),
        });
    }
    if p.bisect_log {
        return Some(InProgress {
            kind: "bisect".into(),
            label: "Bisect in progress".into(),
            detail: String::new(),
            abort_command: "git bisect reset".into(),
            continue_command: None,
        });
    }
    None
}

/// Resolved locations inside `.git` — asked of git rather than assembled by
/// hand, because `.git` is a *file* in submodules and worktrees, and
/// `core.hooksPath` can move the hooks directory anywhere.
pub struct GitPaths {
    pub merge_head: PathBuf,
    pub cherry_pick_head: PathBuf,
    pub revert_head: PathBuf,
    pub bisect_log: PathBuf,
    pub rebase_merge: PathBuf,
    pub rebase_apply: PathBuf,
    pub merge_msg: PathBuf,
    pub hooks: PathBuf,
    pub fetch_head: PathBuf,
}

pub async fn git_paths(repo: &Path) -> Result<GitPaths, AppError> {
    let out = GitCmd::at(repo)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "MERGE_HEAD",
            "--git-path",
            "CHERRY_PICK_HEAD",
            "--git-path",
            "REVERT_HEAD",
            "--git-path",
            "BISECT_LOG",
            "--git-path",
            "rebase-merge",
            "--git-path",
            "rebase-apply",
            "--git-path",
            "MERGE_MSG",
            "--git-path",
            "hooks",
            "--git-path",
            "FETCH_HEAD",
        ])
        .text()
        .await?;
    let lines: Vec<PathBuf> = out.lines().map(PathBuf::from).collect();
    if lines.len() < 9 {
        return Err(AppError::Command(
            "Could not resolve this repository's git directory".into(),
        ));
    }
    Ok(GitPaths {
        merge_head: lines[0].clone(),
        cherry_pick_head: lines[1].clone(),
        revert_head: lines[2].clone(),
        bisect_log: lines[3].clone(),
        rebase_merge: lines[4].clone(),
        rebase_apply: lines[5].clone(),
        merge_msg: lines[6].clone(),
        hooks: lines[7].clone(),
        fetch_head: lines[8].clone(),
    })
}

fn read_num(path: &Path) -> Option<usize> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_line_file(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

pub(crate) fn presence_from_disk(p: &GitPaths) -> OpPresence {
    let rebase_merge = p.rebase_merge.is_dir();
    let rebase_apply = p.rebase_apply.is_dir();
    let step = if rebase_merge {
        match (
            read_num(&p.rebase_merge.join("msgnum")),
            read_num(&p.rebase_merge.join("end")),
        ) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        }
    } else if rebase_apply {
        match (
            read_num(&p.rebase_apply.join("next")),
            read_num(&p.rebase_apply.join("last")),
        ) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        }
    } else {
        None
    };
    OpPresence {
        merge_head: p.merge_head.exists(),
        cherry_pick_head: p.cherry_pick_head.exists(),
        revert_head: p.revert_head.exists(),
        bisect_log: p.bisect_log.exists(),
        rebase_merge,
        rebase_apply,
        rebase_interactive: p.rebase_merge.join("interactive").exists(),
        rebase_applying: p.rebase_apply.join("applying").exists(),
        step,
        head_name: read_line_file(&p.rebase_merge.join("head-name"))
            .or_else(|| read_line_file(&p.rebase_apply.join("head-name"))),
        onto: read_line_file(&p.rebase_merge.join("onto")),
    }
}

/// What git will actually sign and stamp on the next commit, plus how that
/// compares with the folder's profile.
async fn read_identity(repo: &Path, hooks: &Path) -> RepoIdentity {
    let ident = GitCmd::at(repo)
        .args(["var", "GIT_AUTHOR_IDENT"])
        .ok_text()
        .await
        .unwrap_or_default();
    // "Name <email> 1758… +0530"
    let (name, email) = match (ident.rfind(" <"), ident.rfind('>')) {
        (Some(a), Some(b)) if b > a => (
            ident[..a].trim().to_string(),
            ident[a + 2..b].trim().to_string(),
        ),
        _ => (String::new(), String::new()),
    };

    // Scope matters: a repo-local user.email silently beats the profile's
    // includeIf, so the UI has to be able to say *where* the value came from.
    let scoped = GitCmd::at(repo)
        .args([
            "config",
            "--show-scope",
            "--show-origin",
            "--get",
            "user.email",
        ])
        .ok_text()
        .await
        .unwrap_or_default();
    let mut parts = scoped.split('\t');
    let email_scope = parts.next().unwrap_or("").trim().to_string();
    let email_origin = parts
        .next()
        .map(|s| s.trim().trim_start_matches("file:").to_string())
        .filter(|s| !s.is_empty());

    let signing_on = GitCmd::at(repo)
        .args(["config", "--bool", "--get", "commit.gpgsign"])
        .ok_text()
        .await
        .map(|v| v == "true")
        .unwrap_or(false);
    let signing_key = GitCmd::at(repo)
        .args(["config", "--get", "user.signingkey"])
        .ok_text()
        .await
        .filter(|s| !s.is_empty());

    let repo_str = crate::paths::norm(&repo.to_string_lossy());
    let store = profiles::load_profiles().unwrap_or_else(|_| profiles::ProfileStore::new());
    let profile = crate::commit_audit::owning_profile(&repo_str, &store.profiles);

    let matches_profile = match profile {
        Some(p) => p.git_email.trim().eq_ignore_ascii_case(email.trim()),
        None => true, // no profile claims this folder: nothing to disagree with
    };

    RepoIdentity {
        name,
        email,
        email_scope: if email_scope.is_empty() {
            "unset".into()
        } else {
            email_scope
        },
        email_origin,
        profile_id: profile.map(|p| p.id.clone()),
        profile_name: profile.map(|p| p.name.clone()),
        profile_email: profile.map(|p| p.git_email.clone()),
        matches_profile,
        signing_on,
        signing_key,
        guard: crate::commit_audit::guard_state_in(hooks),
    }
}

/// Seconds since the last fetch, from FETCH_HEAD's mtime.
fn seconds_since(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    modified.elapsed().ok().map(|d| d.as_secs())
}

/// Everything the Changes page needs about one repository, in one call.
pub async fn repo_status(repo_path: &str) -> Result<RepoStatus, AppError> {
    let repo = PathBuf::from(repo_path);
    if !repo.is_dir() {
        return Err(AppError::NotFound(format!(
            "{} no longer exists on disk",
            repo_path
        )));
    }
    // Reject anything that isn't a work tree before running six more commands.
    let inside = GitCmd::at(&repo)
        .args(["rev-parse", "--is-inside-work-tree"])
        .ok_text()
        .await;
    if inside.as_deref() != Some("true") {
        return Err(AppError::NotFound(format!(
            "{} is not a git repository",
            repo_path
        )));
    }

    let status_out = GitCmd::at(&repo)
        // Don't fight the user's terminal for index.lock while polling.
        .top_flag("--no-optional-locks")
        .args([
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--show-stash",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ])
        .timeout(READ_TIMEOUT)
        .run()
        .await?;
    if !status_out.ok() {
        // With filter.lfs.required set and git-lfs gone, status itself fails
        // with a message that looks like a dropped connection. Say what it is.
        if let Some(msg) = crate::lfs::missing_lfs_message(&status_out.stderr) {
            return Err(AppError::Command(msg));
        }
        return Err(AppError::Command(format!(
            "git status failed: {}",
            status_out.stderr
        )));
    }
    let mut parsed = parse_porcelain_v2(&status_out.stdout);

    // Line counts come from diff, which status does not provide.
    let unstaged_stats = GitCmd::at(&repo)
        .args(["diff", "--numstat", "-z"])
        .run()
        .await
        .map(|o| parse_numstat(&o.stdout))
        .unwrap_or_default();
    let staged_stats = GitCmd::at(&repo)
        .args(["diff", "--cached", "--numstat", "-z"])
        .run()
        .await
        .map(|o| parse_numstat(&o.stdout))
        .unwrap_or_default();

    for e in parsed.entries.iter_mut() {
        // A gitlink is a pointer, not text. `git diff --numstat -- <sub>` still
        // reports "1 1 <path>" for a moved submodule, which would render as
        // "+1 −1" and read as if one line changed inside it.
        if e.is_submodule {
            continue;
        }
        if let Some((a, r, bin)) = unstaged_stats.get(&e.path) {
            e.unstaged_added = *a;
            e.unstaged_removed = *r;
            e.is_binary |= *bin;
        }
        if let Some((a, r, bin)) = staged_stats.get(&e.path) {
            e.staged_added = *a;
            e.staged_removed = *r;
            e.is_binary |= *bin;
        }
    }

    let paths = git_paths(&repo).await?;
    let operation = parse_in_progress(&presence_from_disk(&paths));
    let merge_message = if paths.merge_head.exists() {
        std::fs::read_to_string(&paths.merge_msg)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    let identity = read_identity(&repo, &paths.hooks).await;
    let push =
        crate::push_guard::push_state(&repo, &paths.hooks, identity.profile_id.as_deref()).await;

    // Amending a published commit would require a force-push, which this app
    // never does — so the UI must know before offering the checkbox.
    let head_subject = GitCmd::at(&repo)
        .args(["log", "-1", "--format=%s"])
        .ok_text()
        .await;
    let published = GitCmd::at(&repo)
        .args([
            "for-each-ref",
            "--contains=HEAD",
            "--format=%(refname)",
            "refs/remotes",
        ])
        .ok_text()
        .await
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let can_amend = !parsed.unborn && head_subject.is_some() && !published && operation.is_none();

    let has_submodules = repo.join(".gitmodules").exists();
    let uses_lfs = crate::lfs::repo_uses_lfs(&repo).await;

    let staged_count = parsed.entries.iter().filter(|e| e.kind == "tracked" && e.is_staged()).count();
    let unstaged_count = parsed
        .entries
        .iter()
        .filter(|e| e.kind == "tracked" && e.is_unstaged())
        .count();
    let untracked_count = parsed.entries.iter().filter(|e| e.kind == "untracked").count();
    let conflicted_count = parsed.entries.iter().filter(|e| e.kind == "conflicted").count();
    let submodule_dirty_count = parsed
        .entries
        .iter()
        .filter(|e| e.is_submodule && (e.sub_commit_changed || e.sub_tracked_changes || e.sub_untracked))
        .count();

    let name = crate::paths::base_name(repo_path);

    Ok(RepoStatus {
        path: crate::paths::norm(repo_path),
        name,
        branch: parsed.branch.clone(),
        detached: parsed.detached,
        unborn: parsed.unborn,
        head_oid: parsed.head_oid.clone(),
        upstream: parsed.upstream.clone(),
        ahead: parsed.ahead,
        behind: parsed.behind,
        last_fetch_secs: seconds_since(&paths.fetch_head),
        entries: parsed.entries,
        staged_count,
        unstaged_count,
        untracked_count,
        conflicted_count,
        submodule_dirty_count,
        untracked_truncated: parsed.untracked_truncated,
        stash_count: parsed.stash_count,
        operation,
        identity,
        push,
        can_amend,
        head_subject,
        merge_message,
        has_submodules,
        uses_lfs,
        sync: crate::sync::summary_for(&repo).await,
    })
}

pub(crate) fn owning_profile_id(repo_path: &str) -> Option<String> {
    let store = profiles::load_profiles().ok()?;
    let repo = crate::paths::norm(repo_path);
    crate::commit_audit::owning_profile(&repo, &store.profiles).map(|p| p.id.clone())
}

/// Push state on its own, for the toggle — resolves the hooks directory the
/// same way `repo_status` does.
pub async fn push_state_for(repo_path: &str) -> Result<crate::push_guard::PushState, AppError> {
    let repo = PathBuf::from(repo_path);
    let paths = git_paths(&repo).await?;
    Ok(crate::push_guard::push_state(&repo, &paths.hooks, owning_profile_id(repo_path).as_deref()).await)
}

pub async fn repair_push_block_for(repo_path: &str) -> Result<crate::push_guard::PushState, AppError> {
    let paths = git_paths(&PathBuf::from(repo_path)).await?;
    crate::push_guard::repair(repo_path, &paths.hooks, owning_profile_id(repo_path).as_deref()).await
}

/// A diff rendered for the UI. Capped, because a 40 MB generated file would
/// otherwise be shipped through the IPC bridge and into the DOM.
#[derive(Debug, Serialize, Clone)]
pub struct FileDiff {
    pub path: String,
    pub staged: bool,
    pub lines: Vec<DiffLine>,
    pub is_binary: bool,
    pub truncated: bool,
    pub total_lines: usize,
    pub added: usize,
    pub removed: usize,
    /// Set when there is simply nothing to show (e.g. a staged-only file
    /// requested on the unstaged side).
    pub empty_reason: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct DiffLine {
    /// "hunk" | "add" | "del" | "context" | "meta"
    pub kind: String,
    pub text: String,
}

const MAX_DIFF_LINES: usize = 3000;
const MAX_DIFF_BYTES: usize = 400 * 1024;

fn classify_diff_line(line: &str) -> Option<&'static str> {
    if line.starts_with("@@") {
        Some("hunk")
    } else if line.starts_with("+++") || line.starts_with("---") {
        Some("meta")
    } else if line.starts_with('+') {
        Some("add")
    } else if line.starts_with('-') {
        Some("del")
    } else if line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("new file")
        || line.starts_with("deleted file")
        || line.starts_with("old mode")
        || line.starts_with("new mode")
        || line.starts_with("similarity index")
        || line.starts_with("rename from")
        || line.starts_with("rename to")
        || line.starts_with("Binary files")
        || line.starts_with('\\')
    {
        Some("meta")
    } else {
        Some("context")
    }
}

/// Render one file's diff. `staged` selects the index-vs-HEAD side; untracked
/// files are diffed against /dev/null so a new file still shows its contents.
pub async fn file_diff(repo_path: &str, path: &str, staged: bool, untracked: bool) -> Result<FileDiff, AppError> {
    let repo = PathBuf::from(repo_path);
    let full = repo.join(path);

    let out = if untracked {
        // --no-index exits 1 when the files differ, which is the normal case.
        GitCmd::at(&repo)
            .args([
                "diff",
                "--no-color",
                "--no-index",
                "--",
                "/dev/null",
                &full.to_string_lossy(),
            ])
            .run()
            .await?
    } else {
        // --ignore-submodules=none is required or `git diff -- <submodule>`
        // prints nothing at all, and the panel then claims the submodule has no
        // changes while it may hold thousands.
        let mut cmd = GitCmd::at(&repo).args(["diff", "--no-color", "--ignore-submodules=none"]);
        if staged {
            cmd = cmd.arg("--cached");
        }
        cmd.args(["--", path]).run().await?
    };

    if !out.ok() && out.stdout.is_empty() && !out.stderr.is_empty() {
        return Err(AppError::Command(format!("git diff failed: {}", out.stderr)));
    }

    let raw = out.stdout;
    let is_binary = raw.contains(&0)
        || String::from_utf8_lossy(&raw[..raw.len().min(4096)]).contains("Binary files ");
    if is_binary {
        return Ok(FileDiff {
            path: path.to_string(),
            staged,
            lines: Vec::new(),
            is_binary: true,
            truncated: false,
            total_lines: 0,
            added: 0,
            removed: 0,
            empty_reason: Some("Binary file — no text diff to show.".into()),
        });
    }

    let too_big = raw.len() > MAX_DIFF_BYTES;
    let text = String::from_utf8_lossy(&raw[..raw.len().min(MAX_DIFF_BYTES)]);
    let all: Vec<&str> = text.lines().collect();
    let total_lines = all.len();
    let truncated = too_big || total_lines > MAX_DIFF_LINES;
    let shown = all.iter().take(MAX_DIFF_LINES);

    let mut lines = Vec::new();
    let mut added = 0usize;
    let mut removed = 0usize;
    for l in shown {
        let kind = classify_diff_line(l).unwrap_or("context");
        if kind == "add" {
            added += 1;
        } else if kind == "del" {
            removed += 1;
        }
        lines.push(DiffLine {
            kind: kind.to_string(),
            text: (*l).to_string(),
        });
    }

    let empty_reason = if lines.is_empty() {
        Some(if staged {
            "Nothing staged for this file.".into()
        } else {
            "No unstaged changes for this file.".into()
        })
    } else {
        None
    };

    Ok(FileDiff {
        path: path.to_string(),
        staged,
        lines,
        is_binary: false,
        truncated,
        total_lines,
        added,
        removed,
        empty_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z(parts: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in parts {
            out.extend_from_slice(p.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn clean_repo_gives_headers_and_no_entries() {
        let data = z(&[
            "# branch.oid 6664656d527e644b0662abc9bdc47273d70e7fce",
            "# branch.head FixesV1",
            "# branch.upstream origin/FixesV1",
            "# branch.ab +2 -3",
        ]);
        let st = parse_porcelain_v2(&data);
        assert_eq!(st.branch.as_deref(), Some("FixesV1"));
        assert_eq!(st.upstream.as_deref(), Some("origin/FixesV1"));
        assert_eq!((st.ahead, st.behind), (2, 3));
        assert!(st.entries.is_empty());
        assert!(!st.detached && !st.unborn);
    }

    #[test]
    fn unborn_and_detached_heads_are_flagged() {
        let st = parse_porcelain_v2(&z(&["# branch.oid (initial)", "# branch.head (detached)"]));
        assert!(st.unborn);
        assert!(st.detached);
        assert!(st.branch.is_none());
        assert!(st.head_oid.is_none());
    }

    #[test]
    fn missing_upstream_headers_are_not_invented() {
        let st = parse_porcelain_v2(&z(&["# branch.oid abc", "# branch.head main"]));
        assert!(st.upstream.is_none());
        assert_eq!((st.ahead, st.behind), (0, 0));
    }

    #[test]
    fn ordinary_records_split_staged_from_unstaged() {
        let st = parse_porcelain_v2(&z(&[
            "1 .M N... 100644 100644 100644 aaa bbb src/app.ts",
            "1 M. N... 100644 100644 100644 aaa bbb src/new.ts",
            "1 MM N... 100644 100644 100644 aaa bbb src/both.ts",
            "1 D. N... 100644 000000 000000 aaa bbb gone.txt",
        ]));
        assert_eq!(st.entries.len(), 4);
        assert_eq!((st.entries[0].staged.as_str(), st.entries[0].unstaged.as_str()), (".", "M"));
        assert!(!st.entries[0].is_staged() && st.entries[0].is_unstaged());
        assert!(st.entries[1].is_staged() && !st.entries[1].is_unstaged());
        assert!(st.entries[2].is_staged() && st.entries[2].is_unstaged());
        assert_eq!(st.entries[3].staged, "D");
    }

    #[test]
    fn rename_original_path_is_a_separate_chunk_and_does_not_shift_later_entries() {
        // The one framing trap in porcelain v2 -z.
        let st = parse_porcelain_v2(&z(&[
            "2 R. N... 100644 100644 100644 aaa bbb R100 new/path.ts",
            "old/path.ts",
            "? untracked.txt",
            "1 .M N... 100644 100644 100644 aaa bbb after.ts",
        ]));
        assert_eq!(st.entries.len(), 3, "the original path must not become an entry");
        assert_eq!(st.entries[0].path, "new/path.ts");
        assert_eq!(st.entries[0].orig_path.as_deref(), Some("old/path.ts"));
        assert_eq!(st.entries[0].rename_score, Some(100));
        assert_eq!(st.entries[0].staged, "R");
        // Everything after the rename still lands correctly.
        assert_eq!(st.entries[1].path, "untracked.txt");
        assert_eq!(st.entries[1].kind, "untracked");
        assert_eq!(st.entries[2].path, "after.ts");
    }

    #[test]
    fn copy_records_carry_their_source_too() {
        let st = parse_porcelain_v2(&z(&[
            "2 C. N... 100644 100644 100644 aaa bbb C75 copy.ts",
            "origin.ts",
        ]));
        assert_eq!(st.entries[0].staged, "C");
        assert_eq!(st.entries[0].rename_score, Some(75));
        assert_eq!(st.entries[0].orig_path.as_deref(), Some("origin.ts"));
    }

    #[test]
    fn every_unmerged_state_gets_plain_words() {
        let st = parse_porcelain_v2(&z(&[
            "u UU N... 100644 100644 100644 100644 a b c both.txt",
            "u DD N... 100644 100644 100644 100644 a b c gone.txt",
            "u AU N... 100644 100644 100644 100644 a b c ours.txt",
            "u UD N... 100644 100644 100644 100644 a b c theirs.txt",
        ]));
        assert_eq!(st.entries.len(), 4);
        assert!(st.entries.iter().all(|e| e.kind == "conflicted"));
        assert_eq!(st.entries[0].conflict.as_deref(), Some("both modified"));
        assert_eq!(st.entries[1].conflict.as_deref(), Some("both deleted"));
        assert_eq!(st.entries[2].conflict.as_deref(), Some("added by us"));
        assert_eq!(st.entries[3].conflict.as_deref(), Some("deleted by them"));
        assert_eq!(classify_conflict("AA"), "both added");
        assert_eq!(classify_conflict("DU"), "deleted by us");
    }

    #[test]
    fn a_filename_containing_a_newline_stays_one_entry() {
        // Proves the -z framing end to end: without it this would parse as two.
        let st = parse_porcelain_v2(&z(&["? weird\nname.txt", "? plain.txt"]));
        assert_eq!(st.entries.len(), 2);
        assert_eq!(st.entries[0].path, "weird\nname.txt");
        assert_eq!(st.entries[1].path, "plain.txt");
    }

    #[test]
    fn filenames_with_spaces_and_separators_survive() {
        let st = parse_porcelain_v2(&z(&[
            "1 .M N... 100644 100644 100644 aaa bbb my docs/a file.md",
            "? ssh setup.md",
            "1 .M N... 100644 100644 100644 aaa bbb tab\there.txt",
        ]));
        assert_eq!(st.entries[0].path, "my docs/a file.md");
        assert_eq!(st.entries[1].path, "ssh setup.md");
        assert_eq!(st.entries[2].path, "tab\there.txt");
    }

    #[test]
    fn submodule_dirtiness_is_broken_out() {
        let st = parse_porcelain_v2(&z(&[
            "1 .M S.M. 160000 160000 160000 aaa bbb vendor/lib",
            "1 .M SC.. 160000 160000 160000 aaa bbb vendor/moved",
            "1 .M S..U 160000 160000 160000 aaa bbb vendor/untracked",
            "1 .M N... 100644 100644 100644 aaa bbb normal.txt",
        ]));
        assert!(st.entries[0].is_submodule && st.entries[0].sub_tracked_changes);
        assert!(!st.entries[0].sub_commit_changed);
        assert!(st.entries[1].sub_commit_changed);
        assert!(st.entries[2].sub_untracked);
        assert!(!st.entries[3].is_submodule);
    }

    #[test]
    fn stash_header_is_read_and_defaults_to_zero() {
        assert_eq!(parse_porcelain_v2(&z(&["# stash 4"])).stash_count, 4);
        assert_eq!(parse_porcelain_v2(&z(&["# branch.head main"])).stash_count, 0);
    }

    #[test]
    fn unknown_headers_and_garbage_records_never_abort_the_parse() {
        let st = parse_porcelain_v2(&z(&[
            "# branch.head main",
            "# some.future.header value",
            "1 short",
            "!! ignored.txt",
            "",
            "? real.txt",
        ]));
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.entries.len(), 1);
        assert_eq!(st.entries[0].path, "real.txt");
    }

    #[test]
    fn untracked_files_are_capped_and_the_cap_is_reported() {
        let many: Vec<String> = (0..MAX_UNTRACKED + 5)
            .map(|i| format!("? file{}.txt", i))
            .collect();
        let refs: Vec<&str> = many.iter().map(|s| s.as_str()).collect();
        let st = parse_porcelain_v2(&z(&refs));
        assert_eq!(st.entries.len(), MAX_UNTRACKED);
        assert!(st.untracked_truncated);
    }

    #[test]
    fn numstat_reads_counts_renames_and_binaries() {
        let data = z(&["3\t1\tsrc/app.ts", "10\t0\t", "old.ts", "new.ts", "-\t-\tlogo.png"]);
        let map = parse_numstat(&data);
        assert_eq!(map.get("src/app.ts"), Some(&(Some(3), Some(1), false)));
        // A rename is keyed by its destination, matching status's path.
        assert_eq!(map.get("new.ts"), Some(&(Some(10), Some(0), false)));
        assert!(!map.contains_key("old.ts"));
        assert_eq!(map.get("logo.png"), Some(&(None, None, true)));
    }

    #[test]
    fn a_gitlink_conflict_keeps_its_three_stages() {
        let line = b"u UU S... 160000 160000 160000 160000 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb cccccccccccccccccccccccccccccccccccccccc staging\0";
        let st = parse_porcelain_v2(line);
        let e = &st.entries[0];
        assert!(e.is_submodule);
        assert_eq!(e.kind, "conflicted");
        assert_eq!(e.stage_modes, vec!["160000", "160000", "160000"]);
        assert_eq!(e.stage_oids[1], "b".repeat(40), "stage 2 is ours (upstream during a rebase)");
        assert_eq!(e.stage_oids[2], "c".repeat(40), "stage 3 is theirs (the commit being replayed)");
        // An ordinary tracked entry carries no stages, and serialises none.
        let plain = parse_porcelain_v2(b"1 .M N... 100644 100644 100644 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa file.txt\0");
        assert!(plain.entries[0].stage_oids.is_empty());
        assert!(!serde_json::to_string(&plain.entries[0]).unwrap().contains("stage_oids"));
    }

    #[test]
    fn in_progress_classification_names_the_right_abort_command() {
        let none = OpPresence::default();
        assert!(parse_in_progress(&none).is_none());

        let merge = OpPresence {
            merge_head: true,
            ..Default::default()
        };
        let op = parse_in_progress(&merge).unwrap();
        assert_eq!(op.kind, "merge");
        assert_eq!(op.abort_command, "git merge --abort");

        let rebase = OpPresence {
            rebase_merge: true,
            step: Some((3, 7)),
            head_name: Some("refs/heads/feature".into()),
            onto: Some("main".into()),
            ..Default::default()
        };
        let op = parse_in_progress(&rebase).unwrap();
        assert_eq!(op.kind, "rebase");
        assert_eq!(op.detail, "step 3 of 7, feature onto main");
        assert_eq!(op.abort_command, "git rebase --abort");

        let interactive = OpPresence {
            rebase_merge: true,
            rebase_interactive: true,
            ..Default::default()
        };
        assert_eq!(parse_in_progress(&interactive).unwrap().kind, "rebase-interactive");

        let am = OpPresence {
            rebase_apply: true,
            rebase_applying: true,
            ..Default::default()
        };
        let op = parse_in_progress(&am).unwrap();
        assert_eq!(op.kind, "am");
        assert_eq!(op.abort_command, "git am --abort");

        let cherry = OpPresence {
            cherry_pick_head: true,
            ..Default::default()
        };
        assert_eq!(
            parse_in_progress(&cherry).unwrap().abort_command,
            "git cherry-pick --abort"
        );

        let revert = OpPresence {
            revert_head: true,
            ..Default::default()
        };
        assert_eq!(parse_in_progress(&revert).unwrap().kind, "revert");

        let bisect = OpPresence {
            bisect_log: true,
            ..Default::default()
        };
        assert_eq!(parse_in_progress(&bisect).unwrap().abort_command, "git bisect reset");
    }

    #[test]
    fn a_merge_outranks_a_leftover_rebase_directory() {
        let both = OpPresence {
            merge_head: true,
            rebase_merge: true,
            ..Default::default()
        };
        assert_eq!(parse_in_progress(&both).unwrap().kind, "merge");
    }

    #[test]
    fn diff_lines_are_classified_for_colouring() {
        assert_eq!(classify_diff_line("@@ -1,3 +1,4 @@"), Some("hunk"));
        assert_eq!(classify_diff_line("+added"), Some("add"));
        assert_eq!(classify_diff_line("-removed"), Some("del"));
        assert_eq!(classify_diff_line(" context"), Some("context"));
        assert_eq!(classify_diff_line("diff --git a/x b/x"), Some("meta"));
        assert_eq!(classify_diff_line("--- a/x"), Some("meta"));
        assert_eq!(classify_diff_line("+++ b/x"), Some("meta"));
        assert_eq!(classify_diff_line("\\ No newline at end of file"), Some("meta"));
    }
}
