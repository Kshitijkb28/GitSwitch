//! Which leftover belongs to the app that was removed — and only to it.
//!
//! A leftover is a file or folder directly inside one of a few known places
//! (`~/Library/Caches`, `%APPDATA%`, `~/.config`…). It belongs to the removed
//! app only by an exact identifier, never by part of a name:
//!
//! - **High** (ticked): its exact bundle / package / Flatpak id, or that id
//!   plus `.plist`, `.savedState`, `.binarycookies`, `.ShipIt`; an id
//!   recorded from inside the app; one of its own app groups.
//! - **Medium** (not ticked): the id followed by `.something` — never a
//!   sibling product's (`com.google.Chrome.canary` is not Chrome's) — or
//!   exactly one of the app's names.
//!
//! And never when anything else installed could own the same name: another
//! app's id or name, a brand another app's name or publisher starts with, a
//! command still on PATH or a package still installed (Linux), a generic word.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Medium,
    High,
}

/// What identifies the removed app, recorded before it went.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Evidence {
    /// Its bundle id and the ids of what is inside the bundle; its package
    /// name; its Flatpak id; its Store family name.
    pub ids: Vec<String>,
    /// The names its folders are made with: display name, bundle name,
    /// executable, file name.
    pub names: Vec<String>,
    /// macOS app groups from its entitlements.
    pub groups: Vec<String>,
    /// The organisation in its bundle id (com.google.Chrome → google), or
    /// its publisher: a `<vendor>/<name>` folder may be its, the vendor
    /// folder itself never is.
    pub vendor: Option<String>,
}

/// Generic words, frameworks and runtimes many apps share a folder name
/// with: never a leftover of any one app by name.
pub const STOP_WORDS: &[&str] = &[
    // generic
    "app", "apps", "application", "applications", "data", "cache", "caches", "common", "shared", "share", "update", "updates",
    "updater", "helper", "helpers", "plugin", "plugins", "extension", "extensions", "temp", "tmp", "log", "logs", "config",
    "configs", "settings", "preferences", "local", "user", "users", "system", "library", "programs", "program files",
    "documents", "desktop", "downloads", "default", "profile", "profiles", "backup", "backups", "themes", "fonts", "storage",
    "session", "sessions", "state", "database", "service", "services", "agent", "daemon", "tools", "utils", "runtime",
    "runtimes", "framework", "frameworks", "packages", "package cache", "vendor", "install", "installer", "setup",
    "uninstall", "uninstaller", "unins000", "launcher", "client", "main", "start", "crashes", "diagnostics", "cookies",
    "webkit", "keychains", "cloudkit", "fileprovider", "httpstorages",
    // folder-like words that are also app names
    "notes", "photos", "music", "movies", "pictures", "terminal", "browser", "studio", "office", "mail", "calendar",
    "contacts", "messages", "maps", "news", "books", "podcasts", "reminders", "weather", "clock", "camera", "games",
    // vendors and platforms
    "microsoft", "apple", "google", "mozilla", "adobe", "windows", "linux", "ubuntu", "android", "macos", "gnome", "kde",
    "nvidia", "intel", "amd",
    // frameworks and runtimes
    "electron", "qtproject", "cef", "squirrel", "squirreltemp", "shipit", "sparkle", "crashpad", "crashreporter",
    "crashpad_handler", "sentry", "ebwebview", "webview2", "unity", "unity3d", "wine", "dotnet", "nuget", "java", "python",
    "node", "nodejs", "npm", "pip", "ruby", "gems", "go", "rust", "cargo", "gtk", "gtk-3.0", "gtk-4.0", "qt", "fontconfig",
    "pulse", "pipewire", "dconf", "systemd", "dbus", "mesa_shader_cache", "xdg", "autostart", "icons", "mime",
    "recently-used.xbel", "trash", "keyrings", "evolution-data-server", "tracker", "tracker3",
    // where keys and passwords live: never anyone's leftover
    "gnupg", "ssh", "pki", "password-store", "gnome-keyring", "keychain", "aws", "kube", "docker-credential-helpers",
];

/// Windows folders that hold many programs' files, never one's.
pub const WINDOWS_NEVER: &[&str] = &[
    "microsoft", "common files", "package cache", "windowsapps", "packages", "virtualstore", "temp", "programs",
    "d3dscache", "installer", "uninstall information", "windows defender", "windows nt", "windows mail",
    "windows media player", "windows photo viewer", "windows sidebar", "windowspowershell", "internet explorer",
    "modifiablewindowsapps", "reference assemblies", "msbuild", "dotnet", "iis", "iis express", "nvidia corporation",
    "intel", "amd", "realtek", "crashdumps", "connecteddevicesplatform", "comms", "publishers", "peerdistrepub",
    "squirreltemp", "ssh", "regid.1991-06.com.microsoft",
];

/// `<id>.<x>` is a different product when `<x>` is one of these.
pub const VARIANTS: &[&str] = &[
    "beta", "dev", "canary", "nightly", "insiders", "insider", "preview", "alpha", "ce", "ee", "edu", "lite", "pro",
    "setapp", "rc", "test", "debug",
];

/// The suffixes that make an id-named file the app's own.
const ID_FILES: &[&str] = &[".plist", ".savedstate", ".binarycookies", ".shipit"];

/// Everything else installed, normalised the same way as leftovers.
#[derive(Debug, Default, Clone)]
pub struct Others {
    ids: Vec<String>,
    names: HashSet<String>,
    first_words: HashSet<String>,
    groups: HashSet<String>,
    commands: HashSet<String>,
    packages: Vec<String>,
    fold_case: bool,
}

impl Others {
    pub fn new(fold_case: bool) -> Others {
        Others { fold_case, ..Default::default() }
    }

    fn norm(&self, s: &str) -> String {
        norm(s, self.fold_case)
    }

    /// Whether names are compared ignoring case (macOS, Windows) or not (Linux).
    pub fn fold_case(&self) -> bool {
        self.fold_case
    }

    /// Another installed app: its ids, names (and the same without spaces), publisher, groups.
    pub fn add_app(&mut self, ids: &[String], names: &[String], publisher: Option<&str>, groups: &[String]) {
        for id in ids.iter().filter(|i| !i.trim().is_empty()) {
            let n = self.norm(id);
            if !self.ids.contains(&n) {
                self.ids.push(n);
            }
        }
        for name in names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
            let n = self.norm(name);
            self.names.insert(n.replace(' ', ""));
            if let Some(first) = n.split_whitespace().next() {
                self.first_words.insert(first.to_string());
            }
            self.names.insert(n);
        }
        if let Some(p) = publisher.map(str::trim).filter(|p| !p.is_empty()) {
            let n = self.norm(p);
            if let Some(first) = n.split_whitespace().next() {
                self.first_words.insert(first.to_string());
            }
            self.names.insert(n);
        }
        for g in groups {
            let n = self.norm(g);
            self.groups.insert(n);
        }
    }

    /// Linux: a command on PATH.
    pub fn add_command(&mut self, name: &str) {
        let n = self.norm(name);
        self.commands.insert(n);
    }

    /// Linux: an installed package.
    pub fn add_package(&mut self, name: &str) {
        let n = self.norm(name);
        self.packages.push(n);
    }

    pub fn has_group(&self, g: &str) -> bool {
        self.groups.contains(&self.norm(g))
    }

    /// Some other installed thing answers to this name.
    fn claims_name(&self, n: &str) -> Option<&'static str> {
        if self.names.contains(n) || self.ids.iter().any(|i| i == n) {
            return Some("another installed app has this name");
        }
        if self.first_words.contains(n) {
            return Some("another installed app's name starts with it");
        }
        if self.commands.contains(n) {
            return Some("a command with this name is still installed");
        }
        if self.packages.iter().any(|p| p == n || p.starts_with(&format!("{}-", n))) {
            return Some("a package with this name is still installed");
        }
        None
    }

    /// The longest other id that `n` is, or starts with followed by a dot.
    fn longest_owner(&self, n: &str) -> Option<&str> {
        self.ids.iter().filter(|o| n == o.as_str() || n.starts_with(&format!("{}.", o))).max_by_key(|o| o.len()).map(|o| o.as_str())
    }
}

pub fn norm(s: &str, fold_case: bool) -> String {
    let t = s.trim();
    if fold_case {
        t.to_lowercase()
    } else {
        t.to_string()
    }
}

/// A name that may stand for one app: four characters or more, not a
/// generic word, not only digits and punctuation.
pub fn usable_name(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    n.chars().count() >= 4 && !STOP_WORDS.contains(&n.as_str()) && n.chars().any(|c| c.is_alphabetic())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Match {
    pub confidence: Confidence,
    pub reason: String,
}

/// Which kinds of match a place allows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Allow {
    /// Ids only (Preferences, Containers, LaunchAgents, Cookies…).
    Ids,
    /// Ids and the app's names (Application Support, Caches, ~/.config…).
    IdsAndNames,
    /// App groups only (Group Containers).
    Groups,
}

/// Whether the entry `entry` (a file or folder name inside a known place)
/// is the removed app's, and how sure that is.
pub fn match_entry(entry: &str, ev: &Evidence, others: &Others, allow: Allow) -> Option<Match> {
    let fold = others.fold_case;
    let n = norm(entry, fold);
    if n.is_empty() || n == "." || n == ".." {
        return None;
    }
    if allow == Allow::Groups {
        let g = ev.groups.iter().find(|g| norm(g, fold) == n)?;
        if others.has_group(g) {
            return None;
        }
        return Some(Match { confidence: Confidence::High, reason: format!("its app group {}", g) });
    }
    // Ids, the longest first: a nested id is more specific than the bundle's.
    let mut ids: Vec<&String> = ev.ids.iter().filter(|i| i.trim().len() >= 3).collect();
    ids.sort_by_key(|i| std::cmp::Reverse(i.len()));
    for id in ids {
        let i = norm(id, fold);
        // Another installed app's id that is this entry, or longer and matching, wins.
        if let Some(owner) = others.longest_owner(&n) {
            if owner.len() >= i.len() {
                return None;
            }
        }
        if n == i {
            return Some(Match { confidence: Confidence::High, reason: format!("named after its id {}", id) });
        }
        for suffix in ID_FILES {
            let s = norm(suffix, fold);
            if n == format!("{}{}", i, s) {
                return Some(Match { confidence: Confidence::High, reason: format!("{}{}", id, suffix) });
            }
        }
        if let Some(rest) = n.strip_prefix(&format!("{}.", i)) {
            let first = rest.split('.').next().unwrap_or("").to_lowercase();
            if VARIANTS.contains(&first.as_str()) {
                continue;
            }
            return Some(Match { confidence: Confidence::Medium, reason: format!("starts with its id {}", id) });
        }
    }
    if allow != Allow::IdsAndNames {
        return None;
    }
    for name in ev.names.iter().filter(|n| usable_name(n)) {
        let exact = norm(name, fold);
        let squeezed = exact.replace(' ', "");
        if n != exact && n != squeezed {
            continue;
        }
        // The vendor's folder is never one app's — unless the vendor is the
        // app (org.libreoffice → LibreOffice).
        let vendor_folder = ev.vendor.as_deref().is_some_and(|v| norm(v, fold) == n) && !ev.names.iter().any(|x| norm(x, fold) == n || norm(x, fold).replace(' ', "") == n);
        if !usable_name(&n) || vendor_folder || others.claims_name(&n).is_some() {
            return None;
        }
        return Some(Match { confidence: Confidence::Medium, reason: format!("named like the app ({})", name.trim()) });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn chrome() -> Evidence {
        Evidence { ids: s(&["com.google.Chrome", "com.google.Chrome.helper"]), names: s(&["Google Chrome", "Chrome"]), groups: vec![], vendor: Some("google".into()) }
    }

    #[test]
    fn the_exact_id_and_its_own_files_are_high() {
        let o = Others::new(true);
        let ev = chrome();
        for e in ["com.google.Chrome", "com.google.Chrome.plist", "com.google.Chrome.savedState", "com.google.Chrome.binarycookies", "COM.GOOGLE.CHROME"] {
            assert_eq!(match_entry(e, &ev, &o, Allow::Ids).map(|m| m.confidence), Some(Confidence::High), "{e}");
        }
        assert_eq!(match_entry("com.google.Chrome.helper", &ev, &o, Allow::Ids).unwrap().reason, "named after its id com.google.Chrome.helper", "a nested id is its own, exactly");
    }

    #[test]
    fn a_sibling_product_or_a_longer_installed_id_is_never_this_apps() {
        let mut o = Others::new(true);
        let ev = chrome();
        for e in ["com.google.Chrome.canary", "com.google.Chrome.beta.plist", "com.google.Chrome.dev", "com.google.ChromeX"] {
            assert_eq!(match_entry(e, &ev, &o, Allow::Ids), None, "{e}");
        }
        assert_eq!(match_entry("com.google.Chrome.framework", &ev, &o, Allow::Ids).map(|m| m.confidence), Some(Confidence::Medium), "another suffix: maybe, unticked");
        o.add_app(&s(&["com.google.Chrome.framework"]), &s(&["Chrome Framework Tool"]), None, &[]);
        assert_eq!(match_entry("com.google.Chrome.framework", &ev, &o, Allow::Ids), None, "an installed app's own id wins");
        assert_eq!(match_entry("com.google.Chrome.framework.plist", &ev, &o, Allow::Ids), None, "…and what starts with it");
    }

    #[test]
    fn names_are_exact_never_generic_and_never_another_apps() {
        let mut o = Others::new(true);
        let ev = Evidence { ids: s(&["com.tinyspeck.slackmacgap"]), names: s(&["Slack", "Electron"]), groups: vec![], vendor: Some("tinyspeck".into()) };
        assert_eq!(match_entry("Slack", &ev, &o, Allow::IdsAndNames).map(|m| m.confidence), Some(Confidence::Medium));
        assert_eq!(match_entry("Slack", &ev, &o, Allow::Ids), None, "names only where the place allows them");
        assert_eq!(match_entry("Slack Helper", &ev, &o, Allow::IdsAndNames), None, "never part of a name");
        assert_eq!(match_entry("Electron", &ev, &o, Allow::IdsAndNames), None, "a framework's name is everyone's");
        assert_eq!(match_entry("tinyspeck", &ev, &o, Allow::IdsAndNames), None, "the vendor folder itself never");
        o.add_app(&s(&["com.slack.huddles"]), &s(&["Slack Huddles"]), None, &[]);
        assert_eq!(match_entry("Slack", &ev, &o, Allow::IdsAndNames), None, "another app's name starts with it");
        let zoom = Evidence { ids: s(&["us.zoom.xos"]), names: s(&["zoom.us", "Zoom"]), groups: vec![], vendor: None };
        let mut o = Others::new(true);
        o.add_app(&[], &s(&["Firefox"]), Some("Zoom Video Communications"), &[]);
        assert_eq!(match_entry("Zoom", &zoom, &o, Allow::IdsAndNames), None, "a publisher starting with it");
        let lo = Evidence { ids: s(&["org.libreoffice.script"]), names: s(&["LibreOffice", "soffice"]), groups: vec![], vendor: Some("libreoffice".into()) };
        assert!(match_entry("LibreOffice", &lo, &Others::new(true), Allow::IdsAndNames).is_some(), "a vendor that is the app itself");
        let short = Evidence { ids: vec![], names: s(&["VLC"]), groups: vec![], vendor: None };
        assert_eq!(match_entry("VLC", &short, &Others::new(true), Allow::IdsAndNames), None, "three letters are too few to be sure");
    }

    #[test]
    fn a_name_without_its_spaces_is_the_same_name() {
        let ev = Evidence { ids: vec![], names: s(&["Sublime Text"]), groups: vec![], vendor: None };
        assert!(match_entry("SublimeText", &ev, &Others::new(true), Allow::IdsAndNames).is_some());
        assert!(match_entry("Sublime Text", &ev, &Others::new(true), Allow::IdsAndNames).is_some());
        assert!(match_entry("Sublime", &ev, &Others::new(true), Allow::IdsAndNames).is_none());
    }

    #[test]
    fn groups_only_the_apps_own_and_only_when_no_other_app_has_them() {
        let ev = Evidence { ids: s(&["com.docker.docker"]), names: vec![], groups: s(&["group.com.docker"]), vendor: None };
        let mut o = Others::new(true);
        assert_eq!(match_entry("group.com.docker", &ev, &o, Allow::Groups).map(|m| m.confidence), Some(Confidence::High));
        assert_eq!(match_entry("UBF8T346G9.Office", &ev, &o, Allow::Groups), None, "a team's shared group is not in its entitlements");
        o.add_app(&s(&["com.docker.extension"]), &[], None, &s(&["group.com.docker"]));
        assert_eq!(match_entry("group.com.docker", &ev, &o, Allow::Groups), None, "shared with an installed app");
    }

    #[test]
    fn linux_is_case_sensitive_and_respects_commands_and_packages() {
        // A .deb's evidence holds names, not ids: a home folder named after a
        // package is only ever a maybe.
        let ev = Evidence { ids: vec![], names: s(&["evolution", "Evolution"]), groups: vec![], vendor: None };
        let mut o = Others::new(false);
        assert_eq!(match_entry("evolution", &ev, &o, Allow::IdsAndNames).map(|m| m.confidence), Some(Confidence::Medium));
        o.add_package("evolution-data-server");
        assert_eq!(match_entry("evolution", &ev, &o, Allow::IdsAndNames), None, "evolution-data-server is still installed and uses it");
        let ev = Evidence { ids: vec![], names: s(&["mousepad", "Mousepad"]), groups: vec![], vendor: None };
        let mut o = Others::new(false);
        assert!(match_entry("Mousepad", &ev, &o, Allow::IdsAndNames).is_some());
        assert!(match_entry("MOUSEPAD", &ev, &o, Allow::IdsAndNames).is_none(), "case counts on Linux");
        o.add_command("mousepad");
        assert!(match_entry("mousepad", &ev, &o, Allow::IdsAndNames).is_none(), "still on PATH: someone else's");
        let keys = Evidence { ids: vec![], names: s(&["gnupg", "ssh", "password-store"]), groups: vec![], vendor: None };
        for n in ["gnupg", "ssh", "password-store"] {
            assert!(match_entry(n, &keys, &Others::new(false), Allow::IdsAndNames).is_none(), "{n}: where keys live is never a leftover");
        }
    }

    #[test]
    fn generic_names_are_never_usable() {
        for n in ["Code2", "Data", "data", "Electron", "Google", "VLC", "123456", "Updater"] {
            let ok = usable_name(n);
            assert_eq!(ok, n == "Code2", "{n}");
        }
    }
}
