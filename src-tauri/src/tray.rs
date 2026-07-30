use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Wry,
};

use crate::profiles;

/// Keeps a handle to the "Active: X" menu item so it can be updated when the
/// default profile changes (it used to be computed once at startup and go stale).
pub struct TrayState {
    active_item: MenuItem<Wry>,
}

fn default_profile_name() -> String {
    profiles::load_profiles()
        .ok()
        .and_then(|s| s.profiles.into_iter().find(|p| p.is_default))
        .map(|p| p.name)
        .unwrap_or_else(|| "None".to_string())
}

/// Refresh the tray's "Active: X" label from the current default profile.
pub fn update_active_label(app: &AppHandle) {
    if let Some(state) = app.try_state::<TrayState>() {
        let _ = state
            .active_item
            .set_text(format!("Active: {}", default_profile_name()));
    }
}

pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let show_item = MenuItem::with_id(app, "show", "Show GitSwitch", true, None::<&str>)?;
    let active_item = MenuItem::with_id(
        app,
        "active",
        format!("Active: {}", default_profile_name()),
        false,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&show_item, &active_item, &quit_item])?;

    // Keep the handle so profile commands can refresh the label live.
    app.manage(TrayState {
        active_item: active_item.clone(),
    });

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
