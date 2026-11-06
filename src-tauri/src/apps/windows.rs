//! Windows: the programs "Installed apps" lists — the uninstall entries in
//! the 64-bit and 32-bit registry views and the user's own — and the Store
//! apps. A program goes with its own uninstaller (MSI: `msiexec /x` with its
//! product code), started the way Explorer starts it, so the uninstaller
//! asks for permission itself; it is finished when its entry is gone and no
//! process it started still runs. Leftovers in the user's folders go to the
//! Recycle Bin; what is under Program Files or ProgramData is listed for the
//! administrator.
#![cfg_attr(not(windows), allow(dead_code, unused_imports, unused_mut))]

use super::matcher::{self, Allow, Confidence, Evidence, Match, Others};
use super::{Action, Candidate, Closed, ForceFolder, ForcePlan, InstalledApp, JobItem, JobWhat, Place, Prepared, Record, Session, Startup};
use crate::error::AppError;
use std::path::{Path, PathBuf};

pub(crate) const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

/// One uninstall entry, as the registry holds it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RegEntry {
    /// HKLM64 | HKLM32 | HKCU
    pub view: String,
    pub key: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub install_location: Option<String>,
    pub install_date: Option<String>,
    pub uninstall: Option<String>,
    pub windows_installer: bool,
    pub estimated_kb: Option<u32>,
    pub icon: Option<String>,
    pub system_component: bool,
    pub parent: Option<String>,
    pub release_type: Option<String>,
    pub no_remove: bool,
    /// Written by a WiX Burn bundle (BundleCachePath / BundleUpgradeCode).
    pub bundle: bool,
}

pub(crate) fn is_guid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 38
        && b[0] == b'{'
        && b[37] == b'}'
        && s[1..37].char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// The MSI product code: the entry's key when Windows Installer made it,
/// else the `{…}` in an `MsiExec.exe /I{…}` command.
pub(crate) fn msi_guid(e: &RegEntry) -> Option<String> {
    if e.windows_installer && is_guid(&e.key) {
        return Some(e.key.to_uppercase());
    }
    let u = e.uninstall.as_deref()?;
    if !u.to_lowercase().contains("msiexec") {
        return None;
    }
    let start = u.find('{')?;
    let g = u.get(start..start + 38)?;
    is_guid(g).then(|| g.to_uppercase())
}

/// Program and arguments of an UninstallString. Quoted: the quoted part.
/// Unquoted: the shortest prefix that is a file (Windows resolves
/// `C:\Program Files\X\unins.exe /S` the same way); the rest passes on
/// exactly as written.
pub(crate) fn split_command(cmd: &str, exists: &dyn Fn(&str) -> bool) -> Option<(String, String)> {
    let c = cmd.trim();
    if c.is_empty() {
        return None;
    }
    if let Some(rest) = c.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some((rest[..end].to_string(), rest[end + 1..].trim().to_string()));
    }
    let mut bounds: Vec<usize> = c.match_indices(' ').map(|(i, _)| i).collect();
    bounds.push(c.len());
    for b in bounds {
        let p = &c[..b];
        if exists(p) {
            return Some((p.to_string(), c[b..].trim().to_string()));
        }
        let with_exe = format!("{}.exe", p);
        if !p.to_lowercase().ends_with(".exe") && exists(&with_exe) {
            return Some((with_exe, c[b..].trim().to_string()));
        }
    }
    let (p, r) = c.split_once(' ').unwrap_or((c, ""));
    let p = if p.eq_ignore_ascii_case("rundll32.exe") || p.eq_ignore_ascii_case("rundll32") {
        format!(r"{}\System32\rundll32.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
    } else {
        p.to_string()
    };
    Some((p, r.trim().to_string()))
}

/// "7-Zip 23.01 (x64)" → "7-Zip": the name its folders are made with.
pub(crate) fn strip_version(name: &str) -> String {
    let mut s = name.trim().to_string();
    loop {
        let before = s.clone();
        if s.ends_with(')') {
            if let Some(i) = s.rfind(" (") {
                s.truncate(i);
            }
        }
        if let Some(i) = s.rfind(" - ") {
            if s[i + 3..].chars().next().is_some_and(|c| c.is_ascii_digit()) {
                s.truncate(i);
            }
        }
        let words: Vec<&str> = s.split(' ').collect();
        if words.len() > 1 {
            let last = words[words.len() - 1].to_lowercase();
            let version_like = last.trim_start_matches('v').chars().next().is_some_and(|c| c.is_ascii_digit()) && last.trim_start_matches('v').chars().all(|c| c.is_ascii_digit() || c == '.');
            if version_like || ["x64", "x86", "64-bit", "32-bit", "version", "(x64)", "(x86)"].contains(&last.as_str()) {
                s = words[..words.len() - 1].join(" ");
            }
        }
        s = s.trim().trim_end_matches(['-', ',']).trim().to_string();
        if s == before {
            return s;
        }
    }
}

/// `C:\x\app.exe,0` or a command → the program's name without `.exe`.
pub(crate) fn exe_stem(icon_or_cmd: &str) -> Option<String> {
    let t = icon_or_cmd.trim().trim_matches('"');
    let path = t.split(',').next().unwrap_or(t).trim().trim_matches('"');
    let lower = path.to_lowercase();
    let end = lower.find(".exe")?;
    let file = &path[..end];
    let stem = file.rsplit(['\\', '/']).next()?.to_string();
    (!stem.is_empty()).then_some(stem)
}

fn expand(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if j > 0 => {
                let var = &after[..j];
                match std::env::var(var) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => out.push_str(&rest[i..i + j + 2]),
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// An InstallLocation as a usable folder: quotes and a trailing `\` off,
/// variables expanded, short (8.3) names made long.
pub(crate) fn clean_dir(raw: &str) -> Option<PathBuf> {
    let t = expand(raw.trim().trim_matches('"').trim());
    let t = t.trim_end_matches(['\\', '/']);
    if t.len() < 3 {
        return None;
    }
    Some(long_path(Path::new(t)))
}

fn long_path(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        sys::long_path(p)
    }
    #[cfg(not(windows))]
    {
        p.to_path_buf()
    }
}

/// Never a leftover: folders shared by many programs.
pub(crate) fn never_dir(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    matcher::WINDOWS_NEVER.contains(&n.as_str()) || n.starts_with("windows") || n.starts_with("regid.") || n.starts_with("microsoft")
}

fn known_folders() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [
        "SystemDrive", "SystemRoot", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "CommonProgramFiles", "CommonProgramFiles(x86)",
        "USERPROFILE", "APPDATA", "LOCALAPPDATA", "PUBLIC", "ALLUSERSPROFILE", "OneDrive", "TEMP",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok())
    .map(|v| PathBuf::from(format!("{}\\", v.trim_end_matches('\\'))))
    .collect();
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        v.push(PathBuf::from(l).join("Programs"));
    }
    v
}

fn lower(p: &Path) -> String {
    p.to_string_lossy().to_lowercase().replace('/', "\\").trim_end_matches('\\').to_string()
}

/// An install folder that may be offered: two levels below its drive or
/// more, not one of Windows' own folders or a parent of one.
pub fn safe_install_dir(dir: &Path) -> bool {
    let d = lower(dir);
    let parts: Vec<&str> = d.split('\\').filter(|p| !p.is_empty()).collect();
    if parts.len() < 3 || !parts[0].ends_with(':') || never_dir(parts.last().unwrap_or(&"")) {
        return false;
    }
    let system_root = std::env::var("SystemRoot").map(|s| s.to_lowercase()).unwrap_or_else(|_| r"c:\windows".into());
    if d == system_root || d.starts_with(&format!("{}\\", system_root)) {
        return false;
    }
    !known_folders().iter().map(|k| lower(k)).any(|k| k == d || k.starts_with(&format!("{}\\", d)))
}

/// `a` is `dir` or inside it.
pub(crate) fn inside(a: &Path, dir: &Path) -> bool {
    let (a, d) = (lower(a), lower(dir));
    a == d || a.starts_with(&format!("{}\\", d))
}

/// A folder an installer keeps its own copies in, or the person's own
/// files: a program found there doesn't make it an install folder.
pub(crate) fn cache_like(p: &Path) -> bool {
    let l = lower(p);
    let parts: Vec<&str> = l.split('\\').filter(|x| !x.is_empty()).collect();
    if parts.get(1).is_some_and(|s| *s == "windows") {
        return true;
    }
    const CACHES: &[&str] = &[
        "package cache", "installer", "temp", "tmp", "downloads", "desktop", "documents", "onedrive", "uninstall information", "$recycle.bin",
        "installshield installation information", "caphyon", "inetcache", "crashdumps",
    ];
    parts.iter().any(|s| CACHES.contains(s) || is_guid(s) || (s.starts_with('{') && s.ends_with('}')))
}

fn version_like(n: &str) -> bool {
    let v = n.trim_start_matches('v');
    v.contains('.') && v.chars().next().is_some_and(|c| c.is_ascii_digit()) && v.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// A subfolder a program sits in inside its install folder.
fn generic_dir(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    const GENERIC: &[&str] = &["bin", "bin32", "bin64", "x64", "x86", "amd64", "arm64", "win32", "win64", "app", "application", "program", "current", "lib", "release", "64bit", "32bit", "64-bit", "32-bit"];
    GENERIC.contains(&n.as_str()) || n.strip_prefix("app-").is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit())) || version_like(&n)
}

/// `"C:\x\app.exe",0` -> `C:\x\app.exe`: the file an icon setting names.
pub(crate) fn icon_file(icon: &str) -> Option<PathBuf> {
    let t = icon.trim();
    let t = match t.rsplit_once(',') {
        Some((p, idx)) if idx.trim().trim_start_matches('-').chars().all(|c| c.is_ascii_digit()) => p,
        _ => t,
    };
    let p = expand(t.trim().trim_matches('"').trim());
    (p.len() > 3 && p.as_bytes().get(1) == Some(&b':')).then(|| PathBuf::from(p))
}

/// Where programs are installed: Program Files (either, on any drive),
/// ProgramData, or a person's AppData (Local, Programs, Roaming).
pub(crate) fn in_install_root(p: &Path) -> bool {
    let l = lower(p);
    let parts: Vec<&str> = l.split('\\').filter(|x| !x.is_empty()).collect();
    if parts.len() < 3 || !parts[0].ends_with(':') {
        return false;
    }
    match parts[1] {
        "program files" | "program files (x86)" | "programdata" => true,
        "users" => parts.len() >= 5 && parts[3] == "appdata" && (parts[4] == "local" || parts[4] == "roaming"),
        _ => false,
    }
}

/// The folder a program of the app is installed in, from one of its files:
/// the file's folder above any generic subfolder (bin, x64, app-1.2.3), when
/// it is named like the app, inside a place programs are installed in, and
/// neither an installer's cache nor one of Windows' own folders. (A
/// developer's build or a portable copy elsewhere is never taken for it.)
pub(crate) fn install_dir_of(file: &Path, names: &[String]) -> Option<PathBuf> {
    let mut dir = file.parent()?.to_path_buf();
    for _ in 0..2 {
        let n = dir.file_name()?.to_string_lossy().to_string();
        if !generic_dir(&n) {
            break;
        }
        dir = dir.parent()?.to_path_buf();
    }
    if cache_like(&dir) || !safe_install_dir(&dir) || !in_install_root(&dir) {
        return None;
    }
    let n = dir.file_name()?.to_string_lossy().trim().to_lowercase();
    let named = names.iter().filter(|x| matcher::usable_name(x)).any(|x| {
        let x = x.trim().to_lowercase();
        n == x || n.replace(' ', "") == x.replace(' ', "")
    });
    named.then_some(dir)
}

/// Add a folder unless it, or a folder holding it, is already there; drop
/// those inside it.
pub(crate) fn push_dir(dirs: &mut Vec<PathBuf>, d: PathBuf) {
    if dirs.iter().any(|e| inside(&d, e)) {
        return;
    }
    dirs.retain(|e| !inside(e, &d));
    dirs.push(d);
}

/// The program a command line starts.
pub(crate) fn command_program(cmd: &str) -> Option<PathBuf> {
    let c = expand(cmd.trim());
    let c = c.trim_start_matches(r"\??\");
    let c = if c.to_lowercase().starts_with(r"\systemroot\") { format!("{}{}", std::env::var("SystemRoot").unwrap_or_default(), &c[11..]) } else { c.to_string() };
    let (p, _) = split_command(&c, &|p| Path::new(p).is_file())?;
    let p = if p.to_lowercase().starts_with(r"system32\") { format!(r"{}\{}", std::env::var("SystemRoot").unwrap_or_default(), p) } else { p };
    Some(PathBuf::from(p))
}

/// The folders a program is installed in: those recorded before it went,
/// or (an older session) what its entry itself points at.
pub(crate) fn recorded_dirs(app_name: &str, rec: &Record) -> Vec<PathBuf> {
    let Record::WinEntry { uninstall, install_dir, icon, dirs, exes, .. } = rec else { return vec![] };
    if !dirs.is_empty() {
        return dirs.clone();
    }
    let mut names = names_of(app_name, install_dir.as_deref(), icon.as_deref());
    names.extend(exes.iter().cloned());
    let mut out = Vec::new();
    if let Some(d) = install_dir {
        push_dir(&mut out, d.clone());
    }
    if let Some(f) = icon.as_deref().and_then(icon_file) {
        if let Some(d) = install_dir_of(&f, &names) {
            push_dir(&mut out, d);
        }
    }
    if let Some((prog, _)) = split_command(uninstall, &|p| Path::new(p).is_file()) {
        if let Some(d) = install_dir_of(Path::new(&prog), &names) {
            push_dir(&mut out, d);
        }
    }
    out
}

/// Before a program goes: every folder it is installed in, its programs'
/// names and what starts it with Windows — from its entry, App Paths, its
/// shortcuts and the Run keys — kept in the session for later.
pub fn enrich(app: &InstalledApp, rec: &Record) -> Record {
    let Record::WinEntry { view, key, install_dir, icon, .. } = rec else { return rec.clone() };
    let removed = format!("{}:{}", view, key);
    let names = names_of(&app.name, install_dir.as_deref(), icon.as_deref());
    let mut found = recorded_dirs(&app.name, rec);
    let mut exes: Vec<String> = Vec::new();
    #[allow(unused_mut)]
    let mut programs: Vec<PathBuf> = icon.as_deref().and_then(icon_file).into_iter().collect();
    // App Paths and shortcuts know programs only by name: never when
    // another installed program has this one's name.
    #[cfg(windows)]
    if !sys::name_shared(&app.name, &removed) {
        programs.extend(sys::app_paths(&names));
        programs.extend(sys::shortcut_targets(&names));
    }
    for f in &programs {
        let Some(d) = install_dir_of(f, &names) else { continue };
        let stem = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if f.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) && matcher::usable_name(&stem) && !exes.iter().any(|e| e.eq_ignore_ascii_case(&stem)) {
            exes.push(stem);
        }
        push_dir(&mut found, d);
    }
    // What starts it: Run values pointing into its own folders — never a
    // shared one, nor one another program is installed in.
    #[cfg(windows)]
    let startup: Vec<Startup> = {
        let own: Vec<&PathBuf> = found.iter().filter(|d| safe_install_dir(d) && sys::install_dir_claimed(d, &removed).is_none()).collect();
        sys::run_values().into_iter().filter(|r| command_program(&r.data).is_some_and(|p| own.iter().any(|d| inside(&p, d)))).collect()
    };
    #[cfg(not(windows))]
    let startup: Vec<Startup> = {
        let _ = &removed;
        Vec::new()
    };
    let mut out = rec.clone();
    if let Record::WinEntry { dirs, exes: e, startup: st, .. } = &mut out {
        *dirs = found;
        *e = exes;
        *st = startup;
    }
    out
}

/// Whether GitSwitch itself may move it (else Windows asks for permission):
/// it can be opened for deleting.
pub(crate) fn can_delete(path: &Path) -> bool {
    #[cfg(windows)]
    {
        sys::can_delete(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        true
    }
}

/// Windows: deleting a program's files directly — what would go, or why not.
pub(crate) fn force_plan(app: &InstalledApp, rec: &Record) -> (Option<ForcePlan>, Option<String>) {
    let Record::WinEntry { view, key, msi, startup, bundle, .. } = rec else { return (None, None) };
    if msi.is_some() {
        return (None, Some("Windows Installer removes it cleanly, so its files aren't deleted directly — use Uninstall.".into()));
    }
    if *bundle {
        return (None, Some("A setup bundle installed it together with other parts, so its files aren't deleted directly — use Uninstall.".into()));
    }
    let dirs: Vec<PathBuf> = recorded_dirs(&app.name, rec);
    if dirs.is_empty() {
        return (None, Some("Where its files are isn't known, so there is nothing to delete directly — use Uninstall.".into()));
    }
    let mut folders = Vec::new();
    for d in &dirs {
        if !safe_install_dir(d) {
            return (None, Some(format!("{} is one of Windows' own folders, so it isn't deleted directly.", d.display())));
        }
        // Its drive not there (unplugged): nothing can be said about its files.
        if !d.ancestors().last().is_some_and(|root| root.exists()) {
            return (None, Some(format!("{} can't be reached right now — is its drive connected?", d.display())));
        }
        #[cfg(windows)]
        if let Some(why) = sys::install_dir_claimed(d, &format!("{}:{}", view, key)) {
            return (None, Some(format!("{} is still needed — {} — so use Uninstall.", d.display(), why)));
        }
        // Already gone (deleted by hand, or an earlier try): only its entry is left.
        if !d.is_dir() {
            continue;
        }
        let (size, done) = super::size_of(d, std::time::Duration::from_secs(2));
        folders.push(ForceFolder { path: d.to_string_lossy().to_string(), size: Some(size), size_complete: done, admin: !can_delete(d) });
    }
    let entry_admin = view != "HKCU";
    let admin = entry_admin || folders.iter().any(|f| f.admin) || startup.iter().any(|s| s.view != "HKCU");
    let plan = ForcePlan {
        folders,
        entry: format!(r"{}\{}\{}", if view == "HKCU" { "HKCU" } else { "HKLM" }, UNINSTALL, key),
        entry_admin,
        startup: startup.iter().map(|s| format!("{} ({})", s.name, if s.view == "HKCU" { "starts when you sign in" } else { "starts for everyone" })).collect(),
        admin,
    };
    (Some(plan), None)
}

/// What deleting a program's files directly does, in order — each with
/// whether it needs the administrator: its folders, then its entry and its
/// start-up values (only once every folder went).
pub(crate) fn force_items(s: &Session) -> Result<Vec<(JobItem, bool)>, String> {
    let Record::WinEntry { view, key, startup, .. } = &s.record else { return Err("Only a Windows program's files are deleted directly.".into()) };
    let (plan, why) = force_plan(&s.app, &s.record);
    let Some(plan) = plan else { return Err(why.unwrap_or_else(|| "Its files can't be deleted directly.".into())) };
    if plan.folders.iter().any(|f| running_in(Path::new(&f.path))) {
        return Err(format!("{} is running — close it first.", s.app.name));
    }
    let mut items = Vec::new();
    // Every folder is checked before anything moves.
    for (i, f) in plan.folders.iter().enumerate() {
        let path = PathBuf::from(&f.path);
        let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let ident = super::ident(&path).ok_or_else(|| format!("{} can't be read (or is a link), so nothing was deleted.", path.display()))?;
        #[cfg(windows)]
        if let Some(why) = recycle_refusal(&path) {
            return Err(format!("{} Nothing was deleted.", why));
        }
        items.push((JobItem { id: format!("folder{}", i), what: JobWhat::Trash { path, root, ident: Some(ident) } }, f.admin));
    }
    items.push((JobItem { id: "entry".into(), what: JobWhat::UninstallEntry { view: view.clone(), key: key.clone(), name: s.app.name.clone() } }, view != "HKCU"));
    for (i, st) in startup.iter().enumerate() {
        items.push((JobItem { id: format!("startup{}", i), what: JobWhat::RunValue { view: st.view.clone(), name: st.name.clone(), data: st.data.clone() } }, st.view != "HKCU"));
    }
    Ok(items)
}

/// What the administrator's cleanup may touch: inside Program Files,
/// ProgramData, the shared Desktop, the person's app-data folders, or a
/// program's own folder elsewhere — never Windows itself, a folder Windows
/// uses, or one of the folders shared by many programs.
pub(crate) fn elevated_path_ok(path: &Path) -> Result<(), String> {
    let p = lower(path);
    let system_root = std::env::var("SystemRoot").map(|s| s.to_lowercase()).unwrap_or_else(|_| r"c:\windows".into());
    if p == system_root || p.starts_with(&format!("{}\\", system_root)) {
        return Err("It is inside Windows' own folder.".into());
    }
    if known_folders().iter().map(|k| lower(k)).any(|k| k == p || k.starts_with(&format!("{}\\", p))) {
        return Err("It is a folder Windows itself uses.".into());
    }
    if path.file_name().is_some_and(|n| never_dir(&n.to_string_lossy())) {
        return Err("Many programs share that folder.".into());
    }
    let parts: Vec<&str> = p.split('\\').filter(|x| !x.is_empty()).collect();
    // The shared Desktop's shortcuts.
    let public_shortcut = parts.len() == 5 && parts[1] == "users" && parts[2] == "public" && parts[3] == "desktop" && parts[4].ends_with(".lnk");
    // A program's own folder on another drive or outside the profiles (D:\Games\X).
    let elsewhere = parts.get(1).is_some_and(|s| *s != "users") && safe_install_dir(path);
    if in_install_root(path) || public_shortcut || elsewhere {
        Ok(())
    } else {
        Err("It is not in a place programs are installed in.".into())
    }
}

/// Hidden from "Installed apps" too: parts, updates, entries that can't be removed.
pub(crate) fn hidden(e: &RegEntry) -> bool {
    e.system_component || e.parent.is_some() || e.release_type.is_some() || e.no_remove || e.uninstall.as_deref().is_none_or(|u| u.trim().is_empty()) || e.name.as_deref().is_none_or(|n| n.trim().is_empty())
}

fn date_of(yyyymmdd: &str) -> Option<i64> {
    chrono::NaiveDate::parse_from_str(yyyymmdd.trim(), "%Y%m%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)).map(|d| d.and_utc().timestamp())
}

pub(crate) fn entry_app(e: &RegEntry) -> Option<(InstalledApp, Record)> {
    if hidden(e) {
        return None;
    }
    let name = e.name.clone()?.trim().to_string();
    let msi = msi_guid(e);
    let install_dir = e.install_location.as_deref().and_then(clean_dir);
    let mut notes = Vec::new();
    if e.view == "HKCU" {
        notes.push("Installed for you only.".to_string());
    }
    if e.view == "HKLM32" {
        notes.push("32-bit.".to_string());
    }
    let lname = name.to_lowercase();
    let blocked = if lname.contains("webview2 runtime") {
        Some("GitSwitch's own window runs on it.".to_string())
    } else if (lname.starts_with("git ") || lname == "git") && e.publisher.as_deref().is_some_and(|p| p.contains("Git Development Community")) {
        Some("GitSwitch needs git.".to_string())
    } else {
        None
    };
    Some((
        InstalledApp {
            id: format!("win:{}:{}", e.view, e.key),
            name: name.clone(),
            version: e.version.clone(),
            publisher: e.publisher.clone(),
            location: install_dir.as_ref().map(|d| d.to_string_lossy().to_string()),
            source: if msi.is_some() { "msi".into() } else { "exe".into() },
            size: e.estimated_kb.map(|k| k as u64 * 1024),
            date: e.install_date.as_deref().and_then(date_of),
            notes,
            blocked,
            is_self: name == "GitSwitch",
            system: None,
        },
        Record::WinEntry {
            view: e.view.clone(),
            key: e.key.clone(),
            uninstall: e.uninstall.clone().unwrap_or_default(),
            msi,
            install_dir,
            icon: e.icon.clone(),
            dirs: vec![],
            exes: vec![],
            startup: vec![],
            bundle: e.bundle,
        },
    ))
}

/// A Store app, as the listing script gives it.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub(crate) struct Appx {
    pub name: String,
    #[serde(default)]
    pub full: String,
    pub family: String,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct AppxListing {
    #[serde(default)]
    pub main: Vec<Appx>,
    #[serde(default)]
    pub all: Vec<Appx>,
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !t.len().is_multiple_of(2) {
        return None;
    }
    (0..t.len()).step_by(2).map(|i| u8::from_str_radix(&t[i..i + 2], 16).ok()).collect()
}

/// A value in a Store app manifest's <Properties>, unless it is a resource reference.
fn manifest_prop(xml: &str, tag: &str) -> Option<String> {
    let props = xml.find("<Properties>")?;
    let rest = &xml[props..];
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = rest.find(&open)? + open.len();
    let end = rest[start..].find(&close)?;
    let n = rest[start..start + end].trim();
    (!n.is_empty() && !n.starts_with("ms-resource:")).then(|| n.replace("&amp;", "&"))
}

/// A Store app's name as its manifest gives it.
pub(crate) fn manifest_name(xml: &str) -> Option<String> {
    manifest_prop(xml, "DisplayName")
}

/// A Store app's publisher as people know it ("ASUSTeK COMPUTER INC."),
/// where the package itself only carries a Store id.
pub(crate) fn manifest_publisher(xml: &str) -> Option<String> {
    manifest_prop(xml, "PublisherDisplayName")
}

fn appx_app(a: &Appx, facts: &SystemFacts) -> (InstalledApp, Record) {
    let dir = a.dir.as_deref().filter(|d| !d.is_empty()).map(PathBuf::from);
    let manifest = dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("AppxManifest.xml")).ok());
    let shown = manifest.as_deref().and_then(manifest_name).unwrap_or_else(|| a.name.clone());
    let cn = a.publisher.as_deref().and_then(|p| p.split(',').next()).map(|p| p.trim_start_matches("CN=").trim_matches('"').to_string());
    // A sparse package (a program's identity for Windows) may say only "Sparse Package".
    let shown_publisher = manifest.as_deref().and_then(manifest_publisher).filter(|p| !p.eq_ignore_ascii_case("sparse package"));
    let system = appx_system_reason(&a.name, &a.family, shown_publisher.as_deref(), facts).or_else(|| appx_system_reason(&a.name, &a.family, cn.as_deref(), facts));
    let publisher = shown_publisher.or(cn);
    (
        InstalledApp {
            id: format!("appx:{}", a.full),
            name: shown,
            version: a.version.clone(),
            publisher,
            location: None,
            source: "store".into(),
            notes: vec!["A Store app: removed for you only.".into()],
            system,
            ..Default::default()
        },
        Record::WinAppx { full_name: a.full.clone(), family: a.family.clone(), name: a.name.clone(), install_dir: dir },
    )
}

// ---------------------------------------------------------------------------
// System apps: what came with Windows or this PC, runs its hardware, or
// other apps use. Everything else is the person's own.

/// What the "System apps" tab is told apart by, read once per listing.
#[derive(Debug, Clone, Default)]
pub(crate) struct SystemFacts {
    /// The PC maker's first word ("asustek"), from the firmware.
    pub maker: Option<String>,
    /// The Store packages provisioned with Windows or by the PC maker, as
    /// lower-case family names (`clipchamp.clipchamp_yxz26nhyzhsrt`).
    pub provisioned: std::collections::HashSet<String>,
}

/// The first word of a company or product name, as compared: lower case,
/// its leading letters, digits and dashes only ("Intel(R)" -> "intel").
pub(crate) fn first_word(s: &str) -> Option<String> {
    let w = s.split_whitespace().next()?;
    let w: String = w.chars().take_while(|c| c.is_alphanumeric() || *c == '-').collect();
    let w = w.trim_matches('-').to_lowercase();
    (w.chars().count() >= 2).then_some(w)
}

/// The PC maker as the firmware names it, unless that is a placeholder
/// (a self-built PC) or Microsoft (a Surface: Microsoft's apps are not "the maker's").
pub(crate) fn maker_word(raw: &str) -> Option<String> {
    let l = raw.trim().to_lowercase();
    let placeholder = ["system manufacturer", "default string", "default", "oem", "o.e.m.", "not applicable", "none", "unknown", "to be filled by o.e.m."];
    if l.is_empty() || placeholder.contains(&l.as_str()) || l.starts_with("to be filled") || l.starts_with("microsoft") {
        return None;
    }
    first_word(&l)
}

/// `Name_Version_Arch_ResourceId_PublisherId` -> `name_publisherid`.
pub(crate) fn provisioned_family(full: &str) -> Option<String> {
    let parts: Vec<&str> = full.split('_').collect();
    if parts.len() < 5 || parts[0].is_empty() || parts[parts.len() - 1].is_empty() {
        return None;
    }
    Some(format!("{}_{}", parts[0], parts[parts.len() - 1]).to_lowercase())
}

/// Makers of chips and devices: their programs are drivers and their tools.
const HARDWARE: &[&str] = &[
    "intel", "nvidia", "amd", "realtek", "qualcomm", "mediatek", "synaptics", "elan", "elantech", "dolby", "dts", "conexant", "broadcom", "rivet",
    "killer", "waves", "cirrus",
];

fn words(name: &str) -> Vec<String> {
    name.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn is_microsoft(publisher: Option<&str>) -> bool {
    publisher.is_some_and(|p| p.trim().to_lowercase().starts_with("microsoft"))
}

/// Why a program or Store app counts as a system app, if it does.
pub(crate) fn system_reason(name: &str, publisher: Option<&str>, facts: &SystemFacts) -> Option<String> {
    let pw = publisher.and_then(first_word);
    if facts.maker.is_some() && pw == facts.maker {
        return Some("From your PC's maker".into());
    }
    let pl = publisher.unwrap_or("").trim().to_lowercase();
    if pw.as_deref().is_some_and(|w| HARDWARE.contains(&w)) || pl.starts_with("advanced micro devices") {
        return Some("For your hardware".into());
    }
    let lname = name.to_lowercase();
    if !(is_microsoft(publisher) || (pl.is_empty() && lname.starts_with("microsoft windows"))) {
        return None;
    }
    let w = words(name);
    let has = |x: &str| w.iter().any(|y| y == x);
    let starts = |x: &str| w.iter().any(|y| y.starts_with(x));
    if has("sdk") {
        return None;
    }
    if has("redistributable") || has("runtime") || has("webview2") || lname.contains("shared framework") || lname.contains(".net") || starts("winappruntime") || starts("vclibs") {
        return Some("Used by other apps".into());
    }
    if has("edge") || has("onedrive") || has("gameinput") || has("xbox") || lname.contains("update health") || lname.contains("pc health check") || lname.contains("application compatibility") {
        return Some("Part of Windows".into());
    }
    if ["gamingservices", "winget", "startexperiences", "widgets", "webexperience", "crossdevice", "desktopappinstaller"].iter().any(|x| starts(x)) {
        return Some("Part of Windows".into());
    }
    None
}

/// A Store app: provisioned ones came with Windows (or with the PC).
pub(crate) fn appx_system_reason(name: &str, family: &str, publisher: Option<&str>, facts: &SystemFacts) -> Option<String> {
    if facts.provisioned.contains(&family.to_lowercase()) {
        return Some(if is_microsoft(publisher) { "Came with Windows" } else { "Came with this PC" }.into());
    }
    system_reason(name, publisher, facts)
}

pub fn list() -> Result<super::Listing, AppError> {
    #[cfg(windows)]
    {
        let facts = sys::system_facts();
        let mut rows: Vec<(InstalledApp, Record)> = sys::read_entries()
            .iter()
            .filter_map(entry_app)
            .map(|(mut a, r)| {
                a.system = system_reason(&a.name, a.publisher.as_deref(), &facts);
                (a, r)
            })
            .collect();
        let mut notes = Vec::new();
        match sys::appx() {
            Ok(l) => rows.extend(l.main.iter().map(|a| appx_app(a, &facts))),
            Err(e) => notes.push(format!("Store apps could not be listed: {}", e)),
        }
        Ok((rows, notes))
    }
    #[cfg(not(windows))]
    Err(AppError::Config("Windows only.".into()))
}

/// The names a program's folders are made with.
fn names_of(name: &str, install_dir: Option<&Path>, icon: Option<&str>) -> Vec<String> {
    let mut v = vec![name.to_string(), strip_version(name)];
    if let Some(d) = install_dir.and_then(|d| d.file_name()) {
        v.push(d.to_string_lossy().to_string());
    }
    if let Some(s) = icon.and_then(exe_stem) {
        v.push(s);
    }
    v.retain(|n| !n.trim().is_empty());
    v.dedup();
    v
}

pub fn prepare(app: &InstalledApp, rec: &Record) -> Result<(Prepared, Evidence, Vec<String>), AppError> {
    let mut p = Prepared { blocked: app.blocked.clone(), ..Default::default() };
    let ev;
    match rec {
        Record::WinEntry { view, key, uninstall, msi, install_dir, icon, exes, .. } => {
            let mut names = names_of(&app.name, install_dir.as_deref(), icon.as_deref());
            for x in exes {
                if !names.iter().any(|n| n.eq_ignore_ascii_case(x)) {
                    names.push(x.clone());
                }
            }
            ev = Evidence { ids: vec![], names, groups: vec![], vendor: app.publisher.clone() };
            let dirs = recorded_dirs(&app.name, rec);
            let (program, args) = match msi {
                Some(g) => (msiexec(), format!("/x {}", g)),
                None => split_command(uninstall, &|p| Path::new(p).is_file()).ok_or_else(|| AppError::Config("Its uninstall command is empty.".into()))?,
            };
            p.command = Some(if args.is_empty() { format!("\"{}\"", program) } else { format!("\"{}\" {}", program, args) });
            p.method = if msi.is_some() {
                "Windows Installer removes it (msiexec); Windows asks for permission if it needs to.".into()
            } else {
                "Its own uninstaller opens; follow its steps. Windows asks for permission if it needs to.".into()
            };
            p.facts = vec![format!("Entry: {}\\{}\\{} ({})", if view == "HKCU" { "HKCU" } else { "HKLM" }, UNINSTALL, key, match view.as_str() { "HKLM32" => "32-bit view", "HKLM64" => "64-bit view", _ => "your own" })];
            for d in &dirs {
                p.facts.push(format!("Installed in: {}", d.display()));
            }
            if let Some(pb) = &app.publisher {
                p.facts.push(format!("Publisher: {}", pb));
            }
            #[cfg(windows)]
            if let Some(signer) = sys::signer(Path::new(&program)) {
                p.facts.push(format!("The uninstaller is signed by: {}", signer));
            }
            p.running = dirs.iter().filter(|d| safe_install_dir(d)).any(|d| running_in(d));
            if p.running {
                p.warnings.push(format!("{} is running — close it first, or its uninstaller may ask you to.", app.name));
            }
            let (force, why) = force_plan(app, rec);
            p.force = force;
            p.force_unavailable = why;
        }
        Record::WinAppx { full_name, family, .. } => {
            ev = Evidence { ids: vec![family.clone()], names: vec![app.name.clone()], groups: vec![], vendor: None };
            p.method = "Windows removes the Store app for you (other people on this PC keep theirs).".into();
            p.command = Some(format!("Remove-AppxPackage {}", full_name));
            p.facts = vec![format!("Store package: {}", full_name)];
        }
        _ => return Err(AppError::Config("Not a Windows program.".into())),
    }
    Ok((p, ev, Vec::new()))
}

fn msiexec() -> String {
    format!(r"{}\System32\msiexec.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
}

fn running_in(dir: &Path) -> bool {
    #[cfg(windows)]
    {
        let d = format!("{}\\", lower(dir));
        sys::processes().iter().any(|p| p.path.as_ref().is_some_and(|x| lower(x).starts_with(&d)))
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        false
    }
}

pub async fn uninstall(s: &mut Session) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        match s.record.clone() {
            Record::WinEntry { uninstall, msi, .. } => {
                let (program, args) = match &msi {
                    Some(g) => (msiexec(), format!("/x {}", g)),
                    None => split_command(&uninstall, &|p| Path::new(p).is_file()).ok_or_else(|| AppError::Config("Its uninstall command is empty.".into()))?,
                };
                let session = s.id.clone();
                let started = tokio::task::spawn_blocking(move || sys::launch(&program, &args, &session)).await.map_err(|e| AppError::Command(e.to_string()))?;
                let (pid, created) = started.map_err(AppError::Config)?;
                s.process = Some((pid, created));
                s.state = "uninstalling".into();
                s.message = Some("The uninstaller is open — follow its steps.".into());
                Ok(())
            }
            Record::WinAppx { full_name, .. } => {
                let r = tokio::task::spawn_blocking(move || sys::run_ps(sys::REMOVE_APPX, &[("GS_PKG", &full_name)])).await.map_err(|e| AppError::Command(e.to_string()))?;
                if let Err(e) = r {
                    return Err(AppError::Command(if e.contains("0x80073D02") { "The app is in use. Close it and try again.".into() } else { e }));
                }
                match gone(s) {
                    Ok(()) => s.state = "gone".into(),
                    Err(why) => {
                        s.state = "still-installed".into();
                        s.message = Some(why);
                    }
                }
                Ok(())
            }
            _ => Err(AppError::Config("Not a Windows program.".into())),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = s;
        Err(AppError::Config("Windows only.".into()))
    }
}

/// msiexec's exit codes, in words; None = done (gone or not).
pub(crate) fn msi_result(code: u32) -> Option<(&'static str, String)> {
    match code {
        0 | 1605 => None,
        3010 | 1641 => Some(("restart", "Windows finishes removing it after a restart.".into())),
        1602 => Some(("failed", "The uninstall was cancelled.".into())),
        1618 => Some(("failed", "Another installation is running. Try again when it has finished.".into())),
        c => Some(("failed", format!("Windows Installer stopped with code {}.", c))),
    }
}

pub async fn status(s: &mut Session) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        if let Record::WinEntry { msi: Some(_), .. } = &s.record {
            if let Some(code) = sys::exit_code(&s.id) {
                match msi_result(code) {
                    Some(("failed", m)) => {
                        s.state = "failed".into();
                        s.message = Some(m);
                        return Ok(());
                    }
                    Some((_, m)) => s.message = Some(m),
                    None => {}
                }
                settle(s);
                return Ok(());
            }
        }
        if sys::still_running(&s.id, s.process) {
            s.state = "uninstalling".into();
            return Ok(());
        }
        settle(s);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = s;
        Ok(())
    }
}

/// The uninstaller has finished: gone, or still installed.
fn settle(s: &mut Session) {
    s.process = None;
    match gone(s) {
        Ok(()) => s.state = "gone".into(),
        Err(why) => {
            s.state = "still-installed".into();
            s.message = Some(format!("The uninstaller has closed, but {} — cancelled, or it finishes after a restart.", why.trim_end_matches('.')));
        }
    }
}

pub fn gone(s: &Session) -> Result<(), String> {
    #[cfg(windows)]
    {
        match &s.record {
            Record::WinEntry { view, key, .. } => {
                if sys::still_running(&s.id, s.process) {
                    return Err("its uninstaller is still running".into());
                }
                if sys::key_exists(view, key) {
                    Err("its entry in Installed apps is still there".into())
                } else {
                    Ok(())
                }
            }
            Record::WinAppx { name, .. } => match sys::run_ps(sys::COUNT_APPX, &[("GS_NAME", name)]) {
                Ok(v) if v.trim() == "0" => Ok(()),
                Ok(_) => Err("the Store app is still installed (another version or architecture)".into()),
                Err(e) => Err(e),
            },
            _ => Err("Not a Windows program.".into()),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = s;
        Err("Windows only.".into())
    }
}

pub fn others(s: &Session) -> Others {
    let mut o = Others::new(true);
    #[cfg(windows)]
    {
        let removed = match &s.record {
            Record::WinEntry { view, key, .. } => format!("{}:{}", view, key),
            _ => String::new(),
        };
        for e in sys::read_entries() {
            if format!("{}:{}", e.view, e.key) == removed {
                continue;
            }
            let Some(name) = e.name.clone() else { continue };
            let dir = e.install_location.as_deref().and_then(clean_dir);
            o.add_app(&[], &names_of(&name, dir.as_deref(), e.icon.as_deref()), e.publisher.as_deref(), &[]);
            // "VS Revo Group, Ltd." also answers to "VS Revo Group".
            if let Some(p) = e.publisher.as_deref() {
                o.add_app(&[], &[], Some(&matcher::company_stem(p)), &[]);
            }
        }
        if let Ok(l) = sys::appx() {
            let family = match &s.record {
                Record::WinAppx { family, .. } => family.clone(),
                _ => String::new(),
            };
            for a in l.all.iter().filter(|a| a.family != family) {
                o.add_app(std::slice::from_ref(&a.family), std::slice::from_ref(&a.name), None, &[]);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = s;
    o
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var(var).ok().filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn leftovers(s: &Session, others: &Others, registry: bool) -> (Vec<Candidate>, Vec<String>) {
    let mut places = Vec::new();
    if let Some(d) = env_dir("APPDATA") {
        places.push(Place::new(d, "%APPDATA%", Allow::IdsAndNames).vendor().sure());
    }
    if let Some(d) = env_dir("LOCALAPPDATA") {
        places.push(Place::new(d.clone(), "%LOCALAPPDATA%", Allow::IdsAndNames).vendor().sure());
        places.push(Place::new(d.join("Programs"), "%LOCALAPPDATA%\\Programs", Allow::IdsAndNames));
        if let Record::WinAppx { .. } = &s.record {
            places.push(Place::new(d.join("Packages"), "%LOCALAPPDATA%\\Packages", Allow::Ids));
        }
    }
    // A program's other files: the machine-wide places, exact names only.
    if let Record::WinEntry { .. } = &s.record {
        for (var, label) in [("ProgramData", "%ProgramData%"), ("ProgramFiles", "Program Files"), ("ProgramFiles(x86)", "Program Files (x86)")] {
            if let Some(d) = env_dir(var) {
                if !places.iter().any(|p: &Place| lower(&p.path) == lower(&d)) {
                    places.push(Place::new(d, label, Allow::IdsAndNames).vendor().sure());
                }
            }
        }
    }
    let (mut out, mut notes) = super::scan_places(&places, &s.evidence, others);
    // In the machine-wide places, a folder named exactly like the app one
    // level down, under any publisher's folder (the publisher may not be known).
    if let Record::WinEntry { .. } = &s.record {
        let roots: Vec<PathBuf> = ["ProgramData", "ProgramFiles", "ProgramFiles(x86)"].iter().filter_map(|v| env_dir(v)).collect();
        under_any_publisher(&roots, &s.evidence, others, &mut out);
    }
    out.retain(|c| c.path.file_name().is_none_or(|n| !never_dir(&n.to_string_lossy())));
    updater_caches(s, others, &mut out);
    #[cfg(windows)]
    {
        let removed = match &s.record {
            Record::WinEntry { view, key, .. } => format!("{}:{}", view, key),
            _ => String::new(),
        };
        // Its own folders: not shared, not another installed program's.
        let mut own: Vec<PathBuf> = Vec::new();
        for dir in recorded_dirs(&s.app.name, &s.record) {
            let claimed = sys::install_dir_claimed(&dir, &removed);
            if claimed.is_none() && safe_install_dir(&dir) {
                own.push(dir.clone());
            }
            if !dir.is_dir() {
                continue;
            }
            match claimed {
                Some(why) => notes.push(format!("{} is still there, but {} — so it is not offered.", dir.display(), why)),
                None if safe_install_dir(&dir) => out.push(Candidate {
                    path: dir.clone(),
                    root: dir.parent().map(Path::to_path_buf).unwrap_or_default(),
                    place: "Install folder".into(),
                    kind: "folder",
                    m: Match { confidence: Confidence::High, reason: "the folder it was installed in".into() },
                    admin: false,
                    command: None,
                    action: Action::Trash,
                    sized: true,
                    entry: None,
                }),
                None => notes.push(format!("{} is still there, but it is one of Windows' own folders — not offered.", dir.display())),
            }
        }
        sys::shortcuts(s, &own, &mut out);
        startup_leftovers(s, &own, &mut out);
        if registry {
            sys::registry_leftovers(s, others, &mut out);
        }
        let pending = sys::pending_renames();
        out.retain(|c| !pending.contains(&lower(&c.path)));
    }
    #[cfg(not(windows))]
    let _ = registry;
    let roots: Vec<PathBuf> = places.iter().map(|p| p.path.clone()).collect();
    promote_vendor_folders(&mut out, &s.evidence, others, &roots);
    // Whether Windows asks for permission: measured, item by item.
    for c in out.iter_mut().filter(|c| super::file_backed(c.kind)) {
        c.admin = !can_delete(&c.path);
        c.action = Action::Trash;
        c.command = c.admin.then(|| if c.kind == "folder" { format!("rd /s /q \"{}\"   (in a Command Prompt run as administrator)", c.path.display()) } else { format!("del \"{}\"   (as administrator)", c.path.display()) });
    }
    (out, notes)
}

/// `<root>\<any folder>\<the app's exact name>`: a maybe, never ticked.
pub(crate) fn under_any_publisher(roots: &[PathBuf], ev: &Evidence, others: &Others, out: &mut Vec<Candidate>) {
    let names_only = Evidence { ids: vec![], names: ev.names.clone(), groups: vec![], vendor: None };
    for root in roots {
        let label = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for v in rd.flatten() {
            let Ok(md) = v.path().symlink_metadata() else { continue };
            let vname = v.file_name().to_string_lossy().to_string();
            if super::is_link(&md) || !md.is_dir() || never_dir(&vname) || matcher::vendor_matches(&vname, ev.vendor.as_deref()) {
                continue;
            }
            let Ok(inner) = std::fs::read_dir(v.path()) else { continue };
            for c in inner.flatten() {
                let Ok(cmd) = c.path().symlink_metadata() else { continue };
                if super::is_link(&cmd) || !cmd.is_dir() {
                    continue;
                }
                let cname = c.file_name().to_string_lossy().to_string();
                if let Some(m) = matcher::match_entry(&cname, &names_only, others, Allow::IdsAndNames) {
                    out.push(Candidate {
                        path: c.path(),
                        root: root.clone(),
                        place: label.clone(),
                        kind: "folder",
                        m: Match { confidence: Confidence::Medium, reason: format!("in {}: {}", vname, m.reason) },
                        admin: false,
                        command: None,
                        action: Action::Trash,
                        sized: true,
                        entry: None,
                    });
                }
            }
        }
    }
}

/// An Electron app's update cache: `%LOCALAPPDATA%\<its-name>-updater`.
fn updater_caches(s: &Session, others: &Others, out: &mut Vec<Candidate>) {
    let Record::WinEntry { .. } = &s.record else { return };
    let Some(local) = env_dir("LOCALAPPDATA") else { return };
    let mut seen: Vec<String> = Vec::new();
    for n in s.evidence.names.iter().filter(|n| matcher::usable_name(n)) {
        let k = matcher::kebab(n);
        if k.chars().count() < 4 || seen.contains(&k) || others.kebab_claimed(&k) {
            continue;
        }
        seen.push(k.clone());
        let p = local.join(format!("{}-updater", k));
        let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
        if super::is_link(&md) || !md.is_dir() {
            continue;
        }
        out.push(Candidate {
            path: p,
            root: local.clone(),
            place: "%LOCALAPPDATA%".into(),
            kind: "folder",
            m: Match { confidence: Confidence::Medium, reason: format!("its updater's cache, named after the app ({})", n.trim()) },
            admin: false,
            command: None,
            action: Action::Trash,
            sized: true,
            entry: None,
        });
    }
}

/// A publisher's folder that holds nothing but the removed app's folder is
/// offered instead of that folder — never when another installed app has
/// that publisher or name, nor a folder many programs share.
pub(crate) fn promote_vendor_folders(out: &mut [Candidate], ev: &Evidence, others: &Others, place_roots: &[PathBuf]) {
    for c in out.iter_mut() {
        // Sure ones under their publisher; maybes only when the publisher isn't known.
        if c.kind != "folder" || (c.m.confidence != Confidence::High && ev.vendor.is_some()) {
            continue;
        }
        let Some(parent) = c.path.parent().map(Path::to_path_buf) else { continue };
        if place_roots.iter().any(|r| lower(r) == lower(&parent)) {
            continue;
        }
        let Some(pname) = parent.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
        // The publisher's folder; or, when the publisher isn't known, a
        // folder directly in one of the places.
        let publisher = matcher::vendor_matches(&pname, ev.vendor.as_deref());
        let in_place = ev.vendor.is_none() && parent.parent().is_some_and(|g| place_roots.iter().any(|r| lower(r) == lower(g)));
        if !(publisher || in_place) || !matcher::usable_name(&pname) || never_dir(&pname) || others.claims(&pname) || !safe_install_dir(&parent) {
            continue;
        }
        if std::fs::symlink_metadata(&parent).map(|m| super::is_link(&m)).unwrap_or(true) {
            continue;
        }
        let only = std::fs::read_dir(&parent)
            .map(|rd| {
                let v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
                v.len() == 1 && lower(&v[0]) == lower(&c.path)
            })
            .unwrap_or(false);
        let Some(grand) = parent.parent().map(Path::to_path_buf) else { continue };
        if !only {
            continue;
        }
        if lower(&c.root) == lower(&parent) {
            c.root = grand;
        }
        c.m.reason = if c.m.confidence == Confidence::High { format!("{}'s folder, holding nothing but this app", pname) } else { format!("{}, holding nothing but a folder named like the app", pname) };
        c.path = parent;
        c.entry = None;
    }
}

/// Run values that started the removed program from its own folders, while
/// what they start is gone.
#[cfg(windows)]
fn startup_leftovers(s: &Session, own: &[PathBuf], out: &mut Vec<Candidate>) {
    let Record::WinEntry { startup, .. } = &s.record else { return };
    let now = sys::run_values();
    let list: Vec<Startup> = if !startup.is_empty() { startup.clone() } else { now.clone() };
    let list: Vec<Startup> = list.into_iter().filter(|r| command_program(&r.data).is_some_and(|p| own.iter().any(|d| inside(&p, d)))).collect();
    for st in list {
        if !now.iter().any(|r| r == &st) || command_program(&st.data).is_some_and(|p| p.exists()) {
            continue;
        }
        out.push(Candidate {
            path: PathBuf::from(format!(r"{}\{}", sys::run_key_label(&st.view), st.name)),
            root: PathBuf::new(),
            place: "Starts with Windows".into(),
            kind: "startup",
            m: Match { confidence: Confidence::High, reason: format!("started it: {}", st.data) },
            admin: st.view != "HKCU",
            command: None,
            action: Action::RunValue { view: st.view.clone(), name: st.name.clone(), data: st.data.clone() },
            sized: false,
            entry: None,
        });
    }
}

/// A key under HKCU\Software: exported to a .reg file, then deleted.
pub fn delete_registry(key: &str) -> (&'static str, String) {
    #[cfg(windows)]
    {
        sys::delete_registry(key)
    }
    #[cfg(not(windows))]
    {
        let _ = key;
        ("failed", "Windows only.".into())
    }
}

/// A program's entry in Installed apps, exported to a .reg file first.
pub fn delete_uninstall_entry(view: &str, key: &str, name: &str, backups: &Path) -> (&'static str, String) {
    #[cfg(windows)]
    {
        sys::delete_uninstall_entry(view, key, name, backups)
    }
    #[cfg(not(windows))]
    {
        let _ = (view, key, name, backups);
        ("failed", "Windows only.".into())
    }
}

/// A Run value — only while it still holds `data` — exported first.
pub fn delete_run_value(view: &str, name: &str, data: &str, backups: &Path) -> (&'static str, String) {
    #[cfg(windows)]
    {
        sys::delete_run_value(view, name, data, backups)
    }
    #[cfg(not(windows))]
    {
        let _ = (view, name, data, backups);
        ("failed", "Windows only.".into())
    }
}

/// The running programs that have files of `dir` open or loaded, by name —
/// Windows' Restart Manager, what installers use to say "files in use".
pub fn users_of(dir: &Path) -> Vec<String> {
    #[cfg(windows)]
    {
        sys::users_of(dir)
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        Vec::new()
    }
}

/// A file or folder deleted when Windows next starts — for good (what is
/// in use can't go to the Recycle Bin). Only the administrator may.
pub fn delete_at_restart(path: &Path) -> (&'static str, String) {
    #[cfg(windows)]
    {
        sys::delete_at_restart(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        ("failed", "Windows only.".into())
    }
}

/// The program's own folders: not shared, not another installed program's.
pub(crate) fn own_dirs(s: &Session) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let removed = match &s.record {
            Record::WinEntry { view, key, .. } => format!("{}:{}", view, key),
            _ => String::new(),
        };
        recorded_dirs(&s.app.name, &s.record).into_iter().filter(|d| safe_install_dir(d) && sys::install_dir_claimed(d, &removed).is_none()).collect()
    }
    #[cfg(not(windows))]
    {
        recorded_dirs(&s.app.name, &s.record).into_iter().filter(|d| safe_install_dir(d)).collect()
    }
}

/// Close what runs from the program's own folders: its windows are asked to
/// close; what is still running after `grace` is ended.
pub fn close(s: &Session, grace: std::time::Duration) -> Closed {
    #[cfg(windows)]
    {
        sys::close_in(&own_dirs(s), grace)
    }
    #[cfg(not(windows))]
    {
        let _ = (s, grace);
        Closed::default()
    }
}

/// A registry backup's file name: when, which registry view, what — and a
/// part no other backup has, so none is ever written over.
pub(crate) fn backup_name(view: &str, label: &str) -> String {
    let unique = uuid::Uuid::new_v4().simple().to_string();
    format!("{}-{}-{}-{}.reg", chrono::Local::now().format("%Y%m%d-%H%M%S"), view, label.replace(|c: char| !c.is_alphanumeric(), "_"), &unique[..8])
}

#[cfg(windows)]
pub fn file_id(path: &Path) -> Option<(u64, u64)> {
    sys::file_id(path)
}

#[cfg(windows)]
pub fn recycle_refusal(path: &Path) -> Option<String> {
    sys::recycle_refusal(path)
}

#[cfg(windows)]
mod sys {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::{Mutex, OnceLock};
    use winreg::enums::*;
    use winreg::RegKey;

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    const VIEWS: [(&str, isize, u32); 3] = [("HKLM64", 0, KEY_WOW64_64KEY), ("HKLM32", 0, KEY_WOW64_32KEY), ("HKCU", 1, 0)];

    fn hive(i: isize) -> RegKey {
        RegKey::predef(if i == 0 { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER })
    }

    pub fn read_entries() -> Vec<RegEntry> {
        let mut out = Vec::new();
        for (view, h, flag) in VIEWS {
            let Ok(un) = hive(h).open_subkey_with_flags(UNINSTALL, KEY_READ | flag) else { continue };
            for key in un.enum_keys().flatten() {
                let Ok(k) = un.open_subkey_with_flags(&key, KEY_READ | flag) else { continue };
                let s = |v: &str| k.get_value::<String, _>(v).ok().map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
                let d = |v: &str| k.get_value::<u32, _>(v).ok();
                out.push(RegEntry {
                    view: view.into(),
                    key,
                    name: s("DisplayName"),
                    version: s("DisplayVersion"),
                    publisher: s("Publisher"),
                    install_location: s("InstallLocation"),
                    install_date: s("InstallDate"),
                    uninstall: s("UninstallString"),
                    windows_installer: d("WindowsInstaller") == Some(1),
                    estimated_kb: d("EstimatedSize"),
                    icon: s("DisplayIcon"),
                    system_component: d("SystemComponent") == Some(1),
                    parent: s("ParentKeyName"),
                    release_type: s("ReleaseType"),
                    no_remove: d("NoRemove") == Some(1),
                    bundle: s("BundleCachePath").is_some() || s("BundleUpgradeCode").is_some() || s("BundleProviderKey").is_some(),
                });
            }
        }
        out
    }

    pub fn key_exists(view: &str, key: &str) -> bool {
        let Some((_, h, flag)) = VIEWS.iter().find(|v| v.0 == view) else { return false };
        hive(*h).open_subkey_with_flags(format!(r"{}\{}", UNINSTALL, key), KEY_READ | flag).is_ok()
    }

    /// PowerShell 5.1 (the Store cmdlets need it). Scripts carry no data:
    /// what they need comes in environment variables, and what they print
    /// is hex of UTF-8 — the console's code page never touches it.
    pub fn run_ps(script: &str, env: &[(&str, &str)]) -> Result<String, String> {
        let ps = format!(r"{}\System32\WindowsPowerShell\v1.0\powershell.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
        let mut cmd = crate::spawn::command(&ps);
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().map_err(|e| format!("PowerShell could not start: {}", e))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).lines().take(3).collect::<Vec<_>>().join(" "));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    const LIST_APPX: &str = r#"$ErrorActionPreference = 'Stop'
$main = @(Get-AppxPackage -PackageTypeFilter Main | Where-Object { -not $_.NonRemovable -and [string]$_.SignatureKind -ne 'System' } | ForEach-Object { [pscustomobject]@{ name = $_.Name; full = $_.PackageFullName; family = $_.PackageFamilyName; publisher = $_.Publisher; version = $_.Version.ToString(); dir = $_.InstallLocation } })
$all = @(Get-AppxPackage | ForEach-Object { [pscustomobject]@{ name = $_.Name; family = $_.PackageFamilyName } })
$j = ConvertTo-Json -InputObject @{ main = $main; all = $all } -Depth 4 -Compress
[System.BitConverter]::ToString([System.Text.Encoding]::UTF8.GetBytes($j)).Replace('-', '')"#;

    pub const REMOVE_APPX: &str = "$ErrorActionPreference = 'Stop'\nRemove-AppxPackage -Package $env:GS_PKG\n'ok'";
    pub const COUNT_APPX: &str = "$ErrorActionPreference = 'Stop'\n@(Get-AppxPackage -Name $env:GS_NAME).Count";

    const SHORTCUTS: &str = r#"$ErrorActionPreference = 'SilentlyContinue'
$paths = [System.IO.File]::ReadAllText($env:GS_IN, [System.Text.Encoding]::UTF8) | ConvertFrom-Json
$sh = New-Object -ComObject WScript.Shell
$out = @(foreach ($p in @($paths)) { [pscustomobject]@{ p = $p; t = [string]$sh.CreateShortcut($p).TargetPath } })
$j = ConvertTo-Json -InputObject $out -Compress
[System.BitConverter]::ToString([System.Text.Encoding]::UTF8.GetBytes($j)).Replace('-', '')"#;

    const SIGNER: &str = "$s = Get-AuthenticodeSignature -LiteralPath $env:GS_FILE\nif ($s.Status -eq 'Valid') { $s.SignerCertificate.GetNameInfo('SimpleName', $false) } else { [string]$s.Status }";

    pub fn appx() -> Result<AppxListing, String> {
        let hex = run_ps(LIST_APPX, &[])?;
        let bytes = unhex(&hex).ok_or("unreadable answer")?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    pub fn signer(file: &Path) -> Option<String> {
        if !file.is_file() {
            return None;
        }
        run_ps(SIGNER, &[("GS_FILE", &file.to_string_lossy())]).ok().filter(|s| !s.is_empty())
    }

    pub fn long_path(p: &Path) -> PathBuf {
        use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
        let w = wide(p.as_os_str());
        let mut buf = vec![0u16; 1024];
        // SAFETY: both buffers are valid for the lengths given.
        let n = unsafe { GetLongPathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n == 0 || n >= buf.len() {
            return p.to_path_buf();
        }
        PathBuf::from(String::from_utf16_lossy(&buf[..n]))
    }

    pub fn file_id(path: &Path) -> Option<(u64, u64)> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
        let w = wide(path.as_os_str());
        // SAFETY: a valid wide path; the handle is closed below.
        unsafe {
            let h = CreateFileW(w.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, std::ptr::null_mut());
            if h == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            let ok = GetFileInformationByHandle(h, &mut info) != 0;
            CloseHandle(h);
            ok.then_some((info.dwVolumeSerialNumber as u64, ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64))
        }
    }

    pub struct Proc {
        pub pid: u32,
        pub parent: u32,
        pub created: u64,
        pub path: Option<PathBuf>,
    }

    fn created_and_path(pid: u32) -> Option<(u64, Option<PathBuf>)> {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: the handle is checked and closed; buffers are sized.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
            let created = (GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0).then_some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64);
            let mut buf = vec![0u16; 1024];
            let mut len = buf.len() as u32;
            let path = (QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
            CloseHandle(h);
            created.map(|c| (c, path))
        }
    }

    pub fn processes() -> Vec<Proc> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
        let mut out = Vec::new();
        // SAFETY: the snapshot handle is checked and closed; the entry is sized.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok {
                let (created, path) = created_and_path(e.th32ProcessID).unwrap_or((0, None));
                out.push(Proc { pid: e.th32ProcessID, parent: e.th32ParentProcessID, created, path });
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        out
    }

    /// The processes an uninstall started (and theirs), and msiexec's exit codes.
    struct Tracking {
        procs: HashMap<String, Vec<(u32, u64)>>,
        exits: HashMap<String, u32>,
    }

    fn tracking() -> &'static Mutex<Tracking> {
        static T: OnceLock<Mutex<Tracking>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(Tracking { procs: HashMap::new(), exits: HashMap::new() }))
    }

    /// Start the uninstaller the way Explorer would ("open"): its manifest
    /// decides whether Windows asks for permission.
    pub fn launch(program: &str, args: &str, session: &str) -> Result<(u32, u64), String> {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessId, WaitForSingleObject, INFINITE};
        use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let verb = wide(OsStr::new("open"));
        let file = wide(OsStr::new(program));
        let params = wide(OsStr::new(args));
        // SAFETY: a zeroed SHELLEXECUTEINFOW is the documented start; the
        // wide strings outlive the call; the handle is closed by its waiter.
        unsafe {
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = if args.is_empty() { std::ptr::null() } else { params.as_ptr() };
            info.nShow = SW_SHOWNORMAL;
            if ShellExecuteExW(&mut info) == 0 {
                let err = GetLastError();
                return Err(if err == ERROR_CANCELLED { "The Windows permission prompt was declined, so nothing was removed.".into() } else { format!("Windows could not start the uninstaller (error {}).", err) });
            }
            if info.hProcess.is_null() {
                return Err("Windows started the uninstaller but gave no process to follow; press \"It's finished\" when it is done.".into());
            }
            let pid = GetProcessId(info.hProcess);
            let created = created_and_path(pid).map(|c| c.0).unwrap_or(0);
            tracking().lock().unwrap_or_else(|e| e.into_inner()).procs.insert(session.to_string(), vec![(pid, created)]);
            let handle = info.hProcess as usize;
            let session = session.to_string();
            std::thread::spawn(move || {
                let h = handle as windows_sys::Win32::Foundation::HANDLE;
                WaitForSingleObject(h, INFINITE);
                let mut code = 0u32;
                GetExitCodeProcess(h, &mut code);
                CloseHandle(h);
                tracking().lock().unwrap_or_else(|e| e.into_inner()).exits.insert(session, code);
            });
            Ok((pid, created))
        }
    }

    pub fn exit_code(session: &str) -> Option<u32> {
        tracking().lock().unwrap_or_else(|e| e.into_inner()).exits.get(session).copied()
    }

    /// Whether the uninstaller, or any process it started (an NSIS or Inno
    /// uninstaller re-launches itself from %TEMP%), still runs.
    pub fn still_running(session: &str, started: Option<(u32, u64)>) -> bool {
        let procs = processes();
        let mut t = tracking().lock().unwrap_or_else(|e| e.into_inner());
        let tracked = t.procs.entry(session.to_string()).or_insert_with(|| started.into_iter().collect());
        if tracked.is_empty() {
            return false;
        }
        let since = tracked.iter().map(|x| x.1).min().unwrap_or(0);
        // Children of what is tracked, started after it.
        loop {
            let pids: Vec<u32> = tracked.iter().map(|x| x.0).collect();
            let new: Vec<(u32, u64)> = procs.iter().filter(|p| pids.contains(&p.parent) && p.created >= since && !tracked.iter().any(|x| x.0 == p.pid)).map(|p| (p.pid, p.created)).collect();
            if new.is_empty() {
                break;
            }
            tracked.extend(new);
        }
        // The living ones: same pid and same start time.
        tracked.retain(|(pid, created)| procs.iter().any(|p| p.pid == *pid && (p.created == *created || *created == 0)));
        !tracked.is_empty()
    }

    fn command_path(cmd: &str) -> Option<PathBuf> {
        command_program(cmd)
    }

    /// Something still installed lives in that folder: another program's
    /// entry, a service or driver, a shared DLL. `removed` is the
    /// `view:key` of the program being removed.
    pub fn install_dir_claimed(dir: &Path, removed: &str) -> Option<String> {
        let d = format!("{}\\", lower(dir));
        let inside = |p: &Path| lower(p).starts_with(&d) || lower(p) == lower(dir);
        for e in read_entries() {
            if format!("{}:{}", e.view, e.key) == removed {
                continue;
            }
            let paths = [e.install_location.as_deref().and_then(clean_dir), e.icon.as_deref().map(|i| PathBuf::from(i.split(',').next().unwrap_or(i).trim_matches('"'))), e.uninstall.as_deref().and_then(command_path)];
            if paths.iter().flatten().any(|p| inside(p)) {
                return Some(format!("another program ({}) is installed there", e.name.unwrap_or(e.key)));
            }
        }
        let lm = RegKey::predef(HKEY_LOCAL_MACHINE);
        if let Ok(services) = lm.open_subkey_with_flags(r"SYSTEM\CurrentControlSet\Services", KEY_READ) {
            for name in services.enum_keys().flatten() {
                let Ok(k) = services.open_subkey_with_flags(&name, KEY_READ) else { continue };
                let Ok(img) = k.get_value::<String, _>("ImagePath") else { continue };
                if command_path(&img).is_some_and(|p| inside(&p)) {
                    return Some(format!("the service or driver \"{}\" runs from it", name));
                }
            }
        }
        for flag in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            if let Ok(k) = lm.open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows\CurrentVersion\SharedDLLs", KEY_READ | flag) {
                for (name, _) in k.enum_values().flatten() {
                    if inside(Path::new(&name)) {
                        return Some("a shared library other programs count on is in it".into());
                    }
                }
            }
        }
        None
    }

    /// Paths Windows deletes at the next restart.
    pub fn pending_renames() -> Vec<String> {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(r"SYSTEM\CurrentControlSet\Control\Session Manager", KEY_READ)
            .ok()
            .and_then(|k| k.get_value::<Vec<String>, _>("PendingFileRenameOperations").ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !p.is_empty())
            .map(|p| p.trim_start_matches(r"\??\").to_lowercase().trim_end_matches('\\').to_string())
            .collect()
    }

    fn lnk_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let Ok(md) = e.path().symlink_metadata() else { continue };
            if super::super::is_link(&md) {
                continue;
            }
            if md.is_dir() && depth > 0 {
                lnk_files(&e.path(), depth - 1, out);
            } else if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                out.push(e.path());
            }
        }
    }

    /// Where shortcuts are: (folder, shared by everyone, label).
    fn shortcut_places() -> Vec<(PathBuf, bool, String)> {
        let mut places: Vec<(PathBuf, bool, String)> = Vec::new();
        if let Some(a) = env_dir("APPDATA") {
            places.push((a.join(r"Microsoft\Windows\Start Menu\Programs"), false, "Start menu".into()));
        }
        if let Some(p) = env_dir("ProgramData") {
            places.push((p.join(r"Microsoft\Windows\Start Menu\Programs"), true, "Start menu (everyone)".into()));
        }
        if let Some(d) = dirs::desktop_dir() {
            places.push((d, false, "Desktop".into()));
        }
        if let Some(p) = env_dir("PUBLIC") {
            places.push((p.join("Desktop"), true, "Desktop (everyone)".into()));
        }
        places
    }

    /// What each shortcut starts, by its path in lower case.
    fn resolve_targets(files: &[PathBuf]) -> HashMap<String, String> {
        let input = std::env::temp_dir().join(format!("gitswitch-lnk-{}-{}.json", std::process::id(), uuid::Uuid::new_v4()));
        let list: Vec<String> = files.iter().map(|f| f.to_string_lossy().to_string()).collect();
        if std::fs::write(&input, serde_json::to_vec(&list).unwrap_or_default()).is_err() {
            return HashMap::new();
        }
        let answer = run_ps(SHORTCUTS, &[("GS_IN", &input.to_string_lossy())]);
        let _ = std::fs::remove_file(&input);
        let Some(targets) = answer.ok().and_then(|h| unhex(&h)).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else { return HashMap::new() };
        // ConvertTo-Json writes one object, not a list, for a single shortcut.
        let targets = match targets {
            serde_json::Value::Array(v) => v,
            other => vec![other],
        };
        targets.iter().filter_map(|t| Some((t["p"].as_str()?.to_lowercase(), t["t"].as_str().unwrap_or("").to_string()))).collect()
    }

    /// The programs that shortcuts named like the app start.
    pub fn shortcut_targets(names: &[String]) -> Vec<PathBuf> {
        let keys: Vec<String> = names.iter().filter(|n| matcher::usable_name(n)).map(|n| n.trim().to_lowercase().replace(' ', "")).collect();
        if keys.is_empty() {
            return vec![];
        }
        let mut files = Vec::new();
        for (dir, _, _) in shortcut_places() {
            lnk_files(&dir, if dir.ends_with("Desktop") { 0 } else { 2 }, &mut files);
        }
        // Named exactly like the app ("Disk Drill.lnk", "Disk Drill (x64).lnk").
        files.retain(|f| {
            f.file_stem().map(|s| s.to_string_lossy().to_string()).is_some_and(|s| {
                let whole = s.to_lowercase().replace(' ', "");
                let plain = strip_version(&s).to_lowercase().replace(' ', "");
                keys.contains(&whole) || keys.contains(&plain)
            })
        });
        if files.is_empty() {
            return vec![];
        }
        resolve_targets(&files).into_values().filter(|t| !t.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute()).collect()
    }

    /// Another installed program (not `removed`, "view:key") has this name.
    pub fn name_shared(name: &str, removed: &str) -> bool {
        let n = strip_version(name).to_lowercase();
        read_entries().iter().any(|e| format!("{}:{}", e.view, e.key) != removed && e.name.as_deref().is_some_and(|x| strip_version(x).to_lowercase() == n))
    }

    const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";

    /// Programs App Paths registers under one of the app's names.
    pub fn app_paths(names: &[String]) -> Vec<PathBuf> {
        let keys: Vec<String> = names.iter().filter(|n| matcher::usable_name(n)).map(|n| n.trim().to_lowercase().replace(' ', "")).collect();
        let mut out = Vec::new();
        for (_, h, flag) in VIEWS {
            let Ok(ap) = hive(h).open_subkey_with_flags(APP_PATHS, KEY_READ | flag) else { continue };
            for k in ap.enum_keys().flatten() {
                let l = k.to_lowercase();
                let stem = l.strip_suffix(".exe").unwrap_or(&l).replace(' ', "");
                if !keys.contains(&stem) {
                    continue;
                }
                let Ok(sub) = ap.open_subkey_with_flags(&k, KEY_READ | flag) else { continue };
                if let Ok(v) = sub.get_value::<String, _>("") {
                    let p = PathBuf::from(expand(v.trim().trim_matches('"')));
                    if p.is_absolute() {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    pub const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";

    /// The Run key's full name for a view, as shown.
    pub fn run_key_label(view: &str) -> String {
        match view {
            "HKCU" => format!(r"HKCU\{}", RUN),
            "HKLM32" => r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run".to_string(),
            _ => format!(r"HKLM\{}", RUN),
        }
    }

    /// Every Run value: yours, and everyone's in both registry views.
    pub fn run_values() -> Vec<Startup> {
        let mut out = Vec::new();
        for (view, h, flag) in VIEWS {
            let Ok(k) = hive(h).open_subkey_with_flags(RUN, KEY_READ | flag) else { continue };
            for (name, _) in k.enum_values().flatten() {
                if name.is_empty() {
                    continue;
                }
                if let Ok(data) = k.get_value::<String, _>(&name) {
                    out.push(Startup { view: view.into(), name, data });
                }
            }
        }
        out
    }

    /// The PC maker, and the Store apps provisioned with Windows or by the maker.
    pub fn system_facts() -> SystemFacts {
        let lm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let maker = lm
            .open_subkey_with_flags(r"HARDWARE\DESCRIPTION\System\BIOS", KEY_READ)
            .ok()
            .and_then(|k| k.get_value::<String, _>("SystemManufacturer").ok())
            .and_then(|m| maker_word(&m));
        let provisioned = lm
            .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Appx\AppxAllUserStore\Applications", KEY_READ | KEY_WOW64_64KEY)
            .map(|k| k.enum_keys().flatten().filter_map(|n| provisioned_family(&n)).collect())
            .unwrap_or_default();
        SystemFacts { maker, provisioned }
    }

    /// Opening it for deleting works (else it needs the administrator).
    pub fn can_delete(path: &Path) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{CreateFileW, DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
        let w = wide(path.as_os_str());
        // SAFETY: a valid wide path; the handle is closed below.
        unsafe {
            let h = CreateFileW(w.as_ptr(), DELETE, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, std::ptr::null_mut());
            if h == INVALID_HANDLE_VALUE {
                return GetLastError() != ERROR_ACCESS_DENIED;
            }
            CloseHandle(h);
            true
        }
    }

    /// A folder's files (no links followed), its programs and libraries first.
    fn files_in(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let Ok(md) = e.path().symlink_metadata() else { continue };
            if super::super::is_link(&md) {
                continue;
            }
            if md.is_dir() {
                if depth > 0 {
                    files_in(&e.path(), depth - 1, out);
                }
            } else {
                out.push(e.path());
            }
        }
    }

    pub fn users_of(dir: &Path) -> Vec<String> {
        use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
        use windows_sys::Win32::System::RestartManager::{RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY, RM_PROCESS_INFO};
        let mut files = Vec::new();
        if dir.is_dir() {
            files_in(dir, 6, &mut files);
        } else {
            files.push(dir.to_path_buf());
        }
        let code = |p: &PathBuf| p.extension().is_some_and(|x| ["dll", "exe", "sys", "ocx"].iter().any(|c| x.eq_ignore_ascii_case(c)));
        files.sort_by_key(|f| !code(f));
        files.truncate(400);
        if files.is_empty() {
            return Vec::new();
        }
        let wides: Vec<Vec<u16>> = files.iter().map(|f| wide(f.as_os_str())).collect();
        let ptrs: Vec<*const u16> = wides.iter().map(|w| w.as_ptr()).collect();
        let mut names: Vec<String> = Vec::new();
        // SAFETY: the session is ended below; buffers are sized as passed;
        // the wide strings outlive the calls.
        unsafe {
            let mut session = 0u32;
            let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
            if RmStartSession(&mut session, 0, key.as_mut_ptr()) != 0 {
                return names;
            }
            if RmRegisterResources(session, ptrs.len() as u32, ptrs.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) == 0 {
                let (mut needed, mut count, mut reasons) = (0u32, 0u32, 0u32);
                let r = RmGetList(session, &mut needed, &mut count, std::ptr::null_mut(), &mut reasons);
                if (r == ERROR_MORE_DATA || r == 0) && needed > 0 {
                    let mut infos: Vec<RM_PROCESS_INFO> = vec![std::mem::zeroed(); needed as usize];
                    count = needed;
                    if RmGetList(session, &mut needed, &mut count, infos.as_mut_ptr(), &mut reasons) == 0 {
                        for i in &infos[..(count as usize).min(infos.len())] {
                            let len = i.strAppName.iter().position(|c| *c == 0).unwrap_or(i.strAppName.len());
                            let n = String::from_utf16_lossy(&i.strAppName[..len]).trim().to_string();
                            if !n.is_empty() && !names.contains(&n) {
                                names.push(n);
                            }
                        }
                    }
                }
            }
            RmEndSession(session);
        }
        names
    }

    pub fn delete_at_restart(path: &Path) -> (&'static str, String) {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED};
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
        // Files first, then folders from the deepest up: Windows works the
        // list in order when it starts. A link is removed as itself.
        fn walk(p: &Path, files: &mut Vec<PathBuf>, dirs: &mut Vec<PathBuf>) {
            let Ok(md) = p.symlink_metadata() else { return };
            if md.is_dir() && !super::super::is_link(&md) {
                dirs.push(p.to_path_buf());
                if let Ok(rd) = std::fs::read_dir(p) {
                    for e in rd.flatten() {
                        walk(&e.path(), files, dirs);
                    }
                }
            } else {
                files.push(p.to_path_buf());
            }
        }
        let (mut files, mut dirs) = (Vec::new(), Vec::new());
        walk(path, &mut files, &mut dirs);
        dirs.reverse();
        let (mut failed, mut denied) = (0usize, false);
        for p in files.iter().chain(dirs.iter()) {
            let w = wide(p.as_os_str());
            // SAFETY: a valid wide path; no new name (delete).
            if unsafe { MoveFileExW(w.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) } == 0 {
                failed += 1;
                denied |= unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
            }
        }
        if failed == 0 {
            ("at-restart", "Windows deletes it when this PC restarts — for good, since what is in use can't go to the Recycle Bin.".into())
        } else if denied {
            ("needs-admin", "Only the administrator can have Windows delete it at the next restart.".into())
        } else {
            ("failed", format!("{} of its {} files and folders could not be scheduled for the next restart.", failed, files.len() + dirs.len()))
        }
    }

    /// Ask the windows of these processes to close.
    fn post_close(pids: &[u32]) {
        use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE};
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
            // SAFETY: lparam is the &Vec<u32> passed below, alive for the call.
            let pids = &*(lparam as *const Vec<u32>);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pids.contains(&pid) && IsWindowVisible(hwnd) != 0 {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            1
        }
        let list: Vec<u32> = pids.to_vec();
        // SAFETY: the callback only reads the list, which outlives EnumWindows.
        unsafe {
            EnumWindows(Some(each), &list as *const Vec<u32> as LPARAM);
        }
    }

    pub fn close_in(dirs: &[PathBuf], grace: std::time::Duration) -> Closed {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        if dirs.is_empty() {
            return Closed::default();
        }
        let own = || -> Vec<(u32, u64, String)> {
            processes()
                .into_iter()
                .filter_map(|p| {
                    let path = p.path?;
                    dirs.iter().any(|d| inside(&path, d)).then(|| (p.pid, p.created, path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
                })
                .collect()
        };
        let first = own();
        if first.is_empty() {
            return Closed::default();
        }
        let alive = |list: &[(u32, u64, String)]| -> Vec<(u32, u64, String)> {
            let now = own();
            list.iter().filter(|(pid, created, _)| now.iter().any(|(p, c, _)| p == pid && c == created)).cloned().collect()
        };
        post_close(&first.iter().map(|p| p.0).collect::<Vec<_>>());
        let until = std::time::Instant::now() + grace;
        let mut left = alive(&first);
        while !left.is_empty() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(250));
            left = alive(&left);
        }
        for (pid, _, _) in &left {
            // SAFETY: the handle is checked and closed.
            unsafe {
                let h = OpenProcess(PROCESS_TERMINATE, 0, *pid);
                if !h.is_null() {
                    TerminateProcess(h, 1);
                    CloseHandle(h);
                }
            }
        }
        if !left.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(700));
            left = alive(&left);
        }
        let names = |v: &[(u32, u64, String)]| -> Vec<String> {
            let mut n: Vec<String> = v.iter().map(|p| p.2.clone()).collect();
            n.sort();
            n.dedup();
            n
        };
        let still = names(&left);
        let closed = names(&first).into_iter().filter(|n| !still.contains(n)).collect();
        Closed { closed, left: still }
    }

    fn view_of(view: &str) -> Option<(isize, u32)> {
        VIEWS.iter().find(|v| v.0 == view).map(|v| (v.1, v.2))
    }

    /// `reg export` of a key, into the backups folder.
    fn export(view: &str, sub: &str, backups: &Path, label: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(backups).map_err(|e| format!("The backup folder could not be made ({}), so it was kept.", e))?;
        let file = backups.join(backup_name(view, label));
        if std::fs::symlink_metadata(&file).is_ok() {
            return Err("The backup could not be written, so it was kept.".into());
        }
        let reg = format!(r"{}\System32\reg.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
        let full = format!(r"{}\{}", if view == "HKCU" { "HKCU" } else { "HKLM" }, sub);
        let out = crate::spawn::command(&reg).args(["export", &full]).arg(&file).args(["/y", if view == "HKLM32" { "/reg:32" } else { "/reg:64" }]).output();
        if out.as_ref().is_ok_and(|o| o.status.success()) {
            Ok(file)
        } else {
            Err("The backup could not be written, so it was kept.".into())
        }
    }

    pub fn delete_uninstall_entry(view: &str, key: &str, name: &str, backups: &Path) -> (&'static str, String) {
        let Some((h, flag)) = view_of(view) else { return ("refused", "Not an entry GitSwitch removes.".into()) };
        if key.trim().is_empty() || key.contains(['\\', '/']) {
            return ("refused", "Not an entry GitSwitch removes.".into());
        }
        let sub = format!(r"{}\{}", UNINSTALL, key);
        let Ok(k) = hive(h).open_subkey_with_flags(&sub, KEY_READ | flag) else { return ("gone", "Already gone from Installed apps.".into()) };
        let shown = k.get_value::<String, _>("DisplayName").map(|x| x.trim().to_string()).unwrap_or_default();
        drop(k);
        // The same program after an update ("Disk Drill 6.3" -> "Disk Drill 6.4") still is.
        if !name.trim().is_empty() && shown != name.trim() && strip_version(&shown).to_lowercase() != strip_version(name).to_lowercase() {
            return ("changed", "That entry now names another program, so it was kept.".into());
        }
        let backup = match export(view, &sub, backups, key) {
            Ok(b) => b,
            Err(e) => return ("failed", e),
        };
        match hive(h).open_subkey_with_flags(UNINSTALL, KEY_ALL_ACCESS | flag).and_then(|u| u.delete_subkey_all(key)) {
            Ok(()) => ("removed", format!("Removed from Installed apps; the backup is {}", backup.display())),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => ("needs-admin", format!("It needs the administrator (the backup is {}).", backup.display())),
            Err(e) => ("failed", format!("{} (the backup is {})", e, backup.display())),
        }
    }

    pub fn delete_run_value(view: &str, name: &str, data: &str, backups: &Path) -> (&'static str, String) {
        let Some((h, flag)) = view_of(view) else { return ("refused", "Not a start-up entry GitSwitch removes.".into()) };
        if name.is_empty() {
            return ("refused", "Not a start-up entry GitSwitch removes.".into());
        }
        let Ok(k) = hive(h).open_subkey_with_flags(RUN, KEY_READ | flag) else { return ("gone", "Already gone.".into()) };
        match k.get_value::<String, _>(name) {
            Err(_) => return ("gone", "Already gone.".into()),
            Ok(v) if v != data => return ("changed", "It now starts something else, so it was kept.".into()),
            Ok(_) => {}
        }
        drop(k);
        let backup = match export(view, RUN, backups, &format!("run-{}", name)) {
            Ok(b) => b,
            Err(e) => return ("failed", e),
        };
        match hive(h).open_subkey_with_flags(RUN, KEY_SET_VALUE | flag).and_then(|k| k.delete_value(name)) {
            Ok(()) => ("removed", format!("Windows no longer starts it; the backup is {}", backup.display())),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => ("needs-admin", format!("It needs the administrator (the backup is {}).", backup.display())),
            Err(e) => ("failed", format!("{} (the backup is {})", e, backup.display())),
        }
    }

    /// Shortcuts in the Start menu and on the desktop that started the
    /// removed program — by their target, never by their name.
    pub fn shortcuts(s: &Session, own: &[PathBuf], out: &mut Vec<Candidate>) {
        let Record::WinEntry { icon, .. } = &s.record else { return };
        let places = shortcut_places();
        let mut files: Vec<(PathBuf, usize)> = Vec::new();
        for (i, (dir, _, _)) in places.iter().enumerate() {
            let mut v = Vec::new();
            lnk_files(dir, if dir.ends_with("Desktop") { 0 } else { 2 }, &mut v);
            files.extend(v.into_iter().map(|f| (f, i)));
        }
        if files.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = files.iter().map(|f| f.0.clone()).collect();
        let target_of = resolve_targets(&paths);
        if target_of.is_empty() {
            return;
        }
        let stem = icon.as_deref().and_then(exe_stem).map(|s| s.to_lowercase());
        let dirs_l: Vec<String> = own.iter().map(|d| format!("{}\\", lower(d))).collect();
        let qualifies = |f: &Path| -> Option<Confidence> {
            let t = target_of.get(&f.to_string_lossy().to_lowercase())?;
            if t.is_empty() {
                return None;
            }
            let tl = t.to_lowercase();
            if dirs_l.iter().any(|d| tl.starts_with(d)) {
                return Some(Confidence::High);
            }
            let tstem = exe_stem(t).map(|x| x.to_lowercase());
            (!Path::new(t).exists() && tstem.is_some() && tstem == stem).then_some(Confidence::Medium)
        };
        let mut folders: HashMap<PathBuf, (bool, usize)> = HashMap::new();
        for (f, i) in &files {
            let (root, admin, label) = &places[*i];
            let Some(conf) = qualifies(f) else {
                if let Some(parent) = f.parent().filter(|p| p != root) {
                    folders.entry(parent.to_path_buf()).or_insert((true, *i)).0 = false;
                }
                continue;
            };
            if let Some(parent) = f.parent().filter(|p| p != root) {
                folders.entry(parent.to_path_buf()).or_insert((true, *i));
            }
            out.push(Candidate {
                path: f.clone(),
                root: root.clone(),
                place: label.clone(),
                kind: "file",
                m: Match { confidence: conf, reason: "a shortcut that started it".into() },
                admin: *admin,
                command: admin.then(|| format!("del \"{}\"   (as administrator)", f.display())),
                action: if *admin { Action::None } else { Action::Trash },
                sized: false,
                entry: None,
            });
        }
        // A Start menu folder only when every shortcut in it qualified and
        // nothing else is in it.
        for (folder, (all, i)) in folders {
            let (root, admin, label) = &places[i];
            let only_lnks = std::fs::read_dir(&folder).map(|rd| rd.flatten().all(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")))).unwrap_or(false);
            if all && only_lnks && folder.parent() == Some(root.as_path()) {
                out.push(Candidate {
                    path: folder.clone(),
                    root: root.clone(),
                    place: label.clone(),
                    kind: "folder",
                    m: Match { confidence: Confidence::High, reason: "a Start menu folder holding only its shortcuts".into() },
                    admin: *admin,
                    command: admin.then(|| format!("rd /s /q \"{}\"   (as administrator)", folder.display())),
                    action: if *admin { Action::None } else { Action::Trash },
                    sized: false,
                    entry: None,
                });
            }
        }
    }

    const REG_NEVER: &[&str] = &["microsoft", "classes", "policies", "wow6432node", "clients", "registeredapplications", "intel", "nvidia corporation", "google", "apple inc.", "mozilla"];

    /// HKCU\Software\<Publisher>\<App> and HKCU\Software\<App> — exact names only.
    pub fn registry_leftovers(s: &Session, others: &Others, out: &mut Vec<Candidate>) {
        let names_only = Evidence { ids: vec![], names: s.evidence.names.clone(), groups: vec![], vendor: None };
        let Ok(sw) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags("Software", KEY_READ | KEY_WOW64_64KEY) else { return };
        let vendor = s.evidence.vendor.as_deref().map(|v| v.trim().to_lowercase());
        for key in sw.enum_keys().flatten() {
            let k = key.to_lowercase();
            if REG_NEVER.contains(&k.as_str()) {
                continue;
            }
            if let Some(m) = matcher::match_entry(&key, &names_only, others, Allow::IdsAndNames) {
                out.push(reg_candidate(format!(r"HKCU\Software\{}", key), m));
            } else if vendor.as_deref() == Some(k.as_str()) && super::matcher::usable_name(&key) {
                let Ok(v) = sw.open_subkey_with_flags(&key, KEY_READ | KEY_WOW64_64KEY) else { continue };
                for child in v.enum_keys().flatten() {
                    if let Some(m) = matcher::match_entry(&child, &names_only, others, Allow::IdsAndNames) {
                        out.push(reg_candidate(format!(r"HKCU\Software\{}\{}", key, child), Match { confidence: m.confidence, reason: format!("in {}: {}", key, m.reason) }));
                    }
                }
            }
        }
    }

    fn reg_candidate(key: String, m: Match) -> Candidate {
        Candidate {
            path: PathBuf::from(&key),
            root: PathBuf::new(),
            place: "Registry (yours)".into(),
            kind: "registry",
            m: Match { confidence: Confidence::Medium, reason: m.reason },
            admin: false,
            command: None,
            action: Action::Registry { key },
            sized: false,
            entry: None,
        }
    }

    pub fn delete_registry(key: &str) -> (&'static str, String) {
        let Some(rel) = key.strip_prefix(r"HKCU\Software\") else { return ("refused", "Only keys under HKCU\\Software are deleted.".into()) };
        let parts: Vec<&str> = rel.split('\\').collect();
        if parts.is_empty() || parts.len() > 2 || parts.iter().any(|p| p.is_empty()) || REG_NEVER.contains(&parts[0].to_lowercase().as_str()) {
            return ("refused", "Not a key GitSwitch deletes.".into());
        }
        let Some(dir) = crate::paths::data_dir().map(|d| d.join("com.gitswitch.app").join("registry-backups")) else { return ("failed", "No data folder for the backup.".into()) };
        let _ = std::fs::create_dir_all(&dir);
        let backup = dir.join(format!("{}-{}.reg", chrono::Local::now().format("%Y%m%d-%H%M%S"), parts.last().unwrap_or(&"key").replace(|c: char| !c.is_alphanumeric(), "_")));
        let reg = format!(r"{}\System32\reg.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
        let exported = crate::spawn::command(&reg).args(["export", key]).arg(&backup).args(["/y", "/reg:64"]).output();
        if !exported.as_ref().is_ok_and(|o| o.status.success()) {
            return ("failed", "The backup could not be written, so the key was kept.".into());
        }
        let sw = RegKey::predef(HKEY_CURRENT_USER);
        let parent = if parts.len() == 2 { format!(r"Software\{}", parts[0]) } else { "Software".into() };
        match sw.open_subkey_with_flags(&parent, KEY_ALL_ACCESS | KEY_WOW64_64KEY).and_then(|p| p.delete_subkey_all(parts.last().unwrap_or(&""))) {
            Ok(()) => ("removed", format!("Deleted; the backup is {}", backup.display())),
            Err(e) => ("failed", format!("{} (the backup is {})", e, backup.display())),
        }
    }

    /// The Recycle Bin can't take it: recycling is off on that drive, or it
    /// is bigger than the bin — Windows would delete it for good.
    pub fn recycle_refusal(path: &Path) -> Option<String> {
        use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW};
        let w = wide(path.as_os_str());
        let mut mount = vec![0u16; 512];
        let mut guid = vec![0u16; 64];
        // SAFETY: buffers are sized as passed.
        let (mount_s, guid_s) = unsafe {
            if GetVolumePathNameW(w.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) == 0 {
                return None;
            }
            let mlen = mount.iter().position(|c| *c == 0).unwrap_or(0);
            if GetVolumeNameForVolumeMountPointW(mount.as_ptr(), guid.as_mut_ptr(), guid.len() as u32) == 0 {
                return None;
            }
            let glen = guid.iter().position(|c| *c == 0).unwrap_or(0);
            (String::from_utf16_lossy(&mount[..mlen]), String::from_utf16_lossy(&guid[..glen]))
        };
        let id = guid_s.split('{').nth(1).and_then(|r| r.split('}').next()).map(|g| format!("{{{}}}", g))?;
        let bb = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{}", id), KEY_READ).ok();
        if bb.as_ref().and_then(|k| k.get_value::<u32, _>("NukeOnDelete").ok()) == Some(1) {
            return Some(format!("The Recycle Bin is turned off on {}, so it would be deleted for good. Delete it yourself if you mean to.", mount_s));
        }
        let mut total: u64 = 0;
        let mw = wide(OsStr::new(&mount_s));
        // SAFETY: valid wide string; out-pointer valid.
        unsafe {
            GetDiskFreeSpaceExW(mw.as_ptr(), std::ptr::null_mut(), &mut total, std::ptr::null_mut());
        }
        let cap = bb.and_then(|k| k.get_value::<u32, _>("MaxCapacity").ok()).map(|mb| mb as u64 * 1024 * 1024).unwrap_or(total / 20);
        let (size, _) = super::super::size_of(path, std::time::Duration::from_secs(3));
        (cap > 0 && size > cap).then(|| format!("It is bigger than the Recycle Bin on {} can hold, so Windows would delete it for good. Delete it yourself if you mean to.", mount_s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_codes_come_from_the_entry_never_from_an_install_command() {
        let g = "{23170F69-40C1-2702-2301-000001000000}";
        let e = RegEntry { key: g.into(), windows_installer: true, uninstall: Some(format!("MsiExec.exe /I{}", g)), ..Default::default() };
        assert_eq!(msi_guid(&e).as_deref(), Some(g));
        let e = RegEntry { key: "7-Zip".into(), uninstall: Some(format!("MsiExec.exe /I{} REBOOT=ReallySuppress", g.to_lowercase())), ..Default::default() };
        assert_eq!(msi_guid(&e).as_deref(), Some(g), "the /I is never used: /x is built from the code");
        assert_eq!(msi_guid(&RegEntry { key: "x".into(), uninstall: Some(r#""C:\x\unins000.exe""#.into()), ..Default::default() }), None);
        assert!(!is_guid("{23170F69-40C1-2702-2301-00000100000}"));
    }

    #[test]
    fn uninstall_strings_split_the_way_windows_reads_them() {
        let files = [r"C:\Program Files\Notepad++\uninstall.exe", r"C:\Program Files (x86)\Foo Bar\unins000.exe", r"C:\Tools\app\setup.exe"];
        let exists = |p: &str| files.iter().any(|f| f.eq_ignore_ascii_case(p));
        assert_eq!(split_command(r#""C:\Program Files\Notepad++\uninstall.exe" /S"#, &exists), Some((r"C:\Program Files\Notepad++\uninstall.exe".into(), "/S".into())));
        assert_eq!(split_command(r"C:\Program Files (x86)\Foo Bar\unins000.exe /SILENT", &exists), Some((r"C:\Program Files (x86)\Foo Bar\unins000.exe".into(), "/SILENT".into())), "unquoted, with spaces");
        assert_eq!(split_command(r"C:\Tools\app\setup -uninstall", &exists), Some((r"C:\Tools\app\setup.exe".into(), "-uninstall".into())), ".exe left off");
        assert_eq!(
            split_command(r#""C:\Program Files\X\uninst.exe" "/U:C:\Program Files\X\irunin.xml""#, &|_| false),
            Some((r"C:\Program Files\X\uninst.exe".into(), r#""/U:C:\Program Files\X\irunin.xml""#.into())),
            "InstallShield's quoted argument passes on exactly"
        );
        let (p, a) = split_command("RunDll32 advpack.dll,LaunchINFSection x.inf,DefaultUninstall", &|_| false).unwrap();
        assert!(p.to_lowercase().ends_with(r"\system32\rundll32.exe") || p == "RunDll32", "{p}");
        assert_eq!(a, "advpack.dll,LaunchINFSection x.inf,DefaultUninstall");
        assert_eq!(split_command("  ", &|_| true), None);
    }

    #[test]
    fn display_names_lose_versions_and_architectures() {
        assert_eq!(strip_version("7-Zip 23.01 (x64)"), "7-Zip");
        assert_eq!(strip_version("Notepad++ (64-bit x64)"), "Notepad++");
        assert_eq!(strip_version("Microsoft Visual C++ 2015-2022 Redistributable (x64) - 14.38.33135"), "Microsoft Visual C++ 2015-2022 Redistributable");
        assert_eq!(strip_version("Mozilla Firefox (x64 en-US)"), "Mozilla Firefox");
        assert_eq!(strip_version("VLC media player"), "VLC media player");
        assert_eq!(strip_version("Python 3.12.1 (64-bit)"), "Python");
        assert_eq!(exe_stem(r#""C:\Program Files\7-Zip\7zFM.exe",0"#).as_deref(), Some("7zFM"));
        assert_eq!(exe_stem(r"C:\x\y.ico"), None);
    }

    #[test]
    fn hidden_entries_and_gitswitchs_own_needs() {
        let base = RegEntry { view: "HKLM64".into(), key: "k".into(), name: Some("Thing".into()), uninstall: Some("x.exe".into()), ..Default::default() };
        assert!(entry_app(&base).is_some());
        for e in [
            RegEntry { system_component: true, ..base.clone() },
            RegEntry { parent: Some("Office".into()), ..base.clone() },
            RegEntry { release_type: Some("Update".into()), ..base.clone() },
            RegEntry { no_remove: true, ..base.clone() },
            RegEntry { uninstall: None, ..base.clone() },
        ] {
            assert!(entry_app(&e).is_none(), "{e:?}");
        }
        let wv = RegEntry { name: Some("Microsoft Edge WebView2 Runtime".into()), ..base.clone() };
        assert!(entry_app(&wv).unwrap().0.blocked.is_some());
        let git = RegEntry { name: Some("Git".into()), publisher: Some("The Git Development Community".into()), ..base.clone() };
        assert!(entry_app(&git).unwrap().0.blocked.is_some());
        let mine = RegEntry { name: Some("GitSwitch".into()), ..base.clone() };
        assert!(entry_app(&mine).unwrap().0.is_self);
        assert_eq!(entry_app(&RegEntry { install_date: Some("20240115".into()), ..base }).unwrap().0.date, Some(1705276800));
    }

    #[test]
    fn install_folders_are_offered_only_well_inside_a_drive() {
        assert!(safe_install_dir(Path::new(r"C:\Program Files\7-Zip")));
        assert!(safe_install_dir(Path::new(r"D:\Games\Some Game")));
        for bad in [r"C:\", r"C:\Program Files", r"C:\Tools", r"C:\Windows\System32\x", r"C:\Program Files\Common Files", r"C:\Program Files\Microsoft Office", r"C:\Program Files\WindowsApps"] {
            assert!(!safe_install_dir(Path::new(bad)), "{bad}");
        }
        assert!(never_dir("Microsoft") && never_dir("Windows Defender") && never_dir("regid.1991-06.com.microsoft") && !never_dir("7-Zip"));
    }

    #[test]
    fn store_listings_and_manifests_are_read() {
        let json = r#"{"main":[{"name":"SpotifyAB.SpotifyMusic","full":"SpotifyAB.SpotifyMusic_1.2.3.0_x64__zpdnekdrzrea0","family":"SpotifyAB.SpotifyMusic_zpdnekdrzrea0","publisher":"CN=5C0B8F5D, O=Spotify AB","version":"1.2.3.0","dir":""}],"all":[{"name":"Microsoft.VCLibs.140.00","family":"Microsoft.VCLibs.140.00_8wekyb3d8bbwe"}]}"#;
        let hex: String = json.as_bytes().iter().map(|b| format!("{:02X}", b)).collect();
        let l: AppxListing = serde_json::from_slice(&unhex(&hex).unwrap()).unwrap();
        assert_eq!(l.main[0].family, "SpotifyAB.SpotifyMusic_zpdnekdrzrea0");
        assert_eq!(l.all.len(), 1);
        let (app, _) = appx_app(&l.main[0], &SystemFacts::default());
        assert_eq!(app.publisher.as_deref(), Some("5C0B8F5D"));
        assert_eq!(app.system, None, "a Store app the person installed is theirs");
        assert_eq!(manifest_name("<Package><Properties><DisplayName>Spotify Music</DisplayName></Properties></Package>").as_deref(), Some("Spotify Music"));
        let xml = "<Package><Properties><DisplayName>ARMOURY CRATE</DisplayName><PublisherDisplayName>ASUSTeK COMPUTER INC.</PublisherDisplayName></Properties></Package>";
        assert_eq!(manifest_name(xml).as_deref(), Some("ARMOURY CRATE"), "PublisherDisplayName is not the DisplayName");
        assert_eq!(manifest_publisher(xml).as_deref(), Some("ASUSTeK COMPUTER INC."));
        assert_eq!(manifest_publisher("<Properties><PublisherDisplayName>ms-resource:Pub</PublisherDisplayName></Properties>"), None);
        assert_eq!(manifest_name("<Properties><DisplayName>ms-resource:AppName</DisplayName></Properties>"), None);
        assert_eq!(unhex("4G"), None);
        assert_eq!(msi_result(0), None);
        assert_eq!(msi_result(1602).unwrap().0, "failed");
        assert_eq!(msi_result(3010).unwrap().0, "restart");
    }

    fn asus() -> SystemFacts {
        let mut provisioned = std::collections::HashSet::new();
        provisioned.insert(provisioned_family("Clipchamp.Clipchamp_4.7.10220.0_neutral_~_yxz26nhyzhsrt").unwrap());
        provisioned.insert(provisioned_family("Microsoft.WindowsCalculator_11.2502.2.0_x64__8wekyb3d8bbwe").unwrap());
        SystemFacts { maker: maker_word("ASUSTeK COMPUTER INC."), provisioned }
    }

    #[test]
    fn system_apps_are_what_came_with_windows_or_the_pc_runs_its_hardware_or_others_use() {
        let f = asus();
        let r = |name: &str, publisher: Option<&str>| system_reason(name, publisher, &f);
        assert_eq!(r("Armoury Crate Service", Some("ASUSTek COMPUTER INC.")).as_deref(), Some("From your PC's maker"), "ASUSTeK and ASUSTek are one maker");
        assert_eq!(r("NVIDIA Graphics Driver 555.97", Some("NVIDIA Corporation")).as_deref(), Some("For your hardware"));
        assert_eq!(r("Intel(R) Wireless Bluetooth(R)", Some("Intel Corporation")).as_deref(), Some("For your hardware"));
        assert_eq!(r("AMD Software", Some("Advanced Micro Devices, Inc.")).as_deref(), Some("For your hardware"));
        assert_eq!(r("Microsoft Visual C++ v14 Redistributable (x64) - 14.50.35719", Some("Microsoft Corporation")).as_deref(), Some("Used by other apps"));
        assert_eq!(r("Microsoft .NET Runtime - 8.0.21 (x64)", Some("Microsoft Corporation")).as_deref(), Some("Used by other apps"));
        assert_eq!(r("Microsoft ASP.NET Core 8.0.14 - Shared Framework (x86)", Some("Microsoft Corporation")).as_deref(), Some("Used by other apps"));
        assert_eq!(r("Microsoft OneDrive", Some("Microsoft Corporation")).as_deref(), Some("Part of Windows"));
        assert_eq!(r("Xbox Service", Some("Microsoft Corporation")).as_deref(), Some("Part of Windows"));
        assert_eq!(r("Microsoft Windows Application Compatibility Fix Database", None).as_deref(), Some("Part of Windows"));
        for (name, publisher) in [
            ("Microsoft Visual Studio Code (User)", Some("Microsoft Corporation")),
            ("Microsoft Office Professional Plus 2024 - en-us", Some("Microsoft Corporation")),
            ("Microsoft Build of OpenJDK  17.0.17+10 (x64)", Some("Microsoft")),
            ("Windows Software Development Kit - Windows 10.0.26100.7705", Some("Microsoft Corporation")),
            ("Windows SDK AddOn", Some("Microsoft Corporation")),
            ("Microsoft .NET SDK 8.0.100 (x64)", Some("Microsoft Corporation")),
            ("µTorrent", Some("BitTorrent Limited")),
            ("Brave", Some("Brave Software Inc")),
            ("Google Chrome", Some("Google LLC")),
            ("Microsoft Knowledge Base Reader", Some("Microsoft Corporation")),
            ("Docker Desktop", Some("Docker Inc.")),
            ("Ninja", None),
        ] {
            assert_eq!(r(name, publisher), None, "{name}: the person's own");
        }
        // Store apps: provisioned ones came with Windows or with the PC.
        assert_eq!(appx_system_reason("Microsoft.WindowsCalculator", "Microsoft.WindowsCalculator_8wekyb3d8bbwe", Some("Microsoft Corporation"), &f).as_deref(), Some("Came with Windows"));
        assert_eq!(appx_system_reason("Clipchamp.Clipchamp", "Clipchamp.Clipchamp_yxz26nhyzhsrt", Some("Clipchamp"), &f).as_deref(), Some("Came with this PC"));
        assert_eq!(appx_system_reason("B9ECED6F.ArmouryCrate", "B9ECED6F.ArmouryCrate_qmba6cd70vzyy", Some("ASUSTeK COMPUTER INC."), &f).as_deref(), Some("From your PC's maker"));
        assert_eq!(appx_system_reason("MicrosoftCorporationII.WinAppRuntime.Singleton", "x_8wekyb3d8bbwe", Some("Microsoft Corporation"), &f).as_deref(), Some("Used by other apps"));
        assert_eq!(appx_system_reason("Microsoft.WidgetsPlatformRuntime", "x_8wekyb3d8bbwe", Some("Microsoft Corporation"), &f).as_deref(), Some("Part of Windows"));
        assert_eq!(appx_system_reason("Microsoft.Winget.Source", "x_8wekyb3d8bbwe", Some("Microsoft Corporation"), &f).as_deref(), Some("Part of Windows"));
        assert_eq!(appx_system_reason("com.tinyspeck.slackdesktop", "com.tinyspeck.slackdesktop_8yrtsj140pw4g", Some("Slack Technologies Inc."), &f), None);
        // ASUS's sparse package: its manifest says "Sparse Package", its signer says ASUS.
        let sparse = Appx { name: "ASUSAmbientHAL64".into(), full: "ASUSAmbientHAL64_1.0.0.3_neutral__ecvjzhf6c4w7r".into(), family: "ASUSAmbientHAL64_ecvjzhf6c4w7r".into(), publisher: Some("CN=ASUSTeK COMPUTER INC., O=ASUSTeK COMPUTER INC., L=Taipei, C=TW".into()), version: None, dir: None };
        let (app, _) = appx_app(&sparse, &f);
        assert_eq!(app.system.as_deref(), Some("From your PC's maker"));
        assert_eq!(app.publisher.as_deref(), Some("ASUSTeK COMPUTER INC."));
        assert_eq!(appx_system_reason("Microsoft.PowerToys", "Microsoft.PowerToys_8wekyb3d8bbwe", Some("Microsoft Corporation"), &f), None, "a Store app the person installed");
        // A self-built PC or a Surface has no "maker's apps".
        for m in ["System manufacturer", "To Be Filled By O.E.M.", "Default string", "Microsoft Corporation", ""] {
            assert_eq!(maker_word(m), None, "{m}");
        }
        assert_eq!(maker_word("Dell Inc.").as_deref(), Some("dell"));
        assert_eq!(first_word("Intel(R) Corporation").as_deref(), Some("intel"));
        assert_eq!(first_word("Micro-Star International Co., Ltd.").as_deref(), Some("micro-star"));
        assert_eq!(provisioned_family("Microsoft.X_1.0.0.0_x64__8wekyb3d8bbwe").as_deref(), Some("microsoft.x_8wekyb3d8bbwe"));
        assert_eq!(provisioned_family("nounderscores"), None);
    }

    // Backslash paths only split into folders on Windows, where this code runs.
    #[cfg(windows)]
    #[test]
    fn an_install_folder_comes_from_its_program_named_like_the_app_never_from_a_cache() {
        let names = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let zoho = names(&["Zoho Mail - Desktop 1.7.4", "Zoho Mail - Desktop"]);
        assert_eq!(install_dir_of(Path::new(r"C:\Program Files\Zoho Mail - Desktop\Zoho Mail - Desktop.exe"), &zoho), Some(PathBuf::from(r"C:\Program Files\Zoho Mail - Desktop")));
        let dd = names(&["Disk Drill 6.3.1398.0", "Disk Drill", "DiskDrill"]);
        assert_eq!(install_dir_of(Path::new(r"C:\Program Files\CleverFiles\Disk Drill\DiskDrill.exe"), &dd), Some(PathBuf::from(r"C:\Program Files\CleverFiles\Disk Drill")));
        assert_eq!(install_dir_of(Path::new(r"C:\ProgramData\Package Cache\{E6E5DD13-939D-4CB1-8E74-ACBF3501726B}\DiskDrillSetup.6.3.1398.0.exe"), &dd), None, "an installer's cache");
        assert_eq!(install_dir_of(Path::new(r"C:\Windows\Installer\{E6E5DD13-939D-4CB1-8E74-ACBF3501726B}\icon.exe"), &dd), None, "Windows' own folder");
        let idm = names(&["Internet Download Manager", "IDMan"]);
        assert_eq!(install_dir_of(Path::new(r"C:\Users\me\Downloads\Internet Download Manager\IDMan.exe"), &idm), None, "a download is not an install");
        assert_eq!(install_dir_of(Path::new(r"C:\Program Files\Foo Tool\bin\x64\foo.exe"), &names(&["Foo Tool"])), Some(PathBuf::from(r"C:\Program Files\Foo Tool")), "bin and x64 walked up");
        assert_eq!(install_dir_of(Path::new(r"C:\Users\me\AppData\Local\Discordia\app-1.0.9\Discordia.exe"), &names(&["Discordia"])), Some(PathBuf::from(r"C:\Users\me\AppData\Local\Discordia")), "a Squirrel version folder");
        assert_eq!(install_dir_of(Path::new(r"C:\Program Files\Common Files\Foo\foo.exe"), &names(&["Bar"])), None, "named like another app");
        assert_eq!(install_dir_of(Path::new(r"C:\Tools\foo.exe"), &names(&["Tools"])), None, "too near the drive");
        assert_eq!(install_dir_of(Path::new(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"), &names(&["Microsoft Edge", "msedge"])), None);
        // A developer's build or a portable copy named like the app is not where it is installed.
        assert_eq!(install_dir_of(Path::new(r"C:\Users\me\source\repos\AcmeStudio\x64\Release\AcmeStudio.exe"), &names(&["Acme Studio"])), None);
        assert_eq!(install_dir_of(Path::new(r"D:\Apps\AcmeStudio\AcmeStudio.exe"), &names(&["Acme Studio"])), None);
        assert_eq!(install_dir_of(Path::new(r"D:\Program Files\Acme Studio\acme.exe"), &names(&["Acme Studio"])), Some(PathBuf::from(r"D:\Program Files\Acme Studio")), "Program Files on another drive");
        assert_eq!(install_dir_of(Path::new(r"C:\Users\me\Documents\Acme Studio\acme.exe"), &names(&["Acme Studio"])), None, "a person's documents");
        assert!(in_install_root(Path::new(r"C:\ProgramData\CleverFiles\Disk Drill")) && in_install_root(Path::new(r"C:\Users\me\AppData\Roaming\uTorrent")) && !in_install_root(Path::new(r"C:\Users\me\Desktop\x")));
        assert_eq!(icon_file(r#""C:\Users\me\AppData\Roaming\uTorrent\uTorrent.exe",0"#), Some(PathBuf::from(r"C:\Users\me\AppData\Roaming\uTorrent\uTorrent.exe")));
        assert_eq!(icon_file(r"C:\Program Files\X\x.ico"), Some(PathBuf::from(r"C:\Program Files\X\x.ico")));
        assert_eq!(icon_file("x.exe,0"), None, "not a full path");
        let mut v = vec![PathBuf::from(r"C:\Program Files\Foo\sub")];
        push_dir(&mut v, PathBuf::from(r"C:\Program Files\Foo"));
        push_dir(&mut v, PathBuf::from(r"c:\program files\foo\other"));
        assert_eq!(v, vec![PathBuf::from(r"C:\Program Files\Foo")], "the outer folder only, once");
    }

    // Backslash paths only split into folders on Windows, where this code runs.
    #[cfg(windows)]
    #[test]
    fn an_older_session_still_finds_its_folder_from_what_its_entry_points_at() {
        // Zoho Mail's real entry: no InstallLocation, its icon in its folder.
        let rec = Record::WinEntry {
            view: "HKLM64".into(),
            key: "435bda16-99fd-51d0-938d-c156968a2aa4".into(),
            uninstall: r#""C:\Program Files\Zoho Mail - Desktop\Uninstall Zoho Mail - Desktop.exe" /allusers"#.into(),
            msi: None,
            install_dir: None,
            icon: Some(r"C:\Program Files\Zoho Mail - Desktop\Zoho Mail - Desktop.exe,0".into()),
            dirs: vec![],
            exes: vec![],
            startup: vec![],
            bundle: false,
        };
        assert_eq!(recorded_dirs("Zoho Mail - Desktop 1.7.4", &rec), vec![PathBuf::from(r"C:\Program Files\Zoho Mail - Desktop")]);
        // Disk Drill's: everything in an installer's cache — nothing derived.
        let dd = |msi: Option<&str>, bundle: bool| Record::WinEntry {
            view: "HKLM64".into(),
            key: "{E6E5DD13-939D-4CB1-8E74-ACBF3501726B}".into(),
            uninstall: r#""C:\ProgramData\Package Cache\{E6E5DD13-939D-4CB1-8E74-ACBF3501726B}\DiskDrillSetup.6.3.1398.0.exe"  /uninstall"#.into(),
            msi: msi.map(str::to_string),
            install_dir: None,
            icon: Some(r"C:\ProgramData\Package Cache\{E6E5DD13-939D-4CB1-8E74-ACBF3501726B}\DiskDrillSetup.6.3.1398.0.exe,0".into()),
            dirs: vec![],
            exes: vec![],
            startup: vec![],
            bundle,
        };
        assert!(recorded_dirs("Disk Drill 6.3.1398.0", &dd(None, true)).is_empty());
        let app = InstalledApp { name: "Disk Drill 6.3.1398.0".into(), ..Default::default() };
        let (plan, why) = force_plan(&app, &dd(None, true));
        assert!(plan.is_none() && why.unwrap().contains("setup bundle"), "a bundle is removed by its own uninstaller");
        assert!(force_plan(&app, &dd(Some("{23170F69-40C1-2702-2301-000001000000}"), false)).1.unwrap().contains("Windows Installer"));
        assert!(force_plan(&app, &dd(None, false)).1.unwrap().contains("isn't known"));
    }

    // Backslash paths only split into folders on Windows, where this code runs.
    #[cfg(windows)]
    #[test]
    fn the_administrators_cleanup_never_touches_windows_or_shared_folders() {
        // Shallow or shared folders are refused wherever this runs.
        for bad in [r"C:\", r"C:\Program Files", r"C:\Windows\System32\drivers", r"C:\Program Files\Common Files", r"C:\Program Files\Microsoft Office", r"C:\Program Files\WindowsApps"] {
            assert!(elevated_path_ok(Path::new(bad)).is_err(), "{bad}");
        }
        assert!(elevated_path_ok(Path::new(r"D:\Games\Some Game")).is_ok(), "a program's own folder on another drive");
        for bad in [r"C:\Users\me\Documents\Taxes", r"C:\Users\other\AppData", r"C:\Users\Public\Desktop\Notes.txt", r"C:\Users\me\Desktop\Projects"] {
            assert!(elevated_path_ok(Path::new(bad)).is_err(), "{bad}: a person's own files");
        }
        assert!(elevated_path_ok(Path::new(r"C:\Users\Public\Desktop\Disk Drill.lnk")).is_ok(), "the shared Desktop's shortcut");
        assert!(elevated_path_ok(Path::new(r"C:\Users\me\AppData\Roaming\uTorrent")).is_ok());
        assert_ne!(backup_name("HKLM64", "run-x"), backup_name("HKLM64", "run-x"), "never the same backup file twice");
        assert!(backup_name("HKLM32", "run-x").contains("-HKLM32-run_x-"));
    }

    #[cfg(windows)]
    #[test]
    fn files_in_use_are_named_and_a_program_running_from_its_folder_is_closed() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("gitswitch-inuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hook.dll");
        std::fs::write(&file, b"x").unwrap();
        // Held open, unshared — as a program holds a library it loaded.
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&file).unwrap();
        let users = users_of(&dir);
        assert!(!users.is_empty(), "this test process holds it");
        let (outcome, why) = crate::apps::failed_move_for_test(&dir, "Unknown { description: \"Some operations were aborted\" }");
        assert_eq!(outcome, "in-use", "{why}");
        assert!(why.starts_with("In use by ") && why.contains("delete it at the next restart"), "{why}");
        drop(held);
        // Scheduling a delete at the next restart is the administrator's; as
        // a person nothing is scheduled and nothing goes now.
        let (o, _) = delete_at_restart(&dir);
        assert!(o == "needs-admin" || o == "at-restart", "{o}");
        assert!(file.exists(), "nothing deleted now");
        // A program running from the app's folder is closed.
        let ping = dir.join("ping.exe");
        std::fs::copy(format!(r"{}\System32\PING.EXE", std::env::var("SystemRoot").unwrap()), &ping).unwrap();
        let mut child = std::process::Command::new(&ping).args(["-n", "60", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let closed = sys::close_in(std::slice::from_ref(&dir), std::time::Duration::from_millis(500));
        assert_eq!(closed.closed, vec!["ping.exe".to_string()], "{closed:?}");
        assert!(closed.left.is_empty(), "{closed:?}");
        assert!(child.try_wait().unwrap().is_some() || child.wait().is_ok());
        assert!(sys::close_in(std::slice::from_ref(&dir), std::time::Duration::from_millis(100)).closed.is_empty(), "nothing left to close");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_program_whose_files_are_already_gone_can_still_be_removed_from_the_list() {
        let gone = PathBuf::from(r"C:\Program Files\GitSwitch Test Program That Is Not There");
        let rec = Record::WinEntry { view: "HKCU".into(), key: "GitSwitchTestNothing".into(), uninstall: String::new(), msi: None, install_dir: Some(gone.clone()), icon: None, dirs: vec![gone], exes: vec![], startup: vec![], bundle: false };
        let app = InstalledApp { name: "GitSwitch Test Program".into(), ..Default::default() };
        let (plan, why) = force_plan(&app, &rec);
        let plan = plan.unwrap_or_else(|| panic!("{why:?}"));
        assert!(plan.folders.is_empty() && plan.entry.ends_with(r"\GitSwitchTestNothing"), "only its entry is left to remove");
        let unplugged = PathBuf::from(r"Q:\Games\Gone");
        if !Path::new(r"Q:\").exists() {
            let rec = Record::WinEntry { view: "HKCU".into(), key: "k".into(), uninstall: String::new(), msi: None, install_dir: Some(unplugged.clone()), icon: None, dirs: vec![unplugged], exes: vec![], startup: vec![], bundle: false };
            assert!(force_plan(&app, &rec).1.unwrap().contains("can't be reached"), "a drive that isn't there");
        }
    }

    #[cfg(windows)]
    #[test]
    fn what_needs_the_administrator_is_measured() {
        let f = std::env::temp_dir().join(format!("gitswitch-can-delete-{}", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        assert!(can_delete(&f), "your own file");
        let _ = std::fs::remove_file(&f);
        let pf = std::env::var("ProgramFiles").unwrap();
        // SAFETY: no arguments. (CI's Windows runner runs elevated, UAC off.)
        if unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() } == 0 {
            assert!(!can_delete(Path::new(&pf)), "Program Files needs the administrator");
        }
        assert!(elevated_path_ok(Path::new(&pf).join("CleverFiles").as_path()).is_ok());
        assert!(elevated_path_ok(Path::new(&std::env::var("ProgramData").unwrap()).join("CleverFiles").as_path()).is_ok());
        assert!(elevated_path_ok(Path::new(&std::env::var("SystemRoot").unwrap()).join("Temp").as_path()).is_err());
        assert!(elevated_path_ok(Path::new(&std::env::var("APPDATA").unwrap())).is_err(), "a folder Windows uses");
    }
}
