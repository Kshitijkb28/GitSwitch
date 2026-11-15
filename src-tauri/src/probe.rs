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
    // The command runs in a future of its own (`boxed`), made there: `invoke`
    // keeps only a pointer per command. Built in `invoke` itself, every
    // command's state machine and answer took a stack slot in its debug frame
    // (722 KB, under every live call), and an update's Continue overflowed the
    // test thread's 2 MiB stack.
    macro_rules! forward {
        ($f:path, $args:expr $(, $p:ident)*) => {{
            $( let $p = match arg(&$args, stringify!($p)) { Ok(v) => v, Err(e) => return reply_err(e) }; )*
            boxed(move || $f($($p),*)).await
        }};
    }

    /// Make the command's future, await it and print its answer, all inside a
    /// boxed future: what `make` builds never lies in the caller's frame.
    fn boxed<T, Fut>(make: impl FnOnce() -> Fut + 'static) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>
    where
        T: serde::Serialize,
        Fut: std::future::Future<Output = Result<T, crate::error::AppError>> + 'static,
    {
        Box::pin(async move { reply(make().await) })
    }

    /// Commands that return a plain value rather than a `Result`.
    macro_rules! forward_plain {
        ($f:path, $args:expr $(, $p:ident)*) => {{
            $( let $p = match arg(&$args, stringify!($p)) { Ok(v) => v, Err(e) => return reply_err(e) }; )*
            reply(Ok::<_, crate::error::AppError>(Box::pin($f($($p),*)).await))
        }};
    }

    /// Coverage read while the submodules' pieces are re-applied (a lock
    /// job's last step, a Repair), `rounds` times, in one process as in the
    /// app: the readings that called a sound lock or block broken, and what the
    /// re-applying could not write.
    async fn coverage_while_covering(rounds: usize) {
        let path = repo();
        let mut flagged: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for _ in 0..rounds {
            let state = match Box::pin(git_status::push_state_for(&path)).await {
                Ok(s) => s,
                Err(e) => return emit_err(e),
            };
            let (cov, n) = tokio::join!(
                Box::pin(crate::push_guard::coverage(&path, &state)),
                Box::pin(crate::push_guard::cover_submodules(&path, None))
            );
            if !cov.uncovered.is_empty() {
                flagged.push(cov.summary);
            }
            notes.extend(n);
        }
        emit(&serde_json::json!({ "rounds": rounds, "flagged": flagged, "notes": notes }));
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
            "doctor_apply_fix" => forward!(c::doctor_apply_fix, args, fix),
            "doctor_check_repo_access" => forward!(c::doctor_check_repo_access, args),
            "audit_commits" => forward!(c::audit_commits, args),
            "fix_unpushed_commits" => forward!(c::fix_unpushed_commits, args, repo_path),
            "verify_github_token" => forward!(c::verify_github_token, args, token),
            "github_signin_keep" => forward!(c::github_signin_keep, args, token),
            "gh_register_ssh_key" => forward!(c::gh_register_ssh_key, args, account, key_path),
            "github_signin_forget" => match arg::<String>(&args, "login") {
                Ok(login) => reply(c::github_signin_forget(login)),
                Err(e) => reply_err(e),
            },
            "github_signins" => reply(Ok::<_, crate::error::AppError>(c::github_signins())),
            "repo_accounts" => forward!(c::repo_accounts, args),
            "gh_list_accounts" => forward!(c::gh_list_accounts, args),
            "gh_logout" => forward!(c::gh_logout, args, account),
            "gh_auth_accounts" => forward!(c::gh_auth_accounts, args),
            "gh_auth_switch" => forward!(c::gh_auth_switch, args, host, login),
            "gh_auth_login_start" => forward!(c::gh_auth_login_start, args, host),
            "gh_auth_login_wait" => forward!(c::gh_auth_login_wait, args),
            "gh_auth_login_cancel" => forward!(c::gh_auth_login_cancel, args),
            "apply_git_config" => reply(c::apply_git_config()),
            "history_list_repos" => forward!(c::history_list_repos, args),
            "folder_state" => forward!(c::folder_state, args, path),
            "repo_init" => forward!(c::repo_init, args, path),
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
            "convert_repos_to_ssh" => forward!(c::convert_repos_to_ssh, args, directory),
            // Changes page: operations
            "changes_stage" => forward!(c::changes_stage, args, repo_path, paths, allow_markers),
            "changes_stage_all" => forward!(c::changes_stage_all, args, repo_path),
            "changes_unstage" => forward!(c::changes_unstage, args, repo_path, paths),
            "changes_discard" => forward!(c::changes_discard, args, repo_path, paths),
            "changes_commit" => forward!(c::changes_commit, args, repo_path, message, amend),
            "changes_push" => forward!(c::changes_push, args, repo_path, set_upstream, allow_unpushed_submodules),
            "changes_pull" => forward!(c::changes_pull, args, repo_path, mode, with_lfs, autostash, source),
            "changes_submodule_update" => forward!(c::changes_submodule_update, args, repo_path, paths),
            "changes_submodule_align" => forward!(c::changes_submodule_align, args, repo_path, paths),
            "changes_lfs_pull" => forward!(c::changes_lfs_pull, args, repo_path),
            "changes_lfs_files" => forward!(c::changes_lfs_files, args, repo_path),
            "changes_lfs_pull_paths" => forward!(c::changes_lfs_pull_paths, args, repo_path, paths),
            "changes_lfs_progress" => forward_plain!(c::changes_lfs_progress, args, repo_path),
            "console_entries" => forward_plain!(c::console_entries, args, since_id, repo_path),
            "console_clear" => forward_plain!(c::console_clear, args),
            "changes_continue" => forward!(c::changes_continue, args, repo_path, allow_markers),
            "changes_abort" => forward!(c::changes_abort, args, repo_path),
            // sync
            "changes_sync_plan" => forward!(c::changes_sync_plan, args, repo_path, stash, fetch),
            "changes_sync_run" => forward!(c::changes_sync_run, args, repo_path, stash, bundles, fingerprint, with_lfs),
            "changes_sync_continue" => forward!(c::changes_sync_continue, args, repo_path, allow_markers),
            "changes_sync_abort" => forward!(c::changes_sync_abort, args, repo_path),
            "changes_open_path" => forward!(c::changes_open_path, args, repo_path, path, reveal),
            "changes_conflict_view" => forward!(c::changes_conflict_view, args, repo_path, path),
            "changes_record_pointers" => forward!(c::changes_record_pointers, args, repo_path, paths),
            // push access
            "changes_push_state" => forward!(c::changes_push_state, args, repo_path),
            "changes_push_identity" => forward!(c::changes_push_identity, args, repo_path, refresh),
            "changes_set_push_mode" => forward!(c::changes_set_push_mode, args, repo_path, mode),
            "changes_repair_push_lock" => forward!(c::changes_repair_push_lock, args, repo_path),
            "changes_repair_push_block" => forward!(c::changes_repair_push_block, args, repo_path),
            "changes_push_coverage" => forward!(c::changes_push_coverage, args, repo_path),
            "push_lock_helper_status" => forward_plain!(c::push_lock_helper_status, args),
            "apps_list" => forward!(c::apps_list, args, kind),
            "apps_prepare" => forward!(c::apps_prepare, args, id),
            "apps_uninstall" => forward!(c::apps_uninstall, args, session),
            "apps_status" => forward!(c::apps_status, args, session),
            "apps_check_gone" => forward!(c::apps_check_gone, args, session),
            "apps_scan" => forward!(c::apps_scan, args, session, registry),
            "apps_delete" => forward!(c::apps_delete, args, scan, ids),
            "apps_sessions" => reply(Ok::<_, crate::error::AppError>(c::apps_sessions())),
            "apps_dry_scan" => forward!(crate::apps::dry_scan, args, id),
            // Read-only reviews against a real computer: a saved session's
            // leftovers (no gone check), and what deleting an installed
            // program's files directly would remove.
            "apps_dry_session" => forward!(crate::apps::dry_session, args, path),
            "apps_dry_force" => forward!(crate::apps::dry_force, args, id),
            // Read only: the "uninstalled before" list (after listing the
            // apps, as the page does), and who holds a folder's files.
            "apps_dry_history" => {
                let _ = Box::pin(crate::apps::list("apps".into())).await;
                reply(Ok::<_, crate::error::AppError>(crate::apps::history()))
            }
            // Read only: this computer's adapters, twice a second apart, and its Wi-Fi.
            "netspeed_probe" => {
                let a = crate::netspeed::read();
                std::thread::sleep(std::time::Duration::from_secs(1));
                let b = crate::netspeed::read();
                let prev: std::collections::HashMap<String, [u64; 2]> = a.iter().map(|i| (i.id.clone(), [i.rx, i.tx])).collect();
                let (rates, _) = crate::netspeed::rates_between(&prev, &b, 1.0);
                let wifi = crate::netspeed::wifi_for_probe();
                reply(Ok::<_, crate::error::AppError>(serde_json::json!({ "interfaces": b, "rates": rates, "wifi": wifi })))
            }
            // Read only: the speed as the app works it out, once a second for
            // `seconds` (at most 60), with what moved through the connection in use.
            "netspeed_watch" => {
                let secs = arg::<Option<u64>>(&args, "seconds").ok().flatten().unwrap_or(10).clamp(1, 60);
                reply(Ok::<_, crate::error::AppError>(crate::netspeed::watch_for_probe(secs)))
            }
            // The tab's two calls. Saving without the tray menu's tick, which needs the app.
            "netspeed_snapshot" => {
                crate::netspeed::load_for_probe();
                reply(Ok::<_, crate::error::AppError>(crate::netspeed::snapshot()))
            }
            "netspeed_set_settings" => match arg::<crate::netspeed::Settings>(&args, "settings") {
                Ok(s) => {
                    crate::netspeed::load_for_probe();
                    reply(Ok::<_, crate::error::AppError>(crate::netspeed::set_settings_core(s)))
                }
                Err(e) => reply_err(e),
            },
            // What a start at sign-in decides about the window, for these arguments.
            "netspeed_start_hidden" => {
                crate::netspeed::load_for_probe();
                let argv = arg::<Vec<String>>(&args, "argv").unwrap_or_default();
                let at_login = crate::netspeed::autostart::launched_at_login(&argv);
                reply(Ok::<_, crate::error::AppError>(serde_json::json!({
                    "at_login": at_login,
                    "hidden": crate::netspeed::autostart::start_hidden(at_login, crate::netspeed::keeps_running()),
                })))
            }
            // Read only: this computer's taskbar, its buttons (UI Automation),
            // the free stretches, and where the speed bar would go.
            #[cfg(windows)]
            "netspeed_taskbar" => reply(Ok::<_, crate::error::AppError>(crate::netspeed::taskbar_probe())),
            // The real taskbar speed bar for a few seconds (to look at it), then gone.
            #[cfg(windows)]
            "netspeed_taskbar_show" => {
                let secs = arg::<u64>(&args, "seconds").unwrap_or(4).min(30);
                let light = arg::<bool>(&args, "light").unwrap_or(false);
                reply(Ok::<_, crate::error::AppError>(crate::netspeed::taskbar_show_for(secs, "300 KB/s", "12.4 MB/s", light)))
            }
            // The real taskbar speed bar, hidden and then moved from outside it
            // (as Explorer or a display change might): whether it put itself right.
            #[cfg(windows)]
            "netspeed_taskbar_heal" => reply(Ok::<_, crate::error::AppError>(crate::netspeed::taskbar_heal_check())),
            "apps_users_of" => match arg::<String>(&args, "path") {
                Ok(p) => reply(Ok::<_, crate::error::AppError>(crate::apps::users_of(std::path::Path::new(&p)))),
                Err(e) => reply_err(e),
            },
            "apps_force_remove" => forward!(c::apps_force_remove, args, session),
            "apps_script" => forward!(crate::apps::script, args, steps),
            // The real Trash (no GITSWITCH_TRASH_DIR): what macOS, Windows or the desktop's Trash does with one path.
            "apps_trash" => match arg::<String>(&args, "path") {
                Ok(p) => reply(Ok::<_, crate::error::AppError>(format!("{:?}", crate::apps::to_trash(std::path::Path::new(&p))))),
                Err(e) => reply_err(e),
            },
            "uninstall_plan" => forward_plain!(c::uninstall_plan, args),
            "uninstall_prepare" => forward_plain!(c::uninstall_prepare, args, keep_data),
            // The real removal script, for a stand-in bundle and a process that has already quit.
            "uninstall_start_remover" => {
                let (bundle, pid, keep_data) = match (arg::<String>(&args, "bundle"), arg::<u32>(&args, "pid"), arg::<bool>(&args, "keep_data")) {
                    (Ok(b), Ok(p), Ok(k)) => (b, p, k),
                    _ => return reply_err("bundle, pid and keep_data are needed".to_string()),
                };
                let app = crate::uninstall::macos_removal(&std::path::Path::new(&bundle).join("Contents").join("MacOS").join("gitswitch"), &dirs::home_dir().unwrap_or_default());
                let remove = if keep_data { Vec::new() } else { crate::uninstall::data_folders() };
                reply(crate::uninstall::start_remover(pid, &remove, &app).map(|p| p.to_string_lossy().to_string()))
            }
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
            // In a function of their own: each arm written here adds its debug
            // slots to this frame, which lies under every live call.
            s if s.starts_with("states_") => Box::pin(invoke_states(s.to_string(), args)).await,
            "changes_stash_push" => forward!(c::changes_stash_push, args, repo_path, message, include_untracked),
            "changes_stash_apply" => forward!(c::changes_stash_apply, args, repo_path, index, pop, restore_index, oid),
            "changes_stash_drop" => forward!(c::changes_stash_drop, args, repo_path, index, oid),
            "changes_stash_restore_file" => forward!(c::changes_stash_restore_file, args, repo_path, index, path, oid),
            // History page
            "history_branches" => forward!(c::history_branches, args, repo_path),
            "history_page" => forward!(c::history_page, args, repo_path, rev, offset, limit, search, authors, hide_pointer_only, unpushed_only),
            "history_commit_detail" => forward!(c::history_commit_detail, args, repo_path, hash),
            "history_branch_merges" => forward!(c::history_branch_merges, args, repo_path, branch),
            "history_fetch" => forward!(c::history_fetch, args, repo_path),
            "history_sync_status" => forward!(c::history_sync_status, args, repo_path, branch),
            "history_head" => forward!(c::history_head, args, repo_path),
            "history_authors" => forward!(c::history_authors, args, repo_path, rev),
            "clone_progress" => forward_plain!(c::clone_progress, args, dest_path),
            "clone_size_estimate" => forward_plain!(c::clone_size_estimate, args, url),
            // doctor
            "doctor_check_environment" => forward!(c::doctor_check_environment, args),
            "doctor_check_profiles" => forward!(c::doctor_check_profiles, args),
            "doctor_check_keys" => forward!(c::doctor_check_keys, args),
            "doctor_check_repos" => forward!(c::doctor_check_repos, args),
            "doctor_unset_local" => forward!(c::doctor_unset_local, args, repo_path, key),
            "history_peek" => forward!(c::history_peek, args, repo_path),
            "history_commit_file_diff" => forward!(c::history_commit_file_diff, args, repo_path, hash, path),
            "history_resolve" => forward!(c::history_resolve, args, repo_path, text),
            // No link started the probe: there is never a page waiting.
            "deep_link_take" => reply(Ok::<Option<&str>, crate::error::AppError>(None)),
            "ports_list" => forward!(c::ports_list, args),
            "ports_kill" => forward!(c::ports_kill, args, pid, force, name),
            "ports_container_stop" => forward!(c::ports_container_stop, args, id, project),
            "workspace_info" => forward!(c::workspace_info, args, path),
            "workspace_set_offset" => forward!(c::workspace_set_offset, args, path, offset),
            // Find repositories
            "find_unlisted_repos" => forward!(c::find_unlisted_repos, args, scan_id, extra_roots),
            "find_repos_progress" => {
                let scan_id: String = match arg(&args, "scan_id") { Ok(v) => v, Err(e) => return reply_err(e) };
                reply(Ok::<_, crate::error::AppError>(c::find_repos_progress(scan_id)))
            }
            "find_repos_cancel" => {
                let scan_id: String = match arg(&args, "scan_id") { Ok(v) => v, Err(e) => return reply_err(e) };
                c::find_repos_cancel(scan_id);
                reply(Ok::<_, crate::error::AppError>(()))
            }
            "repo_access_matrix" => forward!(c::repo_access_matrix, args, scan_id, repos),
            "github_repo_context" => forward!(c::github_repo_context, args, repo_path, repo),
            "github_pulls" => forward!(c::github_pulls, args, account, owner, name, state, page),
            "github_pulls_search" => forward!(c::github_pulls_search, args, account, scope, state, repo, page),
            "github_pull_repos" => forward!(c::github_pull_repos, args, account),
            "scan_repos_tracked" => forward!(c::scan_repos_tracked, args, scan_id, root),
            "github_pull" => forward!(c::github_pull, args, account, owner, name, number),
            "github_pull_files" => forward!(c::github_pull_files, args, account, owner, name, number),
            "github_pull_commits" => forward!(c::github_pull_commits, args, account, owner, name, number),
            "github_pull_review" => forward!(c::github_pull_review, args, account, owner, name, number, event, body),
            "github_pull_merge" => forward!(c::github_pull_merge, args, account, owner, name, number, method, sha, title, message, delete_branch),
            "github_collaborators" => forward!(c::github_collaborators, args, account, owner, name),
            "github_collaborator_add" => forward!(c::github_collaborator_add, args, account, owner, name, username, permission),
            "github_collaborator_remove" => forward!(c::github_collaborator_remove, args, account, owner, name, username),
            "github_invitation_cancel" => forward!(c::github_invitation_cancel, args, account, owner, name, id),
            // The commands update the tray too, which needs the app: their cores.
            "profile_folders_add" => forward!(c::profile_folders_add_core, args, changes),
            "profile_folders_remove" => forward!(c::profile_folders_remove_core, args, changes),
            "install_commit_guard" => forward!(c::install_commit_guard, args, repo_path, expected_email),
            // These take a `tauri::AppHandle` (tray label updates) and cannot run outside the app.
            "create_profile" | "update_profile" | "delete_profile" | "set_default_profile" | "sparse_clone"
            | "full_clone" => reply_err(format!(
                "`{}` needs a Tauri AppHandle, which the probe cannot supply",
                cmd
            )),
            other => reply_err(format!("unknown command `{}`", other)),
        }
    }

    /// The States commands, reached through `invoke` (see the arm there).
    async fn invoke_states(cmd: String, args: serde_json::Value) {
        use crate::commands as c;
        match cmd.as_str() {
            "states_list" => forward!(c::states_list, args, repo_path),
            "states_save" => forward!(c::states_save, args, repo_path, slot, name, replace),
            "states_restore" => forward!(c::states_restore, args, repo_path, slot, oid, new_branch, allow_pushed),
            "states_delete" => forward!(c::states_delete, args, repo_path, slot, oid),
            "states_set_max" => forward!(c::states_set_max, args, repo_path, max),
            other => reply_err(format!("unknown command `{}`", other)),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn probe() {
        let op = std::env::var("PROBE_OP").expect("PROBE_OP");
        // Each in a boxed future of its own. Written in this function, every
        // op's temporaries sat in its debug frame (some 700 KB), which then lay
        // under every live `invoke` call too.
        if op == "invoke" {
            // --- the UI's IPC, forwarded (scripts/verify/harness/live.mjs) ---
            Box::pin(invoke()).await;
        } else {
            Box::pin(ops(op)).await;
        }
        // Every clone-progress entry the op set, in order, for the suites to
        // read back: against a local remote a phase lasts milliseconds, so
        // sampling `clone_progress` from outside could miss it.
        if let Ok(path) = std::env::var("PROBE_TRACE_FILE") {
            let trace = crate::sparse::progress_trace().lock().map(|t| t.clone()).unwrap_or_default();
            let _ = std::fs::write(path, serde_json::to_string(&trace).unwrap_or_else(|_| "[]".into()));
        }
        // Every git command the op ran, with the class the runner gave it
        // (scripts/verify/hostile.sh: a passive entry point runs passive
        // commands only).
        if let Ok(path) = std::env::var("PROBE_CLASS_FILE") {
            let ran = crate::git_exec::test_support::ran_all();
            let _ = std::fs::write(path, serde_json::to_string(&ran).unwrap_or_else(|_| "[]".into()));
        }
        // What a poll for this path would read now that the op is over: an
        // entry left behind would open the next download on this one's line.
        if let (Ok(file), Ok(path)) = (std::env::var("PROBE_LEFT_FILE"), std::env::var("PROBE_LEFT_FOR")) {
            let left = crate::sparse::clone_progress(&path);
            let _ = std::fs::write(file, serde_json::to_string(&left).unwrap_or_else(|_| "null".into()));
        }
    }

    /// Every `PROBE_OP` but `invoke`.
    async fn ops(op: String) {
        let a = args();
        let first = a.first().map(|s| s.as_str()).unwrap_or("");
        match op.as_str() {
            // --- identity: a profile written into the sandbox HOME's gitconfig ---
            // first = a JSON file {name, git_name, git_email, ssh_key_path?, directories[]}
            // PROBE_ARGS = "<script>": what is on this Mac, read once, then the
            // script runs (a clone deleted, moved or made behind the app's
            // back), then read again — in one process, as the app does.
            "local_clones_around" => {
                let before = crate::remote_repos::local_clones().await.map(|c| c.clones).unwrap_or_default();
                let _ = std::process::Command::new("sh").arg(first).status();
                let after = crate::remote_repos::local_clones().await.map(|c| c.clones).unwrap_or_default();
                emit(&serde_json::json!({ "before": before, "after": after }));
            }
            "profile_apply" => {
                let text = std::fs::read_to_string(first).expect("profile json file");
                let v: serde_json::Value = serde_json::from_str(&text).expect("profile json");
                let str_of = |k: &str| v[k].as_str().unwrap_or_default().to_string();
                let dirs: Vec<String> = v["directories"].as_array().map(|a| a.iter().filter_map(|d| d.as_str().map(String::from)).collect()).unwrap_or_default();
                let remotes: Vec<String> = v["remote_patterns"].as_array().map(|a| a.iter().filter_map(|d| d.as_str().map(String::from)).collect()).unwrap_or_default();
                match crate::profiles::create_profile(str_of("name"), str_of("git_name"), str_of("git_email"), v["ssh_key_path"].as_str().map(String::from), dirs, v["allow_push"].as_bool().unwrap_or(true), v["signing_enabled"].as_bool().unwrap_or(false), remotes, v["github_login"].as_str().map(String::from), v["push_over_ssh"].as_bool()) {
                    Ok(p) => match crate::git_config::apply_git_config() {
                        Ok(()) => emit(&p),
                        Err(e) => emit_err(e),
                    },
                    Err(e) => emit_err(e),
                }
            }
            // What the app does at start-up: every profile's include file
            // written again from the store.
            "refresh_profile_includes" => match crate::git_config::refresh_profile_includes() {
                Ok(()) => emit(&"refreshed"),
                Err(e) => emit_err(e),
            },
            // A clone running into DEST (PROBE_ARGS "<dest>,<url>"): what the
            // local index and the Repository page's picker say meanwhile, and
            // after it is over.
            "local_clones_while_cloning" => {
                let dest = a.first().cloned().unwrap_or_default();
                let url = a.get(1).cloned().unwrap_or_default();
                let guard = crate::sparse::CloneGuard::hold(std::path::Path::new(&dest), &url);
                let during = crate::remote_repos::local_clones().await.ok();
                let picker: Vec<String> = crate::git_history::list_repos()
                    .await
                    .map(|v| v.into_iter().filter(|r| r.kind == crate::git_history::RepoKind::Repo).map(|r| r.path).collect())
                    .unwrap_or_default();
                drop(guard);
                let after = crate::remote_repos::local_clones().await.ok();
                emit(&serde_json::json!({ "during": during, "picker": picker, "after": after }));
            }
            // Folders added to one profile from many threads at once (PROBE_REPO =
            // the profile id, PROBE_ARGS = the folders): the store must keep
            // every one, whatever order the writes land in.
            "profile_add_concurrently" => {
                let id = std::env::var("PROBE_REPO").unwrap_or_default();
                let threads: Vec<_> = a
                    .iter()
                    .cloned()
                    .map(|dir| {
                        let id = id.clone();
                        std::thread::spawn(move || crate::profiles::add_folders(&id, &[dir]).map_err(|e| e.to_string()))
                    })
                    .collect();
                let errors: Vec<String> = threads.into_iter().filter_map(|t| t.join().ok().and_then(|r| r.err())).collect();
                let dirs = crate::profiles::load_profiles().ok().and_then(|s| s.profiles.into_iter().find(|p| p.id == id)).map(|p| p.directories).unwrap_or_default();
                emit(&serde_json::json!({ "directories": dirs, "errors": errors }));
            }
            // Saves a profile with the folders and email given, as the app's
            // update_profile command does (which also updates the tray, and
            // that needs the app) — Doctor's "Save it as the disk spells it",
            // or an email change the identity guards must follow.
            "profile_update" => {
                let text = std::fs::read_to_string(first).expect("profile json file");
                let v: serde_json::Value = serde_json::from_str(&text).expect("profile json");
                let dirs: Option<Vec<String>> = v["directories"].as_array().map(|a| a.iter().filter_map(|d| d.as_str().map(String::from)).collect());
                let email = v["git_email"].as_str().map(String::from);
                let key = v["ssh_key_path"].as_str().map(String::from);
                let signs = v["signing_enabled"].as_bool();
                match crate::commands::update_profile_core(v["id"].as_str().unwrap_or_default().to_string(), None, None, email, key, dirs, None, signs, None, None, v["push_over_ssh"].as_bool()).await {
                    Ok(p) => emit(&p),
                    Err(e) => emit_err(e),
                }
            }
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
            "stage" => show(git_ops::stage(&repo(), a, false).await),
            "stage_anyway" => show(git_ops::stage(&repo(), a, true).await),
            "stage_all" => show(git_ops::stage_all(&repo()).await),
            "unstage" => show(git_ops::unstage(&repo(), a).await),
            "discard" => show(git_ops::discard(&repo(), a).await),
            "commit" => {
                let amend = a.get(1).map(|s| s == "amend").unwrap_or(false);
                show(git_ops::commit(&repo(), first, amend).await)
            }
            "push" => show(git_ops::push(&repo(), first == "upstream", false).await),
            // PROBE_ARGS: "<mode>[,lfs][,autostash]"
            "pull" => {
                let with_lfs = a.iter().skip(1).any(|s| s == "lfs");
                let autostash = a.iter().skip(1).any(|s| s == "autostash");
                show(git_ops::pull(&repo(), PullMode::parse(first).unwrap(), with_lfs, autostash, None).await)
            }
            // The History page's Fetch, on a blocking thread as the command runs it.
            "history_fetch" => {
                let r = repo();
                match tokio::task::spawn_blocking(move || crate::git_history::fetch_repo(&r)).await {
                    Ok(Ok(res)) => emit(&res),
                    Ok(Err(e)) => emit_err(e),
                    Err(e) => emit_err(e),
                }
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
            "submodule" => show(git_ops::submodule_update(&repo(), None).await),
            "abort" => show(git_ops::abort(&repo()).await),
            "continue" => show(git_ops::continue_op(&repo(), false).await),
            "continue_anyway" => show(git_ops::continue_op(&repo(), true).await),
            // PROBE_ARGS: "stash" | "nostash"
            "sync_plan" => match crate::sync::sync_plan(&repo(), first != "nostash", true).await {
                Ok(p) => emit(&p),
                Err(e) => emit_err(e),
            },
            // PROBE_ARGS: "[nostash][,bundles]" — no fingerprint: the run measures afresh
            "sync_run" => show(crate::sync::sync_run(&repo(), crate::sync::RunOptions { stash: !a.iter().any(|s| s == "nostash"), bundles: a.iter().any(|s| s == "bundles"), with_lfs: a.iter().any(|s| s == "lfs"), fingerprint: Vec::new() }).await),
            "sync_continue" => show(crate::sync::sync_continue(&repo(), false).await),
            "sync_continue_anyway" => show(crate::sync::sync_continue(&repo(), true).await),
            "sync_abort" => show(crate::sync::sync_abort(&repo()).await),
            // PROBE_ARGS: "[<path>[,<path>…]]" — none records every submodule that is ahead
            "record_pointers" => show(crate::sync::record_pointers(&repo(), if a.is_empty() { None } else { Some(&a) }).await),
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
            // Coverage read while the submodules' pieces are re-applied (a lock
            // job's last step, a Repair), in one process as in the app,
            // PROBE_ARGS rounds times: the readings that called a sound lock or
            // block broken, and what the re-applying could not write.
            // Its own function, boxed: written inline, this op's futures sat in
            // this function's debug frame, which lies under every live
            // `invoke` — and an Update's Continue overflowed the 2 MiB stack.
            "coverage_while_covering" => Box::pin(coverage_while_covering(first.parse().unwrap_or(10))).await,
            // The Ports page's refusals for the probe's own process and the
            // drawer's shell: the error text, or null when it was not refused.
            "ports_kill_self" => emit(&crate::ports::kill(std::process::id(), false, "").await.err().map(|e| e.to_string())),
            "ports_kill_shell" => {
                let (tx, _rx) = std::sync::mpsc::channel::<String>();
                let id = crate::terminal::open_with(&repo(), 80, 24, move |chunk| tx.send(chunk).is_ok()).expect("a shell");
                let pid = crate::terminal::shell_pids().into_iter().next().expect("the shell's pid");
                let err = crate::ports::kill(pid, false, "").await.err().map(|e| e.to_string());
                crate::terminal::close(&id);
                emit(&err);
            }
            // What the drawer would export in this folder, and its banner — no shell started.
            "workspace_env" => emit(&crate::workspace::env_for(&repo())),
            // term_run, and also what the shell showed before the line was
            // typed: the workspace banner must be the first thing on screen.
            "term_run_prelude" => {
                let line = a.join(",");
                match crate::terminal::run_once_with_prelude(&repo(), &line, std::time::Duration::from_secs(20)) {
                    Ok((prelude, out)) => emit(&serde_json::json!({ "prelude": prelude, "output": out })),
                    Err(e) => emit_err(e),
                }
            }
            // `gh auth login` from the app, start and wait in one process as the
            // page does: PROBE_ARGS = "" waits it out, "cancel" cancels while
            // waiting. {"start": code or error, "done": login or error}.
            "gh_login" => {
                let said = |r: Result<serde_json::Value, String>| match r {
                    Ok(v) => serde_json::json!({ "ok": true, "value": v }),
                    Err(e) => serde_json::json!({ "ok": false, "error": e }),
                };
                let start = crate::gh_cli::gh_auth_login_start("github.com").await;
                let started = start.is_ok();
                let start = said(start.map(|p| serde_json::to_value(p).unwrap()).map_err(|e| e.to_string()));
                let done = if !started {
                    serde_json::Value::Null
                } else if first == "cancel" {
                    let waiting = tokio::spawn(crate::gh_cli::gh_auth_login_wait());
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    crate::gh_cli::gh_auth_login_cancel();
                    said(waiting.await.unwrap().map(|l| serde_json::to_value(l).unwrap()).map_err(|e| e.to_string()))
                } else {
                    said(crate::gh_cli::gh_auth_login_wait().await.map(|l| serde_json::to_value(l).unwrap()).map_err(|e| e.to_string()))
                };
                emit(&serde_json::json!({ "start": start, "done": done }));
            }
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
            // PROBE_ARGS: the command line to run in a real shell, in a real
            // pty, through the real shim. One process, so open/write/close all
            // happen here rather than across probe invocations.
            "term_run" => {
                let line = a.join(",");
                match crate::terminal::run_once(
                    &repo(),
                    &line,
                    std::time::Duration::from_secs(20),
                ) {
                    Ok(out) => emit(&serde_json::json!({ "output": out })),
                    Err(e) => emit_err(e),
                }
            }
            // PROBE_ARGS = "<cols>x<rows>,<cols>x<rows>,<line>": the line run in a
            // shell opened at the first size, and again after a resize to the
            // second — what the shell itself reports each time.
            "term_resized" => {
                let size = |s: &str| -> (u16, u16) {
                    let (c, r) = s.split_once('x').unwrap_or(("80", "24"));
                    (c.trim().parse().unwrap_or(80), r.trim().parse().unwrap_or(24))
                };
                let first_size = size(first);
                let second_size = size(a.get(1).map(|s| s.as_str()).unwrap_or(""));
                let line = a.iter().skip(2).cloned().collect::<Vec<_>>().join(",");
                match crate::terminal::run_resized(&repo(), &line, first_size, second_size, std::time::Duration::from_secs(20)) {
                    Ok((before, after)) => emit(&serde_json::json!({ "first": before, "second": after })),
                    Err(e) => emit_err(e),
                }
            }
            // Run a real status read — ~18 commands through GitCmd — then dump
            // what was recorded. One process, because the log is in memory.
            "cmdlog" => {
                let _ = crate::git_status::repo_status(&repo()).await;
                emit(&crate::cmdlog::since(0, Some(&repo())))
            }
            "term_shim" => emit(&serde_json::json!({
                "script": crate::terminal::shim_script("/usr/bin/git"),
            })),
            "lfs_progress" => match crate::lfs::read_progress(&repo()) {
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
            "guard_email" => match crate::commit_audit::hooks_dir(std::path::Path::new(&repo())) {
                Ok(h) => emit(&crate::commit_audit::guard_email_in(&h.dir)),
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
            "hpage" => match crate::git_history::history_page(&repo(), "HEAD", 0, 20, None, None, a.iter().any(|s| s == "hide-pointer-only"), a.iter().any(|s| s == "unpushed-only")) {
                Ok(r) => emit(&r),
                Err(e) => emit_err(e),
            },
            other => panic!("unknown op {}", other),
        }
    }
}
