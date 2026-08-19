mod commands;
mod commit_audit;
mod credentials;
mod doctor;
mod error;
mod gh_cli;
mod git_config;
mod git_history;
mod git_remote;
mod github;
mod oauth;
mod paths;
mod profiles;
mod repo_scan;
mod sparse;
mod signing;
mod ssh_keys;
mod tray;

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
        parts.push(p.to_string());
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

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
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
            commands::gh_register_ssh_key,
            commands::resolve_key_account,
            commands::convert_repos_to_ssh,
            commands::register_signing_key,
            commands::signing_key_registered,
            commands::allowed_signers_path,
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
            commands::scan_repos,
            commands::check_repo_access,
            commands::sparse_clone,
            commands::sparse_repo_info,
            commands::sparse_set,
            commands::sparse_add,
            commands::store_github_token,
            commands::get_github_token,
            commands::apply_git_config,
            commands::get_current_git_config,
            commands::verify_github_token,
            commands::upload_ssh_key_to_github,
            commands::list_github_ssh_keys,
            commands::github_device_code,
            commands::github_poll_token,
        ])
        .setup(|app| {
            tray::setup_tray(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
