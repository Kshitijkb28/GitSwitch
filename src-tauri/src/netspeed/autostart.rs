//! Starting GitSwitch when the person signs in, to show the net speed: only
//! when they ticked it in the Net speed tab, and only while the speed is shown
//! somewhere. It starts without a window ([`FLAG`]).
//!
//! The entry names this program and `--startup`:
//! - Windows: the value "GitSwitch" under HKCU\…\CurrentVersion\Run. When the
//!   person turned it off in Windows' Startup apps (StartupApproved), that is
//!   said and never overridden.
//! - macOS: ~/Library/LaunchAgents/com.gitswitch.app.startup.plist.
//! - Linux: ~/.config/autostart/gitswitch.desktop (under $XDG_CONFIG_HOME when set).
//!
//! What the person chose is kept in the net speed settings. The entry is made
//! to match whenever that changes, and at every start: the program may have
//! moved, or an uninstall step may have taken the entry away.

use serde::Serialize;
use std::path::PathBuf;

/// What the entry starts GitSwitch with: no window, the speed shown.
pub const FLAG: &str = "--startup";
/// The Run value's name (Windows).
#[cfg_attr(not(windows), allow(dead_code))]
const NAME: &str = "GitSwitch";
/// The LaunchAgent's label and file name (macOS).
const LABEL: &str = "com.gitswitch.app.startup";

/// Starting at sign-in, as the Net speed tab shows it.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct StartupView {
    /// The person's choice.
    pub on: bool,
    /// The system's entry is there and starts this program.
    pub active: bool,
    /// The system has it turned off (Windows' Startup apps): it won't start.
    pub blocked: Option<String>,
    /// Writing or removing the entry failed.
    pub error: Option<String>,
}

/// Started by the sign-in entry.
pub fn launched_at_login<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().any(|a| a.as_ref() == FLAG)
}

/// Started at sign-in to show the speed: the window stays hidden. Any other
/// start shows it, and so does a sign-in start with nothing to show (the
/// setting was lost), so GitSwitch never runs unseen.
pub fn start_hidden(at_login: bool, speed_shown: bool) -> bool {
    at_login && speed_shown
}

/// Windows: the Run value.
pub(crate) fn run_command(program: &str) -> String {
    format!("\"{}\" {}", program, FLAG)
}

/// Windows' Startup apps keeps each entry's on/off as 12 bytes. The first one
/// is even when the entry may start (02, 06) and odd when the person turned it
/// off (03, 07). No value at all: never touched, so it may start.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn approved(bytes: &[u8]) -> bool {
    bytes.first().is_none_or(|b| b & 1 == 0)
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// macOS: the LaunchAgent. Aqua: only a sign-in at the screen, never an ssh login.
pub(crate) fn launch_agent(program: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>AssociatedBundleIdentifiers</key>
	<array>
		<string>com.gitswitch.app</string>
	</array>
	<key>ProgramArguments</key>
	<array>
		<string>{program}</string>
		<string>{flag}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#,
        label = LABEL,
        program = xml(program),
        flag = FLAG
    )
}

/// One argument of a desktop entry's Exec, quoted as the Desktop Entry spec
/// asks. It goes in double quotes when it holds a space or a reserved
/// character, with ", `, $ and \ escaped. Then, the value being a string,
/// each \ is doubled. A % is %% (a field code otherwise).
pub(crate) fn exec_arg(arg: &str) -> String {
    let arg = arg.replace('%', "%%");
    let reserved = |c: char| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c);
    let quoted = if arg.chars().any(reserved) {
        let mut q = String::from("\"");
        for c in arg.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
    } else {
        arg
    };
    quoted.replace('\\', "\\\\")
}

/// Linux: the autostart entry. TryExec: once the program is gone (the package
/// removed), the desktop skips the entry instead of failing at every sign-in.
pub(crate) fn desktop_entry(program: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=GitSwitch\nComment=Shows the net speed in the top bar\nExec={} {}\nTryExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exec_arg(program),
        FLAG,
        program.replace('\\', "\\\\")
    )
}

/// The entry's text for this program, as this system keeps it.
fn entry_for(program: &str) -> String {
    if cfg!(windows) {
        run_command(program)
    } else if cfg!(target_os = "macos") {
        launch_agent(program)
    } else {
        desktop_entry(program)
    }
}

/// Where the entry lives.
#[derive(Debug, Clone)]
pub(crate) enum Place {
    /// Windows: the Run key and Startup apps' StartupApproved key, under HKCU.
    #[cfg_attr(not(windows), allow(dead_code))]
    Registry { run: String, approved: String },
    /// macOS and Linux: a file.
    File(PathBuf),
}

/// The entry's file name (macOS, Linux).
fn file_name() -> String {
    if cfg!(target_os = "macos") {
        format!("{}.plist", LABEL)
    } else {
        "gitswitch.desktop".into()
    }
}

/// Where this system keeps start-up entries.
fn default_place() -> Option<Place> {
    if cfg!(windows) {
        Some(Place::Registry {
            run: r"Software\Microsoft\Windows\CurrentVersion\Run".into(),
            approved: r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run".into(),
        })
    } else if cfg!(target_os = "macos") {
        crate::paths::home_dir().map(|h| Place::File(h.join("Library").join("LaunchAgents").join(file_name())))
    } else {
        dirs::config_dir().map(|c| Place::File(c.join("autostart").join(file_name())))
    }
}

fn place() -> Option<Place> {
    // A debug build (the verify suites) can be pointed at a test key or
    // folder, and a test binary must be: a test never touches the real
    // entry. A release build ignores both.
    #[cfg(debug_assertions)]
    {
        if cfg!(windows) {
            if let Some(base) = std::env::var("GITSWITCH_AUTOSTART_KEY").ok().filter(|k| !k.trim().is_empty()) {
                return Some(Place::Registry { run: format!(r"{}\Run", base), approved: format!(r"{}\StartupApproved\Run", base) });
            }
        } else if let Some(dir) = std::env::var_os("GITSWITCH_AUTOSTART_DIR").filter(|d| !d.is_empty()) {
            return Some(Place::File(PathBuf::from(dir).join(file_name())));
        }
    }
    if cfg!(test) {
        return None;
    }
    default_place()
}

/// The program the entry starts: this one. An AppImage: its file, not the
/// temporary mount it runs from.
fn program() -> Option<String> {
    if cfg!(target_os = "linux") {
        if let Some(p) = std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p).to_string_lossy().into_owned());
        }
    }
    std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned())
}

/// What the entry says now (none: there isn't one).
fn read_entry(place: &Place) -> Option<String> {
    match place {
        Place::Registry { run, .. } => reg::read(run),
        Place::File(p) => std::fs::read_to_string(p).ok(),
    }
}

fn write_entry(place: &Place, text: &str) -> Result<(), String> {
    match place {
        Place::Registry { run, .. } => reg::write(run, text),
        Place::File(p) => {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
            }
            let tmp = p.with_extension("tmp");
            std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, p)).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("{}: {}", p.display(), e)
            })
        }
    }
}

/// True when there was an entry to remove.
fn remove_entry(place: &Place) -> Result<bool, String> {
    match place {
        Place::Registry { run, .. } => reg::remove(run),
        Place::File(p) => match std::fs::remove_file(p) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("{}: {}", p.display(), e)),
        },
    }
}

fn blocked(place: &Place) -> Option<String> {
    match place {
        Place::Registry { approved, .. } if !reg::approved(approved) => {
            Some("Windows' Startup apps has GitSwitch turned off, so it won't start when you sign in.".into())
        }
        _ => None,
    }
}

pub(crate) fn status_at(place: &Place, program: &str, on: bool) -> StartupView {
    let text = read_entry(place);
    StartupView { on, active: text.as_deref() == Some(entry_for(program).as_str()), blocked: text.as_ref().and_then(|_| blocked(place)), error: None }
}

/// The entry made to match the choice: there, naming this program, or gone.
pub(crate) fn apply_at(place: &Place, program: &str, want: bool) -> StartupView {
    let expected = entry_for(program);
    let now = read_entry(place);
    let result = if want {
        if program.contains(['\n', '\r']) {
            Err("this program's path has a line break in it, which a start-up entry can't hold".to_string())
        } else if now.as_deref() == Some(expected.as_str()) {
            Ok(())
        } else {
            write_entry(place, &expected)
        }
    } else if now.is_some() {
        remove_entry(place).map(|_| ())
    } else {
        Ok(())
    };
    let mut view = status_at(place, program, want);
    view.error = result.err().map(|e| {
        if want {
            format!("GitSwitch could not be set to start when you sign in: {}", e)
        } else {
            format!("GitSwitch's start-up entry could not be removed: {}", e)
        }
    });
    view
}

fn unavailable(on: bool) -> StartupView {
    StartupView { on, error: Some("This system's start-up settings could not be found.".into()), ..Default::default() }
}

/// What the system has now, for the person's choice `on`. Nothing is changed.
pub fn status(on: bool) -> StartupView {
    match (place(), program()) {
        (Some(place), Some(program)) => status_at(&place, &program, on),
        _ => unavailable(on),
    }
}

/// The system's entry made to match the choice.
pub fn apply(want: bool) -> StartupView {
    match (place(), program()) {
        (Some(place), Some(program)) => apply_at(&place, &program, want),
        // Nothing to take away where nothing can be found.
        _ if !want => StartupView::default(),
        _ => unavailable(want),
    }
}

/// Uninstalling: is there an entry, whatever program it names?
pub fn present() -> bool {
    place().is_some_and(|p| read_entry(&p).is_some())
}

/// Uninstalling: the entry goes, whatever program it names. True when there was one.
pub fn take_back() -> Result<bool, String> {
    match place() {
        Some(p) => remove_entry(&p),
        None => Ok(false),
    }
}

#[cfg(windows)]
mod reg {
    use super::NAME;
    use std::io::ErrorKind;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;

    fn hkcu() -> RegKey {
        RegKey::predef(HKEY_CURRENT_USER)
    }

    pub fn read(key: &str) -> Option<String> {
        hkcu().open_subkey(key).ok()?.get_value::<String, _>(NAME).ok()
    }

    pub fn write(key: &str, text: &str) -> Result<(), String> {
        let (k, _) = hkcu().create_subkey(key).map_err(|e| e.to_string())?;
        k.set_value(NAME, &text.to_string()).map_err(|e| e.to_string())
    }

    pub fn remove(key: &str) -> Result<bool, String> {
        match hkcu().open_subkey_with_flags(key, KEY_SET_VALUE) {
            Ok(k) => match k.delete_value(NAME) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e.to_string()),
            },
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn approved(key: &str) -> bool {
        hkcu().open_subkey(key).and_then(|k| k.get_raw_value(NAME)).map(|v| super::approved(&v.bytes)).unwrap_or(true)
    }

    /// Tests: a throwaway key under HKCU, gone afterwards, and its parent
    /// once no other test's key is left in it.
    #[cfg(test)]
    pub fn delete_tree(key: &str) {
        let _ = hkcu().delete_subkey_all(key);
        if let Some((parent, _)) = key.rsplit_once('\\') {
            let _ = hkcu().delete_subkey(parent);
        }
    }

    #[cfg(test)]
    pub fn set_approved(key: &str, bytes: &[u8]) {
        let (k, _) = hkcu().create_subkey(key).unwrap();
        k.set_raw_value(NAME, &winreg::RegValue { bytes: bytes.to_vec(), vtype: winreg::enums::RegType::REG_BINARY }).unwrap();
    }
}

#[cfg(not(windows))]
mod reg {
    pub fn read(_: &str) -> Option<String> {
        None
    }
    pub fn write(_: &str, _: &str) -> Result<(), String> {
        Err("the registry is Windows' only".into())
    }
    pub fn remove(_: &str) -> Result<bool, String> {
        Ok(false)
    }
    pub fn approved(_: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_at_sign_in_is_told_apart_and_keeps_the_window_hidden_only_with_something_to_show() {
        assert!(launched_at_login(&["gitswitch.exe", "--startup"]));
        assert!(!launched_at_login(&["gitswitch.exe"]));
        assert!(!launched_at_login(&["gitswitch.exe", "gitswitch://open", "--startupx"]));
        assert!(start_hidden(true, true));
        assert!(!start_hidden(true, false), "nothing shown: the window, never an unseen app");
        assert!(!start_hidden(false, true), "started by hand: the window");
    }

    #[test]
    fn the_entry_lives_where_each_system_looks_and_a_test_never_reaches_it() {
        match default_place() {
            Some(Place::Registry { run, approved }) if cfg!(windows) => {
                assert_eq!(run, r"Software\Microsoft\Windows\CurrentVersion\Run");
                assert_eq!(approved, r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");
            }
            Some(Place::File(p)) if cfg!(target_os = "macos") => assert!(p.ends_with("Library/LaunchAgents/com.gitswitch.app.startup.plist"), "{p:?}"),
            Some(Place::File(p)) if !cfg!(windows) => assert!(p.ends_with("autostart/gitswitch.desktop"), "{p:?}"),
            other => panic!("not where this system keeps start-up entries: {other:?}"),
        }
        // Run without the suites' override, a test build has nowhere to write.
        if std::env::var_os("GITSWITCH_AUTOSTART_KEY").is_none() && std::env::var_os("GITSWITCH_AUTOSTART_DIR").is_none() {
            assert!(place().is_none());
            assert!(!present());
            assert_eq!(take_back(), Ok(false));
            assert!(apply(true).error.is_some() && !apply(true).active);
        }
    }

    #[test]
    fn windows_entry_and_startup_apps_switch_are_read_as_windows_writes_them() {
        assert_eq!(run_command(r"C:\Program Files\GitSwitch\gitswitch.exe"), r#""C:\Program Files\GitSwitch\gitswitch.exe" --startup"#);
        assert!(approved(&[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(approved(&[0x06, 0, 0, 0]));
        assert!(!approved(&[0x03, 0, 0, 0, 0x10, 0x20, 0, 0, 0, 0, 0, 0]), "turned off in Startup apps");
        assert!(!approved(&[0x07]));
        assert!(approved(&[]), "never touched");
    }

    #[test]
    fn the_macos_launch_agent_names_the_program_and_escapes_it() {
        let p = launch_agent("/Applications/Git & Co <beta>.app/Contents/MacOS/gitswitch");
        assert!(p.contains("<string>/Applications/Git &amp; Co &lt;beta&gt;.app/Contents/MacOS/gitswitch</string>\n\t\t<string>--startup</string>"), "{p}");
        assert!(p.contains("<key>Label</key>\n\t<string>com.gitswitch.app.startup</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(p.contains("<string>Aqua</string>"));
        assert!(p.starts_with("<?xml") && p.trim_end().ends_with("</plist>"));
    }

    #[test]
    fn the_desktop_entry_quotes_exec_as_the_spec_asks() {
        assert_eq!(exec_arg("/usr/bin/gitswitch"), "/usr/bin/gitswitch");
        assert_eq!(exec_arg("/home/me/My Apps/GitSwitch.AppImage"), "\"/home/me/My Apps/GitSwitch.AppImage\"");
        assert_eq!(exec_arg("/opt/a$b"), "\"/opt/a\\\\$b\"", "a $ is \\\\$ in the file");
        assert_eq!(exec_arg("/opt/a\\b"), "\"/opt/a\\\\\\\\b\"", "a backslash is four");
        assert_eq!(exec_arg("/opt/100%/x"), "/opt/100%%/x", "a % is not a field code");
        assert_eq!(exec_arg("/opt/\"q\""), "\"/opt/\\\\\"q\\\\\"\"");
        let d = desktop_entry("/home/me/My Apps/GitSwitch.AppImage");
        assert!(d.starts_with("[Desktop Entry]\nType=Application\nName=GitSwitch\n"), "{d}");
        assert!(d.contains("\nExec=\"/home/me/My Apps/GitSwitch.AppImage\" --startup\n"), "{d}");
        assert!(d.contains("\nTryExec=/home/me/My Apps/GitSwitch.AppImage\n"), "{d}");
        assert!(d.contains("\nX-GNOME-Autostart-enabled=true\n"));
    }

    /// A throwaway place for the entry: a test key under HKCU, removed afterwards.
    #[cfg(windows)]
    fn scratch(tag: &str) -> (Place, Box<dyn Fn()>) {
        let base = format!(r"Software\GitSwitchTest\autostart-{}-{}", tag, std::process::id());
        let place = Place::Registry { run: format!(r"{}\Run", base), approved: format!(r"{}\StartupApproved\Run", base) };
        (place, Box::new(move || reg::delete_tree(&base)))
    }

    /// A throwaway place for the entry: a file in a temporary folder that doesn't exist yet.
    #[cfg(not(windows))]
    fn scratch(tag: &str) -> (Place, Box<dyn Fn()>) {
        let dir = std::env::temp_dir().join(format!("gitswitch-autostart-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("deep").join("entry");
        (
            Place::File(file),
            Box::new(move || {
                let _ = std::fs::remove_dir_all(&dir);
            }),
        )
    }

    #[test]
    fn the_entry_is_made_kept_corrected_and_removed() {
        let (place, clean) = scratch("cycle");
        let program = if cfg!(windows) { r"C:\Program Files\GitSwitch\gitswitch.exe" } else { "/opt/GitSwitch/gitswitch" };
        assert_eq!(status_at(&place, program, false), StartupView::default(), "nothing there yet");
        let on = apply_at(&place, program, true);
        assert_eq!((on.on, on.active, on.blocked.clone(), on.error.clone()), (true, true, None, None), "{on:?}");
        assert_eq!(read_entry(&place).as_deref(), Some(entry_for(program).as_str()));
        assert_eq!(apply_at(&place, program, true), on, "again: nothing changes");
        let moved = if cfg!(windows) { r"D:\Apps\GitSwitch\gitswitch.exe" } else { "/home/me/Apps/gitswitch" };
        assert!(!status_at(&place, moved, true).active, "the entry names the old place");
        assert!(apply_at(&place, moved, true).active, "and is corrected");
        assert_eq!(read_entry(&place).as_deref(), Some(entry_for(moved).as_str()));
        let off = apply_at(&place, moved, false);
        assert_eq!((off.on, off.active, off.error.clone()), (false, false, None), "{off:?}");
        assert_eq!(read_entry(&place), None, "gone");
        assert_eq!(remove_entry(&place), Ok(false), "removing nothing is fine");
        clean();
    }

    #[cfg(windows)]
    #[test]
    fn startup_apps_turning_it_off_is_said_and_left_alone() {
        let (place, clean) = scratch("approved");
        let Place::Registry { approved: key, .. } = place.clone() else { unreachable!() };
        let program = r"C:\Program Files\GitSwitch\gitswitch.exe";
        assert!(apply_at(&place, program, true).blocked.is_none());
        reg::set_approved(&key, &[0x03, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
        let v = apply_at(&place, program, true);
        assert!(v.active && v.blocked.as_deref().is_some_and(|b| b.contains("Startup apps")), "{v:?}");
        assert!(!reg::approved(&key), "GitSwitch never switches it back on");
        reg::set_approved(&key, &[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(status_at(&place, program, true).blocked.is_none());
        clean();
    }
}
