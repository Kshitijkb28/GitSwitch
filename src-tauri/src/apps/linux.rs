//! Ubuntu (and other dpkg systems): the apps in the menu — each owned by the
//! package, snap, Flatpak or AppImage its menu entry comes from — and, on a
//! second tab, every package. A package goes with dpkg after apt has said
//! exactly what goes with it; leftovers are only ever "maybe" in the home
//! folder, and sure only in a sandboxed app's own folder.
#![cfg_attr(target_os = "macos", allow(dead_code))]
#![cfg_attr(windows, allow(dead_code))]

use super::matcher::{Allow, Confidence, Evidence, Match, Others};
use super::{home, sys, unsys, Action, Candidate, InstalledApp, Place, Prepared, Record, Session, TrashOutcome};
use crate::error::AppError;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Packages whose removal breaks the system, or GitSwitch itself.
const NEVER: &[&str] = &[
    "ubuntu-minimal", "ubuntu-standard", "gnome-shell", "gdm3", "network-manager", "sudo", "pkexec", "policykit-1",
    "polkitd", "snapd", "apt", "dpkg", "systemd", "systemd-sysv", "libc6", "bash", "coreutils", "login", "passwd",
    "init", "linux-generic", "grub-efi-amd64-signed", "grub-pc", "shim-signed", "xorg", "xwayland",
    // GitSwitch's own
    "git", "libwebkit2gtk-4.1-0", "libgtk-3-0", "git-switch", "gitswitch",
];

/// One `[Desktop Entry]`.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Desktop {
    pub name: String,
    pub exec: Option<String>,
    pub try_exec: Option<String>,
    pub hidden: bool,
    pub application: bool,
    pub snap: Option<String>,
    pub flatpak: Option<String>,
}

pub(crate) fn parse_desktop(text: &str) -> Option<Desktop> {
    let mut d = Desktop::default();
    let mut in_entry = false;
    let mut seen = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
            seen |= in_entry;
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = v.trim().to_string();
        match k.trim() {
            "Name" => d.name = v,
            "Exec" => d.exec = Some(v),
            "TryExec" => d.try_exec = Some(v),
            "NoDisplay" | "Hidden" if v.eq_ignore_ascii_case("true") => d.hidden = true,
            "Type" => d.application = v == "Application",
            "X-SnapInstanceName" => d.snap = Some(v),
            "X-Flatpak" => d.flatpak = Some(v),
            _ => {}
        }
    }
    (seen && !d.name.is_empty()).then_some(d)
}

/// The program an `Exec=` line starts: quotes honoured, `env VAR=… ` and
/// field codes skipped.
pub(crate) fn exec_program(exec: &str) -> Option<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut escaped, mut any) = (false, false, false);
    for c in exec.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
            any = true;
        } else if c.is_whitespace() && !quoted {
            if !cur.is_empty() || any {
                words.push(std::mem::take(&mut cur));
                any = false;
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() || any {
        words.push(cur);
    }
    let mut it = words.into_iter().filter(|w| !(w.starts_with('%') && w.len() == 2));
    let mut first = it.next()?;
    if first == "env" || first.ends_with("/env") {
        first = it.find(|w| !w.contains('=') && !w.starts_with('-'))?;
    }
    Some(first)
}

/// An AppImage: "AI" and type 2 at byte 8 of an ELF file.
pub(crate) fn is_appimage(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = [0u8; 11];
    f.read_exact(&mut head).is_ok() && &head[..4] == b"\x7fELF" && &head[8..11] == b"AI\x02"
}

fn desktop_dirs() -> Vec<(PathBuf, &'static str)> {
    let h = home();
    vec![
        (sys("/usr/share/applications"), "system"),
        (sys("/usr/local/share/applications"), "local"),
        (h.join(".local/share/applications"), "user"),
        (sys("/var/lib/snapd/desktop/applications"), "snap"),
        (sys("/var/lib/flatpak/exports/share/applications"), "flatpak-system"),
        (h.join(".local/share/flatpak/exports/share/applications"), "flatpak-user"),
    ]
}

/// Which package installed each menu entry, from dpkg's own file lists.
fn dpkg_desktop_owners() -> HashMap<PathBuf, String> {
    let mut out = HashMap::new();
    let Ok(rd) = std::fs::read_dir(sys("/var/lib/dpkg/info")) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(pkg) = name.strip_suffix(".list") else { continue };
        let pkg = pkg.split(':').next().unwrap_or(pkg).to_string();
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        for line in text.lines().filter(|l| l.ends_with(".desktop") && l.contains("/applications/")) {
            out.insert(PathBuf::from(line), pkg.clone());
        }
    }
    out
}

/// One row of `dpkg-query -W -f '${binary:Package}\t${Version}\t${Installed-Size}\t${db:Status-Abbrev}\t${Essential}\t${Priority}\t${binary:Summary}\n'`.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct DpkgRow {
    pub package: String,
    pub version: String,
    pub size_kib: Option<u64>,
    pub status: String,
    pub essential: bool,
    pub priority: String,
    pub summary: String,
}

pub(crate) const DPKG_FORMAT: &str = "${binary:Package}\\t${Version}\\t${Installed-Size}\\t${db:Status-Abbrev}\\t${Essential}\\t${Priority}\\t${binary:Summary}\\n";

pub(crate) fn parse_dpkg(text: &str) -> Vec<DpkgRow> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 4 && !f[0].is_empty()).then(|| DpkgRow {
                package: f[0].split(':').next().unwrap_or(f[0]).to_string(),
                version: f[1].to_string(),
                size_kib: f[2].trim().parse().ok(),
                status: f[3].trim().to_string(),
                essential: f.get(4).is_some_and(|e| e.trim() == "yes"),
                priority: f.get(5).map(|p| p.trim().to_string()).unwrap_or_default(),
                summary: f.get(6).map(|s| s.trim().to_string()).unwrap_or_default(),
            })
        })
        .collect()
}

impl DpkgRow {
    pub fn installed(&self) -> bool {
        self.status.starts_with("ii") || self.status.starts_with("hi")
    }
}

fn dpkg_rows(packages: &[String]) -> Vec<DpkgRow> {
    let mut cmd = crate::spawn::command("dpkg-query");
    cmd.env("LC_ALL", "C").args(["-W", "-f", &DPKG_FORMAT.replace("\\t", "\t").replace("\\n", "\n")]);
    cmd.args(packages);
    cmd.output().map(|o| parse_dpkg(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

/// `snap list --unicode=never --color=never`: name, version, notes (classic, base, snapd…).
pub(crate) fn parse_snap_list(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .skip_while(|l| !l.starts_with("Name"))
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 2).then(|| (f[0].to_string(), f[1].to_string(), f.get(5).copied().unwrap_or("-").to_string()))
        })
        .collect()
}

/// `flatpak list --app --columns=application,name,version,branch,installation`.
pub(crate) fn parse_flatpak_list(text: &str) -> Vec<[String; 5]> {
    text.lines()
        .filter(|l| !l.starts_with("Application ID") && !l.trim().is_empty())
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 5).then(|| [f[0], f[1], f[2], f[3], f[4]].map(|x| x.trim().to_string()))
        })
        .collect()
}

fn tool(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

fn blocked_package(r: &DpkgRow) -> Option<String> {
    if r.essential {
        return Some("Essential to the system.".into());
    }
    if r.priority == "required" || r.priority == "important" {
        return Some(format!("The system needs it (priority {}).", r.priority));
    }
    never(&r.package)
}

fn never(pkg: &str) -> Option<String> {
    if ["git", "libwebkit2gtk-4.1-0", "libgtk-3-0", "git-switch", "gitswitch"].contains(&pkg) {
        return Some("GitSwitch itself needs it.".into());
    }
    if NEVER.contains(&pkg) || pkg.starts_with("ubuntu-desktop") || running_kernel().is_some_and(|k| pkg == format!("linux-image-{}", k) || pkg == format!("linux-modules-{}", k)) {
        return Some("The system needs it.".into());
    }
    None
}

fn running_kernel() -> Option<&'static str> {
    static K: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    K.get_or_init(|| std::fs::read_to_string("/proc/sys/kernel/osrelease").ok().map(|s| s.trim().to_string())).as_deref()
}

pub fn list(kind: &str) -> Result<super::Listing, AppError> {
    if kind == "packages" {
        return Ok((list_packages(), Vec::new()));
    }
    let mut notes = Vec::new();
    let owners = dpkg_desktop_owners();
    // owner key → (name, record, menu entries)
    let mut groups: BTreeMap<String, (String, Record, Vec<PathBuf>, String)> = BTreeMap::new();
    let mut unowned: Vec<(PathBuf, Desktop, &'static str)> = Vec::new();
    for (dir, kind) in desktop_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Some(d) = parse_desktop(&text) else { continue };
            if d.hidden || !d.application {
                continue;
            }
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            // The owner comes from the menu entry itself, never from the
            // program it starts (a browser's web app starts the browser).
            let owner = if kind == "snap" {
                let name = d.snap.clone().unwrap_or_else(|| stem.split('_').next().unwrap_or(&stem).to_string());
                Some((format!("snap:{}", name), Record::Snap { name, classic: false }, "snap"))
            } else if kind.starts_with("flatpak") {
                let id = d.flatpak.clone().unwrap_or_else(|| stem.clone());
                let inst = if kind == "flatpak-user" { "user" } else { "system" };
                Some((format!("flatpak:{}:{}", inst, id), Record::Flatpak { id, installation: inst.into(), branch: None }, "flatpak"))
            } else if let Some(pkg) = owners.get(&unsys(&path)) {
                Some((format!("deb:{}", pkg), Record::Deb { package: pkg.clone(), desktop: vec![] }, "deb"))
            } else {
                let prog = d.try_exec.clone().or_else(|| d.exec.as_deref().and_then(exec_program));
                prog.map(PathBuf::from)
                    .filter(|p| p.is_absolute() && is_appimage(p))
                    .map(|file| (format!("appimage:{}", file.display()), Record::AppImage { file, desktop: vec![] }, "appimage"))
            };
            match owner {
                Some((key, rec, source)) => {
                    let g = groups.entry(key).or_insert_with(|| (d.name.clone(), rec, Vec::new(), source.to_string()));
                    // The entry named after the package names the app.
                    let primary = match &g.1 {
                        Record::Deb { package, .. } => stem == *package,
                        _ => false,
                    };
                    if primary {
                        g.0 = d.name.clone();
                    }
                    g.2.push(path.clone());
                }
                None => unowned.push((path.clone(), d, kind)),
            }
        }
    }
    let debs: Vec<String> = groups.values().filter_map(|g| if let Record::Deb { package, .. } = &g.1 { Some(package.clone()) } else { None }).collect();
    let dpkg: HashMap<String, DpkgRow> = if debs.is_empty() { HashMap::new() } else { dpkg_rows(&debs).into_iter().map(|r| (r.package.clone(), r)).collect() };
    let snaps: HashMap<String, (String, String)> = if tool("snap") {
        crate::spawn::command("snap").args(["list", "--unicode=never", "--color=never"]).output().map(|o| parse_snap_list(&String::from_utf8_lossy(&o.stdout)).into_iter().map(|(n, v, notes)| (n, (v, notes))).collect()).unwrap_or_default()
    } else {
        HashMap::new()
    };
    let flatpaks: Vec<[String; 5]> = if tool("flatpak") {
        crate::spawn::command("flatpak").args(["list", "--app", "--columns=application,name,version,branch,installation"]).output().map(|o| parse_flatpak_list(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut rows = Vec::new();
    for (key, (name, mut rec, desktop, source)) in groups {
        let mut app = InstalledApp { id: key.clone(), name, source: source.clone(), ..Default::default() };
        match &mut rec {
            Record::Deb { package, desktop: d } => {
                *d = desktop.clone();
                let r = dpkg.get(package.as_str());
                app.version = r.map(|r| r.version.clone());
                app.size = r.and_then(|r| r.size_kib).map(|k| k * 1024);
                app.blocked = r.and_then(blocked_package).or_else(|| never(package));
                app.location = Some(format!("package {}", package));
                if r.is_some_and(|r| !r.installed()) {
                    continue;
                }
            }
            Record::Snap { name, classic } => {
                let s = snaps.get(name.as_str());
                app.version = s.map(|s| s.0.clone());
                *classic = s.is_some_and(|s| s.1.contains("classic"));
                app.location = Some(format!("snap {}", name));
                if name == "snapd" || s.is_some_and(|s| s.1.contains("base") || s.1.contains("snapd")) {
                    app.blocked = Some("The system needs it.".into());
                }
            }
            Record::Flatpak { id, installation, branch } => {
                if let Some(f) = flatpaks.iter().find(|f| f[0] == *id && f[4] == *installation) {
                    app.version = Some(f[2].clone()).filter(|v| !v.is_empty());
                    *branch = Some(f[3].clone()).filter(|b| !b.is_empty());
                }
                app.location = Some(format!("Flatpak {} ({})", id, installation));
            }
            Record::AppImage { file, desktop: d } => {
                *d = desktop.clone();
                app.location = Some(file.to_string_lossy().to_string());
                app.size = std::fs::metadata(&*file).ok().map(|m| m.len());
            }
            _ => {}
        }
        rows.push((app, rec));
    }
    // Flatpaks with no menu entry are still apps.
    for f in &flatpaks {
        let key = format!("flatpak:{}:{}", f[4], f[0]);
        if rows.iter().any(|(a, _)| a.id == key) {
            continue;
        }
        rows.push((
            InstalledApp { id: key, name: f[1].clone(), version: Some(f[2].clone()).filter(|v| !v.is_empty()), location: Some(format!("Flatpak {} ({})", f[0], f[4])), source: "flatpak".into(), ..Default::default() },
            Record::Flatpak { id: f[0].clone(), installation: f[4].clone(), branch: Some(f[3].clone()).filter(|b| !b.is_empty()) },
        ));
    }
    let made_elsewhere = unowned.iter().filter(|(_, _, k)| *k == "user").count();
    if made_elsewhere > 0 {
        notes.push(format!("{} menu entr{} in ~/.local/share/applications belong to no package — made by another app (a browser's web apps, a game launcher); remove them there.", made_elsewhere, if made_elsewhere == 1 { "y" } else { "ies" }));
    }
    Ok((rows, notes))
}

fn list_packages() -> Vec<(InstalledApp, Record)> {
    let mut rows = Vec::new();
    for r in dpkg_rows(&[]) {
        let left = r.status.starts_with("rc");
        if !r.installed() && !left {
            continue;
        }
        rows.push((
            InstalledApp {
                id: format!("pkg:{}", r.package),
                name: r.package.clone(),
                version: Some(r.version.clone()),
                publisher: None,
                location: Some(r.summary.clone()).filter(|s| !s.is_empty()),
                source: "package".into(),
                size: r.size_kib.map(|k| k * 1024),
                date: None,
                notes: if left { vec!["Removed — only its settings are left.".into()] } else { vec![] },
                blocked: if left { Some("Already removed; its settings are left (purge it with apt).".into()) } else { blocked_package(&r) },
                is_self: false,
                system: None,
            },
            Record::Deb { package: r.package.clone(), desktop: vec![] },
        ));
    }
    rows
}

/// `apt-get -s remove`: every package it would remove.
pub(crate) fn parse_simulation(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("Remv ").or_else(|| l.strip_prefix("Purg ")))
        .filter_map(|r| r.split_whitespace().next())
        .map(|p| p.split(':').next().unwrap_or(p).to_string())
        .collect()
}

fn package_files(pkg: &str) -> Vec<PathBuf> {
    crate::spawn::command("dpkg")
        .args(["-L", pkg])
        .env("LC_ALL", "C")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(PathBuf::from).collect())
        .unwrap_or_default()
}

fn commands_of(files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .filter(|f| ["/usr/bin", "/bin", "/usr/games", "/usr/sbin", "/usr/local/bin"].iter().any(|d| f.parent() == Some(Path::new(d))))
        .filter_map(|f| f.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect()
}

fn as_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid cannot fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    false
}

/// Processes whose program is one of `paths`, or under `prefix`.
fn running(paths: &[PathBuf], prefix: Option<&Path>, appimage: Option<&Path>) -> bool {
    let Ok(rd) = std::fs::read_dir("/proc") else { return false };
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(exe) = std::fs::read_link(e.path().join("exe")) {
            // /bin is /usr/bin on merged-/usr systems.
            let same = |p: &Path| p.to_string_lossy().trim_end_matches(" (deleted)").replacen("/usr/bin/", "/bin/", 1);
            let exe_s = same(&exe);
            if paths.iter().any(|p| same(p) == exe_s) || prefix.is_some_and(|pre| Path::new(&exe_s).starts_with(pre)) {
                return true;
            }
        }
        if let Some(img) = appimage {
            if let Ok(env) = std::fs::read(e.path().join("environ")) {
                let want = format!("APPIMAGE={}", img.display());
                if env.split(|b| *b == 0).any(|kv| kv == want.as_bytes()) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn prepare(app: &InstalledApp, rec: &Record) -> Result<(Prepared, Evidence, Vec<String>), AppError> {
    let mut p = Prepared { blocked: app.blocked.clone(), ..Default::default() };
    let ev;
    let mut packages = Vec::new();
    match rec {
        Record::Deb { package, desktop } => {
            if !tool("apt-get") || !tool("dpkg") {
                p.blocked = Some("apt and dpkg are not available here.".into());
            }
            if !as_root() && !tool("pkexec") {
                p.blocked = Some(format!("Removing a package needs the administrator, and pkexec is not installed. Run: sudo apt remove {}", package));
            }
            let sim = crate::spawn::command("apt-get").env("LC_ALL", "C").args(["-s", "remove", package]).output();
            let mut gone = match sim {
                Ok(o) if o.status.success() => parse_simulation(&String::from_utf8_lossy(&o.stdout)),
                Ok(o) => {
                    p.blocked = Some(format!("apt can't remove it: {}", String::from_utf8_lossy(&o.stderr).trim()));
                    vec![]
                }
                Err(e) => {
                    p.blocked.get_or_insert(format!("apt could not be asked: {}", e));
                    vec![]
                }
            };
            gone.sort();
            gone.dedup();
            if !gone.contains(package) && p.blocked.is_none() {
                p.blocked = Some("apt would not remove it (is it still installed?).".into());
            }
            let rows: HashMap<String, DpkgRow> = dpkg_rows(&gone).into_iter().map(|r| (r.package.clone(), r)).collect();
            for g in &gone {
                if let Some(why) = rows.get(g).and_then(blocked_package).or_else(|| never(g)) {
                    p.blocked = Some(format!("Removing it would also remove {}: {}", g, why));
                }
            }
            p.also_removed = gone.iter().filter(|g| *g != package).cloned().collect();
            packages = gone.clone();
            let files = package_files(package);
            let cmds = commands_of(&files);
            let stems: Vec<String> = desktop.iter().filter_map(|d| d.file_stem().map(|s| s.to_string_lossy().to_string())).collect();
            let mut names = vec![package.clone(), app.name.clone()];
            names.extend(stems.iter().cloned());
            names.extend(cmds.iter().cloned());
            names.sort();
            names.dedup();
            ev = Evidence { ids: vec![], names, groups: vec![], vendor: None };
            p.facts = vec![format!("Package: {} {}", package, app.version.clone().unwrap_or_default())];
            if !cmds.is_empty() {
                p.facts.push(format!("Commands: {}", cmds.join(", ")));
            }
            if !stems.is_empty() {
                p.facts.push(format!("Menu entries: {}", stems.join(", ")));
            }
            p.method = if p.also_removed.is_empty() {
                format!("The package {} is removed; the system asks for your password.", package)
            } else {
                format!("The package {} and the {} other package(s) listed are removed; the system asks for your password.", package, p.also_removed.len())
            };
            p.command = Some(format!("pkexec env DEBIAN_FRONTEND=noninteractive dpkg -r {}", gone.join(" ")));
            p.running = running(&files, None, None);
        }
        Record::Snap { name, classic } => {
            ev = Evidence { ids: vec![], names: vec![name.clone(), app.name.clone()], groups: vec![], vendor: None };
            p.facts = vec![format!("Snap: {} ({})", name, if *classic { "classic: it can write anywhere in your home folder" } else { "strict: its data is in ~/snap" })];
            p.method = format!("The snap {} is removed; the system asks for your password.", name);
            p.command = Some(format!("snap remove {}", name));
            p.running = running(&[], Some(&Path::new("/snap").join(name)), None);
        }
        Record::Flatpak { id, installation, .. } => {
            ev = Evidence { ids: vec![id.clone()], names: vec![], groups: vec![], vendor: None };
            p.facts = vec![format!("Flatpak: {} (installed for {})", id, if installation == "user" { "you" } else { "everyone" })];
            p.method = format!("The Flatpak {} is removed{}.", id, if installation == "user" { "" } else { "; the system asks for your password" });
            p.command = Some(format!("flatpak uninstall -y --{} {}", if installation == "user" { "user" } else { "system" }, id));
            p.running = crate::spawn::command("flatpak").args(["ps", "--columns=application"]).output().map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == id)).unwrap_or(false);
        }
        Record::AppImage { file, desktop } => {
            let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let mut names = vec![app.name.clone(), stem];
            names.extend(desktop.iter().filter_map(|d| d.file_stem().map(|s| s.to_string_lossy().to_string())));
            ev = Evidence { ids: vec![], names, groups: vec![], vendor: None };
            p.facts = vec![format!("AppImage: {}", file.display())];
            p.method = format!("{} and its menu entry go to the Trash.", file.display());
            p.running = running(std::slice::from_ref(file), None, Some(file));
        }
        _ => return Err(AppError::Config("Not a Linux app.".into())),
    }
    Ok((p, ev, packages))
}

async fn run(program: &str, args: &[&str], env: &[(&str, &str)]) -> Result<std::process::Output, AppError> {
    let mut cmd = crate::spawn::async_command(program);
    cmd.args(args).stdin(std::process::Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    // No timeout and no kill: an interrupted dpkg leaves the system half-done.
    cmd.output().await.map_err(|e| AppError::Command(format!("{} could not start: {}", program, e)))
}

fn tail(o: &std::process::Output) -> String {
    let mut t = String::from_utf8_lossy(&o.stderr).trim().to_string();
    if t.is_empty() {
        t = String::from_utf8_lossy(&o.stdout).trim().to_string();
    }
    let lines: Vec<&str> = t.lines().collect();
    lines[lines.len().saturating_sub(6)..].join("\n")
}

pub async fn uninstall(s: &mut Session) -> Result<(), AppError> {
    match s.record.clone() {
        Record::Deb { .. } => {
            if s.packages.is_empty() {
                return Err(AppError::Config("Nothing approved to remove.".into()));
            }
            let mut args: Vec<&str> = Vec::new();
            let env = [("DEBIAN_FRONTEND", "noninteractive"), ("NEEDRESTART_SUSPEND", "1"), ("LC_ALL", "C")];
            let out = if as_root() {
                args.push("-r");
                args.extend(s.packages.iter().map(String::as_str));
                run("dpkg", &args, &env).await?
            } else {
                args.extend(["/usr/bin/env", "DEBIAN_FRONTEND=noninteractive", "NEEDRESTART_SUSPEND=1", "LC_ALL=C", "/usr/bin/dpkg", "-r"]);
                args.extend(s.packages.iter().map(String::as_str));
                run("pkexec", &args, &[]).await?
            };
            match out.status.code() {
                Some(0) => {}
                Some(126) | Some(127) if !as_root() => return Err(AppError::Config("The password prompt was closed, so nothing was removed.".into())),
                _ => {
                    let t = tail(&out);
                    if t.contains("lock") {
                        return Err(AppError::Config("Another package operation is running (Software Updater?). Try again when it has finished.".into()));
                    }
                    return Err(AppError::Command(format!("dpkg could not remove it:\n{}", t)));
                }
            }
        }
        Record::Snap { name, .. } => {
            let out = run("snap", &["remove", &name], &[]).await?;
            if !out.status.success() {
                let t = tail(&out);
                return Err(AppError::Command(if t.contains("access denied") { format!("The system did not allow it. In a terminal: sudo snap remove {}", name) } else { t }));
            }
        }
        Record::Flatpak { id, installation, .. } => {
            let scope = if installation == "user" { "--user" } else { "--system" };
            let out = run("flatpak", &["uninstall", "-y", "--noninteractive", scope, &id], &[]).await?;
            if !out.status.success() {
                return Err(AppError::Command(tail(&out)));
            }
        }
        Record::AppImage { file, desktop } => {
            match super::to_trash(&file) {
                TrashOutcome::Trashed | TrashOutcome::Gone => {}
                TrashOutcome::NeedsAdmin(m) | TrashOutcome::Protected(m) | TrashOutcome::Failed(m) => return Err(AppError::Command(format!("Couldn't move {} to the Trash: {}", file.display(), m))),
            }
            // Its menu entries in your own folder go with it.
            let mine = home().join(".local/share/applications");
            for d in desktop.iter().filter(|d| d.starts_with(&mine)) {
                let _ = super::to_trash(d);
            }
        }
        _ => return Err(AppError::Config("Not a Linux app.".into())),
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

pub async fn status(s: &mut Session) -> Result<(), AppError> {
    match gone(s) {
        Ok(()) => s.state = "gone".into(),
        Err(why) if s.state != "uninstalling" => {
            s.state = "still-installed".into();
            s.message = Some(why);
        }
        Err(_) => {}
    }
    Ok(())
}

pub fn gone(s: &Session) -> Result<(), String> {
    match &s.record {
        Record::Deb { package, .. } => match dpkg_rows(std::slice::from_ref(package)).first() {
            Some(r) if r.installed() => Err(format!("The package {} is still installed.", package)),
            _ => Ok(()),
        },
        Record::Snap { name, .. } => {
            let still = crate::spawn::command("snap").args(["list", name]).output().is_ok_and(|o| o.status.success());
            if still {
                Err(format!("The snap {} is still installed.", name))
            } else {
                Ok(())
            }
        }
        Record::Flatpak { id, .. } => {
            let still = ["--system", "--user"].iter().any(|scope| crate::spawn::command("flatpak").args(["info", scope, id]).output().is_ok_and(|o| o.status.success()));
            if still {
                Err(format!("{} is still installed (for you or for everyone).", id))
            } else {
                Ok(())
            }
        }
        Record::AppImage { file, .. } => {
            if file.exists() {
                Err(format!("{} is still there.", file.display()))
            } else {
                Ok(())
            }
        }
        _ => Err("Not a Linux app.".into()),
    }
}

/// Everything still installed: packages, snaps, Flatpaks, commands, menu entries.
pub fn others(_s: &Session) -> Others {
    let mut o = Others::new(false);
    for r in dpkg_rows(&[]).into_iter().filter(|r| r.installed()) {
        o.add_package(&r.package);
    }
    if let Some(path) = std::env::var_os("PATH") {
        let mut seen = HashSet::new();
        for d in std::env::split_paths(&path) {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if seen.insert(n.clone()) {
                    o.add_command(&n);
                }
            }
        }
    }
    if tool("snap") {
        if let Ok(out) = crate::spawn::command("snap").args(["list", "--unicode=never", "--color=never"]).output() {
            for (n, _, _) in parse_snap_list(&String::from_utf8_lossy(&out.stdout)) {
                o.add_app(std::slice::from_ref(&n), std::slice::from_ref(&n), None, &[]);
            }
        }
    }
    if tool("flatpak") {
        if let Ok(out) = crate::spawn::command("flatpak").args(["list", "--columns=application,name"]).output() {
            for l in String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.starts_with("Application ID")) {
                let f: Vec<String> = l.split('\t').map(|x| x.trim().to_string()).collect();
                o.add_app(&f[..1], &f[1..], None, &[]);
            }
        }
    }
    for (dir, _) in desktop_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if let Some(d) = std::fs::read_to_string(e.path()).ok().and_then(|t| parse_desktop(&t)) {
                let stem = e.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                o.add_app(&[], &[d.name, stem], None, &[]);
            }
        }
    }
    o
}

pub fn leftovers(s: &Session, others: &Others) -> (Vec<Candidate>, Vec<String>) {
    let h = home();
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let sure = |path: PathBuf, root: PathBuf, place: &str, reason: String, out: &mut Vec<Candidate>| {
        if path.symlink_metadata().is_ok_and(|m| !m.file_type().is_symlink()) {
            out.push(Candidate { path, root, place: place.into(), kind: "folder", m: Match { confidence: Confidence::High, reason }, admin: false, command: None, action: Action::Trash, sized: true, entry: None });
        }
    };
    let home_places = || {
        vec![
            Place::new(h.join(".config"), "~/.config", Allow::IdsAndNames),
            Place::new(h.join(".local/share"), "~/.local/share", Allow::IdsAndNames),
            Place::new(h.join(".local/state"), "~/.local/state", Allow::IdsAndNames),
            Place::new(h.join(".cache"), "~/.cache", Allow::IdsAndNames),
            Place::new(h.clone(), "~ (hidden folders)", Allow::IdsAndNames).dots(),
        ]
    };
    match &s.record {
        Record::Flatpak { id, .. } => {
            // A Flatpak writes only in its own folder.
            sure(h.join(".var/app").join(id), h.join(".var/app"), "~/.var/app", format!("the Flatpak's own folder ({})", id), &mut out);
        }
        Record::Snap { name, classic } => {
            sure(h.join("snap").join(name), h.join("snap"), "~/snap", format!("the snap's own folder ({})", name), &mut out);
            if *classic {
                let (c, n) = super::scan_places(&home_places(), &s.evidence, others);
                out.extend(c);
                notes.extend(n);
            }
        }
        Record::Deb { package, desktop } => {
            let (c, n) = super::scan_places(&home_places(), &s.evidence, others);
            out.extend(c);
            notes.extend(n);
            menu_entries(desktop, &s.evidence, &mut out);
            if dpkg_rows(std::slice::from_ref(package)).first().is_some_and(|r| r.status.starts_with("rc")) {
                out.push(Candidate {
                    path: PathBuf::from(format!("{} — settings kept by the package (in /etc)", package)),
                    root: PathBuf::new(),
                    place: "Package settings".into(),
                    kind: "package-settings",
                    m: Match { confidence: Confidence::Medium, reason: "removed with dpkg --purge, which also runs the package's own clean-up (it may delete data it kept under /var/lib)".into() },
                    admin: false,
                    command: None,
                    action: Action::Purge { package: package.clone() },
                    sized: false,
                    entry: None,
                });
            }
        }
        Record::AppImage { desktop, .. } => {
            let (c, n) = super::scan_places(&home_places(), &s.evidence, others);
            out.extend(c);
            notes.extend(n);
            menu_entries(desktop, &s.evidence, &mut out);
        }
        _ => {}
    }
    (out, notes)
}

/// `~/.config/autostart/<entry>.desktop` and your own copies of its menu entries.
fn menu_entries(desktop: &[PathBuf], _ev: &Evidence, out: &mut Vec<Candidate>) {
    let h = home();
    let stems: Vec<String> = desktop.iter().filter_map(|d| d.file_name().map(|s| s.to_string_lossy().to_string())).collect();
    for (dir, place) in [(h.join(".config/autostart"), "~/.config/autostart"), (h.join(".local/share/applications"), "~/.local/share/applications")] {
        for s in &stems {
            let p = dir.join(s);
            if p.symlink_metadata().is_ok_and(|m| m.is_file()) {
                out.push(Candidate {
                    path: p,
                    root: dir.clone(),
                    place: place.into(),
                    kind: "file",
                    m: Match { confidence: Confidence::Medium, reason: format!("your copy of its menu entry {}", s) },
                    admin: false,
                    command: None,
                    action: Action::Trash,
                    sized: false,
                    entry: None,
                });
            }
        }
    }
}

/// Remove what a removed package left in /etc, with its own clean-up.
pub fn purge(package: &str) -> (&'static str, String) {
    let out = if as_root() {
        crate::spawn::command("dpkg").env("DEBIAN_FRONTEND", "noninteractive").args(["-P", package]).stdin(std::process::Stdio::null()).output()
    } else {
        crate::spawn::command("pkexec").args(["/usr/bin/env", "DEBIAN_FRONTEND=noninteractive", "/usr/bin/dpkg", "-P", package]).stdin(std::process::Stdio::null()).output()
    };
    match out {
        Ok(o) if o.status.success() => ("removed", format!("dpkg --purge {} done.", package)),
        Ok(o) => ("failed", tail(&o)),
        Err(e) => ("failed", e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_entries_are_read_from_their_own_group() {
        let t = "[Desktop Entry]\nName=XTerm\nName[de]=XTerm DE\nExec=xterm\nType=Application\n[Desktop Action new]\nName=New\nExec=xterm -e x\n";
        let d = parse_desktop(t).unwrap();
        assert_eq!((d.name.as_str(), d.exec.as_deref(), d.application, d.hidden), ("XTerm", Some("xterm"), true, false));
        assert!(parse_desktop("[Desktop Entry]\nName=X\nNoDisplay=true\nType=Application\n").unwrap().hidden);
        let snap = parse_desktop("[Desktop Entry]\nName=Firefox\nX-SnapInstanceName=firefox\nType=Application\nExec=env BAMF=1 /snap/bin/firefox %u\n").unwrap();
        assert_eq!(snap.snap.as_deref(), Some("firefox"));
    }

    #[test]
    fn the_program_an_exec_line_starts() {
        assert_eq!(exec_program("xterm").as_deref(), Some("xterm"));
        assert_eq!(exec_program("\"/home/me/Apps/My App.AppImage\" %U").as_deref(), Some("/home/me/Apps/My App.AppImage"));
        assert_eq!(exec_program("env WINEPREFIX=/home/me/.wine wine C:\\\\x.exe").as_deref(), Some("wine"));
        assert_eq!(exec_program("/opt/google/chrome/google-chrome --profile-directory=Default --app-id=abc").as_deref(), Some("/opt/google/chrome/google-chrome"));
        assert_eq!(exec_program(""), None);
    }

    #[test]
    fn dpkg_snap_flatpak_and_apt_output_are_read() {
        let d = "xterm:amd64\t379-1ubuntu1\t1400\tii \t\toptional\tX terminal emulator\nold\t1.0\t10\trc \t\toptional\tgone\nbash\t5.1\t1800\tii \tyes\trequired\tshell\n";
        let rows = parse_dpkg(d);
        assert_eq!(rows[0].package, "xterm", "the architecture is not part of the name");
        assert!(rows[0].installed() && !rows[1].installed() && rows[2].essential);
        assert!(blocked_package(&rows[2]).is_some());
        let snaps = "Name    Version   Rev    Tracking         Publisher   Notes\ncore22  20240111  1122   latest/stable    canonical**  base\nfirefox 125.0     4173   latest/stable/…  mozilla**    -\ncode    1.88      190    latest/stable    vscode**     classic\n";
        let p = parse_snap_list(snaps);
        assert_eq!(p.iter().map(|x| (x.0.as_str(), x.2.as_str())).collect::<Vec<_>>(), vec![("core22", "base"), ("firefox", "-"), ("code", "classic")]);
        let fl = "Application ID\tName\tVersion\tBranch\tInstallation\norg.gimp.GIMP\tGNU Image Manipulation Program\t2.10.38\tstable\tsystem\n";
        assert_eq!(parse_flatpak_list(fl)[0][0], "org.gimp.GIMP");
        let sim = "NOTE: This is only a simulation!\nRemv xterm [379-1ubuntu1]\nRemv xterm-extra:amd64 [1]\nInst foo\n";
        assert_eq!(parse_simulation(sim), vec!["xterm", "xterm-extra"]);
    }

    #[test]
    fn system_pieces_and_gitswitchs_own_are_never_removed() {
        for p in ["snapd", "dpkg", "apt", "sudo", "gnome-shell", "ubuntu-desktop-minimal", "git", "libwebkit2gtk-4.1-0"] {
            assert!(never(p).is_some(), "{p}");
        }
        assert!(never("xterm").is_none());
    }

    #[test]
    fn an_appimage_is_known_by_its_magic() {
        let dir = std::env::temp_dir().join(format!("gitswitch-appimage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut head = b"\x7fELF\x02\x01\x01\x00AI\x02".to_vec();
        head.extend_from_slice(&[0u8; 64]);
        std::fs::write(dir.join("a.AppImage"), &head).unwrap();
        std::fs::write(dir.join("b.AppImage"), b"#!/bin/sh\necho not one\n").unwrap();
        assert!(is_appimage(&dir.join("a.AppImage")));
        assert!(!is_appimage(&dir.join("b.AppImage")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
