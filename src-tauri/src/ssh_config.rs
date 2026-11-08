//! What ssh itself would do for a host name.
//!
//! `ssh -G <host>` prints the configuration ssh would use after ~/.ssh/config,
//! `Include` and `Match` blocks — hostname, user, port, identity files —
//! without connecting (OpenSSH 6.8+, 2015; macOS, Linux and Windows' bundled
//! OpenSSH all have it). It is the only honest answer to "which key does
//! `git@gh-work:…` use": a hand parser of the config file would have to
//! reimplement `Match`, and would still disagree with ssh somewhere.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SshHost {
    pub alias: String,
    /// The real host: `github.com` behind `gh-work`.
    pub hostname: String,
    pub user: Option<String>,
    pub port: u16,
    /// As ssh lists them, `~` expanded. ssh tries them in this order.
    pub identity_files: Vec<String>,
    pub identities_only: bool,
}

/// `ssh -G` output: one `key value` per line, keys lowercase.
pub fn parse_ssh_g(alias: &str, out: &str) -> SshHost {
    let mut h = SshHost {
        alias: alias.to_string(),
        hostname: alias.to_string(),
        user: None,
        port: 22,
        identity_files: Vec::new(),
        identities_only: false,
    };
    for line in out.lines() {
        let Some((k, v)) = line.trim().split_once(' ') else { continue };
        let v = v.trim();
        match k.to_ascii_lowercase().as_str() {
            "hostname" => h.hostname = v.to_string(),
            "user" => h.user = Some(v.to_string()),
            "port" => h.port = v.parse().unwrap_or(22),
            "identityfile" => h.identity_files.push(expand_tilde(v)),
            "identitiesonly" => h.identities_only = v.eq_ignore_ascii_case("yes"),
            _ => {}
        }
    }
    h
}

fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    p.to_string()
}

/// Ask ssh, cached for a minute per alias: the Doctor asks for every remote of
/// every repository, and the answer only changes when ~/.ssh/config does.
pub async fn resolve(alias: &str) -> Option<SshHost> {
    if alias.is_empty() || alias.starts_with('-') {
        return None;
    }
    type Cache = HashMap<String, (Instant, Option<SshHost>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(c) = cache.lock() {
        if let Some((at, v)) = c.get(alias) {
            if at.elapsed() < Duration::from_secs(60) {
                return v.clone();
            }
        }
    }
    let mut cmd = crate::spawn::async_command("ssh");
    // Debug builds only: the suites point ssh at a sandbox config, because ssh
    // finds `~` through the passwd database rather than $HOME.
    #[cfg(debug_assertions)]
    if let Some(f) = std::env::var_os("GITSWITCH_SSH_CONFIG") {
        cmd.arg("-F").arg(f);
    }
    cmd.arg("-G").arg(alias);
    let out = tokio::time::timeout(Duration::from_secs(5), cmd.output()).await.ok().and_then(|r| r.ok());
    let host = out
        .filter(|o| o.status.success())
        .map(|o| parse_ssh_g(alias, &String::from_utf8_lossy(&o.stdout)));
    if let Ok(mut c) = cache.lock() {
        c.insert(alias.to_string(), (Instant::now(), host.clone()));
    }
    host
}

/// The SSH host, user and explicit port a git remote URL names — `None` for
/// HTTP(S), git:// and local paths. `git@gh-work:org/repo.git` → gh-work, git,
/// no port; `ssh://git@ssh.github.com:443/org/repo` → ssh.github.com, git, 443.
#[derive(Debug, Clone, PartialEq)]
pub struct SshRemote {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

pub fn ssh_host_of_url(url: &str) -> Option<SshRemote> {
    let u = url.trim();
    for scheme in ["http://", "https://", "file://", "git://", "gitswitch-push-blocked://"] {
        if u.starts_with(scheme) {
            return None;
        }
    }
    if let Some(rest) = u.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let (user, hostport) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u.to_string()), h),
            None => (None, authority),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, p.parse().ok()),
            _ => (hostport, None),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        return (!host.is_empty()).then(|| SshRemote { host: host.to_string(), user, port });
    }
    // scp-like `[user@]host:path`. Not a path: a slash before the first colon
    // means `./x:y` or `/a/b`, and a single letter before it is a drive (C:\…).
    let colon = u.find(':')?;
    if u[..colon].contains('/') {
        return None;
    }
    let before = &u[..colon];
    if before.is_empty() || (before.len() == 1 && before.chars().all(|c| c.is_ascii_alphabetic())) {
        return None;
    }
    let (user, host) = match before.rsplit_once('@') {
        Some((u, h)) => (Some(u.to_string()), h),
        None => (None, before),
    };
    (!host.is_empty()).then(|| SshRemote { host: host.to_string(), user, port: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_g_output_is_read_into_the_facts_a_push_depends_on() {
        let out = "user git\nhostname github.com\nport 22\nidentitiesonly yes\nidentityfile ~/.ssh/work_key\nidentityfile /abs/other\nbatchmode no\n";
        let h = parse_ssh_g("gh-work", out);
        assert_eq!(h.alias, "gh-work");
        assert_eq!(h.hostname, "github.com");
        assert_eq!(h.user.as_deref(), Some("git"));
        assert_eq!(h.port, 22);
        assert!(h.identities_only);
        assert_eq!(h.identity_files.len(), 2);
        // The home directory is joined by the platform (a backslash on Windows); ssh accepts either.
        let first = h.identity_files[0].replace('\\', "/");
        assert!(first.ends_with("/.ssh/work_key") && !first.starts_with('~'), "{:?}", h.identity_files);
        assert_eq!(h.identity_files[1], "/abs/other");
        // No answer at all: the alias is taken literally as the host.
        assert_eq!(parse_ssh_g("plain.host", "").hostname, "plain.host");
    }

    fn r(host: &str, user: Option<&str>, port: Option<u16>) -> Option<SshRemote> {
        Some(SshRemote { host: host.into(), user: user.map(String::from), port })
    }

    #[test]
    fn the_ssh_host_is_read_from_every_remote_spelling_and_from_nothing_else() {
        assert_eq!(ssh_host_of_url("git@github.com:acme/x.git"), r("github.com", Some("git"), None));
        assert_eq!(ssh_host_of_url("git@gh-work:acme/x.git"), r("gh-work", Some("git"), None));
        assert_eq!(ssh_host_of_url("gh-work:acme/x.git"), r("gh-work", None, None));
        assert_eq!(ssh_host_of_url("ssh://git@github.com/acme/x.git"), r("github.com", Some("git"), None));
        // GitHub's documented firewall form: the port must survive, or the probe asks port 22.
        assert_eq!(ssh_host_of_url("ssh://git@ssh.github.com:443/acme/x.git"), r("ssh.github.com", Some("git"), Some(443)));
        assert_eq!(ssh_host_of_url("ssh://host/acme/x"), r("host", None, None));
        for not_ssh in ["https://github.com/acme/x.git", "http://h/x", "git://h/x", "/abs/path/remote.git", "./rel:ative", "C:\\Users\\me\\remote.git", "gitswitch-push-blocked://github.com/x", "no-colon-here"] {
            assert_eq!(ssh_host_of_url(not_ssh), None, "{}", not_ssh);
        }
    }
}
