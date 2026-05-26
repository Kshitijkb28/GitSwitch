use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};

use crate::profiles;

pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let profiles_store = profiles::load_profiles().ok();
    let default_name = profiles_store
        .as_ref()
        .and_then(|s| s.profiles.iter().find(|p| p.is_default))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "None".to_string());

    let show_item = MenuItem::with_id(app, "show", "Show GitSwitch", true, None::<&str>)?;
    let active_item = MenuItem::with_id(
        app,
        "active",
        format!("Active: {}", default_name),
        false,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&show_item, &active_item, &quit_item])?;

    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .tooltip("GitSwitch")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}
