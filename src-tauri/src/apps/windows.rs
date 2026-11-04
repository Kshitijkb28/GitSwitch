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
use super::{Action, Candidate, InstalledApp, Place, Prepared, Record, Session};
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
        },
        Record::WinEntry { view: e.view.clone(), key: e.key.clone(), uninstall: e.uninstall.clone().unwrap_or_default(), msi, install_dir, icon: e.icon.clone() },
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

/// A Store app's name as its manifest gives it, unless that is a resource reference.
pub(crate) fn manifest_name(xml: &str) -> Option<String> {
    let props = xml.find("<Properties>")?;
    let rest = &xml[props..];
    let start = rest.find("<DisplayName>")? + "<DisplayName>".len();
    let end = rest[start..].find("</DisplayName>")?;
    let n = rest[start..start + end].trim();
    (!n.is_empty() && !n.starts_with("ms-resource:")).then(|| n.replace("&amp;", "&"))
}

fn appx_app(a: &Appx) -> (InstalledApp, Record) {
    let dir = a.dir.as_deref().filter(|d| !d.is_empty()).map(PathBuf::from);
    let shown = dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("AppxManifest.xml")).ok()).and_then(|x| manifest_name(&x)).unwrap_or_else(|| a.name.clone());
    (
        InstalledApp {
            id: format!("appx:{}", a.full),
            name: shown,
            version: a.version.clone(),
            publisher: a.publisher.as_deref().and_then(|p| p.split(',').next()).map(|p| p.trim_start_matches("CN=").to_string()),
            location: None,
            source: "store".into(),
            notes: vec!["A Store app: removed for you only.".into()],
            ..Default::default()
        },
        Record::WinAppx { full_name: a.full.clone(), family: a.family.clone(), name: a.name.clone(), install_dir: dir },
    )
}

pub fn list() -> Result<super::Listing, AppError> {
    #[cfg(windows)]
    {
        let mut rows: Vec<(InstalledApp, Record)> = sys::read_entries().iter().filter_map(entry_app).collect();
        let mut notes = Vec::new();
        match sys::appx() {
            Ok(l) => rows.extend(l.main.iter().map(appx_app)),
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
        Record::WinEntry { view, key, uninstall, msi, install_dir, icon } => {
            ev = Evidence { ids: vec![], names: names_of(&app.name, install_dir.as_deref(), icon.as_deref()), groups: vec![], vendor: app.publisher.clone() };
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
            if let Some(d) = install_dir {
                p.facts.push(format!("Installed in: {}", d.display()));
            }
            if let Some(pb) = &app.publisher {
                p.facts.push(format!("Publisher: {}", pb));
            }
            #[cfg(windows)]
            if let Some(signer) = sys::signer(Path::new(&program)) {
                p.facts.push(format!("The uninstaller is signed by: {}", signer));
            }
            p.running = install_dir.as_ref().filter(|d| safe_install_dir(d)).is_some_and(|d| running_in(d));
            if p.running {
                p.warnings.push(format!("{} is running — close it first, or its uninstaller may ask you to.", app.name));
            }
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
        places.push(Place::new(d, "%APPDATA%", Allow::IdsAndNames).vendor());
    }
    if let Some(d) = env_dir("LOCALAPPDATA") {
        places.push(Place::new(d.clone(), "%LOCALAPPDATA%", Allow::IdsAndNames).vendor());
        places.push(Place::new(d.join("Programs"), "%LOCALAPPDATA%\\Programs", Allow::IdsAndNames));
        if let Record::WinAppx { .. } = &s.record {
            places.push(Place::new(d.join("Packages"), "%LOCALAPPDATA%\\Packages", Allow::Ids));
        }
    }
    let (mut out, mut notes) = super::scan_places(&places, &s.evidence, others);
    out.retain(|c| c.path.file_name().is_none_or(|n| !never_dir(&n.to_string_lossy())));
    #[cfg(windows)]
    {
        let install_dir = match &s.record {
            Record::WinEntry { install_dir, .. } => install_dir.clone(),
            _ => None,
        };
        if let Some(dir) = install_dir.filter(|d| d.is_dir()) {
            match sys::install_dir_claimed(&dir, s) {
                Some(why) => notes.push(format!("{} is still there, but {} — so it is not offered.", dir.display(), why)),
                None if safe_install_dir(&dir) => {
                    let admin = !env_dir("LOCALAPPDATA").is_some_and(|l| lower(&dir).starts_with(&format!("{}\\", lower(&l))));
                    out.push(Candidate {
                        path: dir.clone(),
                        root: dir.parent().map(Path::to_path_buf).unwrap_or_default(),
                        place: "Install folder".into(),
                        kind: "folder",
                        m: Match { confidence: Confidence::High, reason: "the folder it was installed in".into() },
                        admin,
                        command: admin.then(|| format!("rd /s /q \"{}\"   (in a Command Prompt run as administrator)", dir.display())),
                        action: if admin { Action::None } else { Action::Trash },
                        sized: true,
                        entry: None,
                    });
                }
                None => notes.push(format!("{} is still there, but it is one of Windows' own folders — not offered.", dir.display())),
            }
        }
        sys::shortcuts(s, &mut out);
        if registry {
            sys::registry_leftovers(s, others, &mut out);
        }
        let pending = sys::pending_renames();
        out.retain(|c| !pending.contains(&lower(&c.path)));
    }
    #[cfg(not(windows))]
    let _ = registry;
    (out, notes)
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
        let c = expand(cmd.trim());
        let c = c.trim_start_matches(r"\??\");
        let c = if c.to_lowercase().starts_with(r"\systemroot\") { format!("{}{}", std::env::var("SystemRoot").unwrap_or_default(), &c[11..]) } else { c.to_string() };
        let (p, _) = split_command(&c, &|p| Path::new(p).is_file())?;
        let p = if p.to_lowercase().starts_with(r"system32\") { format!(r"{}\{}", std::env::var("SystemRoot").unwrap_or_default(), p) } else { p };
        Some(PathBuf::from(p))
    }

    /// Something still installed lives in that folder: another program's
    /// entry, a service or driver, a shared DLL.
    pub fn install_dir_claimed(dir: &Path, s: &Session) -> Option<String> {
        let d = format!("{}\\", lower(dir));
        let inside = |p: &Path| lower(p).starts_with(&d) || lower(p) == lower(dir);
        let removed = match &s.record {
            Record::WinEntry { view, key, .. } => format!("{}:{}", view, key),
            _ => String::new(),
        };
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

    /// Shortcuts in the Start menu and on the desktop that started the
    /// removed program — by their target, never by their name.
    pub fn shortcuts(s: &Session, out: &mut Vec<Candidate>) {
        let Record::WinEntry { install_dir, icon, .. } = &s.record else { return };
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
        let mut files: Vec<(PathBuf, usize)> = Vec::new();
        for (i, (dir, _, _)) in places.iter().enumerate() {
            let mut v = Vec::new();
            lnk_files(dir, if dir.ends_with("Desktop") { 0 } else { 2 }, &mut v);
            files.extend(v.into_iter().map(|f| (f, i)));
        }
        if files.is_empty() {
            return;
        }
        let input = std::env::temp_dir().join(format!("gitswitch-lnk-{}.json", std::process::id()));
        let list: Vec<String> = files.iter().map(|f| f.0.to_string_lossy().to_string()).collect();
        if std::fs::write(&input, serde_json::to_vec(&list).unwrap_or_default()).is_err() {
            return;
        }
        let answer = run_ps(SHORTCUTS, &[("GS_IN", &input.to_string_lossy())]);
        let _ = std::fs::remove_file(&input);
        let Some(targets) = answer.ok().and_then(|h| unhex(&h)).and_then(|b| serde_json::from_slice::<Vec<serde_json::Value>>(&b).ok()) else { return };
        let target_of: HashMap<String, String> = targets.iter().filter_map(|t| Some((t["p"].as_str()?.to_lowercase(), t["t"].as_str().unwrap_or("").to_string()))).collect();
        let stem = icon.as_deref().and_then(exe_stem).map(|s| s.to_lowercase());
        let dir_l = install_dir.as_ref().map(|d| format!("{}\\", lower(d)));
        let qualifies = |f: &Path| -> Option<Confidence> {
            let t = target_of.get(&f.to_string_lossy().to_lowercase())?;
            if t.is_empty() {
                return None;
            }
            let tl = t.to_lowercase();
            if dir_l.as_ref().is_some_and(|d| tl.starts_with(d)) {
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
        let Some(dir) = dirs::data_dir().map(|d| d.join("com.gitswitch.app").join("registry-backups")) else { return ("failed", "No data folder for the backup.".into()) };
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
        let (app, _) = appx_app(&l.main[0]);
        assert_eq!(app.publisher.as_deref(), Some("5C0B8F5D"));
        assert_eq!(manifest_name("<Package><Properties><DisplayName>Spotify Music</DisplayName></Properties></Package>").as_deref(), Some("Spotify Music"));
        assert_eq!(manifest_name("<Properties><DisplayName>ms-resource:AppName</DisplayName></Properties>"), None);
        assert_eq!(unhex("4G"), None);
        assert_eq!(msi_result(0), None);
        assert_eq!(msi_result(1602).unwrap().0, "failed");
        assert_eq!(msi_result(3010).unwrap().0, "restart");
    }
}
