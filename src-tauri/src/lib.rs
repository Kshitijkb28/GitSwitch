mod commands;
mod commit_audit;
mod credentials;
mod doctor;
mod elevate;
mod error;
mod find_clones;
mod gh_api;
mod gh_cli;
mod gh_repo;
mod pulls;
mod git_advice;
mod git_config;
mod git_exec;
mod git_history;
mod git_ops;
mod git_status;
mod cmdlog;
mod lfs;
mod terminal;
mod git_remote;
mod github;
mod oauth;
mod paths;
mod peek;
/// Test-only driver for scripts/verify — never part of the app binary.
#[cfg(test)]
mod probe;
mod profiles;
mod push_guard;
mod push_lock;
mod remote_repos;
mod repo_scan;
mod sparse;
mod spawn;
mod ssh_config;
mod push_identity;

/// sha256 hex of a secret, for cache keys that must not hold the secret itself.
pub(crate) fn lfs_sha(secret: &str) -> String {
    gitswitch_lock_core::sha256_hex(secret.as_bytes())
}
mod submodules;
mod stash;
mod sync;
mod tree;
mod signing;
mod ssh_keys;
mod deep_link;
mod ports;
mod workspace;
mod tray;
mod uninstall;
mod apps;

/// macOS/Linux GUI apps launched from Finder/the dock inherit a minimal PATH that
/// omits Homebrew (`/opt/homebrew/bin`, `/usr/local/bin`). Tools like `gh` live
/// there, so prepend the common locations once at startup — every child process
/// (gh/git/ssh) then inherits the fixed PATH.
#[cfg(unix)]
fn fix_path_env() {
    let extra = [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/home/linuxbrew/.linuxbrew/bin",
        "/usr/bin",
        "/bin",
    ];
    let current = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    for p in extra {
        // Only folders that exist: on macOS `/home` is an automounter trigger,
        // so a PATH entry under it made every child process the app started
        // (thousands of git runs per page) wake automountd and query Open
        // Directory for the Linuxbrew home that is not there.
        if std::path::Path::new(p).is_dir() {
            parts.push(p.to_string());
        }
    }
    for p in current.split(':') {
        if !p.is_empty() && !parts.iter().any(|x| x == p) {
            parts.push(p.to_string());
        }
    }
    std::env::set_var("PATH", parts.join(":"));
}

#[cfg(not(unix))]
fn fix_path_env() {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    fix_path_env();

    let builder = tauri::Builder::default();
    // First of all: a second launch — a gitswitch:// link opened while the
    // app runs — hands its link over (the deep-link plugin routes it) and
    // brings this window forward instead of starting another app.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| tray::show_main(app)));
    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(deep_link::Pending::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_profiles,
            commands::create_profile,
            commands::update_profile,
            commands::delete_profile,
            commands::set_default_profile,
            commands::generate_ssh_key,
            commands::get_public_key,
            commands::test_connection,
            commands::test_connection_with_key,
            commands::list_ssh_keys,
            commands::delete_ssh_key,
            commands::gh_list_accounts,
            commands::gh_get_token,
            commands::gh_logout,
            commands::gh_auth_accounts,
            commands::gh_auth_switch,
            commands::gh_auth_login_start,
            commands::gh_auth_login_wait,
            commands::gh_auth_login_cancel,
            commands::gh_register_ssh_key,
            commands::resolve_key_account,
            commands::convert_repos_to_ssh,
            commands::register_signing_key,
            commands::signing_key_registered,
            commands::allowed_signers_path,
            commands::changes_repo_status,
            commands::changes_stage,
            commands::changes_stage_all,
            commands::changes_unstage,
            commands::changes_discard,
            commands::changes_commit,
            commands::changes_push,
            commands::changes_pull,
            commands::changes_submodules,
            commands::changes_submodule_update,
            commands::changes_submodule_align,
            commands::changes_lfs_status,
            commands::changes_lfs_pull,
            commands::changes_lfs_files,
            commands::changes_lfs_pull_paths,
            commands::changes_lfs_progress,
            commands::console_entries,
            commands::console_clear,
            commands::terminal_open,
            commands::terminal_write,
            commands::terminal_resize,
            commands::terminal_close,
            commands::lfs_available,
            commands::changes_abort,
            commands::changes_continue,
            commands::changes_sync_plan,
            commands::changes_sync_run,
            commands::changes_sync_continue,
            commands::changes_sync_abort,
            commands::changes_open_path,
            commands::changes_conflict_view,
            commands::changes_record_pointers,
            commands::changes_submodule_statuses,
            commands::changes_switch_branch,
            commands::changes_create_branch,
            commands::changes_delete_branch,
            commands::changes_rename_branch,
            commands::changes_undo_commit,
            commands::changes_reset,
            commands::changes_detach,
            commands::changes_revert,
            commands::changes_cherry_pick,
            commands::changes_resolve_side,
            commands::changes_discard_all,
            commands::changes_stash_list,
            commands::changes_stash_show,
            commands::changes_stash_file_diff,
            commands::changes_stash_push,
            commands::changes_stash_apply,
            commands::changes_stash_drop,
            commands::changes_stash_restore_file,
            commands::history_commit_file_diff,
            commands::history_resolve,
            commands::history_peek,
            commands::changes_file_diff,
            commands::changes_push_state,
            commands::changes_set_push_mode,
            commands::changes_repair_push_block,
            commands::changes_push_coverage,
            commands::changes_repair_push_lock,
            commands::push_lock_helper_status,
            commands::push_lock_uninstall,
            commands::uninstall_plan,
            commands::uninstall_prepare,
            commands::uninstall_finish,
            commands::apps_list,
            commands::apps_size,
            commands::apps_prepare,
            commands::apps_uninstall,
            commands::apps_status,
            commands::apps_check_gone,
            commands::apps_scan,
            commands::apps_delete,
            commands::apps_sessions,
            commands::apps_forget,
            commands::apps_open_uninstaller,
            commands::push_lock_fix,
            commands::push_lock_finish_manual,
            commands::doctor_check_push_locks,
            commands::history_fetch,
            commands::history_sync_status,
            commands::history_head,
            commands::history_authors,
            commands::clone_progress,
            commands::clone_size_estimate,
            commands::history_list_repos,
            commands::history_branches,
            commands::history_page,
            commands::history_commit_detail,
            commands::history_branch_merges,
            commands::audit_commits,
            commands::fix_unpushed_commits,
            commands::install_commit_guard,
            commands::uninstall_commit_guard,
            commands::doctor_check_environment,
            commands::doctor_check_profiles,
            commands::doctor_check_keys,
            commands::doctor_check_repos,
            commands::doctor_fix_ssh_config,
            commands::doctor_unset_local,
            commands::doctor_apply_fix,
            commands::doctor_check_repo_access,
            commands::changes_push_identity,
            commands::scan_repos,
            commands::scan_repos_tracked,
            commands::check_repo_access,
            commands::find_unlisted_repos,
            commands::find_repos_progress,
            commands::find_repos_cancel,
            commands::repo_access_matrix,
            commands::github_repo_context,
            commands::github_pulls,
            commands::github_pulls_search,
            commands::github_pull_repos,
            commands::github_pull,
            commands::github_pull_files,
            commands::github_pull_commits,
            commands::github_pull_review,
            commands::github_pull_merge,
            commands::github_collaborators,
            commands::github_collaborator_add,
            commands::github_collaborator_remove,
            commands::github_invitation_cancel,
            commands::profile_folders_add,
            commands::profile_folders_remove,
            commands::sparse_clone,
            commands::full_clone,
            commands::repo_accounts,
            commands::list_remote_repos,
            commands::list_remote_repos_page,
            commands::local_clone_index,
            commands::path_exists,
            commands::ssh_certificate_info,
            commands::clone_destination_status,
            commands::switch_repo_origin,
            commands::sparse_repo_info,
            commands::sparse_set,
            commands::sparse_add,
            commands::github_signin_keep,
            commands::github_signin_forget,
            commands::github_signins,
            commands::apply_git_config,
            commands::get_current_git_config,
            commands::verify_github_token,
            commands::upload_ssh_key_to_github,
            commands::list_github_ssh_keys,
            commands::github_device_code,
            commands::github_poll_token,
            commands::deep_link_take,
            commands::ports_list,
            commands::ports_kill,
            commands::ports_container_stop,
            commands::workspace_info,
            commands::workspace_set_offset,
        ])
        .setup(|app| {
            tray::setup_tray(app.handle())?;
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        deep_link::open(&handle, url.as_str());
                    }
                });
                // Started by a link: the page it names.
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in urls {
                        deep_link::open(app.handle(), url.as_str());
                    }
                }
                // An AppImage is not installed: it registers the scheme itself.
                #[cfg(target_os = "linux")]
                if std::env::var_os("APPIMAGE").is_some() {
                    let _ = app.deep_link().register_all();
                }
            }
            if let Err(e) = git_config::refresh_profile_includes() {
                eprintln!("GitSwitch: the profiles' git config files could not be refreshed: {}", e);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app, event| {
            // A shell started in the terminal drawer must not outlive the
            // window that opened it.
            if let tauri::RunEvent::Exit = event {
                terminal::close_all();
            }
        });
}
