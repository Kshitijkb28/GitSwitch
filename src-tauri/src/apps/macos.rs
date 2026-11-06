//! macOS: the `.app` bundles in /Applications and ~/Applications; removing
//! one moves it to the Trash; its leftovers are in ~/Library (to the Trash)
//! and /Library (listed for the administrator).
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use super::matcher::{Allow, Confidence, Evidence, Match, Others};
use super::{home, sandboxed, sys, Action, Candidate, InstalledApp, Place, Prepared, Record, Session, TrashOutcome};
use crate::error::AppError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SELF_ID: &str = "com.gitswitch.app";

/// What an Info.plist says about a bundle.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct BundleInfo {
    pub id: Option<String>,
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub executable: Option<String>,
    pub version: Option<String>,
}

pub(crate) fn read_info(bundle: &Path) -> BundleInfo {
    let Ok(v) = plist::Value::from_file(bundle.join("Contents").join("Info.plist")) else { return BundleInfo::default() };
    info_from(&v)
}

pub(crate) fn info_from(v: &plist::Value) -> BundleInfo {
    let d = v.as_dictionary();
    let s = |k: &str| d.and_then(|d| d.get(k)).and_then(|x| x.as_string()).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);
    BundleInfo {
        id: s("CFBundleIdentifier"),
        name: s("CFBundleName"),
        display_name: s("CFBundleDisplayName"),
        executable: s("CFBundleExecutable"),
        version: s("CFBundleShortVersionString").or_else(|| s("CFBundleVersion")),
    }
}

/// The organisation in a bundle id: com.google.Chrome → google.
pub(crate) fn vendor_of(id: &str) -> Option<String> {
    let parts: Vec<&str> = id.split('.').collect();
    (parts.len() >= 3).then(|| parts[1].to_string()).filter(|v| v.len() >= 3)
}

/// Safari in /Applications is a link into the system's sealed volume,
/// flagged `restricted`: not something to list.
fn restricted(md: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        const SF_RESTRICTED: u32 = 0x0008_0000;
        md.st_flags() & SF_RESTRICTED != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = md;
        false
    }
}

/// Every `.app` directly in a folder, or one folder down (vendor folders).
fn bundles_in(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(md) = e.path().symlink_metadata() else { continue };
        if md.file_type().is_symlink() || restricted(&md) || !md.is_dir() {
            continue;
        }
        let p = e.path();
        if p.extension().is_some_and(|x| x == "app") {
            out.push(p);
        } else if depth > 0 && !e.file_name().to_string_lossy().starts_with('.') {
            bundles_in(&p, depth - 1, out);
        }
    }
}

pub(crate) fn app_folders() -> Vec<PathBuf> {
    vec![sys("/Applications"), home().join("Applications")]
}

fn listed_bundles() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for f in app_folders() {
        bundles_in(&f, 1, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Homebrew casks: the app file name each one installed → its token.
fn casks() -> HashMap<String, String> {
    let mut out = HashMap::new();
    for room in [sys("/opt/homebrew/Caskroom"), sys("/usr/local/Caskroom")] {
        let Ok(rd) = std::fs::read_dir(&room) else { continue };
        for e in rd.flatten() {
            let token = e.file_name().to_string_lossy().to_string();
            let mut files = Vec::new();
            walk_files(&e.path().join(".metadata"), 4, &mut files);
            for f in files.into_iter().filter(|f| f.extension().is_some_and(|x| x == "json" || x == "rb")) {
                let Ok(text) = std::fs::read_to_string(&f) else { continue };
                for name in app_names_in(&text) {
                    out.entry(name).or_insert_with(|| token.clone());
                }
            }
        }
    }
    out
}

fn walk_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth > 0 {
            walk_files(&e.path(), depth - 1, out);
        } else if ft.is_file() && e.metadata().is_ok_and(|m| m.len() < 512 * 1024) {
            out.push(e.path());
        }
    }
}

/// `"Foo Bar.app"` strings in a cask's file.
pub(crate) fn app_names_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices(".app\"") {
        if let Some(start) = text[..i].rfind('"') {
            let name = &text[start + 1..i + 4];
            if !name.is_empty() && !name.contains('/') && !name.contains('\n') && !out.contains(&name.to_string()) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The app's own uninstaller: in the bundle, or beside it in a vendor folder.
fn vendor_uninstaller(bundle: &Path) -> Option<PathBuf> {
    let contents = bundle.join("Contents");
    for dir in [contents.join("MacOS"), contents.join("Resources")] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n.contains("uninstall") && (n.ends_with(".app") || e.path().is_file()) {
                return Some(e.path());
            }
        }
    }
    let parent = bundle.parent()?;
    if app_folders().iter().any(|f| f == parent) {
        return None;
    }
    let rd = std::fs::read_dir(parent).ok()?;
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_lowercase();
        if n.starts_with("uninstall") && n.ends_with(".app") {
            return Some(e.path());
        }
    }
    None
}

fn non_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut r| r.next().is_some()).unwrap_or(false)
}

/// Where `xcode-select` points: GitSwitch's git comes from there.
fn developer_dir() -> Option<PathBuf> {
    if sandboxed() {
        return None;
    }
    let out = crate::spawn::command("xcode-select").arg("-p").output().ok()?;
    out.status.success().then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// One of Apple's own apps (Safari, Xcode, Keynote…): the System apps tab.
pub(crate) fn apple_app(bundle_id: &str) -> bool {
    bundle_id.to_lowercase().starts_with("com.apple.")
}

pub fn list() -> Result<super::Listing, AppError> {
    let casks = casks();
    let dev = developer_dir();
    let mut rows = Vec::new();
    for bundle in listed_bundles() {
        let info = read_info(&bundle);
        let file = bundle.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let file_name = bundle.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let id = info.id.clone().unwrap_or_default();
        let app_store = bundle.join("Contents").join("_MASReceipt").exists();
        let cask = casks.get(&file_name).cloned();
        let uninstaller = vendor_uninstaller(&bundle);
        let mut notes = Vec::new();
        let contents = bundle.join("Contents");
        if non_empty(&contents.join("Library").join("LaunchServices")) || non_empty(&contents.join("Library").join("SystemExtensions")) {
            notes.push("Installs a system helper or extension — use its own uninstaller if it has one.".to_string());
        }
        if let Some(u) = &uninstaller {
            notes.push(format!("Has its own uninstaller: {}", u.display()));
        }
        if let Some(t) = &cask {
            notes.push(format!("Installed with Homebrew — `brew uninstall --cask {}` also removes Homebrew's record of it.", t));
        }
        let blocked = dev
            .as_ref()
            .filter(|d| d.starts_with(&bundle))
            .map(|_| "GitSwitch's git comes from it (xcode-select points into it).".to_string());
        let md = std::fs::symlink_metadata(&bundle).ok();
        rows.push((
            InstalledApp {
                id: format!("mac:{}", bundle.display()),
                name: file.clone(),
                version: info.version.clone(),
                publisher: None,
                location: Some(bundle.to_string_lossy().to_string()),
                source: if app_store {
                    "app-store".into()
                } else if cask.is_some() {
                    "homebrew".into()
                } else {
                    "app".into()
                },
                size: None,
                date: md.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64),
                notes,
                blocked,
                is_self: id.eq_ignore_ascii_case(SELF_ID),
                system: apple_app(&id).then(|| "Apple's own app".to_string()),
            },
            Record::MacBundle { bundle: bundle.clone(), bundle_id: id, cask, vendor_uninstaller: uninstaller, app_store },
        ));
    }
    Ok((rows, Vec::new()))
}

/// Bundle ids running now (`lsappinfo list`).
pub(crate) fn running_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("bundleID=\""))
        .filter_map(|r| r.split('"').next())
        .map(str::to_string)
        .collect()
}

fn is_running(bundle: &Path, id: &str) -> bool {
    if sandboxed() {
        return false;
    }
    if !id.is_empty() {
        if let Ok(out) = crate::spawn::command("lsappinfo").arg("list").output() {
            if running_ids(&String::from_utf8_lossy(&out.stdout)).iter().any(|r| r.eq_ignore_ascii_case(id)) {
                return true;
            }
        }
    }
    let prefix = format!("{}/", bundle.display());
    crate::spawn::command("ps")
        .args(["-axo", "comm="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim().starts_with(&prefix)))
        .unwrap_or(false)
}

/// `codesign -dv`'s TeamIdentifier ("not set" for Apple's and ad-hoc apps).
pub(crate) fn team_of(stderr: &str) -> Option<String> {
    stderr.lines().filter_map(|l| l.strip_prefix("TeamIdentifier=")).map(str::trim).find(|t| !t.is_empty() && *t != "not set").map(str::to_string)
}

/// The app groups in an entitlements plist.
pub(crate) fn groups_of(entitlements_xml: &[u8]) -> Vec<String> {
    let Ok(v) = plist::Value::from_reader_xml(entitlements_xml) else { return Vec::new() };
    v.as_dictionary()
        .and_then(|d| d.get("com.apple.security.application-groups"))
        .and_then(|g| g.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_string().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn codesign_facts(bundle: &Path) -> (Option<String>, Vec<String>) {
    if sandboxed() {
        return (None, Vec::new());
    }
    let team = crate::spawn::command("codesign").arg("-dv").arg(bundle).output().ok().and_then(|o| team_of(&String::from_utf8_lossy(&o.stderr)));
    let groups = crate::spawn::command("codesign")
        .args(["-d", "--entitlements", "-", "--xml"])
        .arg(bundle)
        .output()
        .ok()
        .map(|o| groups_of(&o.stdout))
        .unwrap_or_default();
    (team, groups)
}

/// Ids of what is inside the bundle (login items, helpers, extensions,
/// XPC services, plug-ins, nested apps; not frameworks, which are shared
/// code with other apps' ids) and the launchd labels it brings.
pub(crate) fn inner_ids(bundle: &Path) -> Vec<String> {
    let c = bundle.join("Contents");
    let mut out = Vec::new();
    let dirs = [
        c.join("Library").join("LoginItems"),
        c.join("Library").join("SystemExtensions"),
        c.join("XPCServices"),
        c.join("PlugIns"),
        c.join("Helpers"),
        c.join("Applications"),
        c.join("MacOS"),
    ];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.join("Contents").join("Info.plist").is_file() {
                if let Some(id) = read_info(&p).id {
                    out.push(id);
                }
            }
        }
    }
    // Privileged helpers: the file name is the label.
    if let Ok(rd) = std::fs::read_dir(c.join("Library").join("LaunchServices")) {
        for e in rd.flatten() {
            out.push(e.file_name().to_string_lossy().to_string());
        }
    }
    for d in [c.join("Library").join("LaunchAgents"), c.join("Library").join("LaunchDaemons")] {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if let Some(label) = plist::Value::from_file(e.path()).ok().and_then(|v| v.as_dictionary().and_then(|d| d.get("Label")).and_then(|l| l.as_string()).map(str::to_string)) {
                out.push(label);
            }
        }
    }
    out.retain(|i| i.contains('.') && i.len() >= 5);
    out.sort();
    out.dedup();
    out
}

/// The names of the apps inside the bundle (Docker.app holds "Docker Desktop.app").
pub(crate) fn inner_app_names(bundle: &Path) -> Vec<String> {
    let c = bundle.join("Contents");
    let mut out = Vec::new();
    for d in [c.join("MacOS"), c.join("Applications"), c.join("Helpers"), c.join("Library").join("LoginItems")] {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "app") {
                if let Some(stem) = p.file_stem() {
                    out.push(stem.to_string_lossy().to_string());
                }
                if let Some(n) = read_info(&p).name {
                    out.push(n);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn receipts(bundle: &Path) -> Vec<String> {
    if sandboxed() {
        return Vec::new();
    }
    crate::spawn::command("pkgutil")
        .arg("--file-info")
        .arg(bundle)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.strip_prefix("pkgid: ")).map(|s| s.trim().to_string()).collect())
        .unwrap_or_default()
}

pub fn prepare(app: &InstalledApp, rec: &Record) -> Result<(Prepared, Evidence, Vec<String>), AppError> {
    let Record::MacBundle { bundle, bundle_id, cask, vendor_uninstaller, app_store } = rec else {
        return Err(AppError::Config("Not a macOS app.".into()));
    };
    if !bundle.exists() {
        return Err(AppError::Config(format!("{} is no longer there — list the apps again.", bundle.display())));
    }
    let info = read_info(bundle);
    let (team, groups) = codesign_facts(bundle);
    let mut ids = vec![bundle_id.clone()];
    ids.extend(inner_ids(bundle));
    ids.retain(|i| !i.is_empty());
    ids.dedup();
    let file = bundle.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut names = vec![file.clone()];
    for n in [info.name.clone(), info.display_name.clone(), info.executable.clone()].into_iter().flatten().chain(inner_app_names(bundle)) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let ev = Evidence { ids: ids.clone(), names: names.clone(), groups: groups.clone(), vendor: vendor_of(bundle_id) };
    let mut facts = vec![format!("Bundle id: {}", if bundle_id.is_empty() { "none" } else { bundle_id })];
    if ids.len() > 1 {
        facts.push(format!("Inside it: {}", ids[1..].join(", ")));
    }
    facts.push(format!("Developer team: {}", team.as_deref().unwrap_or("not set (Apple's own, or not signed by a developer)")));
    if !groups.is_empty() {
        facts.push(format!("App groups: {}", groups.join(", ")));
    }
    let pkgs = receipts(bundle);
    if !pkgs.is_empty() {
        facts.push(format!("Installed by a package: {}", pkgs.join(", ")));
    }
    let mut warnings = Vec::new();
    let c = bundle.join("Contents").join("Library");
    if non_empty(&c.join("LaunchServices")) || non_empty(&c.join("SystemExtensions")) {
        warnings.push("It installed a privileged helper or a system extension. Moving it to the Trash leaves those in place — its own uninstaller removes them.".into());
    }
    if *app_store {
        warnings.push("From the App Store: if macOS refuses, delete it from Launchpad (hold the icon, then ✕).".into());
    }
    if let Some(t) = cask {
        warnings.push(format!("Installed with Homebrew: `brew uninstall --cask {}` removes it and Homebrew's record of it.", t));
    }
    if !writable_parent(bundle) {
        warnings.push("It belongs to the administrator: GitSwitch can't move it; Finder can (it asks for your password).".into());
    }
    if bundle_id.is_empty() {
        warnings.push("It has no bundle id, so only folders named exactly like it can be found afterwards.".into());
    }
    let prepared = Prepared {
        method: format!("{} is moved to the Trash.", bundle.display()),
        command: None,
        facts,
        running: is_running(bundle, bundle_id),
        warnings,
        vendor_uninstaller: vendor_uninstaller.as_ref().map(|u| u.to_string_lossy().to_string()),
        blocked: app.blocked.clone(),
        ..Default::default()
    };
    Ok((prepared, ev, Vec::new()))
}

fn writable_parent(p: &Path) -> bool {
    #[cfg(unix)]
    {
        p.parent().map(super::writable).unwrap_or(false) && super::writable(p)
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        true
    }
}

pub async fn uninstall(s: &mut Session) -> Result<(), AppError> {
    let Record::MacBundle { bundle, bundle_id, .. } = &s.record else {
        return Err(AppError::Config("Not a macOS app.".into()));
    };
    if is_running(bundle, bundle_id) {
        return Err(AppError::Config(format!("{} is running. Quit it first, then try again.", s.app.name)));
    }
    match super::to_trash(bundle) {
        TrashOutcome::Trashed | TrashOutcome::Gone => {}
        TrashOutcome::NeedsAdmin(_) => {
            return Err(AppError::Config(format!(
                "{} belongs to the administrator, so GitSwitch can't move it. In Finder, drag it to the Trash — Finder asks for your password — then press \"It's gone — look for leftovers\".",
                s.app.name
            )))
        }
        TrashOutcome::Protected(_) => {
            return Err(AppError::Config(format!(
                "macOS stopped GitSwitch from removing another developer's app (App Management). Drag {} to the Trash in Finder, or allow GitSwitch in System Settings › Privacy & Security › App Management.",
                s.app.name
            )))
        }
        TrashOutcome::Failed(m) => return Err(AppError::Command(format!("Couldn't move {} to the Trash: {}", s.app.name, m))),
    }
    match gone(s) {
        Ok(()) => {
            s.state = "gone".into();
            s.message = Some(format!("{} is in the Trash.", s.app.name));
        }
        Err(why) => {
            s.state = "still-installed".into();
            s.message = Some(why);
        }
    }
    Ok(())
}

pub async fn status(s: &mut Session) -> Result<(), AppError> {
    match gone(s) {
        Ok(()) => s.state = "gone".into(),
        Err(why) => {
            if s.state != "uninstalling" {
                s.state = "still-installed".into();
            }
            s.message = Some(why);
        }
    }
    Ok(())
}

/// Every app bundle with this id outside any Trash.
fn copies_of(id: &str) -> Vec<PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)) {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = listed_bundles().into_iter().filter(|b| read_info(b).id.is_some_and(|i| i.eq_ignore_ascii_case(id))).collect();
    if !sandboxed() {
        if let Ok(o) = crate::spawn::command("mdfind").arg(format!("kMDItemCFBundleIdentifier == '{}'", id)).output() {
            out.extend(String::from_utf8_lossy(&o.stdout).lines().map(PathBuf::from));
        }
    }
    out.retain(|p| {
        let s = p.to_string_lossy();
        !s.contains("/.Trash/") && !s.contains("/.Trashes/") && p.exists() && p.extension().is_some_and(|x| x == "app")
    });
    out.sort();
    out.dedup();
    out
}

pub fn gone(s: &Session) -> Result<(), String> {
    let Record::MacBundle { bundle, bundle_id, .. } = &s.record else { return Err("Not a macOS app.".into()) };
    if bundle.exists() {
        return Err(format!("{} is still there.", bundle.display()));
    }
    if let Some(other) = copies_of(bundle_id).first() {
        return Err(format!("Another copy with the same id is installed at {} — its files are its own.", other.display()));
    }
    Ok(())
}

/// Every other app on this Mac: its id, names; and their app groups when the
/// removed app has any to compare.
pub fn others(s: &Session) -> Others {
    let mut o = Others::new(true);
    let removed = match &s.record {
        Record::MacBundle { bundle, .. } => bundle.clone(),
        _ => PathBuf::new(),
    };
    let mut bundles = listed_bundles();
    for extra in [sys("/System/Applications"), sys("/System/Applications/Utilities"), sys("/System/Library/CoreServices")] {
        bundles_in(&extra, 0, &mut bundles);
    }
    if !sandboxed() {
        if let Ok(out) = crate::spawn::command("mdfind").arg("kMDItemContentType == 'com.apple.application-bundle'").output() {
            bundles.extend(String::from_utf8_lossy(&out.stdout).lines().map(PathBuf::from));
        }
    }
    bundles.sort();
    bundles.dedup();
    let check_groups = !s.evidence.groups.is_empty();
    for b in bundles {
        let t = b.to_string_lossy();
        if b == removed || t.contains("/.Trash/") || t.contains("/.Trashes/") || !b.exists() {
            continue;
        }
        let info = read_info(&b);
        let file = b.file_stem().map(|n| n.to_string_lossy().to_string());
        let names: Vec<String> = [file, info.name, info.display_name, info.executable].into_iter().flatten().collect();
        let groups = if check_groups && !t.starts_with("/System/") { codesign_facts(&b).1 } else { Vec::new() };
        o.add_app(&info.id.into_iter().collect::<Vec<_>>(), &names, None, &groups);
    }
    o
}

/// The places, in ~/Library (to the Trash) and /Library (for the administrator).
fn places() -> Vec<Place> {
    let lib = home().join("Library");
    let mut v = vec![
        Place::new(lib.join("Application Support"), "~/Library/Application Support", Allow::IdsAndNames).vendor(),
        Place::new(lib.join("Caches"), "~/Library/Caches", Allow::IdsAndNames).vendor(),
        Place::new(lib.join("Preferences"), "~/Library/Preferences", Allow::Ids),
        Place::new(lib.join("Preferences").join("ByHost"), "~/Library/Preferences/ByHost", Allow::Ids),
        // Reading inside another app's container asks for permission (macOS 14+).
        Place::new(lib.join("Containers"), "~/Library/Containers", Allow::Ids).no_size(),
        Place::new(lib.join("Group Containers"), "~/Library/Group Containers", Allow::Groups).no_size(),
        Place::new(lib.join("Saved Application State"), "~/Library/Saved Application State", Allow::Ids),
        Place::new(lib.join("Logs"), "~/Library/Logs", Allow::IdsAndNames),
        Place::new(lib.join("HTTPStorages"), "~/Library/HTTPStorages", Allow::Ids),
        Place::new(lib.join("WebKit"), "~/Library/WebKit", Allow::Ids),
        Place::new(lib.join("Cookies"), "~/Library/Cookies", Allow::Ids),
        Place::new(lib.join("Application Scripts"), "~/Library/Application Scripts", Allow::Ids).no_size(),
    ];
    for (p, label) in [
        ("/Library/Application Support", "/Library/Application Support"),
        ("/Library/Caches", "/Library/Caches"),
        ("/Library/Preferences", "/Library/Preferences"),
        ("/Library/PrivilegedHelperTools", "/Library/PrivilegedHelperTools"),
    ] {
        v.push(Place::new(sys(p), label, Allow::Ids).admin());
    }
    v
}

/// A launchd job's program and the apps it says it belongs to.
pub(crate) fn launchd_job(v: &plist::Value) -> (Option<String>, Option<String>, Vec<String>) {
    let d = v.as_dictionary();
    let label = d.and_then(|d| d.get("Label")).and_then(|x| x.as_string()).map(str::to_string);
    let program = d
        .and_then(|d| d.get("Program"))
        .and_then(|x| x.as_string())
        .map(str::to_string)
        .or_else(|| d.and_then(|d| d.get("ProgramArguments")).and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|x| x.as_string()).map(str::to_string));
    let assoc = match d.and_then(|d| d.get("AssociatedBundleIdentifiers")) {
        Some(plist::Value::String(s)) => vec![s.clone()],
        Some(plist::Value::Array(a)) => a.iter().filter_map(|x| x.as_string().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    (label, program, assoc)
}

fn launch_jobs(dir: &Path, label: &str, bundle: &Path, ev: &Evidence, admin: bool, out: &mut Vec<Candidate>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let inside = format!("{}/", bundle.display());
    for e in rd.flatten() {
        let Ok(md) = e.path().symlink_metadata() else { continue };
        if md.file_type().is_symlink() || !md.is_file() || e.path().extension().is_none_or(|x| x != "plist") {
            continue;
        }
        let Ok(v) = plist::Value::from_file(e.path()) else { continue };
        let (job, program, assoc) = launchd_job(&v);
        let why = if program.as_deref().is_some_and(|p| p.starts_with(&inside)) {
            Some(format!("a background job that starts {}", program.unwrap_or_default()))
        } else if let Some(a) = assoc.iter().find(|a| ev.ids.iter().any(|i| i.eq_ignore_ascii_case(a))) {
            Some(format!("a background job belonging to {}", a))
        } else {
            job.as_deref().filter(|l| ev.ids.iter().any(|i| i.eq_ignore_ascii_case(l))).map(|l| format!("the app's background job {}", l))
        };
        let Some(why) = why else { continue };
        let job = job.unwrap_or_default();
        let quoted = crate::paths::shell_quote(&e.path().to_string_lossy());
        out.push(Candidate {
            path: e.path(),
            root: dir.to_path_buf(),
            place: label.to_string(),
            kind: "launch-agent",
            m: Match { confidence: Confidence::High, reason: why },
            admin,
            command: admin.then(|| format!("sudo launchctl bootout system/{} ; sudo rm {}", job, quoted)),
            action: if admin { Action::None } else { Action::LaunchAgent { label: job } },
            sized: false,
            entry: None,
        });
    }
}

/// Command-line links (/usr/local/bin, /opt/homebrew/bin) that pointed into the removed app.
fn dangling_links(bundle: &Path, out: &mut Vec<Candidate>) {
    for dir in [sys("/usr/local/bin"), sys("/opt/homebrew/bin")] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(md) = e.path().symlink_metadata() else { continue };
            if !md.file_type().is_symlink() {
                continue;
            }
            let Ok(target) = std::fs::read_link(e.path()) else { continue };
            let target = if target.is_absolute() { target } else { dir.join(target) };
            if !target.starts_with(bundle) || target.exists() {
                continue;
            }
            #[cfg(unix)]
            let admin = !super::writable(&dir);
            #[cfg(not(unix))]
            let admin = true;
            out.push(Candidate {
                path: e.path(),
                root: dir.clone(),
                place: dir.to_string_lossy().to_string(),
                kind: "link",
                m: Match { confidence: Confidence::High, reason: format!("a command that pointed into the removed app ({})", target.display()) },
                admin,
                command: admin.then(|| format!("sudo rm {}", crate::paths::shell_quote(&e.path().to_string_lossy()))),
                action: if admin { Action::None } else { Action::Link { target } },
                sized: false,
                entry: None,
            });
        }
    }
}

pub fn leftovers(s: &Session, others: &Others) -> (Vec<Candidate>, Vec<String>) {
    let Record::MacBundle { bundle, .. } = &s.record else { return (Vec::new(), Vec::new()) };
    let (mut out, mut notes) = super::scan_places(&places(), &s.evidence, others);
    // Admin places: a sure match only, and a command for the administrator.
    for c in out.iter_mut().filter(|c| c.admin) {
        c.command = Some(format!("sudo rm -rf {}", crate::paths::shell_quote(&c.path.to_string_lossy())));
    }
    launch_jobs(&home().join("Library").join("LaunchAgents"), "~/Library/LaunchAgents", bundle, &s.evidence, false, &mut out);
    launch_jobs(&sys("/Library/LaunchAgents"), "/Library/LaunchAgents", bundle, &s.evidence, true, &mut out);
    launch_jobs(&sys("/Library/LaunchDaemons"), "/Library/LaunchDaemons", bundle, &s.evidence, true, &mut out);
    dangling_links(bundle, &mut out);
    for r in receipts(bundle) {
        notes.push(format!("macOS still keeps the installer's record \"{}\". To forget it: sudo pkgutil --forget {}", r, r));
    }
    (out, notes)
}

/// Stop a background job before its file goes.
pub fn bootout(label: &str) {
    let sound = !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)) && !sandboxed();
    #[cfg(unix)]
    if sound {
        // SAFETY: getuid cannot fail.
        let uid = unsafe { libc::getuid() };
        let _ = crate::spawn::command("launchctl").arg("bootout").arg(format!("gui/{}/{}", uid, label)).output();
    }
    #[cfg(not(unix))]
    let _ = sound;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_plists_with_data_and_dates_are_read() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.apple.stocks</string>
<key>CFBundleName</key><string>Stocks</string>
<key>CFBundleExecutable</key><string>Stocks</string>
<key>CFBundleShortVersionString</key><string>7.1</string>
<key>Blob</key><data>AAEC</data>
<key>When</key><date>2026-01-01T00:00:00Z</date>
</dict></plist>"#;
        let v = plist::Value::from_reader_xml(&xml[..]).unwrap();
        let i = info_from(&v);
        assert_eq!(i.id.as_deref(), Some("com.apple.stocks"));
        assert_eq!(i.version.as_deref(), Some("7.1"));
        assert_eq!(vendor_of("com.google.Chrome").as_deref(), Some("google"));
        assert_eq!(vendor_of("us.zoom.xos").as_deref(), Some("zoom"));
        assert_eq!(vendor_of("Foo"), None);
    }

    #[test]
    fn running_apps_teams_and_groups_are_read_from_the_tools() {
        let ls = "1) \"Dock\" ASN:0x0-0xb00b:\n    bundleID=\"com.apple.dock\"\n    bundle path=\"/System/Library/CoreServices/Dock.app\"\n 2) \"x\"\n    bundleID=[ NULL ] \n";
        assert_eq!(running_ids(ls), vec!["com.apple.dock"]);
        assert_eq!(team_of("Executable=/x\nTeamIdentifier=EQHXZ8M8AV\n").as_deref(), Some("EQHXZ8M8AV"));
        assert_eq!(team_of("TeamIdentifier=not set\n"), None, "Apple's and ad-hoc apps have none");
        let ent = br#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>com.apple.security.application-groups</key><array><string>group.com.docker</string></array></dict></plist>"#;
        assert_eq!(groups_of(ent), vec!["group.com.docker"]);
        assert!(groups_of(b"").is_empty());
    }

    #[test]
    fn cask_files_name_the_apps_they_installed() {
        let rb = "cask \"visual-studio-code\" do\n  app \"Visual Studio Code.app\"\n  binary \"#{appdir}/Visual Studio Code.app/Contents/Resources/app/bin/code\"\nend";
        assert_eq!(app_names_in(rb), vec!["Visual Studio Code.app"]);
        let json = r#"{"artifacts":[{"app":["Slack.app"]},{"zap":[{"trash":["~/Library/Caches/com.tinyspeck.slackmacgap"]}]}]}"#;
        assert_eq!(app_names_in(json), vec!["Slack.app"]);
    }

    #[test]
    fn launchd_jobs_say_what_they_start() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>Label</key><string>com.docker.helper</string>
<key>ProgramArguments</key><array><string>/Applications/Docker.app/Contents/MacOS/helper</string><string>-x</string></array>
<key>AssociatedBundleIdentifiers</key><string>com.docker.docker</string></dict></plist>"#;
        let v = plist::Value::from_reader_xml(&xml[..]).unwrap();
        let (label, program, assoc) = launchd_job(&v);
        assert_eq!(label.as_deref(), Some("com.docker.helper"));
        assert_eq!(program.as_deref(), Some("/Applications/Docker.app/Contents/MacOS/helper"));
        assert_eq!(assoc, vec!["com.docker.docker"]);
    }
}
