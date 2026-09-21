//! What each submodule is actually doing.
//!
//! The superproject's status tells you almost nothing about a submodule: one
//! row, three booleans, and no row at all when the submodule was never
//! initialised. This module answers the questions a person actually has —
//! which commit it moved from and to, how far, what changed inside it, and
//! whether git can even fetch it.
//!
//! **`git submodule status` is not used, and must not be.** On a real repo
//! (`argos`: 32 gitlinks, 5 of them mapped) it aborts on the first gitlink
//! missing from `.gitmodules`:
//!
//! ```text
//! fatal: no submodule mapping found in .gitmodules for path 'forge-tooling/aggtest/astropy'
//! ```
//!
//! …and prints nothing further, so 31 submodules become invisible because of
//! one unmapped entry. `git submodule foreach` fails the same way. The list is
//! therefore built from `git ls-files -s` plus `.gitmodules`, and every probe
//! runs per path — the same reasoning `sparse::update_submodules` documents.

use crate::error::AppError;
use crate::git_exec::GitCmd;
use crate::git_status::parse_porcelain_v2;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A commit the submodule moved across, for the "what changed" list.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct MovedCommit {
    pub short: String,
    pub subject: String,
}

/// One `.gitmodules` entry.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ModuleCfg {
    pub name: String,
    pub url: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct SubmoduleInfo {
    /// Path inside the superproject, e.g. "vendor/lib".
    pub path: String,
    pub name: Option<String>,
    /// Masked, because a URL can carry a token.
    pub url: Option<String>,
    pub configured_branch: Option<String>,
    /// The commit the superproject records for this path.
    pub recorded: String,
    pub recorded_short: String,
    /// What is actually checked out. `None` when not initialised.
    pub actual: Option<String>,
    pub actual_short: Option<String>,
    pub initialised: bool,
    /// Has an entry in `.gitmodules`. Without one git has no address to fetch.
    pub listed: bool,
    /// How the checkout compares with the recorded commit.
    pub ahead: usize,
    pub behind: usize,
    /// The recorded commit isn't in the submodule's object store, so the two
    /// can't be compared until it fetches.
    pub recorded_missing: bool,
    pub moved_commits: Vec<MovedCommit>,
    pub more_moved: usize,
    /// The submodule's own repository state.
    pub own_branch: Option<String>,
    pub own_upstream: Option<String>,
    pub own_ahead: usize,
    pub own_behind: usize,
    pub dirty_tracked: usize,
    pub dirty_untracked: usize,
    /// The untracked count hit the parser's cap, so it is a floor, not a total.
    pub dirty_untracked_capped: bool,
    /// "clean" | "moved" | "dirty" | "moved-and-dirty" | "not-initialised" | "unmapped"
    pub state: String,
    /// One plain sentence, written here so every surface says the same thing.
    pub summary: String,
}

const MAX_MOVED: usize = 10;

/// Parse `git ls-files -s`, keeping only gitlinks.
///
/// Format is `<mode> <object> <stage>\t<path>`; mode `160000` is a gitlink.
/// A tab separates the path, so paths containing spaces survive.
pub fn parse_gitlinks(out: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in out.lines() {
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split_whitespace();
        let (Some(mode), Some(oid)) = (parts.next(), parts.next()) else {
            continue;
        };
        if mode == "160000" && !path.is_empty() {
            found.push((oid.to_string(), path.to_string()));
        }
    }
    found
}

/// Parse `git config -f .gitmodules --get-regexp '\.(path|url|branch)$'` into a
/// map keyed by **path**, which is how every other part of git refers to a
/// submodule. A name may contain dots, so the key is taken between the
/// `submodule.` prefix and the final `.path` / `.url` / `.branch`.
pub fn parse_gitmodules(out: &str) -> HashMap<String, ModuleCfg> {
    /// path, url, branch — collected per name before being keyed by path.
    type Fields = (Option<String>, Option<String>, Option<String>);
    let mut by_name: HashMap<String, Fields> = HashMap::new();

    for line in out.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let Some(rest) = key.strip_prefix("submodule.") else {
            continue;
        };
        let Some((name, field)) = rest.rsplit_once('.') else {
            continue;
        };
        if name.is_empty() || value.is_empty() {
            continue;
        }
        let slot = by_name.entry(name.to_string()).or_default();
        match field {
            "path" => slot.0 = Some(value.to_string()),
            "url" => slot.1 = Some(value.to_string()),
            "branch" => slot.2 = Some(value.to_string()),
            _ => {}
        }
    }

    let mut by_path = HashMap::new();
    for (name, (path, url, branch)) in by_name {
        // An entry with no `path` can't be matched to a gitlink, so it is
        // dropped rather than guessed at.
        if let Some(path) = path {
            by_path.insert(path, ModuleCfg { name, url, branch });
        }
    }
    by_path
}

/// Which of the six states this submodule is in.
pub fn classify(
    initialised: bool,
    listed: bool,
    moved: bool,
    dirty_tracked: usize,
    dirty_untracked: usize,
) -> &'static str {
    if !initialised {
        return if listed { "not-initialised" } else { "unmapped" };
    }
    let dirty = dirty_tracked > 0 || dirty_untracked > 0;
    match (moved, dirty) {
        (true, true) => "moved-and-dirty",
        (true, false) => "moved",
        (false, true) => "dirty",
        (false, false) => "clean",
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// The status parser stops collecting untracked files at a cap, so a capped
/// count is a floor. Saying "2000" when the real number is 8060 would be wrong.
fn count_with_cap(n: usize, capped: bool, one: &str, many: &str) -> String {
    if capped {
        format!("{}+ {}", n, many)
    } else {
        plural(n, one, many)
    }
}

/// The sentence shown next to the submodule. Pure, so the wording is testable.
#[allow(clippy::too_many_arguments)] // one argument per fact the sentence states
pub fn summarise(
    state: &str,
    listed: bool,
    ahead: usize,
    behind: usize,
    recorded_missing: bool,
    dirty_tracked: usize,
    dirty_untracked: usize,
    untracked_capped: bool,
    own_branch: Option<&str>,
    own_upstream: Option<&str>,
    own_behind: usize,
    own_ahead: usize,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    match state {
        "unmapped" => {
            return "not initialised, and it has no entry in .gitmodules, so git can't fetch it"
                .to_string()
        }
        "not-initialised" => {
            return "not initialised — Update submodules will fetch it".to_string()
        }
        _ => {}
    }

    if state == "clean" {
        parts.push("up to date with what this repo records".to_string());
    }

    if recorded_missing {
        parts.push(
            "points at a commit this submodule doesn't have yet — fetch inside it to compare"
                .to_string(),
        );
    } else if ahead > 0 && behind > 0 {
        parts.push(format!(
            "has diverged from what this repo records: {} ahead, {} behind",
            ahead, behind
        ));
    } else if ahead > 0 {
        parts.push(format!(
            "moved {} ahead of what this repo records",
            plural(ahead, "commit", "commits")
        ));
    } else if behind > 0 {
        parts.push(format!(
            "sits {} behind what this repo records",
            plural(behind, "commit", "commits")
        ));
    }

    let untracked_text =
        count_with_cap(dirty_untracked, untracked_capped, "untracked file", "untracked files");
    if dirty_tracked > 0 && dirty_untracked > 0 {
        parts.push(format!(
            "{} and {} inside",
            plural(dirty_tracked, "changed file", "changed files"),
            untracked_text
        ));
    } else if dirty_tracked > 0 {
        parts.push(format!(
            "{} inside",
            plural(dirty_tracked, "changed file", "changed files")
        ));
    } else if dirty_untracked > 0 {
        parts.push(format!("{} inside", untracked_text));
    }

    if let Some(branch) = own_branch {
        let mut where_ = format!("on {}", branch);
        if let Some(up) = own_upstream {
            if own_behind > 0 && own_ahead > 0 {
                where_.push_str(&format!(", {} ahead and {} behind {}", own_ahead, own_behind, up));
            } else if own_behind > 0 {
                where_.push_str(&format!(", {} behind {}", own_behind, up));
            } else if own_ahead > 0 {
                where_.push_str(&format!(", {} ahead of {}", own_ahead, up));
            }
        }
        parts.push(where_);
    } else {
        parts.push("not on a branch (detached)".to_string());
    }

    if parts.is_empty() {
        return "up to date with what this repo records".to_string();
    }
    if !listed {
        parts.push("no entry in .gitmodules, so git can't fetch it".to_string());
    }
    parts.join("; ")
}

fn short(oid: &str) -> String {
    oid.chars().take(7).collect()
}

/// Read one submodule's own repository state. Cheap — measured at 70 ms even
/// for a submodule holding 8060 untracked files.
type OwnState = (Option<String>, Option<String>, usize, usize, usize, usize, bool);

async fn read_own_state(sub: &Path) -> OwnState {
    let Ok(out) = GitCmd::at(sub)
        .top_flag("--no-optional-locks")
        .args([
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ])
        .run()
        .await
    else {
        return (None, None, 0, 0, 0, 0, false);
    };
    if !out.ok() {
        return (None, None, 0, 0, 0, 0, false);
    }
    let parsed = parse_porcelain_v2(&out.stdout);
    let tracked = parsed
        .entries
        .iter()
        .filter(|e| e.kind != "untracked")
        .count();
    let untracked = parsed
        .entries
        .iter()
        .filter(|e| e.kind == "untracked")
        .count();
    (
        parsed.branch,
        parsed.upstream,
        parsed.ahead,
        parsed.behind,
        tracked,
        untracked,
        parsed.untracked_truncated,
    )
}

/// Every gitlink in the repo, with everything known about it.
pub async fn list_submodules(repo_path: &str) -> Result<Vec<SubmoduleInfo>, AppError> {
    let repo = PathBuf::from(repo_path);
    if !repo.is_dir() {
        return Err(AppError::NotFound(format!(
            "{} no longer exists on disk",
            repo_path
        )));
    }

    let listing = GitCmd::at(&repo)
        .args(["ls-files", "-s"])
        .text()
        .await
        .unwrap_or_default();
    let gitlinks = parse_gitlinks(&listing);
    if gitlinks.is_empty() {
        return Ok(Vec::new());
    }

    // Absent .gitmodules is normal: every gitlink is then unmapped.
    let cfg_out = GitCmd::at(&repo)
        .args([
            "config",
            "-f",
            ".gitmodules",
            "--get-regexp",
            r"\.(path|url|branch)$",
        ])
        .ok_text()
        .await
        .unwrap_or_default();
    let cfg = parse_gitmodules(&cfg_out);

    let mut out = Vec::with_capacity(gitlinks.len());
    for (recorded, path) in gitlinks {
        let module = cfg.get(&path);
        let sub_dir = repo.join(&path);
        // `.git` is a *file* inside a submodule, so this matches either form.
        let initialised = sub_dir.join(".git").exists();

        let mut info = SubmoduleInfo {
            recorded_short: short(&recorded),
            recorded,
            path: path.clone(),
            name: module.map(|m| m.name.clone()),
            url: module
                .and_then(|m| m.url.as_deref())
                .map(crate::repo_scan::mask_token),
            configured_branch: module.and_then(|m| m.branch.clone()),
            actual: None,
            actual_short: None,
            initialised,
            listed: module.is_some(),
            ahead: 0,
            behind: 0,
            recorded_missing: false,
            moved_commits: Vec::new(),
            more_moved: 0,
            own_branch: None,
            own_upstream: None,
            own_ahead: 0,
            own_behind: 0,
            dirty_tracked: 0,
            dirty_untracked: 0,
            dirty_untracked_capped: false,
            state: String::new(),
            summary: String::new(),
        };

        if initialised {
            info.actual = GitCmd::at(&sub_dir)
                .args(["rev-parse", "HEAD"])
                .ok_text()
                .await
                .filter(|s| !s.is_empty());
            info.actual_short = info.actual.as_deref().map(short);

            let (branch, upstream, ahead, behind, tracked, untracked, capped) =
                read_own_state(&sub_dir).await;
            info.own_branch = branch;
            info.own_upstream = upstream;
            info.own_ahead = ahead;
            info.own_behind = behind;
            info.dirty_tracked = tracked;
            info.dirty_untracked = untracked;
            info.dirty_untracked_capped = capped;

            if let Some(actual) = info.actual.clone() {
                if actual != info.recorded {
                    // Without the recorded commit present there is nothing to
                    // compare against, and rev-list would simply fail.
                    let have_recorded = GitCmd::at(&sub_dir)
                        .args(["cat-file", "-e", &format!("{}^{{commit}}", info.recorded)])
                        .run()
                        .await
                        .map(|o| o.ok())
                        .unwrap_or(false);
                    info.recorded_missing = !have_recorded;

                    if have_recorded {
                        if let Some(counts) = GitCmd::at(&sub_dir)
                            .args([
                                "rev-list",
                                "--left-right",
                                "--count",
                                &format!("{}...{}", info.recorded, actual),
                            ])
                            .ok_text()
                            .await
                        {
                            // left = only the recorded side, right = only the checkout.
                            let mut it = counts.split_whitespace();
                            info.behind = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                            info.ahead = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                        }
                        if info.ahead > 0 {
                            if let Some(log) = GitCmd::at(&sub_dir)
                                .args([
                                    "log",
                                    &format!("-n{}", MAX_MOVED),
                                    "--format=%h\x1f%s",
                                    &format!("{}..{}", info.recorded, actual),
                                ])
                                .ok_text()
                                .await
                            {
                                info.moved_commits = log
                                    .lines()
                                    .filter_map(|l| l.split_once('\x1f'))
                                    .map(|(h, s)| MovedCommit {
                                        short: h.to_string(),
                                        subject: s.to_string(),
                                    })
                                    .collect();
                                info.more_moved = info.ahead.saturating_sub(info.moved_commits.len());
                            }
                        }
                    }
                }
            }
        }

        let moved = info.ahead > 0 || info.behind > 0 || info.recorded_missing;
        info.state = classify(
            info.initialised,
            info.listed,
            moved,
            info.dirty_tracked,
            info.dirty_untracked,
        )
        .to_string();
        info.summary = summarise(
            &info.state,
            info.listed,
            info.ahead,
            info.behind,
            info.recorded_missing,
            info.dirty_tracked,
            info.dirty_untracked,
            info.dirty_untracked_capped,
            info.own_branch.as_deref(),
            info.own_upstream.as_deref(),
            info.own_behind,
            info.own_ahead,
        );
        out.push(info);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitlinks_are_picked_out_of_ls_files() {
        let out = "100644 aaa0000 0\tREADME.md\n\
                   160000 dceaf0bd6574d6ea9a551a5374a0188d229b23e0 0\ttrinity\n\
                   100644 bbb0000 0\tsrc/main.rs\n\
                   160000 9ce37bdd5c866dd944eb9ce50bb9a235a88fe17b 0\tstaging\n";
        let links = parse_gitlinks(out);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].1, "trinity");
        assert_eq!(links[0].0, "dceaf0bd6574d6ea9a551a5374a0188d229b23e0");
        assert_eq!(links[1].1, "staging");
    }

    #[test]
    fn a_path_with_spaces_survives_because_a_tab_separates_it() {
        let links = parse_gitlinks("160000 abc123 0\tvendor/my lib\n");
        assert_eq!(links[0].1, "vendor/my lib");
    }

    #[test]
    fn nested_and_malformed_lines_do_not_derail_the_parse() {
        let out = "160000 abc 0\tforge-tooling/aggtest/astropy\n\
                   garbage\n\
                   \n\
                   160000 def 0\tdeep/nested/sub\n";
        let links = parse_gitlinks(out);
        assert_eq!(links.len(), 2);
        assert_eq!(links[1].1, "deep/nested/sub");
    }

    #[test]
    fn gitmodules_is_keyed_by_path_not_name() {
        let out = "submodule.trinity.path trinity\n\
                   submodule.trinity.url org-302118749@github.com:EtharaOrion/trinity.git\n\
                   submodule.trinity.branch main\n\
                   submodule.staging.path staging\n\
                   submodule.staging.url org-302118749@github.com:EtharaOrion/staging.git\n";
        let cfg = parse_gitmodules(out);
        assert_eq!(cfg.len(), 2);
        let t = &cfg["trinity"];
        assert_eq!(t.name, "trinity");
        assert_eq!(t.branch.as_deref(), Some("main"));
        assert!(t.url.as_deref().unwrap().contains("EtharaOrion/trinity"));
        // No branch declared is normal.
        assert!(cfg["staging"].branch.is_none());
    }

    #[test]
    fn a_submodule_name_containing_dots_still_parses() {
        let cfg = parse_gitmodules(
            "submodule.vendor.lib.v2.path vendor/lib\nsubmodule.vendor.lib.v2.url git@h:o/r.git\n",
        );
        assert_eq!(cfg["vendor/lib"].name, "vendor.lib.v2");
    }

    #[test]
    fn an_entry_with_no_path_is_dropped_rather_than_guessed() {
        // A url without a path can't be matched to a gitlink.
        let cfg = parse_gitmodules("submodule.orphan.url git@h:o/r.git\n");
        assert!(cfg.is_empty());
    }

    #[test]
    fn an_absent_gitmodules_leaves_every_gitlink_unmapped() {
        assert!(parse_gitmodules("").is_empty());
    }

    #[test]
    fn every_state_is_reachable() {
        assert_eq!(classify(false, false, false, 0, 0), "unmapped");
        assert_eq!(classify(false, true, false, 0, 0), "not-initialised");
        assert_eq!(classify(true, true, false, 0, 0), "clean");
        assert_eq!(classify(true, true, false, 3, 0), "dirty");
        assert_eq!(classify(true, true, false, 0, 8060), "dirty");
        assert_eq!(classify(true, true, true, 0, 0), "moved");
        assert_eq!(classify(true, true, true, 2, 1), "moved-and-dirty");
    }

    fn s(state: &str, listed: bool, ahead: usize, behind: usize, missing: bool, t: usize, u: usize) -> String {
        summarise(state, listed, ahead, behind, missing, t, u, false, Some("main"), Some("origin/main"), 0, 0)
    }

    #[test]
    fn an_unfetchable_submodule_says_why() {
        let msg = summarise("unmapped", false, 0, 0, false, 0, 0, false, None, None, 0, 0);
        assert!(msg.contains(".gitmodules"));
        assert!(msg.contains("can't fetch"));
    }

    #[test]
    fn a_listed_but_missing_submodule_points_at_the_fix() {
        let msg = summarise("not-initialised", true, 0, 0, false, 0, 0, false, None, None, 0, 0);
        assert!(msg.contains("Update submodules"));
    }

    #[test]
    fn a_moved_submodule_states_the_direction_and_size() {
        // The real argos case: trinity, ten commits ahead of the recorded one.
        let msg = s("moved", true, 10, 0, false, 0, 0);
        assert!(msg.contains("moved 10 commits ahead"), "{}", msg);
        assert!(msg.contains("on main"), "{}", msg);

        assert!(s("moved", true, 0, 3, false, 0, 0).contains("sits 3 commits behind"));
        assert!(s("moved", true, 2, 3, false, 0, 0).contains("diverged"));
        // Singular reads correctly.
        assert!(s("moved", true, 1, 0, false, 0, 0).contains("moved 1 commit ahead"));
    }

    #[test]
    fn a_dirty_submodule_counts_what_is_inside() {
        // The real argos case: staging, 8060 untracked files.
        let msg = s("dirty", true, 0, 0, false, 0, 8060);
        assert!(msg.contains("8060 untracked files inside"), "{}", msg);
        let both = s("moved-and-dirty", true, 2, 0, false, 3, 4);
        assert!(both.contains("3 changed files") && both.contains("4 untracked files"));
    }

    #[test]
    fn a_capped_untracked_count_is_reported_as_a_floor() {
        // argos's `staging` really holds 8060 untracked files; the status parser
        // stops at 2000, so saying "2000" flat would be a lie.
        let msg = summarise(
            "dirty", true, 0, 0, false, 0, 2000, true, Some("main"), None, 0, 0,
        );
        assert!(msg.contains("2000+ untracked files inside"), "{}", msg);
    }

    #[test]
    fn a_clean_submodule_says_it_is_up_to_date() {
        let msg = s("clean", true, 0, 0, false, 0, 0);
        assert!(msg.contains("up to date with what this repo records"), "{}", msg);
    }

    #[test]
    fn a_missing_recorded_commit_is_explained_not_guessed() {
        let msg = s("moved", true, 0, 0, true, 0, 0);
        assert!(msg.contains("doesn't have yet"), "{}", msg);
        assert!(msg.contains("fetch"), "{}", msg);
    }

    #[test]
    fn the_submodules_own_position_is_reported() {
        let behind = summarise("clean", true, 0, 0, false, 0, 0, false, Some("main"), Some("origin/main"), 11, 0);
        assert!(behind.contains("on main, 11 behind origin/main"), "{}", behind);
        let detached = summarise("clean", true, 0, 0, false, 0, 0, false, None, None, 0, 0);
        assert!(detached.contains("detached"));
        let both = summarise("clean", true, 0, 0, false, 0, 0, false, Some("main"), Some("origin/main"), 2, 5);
        assert!(both.contains("5 ahead and 2 behind"), "{}", both);
    }

    #[test]
    fn an_initialised_but_unmapped_submodule_still_notes_it_cannot_be_fetched() {
        let msg = summarise("dirty", false, 0, 0, false, 1, 0, false, Some("main"), None, 0, 0);
        assert!(msg.contains("no entry in .gitmodules"), "{}", msg);
    }
}
