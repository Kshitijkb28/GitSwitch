//! GitSwitch's terminal guard, as a program.
//!
//! On macOS and Linux the terminal puts a small `sh` script named `git` first
//! on PATH (terminal.rs, `shim_script`). Windows has no `sh`, and a batch file
//! cannot see its arguments faithfully (`%*` mangles quotes and carets), so
//! the same three guards are this program, copied into the shim directory as
//! `git.exe`. It refuses what the app refuses everywhere else, then runs the
//! real git with the arguments untouched. It is a guard-rail against a slip,
//! not a barrier: an absolute path to git steps around it, and that is fine.
//!
//! The wording below is the sh script's, word for word, because the terminal
//! suite asserts on it.

use std::path::{Path, PathBuf};
use std::process::{exit, Command};

/// git's own options that take a value, so the value is never read as the
/// subcommand: `git -c foo=bar push` is a push.
const GIT_ARGS_WITH_VALUES: &[&str] = &["-c", "-C", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--super-prefix"];

/// The subcommand, skipping git's own options and their values.
pub fn verb(args: &[String]) -> Option<&str> {
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if GIT_ARGS_WITH_VALUES.contains(&a.as_str()) {
            skip = true;
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        return Some(a);
    }
    None
}

/// Why this command is refused, or nothing when it may run. `env` answers an
/// environment variable's value, so the rules can be tested without one.
pub fn refusal(args: &[String], env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let verb = verb(args).unwrap_or("");
    let set = |k: &str| env(k).filter(|v| !v.is_empty()).is_some();

    if verb == "push" {
        let bypass = if set("GIT_CONFIG_SYSTEM") {
            Some("GIT_CONFIG_SYSTEM")
        } else if set("GIT_CONFIG_NOSYSTEM") {
            Some("GIT_CONFIG_NOSYSTEM")
        } else {
            None
        };
        if let Some(var) = bypass {
            return Some(format!(
                "GitSwitch: this push would skip the system git config, which is where a push lock lives.\nUnset {} first, or use your own Terminal if you really mean to.",
                var
            ));
        }
    }

    // `-n` means --no-verify for commit but --dry-run for push, so only commit
    // takes the short form.
    if (verb == "commit" || verb == "push") && args.iter().any(|a| a == "--no-verify" || (verb == "commit" && a == "-n")) {
        return Some(
            "GitSwitch: --no-verify skips the identity guard that stops commits from the wrong account.\nDrop the flag, or use your own Terminal if you really mean to.".to_string(),
        );
    }

    if verb == "push" {
        let mut seen_verb = false;
        for a in args {
            if a == "--force" || a == "-f" || a == "--force-with-lease" || a.starts_with("--force-with-lease=") {
                return Some("GitSwitch never force-pushes. Use your own Terminal if you really mean to.".to_string());
            }
            if a == "push" {
                seen_verb = true;
            } else if seen_verb && a.starts_with('+') {
                // A refspec beginning with + is a force in another spelling.
                return Some("GitSwitch never force-pushes, and a leading + is a force. Use your own Terminal if you mean to.".to_string());
            }
        }
    }
    None
}

/// The git to hand off to: the one the terminal named, else the first on
/// PATH that is not this program's own directory.
fn real_git() -> PathBuf {
    if let Some(p) = std::env::var_os("GITSWITCH_REAL_GIT").map(PathBuf::from) {
        if p.is_file() {
            return p;
        }
    }
    let me = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let exe = if cfg!(windows) { "git.exe" } else { "git" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if me.as_deref() == Some(dir.as_path()) {
                continue;
            }
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("git")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(why) = refusal(&args, |k| std::env::var(k).ok()) {
        eprintln!("{}", why);
        exit(1);
    }
    match Command::new(real_git()).args(&args).status() {
        Ok(status) => exit(status.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("GitSwitch: could not run git: {}", e);
            exit(127);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }
    fn no_env(_: &str) -> Option<String> {
        None
    }
    fn with(var: &'static str) -> impl Fn(&str) -> Option<String> {
        move |k| (k == var).then(|| "1".to_string())
    }

    #[test]
    fn ordinary_commands_pass() {
        for cmd in ["status --short --branch", "log --oneline", "push origin main", "push -n origin main", "commit -m x", "-c color.ui=never status"] {
            assert!(refusal(&a(cmd), no_env).is_none(), "{} should pass", cmd);
        }
    }

    #[test]
    fn the_documented_push_lock_bypass_is_refused_for_a_push_only() {
        let r = refusal(&a("push origin main"), with("GIT_CONFIG_NOSYSTEM")).unwrap();
        assert!(r.contains("would skip the system git config") && r.contains("GIT_CONFIG_NOSYSTEM"), "{}", r);
        let r = refusal(&a("push origin main"), with("GIT_CONFIG_SYSTEM")).unwrap();
        assert!(r.contains("GIT_CONFIG_SYSTEM"), "{}", r);
        assert!(!r.contains("--uninstall"), "never prints a way to switch a lock off");
        assert!(refusal(&a("status --short"), with("GIT_CONFIG_NOSYSTEM")).is_none(), "reading is still allowed");
    }

    #[test]
    fn force_pushes_in_every_spelling_are_refused() {
        for cmd in ["push --force origin main", "push -f origin main", "push --force-with-lease origin main", "push --force-with-lease=main:abc origin main"] {
            assert!(refusal(&a(cmd), no_env).unwrap().contains("never force-pushes"), "{}", cmd);
        }
        let r = refusal(&a("push origin +main:main"), no_env).unwrap();
        assert!(r.contains("a leading + is a force"), "{}", r);
    }

    #[test]
    fn no_verify_is_refused_and_n_only_means_it_for_commit() {
        assert!(refusal(&a("commit --no-verify --allow-empty -m x"), no_env).unwrap().contains("identity guard"));
        assert!(refusal(&a("commit -n --allow-empty -m x"), no_env).unwrap().contains("identity guard"));
        assert!(refusal(&a("push --no-verify origin main"), no_env).unwrap().contains("identity guard"));
        assert!(refusal(&a("push -n origin main"), no_env).is_none(), "push -n is a dry run");
    }

    #[test]
    fn a_flag_before_the_subcommand_does_not_hide_it() {
        assert!(refusal(&a("-c color.ui=never push --force origin main"), no_env).is_some());
        assert!(refusal(&a("-C . push --force origin main"), no_env).is_some());
        assert_eq!(verb(&a("-c k=v -C . --no-pager push x")), Some("push"));
        assert_eq!(verb(&a("--git-dir=.git status")), Some("status"));
        assert_eq!(verb(&a("--version")), None);
    }
}
