use crate::error::AppError;
use crate::profiles;
use crate::repo_scan;
use serde::Serialize;
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
}

#[derive(Debug, Serialize, Clone)]
pub struct HistoryCommit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
    pub subject: String,
    pub refs: Vec<String>,
    pub is_merge: bool,
    // graph placement
    pub lane: usize,
    pub parent_lanes: Vec<usize>,
    pub active_lanes: Vec<usize>,
}

#[derive(Debug, Serialize, Clone)]
pub struct HistoryPage {
    pub commits: Vec<HistoryCommit>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub max_lane: usize,
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
pub struct MergeInfo {
    /// Branches whose history contains this branch's tip — i.e. it's merged in.
    pub merged_into: Vec<String>,
    /// The merge commits on this branch, newest first.
    pub merges: Vec<HistoryCommit>,
}

fn git(repo: &Path, args: &[&str]) -> Result<String, AppError> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| AppError::Command(format!("failed to run git: {}", e)))?;
    if !out.status.success() {
        return Err(AppError::Command(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

// ---------------------------------------------------------------------------
// Commit graph lane assignment (pure — unit tested)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub struct GraphRow {
    pub lane: usize,
    pub parent_lanes: Vec<usize>,
    pub active: Vec<usize>,
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
        let lane = match lanes.iter().position(|l| l.as_deref() == Some(hash.as_str())) {
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
        });
    }
    rows
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Every repo inside the profile folders, so the picker needs no typing.
pub async fn list_repos() -> Result<Vec<RepoRef>, AppError> {
    let store = profiles::load_profiles()?;
    let mut out: Vec<RepoRef> = Vec::new();
    for p in &store.profiles {
        for dir in &p.directories {
            if !PathBuf::from(dir).is_dir() {
                continue;
            }
            for r in repo_scan::scan_repos(dir.clone()).await.unwrap_or_default() {
                if out.iter().any(|e| e.path == r.path) {
                    continue;
                }
                out.push(RepoRef {
                    name: crate::paths::base_name(&r.path),
                    path: r.path,
                    profile_id: p.id.clone(),
                    profile_name: p.name.clone(),
                    profile_email: p.git_email.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

pub fn list_branches(repo_path: &str) -> Result<Vec<BranchInfo>, AppError> {
    let repo = PathBuf::from(repo_path);
    let fmt = format!(
        "%(refname){US}%(refname:short){US}%(upstream:short){US}%(objectname){US}%(objectname:short){US}%(authorname){US}%(committerdate:iso8601){US}%(HEAD){US}%(subject)",
        US = US
    );
    let raw = git(
        &repo,
        &["for-each-ref", "--sort=-committerdate", "--format", &fmt, "refs/heads", "refs/remotes"],
    )?;

    let mut branches = Vec::new();
    for line in raw.lines() {
        // subject is last so any separator inside it can't shift the other fields
        let f: Vec<&str> = line.splitn(9, US).collect();
        if f.len() < 9 {
            continue;
        }
        let full_ref = f[0];
        let name = f[1].to_string();
        // origin/HEAD is a symbolic pointer, not a real branch.
        if name.ends_with("/HEAD") {
            continue;
        }
        let upstream = if f[2].is_empty() { None } else { Some(f[2].to_string()) };
        let is_remote = full_ref.starts_with("refs/remotes/");

        // ahead/behind only makes sense for a local branch with an upstream.
        let (mut ahead, mut behind) = (0usize, 0usize);
        if let (false, Some(up)) = (is_remote, upstream.as_ref()) {
            if let Ok(counts) = git(&repo, &["rev-list", "--left-right", "--count", &format!("{}...{}", name, up)]) {
                let mut it = counts.split_whitespace();
                ahead = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                behind = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            }
        }

        branches.push(BranchInfo {
            is_current: f[7].trim() == "*",
            is_remote,
            name,
            upstream,
            ahead,
            behind,
            tip: f[3].to_string(),
            short_tip: f[4].to_string(),
            last_author: f[5].to_string(),
            last_date: f[6].to_string(),
            last_subject: f[8].to_string(),
        });
    }
    Ok(branches)
}

fn parse_log(raw: &str) -> Vec<(String, Vec<String>, Vec<String>)> {
    // (hash, parents, remaining fields)
    raw.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(8, US).collect();
            if f.len() < 8 {
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

/// One page of history for a branch (or every branch when `rev` is "--all").
pub fn history_page(
    repo_path: &str,
    rev: &str,
    offset: usize,
    limit: usize,
    search: Option<&str>,
    author: Option<&str>,
) -> Result<HistoryPage, AppError> {
    let repo = PathBuf::from(repo_path);
    if rev != "--all" && rev.starts_with('-') {
        return Err(AppError::Config(format!("Invalid revision: {}", rev)));
    }
    let fmt = format!(
        "%H{US}%h{US}%P{US}%an{US}%ae{US}%cI{US}%D{US}%s",
        US = US
    );

    let mut count_args: Vec<String> = vec!["rev-list".into(), "--count".into()];
    let mut log_args: Vec<String> = vec![
        "log".into(),
        format!("--skip={}", offset),
        format!("-n{}", limit),
        "--date-order".into(),
        format!("--format={}", fmt),
    ];
    // Limiting patterns are matched as FIXED STRINGS: emails and names routinely
    // contain regex metacharacters (`.`, `+`, `[`), and a noreply address like
    // `1234+user@users.noreply.github.com` must match literally.
    let mut limits: Vec<String> = Vec::new();
    if let Some(q) = search.filter(|s| !s.trim().is_empty()) {
        limits.push(format!("--grep={}", q.trim()));
    }
    if let Some(a) = author.filter(|s| !s.trim().is_empty()) {
        limits.push(format!("--author={}", a.trim()));
    }
    if !limits.is_empty() {
        for a in ["--fixed-strings".to_string(), "--regexp-ignore-case".to_string()] {
            count_args.push(a.clone());
            log_args.push(a);
        }
        for a in limits {
            count_args.push(a.clone());
            log_args.push(a);
        }
    }
    if rev == "--all" {
        count_args.push("--all".into());
        log_args.push("--all".into());
    } else {
        count_args.push(rev.to_string());
        log_args.push(rev.to_string());
    }

    let count_ref: Vec<&str> = count_args.iter().map(String::as_str).collect();
    let total: usize = git(&repo, &count_ref)
        .unwrap_or_default()
        .trim()
        .parse()
        .unwrap_or(0);

    let log_ref: Vec<&str> = log_args.iter().map(String::as_str).collect();
    // A repo with no commits yet makes `git log` exit non-zero; that's an empty
    // history, not an error worth showing the user.
    let raw = match git(&repo, &log_ref) {
        Ok(r) => r,
        Err(_) if total == 0 => String::new(),
        Err(e) => return Err(e),
    };
    let parsed = parse_log(&raw);

    let graph_input: Vec<(String, Vec<String>)> = parsed
        .iter()
        .map(|(h, p, _)| (h.clone(), p.clone()))
        .collect();
    let rows = assign_lanes(&graph_input);

    let mut commits = Vec::with_capacity(parsed.len());
    let mut max_lane = 0usize;
    for (i, (hash, parents, f)) in parsed.into_iter().enumerate() {
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
            refs,
            hash,
            parents,
            lane: row.lane,
            parent_lanes: row.parent_lanes.clone(),
            active_lanes: row.active.clone(),
        });
    }

    Ok(HistoryPage {
        has_more: offset + commits.len() < total,
        total,
        offset,
        limit,
        max_lane,
        commits,
    })
}

pub fn commit_detail(repo_path: &str, hash: &str) -> Result<CommitDetail, AppError> {
    let repo = PathBuf::from(repo_path);
    let fmt = format!(
        "%H{US}%h{US}%an{US}%ae{US}%aI{US}%cn{US}%ce{US}%P{US}%D{US}%G?{US}%s{US}%b",
        US = US
    );
    let raw = git(&repo, &["show", "-s", &format!("--format={}", fmt), hash])?;
    let f: Vec<&str> = raw.splitn(12, US).collect();
    if f.len() < 12 {
        return Err(AppError::NotFound(format!("Commit {} not found", hash)));
    }

    // --numstat gives machine-readable per-file add/remove counts.
    let stat = git(&repo, &["show", "--numstat", "--format=", hash]).unwrap_or_default();
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
    let tip = git(&repo, &["rev-parse", branch])?;

    let merged_into: Vec<String> = git(
        &repo,
        &["branch", "--all", "--contains", &tip, "--format=%(refname:short)"],
    )
    .unwrap_or_default()
    .lines()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty() && s != branch && !s.ends_with("/HEAD"))
    .collect();

    let page = history_page(repo_path, branch, 0, 400, None, None)?;
    let merges = page.commits.into_iter().filter(|c| c.is_merge).take(50).collect();

    Ok(MergeInfo {
        merged_into,
        merges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
