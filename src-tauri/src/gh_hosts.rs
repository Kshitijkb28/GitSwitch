//! The GitHub CLI's logins, read from its own `hosts.yml`: no process, no
//! network. `gh auth status` checks every login over HTTPS one after another
//! (and may refresh tokens on the way), which held the Publish dialog for
//! seconds just to list a few names. The file is gh's internal format, so it
//! is read leniently: anything unexpected falls back to `gh auth status`.
//! Token values are never kept: a login's own keys (`oauth_token: …`) are
//! skipped without being split.

use std::path::PathBuf;

/// gh's configuration folder, found the way gh finds it (go-gh's
/// `config.ConfigDir`): GH_CONFIG_DIR, else XDG_CONFIG_HOME/gh, else
/// %AppData%\GitHub CLI on Windows, else ~/.config/gh.
pub(crate) fn config_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = var("GH_CONFIG_DIR") {
        return Some(d);
    }
    if let Some(x) = var("XDG_CONFIG_HOME") {
        return Some(x.join("gh"));
    }
    #[cfg(windows)]
    if let Some(a) = var("AppData") {
        return Some(a.join("GitHub CLI"));
    }
    dirs::home_dir().map(|h| h.join(".config").join("gh"))
}

/// A token in the environment makes gh answer as that token's account, which
/// the file doesn't list: only `gh auth status` knows it.
fn env_token_set() -> bool {
    ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

/// Whether a `gh` would start: logins without the program to hand out their
/// tokens would only fail later, one by one.
fn gh_findable() -> bool {
    let p = crate::spawn::program("gh");
    if p != std::ffi::OsStr::new("gh") {
        return true; // found along the PATH saved since this process started
    }
    let exe = if cfg!(windows) { "gh.exe" } else { "gh" };
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(exe).is_file()))
}

/// A GitHub login: letters, digits and single hyphens, at most 39, no hyphen
/// at either end. Anything else means the file isn't what this reads.
fn plain_login(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 39
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// `key: value` or `key:` (value empty). None for anything else: a list item,
/// a flow mapping, a key with no colon.
fn key_value(body: &str) -> Option<(&str, &str)> {
    if body.starts_with('-') || body.starts_with('[') || body.starts_with('{') {
        return None;
    }
    let (k, v) = match body.find(": ") {
        Some(i) => (&body[..i], &body[i + 2..]),
        None => (body.strip_suffix(':')?, ""),
    };
    let k = unquote(k);
    let v = v.split(" #").next().unwrap_or("").trim();
    (!k.is_empty()).then_some((k, v))
}

/// The logins in a hosts.yml, in gh's order: per host, the active one
/// (`user:`) first, then the others under `users:` as the file lists them.
/// gh before 2.40 kept only `user:`. Each login once. None when the text is
/// not laid out the way gh writes it.
pub(crate) fn parse_hosts(text: &str) -> Option<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Some(Vec::new());
    }
    fn push_once(out: &mut Vec<String>, login: &str) {
        if !out.iter().any(|l| l == login) {
            out.push(login.to_string());
        }
    }
    /// A host read to its end: its active login, then the others.
    fn flush(out: &mut Vec<String>, active: &mut Option<String>, listed: &mut Vec<String>) {
        if let Some(a) = active.take() {
            push_once(out, &a);
        }
        for l in listed.drain(..) {
            push_once(out, &l);
        }
    }
    let mut out: Vec<String> = Vec::new();
    // The host being read: its keys' indent, whether `users:` is open (and
    // its logins' indent), and what it listed so far.
    let mut in_host = false;
    let mut host_indent: Option<usize> = None;
    let mut users_open = false;
    let mut users_indent: Option<usize> = None;
    let mut active: Option<String> = None;
    let mut listed: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.starts_with('\t') {
            return None; // YAML has no tab indents: not gh's file
        }
        let body = line.trim_start_matches(' ');
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let indent = line.len() - body.len();
        if indent == 0 {
            flush(&mut out, &mut active, &mut listed);
            let (_host, v) = key_value(body)?;
            if !v.is_empty() && v != "{}" {
                return None;
            }
            in_host = true;
            host_indent = None;
            users_open = false;
            users_indent = None;
            continue;
        }
        if !in_host {
            return None;
        }
        let hi = *host_indent.get_or_insert(indent);
        if indent < hi {
            return None;
        }
        if indent == hi {
            users_open = false;
            users_indent = None;
            let (k, v) = key_value(body)?;
            match k {
                "users" => {
                    if v.is_empty() {
                        users_open = true;
                    } else if v != "{}" {
                        return None;
                    }
                }
                "user" => {
                    let l = unquote(v);
                    if !plain_login(l) {
                        return None;
                    }
                    active = Some(l.to_string());
                }
                _ => {} // git_protocol, oauth_token…: not read
            }
            continue;
        }
        if !users_open {
            continue; // something under another key of the host
        }
        let ui = *users_indent.get_or_insert(indent);
        if indent < ui {
            return None;
        }
        if indent > ui {
            continue; // a login's own keys (its token, when stored in the file): never split
        }
        let (k, v) = key_value(body)?;
        if !plain_login(k) || !(v.is_empty() || v == "{}") {
            return None;
        }
        listed.push(k.to_string());
    }
    flush(&mut out, &mut active, &mut listed);
    Some(out)
}

/// gh's logins from its hosts.yml, or None when only `gh auth status` can
/// tell: a token in the environment, no gh to start, no file, or a file not
/// laid out as gh writes it.
pub(crate) fn logins() -> Option<Vec<String>> {
    if env_token_set() {
        return None;
    }
    let path = config_dir()?.join("hosts.yml");
    let text = std::fs::read_to_string(path).ok()?;
    if !gh_findable() {
        return None;
    }
    parse_hosts(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logins_come_from_the_users_map_the_active_one_first() {
        let text = "github.com:\n    git_protocol: ssh\n    users:\n        alice:\n        bob:\n            oauth_token: gho_secret\n    user: bob\n";
        assert_eq!(parse_hosts(text), Some(vec!["bob".to_string(), "alice".to_string()]));
    }

    #[test]
    fn every_host_is_read_and_a_login_is_listed_once() {
        let text = "github.com:\n  users:\n    alice:\n  user: alice\nghe.example.com:\n  users:\n    alice:\n    carol:\n  user: carol\n";
        assert_eq!(parse_hosts(text), Some(vec!["alice".to_string(), "carol".to_string()]));
    }

    #[test]
    fn a_file_from_before_gh_2_40_has_only_the_user_key() {
        let text = "github.com:\n    oauth_token: gho_secret\n    user: Alice-1\n    git_protocol: https\n";
        assert_eq!(parse_hosts(text), Some(vec!["Alice-1".to_string()]));
    }

    #[test]
    fn crlf_a_byte_order_mark_comments_and_quotes_are_read() {
        let text = "\u{feff}# gh\r\n\"github.com\":\r\n    users:\r\n        'alice':\r\n    user: \"alice\"\r\n";
        assert_eq!(parse_hosts(text), Some(vec!["alice".to_string()]));
    }

    #[test]
    fn an_empty_file_or_map_lists_nobody() {
        assert_eq!(parse_hosts(""), Some(vec![]));
        assert_eq!(parse_hosts("{}\n"), Some(vec![]));
        assert_eq!(parse_hosts("github.com: {}\n"), Some(vec![]));
        assert_eq!(parse_hosts("github.com:\n    users: {}\n"), Some(vec![]));
    }

    #[test]
    fn anything_not_laid_out_as_gh_writes_it_falls_back() {
        for bad in [
            "github.com:\n\tuser: alice\n",
            "- github.com\n",
            "github.com: alice\n",
            "github.com:\n    user: not a login\n",
            "github.com:\n    users:\n        alice: gho_inline\n",
            "github.com:\n    users:\n        -alice:\n",
            "github.com:\n      users:\n    user: alice\n",
            "  github.com:\n",
            "github.com:\n    users:\n        alice:\n      bob:\n",
        ] {
            assert_eq!(parse_hosts(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_token_is_never_part_of_the_answer() {
        let text = "github.com:\n    users:\n        alice:\n            oauth_token: gho_SECRET\n    oauth_token: gho_SECRET2\n    user: alice\n";
        let got = parse_hosts(text).unwrap();
        assert!(got.iter().all(|l| !l.contains("gho_")), "{got:?}");
    }

    #[test]
    fn gh_names_its_folder_the_way_go_gh_does() {
        // Only GH_CONFIG_DIR is checked here: the other variables belong to
        // the whole test process, and other tests read them.
        if std::env::var_os("GH_CONFIG_DIR").is_some_and(|v| !v.is_empty()) {
            assert_eq!(config_dir(), std::env::var_os("GH_CONFIG_DIR").map(PathBuf::from));
        } else {
            assert!(config_dir().is_some());
        }
    }
}
