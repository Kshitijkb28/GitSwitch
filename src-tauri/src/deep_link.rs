//! `gitswitch://<page>` links — the VS Code extension's "Open GitSwitch"
//! opens gitswitch://profiles. The app comes forward on the page the link
//! names; only pages listed here are opened, anything else opens Profiles.

use tauri::{AppHandle, Emitter, Manager};

/// The page a link asks for, from an allowlist: never a path taken from it.
pub fn route_for(url: &str) -> &'static str {
    let lower = url.trim().to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("gitswitch:") else { return "/" };
    let page = rest.trim_start_matches('/').split(['/', '?', '#']).next().unwrap_or("");
    match page {
        "" | "profiles" => "/",
        "doctor" => "/doctor",
        "repository" => "/repo",
        _ => "/",
    }
}

/// A link that arrives before the page listens — the one that started the
/// app — would be sent to nobody (an event reaches only listeners already
/// there): it waits here until the page, listening at last, takes it.
#[derive(Default)]
pub struct Pending(std::sync::Mutex<(bool, Option<&'static str>)>);

impl Pending {
    /// The page to send now; `None` when it was kept for the page to take.
    fn hold(&self, route: &'static str) -> Option<&'static str> {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 {
            Some(route)
        } else {
            g.1 = Some(route);
            None
        }
    }

    /// The page listens from now on: the link it missed, if one came.
    pub fn take(&self) -> Option<&'static str> {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        g.0 = true;
        g.1.take()
    }
}

/// Bring the app forward on the page `url` names.
pub fn open(app: &AppHandle, url: &str) {
    crate::tray::show_main(app);
    let route = route_for(url);
    let now = match app.try_state::<Pending>() {
        Some(p) => p.hold(route),
        None => Some(route),
    };
    if let Some(r) = now {
        let _ = app.emit("navigate", r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_opens_a_listed_page_and_nothing_else() {
        assert_eq!(route_for("gitswitch://profiles"), "/");
        assert_eq!(route_for("GITSWITCH://Profiles/"), "/");
        assert_eq!(route_for("gitswitch://"), "/");
        assert_eq!(route_for("gitswitch://doctor?x=1"), "/doctor");
        assert_eq!(route_for("gitswitch://repository"), "/repo");
        // An unknown page, a path, another scheme: Profiles, never what was typed.
        assert_eq!(route_for("gitswitch://settings/../../etc"), "/");
        assert_eq!(route_for("gitswitch:///repo/history"), "/");
        assert_eq!(route_for("https://evil.example/doctor"), "/");
    }

    #[test]
    fn a_link_before_the_page_listens_waits_for_it() {
        let p = Pending::default();
        // The link that started the app: kept, the latest one wins.
        assert_eq!(p.hold("/"), None);
        assert_eq!(p.hold("/doctor"), None);
        // The page listens and takes it, once.
        assert_eq!(p.take(), Some("/doctor"));
        assert_eq!(p.take(), None);
        // Links after that are sent straight away.
        assert_eq!(p.hold("/repo"), Some("/repo"));
        assert_eq!(p.take(), None);
    }
}
