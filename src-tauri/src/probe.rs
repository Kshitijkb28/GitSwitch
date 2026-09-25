//! Test-only driver for the verification suites under `scripts/verify/`.
//!
//! Compiled only with `#[cfg(test)]`, and the single test is `#[ignore]`d, so
//! `cargo test` never runs it and the app never contains it. The shell suites
//! invoke the test binary with `PROBE_OP` / `PROBE_REPO` / `PROBE_ARGS` set and
//! read one `PROBE_OUT <json>` line back — the real backend, driven from
//! outside, against throwaway repositories.
#[cfg(test)]
pub(crate) mod tests {
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

    pub(crate) fn emit<T: serde::Serialize>(value: &T) {
        println!("PROBE_OUT {}", serde_json::to_string(value).unwrap());
    }

    pub(crate) fn emit_err(e: impl std::fmt::Display) {
        println!("PROBE_OUT {{\"error\":{:?}}}", e.to_string());
    }

    pub(crate) fn show(r: Result<git_ops::OpResult, crate::error::AppError>) {
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
                "tree": o.tree,
                "stash": o.stash,
                "conflict_source": o.status.as_ref().and_then(|s| s.conflict_source.clone()),
                "detached": o.status.as_ref().map(|s| s.detached),
                "stash_count": o.status.as_ref().map(|s| s.stash_count),
                "head": o.status.as_ref().and_then(|s| s.head_oid.clone()),
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

    // --- `invoke`: the frontend's IPC, forwarded to the real commands ---------
    //
    // `PROBE_OP=invoke PROBE_CMD=<tauri command> PROBE_JSON=<args as the UI
    // sends them, camelCase>` calls the same `#[tauri::command]` function the
    // app would and prints `{"ok":true,"value":<return>}` or
    // `{"ok":false,"error":<AppError>}` — the raw shape, not `show()`'s
    // flattening, so the live UI harness can hand it straight back to the page.

    /// `repo_path` → `repoPath`: the key Tauri's default argument casing expects.
    fn camel(snake: &str) -> String {
        let mut out = String::with_capacity(snake.len());
        let mut up = false;
        for ch in snake.chars() {
            if ch == '_' {
                up = true;
            } else if up {
                out.extend(ch.to_uppercase());
                up = false;
            } else {
                out.push(ch);
            }
        }
        out
    }

    /// One parameter, by its Rust name. Missing or `null` deserialises to `None`
    /// for an `Option<T>` and is an error for anything else — as in Tauri.
    fn arg<T: serde::de::DeserializeOwned>(args: &serde_json::Value, snake: &str) -> Result<T, String> {
        let key = camel(snake);
        let v = args
            .get(&key)
            .or_else(|| args.get(snake))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(v).map_err(|e| format!("invalid argument `{}` for this command: {}", key, e))
    }

    fn reply<T: serde::Serialize>(r: Result<T, crate::error::AppError>) {
        match r {
            Ok(v) => emit(&serde_json::json!({ "ok": true, "value": v })),
            Err(e) => reply_err(e.to_string()),
        }
    }

    fn reply_err(message: String) {
        emit(&serde_json::json!({ "ok": false, "error": message }));
    }

    /// `forward!(c::changes_stage, args, repo_path, paths)`: read each named
    /// parameter from the JSON object, call the command, print the outcome.
    macro_rules! forward {
        ($f:path, $args:expr $(, $p:ident)*) => {{
            $( let $p = match arg(&$args, stringify!($p)) { Ok(v) => v, Err(e) => return reply_err(e) }; )*
            reply($f($($p),*).await)
        }};
    }

    /// Commands that return a plain value rather than a `Result`.
    macro_rules! forward_plain {
        ($f:path, $args:expr $(, $p:ident)*) => {{
            $( let $p = match arg(&$args, stringify!($p)) { Ok(v) => v, Err(e) => return reply_err(e) }; )*
            reply(Ok::<_, crate::error::AppError>($f($($p),*).await))
        }};
    }

    pub(crate) async fn invoke() {
        use crate::commands as c;
        let cmd = std::env::var("PROBE_CMD").unwrap_or_default();
        let raw = std::env::var("PROBE_JSON").unwrap_or_else(|_| "{}".into());
        let args: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return reply_err(format!("PROBE_JSON is not JSON: {}", e)),
        };
        match cmd.as_str() {
            // profiles / repositories
            "get_profiles" => reply(c::get_profiles()),
            "history_list_repos" => forward!(c::history_list_repos, args),
            "path_exists" => {
                let path: String = match arg(&args, "path") { Ok(v) => v, Err(e) => return reply_err(e) };
                reply(Ok::<_, crate::error::AppError>(c::path_exists(path)))
            }
            // Changes page: state and diffs
            "changes_repo_status" => forward!(c::changes_repo_status, args, repo_path),
            "changes_file_diff" => forward!(c::changes_file_diff, args, repo_path, path, staged, untracked),
            "changes_submodules" => forward!(c::changes_submodules, args, repo_path),
            "changes_submodule_statuses" => forward!(c::changes_submodule_statuses, args, repo_path),
            "changes_lfs_status" => forward!(c::changes_lfs_status, args, repo_path),
            "lfs_available" => forward_plain!(c::lfs_available, args),
            // Changes page: operations
            "changes_stage" => forward!(c::changes_stage, args, repo_path, paths),
            "changes_stage_all" => forward!(c::changes_stage_all, args, repo_path),
            "changes_unstage" => forward!(c::changes_unstage, args, repo_path, paths),
            "changes_discard" => forward!(c::changes_discard, args, repo_path, paths),
            "changes_commit" => forward!(c::changes_commit, args, repo_path, message, amend),
            "changes_push" => forward!(c::changes_push, args, repo_path, set_upstream),
            "changes_pull" => forward!(c::changes_pull, args, repo_path, mode, with_lfs, autostash),
            "changes_submodule_update" => forward!(c::changes_submodule_update, args, repo_path),
            "changes_lfs_pull" => forward!(c::changes_lfs_pull, args, repo_path),
            "changes_lfs_files" => forward!(c::changes_lfs_files, args, repo_path),
            "changes_lfs_pull_paths" => forward!(c::changes_lfs_pull_paths, args, repo_path, paths),
            "changes_continue" => forward!(c::changes_continue, args, repo_path),
            "changes_abort" => forward!(c::changes_abort, args, repo_path),
            // sync
            "changes_sync_plan" => forward!(c::changes_sync_plan, args, repo_path, stash, fetch),
            "changes_sync_run" => forward!(c::changes_sync_run, args, repo_path, stash, bundles, fingerprint),
            "changes_sync_continue" => forward!(c::changes_sync_continue, args, repo_path),
            "changes_sync_abort" => forward!(c::changes_sync_abort, args, repo_path),
            "changes_record_pointers" => forward!(c::changes_record_pointers, args, repo_path),
            // push access
            "changes_push_state" => forward!(c::changes_push_state, args, repo_path),
            "changes_set_push_mode" => forward!(c::changes_set_push_mode, args, repo_path, mode),
            "changes_repair_push_lock" => forward!(c::changes_repair_push_lock, args, repo_path),
            "changes_repair_push_block" => forward!(c::changes_repair_push_block, args, repo_path),
            "push_lock_helper_status" => forward_plain!(c::push_lock_helper_status, args),
            "push_lock_finish_manual" => forward_plain!(c::push_lock_finish_manual, args, nonce),
            // tree: branches, undo, reset, revert, conflicts
            "changes_switch_branch" => forward!(c::changes_switch_branch, args, repo_path, name),
            "changes_create_branch" => forward!(c::changes_create_branch, args, repo_path, name, from, switch_to),
            "changes_delete_branch" => forward!(c::changes_delete_branch, args, repo_path, name, force),
            "changes_rename_branch" => forward!(c::changes_rename_branch, args, repo_path, old_name, new_name),
            "changes_undo_commit" => forward!(c::changes_undo_commit, args, repo_path),
            "changes_reset" => forward!(c::changes_reset, args, repo_path, target, mode, stash_first),
            "changes_detach" => forward!(c::changes_detach, args, repo_path, target),
            "changes_revert" => forward!(c::changes_revert, args, repo_path, target, mainline),
            "changes_cherry_pick" => forward!(c::changes_cherry_pick, args, repo_path, target),
            "changes_resolve_side" => forward!(c::changes_resolve_side, args, repo_path, paths, side),
            "changes_discard_all" => forward!(c::changes_discard_all, args, repo_path, include_untracked, stash_first),
            // stashes
            "changes_stash_list" => forward!(c::changes_stash_list, args, repo_path),
            "changes_stash_show" => forward!(c::changes_stash_show, args, repo_path, index),
            "changes_stash_file_diff" => forward!(c::changes_stash_file_diff, args, repo_path, index, path),
            "changes_stash_push" => forward!(c::changes_stash_push, args, repo_path, message, include_untracked),
            "changes_stash_apply" => forward!(c::changes_stash_apply, args, repo_path, index, pop, restore_index, oid),
            "changes_stash_drop" => forward!(c::changes_stash_drop, args, repo_path, index, oid),
            "changes_stash_restore_file" => forward!(c::changes_stash_restore_file, args, repo_path, index, path, oid),
            // History page
            "history_branches" => forward!(c::history_branches, args, repo_path),
            "history_page" => forward!(c::history_page, args, repo_path, rev, offset, limit, search, author),
            "history_commit_detail" => forward!(c::history_commit_detail, args, repo_path, hash),
            "history_branch_merges" => forward!(c::history_branch_merges, args, repo_path, branch),
            "history_fetch" => forward!(c::history_fetch, args, repo_path),
            "history_sync_status" => forward!(c::history_sync_status, args, repo_path, branch),
            "history_commit_file_diff" => forward!(c::history_commit_file_diff, args, repo_path, hash, path),
            "history_resolve" => forward!(c::history_resolve, args, repo_path, text),
            // These take a `tauri::AppHandle` (tray label updates) and cannot run outside the app.
            "create_profile" | "update_profile" | "delete_profile" | "set_default_profile" | "sparse_clone"
            | "full_clone" => reply_err(format!(
                "`{}` needs a Tauri AppHandle, which the probe cannot supply",
                cmd
            )),
            other => reply_err(format!("unknown command `{}`", other)),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn probe() {
        let op = std::env::var("PROBE_OP").expect("PROBE_OP");
        let a = args();
        let first = a.first().map(|s| s.as_str()).unwrap_or("");
        match op.as_str() {
            // --- the UI's IPC, forwarded (scripts/verify/harness/live.mjs) ---
            "invoke" => invoke().await,
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
            // PROBE_ARGS: "<mode>[,lfs][,autostash]"
            "pull" => {
                let with_lfs = a.iter().skip(1).any(|s| s == "lfs");
                let autostash = a.iter().skip(1).any(|s| s == "autostash");
                show(git_ops::pull(&repo(), PullMode::parse(first).unwrap(), with_lfs, autostash).await)
            }
            "peek" => match crate::peek::peek_remote(&repo()).await {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            "sub_statuses" => match crate::submodules::submodule_statuses(&repo()).await {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            // PROBE_ARGS: "<sha>,<path>"
            "commit_diff" => match git_status::commit_file_diff(&repo(), first, a.get(1).map(|s| s.as_str()).unwrap_or("")).await {
                Ok(d) => emit(&d),
                Err(e) => emit_err(e),
            },
            // PROBE_ARGS: "<sha>"
            "commit_detail" => match crate::git_history::commit_detail(&repo(), first) {
                Ok(d) => emit(&d),
                Err(e) => emit_err(e),
            },
            // PROBE_ARGS: "<branch>"
            "merge_info" => match crate::git_history::branch_merge_info(&repo(), first) {
                Ok(d) => emit(&d),
                Err(e) => emit_err(e),
            },
            op if op.starts_with("tree_") => crate::tree::probe(op, &repo(), &a).await,
            op if op.starts_with("stash_") => crate::stash::probe(op, &repo(), &a).await,
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
            "lfs_files" => match crate::lfs::lfs_files(&repo()).await {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            // PROBE_ARGS: "<path>[,<path>…]" — a path holding a comma has to go
            // through `PROBE_OP=invoke` instead, which takes real JSON.
            "lfs_pull_paths" => show(git_ops::lfs_pull_paths(&repo(), a.clone()).await),
            "lfs_progress" => match crate::lfs::read_progress(&repo()).await {
                Some(p) => emit(&p),
                None => emit(&serde_json::Value::Null),
            },
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
