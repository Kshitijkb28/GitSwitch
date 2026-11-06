//! Uninstalling GitSwitch from inside the app.
//!
//! First, while the app still runs, everything it wrote outside its own
//! folders is taken back: the push locks (behind the administrator prompt),
//! each repository's push block, identity guard and `gitswitch.*` keys, the
//! managed block in ~/.gitconfig, the managed signers in
//! ~/.ssh/allowed_signers, and — unless the person keeps their data — the
//! sign-ins in the system's credential store. Then a small script that waits
//! for the app to quit deletes the data folders and removes the app the way
//! it was installed: the bundle to the Trash on macOS, its own NSIS
//! uninstaller on Windows, dpkg or rpm (through pkexec) or the AppImage file
//! on Linux. SSH keys, repositories and the GitHub CLI's logins are never touched.

use crate::error::AppError;
use crate::git_exec::GitCmd;
use serde::Serialize;
use std::path::{Path, PathBuf};

const ID: &str = "com.gitswitch.app";

/// How the app itself goes away once it has quit.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AppRemoval {
    /// macOS: the .app bundle is moved to the Trash.
    Trash { bundle: String, trash: String },
    /// Windows: the installer's own uninstaller, silently (Windows asks for permission).
    Nsis { uninstaller: String },
    /// Linux packages, removed as root (pkexec asks for the password).
    Deb { package: String },
    Rpm { package: String },
    /// Linux: an AppImage is one file.
    AppImage { file: String },
    /// It can't remove itself; `reason` says what to do instead.
    Manual { reason: String },
}

#[derive(Debug, Serialize)]
pub struct UninstallPlan {
    pub platform: String,
    pub app: AppRemoval,
    /// One sentence: what happens to the app itself.
    pub how: String,
    /// ~/.gitconfig, when it holds GitSwitch's block.
    pub gitconfig: Option<String>,
    /// ~/.ssh/allowed_signers, when it holds GitSwitch's signers.
    pub signers: Option<String>,
    /// Repositories (on the Repository page) holding a push block, a guard or GitSwitch's keys.
    pub repos: Vec<String>,
    pub lock_helper: bool,
    pub locks: Vec<String>,
    /// Logins whose tokens are in the system's credential store.
    pub signins: Vec<String>,
    /// GitSwitch's own folders that exist: profiles, logs, update backups, the web view's data.
    pub data: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PrepareReport {
    /// "done", or "stopped" (the push locks could not be removed; nothing else was touched).
    pub outcome: String,
    pub message: String,
    /// When the locks need an administrator's terminal: the command.
    pub command: Option<String>,
    pub done: Vec<String>,
    pub problems: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct FinishReport {
    pub how: String,
    /// The app can't remove itself: what to do once it has quit.
    pub manual: Option<String>,
    /// Where the script writes what it did.
    pub log: String,
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// Every folder (and the two macOS files) GitSwitch or its web view keeps
/// data in. Each one's name is GitSwitch's identifier — nothing else is
/// ever deleted.
pub(crate) fn data_folders() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for base in [crate::paths::data_dir(), dirs::data_local_dir(), dirs::cache_dir(), dirs::config_dir()].into_iter().flatten() {
        out.push(base.join(ID));
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = dirs::home_dir() {
            let lib = home.join("Library");
            out.push(lib.join("WebKit").join(ID));
            out.push(lib.join("HTTPStorages").join(ID));
            out.push(lib.join("Preferences").join(format!("{}.plist", ID)));
            out.push(lib.join("Saved Application State").join(format!("{}.savedState", ID)));
        }
    }
    let mut seen = Vec::new();
    out.retain(|p| {
        let ours = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n == ID || n.starts_with(&format!("{}.", ID)));
        let fresh = !seen.contains(p);
        seen.push(p.clone());
        ours && fresh
    });
    out
}

/// The bundle a macOS executable lives in (…/GitSwitch.app/Contents/MacOS/gitswitch).
pub(crate) fn macos_removal(exe: &Path, home: &Path) -> AppRemoval {
    let bundle = exe.ancestors().find(|a| a.extension().is_some_and(|e| e == "app"));
    match bundle {
        Some(b) if b.to_string_lossy().contains("/AppTranslocation/") => AppRemoval::Manual {
            reason: "macOS is running GitSwitch from a temporary copy (it was opened straight from Downloads or the disk image), so GitSwitch can't tell where the real one is. After it quits, drag GitSwitch to the Trash from where you keep it.".into(),
        },
        Some(b) => AppRemoval::Trash { bundle: b.to_string_lossy().to_string(), trash: home.join(".Trash").to_string_lossy().to_string() },
        None => AppRemoval::Manual { reason: format!("This copy of GitSwitch is not an installed app ({}) — a development build. Delete it yourself.", exe.display()) },
    }
}

/// Windows: the NSIS installer leaves `uninstall.exe` beside the program.
pub(crate) fn windows_removal(exe: &Path) -> AppRemoval {
    let uninstaller = exe.parent().map(|d| d.join("uninstall.exe"));
    match uninstaller {
        Some(u) if u.is_file() => AppRemoval::Nsis { uninstaller: u.to_string_lossy().to_string() },
        _ => AppRemoval::Manual {
            reason: format!("GitSwitch's uninstaller is not beside the program ({}), so it was not installed by the GitSwitch installer. Delete that folder yourself, or remove it in Settings › Apps.", exe.display()),
        },
    }
}

/// The package `dpkg-query -S <file>` names: "git-switch: /usr/bin/gitswitch",
/// or "a:amd64, b: /path" when several share it — the first, without its architecture.
pub(crate) fn package_of_dpkg_query(out: &str, file: &str) -> Option<String> {
    let line = out.lines().find(|l| l.trim_end().ends_with(&format!(": {}", file)))?;
    let names = line.trim_end().strip_suffix(&format!(": {}", file))?;
    let first = names.split(", ").next()?.trim();
    let name = first.split(':').next()?.trim();
    (!name.is_empty() && !name.contains(char::is_whitespace)).then(|| name.to_string())
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}
#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

/// Linux: an AppImage (`APPIMAGE` names the file), else the deb or rpm package
/// that owns the program.
pub(crate) fn linux_removal(exe: &Path, appimage: Option<&str>) -> AppRemoval {
    if let Some(file) = appimage.filter(|f| !f.is_empty()) {
        return AppRemoval::AppImage { file: file.to_string() };
    }
    let file = exe.to_string_lossy().to_string();
    let needs_pkexec = |cmd: &str| -> Option<AppRemoval> {
        (!is_root() && !on_path("pkexec")).then(|| AppRemoval::Manual {
            reason: format!("Removing the package needs administrator rights, and pkexec is not installed. After GitSwitch quits, run: sudo {}", cmd),
        })
    };
    if on_path("dpkg-query") {
        let out = crate::spawn::command("dpkg-query").args(["-S", &file]).output();
        if let Some(package) = out.ok().filter(|o| o.status.success()).and_then(|o| package_of_dpkg_query(&String::from_utf8_lossy(&o.stdout), &file)) {
            return needs_pkexec(&format!("dpkg -r {}", package)).unwrap_or(AppRemoval::Deb { package });
        }
    }
    if on_path("rpm") {
        let out = crate::spawn::command("rpm").args(["-qf", "--qf", "%{NAME}", &file]).output();
        if let Some(package) = out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|n| !n.is_empty() && !n.contains(char::is_whitespace)) {
            return needs_pkexec(&format!("rpm -e {}", package)).unwrap_or(AppRemoval::Rpm { package });
        }
    }
    AppRemoval::Manual { reason: format!("No package owns {}, so it was not installed from the GitSwitch .deb, .rpm or AppImage. Delete it yourself after GitSwitch quits.", file) }
}

fn this_app() -> AppRemoval {
    let Ok(exe) = std::env::current_exe() else {
        return AppRemoval::Manual { reason: "GitSwitch can't tell where its program is. Delete it yourself after it quits.".into() };
    };
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let exe = PathBuf::from(crate::push_lock::strip_verbatim(&exe.to_string_lossy()));
    if cfg!(target_os = "macos") {
        macos_removal(&exe, &dirs::home_dir().unwrap_or_default())
    } else if cfg!(windows) {
        windows_removal(&exe)
    } else {
        linux_removal(&exe, std::env::var("APPIMAGE").ok().as_deref())
    }
}

pub(crate) fn describe(app: &AppRemoval) -> String {
    match app {
        AppRemoval::Trash { bundle, .. } => format!("{} is moved to the Trash once GitSwitch has quit.", bundle),
        AppRemoval::Nsis { .. } => "GitSwitch's Windows uninstaller runs once GitSwitch has quit; Windows asks for permission.".into(),
        AppRemoval::Deb { package } => format!("The package {} is removed once GitSwitch has quit; the system asks for your password.", package),
        AppRemoval::Rpm { package } => format!("The package {} is removed once GitSwitch has quit; the system asks for your password.", package),
        AppRemoval::AppImage { file } => format!("{} is deleted once GitSwitch has quit.", file),
        AppRemoval::Manual { reason } => reason.clone(),
    }
}

/// Whether GitSwitch left anything in this repository: a `gitswitch.*` key,
/// the push-block rewrite, or one of its hooks.
async fn holds_pieces(dir: &Path) -> bool {
    let keys = GitCmd::at(dir)
        .args(["config", "--local", "--get-regexp", r"^(gitswitch\.|url\.gitswitch-push-blocked)"])
        .ok_text()
        .await
        .is_some_and(|s| !s.trim().is_empty());
    if keys {
        return true;
    }
    let Ok(paths) = crate::git_status::git_paths(dir).await else { return false };
    let has = |hook: &str, marker: &str| std::fs::read_to_string(paths.hooks.join(hook)).is_ok_and(|c| c.contains(marker));
    has("pre-push", crate::push_guard::HOOK_MARKER) || has("pre-commit", crate::commit_audit::MARKER)
}

/// The repositories the Repository page lists, and their populated submodules.
async fn repos_and_submodules() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for r in crate::git_history::list_repos().await.unwrap_or_default() {
        let repo = PathBuf::from(&r.path);
        for rel in crate::push_guard::populated_submodules(&repo).await {
            out.push(repo.join(rel));
        }
        out.push(repo);
    }
    out
}

pub async fn plan() -> UninstallPlan {
    let app = this_app();
    let mut repos = Vec::new();
    for dir in repos_and_submodules().await {
        if Box::pin(holds_pieces(&dir)).await {
            repos.push(dir.to_string_lossy().to_string());
        }
    }
    let locks = Box::pin(crate::push_lock::helper_status()).await;
    UninstallPlan {
        platform: platform().into(),
        how: describe(&app),
        app,
        gitconfig: crate::git_config::managed_gitconfig().map(|p| p.to_string_lossy().to_string()),
        signers: crate::signing::has_managed_signers().then(|| crate::signing::allowed_signers_path().to_string_lossy().to_string()),
        repos,
        lock_helper: locks.installed,
        locks: locks.locks.iter().map(|l| l.repo.clone()).collect(),
        signins: crate::credentials::kept_signins().into_iter().map(|s| s.login).collect(),
        data: data_folders().into_iter().filter(|p| p.exists()).map(|p| p.to_string_lossy().to_string()).collect(),
    }
}

/// Take one repository's GitSwitch pieces away: the push block (its hook —
/// a chained one's original put back — its rewrite, flag and stored
/// pushurls), the identity guard (someone else's pre-commit stays), and
/// every other `gitswitch.*` key (workspace numbers).
async fn take_back(dir: &Path) -> Result<(), AppError> {
    let paths = crate::git_status::git_paths(dir).await?;
    crate::push_guard::remove_mirrors(dir, &paths.hooks).await?;
    let _ = crate::commit_audit::uninstall_guard(&dir.to_string_lossy());
    let _ = GitCmd::at(dir).args(["config", "--local", "--remove-section", "gitswitch"]).run().await;
    Ok(())
}

/// Everything but the app and its folders, which go once it has quit.
pub async fn prepare(keep_data: bool) -> PrepareReport {
    let mut done = Vec::new();
    let mut problems = Vec::new();

    // The locks first: they need the administrator, and if that is refused
    // nothing else has changed yet.
    let locks = Box::pin(crate::push_lock::helper_status()).await;
    if locks.installed || !locks.locks.is_empty() {
        let out = Box::pin(crate::push_lock::uninstall_all()).await;
        if !out.applied() {
            return PrepareReport {
                outcome: "stopped".into(),
                message: format!(
                    "The push locks were not removed ({}), so nothing was uninstalled. {}",
                    out.outcome, out.message
                ),
                command: out.command,
                done,
                problems,
            };
        }
        done.push(format!("Removed the push locks ({}) and the lock helper", locks.locks.len()));
    }

    let mut cleaned = 0;
    for dir in repos_and_submodules().await {
        if !Box::pin(holds_pieces(&dir)).await {
            continue;
        }
        match Box::pin(take_back(&dir)).await {
            Ok(()) if !Box::pin(holds_pieces(&dir)).await => cleaned += 1,
            Ok(()) => problems.push(format!("{}: something of GitSwitch's is still there (a hook in a folder core.hooksPath names?)", dir.display())),
            Err(e) => problems.push(format!("{}: {}", dir.display(), e)),
        }
    }
    if cleaned > 0 {
        done.push(format!("Took push blocks, guards and GitSwitch's settings out of {} repositor{}", cleaned, if cleaned == 1 { "y" } else { "ies" }));
    }

    match crate::git_config::remove_managed_block() {
        Ok(true) => done.push("Removed GitSwitch's block from ~/.gitconfig (a backup is beside it)".into()),
        Ok(false) => {}
        Err(e) => problems.push(format!("~/.gitconfig: {}", e)),
    }
    match crate::signing::remove_managed_signers() {
        Ok(true) => done.push("Removed GitSwitch's entries from ~/.ssh/allowed_signers".into()),
        Ok(false) => {}
        Err(e) => problems.push(format!("~/.ssh/allowed_signers: {}", e)),
    }
    if !keep_data {
        let signins = crate::credentials::kept_signins();
        let mut forgotten = 0;
        for s in &signins {
            match crate::credentials::forget_signin_token(&s.login) {
                Ok(()) => {
                    let _ = crate::credentials::forget_signin(&s.login);
                    forgotten += 1;
                }
                Err(e) => problems.push(format!("@{}'s sign-in: {}", s.login, e)),
            }
        }
        if forgotten > 0 {
            done.push(format!("Removed {} GitHub sign-in{} from the system's credential store", forgotten, if forgotten == 1 { "" } else { "s" }));
        }
    }
    PrepareReport {
        outcome: "done".into(),
        message: if problems.is_empty() { "GitSwitch's changes are taken back.".into() } else { "GitSwitch's changes are taken back, except what is listed.".into() },
        command: None,
        done,
        problems,
    }
}

fn sh_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "'\\''"))
}

/// The script macOS and Linux run once the app (`pid`) has quit.
pub(crate) fn unix_script(pid: u32, remove: &[PathBuf], app: &AppRemoval, log: &Path) -> String {
    let mut s = String::from("#!/bin/sh\n# GitSwitch's uninstall: runs once the app has quit.\n");
    s.push_str(&format!("exec >>{} 2>&1\necho \"GitSwitch uninstall, $(date)\"\n", sh_quote(&log.to_string_lossy())));
    s.push_str(&format!("n=0\nwhile kill -0 {} 2>/dev/null && [ $n -lt 300 ]; do sleep 0.2; n=$((n+1)); done\n", pid));
    for p in remove {
        s.push_str(&format!("rm -rf -- {}\n", sh_quote(&p.to_string_lossy())));
    }
    match app {
        AppRemoval::Trash { bundle, trash } => {
            let name = Path::new(bundle).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "GitSwitch".into());
            s.push_str(&format!("app={}\ntrash={}\nname={}\n", sh_quote(bundle), sh_quote(trash), sh_quote(&name)));
            s.push_str(concat!(
                "if [ -e \"$app\" ]; then\n",
                "  mkdir -p \"$trash\"\n",
                "  dest=\"$trash/$name.app\"\n",
                "  [ -e \"$dest\" ] && dest=\"$trash/$name $(date +%Y-%m-%d\\ %H.%M.%S).app\"\n",
                "  mv -- \"$app\" \"$dest\" || rm -rf -- \"$app\" || osascript -e 'on run argv' -e 'do shell script \"/bin/rm -rf \" & quoted form of (item 1 of argv) with administrator privileges' -e 'end run' \"$app\"\n",
                "fi\n",
            ));
        }
        AppRemoval::Deb { package } => s.push_str(&format!(
            "if [ \"$(id -u)\" = 0 ]; then dpkg -r {p}; else pkexec \"$(command -v dpkg)\" -r {p}; fi\n",
            p = sh_quote(package)
        )),
        AppRemoval::Rpm { package } => s.push_str(&format!(
            "if [ \"$(id -u)\" = 0 ]; then rpm -e {p}; else pkexec \"$(command -v rpm)\" -e {p}; fi\n",
            p = sh_quote(package)
        )),
        AppRemoval::AppImage { file } => s.push_str(&format!("rm -f -- {}\n", sh_quote(file))),
        AppRemoval::Nsis { .. } | AppRemoval::Manual { .. } => {}
    }
    s.push_str("rm -f -- \"$0\"\necho done\n");
    s
}

/// The PowerShell script Windows runs once the app has quit. It holds no
/// data: paths go in the job file beside it (UTF-8 JSON), so no quoting rule
/// of PowerShell's — which also reads curly quotes as quotes — can bite.
/// Folders are deleted with .NET's Directory.Delete, which removes a junction
/// or link inside instead of following it (Windows PowerShell 5.1's
/// `Remove-Item -Recurse` has followed them).
pub(crate) const WINDOWS_SCRIPT: &str = r#"# GitSwitch's uninstall: runs once the app has quit.
$ErrorActionPreference = 'SilentlyContinue'
$jobFile = [System.IO.Path]::ChangeExtension($PSCommandPath, '.json')
$job = [System.IO.File]::ReadAllText($jobFile, [System.Text.Encoding]::UTF8) | ConvertFrom-Json
$utf8 = New-Object System.Text.UTF8Encoding($false)
function Note([string]$line) { [System.IO.File]::AppendAllText($job.log, $line + "`r`n", $utf8) }
Note "GitSwitch uninstall, $(Get-Date)"
Wait-Process -Id $job.pid -Timeout 60
foreach ($p in @($job.remove)) {
  try {
    if ([System.IO.Directory]::Exists($p)) { [System.IO.Directory]::Delete($p, $true); Note "removed $p" }
    elseif ([System.IO.File]::Exists($p)) { [System.IO.File]::Delete($p); Note "removed $p" }
  } catch { Note "not removed: $p ($($_.Exception.Message))" }
}
if ($job.uninstaller) {
  try { Start-Process -FilePath $job.uninstaller -ArgumentList '/S' -ErrorAction Stop; Note 'uninstaller started' }
  catch { Note "uninstaller not started: $($_.Exception.Message)" }
}
Note 'done'
Remove-Item -LiteralPath $jobFile -Force
Remove-Item -LiteralPath $PSCommandPath -Force
"#;

/// What the Windows script reads: the app's process, the folders, the uninstaller.
pub(crate) fn windows_job(pid: u32, remove: &[PathBuf], app: &AppRemoval, log: &Path) -> serde_json::Value {
    serde_json::json!({
        "pid": pid,
        "remove": remove.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "uninstaller": match app { AppRemoval::Nsis { uninstaller } => Some(uninstaller.clone()), _ => None },
        "log": log.to_string_lossy(),
    })
}

/// Start the script that waits for `pid` to quit, detached so it outlives the app.
pub(crate) fn start_remover(pid: u32, remove: &[PathBuf], app: &AppRemoval) -> Result<PathBuf, AppError> {
    let tmp = std::env::temp_dir();
    let log = tmp.join("gitswitch-uninstall.log");
    if cfg!(windows) {
        let script = tmp.join(format!("gitswitch-uninstall-{}.ps1", pid));
        std::fs::write(script.with_extension("json"), serde_json::to_vec(&windows_job(pid, remove, app, &log)).unwrap_or_default())?;
        // With a byte-order mark: Windows PowerShell 5.1 reads a script
        // without one in the ANSI code page.
        std::fs::write(&script, format!("\u{feff}{}", WINDOWS_SCRIPT))?;
        spawn_detached("powershell.exe", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File", &script.to_string_lossy()])?;
    } else {
        let script = tmp.join(format!("gitswitch-uninstall-{}.sh", pid));
        std::fs::write(&script, unix_script(pid, remove, app, &log))?;
        spawn_detached("/bin/sh", &[&script.to_string_lossy()])?;
    }
    Ok(log)
}

#[cfg(unix)]
fn spawn_detached(program: &str, args: &[&str]) -> Result<(), AppError> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group: quitting the app (or its terminal) doesn't take it along.
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::Command(format!("Couldn't start the uninstall: {}", e)))
}

#[cfg(windows)]
fn spawn_detached(program: &str, args: &[&str]) -> Result<(), AppError> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let start = |flags: u32| std::process::Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(flags).spawn();
    // Out of any job the app was started in, so it isn't ended with the app;
    // a job that forbids that still lets it run inside.
    start(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| start(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW))
        .map(|_| ())
        .map_err(|e| AppError::Command(format!("Couldn't start the uninstall: {}", e)))
}

/// After `prepare`: start the script that deletes the data folders (unless
/// kept) and removes the app once it quits. The caller then quits the app.
pub fn finish(keep_data: bool) -> Result<FinishReport, AppError> {
    let app = this_app();
    if let AppRemoval::AppImage { .. } = app {
        forget_appimage_handler();
    }
    let remove = if keep_data { Vec::new() } else { data_folders() };
    let log = start_remover(std::process::id(), &remove, &app)?;
    Ok(FinishReport {
        how: describe(&app),
        manual: match &app {
            AppRemoval::Manual { reason } => Some(reason.clone()),
            _ => None,
        },
        log: log.to_string_lossy().to_string(),
    })
}

/// An AppImage registers gitswitch:// links itself (lib.rs): its handler
/// file and the default-application line naming it go with it.
fn forget_appimage_handler() {
    let Some(name) = std::env::current_exe().ok().and_then(|e| e.file_name().map(|n| format!("{}-handler.desktop", n.to_string_lossy()))) else { return };
    if let Some(dir) = dirs::data_dir() {
        let _ = std::fs::remove_file(dir.join("applications").join(&name));
    }
    if let Some(list) = dirs::config_dir().map(|d| d.join("mimeapps.list")) {
        if let Ok(text) = std::fs::read_to_string(&list) {
            let kept: Vec<&str> = text.lines().filter(|l| !l.contains(&name)).collect();
            let _ = std::fs::write(&list, format!("{}\n", kept.join("\n")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_folders_named_after_gitswitch_are_ever_deleted() {
        let folders = data_folders();
        assert!(!folders.is_empty());
        for f in &folders {
            let name = f.file_name().unwrap().to_string_lossy().to_string();
            assert!(name == ID || name.starts_with("com.gitswitch.app."), "{}", f.display());
        }
        let mut sorted = folders.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), folders.len(), "no folder twice");
    }

    #[test]
    fn the_app_is_found_where_each_platform_installs_it() {
        let home = Path::new("/Users/me");
        assert_eq!(
            macos_removal(Path::new("/Applications/GitSwitch.app/Contents/MacOS/gitswitch"), home),
            AppRemoval::Trash { bundle: "/Applications/GitSwitch.app".into(), trash: home.join(".Trash").to_string_lossy().to_string() }
        );
        assert!(matches!(macos_removal(Path::new("/private/var/folders/x/T/AppTranslocation/ABC/d/GitSwitch.app/Contents/MacOS/gitswitch"), home), AppRemoval::Manual { .. }));
        assert!(matches!(macos_removal(Path::new("/Users/me/src/GitSwitch/src-tauri/target/debug/gitswitch"), home), AppRemoval::Manual { .. }));

        let dir = std::env::temp_dir().join(format!("gitswitch-uninstall-win-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(windows_removal(&dir.join("gitswitch.exe")), AppRemoval::Manual { .. }), "no uninstaller beside it: not removed blindly");
        std::fs::write(dir.join("uninstall.exe"), b"").unwrap();
        assert_eq!(windows_removal(&dir.join("gitswitch.exe")), AppRemoval::Nsis { uninstaller: dir.join("uninstall.exe").to_string_lossy().to_string() });
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(linux_removal(Path::new("/tmp/.mount_x/usr/bin/gitswitch"), Some("/home/me/Apps/GitSwitch.AppImage")), AppRemoval::AppImage { file: "/home/me/Apps/GitSwitch.AppImage".into() });
    }

    #[test]
    fn the_package_is_read_from_dpkg_query() {
        assert_eq!(package_of_dpkg_query("git-switch: /usr/bin/gitswitch\n", "/usr/bin/gitswitch").as_deref(), Some("git-switch"));
        assert_eq!(package_of_dpkg_query("git-switch:amd64, other: /usr/bin/gitswitch\n", "/usr/bin/gitswitch").as_deref(), Some("git-switch"));
        assert_eq!(package_of_dpkg_query("diversion by x from: /usr/bin/gitswitch\nfoo: /usr/bin/other\n", "/usr/bin/gitswitch").as_deref(), None);
        assert_eq!(package_of_dpkg_query("", "/usr/bin/gitswitch"), None);
    }

    #[test]
    fn the_scripts_wait_for_the_app_then_remove_only_what_they_name() {
        let log = Path::new("/tmp/gitswitch-uninstall.log");
        let data = vec![PathBuf::from("/Users/me/Library/Application Support/com.gitswitch.app"), PathBuf::from("/Users/o'neil/x/com.gitswitch.app")];
        let mac = unix_script(42, &data, &AppRemoval::Trash { bundle: "/Applications/GitSwitch.app".into(), trash: "/Users/me/.Trash".into() }, log);
        assert!(mac.contains("while kill -0 42 "), "{mac}");
        assert!(mac.contains("rm -rf -- '/Users/me/Library/Application Support/com.gitswitch.app'"), "{mac}");
        assert!(mac.contains(r"rm -rf -- '/Users/o'\''neil/x/com.gitswitch.app'"), "a quote in a path stays inside the word: {mac}");
        assert!(mac.contains("mv -- \"$app\" \"$dest\""), "{mac}");
        assert_eq!(mac.matches("rm -rf -- ").count(), 3, "the two folders, and the bundle only if moving it failed: {mac}");

        let deb = unix_script(7, &[], &AppRemoval::Deb { package: "git-switch".into() }, log);
        assert!(deb.contains("dpkg -r 'git-switch'") && deb.contains("pkexec"), "{deb}");
        assert!(!deb.contains("rm -rf"), "kept data: nothing deleted: {deb}");
        let img = unix_script(7, &[], &AppRemoval::AppImage { file: "/home/me/GitSwitch.AppImage".into() }, log);
        assert!(img.contains("rm -f -- '/home/me/GitSwitch.AppImage'"), "{img}");

        let job = windows_job(9, &[PathBuf::from(r"C:\Users\José ‘o’neil\AppData\Roaming\com.gitswitch.app")], &AppRemoval::Nsis { uninstaller: r"C:\Program Files\GitSwitch\uninstall.exe".into() }, Path::new(r"C:\Temp\gitswitch-uninstall.log"));
        assert_eq!(job["pid"], 9);
        assert_eq!(job["remove"][0], r"C:\Users\José ‘o’neil\AppData\Roaming\com.gitswitch.app", "paths travel as data, unchanged");
        assert_eq!(job["uninstaller"], r"C:\Program Files\GitSwitch\uninstall.exe");
        assert!(windows_job(9, &[], &AppRemoval::Manual { reason: "x".into() }, Path::new("l")).get("uninstaller").is_some_and(|u| u.is_null()));
        assert!(!WINDOWS_SCRIPT.contains("Remove-Item -LiteralPath $p") && WINDOWS_SCRIPT.contains("[System.IO.Directory]::Delete($p, $true)"), "never Remove-Item -Recurse on a folder");
        assert!(WINDOWS_SCRIPT.is_ascii(), "the script itself holds no data");
    }
}
