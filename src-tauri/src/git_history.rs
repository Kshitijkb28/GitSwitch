use crate::error::AppError;
use crate::profiles;
use crate::repo_scan;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Unit separator — safe inside git format strings, never appears in messages.
const US: char = '\x1f';

#[derive(Debug, Serialize, Clone)]
pub struct RepoRef {
    pub path: String,
    pub name: String,
    pub profile_id: String,
    pub profile_name: String,
    pub profile_email: String,
    /// A repository, or a profile folder that holds none (see `list_repos`).
    pub kind: RepoKind,
    /// A folder entry inside a repository: that repository's top folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inside: Option<String>,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RepoKind {
    #[default]
    Repo,
    /// A profile folder with no repository in it: listed so it doesn't vanish
    /// from the page, which offers to make it one.
    Folder,
}

#[derive(Debug, Serialize, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub tip: String,
    pub short_tip: String,
    pub last_author: String,
    pub last_date: String,
    pub last_subject: String,
    /// One of GitSwitch's own backup branches (see `is_safety_branch`).
    #[serde(default)]
    pub is_backup: bool,
    /// `refs/heads/x` or `refs/remotes/origin/x` — unambiguous where the
    /// short name is not (a remote ref shortens to `remotes/origin/x` when a
    /// local branch is called `origin/x`).
    #[serde(default)]
    pub full_ref: String,
    /// A remote-tracking branch's name on its remote: `x` for `origin/x`,
    /// `team/x` for `origin/team/x`. None for a local branch, or a remote ref
    /// no configured remote owns.
    #[serde(default)]
    pub remote_branch: Option<String>,
}

/// The branch name on the remote for `refs/remotes/<remote>/<branch>`. A
/// remote's name may hold a `/` itself, so the longest configured remote that
/// prefixes the ref wins.
fn split_remote(full_ref: &str, remotes: &[String]) -> Option<String> {
    let rest = full_ref.strip_prefix("refs/remotes/")?;
    remotes
        .iter()
        .filter(|r| rest.len() > r.len() + 1 && rest.starts_with(r.as_str()) && rest.as_bytes()[r.len()] == b'/')
        .max_by_key(|r| r.len())
        .map(|r| rest[r.len() + 1..].to_string())
}

#[derive(Debug, Serialize, Clone)]
pub struct HistoryCommit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    /// The committer date — what the graph is ordered by.
    pub date: String,
    pub subject: String,
    pub refs: Vec<String>,
    pub is_merge: bool,
    /// The author date: stable across the rebases an update performs, so a
    /// row keeps its time when its id changes. Shown; `date` sits in the title.
    pub author_date: String,
    /// Submodule pointers this commit moves.
    pub touches_submodules: Vec<String>,
    /// It changes nothing but submodule pointers — a bump, not work.
    pub only_submodules: bool,
    /// No remote-tracking branch has it: it exists only here. Meaningful
    /// only when the page says there are remote-tracking branches at all.
    pub unpushed: bool,
    // graph placement
    pub lane: usize,
    pub parent_lanes: Vec<usize>,
    pub active_lanes: Vec<usize>,
    /// A lane was already waiting for it (a child above it, on this page or
    /// an earlier one): its line comes from above. False for a tip.
    pub from_above: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct HistoryPage {
    pub commits: Vec<HistoryCommit>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub max_lane: usize,
    /// The repository has at least one remote-tracking branch to compare
    /// with. Without one nothing here is known to be pushed, and the rows
    /// carry no mark.
    pub remote_refs: bool,
    /// The comparison with the remote-tracking branches failed (a
    /// remote-tracking ref pointing at a missing object, say): the rows carry
    /// no mark, and this says why.
    pub marks_error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct FileChange {
    pub path: String,
    pub added: String,
    pub removed: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct CommitDetail {
    pub hash: String,
    pub short: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub committer_name: String,
    pub committer_email: String,
    pub subject: String,
    pub body: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub files: Vec<FileChange>,
    pub signature: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct FetchResult {
    pub message: String,
    /// Proof, not a promise: these are re-checked after the fetch runs.
    pub head_unchanged: bool,
    pub worktree_unchanged: bool,
    pub local_branches_unchanged: bool,
    /// How many remote-tracking refs moved — the only thing fetch may change.
    pub updated_remote_refs: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct CommitRef {
    pub short: String,
    pub subject: String,
    pub author: String,
    pub date: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct SyncStatus {
    /// Branch these numbers describe (resolved from HEAD for the "all" view).
    pub branch: String,
    pub upstream: Option<String>,
    /// Local commits the remote doesn't have.
    pub ahead: usize,
    /// Remote commits you haven't pulled — the number people actually want.
    pub behind: usize,
    /// Seconds since the last `git fetch`, from .git/FETCH_HEAD.
    pub last_fetch_secs: Option<u64>,
    /// Revision range listing exactly the commits you're missing.
    pub incoming_rev: Option<String>,
    /// Where your local branch is pointing right now.
    pub local_tip: Option<CommitRef>,
    /// Where the remote branch is pointing (as of the last fetch).
    pub remote_tip: Option<CommitRef>,
}

/// `rev` is a branch name the repository supplies (HEAD's, its upstream's):
/// after `--end-of-options` (git 2.24) it is never read as an option, even
/// for a ref named `refs/heads/--output=<file>`.
fn commit_ref(repo: &Path, rev: &str) -> Option<CommitRef> {
    let raw = git(repo, &["log", "-1", &format!("--format=%h{US}%s{US}%an{US}%cI", US = US), "--end-of-options", rev]).ok()?;
    let f: Vec<&str> = raw.splitn(4, US).collect();
    if f.len() < 4 {
        return None;
    }
    Some(CommitRef {
        short: f[0].into(),
        subject: f[1].into(),
        author: f[2].into(),
        date: f[3].into(),
    })
}

#[derive(Debug, Serialize, Clone)]
pub struct MergeInfo {
    /// Branches whose history contains this branch's tip — i.e. it's merged in.
    pub merged_into: Vec<String>,
    /// The merge commits on this branch, newest first (at most 50).
    pub merges: Vec<HistoryCommit>,
    /// How the tip got into the first few of `merged_into`.
    pub merged_by: Vec<MergedBy>,
    /// Every merge commit in the branch's history, and whether `merges` stops short.
    pub merges_total: usize,
    pub merges_truncated: bool,
}

/// How a branch's tip got into another branch.
#[derive(Debug, Serialize, Clone)]
pub struct MergedBy {
    pub branch: String,
    /// "merge" | "fast-forward" | "same-commit"
    pub how: String,
    /// The merge commit that brought it in, for "merge".
    pub commit: Option<MergeCommit>,
}

#[derive(Debug, Serialize, Clone)]
pub struct MergeCommit {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author_name: String,
    pub date: String,
    pub parents: Vec<String>,
}

/// How `tip` got into `target` (a full ref that contains it): the first
/// commit on the target's main line that contains the tip. Its first parent
/// being the tip means the commits sit on that line — a fast-forward; anything
/// else is the merge that brought the tip in through another parent.
fn merged_by(repo: &Path, tip: &str, target: &str) -> Option<(String, Option<MergeCommit>)> {
    let fp = git(repo, &["rev-list", "--first-parent", "--end-of-options", &format!("{}..{}", tip, target)]).ok()?;
    let fp: Vec<&str> = fp.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if fp.is_empty() {
        return Some(("same-commit".into(), None));
    }
    let anc = git(repo, &["rev-list", "--ancestry-path", "--end-of-options", &format!("{}..{}", tip, target)]).ok()?;
    let anc: std::collections::HashSet<&str> = anc.lines().map(str::trim).collect();
    let m = fp.iter().rev().find(|c| anc.contains(**c))?;
    let raw = git(repo, &["log", "-n1", "--no-show-signature", &format!("--format=%H{US}%h{US}%P{US}%aN{US}%aI{US}%s", US = US), "--end-of-options", m]).ok()?;
    let f: Vec<&str> = raw.trim_end_matches('\n').splitn(6, US).collect();
    if f.len() < 6 {
        return None;
    }
    let parents: Vec<String> = f[2].split_whitespace().map(str::to_string).collect();
    if parents.first().map(String::as_str) == Some(tip) {
        return Some(("fast-forward".into(), None));
    }
    Some((
        "merge".into(),
        Some(MergeCommit { hash: f[0].to_string(), short: f[1].to_string(), parents, author_name: f[3].to_string(), date: f[4].to_string(), subject: f[5].trim().to_string() }),
    ))
}

/// `refs/remotes/<remote>/HEAD`: a remote's symbolic pointer at its default
/// branch, not a branch. Its short name is just the remote's ("origin"), so
/// it has to be recognised by the full ref.
pub(crate) fn is_remote_head(full_ref: &str) -> bool {
    full_ref.starts_with("refs/remotes/") && full_ref.ends_with("/HEAD")
}

fn git(repo: &Path, args: &[&str]) -> Result<String, AppError> {
    git_with(repo, args, crate::git_exec::LOCAL_TIMEOUT)
}

/// The clock for a walk of the whole history: the History page's count and
/// list, the people list, a branch's merges. These are the slowest reads the
/// app makes (path-limited `--full-history --simplify-merges` over a big
/// repository with submodules), and before they went through `GitCmd` they
/// had no clock at all. Waiting for a big walk is what the page has always
/// done; the clock is there to end a hang, not a large repository.
const WALK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

#[cfg(test)]
thread_local! {
    /// A test's own walk clock, so a stopped walk can be produced on demand.
    /// Per thread: the page reads it on the caller's thread, and cargo runs
    /// tests side by side.
    static WALK_CLOCK: std::cell::Cell<Option<std::time::Duration>> = const { std::cell::Cell::new(None) };
}

fn walk_clock() -> std::time::Duration {
    #[cfg(test)]
    if let Some(d) = WALK_CLOCK.with(|c| c.get()) {
        return d;
    }
    WALK_TIMEOUT
}

fn git_with(repo: &Path, args: &[&str], timeout: std::time::Duration) -> Result<String, AppError> {
    // The one runner: its environment, its log, and the signature pin that
    // keeps verification text out of parsed output.
    let out = crate::git_exec::GitCmd::at(repo)
        .args(args.iter().copied())
        .timeout(timeout)
        .run_blocking()?;
    if !out.ok() {
        return Err(AppError::Command(out.stderr));
    }
    Ok(out.text())
}

// ---------------------------------------------------------------------------
// Commit graph lane assignment (pure — unit tested)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub struct GraphRow {
    pub lane: usize,
    pub parent_lanes: Vec<usize>,
    pub active: Vec<usize>,
    /// A lane was waiting for this commit: its line comes from above.
    pub from_above: bool,
}

/// Assign each commit a column, the way `git log --graph` does.
///
/// A "lane" is a rail waiting for a particular commit. Walking newest→oldest:
/// a commit takes the lane that was waiting for it (or opens a new one), then
/// hands that lane to its FIRST parent; extra parents (i.e. a merge) either
/// join an existing rail or open their own — which is exactly what makes a
/// merge visible as two lines converging.
pub fn assign_lanes(commits: &[(String, Vec<String>)]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());

    for (hash, parents) in commits {
        let waiting = lanes.iter().position(|l| l.as_deref() == Some(hash.as_str()));
        let from_above = waiting.is_some();
        let lane = match waiting {
            Some(i) => i,
            None => match lanes.iter().position(|l| l.is_none()) {
                Some(i) => {
                    lanes[i] = Some(hash.clone());
                    i
                }
                None => {
                    lanes.push(Some(hash.clone()));
                    lanes.len() - 1
                }
            },
        };

        // Rails visible on this row, captured before we advance them.
        let active: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_some())
            .map(|(i, _)| i)
            .collect();

        // Any other rail waiting for this same commit converges here.
        for (i, l) in lanes.iter_mut().enumerate() {
            if i != lane && l.as_deref() == Some(hash.as_str()) {
                *l = None;
            }
        }

        // Free our lane, then hand it (or a new one) to each parent.
        lanes[lane] = None;
        let mut parent_lanes = Vec::with_capacity(parents.len());
        for (idx, p) in parents.iter().enumerate() {
            if let Some(i) = lanes.iter().position(|l| l.as_deref() == Some(p.as_str())) {
                parent_lanes.push(i);
            } else if idx == 0 {
                lanes[lane] = Some(p.clone());
                parent_lanes.push(lane);
            } else {
                let i = match lanes.iter().position(|l| l.is_none()) {
                    Some(i) => {
                        lanes[i] = Some(p.clone());
                        i
                    }
                    None => {
                        lanes.push(Some(p.clone()));
                        lanes.len() - 1
                    }
                };
                parent_lanes.push(i);
            }
        }

        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }

        rows.push(GraphRow {
            lane,
            parent_lanes,
            active,
            from_above,
        });
    }
    rows
}

/// The parents the graph draws edges to: all of them, or — in a list filtered
/// by people or words, where git rewrites no parents — only those the list
/// shows, as `git log --graph` does. Lanes waiting for commits that are never
/// listed would otherwise pile up, one more per row.
pub fn graph_parents(parents: &[String], members: Option<&std::collections::HashSet<String>>) -> Vec<String> {
    match members {
        Some(m) => parents.iter().filter(|p| m.contains(p.as_str())).cloned().collect(),
        None => parents.to_vec(),
    }
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Every repo inside the profile folders, so the picker needs no typing — and
/// every profile folder that holds none (a project git doesn't know yet, or a
/// folder inside a repository), so a folder added to a profile never just
/// vanishes. Those are `RepoKind::Folder`; whoever needs repositories only
/// leaves them out.
pub async fn list_repos() -> Result<Vec<RepoRef>, AppError> {
    let store = profiles::load_profiles()?;
    let mut out: Vec<RepoRef> = Vec::new();
    for p in &store.profiles {
        for dir in &p.directories {
            if !PathBuf::from(dir).is_dir() {
                continue;
            }
            let found = repo_scan::scan_repos(dir.clone()).await.unwrap_or_default();
            if found.is_empty() && !out.iter().any(|e| e.path == *dir) {
                let inside = match repo_scan::folder_state(dir.clone()).await {
                    Ok(repo_scan::FolderState::NoRepo) => None,
                    Ok(repo_scan::FolderState::InsideRepo { top }) => Some(top),
                    // Missing by now, or holding a clone still being written.
                    _ => continue,
                };
                let owner = crate::commit_audit::owning_profile(dir, &store.profiles).unwrap_or(p);
                out.push(RepoRef {
                    name: crate::paths::base_name(dir),
                    path: dir.clone(),
                    profile_id: owner.id.clone(),
                    profile_name: owner.name.clone(),
                    profile_email: owner.git_email.clone(),
                    kind: RepoKind::Folder,
                    inside,
                });
                continue;
            }
            for r in found {
                if out.iter().any(|e| e.path == r.path) {
                    continue;
                }
                // Whose it is: the deepest profile folder that holds it — the
                // rule the generated gitconfig follows — not whichever
                // profile's folder happened to be scanned first.
                let owner = crate::commit_audit::owning_profile(&r.path, &store.profiles).unwrap_or(p);
                let (profile_id, profile_name, profile_email) = (owner.id.clone(), owner.name.clone(), owner.git_email.clone());
                out.push(RepoRef {
                    name: crate::paths::base_name(&r.path),
                    path: r.path,
                    profile_id,
                    profile_name,
                    profile_email,
                    kind: RepoKind::Repo,
                    inside: None,
                });
            }
        }
    }
    out.sort_by_key(|r| r.name.to_lowercase());
    Ok(out)
}

/// Where HEAD is, in three facts. The History page only needs these; it used
/// to ask for the Changes page's whole status (seventeen git commands, some
/// proportional to the size of the repository) to learn them.
#[derive(Debug, Serialize, Clone)]
pub struct HeadInfo {
    pub branch: Option<String>,
    pub detached: bool,
    pub unborn: bool,
}

pub fn head_info(repo_path: &str) -> Result<HeadInfo, AppError> {
    let repo = PathBuf::from(repo_path);
    // Unborn: HEAD names a branch that has no commit yet, so symbolic-ref
    // answers but rev-parse does not. Detached: the other way round.
    let branch = git(&repo, &["symbolic-ref", "-q", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let has_commit = git(&repo, &["rev-parse", "-q", "--verify", "HEAD^{commit}"]).is_ok();
    Ok(HeadInfo { detached: branch.is_none() && has_commit, unborn: !has_commit, branch })
}

/// `%(upstream:track,nobracket)`: "ahead 1, behind 2", "ahead 1", "behind 2",
/// "" (level), or "gone" (the upstream no longer exists).
pub fn parse_track(s: &str) -> (usize, usize) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in s.split(',') {
        let mut it = part.split_whitespace();
        match (it.next(), it.next().and_then(|n| n.parse::<usize>().ok())) {
            (Some("ahead"), Some(n)) => ahead = n,
            (Some("behind"), Some(n)) => behind = n,
            _ => {}
        }
    }
    (ahead, behind)
}

pub fn list_branches(repo_path: &str) -> Result<Vec<BranchInfo>, AppError> {
    let repo = PathBuf::from(repo_path);
    let fmt = format!(
        "%(refname){US}%(refname:short){US}%(upstream:short){US}%(objectname){US}%(objectname:short){US}%(authorname){US}%(committerdate:iso8601){US}%(HEAD){US}%(upstream:track,nobracket){US}%(subject)",
        US = US
    );
    let raw = git(
        &repo,
        &["for-each-ref", "--sort=-committerdate", "--format", &fmt, "refs/heads", "refs/remotes"],
    )?;

    // Only needed to name a remote branch on its remote; read once, and only
    // when there is a remote-tracking ref to name.
    let remotes: Vec<String> = if raw.lines().any(|l| l.starts_with("refs/remotes/")) {
        git(&repo, &["remote"]).map(|r| r.lines().map(str::to_string).collect()).unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut branches = Vec::new();
    for line in raw.lines() {
        // subject is last so any separator inside it can't shift the other fields
        let f: Vec<&str> = line.splitn(10, US).collect();
        if f.len() < 10 {
            continue;
        }
        let full_ref = f[0];
        let name = f[1].to_string();
        if is_remote_head(full_ref) {
            continue;
        }
        let upstream = if f[2].is_empty() { None } else { Some(f[2].to_string()) };
        let is_remote = full_ref.starts_with("refs/remotes/");

        // ahead/behind only makes sense for a local branch with an upstream.
        // for-each-ref computes it in the same walk (`%(upstream:track)`);
        // this used to be one `rev-list --count` process per branch, which on
        // a repository with two hundred branches was two hundred processes
        // every time the History page opened.
        let (ahead, behind) = if is_remote || upstream.is_none() { (0, 0) } else { parse_track(f[8]) };

        branches.push(BranchInfo {
            is_current: f[7].trim() == "*",
            is_backup: !is_remote && is_safety_branch(&name),
            is_remote,
            name,
            upstream,
            ahead,
            behind,
            tip: f[3].to_string(),
            short_tip: f[4].to_string(),
            last_author: f[5].to_string(),
            last_date: f[6].to_string(),
            last_subject: f[9].to_string(),
            remote_branch: if is_remote { split_remote(full_ref, &remotes) } else { None },
            full_ref: full_ref.to_string(),
        });
    }
    Ok(branches)
}

fn parse_log(raw: &str) -> Vec<(String, Vec<String>, Vec<String>)> {
    // (hash, parents, remaining fields)
    raw.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(9, US).collect();
            if f.len() < 9 {
                return None;
            }
            let parents = f[2]
                .split_whitespace()
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            Some((
                f[0].to_string(),
                parents,
                f.iter().map(|s| s.to_string()).collect(),
            ))
        })
        .collect()
}

/// One person who has committed to the viewed revision, with how often.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AuthorCount {
    pub name: String,
    pub email: String,
    pub commits: usize,
}

/// `git shortlog -sne`: `   12\tName <email>` per line, most frequent first.
pub fn parse_shortlog(out: &str) -> Vec<AuthorCount> {
    out.lines()
        .filter_map(|line| {
            let (count, who) = line.trim().split_once('\t')?;
            let commits: usize = count.trim().parse().ok()?;
            let (name, email) = match (who.rfind(" <"), who.ends_with('>')) {
                (Some(i), true) => (who[..i].trim().to_string(), who[i + 2..who.len() - 1].trim().to_string()),
                _ => (who.trim().to_string(), String::new()),
            };
            Some(AuthorCount { name, email, commits })
        })
        .collect()
}

/// The names GitSwitch gives its own backup branches: before an update, a
/// reset, a commit-audit rewrite.
fn safety_prefixes() -> impl Iterator<Item = &'static str> {
    crate::sync::BACKUP_PREFIXES.iter().copied().chain([crate::tree::RESET_BACKUP_PREFIX, crate::states::STATE_BACKUP_PREFIX, "gitswitch-backup-"])
}

/// One of GitSwitch's own backup branches — a safety net, not anyone's work.
pub(crate) fn is_safety_branch(name: &str) -> bool {
    safety_prefixes().any(|p| name.starts_with(p))
}

/// What "All branches" walks: every local branch, remote-tracking branch and
/// tag, and HEAD (a detached checkout, a submodule). Not `--all`, which also
/// walks the stash, notes, the refs a preview parks and git's prefetch refs —
/// and not GitSwitch's own backup branches: none of them is work anyone
/// pushes, and listed, each was marked "not pushed" for good. A backup still
/// opens when it is chosen by name. `--exclude` applies to the `--branches`
/// right after it only.
fn all_branches_revs(repo: &Path) -> Vec<String> {
    let mut revs: Vec<String> = safety_prefixes().map(|p| format!("--exclude={}*", p)).collect();
    revs.extend(["--branches", "--remotes", "--tags"].map(String::from));
    if git(repo, &["rev-parse", "-q", "--verify", "HEAD^{commit}"]).is_ok() {
        revs.push("HEAD".into());
    }
    revs
}

/// Who commits on the viewed revision (or on all branches, for "--all"), most
/// frequent first — the list behind the History page's people filter. Capped:
/// a long-lived repository can have thousands of one-off authors.
pub fn authors(repo_path: &str, rev: &str) -> Result<Vec<AuthorCount>, AppError> {
    let repo = PathBuf::from(repo_path);
    if rev != "--all" && rev.starts_with('-') {
        return Err(AppError::Config(format!("Invalid revision: {}", rev)));
    }
    let mut args: Vec<String> = vec!["shortlog".into(), "-sne".into()];
    if rev == "--all" {
        args.extend(all_branches_revs(&repo));
    } else {
        args.push(rev.to_string());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let raw = git_with(&repo, &refs, walk_clock())?;
    let mut list = parse_shortlog(&raw);
    list.truncate(300);
    Ok(list)
}

/// Every gitlink path in the index — the submodules, mapped or not.
fn gitlink_paths(repo: &Path) -> Vec<String> {
    git(repo, &["ls-files", "-s", "-z"])
        .map(|out| {
            out.split('\0')
                .filter(|rec| rec.starts_with("160000 "))
                .filter_map(|rec| rec.split_once('\t').map(|(_, p)| p.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The files each listed commit touches — one git call for the page, so a
/// commit can be told apart from a submodule-pointer bump.
fn files_per_commit(repo: &Path, hashes: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    if hashes.is_empty() {
        return out;
    }
    // -z: names as they are (no quoting of non-ASCII paths). Gitlink changes
    // are listed whatever `submodule.<name>.ignore` or `diff.ignoreSubmodules`
    // says, and a merge lists nothing — its combined diff is not what it
    // "changed".
    // --no-show-signature: with `log.showSignature` set, the verification
    // text would land among the names.
    let mut args: Vec<&str> = vec!["show", "--no-show-signature", "--name-only", "-z", "--format=%x1e%H", "--no-renames", "--ignore-submodules=none", "--no-diff-merges"];
    args.extend(hashes.iter().map(String::as_str));
    let Ok(text) = git(repo, &args) else { return out };
    out.extend(parse_name_only(&text));
    out
}

/// `--name-only -z --format=%x1e%H` output, from `show` or `log`: one record
/// per commit — the hash and a NUL, then, when it touches anything, a
/// newline and its files, each ended by a NUL.
pub(crate) fn parse_name_only(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for chunk in text.split('\x1e') {
        let mut fields = chunk.split('\0');
        let Some(hash) = fields.next().map(str::trim).filter(|h| !h.is_empty()) else { continue };
        let files = fields.map(|f| f.strip_prefix('\n').unwrap_or(f)).filter(|f| !f.is_empty()).map(String::from).collect();
        out.insert(hash.to_string(), files);
    }
    out
}

/// A pathspec that leaves out exactly this path — the gitlink entry — and
/// nothing under it. A plain or literal pathspec is a prefix match: it would
/// also hide every commit that touched files there while the path was still
/// an ordinary folder, earlier in history or on another branch. A pattern
/// with a wildcard is matched against the whole path instead, so one
/// character is written as a one-character class (or escaped, when there is
/// no plain one to use).
/// `s` as a basic regular expression that matches exactly that text. Only
/// the characters that mean something in a BRE are escaped; "+", "?", "(",
/// "{" and "|" are literal there already.
fn bre_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        if matches!(ch, '\\' | '.' | '*' | '[' | '^' | '$') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn exact_exclude(path: &str) -> String {
    let mut out = String::from(":(exclude,glob)");
    let mut classed = false;
    let any_plain = path.chars().any(|c| c.is_ascii_alphanumeric());
    for (i, c) in path.chars().enumerate() {
        match c {
            '\\' | '*' | '?' | '[' => {
                out.push('\\');
                out.push(c);
            }
            c if !classed && c.is_ascii_alphanumeric() => {
                out.push('[');
                out.push(c);
                out.push(']');
                classed = true;
            }
            c if !any_plain && i == 0 => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// Which of a commit's files are submodule pointers, and whether that is all
/// it changes. A commit that touches nothing (a merge, an empty commit) is
/// not a bump: it would otherwise vanish behind the "pointer-only" toggle.
fn classify_touches(files: &[String], gitlinks: &[String]) -> (Vec<String>, bool) {
    let touched: Vec<String> = files.iter().filter(|p| gitlinks.contains(p)).cloned().collect();
    let only = !files.is_empty() && touched.len() == files.len();
    (touched, only)
}

/// One page of history for a branch (or all branches when `rev` is "--all":
/// see `all_branches_revs`).
/// `authors`: only commits by any of these (emails or names, literal);
/// `hide_pointer_only`: leave out commits that change nothing but submodule
/// pointers (the CI bumps a superproject's history fills up with);
/// `unpushed_only`: only commits no remote-tracking branch has.
#[allow(clippy::too_many_arguments)]
pub fn history_page(
    repo_path: &str,
    rev: &str,
    offset: usize,
    limit: usize,
    search: Option<&str>,
    authors: Option<&[String]>,
    hide_pointer_only: bool,
    unpushed_only: bool,
) -> Result<HistoryPage, AppError> {
    let repo = PathBuf::from(repo_path);
    // Ranges (main..origin/main) are legitimate revisions; only flags are not.
    if rev != "--all" && rev.starts_with('-') {
        return Err(AppError::Config(format!("Invalid revision: {}", rev)));
    }
    let fmt = format!(
        "%H{US}%h{US}%P{US}%aN{US}%aE{US}%cI{US}%D{US}%s{US}%aI",
        US = US
    );
    let gitlinks = gitlink_paths(&repo);

    // Counting an author filter has to be done by `log`: it matches authors
    // through .mailmap, as the People list (shortlog) and the rows do, and
    // `rev-list` cannot — the two would disagree on the total.
    let by_author = authors.is_some_and(|a| a.iter().any(|s| !s.trim().is_empty()));
    // --use-mailmap on the command line: the people list is always mapped,
    // and `log.mailmap=false` in someone's config must not make the rows and
    // the count disagree with it.
    // Filtered by people or words, the ids themselves are read (not a count):
    // the graph then joins only the commits the list shows.
    // Not-pushed-only and ranges hide commits too: their parents may never be
    // listed either (git ref names cannot contain "..", so it means a range).
    let filtered = by_author || search.is_some_and(|s| !s.trim().is_empty()) || unpushed_only || rev.contains("..");
    let mut count_args: Vec<String> = if by_author {
        vec!["log".into(), "--use-mailmap".into(), "--no-show-signature".into(), "--format=%H".into()]
    } else if filtered {
        vec!["rev-list".into()]
    } else {
        vec!["rev-list".into(), "--count".into()]
    };
    // Everything above the page as well as the page: a page's lanes carry on
    // from the commits above it, so it is laid out with them, not on its own.
    let mut log_args: Vec<String> = vec![
        "log".into(),
        "--use-mailmap".into(),
        "--no-show-signature".into(),
        format!("-n{}", offset.saturating_add(limit)),
        "--date-order".into(),
        format!("--format={}", fmt),
    ];
    // Limiting patterns are matched as FIXED STRINGS: emails and names routinely
    // contain regex metacharacters (`.`, `+`, `[`), and a noreply address like
    // `1234+user@users.noreply.github.com` must match literally.
    let mut limits: Vec<String> = Vec::new();
    if let Some(q) = search.filter(|s| !s.trim().is_empty()) {
        limits.push(format!("--grep={}", bre_literal(q.trim())));
    }
    // Several --author patterns are ORed by git; capped so the argument list
    // stays well under Windows' limit. Every pattern is the text itself (a
    // "+" or "[bot]" in an address is literal): an address is wrapped in its
    // angle brackets so me@x.test does not also match someone@x.testing; a
    // whole identity ("Sam <>", for a commit without an address) is anchored
    // at both ends, so it does not also mean "Uncle Sam <>".
    for a in authors.unwrap_or(&[]).iter().map(|s| s.trim()).filter(|s| !s.is_empty()).take(200) {
        if a.starts_with('<') && a.ends_with('>') {
            // An address in its brackets, whatever the name before it.
            limits.push(format!("--author={}", bre_literal(a)));
        } else if a.contains('<') {
            limits.push(format!("--author=^{}$", bre_literal(a)));
        } else if a.contains('@') {
            limits.push(format!("--author=<{}>", bre_literal(a)));
        } else {
            limits.push(format!("--author={}", bre_literal(a)));
        }
    }
    if !limits.is_empty() {
        // Basic regular expressions by name, whatever grep.patternType says.
        for a in ["--basic-regexp".to_string(), "--regexp-ignore-case".to_string()] {
            count_args.push(a.clone());
            log_args.push(a);
        }
        for a in limits {
            count_args.push(a.clone());
            log_args.push(a);
        }
    }
    if rev == "--all" {
        let all = all_branches_revs(&repo);
        count_args.extend(all.iter().cloned());
        log_args.extend(all);
    } else {
        count_args.push(rev.to_string());
        log_args.push(rev.to_string());
    }
    // Only what no remote-tracking branch has: `--not` turns every ref that
    // `--remotes` adds into an exclusion.
    if unpushed_only {
        for args in [&mut count_args, &mut log_args] {
            args.push("--not".into());
            args.push("--remotes".into());
        }
    }
    // Everything but the gitlinks: a commit that touches only pointers has
    // nothing left and drops out of the list (and the count). Full history
    // with merge simplification keeps every commit that did real work and
    // rewrites parents past the hidden ones, so the graph stays connected
    // instead of running lanes off to commits that are not shown.
    if hide_pointer_only && !gitlinks.is_empty() {
        for args in [&mut count_args, &mut log_args] {
            args.push("--full-history".into());
            args.push("--simplify-merges".into());
            args.push("--".into());
            args.push(".".into());
            for p in &gitlinks {
                args.push(exact_exclude(p));
            }
        }
    }

    let count_ref: Vec<&str> = count_args.iter().map(String::as_str).collect();
    // A count that fails is read as nothing (a repository without commits),
    // but a count stopped on its clock is not "no commits": reading it as 0
    // emptied the graph's members and ended paging after the first page.
    let counted = match git_with(&repo, &count_ref, walk_clock()) {
        Ok(s) => s,
        Err(e) if crate::git_exec::was_stopped(&e) => return Err(e),
        Err(_) => String::new(),
    };
    let is_oid = |l: &str| (l.len() == 40 || l.len() == 64) && l.chars().all(|c| c.is_ascii_hexdigit());
    let members: Option<std::collections::HashSet<String>> = filtered.then(|| counted.lines().map(str::trim).filter(|l| is_oid(l)).map(str::to_string).collect());
    let total: usize = match &members {
        Some(m) => m.len(),
        None => counted.trim().parse().unwrap_or(0),
    };

    let log_ref: Vec<&str> = log_args.iter().map(String::as_str).collect();
    // A repo with no commits yet makes `git log` exit non-zero; that's an empty
    // history, not an error worth showing the user.
    // (With the unpushed filter a failure is a failure: an empty list would
    // claim everything is pushed.)
    // (Nor is a walk stopped on its clock: that is "this took too long".)
    let raw = match git_with(&repo, &log_ref, walk_clock()) {
        Ok(r) => r,
        Err(e) if crate::git_exec::was_stopped(&e) => return Err(e),
        Err(_) if total == 0 && !unpushed_only => String::new(),
        Err(e) => return Err(e),
    };
    let parsed = parse_log(&raw);

    let graph_input: Vec<(String, Vec<String>)> = parsed
        .iter()
        .map(|(h, p, _)| (h.clone(), graph_parents(p, members.as_ref())))
        .collect();
    let rows = assign_lanes(&graph_input);

    let mut commits = Vec::with_capacity(parsed.len().saturating_sub(offset));
    let mut max_lane = 0usize;
    for (i, (hash, parents, f)) in parsed.into_iter().enumerate().skip(offset) {
        let row = &rows[i];
        max_lane = max_lane
            .max(row.active.iter().copied().max().unwrap_or(0))
            .max(row.parent_lanes.iter().copied().max().unwrap_or(0));
        let refs: Vec<String> = f[6]
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_start_matches("HEAD -> ").to_string())
            .collect();
        commits.push(HistoryCommit {
            short: f[1].clone(),
            is_merge: parents.len() > 1,
            author_name: f[3].clone(),
            author_email: f[4].clone(),
            date: f[5].clone(),
            subject: f[7].clone(),
            author_date: f[8].clone(),
            refs,
            hash,
            parents,
            lane: row.lane,
            parent_lanes: row.parent_lanes.clone(),
            active_lanes: row.active.clone(),
            from_above: row.from_above,
            touches_submodules: Vec::new(),
            only_submodules: false,
            unpushed: false,
        });
    }
    if !gitlinks.is_empty() {
        let hashes: Vec<String> = commits.iter().map(|c| c.hash.clone()).collect();
        let files = files_per_commit(&repo, &hashes);
        for c in commits.iter_mut() {
            if let Some(f) = files.get(&c.hash) {
                (c.touches_submodules, c.only_submodules) = classify_touches(f, &gitlinks);
            }
        }
    }
    let remote_refs = has_remote_refs(&repo);
    let mut marks_error = None;
    if remote_refs {
        if unpushed_only {
            for c in commits.iter_mut() {
                c.unpushed = true;
            }
        } else {
            let hashes: Vec<String> = commits.iter().map(|c| c.hash.clone()).collect();
            match unpushed_among(&repo, &hashes) {
                Ok(unpushed) => {
                    for c in commits.iter_mut() {
                        c.unpushed = unpushed.contains(&c.hash);
                    }
                }
                Err(e) => marks_error = Some(e),
            }
        }
    }

    Ok(HistoryPage {
        has_more: offset + commits.len() < total,
        total,
        offset,
        limit,
        max_lane,
        commits,
        remote_refs,
        marks_error,
    })
}

/// At least one remote-tracking branch exists — something a commit can have
/// been pushed to.
fn has_remote_refs(repo: &Path) -> bool {
    git(repo, &["for-each-ref", "--count=1", "--format=%(refname)", "refs/remotes/"]).map(|o| !o.trim().is_empty()).unwrap_or(false)
}

/// Which of these commits no remote-tracking branch has. `--no-walk` lists
/// only the commits named, while `--not --remotes` still excludes everything
/// a remote-tracking branch reaches — so the answer costs one walk of the
/// remote side down to the oldest commit asked about, never a walk of the
/// whole history. Two hundred at a time, for Windows' argument limit.
fn unpushed_among(repo: &Path, hashes: &[String]) -> Result<std::collections::HashSet<String>, String> {
    let mut out = std::collections::HashSet::new();
    for chunk in hashes.chunks(200) {
        let mut args: Vec<&str> = vec!["rev-list", "--no-walk=unsorted"];
        args.extend(chunk.iter().map(String::as_str));
        args.extend(["--not", "--remotes"]);
        // A failure (a remote-tracking ref at a missing object, say) is not
        // "everything is pushed": the caller says it could not tell.
        let o = git(repo, &args).map_err(|e| e.to_string())?;
        out.extend(o.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()));
    }
    Ok(out)
}

pub fn commit_detail(repo_path: &str, hash: &str) -> Result<CommitDetail, AppError> {
    let repo = PathBuf::from(repo_path);
    let fmt = format!(
        "%H{US}%h{US}%an{US}%ae{US}%aI{US}%cn{US}%ce{US}%P{US}%D{US}%G?{US}%s{US}%b",
        US = US
    );
    let raw = git(&repo, &["show", "-s", &format!("--format={}", fmt), "--end-of-options", hash])?;
    let f: Vec<&str> = raw.splitn(12, US).collect();
    if f.len() < 12 {
        return Err(AppError::NotFound(format!("Commit {} not found", hash)));
    }

    // --numstat gives machine-readable per-file add/remove counts. Without
    // rename detection: a rename would otherwise print `dir/{old => new}`,
    // which is not a path the file diff can be asked for; as a deletion plus
    // an addition both rows carry real paths.
    let stat = git(&repo, &["show", "--numstat", "--no-renames", "--format=", "--end-of-options", hash]).unwrap_or_default();
    let files = stat
        .lines()
        .filter_map(|l| {
            let mut p = l.split('\t');
            match (p.next(), p.next(), p.next()) {
                (Some(a), Some(r), Some(path)) => Some(FileChange {
                    added: a.to_string(),
                    removed: r.to_string(),
                    path: path.to_string(),
                }),
                _ => None,
            }
        })
        .collect();

    let signature = match f[9] {
        "G" => "Good signature",
        "U" => "Good signature (untrusted)",
        "B" => "BAD signature",
        "X" | "Y" | "R" => "Signature expired or revoked",
        "E" => "Signature could not be checked",
        _ => "",
    }
    .to_string();

    Ok(CommitDetail {
        hash: f[0].to_string(),
        short: f[1].to_string(),
        author_name: f[2].to_string(),
        author_email: f[3].to_string(),
        author_date: f[4].to_string(),
        committer_name: f[5].to_string(),
        committer_email: f[6].to_string(),
        parents: f[7].split_whitespace().map(|s| s.to_string()).collect(),
        refs: f[8]
            .split(',')
            .map(|s| s.trim().trim_start_matches("HEAD -> ").to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        subject: f[10].to_string(),
        body: f[11].trim().to_string(),
        files,
        signature,
    })
}

/// How a branch relates to the rest: which branches already contain it, and the
/// merge commits along its own history.
pub fn branch_merge_info(repo_path: &str, branch: &str) -> Result<MergeInfo, AppError> {
    let repo = PathBuf::from(repo_path);
    let tip = git(&repo, &["rev-parse", "--verify", "--end-of-options", branch])?;

    // Full ref names, so each remote's symbolic HEAD (short name: just
    // "origin") can be recognised and left out.
    let listed = git(
        &repo,
        &["for-each-ref", &format!("--contains={}", tip), "--format=%(refname) %(objectname)", "refs/heads", "refs/remotes"],
    )
    .unwrap_or_default();
    // (full ref, short name, its commit)
    let containing: Vec<(String, String, String)> = listed
        .lines()
        .filter_map(|l| l.trim().rsplit_once(' '))
        .filter(|(r, _)| !r.is_empty() && !is_remote_head(r))
        // A backup branch contains what it backed up: it says nothing about where the work went.
        .filter(|(r, _)| !r.strip_prefix("refs/heads/").is_some_and(is_safety_branch))
        .map(|(r, o)| (r.to_string(), r.trim_start_matches("refs/heads/").trim_start_matches("refs/remotes/").to_string(), o.to_string()))
        .filter(|(_, s, _)| s != branch)
        .collect();
    let merged_into: Vec<String> = containing.iter().map(|(_, s, _)| s.clone()).collect();

    // How it got into the first few: local branches first, each target commit once.
    let mut order: Vec<&(String, String, String)> = containing.iter().collect();
    order.sort_by_key(|(r, _, _)| !r.starts_with("refs/heads/"));
    let mut seen = std::collections::HashSet::new();
    let mut merged_by_list = Vec::new();
    for (full, short, oid) in order {
        if merged_by_list.len() >= 5 || !seen.insert(oid.clone()) {
            continue;
        }
        if let Some((how, commit)) = merged_by(&repo, tip.trim(), full) {
            merged_by_list.push(MergedBy { branch: short.clone(), how, commit });
        }
    }

    // The merges in the branch's history, newest first: fifty, and how many there are.
    let fmt = format!("%H{US}%h{US}%P{US}%aN{US}%aE{US}%cI{US}%D{US}%s{US}%aI", US = US);
    // A walk stopped on its clock is an error, not "no merges".
    let raw = match git_with(&repo, &["log", "--merges", "--no-show-signature", "--date-order", "-n51", &format!("--format={}", fmt), "--end-of-options", branch], walk_clock()) {
        Ok(r) => r,
        Err(e) if crate::git_exec::was_stopped(&e) => return Err(e),
        Err(_) => String::new(),
    };
    let mut merges: Vec<HistoryCommit> = parse_log(&raw)
        .into_iter()
        .map(|(hash, parents, f)| HistoryCommit {
            short: f[1].clone(),
            is_merge: true,
            author_name: f[3].clone(),
            author_email: f[4].clone(),
            date: f[5].clone(),
            subject: f[7].clone(),
            author_date: f[8].clone(),
            refs: Vec::new(),
            hash,
            parents,
            lane: 0,
            parent_lanes: Vec::new(),
            active_lanes: Vec::new(),
            from_above: false,
            touches_submodules: Vec::new(),
            only_submodules: false,
            unpushed: false,
        })
        .collect();
    let merges_truncated = merges.len() > 50;
    merges.truncate(50);
    let merges_total = match git_with(&repo, &["rev-list", "--count", "--merges", "--end-of-options", branch], walk_clock()) {
        Ok(s) => s.trim().parse().unwrap_or(merges.len()),
        Err(e) if crate::git_exec::was_stopped(&e) => return Err(e),
        Err(_) => merges.len(),
    };

    Ok(MergeInfo {
        merged_into,
        merges,
        merged_by: merged_by_list,
        merges_total,
        merges_truncated,
    })
}

fn snapshot_refs(repo: &Path, space: &str) -> String {
    git(repo, &["for-each-ref", "--format=%(refname) %(objectname)", space]).unwrap_or_default()
}

/// Refresh remote-tracking refs. This is `fetch`, never `pull`, and it is
/// deliberately conservative:
///
/// * no refspec is passed, so git can only write under `refs/remotes/*`
///   (a refspec ending in `refs/heads/*` is the ONLY way fetch can move a
///   local branch, and we never supply one);
/// * no `--prune` — deleting remote-tracking refs isn't needed to answer
///   "am I behind?", so we don't do it;
/// * nothing merges, rebases or checks out, so the working tree cannot change.
///
/// Rather than just asserting that, the state is snapshotted before and after
/// and the comparison is returned to the caller as evidence.
pub fn fetch_repo(repo_path: &str) -> Result<FetchResult, AppError> {
    let repo = PathBuf::from(repo_path);
    let remotes: Vec<String> = git(&repo, &["remote"])?.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if remotes.is_empty() {
        return Err(AppError::Config("This repo has no remote to fetch from.".into()));
    }
    // This fetch runs on its own when "auto fetch" is on, so it is a passive
    // command (`GitCmd::history_fetch`): refused, once and naming the keys,
    // when the repository's own config names a program a fetch would run.
    // Called from a blocking thread, hence `block_on`.
    let rt = tokio::runtime::Handle::current();
    rt.block_on(crate::git_exec::check_history_fetch(&repo))?;

    let head_before = git(&repo, &["rev-parse", "HEAD"]).unwrap_or_default();
    let worktree_before = git(&repo, &["status", "--porcelain"]).unwrap_or_default();
    let locals_before = snapshot_refs(&repo, "refs/heads");
    let remotes_before = snapshot_refs(&repo, "refs/remotes");

    // Each remote on its own, through the shared runner (no credential prompt
    // can hang it) with git's progress on: only a long SILENCE stops it —
    // never the size of what arrives, and never nothing at all (it used to
    // wait for ever on a server that stopped answering). Not `--all`: with
    // two remotes or more git hands each to a child fetch that is not given
    // `--progress`, and into a pipe that child says nothing while it
    // downloads. The remotes `--all` would skip (`skipFetchAll`) are skipped.
    // No submodule recursion, as everywhere else in the app. Called from a
    // blocking thread, hence `block_on`.
    let mut failures: Vec<String> = Vec::new();
    for remote in &remotes {
        if git(&repo, &["config", "--bool", "--get", &format!("remote.{}.skipFetchAll", remote)]).map(|v| v.trim() == "true").unwrap_or(false) {
            continue;
        }
        if crate::git_exec::validate_remote_name(remote).is_err() {
            failures.push(format!("{}: not a remote name git can be given safely; fetch it in a terminal.", remote));
            continue;
        }
        let fetched = rt.block_on(
            crate::git_exec::GitCmd::at(&repo)
                .history_fetch()
                .for_fetch()
                .args(["fetch", "--progress", "--no-recurse-submodules", "--", remote])
                .run(),
        );
        match fetched {
            Ok(out) if out.ok() => {}
            Ok(out) => {
                // In the fetch's own words (not the Clone page's), with the
                // remote's address so a certificate organisation is recognised.
                let url = git(&repo, &["remote", "get-url", "--", remote]).unwrap_or_default();
                let ctx = crate::git_advice::AdviceCtx { url: Some(url.trim()), remote: Some(remote), ..Default::default() };
                let a = crate::git_advice::explain(crate::git_advice::GitOp::Fetch, &out.stderr, &ctx);
                let guidance = if a.guidance.is_empty() { String::new() } else { format!(" {}", a.guidance) };
                failures.push(format!("{}: {}{}\n\ngit said: {}", remote, a.headline, guidance, a.git_said));
            }
            Err(e) => failures.push(format!("{}: {}", remote, e)),
        }
    }
    rt.block_on(crate::git_exec::history_housekeeping(&repo));
    if !failures.is_empty() {
        return Err(AppError::Command(failures.join("\n\n")));
    }

    let head_after = git(&repo, &["rev-parse", "HEAD"]).unwrap_or_default();
    let worktree_after = git(&repo, &["status", "--porcelain"]).unwrap_or_default();
    let locals_after = snapshot_refs(&repo, "refs/heads");
    let remotes_after = snapshot_refs(&repo, "refs/remotes");

    let before: Vec<&str> = remotes_before.lines().collect();
    let updated = remotes_after
        .lines()
        .filter(|l| !before.contains(l))
        .count();

    let result = FetchResult {
        head_unchanged: head_before == head_after,
        worktree_unchanged: worktree_before == worktree_after,
        local_branches_unchanged: locals_before == locals_after,
        updated_remote_refs: updated,
        message: if updated == 0 {
            "Fetched — no new commits on the remote".into()
        } else {
            format!(
                "Fetched — {} remote branch{} updated. Your branch and working tree were not touched.",
                updated,
                if updated == 1 { "" } else { "es" }
            )
        },
    };

    // Should be unreachable, but if a repo is configured to do something exotic
    // on fetch, say so loudly instead of quietly having changed the user's work.
    if !(result.head_unchanged && result.worktree_unchanged && result.local_branches_unchanged) {
        return Err(AppError::Command(
            "Fetch unexpectedly modified local state — check `git status`.              This repo may have a custom fetch refspec pointing at refs/heads."
                .into(),
        ));
    }
    Ok(result)
}

fn seconds_since_last_fetch(repo: &Path) -> Option<u64> {
    // git rewrites FETCH_HEAD on every fetch, so its mtime is the fetch time.
    // Resolved through git: a submodule's `.git` is a file pointing elsewhere,
    // and the banner used to call such a repository "never fetched".
    let path = git(repo, &["rev-parse", "--path-format=absolute", "--git-path", "FETCH_HEAD"]).ok()?;
    let meta = std::fs::metadata(path.trim()).ok()?;
    let modified = meta.modified().ok()?;
    std::time::SystemTime::now()
        .duration_since(modified)
        .ok()
        .map(|d| d.as_secs())
}

/// How far the branch is from its remote, and how stale that answer is.
pub fn sync_status(repo_path: &str, branch: &str) -> Result<SyncStatus, AppError> {
    let repo = PathBuf::from(repo_path);
    // The "all branches" view has no single branch — describe HEAD's.
    let branch = if branch == "--all" || branch.is_empty() {
        git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|_| "HEAD".into())
    } else {
        branch.to_string()
    };
    // Read from the repository (HEAD can point at `refs/heads/--output=<file>`,
    // which git lets a ref be called) or sent by the page: it starts the
    // arguments below, where a leading `-` would make it an option.
    if branch.starts_with('-') {
        return Err(AppError::Config(format!("Invalid revision: {}", branch)));
    }

    let upstream = git(
        &repo,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", &format!("{}@{{upstream}}", branch)],
    )
    .ok()
    .filter(|s| !s.is_empty());

    let (mut ahead, mut behind) = (0usize, 0usize);
    if let Some(up) = upstream.as_ref() {
        if let Ok(counts) = git(
            &repo,
            &["rev-list", "--left-right", "--count", "--end-of-options", &format!("{}...{}", branch, up)],
        ) {
            let mut it = counts.split_whitespace();
            ahead = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            behind = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        }
    }

    Ok(SyncStatus {
        local_tip: commit_ref(&repo, &branch),
        remote_tip: upstream.as_ref().and_then(|u| commit_ref(&repo, u)),
        incoming_rev: upstream
            .as_ref()
            .filter(|_| behind > 0)
            .map(|up| format!("{}..{}", branch, up)),
        branch,
        upstream,
        ahead,
        behind,
        last_fetch_secs: seconds_since_last_fetch(&repo),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The History page's runner is the one runner.
    #[test]
    fn the_history_runner_goes_through_gitcmd() {
        let env = git(&std::env::temp_dir(), &crate::git_exec::test_support::SHOW_ENV).expect("git runs");
        crate::git_exec::test_support::assert_hardened(&env);
    }

    /// A walk stopped on its clock comes back as an error: never as a page
    /// that says "no commits" (or one that cannot page past the first), nor
    /// as a branch without merges.
    #[test]
    fn a_walk_stopped_on_its_clock_is_an_error_not_an_empty_history() {
        let dir = std::env::temp_dir().join(format!("gitswitch-walk-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");
        git(&dir, &["init", "-q", "-b", "main"]).expect("init");
        let id = ["-c", "user.name=Walk Test", "-c", "user.email=walk@example.test", "-c", "commit.gpgsign=false"];
        for m in ["one", "two"] {
            let mut args = id.to_vec();
            args.extend(["commit", "-q", "--allow-empty", "--no-verify", "-m", m]);
            git(&dir, &args).expect("commit");
        }
        let path = dir.to_string_lossy().to_string();

        // With the real clock the walk answers.
        let page = history_page(&path, "HEAD", 0, 50, None, None, false, false).expect("a page");
        assert_eq!((page.total, page.commits.len()), (2, 2));

        // A clock that has already run out: every walk is stopped.
        WALK_CLOCK.with(|c| c.set(Some(std::time::Duration::ZERO)));
        let page = history_page(&path, "HEAD", 0, 50, None, None, false, false);
        let merges = branch_merge_info(&path, "main");
        let people = authors(&path, "HEAD");
        WALK_CLOCK.with(|c| c.set(None));
        let _ = std::fs::remove_dir_all(&dir);

        let Err(err) = page else { panic!("a stopped walk is not an empty history") };
        assert!(crate::git_exec::was_stopped(&err), "{}", err);
        let Err(err) = merges else { panic!("a stopped walk is not a branch without merges") };
        assert!(crate::git_exec::was_stopped(&err), "{}", err);
        let Err(err) = people else { panic!("a stopped walk is not a repository without authors") };
        assert!(crate::git_exec::was_stopped(&err), "{}", err);
    }

    #[test]
    fn a_tip_is_drawn_as_one_and_a_continuing_line_comes_from_above() {
        let rows = assign_lanes(&[("c".into(), vec!["b".into()]), ("b".into(), vec!["a".into()]), ("a".into(), vec![])]);
        assert_eq!(rows.iter().map(|r| r.from_above).collect::<Vec<_>>(), vec![false, true, true]);
    }

    #[test]
    fn a_filtered_list_joins_only_the_commits_it_shows() {
        // Every other commit is someone else's: unfiltered parents would leave
        // a lane waiting for each of them, one more per row.
        let shown: std::collections::HashSet<String> = ["e", "c", "a"].iter().map(|s| s.to_string()).collect();
        let chain = [("e", "d"), ("c", "b"), ("a", "")];
        let input: Vec<(String, Vec<String>)> = chain.iter().map(|(h, p)| (h.to_string(), graph_parents(&[p.to_string()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>(), Some(&shown)))).collect();
        let rows = assign_lanes(&input);
        assert!(rows.iter().all(|r| r.lane == 0), "{:?}", rows.iter().map(|r| r.lane).collect::<Vec<_>>());
        let raw: Vec<(String, Vec<String>)> = chain.iter().map(|(h, p)| (h.to_string(), [p.to_string()].into_iter().filter(|p| !p.is_empty()).collect())).collect();
        assert!(assign_lanes(&raw).iter().any(|r| r.lane > 0), "without the filter the lanes pile up");
    }

    #[test]
    fn a_log_line_needs_all_nine_fields_and_keeps_the_author_date_last() {
        let line = ["h1", "h", "p1 p2", "Ann", "ann@x", "2026-10-05T10:00:00+02:00", "HEAD -> main", "fix: it", "2026-10-01T09:00:00+02:00"].join("\x1f");
        let parsed = parse_log(&line);
        assert_eq!(parsed.len(), 1);
        let (hash, parents, f) = &parsed[0];
        assert_eq!(hash, "h1");
        assert_eq!(parents, &["p1", "p2"]);
        assert_eq!(f[7], "fix: it");
        assert_eq!(f[8], "2026-10-01T09:00:00+02:00", "the author date rides last");
        // The old eight-field shape is not silently accepted as a commit.
        let eight = ["h1", "h", "", "Ann", "ann@x", "2026-10-05T10:00:00+02:00", "", "fix: it"].join("\x1f");
        assert!(parse_log(&eight).is_empty());
    }

    #[test]
    fn name_only_records_are_split_per_commit_and_a_bump_is_told_from_work() {
        // The bytes git writes with -z: hash, NUL, then (when there are files) a newline and each name with its NUL.
        let text = "\x1eaaa\0\ntrinity\0\x1ebbb\0\nsrc/main.rs\0trinity\0\x1eccc\0\x1eddd\0\nm\u{f3}dulo file.txt\0";
        let files = parse_name_only(text);
        assert_eq!(files["ddd"], vec!["m\u{f3}dulo file.txt".to_string()], "a name is taken as it is, spaces and all");
        assert_eq!(files["aaa"], vec!["trinity".to_string()]);
        assert_eq!(files["bbb"], vec!["src/main.rs".to_string(), "trinity".to_string()]);
        assert!(files["ccc"].is_empty(), "a merge lists no files");
        let gitlinks = vec!["trinity".to_string(), "vendor/sub".to_string()];
        assert_eq!(classify_touches(&files["aaa"], &gitlinks), (vec!["trinity".to_string()], true));
        assert_eq!(classify_touches(&files["bbb"], &gitlinks), (vec!["trinity".to_string()], false));
        assert_eq!(classify_touches(&files["ccc"], &gitlinks), (vec![], false), "nothing touched is not a bump");
        assert_eq!(classify_touches(&["README.md".to_string()], &gitlinks), (vec![], false));
    }

    #[test]
    fn hiding_a_pointer_leaves_out_that_entry_and_nothing_under_it() {
        // One character becomes a class, so git matches the whole path rather than a prefix.
        assert_eq!(bre_literal("1234+user@users.noreply.github.com"), "1234+user@users\\.noreply\\.github\\.com");
        assert_eq!(bre_literal("A.B [bot] <>"), "A\\.B \\[bot] <>");
        assert_eq!(bre_literal("a\\b*^$"), "a\\\\b\\*\\^\\$");
        assert_eq!(exact_exclude("vendor/lib"), ":(exclude,glob)[v]endor/lib");
        // Characters a glob would read are escaped.
        assert_eq!(exact_exclude("libs/my sub[1]*"), ":(exclude,glob)[l]ibs/my sub\\[1]\\*");
        // No plain character to class: the first one is escaped, which makes it a pattern all the same.
        assert_eq!(exact_exclude("\u{65e5}\u{672c}"), ":(exclude,glob)\\\u{65e5}\u{672c}");
    }

    #[test]
    fn a_shortlog_becomes_people_with_counts_in_the_order_git_gave() {
        let list = parse_shortlog("    12\tAnn Example <ann@example.test>\n     3\tBob <bob@x>\n     1\tNo Email\n");
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], AuthorCount { name: "Ann Example".into(), email: "ann@example.test".into(), commits: 12 });
        assert_eq!(list[1].email, "bob@x");
        assert_eq!(list[2], AuthorCount { name: "No Email".into(), email: String::new(), commits: 1 });
        assert!(parse_shortlog("").is_empty());
    }

    #[test]
    fn a_remote_branch_is_named_as_it_is_on_its_remote() {
        let remotes = vec!["origin".to_string(), "a/b".to_string(), "a".to_string()];
        assert_eq!(split_remote("refs/remotes/origin/feature", &remotes).as_deref(), Some("feature"));
        assert_eq!(split_remote("refs/remotes/origin/team/feature", &remotes).as_deref(), Some("team/feature"));
        // A remote whose name holds a slash: the longest remote that prefixes the ref owns it.
        assert_eq!(split_remote("refs/remotes/a/b/x", &remotes).as_deref(), Some("x"));
        assert_eq!(split_remote("refs/remotes/a/c", &remotes).as_deref(), Some("c"));
        // No configured remote owns it, a remote name alone, or a local branch.
        assert_eq!(split_remote("refs/remotes/upstream/x", &remotes), None);
        assert_eq!(split_remote("refs/remotes/origin", &remotes), None);
        assert_eq!(split_remote("refs/remotes/origin/", &remotes), None);
        assert_eq!(split_remote("refs/heads/origin/x", &remotes), None);
    }

    #[test]
    fn upstream_track_is_read_in_every_shape_git_prints() {
        assert_eq!(parse_track("ahead 3, behind 2"), (3, 2));
        assert_eq!(parse_track("ahead 1"), (1, 0));
        assert_eq!(parse_track("behind 7"), (0, 7));
        assert_eq!(parse_track(""), (0, 0));
        assert_eq!(parse_track("gone"), (0, 0));
    }

    fn c(h: &str, parents: &[&str]) -> (String, Vec<String>) {
        (h.into(), parents.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let rows = assign_lanes(&[c("c", &["b"]), c("b", &["a"]), c("a", &[])]);
        assert!(rows.iter().all(|r| r.lane == 0));
        assert_eq!(rows[0].parent_lanes, vec![0]);
        assert_eq!(rows[2].parent_lanes, Vec::<usize>::new(), "root has no parents");
        assert_eq!(rows[2].active, vec![0]);
    }

    #[test]
    fn a_merge_opens_a_second_lane_and_both_rails_are_drawn() {
        // m is a merge of mainline "a" and side branch "s"; s forks from a.
        let rows = assign_lanes(&[
            c("m", &["a", "s"]),
            c("a", &["base"]),
            c("s", &["base"]),
            c("base", &[]),
        ]);
        // The merge sits in lane 0 and points at two different lanes.
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].parent_lanes.len(), 2);
        assert_ne!(
            rows[0].parent_lanes[0], rows[0].parent_lanes[1],
            "a merge must fan out to two lanes"
        );
        // While both sides are pending, two rails are visible.
        assert_eq!(rows[1].active.len(), 2);
        // They converge again at the common ancestor.
        assert_eq!(rows[3].lane, rows[3].active[0]);
        assert_eq!(rows[3].active.len(), 1, "lanes rejoin at the fork point");
    }

    #[test]
    fn lanes_are_reused_after_a_branch_ends() {
        // Two unrelated tips, the first ending immediately.
        let rows = assign_lanes(&[c("x", &[]), c("y", &["z"]), c("z", &[])]);
        assert_eq!(rows[0].lane, 0);
        // x's lane is freed, so y can take lane 0 again.
        assert_eq!(rows[1].lane, 0);
        assert_eq!(rows[2].lane, 0);
    }

    #[test]
    fn octopus_merge_gets_a_lane_per_parent() {
        let rows = assign_lanes(&[c("o", &["p1", "p2", "p3"])]);
        assert_eq!(rows[0].parent_lanes.len(), 3);
        let mut sorted = rows[0].parent_lanes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 3, "each parent needs its own rail");
    }
}
