//! Test-only driver for the verification suites under `scripts/verify/`.
//!
//! Compiled only with `#[cfg(test)]`, and the single test is `#[ignore]`d, so
//! `cargo test` never runs it and the app never contains it. The shell suites
//! invoke the test binary with `PROBE_OP` / `PROBE_REPO` / `PROBE_ARGS` set and
//! read one `PROBE_OUT <json>` line back — the real backend, driven from
//! outside, against throwaway repositories.
#[cfg(test)]
mod tests {
    use crate::git_ops::{self, PullMode};
    use crate::git_status;

    fn repo() -> String {
        std::env::var("PROBE_REPO").expect("PROBE_REPO")
    }

    fn args() -> Vec<String> {
        std::env::var("PROBE_ARGS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    fn emit<T: serde::Serialize>(value: &T) {
        println!("PROBE_OUT {}", serde_json::to_string(value).unwrap());
    }

    fn emit_err(e: impl std::fmt::Display) {
        println!("PROBE_OUT {{\"error\":{:?}}}", e.to_string());
    }

    fn show(r: Result<git_ops::OpResult, crate::error::AppError>) {
        match r {
            Ok(o) => emit(&serde_json::json!({
                "ok": o.ok,
                "headline": o.headline,
                "detail": o.detail,
                "refusal": o.refusal,
                "advice": o.advice,
                "commit": o.commit,
                "push": o.push,
                "pull": o.pull,
                "submodules": o.submodules,
                "lfs": o.lfs,
                "sync": o.sync,
                "staged": o.status.as_ref().map(|s| s.staged_count),
                "unstaged": o.status.as_ref().map(|s| s.unstaged_count),
                "untracked": o.status.as_ref().map(|s| s.untracked_count),
                "conflicted": o.status.as_ref().map(|s| s.conflicted_count),
                "operation": o.status.as_ref().and_then(|s| s.operation.clone()),
                "branch": o.status.as_ref().and_then(|s| s.branch.clone()),
                "ahead": o.status.as_ref().map(|s| s.ahead),
                "behind": o.status.as_ref().map(|s| s.behind),
            })),
            Err(e) => emit_err(e),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn probe() {
        let op = std::env::var("PROBE_OP").expect("PROBE_OP");
        let a = args();
        let first = a.first().map(|s| s.as_str()).unwrap_or("");
        match op.as_str() {
            // --- Changes page ---
            "status" => match git_status::repo_status(&repo()).await {
                Ok(s) => emit(&s),
                Err(e) => emit_err(e),
            },
            "diff" => {
                let staged = a.get(1).map(|s| s == "staged").unwrap_or(false);
                let untracked = a.get(2).map(|s| s == "untracked").unwrap_or(false);
                match git_status::file_diff(&repo(), first, staged, untracked).await {
                    Ok(d) => emit(&d),
                    Err(e) => emit_err(e),
                }
            }
            "stage" => show(git_ops::stage(&repo(), a).await),
            "stage_all" => show(git_ops::stage_all(&repo()).await),
            "unstage" => show(git_ops::unstage(&repo(), a).await),
            "discard" => show(git_ops::discard(&repo(), a).await),
            "commit" => {
                let amend = a.get(1).map(|s| s == "amend").unwrap_or(false);
                show(git_ops::commit(&repo(), first, amend).await)
            }
            "push" => show(git_ops::push(&repo(), first == "upstream").await),
            "pull" => {
                let with_lfs = a.get(1).map(|s| s == "lfs").unwrap_or(false);
                show(git_ops::pull(&repo(), PullMode::parse(first).unwrap(), with_lfs).await)
            }
            "submodule" => show(git_ops::submodule_update(&repo()).await),
            "abort" => show(git_ops::abort(&repo()).await),
            "continue" => show(git_ops::continue_op(&repo()).await),
            // PROBE_ARGS: "stash" | "nostash"
            "sync_plan" => match crate::sync::sync_plan(&repo(), first != "nostash", true).await {
                Ok(p) => emit(&p),
                Err(e) => emit_err(e),
            },
            // PROBE_ARGS: "[nostash][,bundles]" — no fingerprint: the run measures afresh
            "sync_run" => show(crate::sync::sync_run(&repo(), crate::sync::RunOptions { stash: !a.iter().any(|s| s == "nostash"), bundles: a.iter().any(|s| s == "bundles"), fingerprint: Vec::new() }).await),
            "sync_continue" => show(crate::sync::sync_continue(&repo()).await),
            "sync_abort" => show(crate::sync::sync_abort(&repo()).await),
            "record_pointers" => show(crate::sync::record_pointers(&repo()).await),
            "block" => match crate::push_lock::set_push_mode(&repo(), if first == "on" { "guardrail" } else { "allowed" }).await {
                Ok(r) => match r.state {
                    Some(s) => emit(&s),
                    None => emit(&r),
                },
                Err(e) => emit_err(e),
            },
            // The guard-rail path on its own (what the old boolean toggle did),
            // to prove it cannot remove a lock.
            "guardrail_off" => match git_status::git_paths(std::path::Path::new(&repo())).await {
                Ok(paths) => match crate::push_guard::set_repo_blocked(&repo(), &paths.hooks, false, git_status::owning_profile_id(&repo()).as_deref()).await {
                    Ok(s) => emit(&s),
                    Err(e) => emit_err(e),
                },
                Err(e) => emit_err(e),
            },
            "push_mode" => match crate::push_lock::set_push_mode(&repo(), first).await {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            "lock_state" => match git_status::push_state_for(&repo()).await {
                Ok(s) => emit(&s),
                Err(e) => emit_err(e),
            },
            "lock_repair" => match crate::push_lock::repair_lock(&repo()).await {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            "lock_fix" => emit(&crate::push_lock::fix(first).await),
            "lock_helper_status" => emit(&crate::push_lock::helper_status().await),
            "lock_uninstall" => emit(&crate::push_lock::uninstall_all().await),
            "doctor_locks" => match crate::doctor::check_push_locks().await {
                Ok(f) => emit(&f),
                Err(e) => emit_err(e),
            },
            "lfs_status" => match crate::lfs::lfs_status(&repo()).await {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            "lfs_pull" => show(git_ops::lfs_pull(&repo()).await),
            "submodules_list" => match crate::submodules::list_submodules(&repo()).await {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            // --- features that predate the Changes page ---
            "clone" => {
                let with_submodules = a.get(3).map(|s| s == "submodules").unwrap_or(false);
                let with_lfs = a.get(4).map(|s| s == "lfs").unwrap_or(false);
                match crate::sparse::full_clone(first, a.get(1).map(|s| s.as_str()).unwrap_or(""), a.get(2).map(|s| s.as_str()), None, with_submodules, with_lfs).await {
                    Ok(r) => emit(&r),
                    Err(e) => emit_err(e),
                }
            }
            // PROBE_ARGS = "<dir1:dir2>[,lfs]"
            "sparse_set" => {
                let dirs: Vec<String> = first.split(':').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                let with_lfs = a.get(1).map(|s| s == "lfs").unwrap_or(false);
                match crate::sparse::sparse_set(&repo(), dirs, with_lfs).await {
                    Ok(r) => emit(&r),
                    Err(e) => emit_err(e),
                }
            }
            "lfs_available" => emit(&crate::lfs::lfs_available().await),
            "sparse_clone" => match crate::sparse::sparse_clone(first, a.get(1).map(|s| s.as_str()).unwrap_or(""), a.get(2).map(|s| s.as_str()), None).await {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            "sparse_info" => match crate::sparse::repo_info(&repo()).await {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            "guard_install" => match crate::commit_audit::install_guard(&repo(), first) {
                Ok(m) => emit(&serde_json::json!({ "message": m })),
                Err(e) => emit_err(e),
            },
            "guard_uninstall" => match crate::commit_audit::uninstall_guard(&repo()) {
                Ok(m) => emit(&serde_json::json!({ "message": m })),
                Err(e) => emit_err(e),
            },
            "branches" => match crate::git_history::list_branches(&repo()) {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            "sync" => match crate::git_history::sync_status(&repo(), first) {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            "hpage" => match crate::git_history::history_page(&repo(), "HEAD", 0, 20, None, None) {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            other => panic!("unknown op {}", other),
        }
    }
}
