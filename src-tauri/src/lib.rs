mod commands;
mod credentials;
mod error;
mod git_config;
mod github;
mod oauth;
mod profiles;
mod ssh_keys;
mod tray;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
