use crate::error::AppError;
use serde::Serialize;

/// `gh` could not be started. "No such file" means it is not installed, which
/// deserves the answer in words — the raw OS error reads like a bug.
fn gh_error(e: std::io::Error) -> AppError {
    if e.kind() == std::io::ErrorKind::NotFound {
        AppError::Command(
            "The GitHub CLI (gh) isn't installed on this computer. Install it from https://cli.github.com, sign in with `gh auth login`, then try again.".into(),
        )
    } else {
        AppError::Command(format!("Failed to run gh: {}", e))
    }
}

/// One account `gh auth status` lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GhAccount {
    pub host: String,
    pub login: String,
    /// "- Active account: true|false"; None from a gh that does not say.
    pub active: Option<bool>,
    /// "- Token scopes: …": Some(empty) for "none", None when gh prints no
    /// scopes line (a fine-grained or app token: scopes do not apply).
    pub scopes: Option<Vec<String>>,
}

/// gh colours its output on a terminal; through a pipe it should not, but a
/// stray escape must not become part of a login.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Every account in `gh auth status`, in its order: "✓ Logged in to <host>
/// account <login> (keyring)" and "X Failed to log in to <host> account
/// <login>" each start one; the lines under it say whether it is active and
/// which scopes its token has. The token line itself is never read.
pub(crate) fn parse_auth_status(text: &str) -> Vec<GhAccount> {
    let mut out: Vec<GhAccount> = Vec::new();
    for raw in strip_ansi(text).lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("- Active account:") {
            if let Some(a) = out.last_mut() {
                a.active = Some(rest.trim() == "true");
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("- Token scopes:") {
            if let Some(a) = out.last_mut() {
                let list: Vec<String> = rest
                    .split(',')
                    .map(|s| s.trim().trim_matches('\'').trim_matches('"').trim().to_string())
                    .filter(|s| !s.is_empty() && s != "none")
                    .collect();
                a.scopes = Some(list);
            }
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(i) = words.iter().position(|w| *w == "account") else { continue };
        if i < 2 || i + 1 >= words.len() || words[i - 2] != "to" {
            continue;
        }
        let (host, login) = (words[i - 1].to_string(), words[i + 1].trim_end_matches(':').to_string());
        if login.is_empty() || out.iter().any(|a| a.host == host && a.login == login) {
            continue;
        }
        out.push(GhAccount { host, login, active: None, scopes: None });
    }
    out
}

/// `gh auth status`, both streams: gh writes it to stderr in some versions.
pub async fn gh_auth_status() -> Result<Vec<GhAccount>, AppError> {
    let output = crate::spawn::async_command(crate::spawn::program("gh"))
        .args(["auth", "status"])
        .output()
        .await
        .map_err(gh_error)?;
    Ok(parse_auth_status(&format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )))
}

/// The logins of a status listing, once each in its order.
fn logins(accounts: &[GhAccount]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in accounts {
        if !out.contains(&a.login) {
            out.push(a.login.clone());
        }
    }
    out
}

/// List the GitHub accounts the local `gh` CLI is authenticated with.
pub async fn gh_list_accounts() -> Result<Vec<String>, AppError> {
    let accounts = logins(&gh_auth_status().await?);
    if accounts.is_empty() {
        return Err(AppError::Command(
            "No gh accounts found. Run `gh auth login` first.".into(),
        ));
    }
    Ok(accounts)
}

/// The scopes a gh token lacks for what GitSwitch asks of it, each with what
/// needs it: registering SSH keys, private repositories, and — for an account
/// a signing profile uses — registering signing keys. (user:email is the
/// profile form's own concern: it says so where it matters.)
pub(crate) fn missing_scopes(scopes: &[String], needs_signing: bool) -> Vec<(&'static str, &'static str)> {
    let has = |s: &str| scopes.iter().any(|x| x == s);
    let mut out = Vec::new();
    if !has("admin:public_key") && !has("write:public_key") {
        out.push(("admin:public_key", "registering SSH keys on the account"));
    }
    if !has("repo") {
        out.push(("repo", "listing and reaching private repositories"));
    }
    if needs_signing && !has("write:ssh_signing_key") && !has("admin:ssh_signing_key") {
        out.push(("write:ssh_signing_key", "registering a signing key for verified commits"));
    }
    out
}

/// Log a specific account out of the local `gh` CLI (removes its stored
/// token) — from every host gh has it signed in to: the list shows one row
/// per login, and gh refuses `--user` alone when the login is on two hosts.
pub async fn gh_logout(account: &str) -> Result<String, AppError> {
    let hosts: Vec<String> = gh_auth_status()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.login.eq_ignore_ascii_case(account))
        .map(|a| a.host)
        .collect();
    let runs: Vec<Vec<String>> = if hosts.is_empty() {
        vec![vec!["auth".into(), "logout".into(), "--user".into(), account.into()]]
    } else {
        hosts.iter().map(|h| vec!["auth".into(), "logout".into(), "--hostname".into(), h.clone(), "--user".into(), account.into()]).collect()
    };
    let mut removed = false;
    for args in runs {
        let output = crate::spawn::async_command(crate::spawn::program("gh")).args(&args).output().await.map_err(gh_error)?;
        if output.status.success() {
            removed = true;
            continue;
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.contains("no accounts matched") {
            // Signed out elsewhere already (a terminal, another app): the row was stale.
            continue;
        }
        let host = args.iter().position(|a| a == "--hostname").and_then(|i| args.get(i + 1)).cloned();
        return Err(AppError::Command(match host {
            Some(h) => format!("gh could not log {} out of {}: {}", account, h, stderr),
            None => format!("gh logout failed for {}: {}", account, stderr),
        }));
    }
    Ok(if removed {
        format!("Removed {} from the GitHub CLI", account)
    } else {
        format!("{} was already signed out of the GitHub CLI", account)
    })
}

/// Get an OAuth token for a specific gh account without switching the active one.
pub async fn gh_get_token(account: &str) -> Result<String, AppError> {
    let output = crate::spawn::async_command(crate::spawn::program("gh"))
        .args(["auth", "token", "--user", account])
        .output()
        .await
        .map_err(gh_error)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Command(format!(
            "gh auth token failed for {}: {}",
            account, stderr
        )));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(AppError::Command("gh returned an empty token".into()));
    }
    Ok(token)
}

/// A host or login is one plain word: gh would read anything starting with
/// `-` as a flag, and a space or newline is not part of any login.
fn plain_word(kind: &str, value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.starts_with('-') || value.chars().any(char::is_whitespace) {
        return Err(AppError::Config(format!("Not a {}: {:?}", kind, value)));
    }
    Ok(())
}

/// The argv of the switch, host and login named explicitly: with more than
/// two logins on a host gh would otherwise ask at a prompt no one can see.
pub(crate) fn switch_args(host: &str, login: &str) -> Result<Vec<String>, AppError> {
    plain_word("host", host)?;
    plain_word("login", login)?;
    Ok(vec!["auth".into(), "switch".into(), "--hostname".into(), host.into(), "--user".into(), login.into()])
}

/// Make `login` the account gh itself uses on `host` — what `gh` commands in
/// a terminal, `gh auth refresh`, and git's HTTPS credential helper (`gh auth
/// git-credential`) act as from now on. GitSwitch's own reads name their
/// account (`gh auth token --user`), so nothing in the app changes with it.
/// `GH_TOKEN`/`GITHUB_TOKEN` are kept from the child: gh refuses to manage
/// accounts while one is set, and the app may have inherited one from the
/// terminal that started it.
pub async fn gh_auth_switch(host: &str, login: &str) -> Result<String, AppError> {
    let args = switch_args(host, login)?;
    let output = crate::spawn::async_command(crate::spawn::program("gh"))
        .args(&args)
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .output()
        .await
        .map_err(gh_error)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AppError::Command(format!("gh could not make {} active on {}: {}", login, host, stderr)));
    }
    // gh's exit code says it ran; its own listing says what is active now.
    let now = gh_auth_status().await?;
    let active = now
        .iter()
        .find(|a| a.host.eq_ignore_ascii_case(host) && a.login.eq_ignore_ascii_case(login))
        .and_then(|a| a.active);
    match active {
        Some(true) | None => Ok(format!("{} is now the GitHub CLI's active account on {}", login, host)),
        Some(false) => {
            let says = now
                .iter()
                .find(|a| a.host.eq_ignore_ascii_case(host) && a.active == Some(true))
                .map(|a| a.login.clone())
                .unwrap_or_else(|| "no account".into());
            Err(AppError::Command(format!("gh switched, but still reports {} as the active account on {}", says, host)))
        }
    }
}

// --- `gh auth login` from the app ------------------------------------------
//
// gh's own browser sign-in, run through a pipe: without a terminal gh asks
// nothing, prints the one-time code and the address, and waits (about 15
// minutes) for the code to be entered on GitHub. The page shows the code and
// waits on `gh_auth_login_wait`; Cancel or leaving the page ends gh.

/// The scopes GitSwitch asks of a gh token beyond gh's own defaults (repo,
/// read:org, gist): registering SSH and signing keys, and reading the
/// account's verified addresses — what `missing_scopes` would otherwise flag.
const LOGIN_SCOPES: &str = "admin:public_key,admin:ssh_signing_key,user:email";

/// gh gives up on the code after 15 minutes; this is only the backstop.
const LOGIN_WAIT: std::time::Duration = std::time::Duration::from_secs(16 * 60);

/// How long gh may take to print the code before it is called stuck.
const LOGIN_PROMPT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The code to enter on GitHub, and where.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GhDevicePrompt {
    pub code: String,
    pub url: String,
}

/// The account a finished sign-in added (or signed in again).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GhLogin {
    pub host: String,
    pub login: String,
}

/// The argv of the sign-in: host named, SSH as git's protocol (GitSwitch
/// keeps keys per folder, so gh must not install itself as git's HTTPS
/// credential helper), and no key upload — gh would generate or upload one
/// of its own choosing.
pub(crate) fn login_args(host: &str) -> Result<Vec<String>, AppError> {
    plain_word("host", host)?;
    Ok(["auth", "login", "--hostname", host, "--web", "--git-protocol", "ssh", "--skip-ssh-key", "--scopes", LOGIN_SCOPES]
        .iter()
        .map(|s| s.to_string())
        .collect())
}

/// A `XXXX-XXXX` one-time code in one of gh's lines, in either spelling gh
/// has used ("! One-time code (XXXX-XXXX) copied to clipboard", "! First copy
/// your one-time code: XXXX-XXXX").
pub(crate) fn device_code_in(line: &str) -> Option<String> {
    strip_ansi(line)
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .find(|w| {
            let b = w.as_bytes();
            b.len() == 9 && b[4] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        .map(str::to_string)
}

/// The device page's address in one of gh's lines.
pub(crate) fn device_url_in(line: &str) -> Option<String> {
    strip_ansi(line)
        .split_whitespace()
        .find(|w| w.starts_with("https://") && w.contains("/login/device"))
        .map(|w| w.trim_end_matches(['.', ',']).to_string())
}

/// "✓ Logged in as <login>": gh's own last word on who signed in.
pub(crate) fn logged_in_as(line: &str) -> Option<String> {
    let line = strip_ansi(line);
    let rest = line.split("Logged in as ").nth(1)?;
    rest.split_whitespace().next().map(|l| l.trim_end_matches(['.', ',']).to_string()).filter(|l| !l.is_empty())
}

/// gh's last words on a failed sign-in, as the person should read them.
pub(crate) fn login_failure(lines: &[String]) -> String {
    let last = lines.iter().rev().map(|l| strip_ansi(l).trim().to_string()).find(|l| !l.is_empty());
    match last {
        Some(l) if l.contains("context deadline exceeded") || l.contains("expired") => {
            "The code expired before it was entered on GitHub. Start the sign-in again for a new code.".into()
        }
        Some(l) if l.contains("access_denied") || l.contains("denied") => "The sign-in was denied on GitHub.".into(),
        Some(l) => format!("gh could not sign in: {}", l),
        None => "gh stopped without signing in.".into(),
    }
}

/// A sign-in gh is running: its process, every line it printed (both streams,
/// in order), the host, and the logins that host had before.
struct Login {
    id: u64,
    child: tokio::process::Child,
    lines: tokio::sync::mpsc::UnboundedReceiver<String>,
    seen: Vec<String>,
    host: String,
    cancel: tokio::sync::oneshot::Receiver<()>,
}

/// The sign-in started and not yet waited on.
static PENDING: std::sync::Mutex<Option<Login>> = std::sync::Mutex::new(None);
/// The Cancel of the sign-in in progress, by id: a late Cancel of an earlier
/// sign-in must not end the next one.
static CANCEL: std::sync::Mutex<Option<(u64, tokio::sync::oneshot::Sender<()>)>> = std::sync::Mutex::new(None);
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Each line of a child's stream into `tx`, until the stream ends.
fn forward_lines<R: tokio::io::AsyncRead + Unpin + Send + 'static>(stream: R, tx: tokio::sync::mpsc::UnboundedSender<String>) {
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(stream).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
}

/// Start `gh auth login` for `host` and answer with the code it shows. A
/// sign-in already running is ended first: there is one code on screen.
pub async fn gh_auth_login_start(host: &str) -> Result<GhDevicePrompt, AppError> {
    let args = login_args(host)?;
    gh_auth_login_cancel();
    let mut child = crate::spawn::async_command(crate::spawn::program("gh"))
        .args(&args)
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(gh_error)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    if let Some(out) = child.stdout.take() {
        forward_lines(out, tx.clone());
    }
    if let Some(err) = child.stderr.take() {
        forward_lines(err, tx);
    }
    let mut seen: Vec<String> = Vec::new();
    let mut code: Option<String> = None;
    let mut url: Option<String> = None;
    let deadline = tokio::time::Instant::now() + LOGIN_PROMPT_WAIT;
    // The code and the address come on two lines; once the code is here the
    // address gets a moment, and gh's usual one is assumed without it.
    while url.is_none() {
        let until = if code.is_some() { deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(2)) } else { deadline };
        match tokio::time::timeout_at(until, rx.recv()).await {
            Ok(Some(line)) => {
                code = code.or_else(|| device_code_in(&line));
                url = url.or_else(|| device_url_in(&line));
                seen.push(line);
            }
            // Both streams closed: gh has ended without a code.
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let Some(code) = code else {
        let _ = child.start_kill();
        return Err(AppError::Command(if seen.is_empty() { "gh did not show a sign-in code.".into() } else { login_failure(&seen) }));
    };
    let url = url.unwrap_or_else(|| format!("https://{}/login/device", host));
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    *CANCEL.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, cancel_tx));
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Login { id, child, lines: rx, seen, host: host.to_string(), cancel: cancel_rx });
    Ok(GhDevicePrompt { code, url })
}

/// Wait for the sign-in `gh_auth_login_start` began: the account gh signed
/// in, or why it did not.
pub async fn gh_auth_login_wait() -> Result<GhLogin, AppError> {
    let taken = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(mut job) = taken else {
        return Err(AppError::Command("No GitHub CLI sign-in is running.".into()));
    };
    let outcome = tokio::select! {
        r = tokio::time::timeout(LOGIN_WAIT, job.child.wait()) => Some(r),
        _ = &mut job.cancel => None,
    };
    let clear = |id: u64| {
        let mut c = CANCEL.lock().unwrap_or_else(|e| e.into_inner());
        if c.as_ref().is_some_and(|(i, _)| *i == id) {
            *c = None;
        }
    };
    clear(job.id);
    let status = match outcome {
        None => {
            let _ = job.child.kill().await;
            return Err(AppError::Command("The sign-in was cancelled.".into()));
        }
        Some(Err(_)) => {
            let _ = job.child.kill().await;
            return Err(AppError::Command(login_failure(&["the code expired".to_string()])));
        }
        Some(Ok(s)) => s.map_err(|e| AppError::Command(format!("Failed to wait for gh: {}", e)))?,
    };
    // What gh printed after the code, up to its exit (the readers end with it).
    while let Ok(Some(l)) = tokio::time::timeout(std::time::Duration::from_secs(1), job.lines.recv()).await {
        job.seen.push(l);
    }
    if !status.success() {
        return Err(AppError::Command(login_failure(&job.seen)));
    }
    if let Some(login) = job.seen.iter().rev().find_map(|l| logged_in_as(l)) {
        return Ok(GhLogin { host: job.host, login });
    }
    // An older gh that does not say: its listing does — the active login on the host.
    let now = gh_auth_status().await.unwrap_or_default();
    now.into_iter()
        .find(|a| a.host.eq_ignore_ascii_case(&job.host) && a.active == Some(true))
        .map(|a| GhLogin { host: a.host, login: a.login })
        .ok_or_else(|| AppError::Command("gh finished, but its listing names no signed-in account on this host.".into()))
}

/// End the sign-in in progress, if any: the one being waited on, or one
/// started and not yet waited on.
pub fn gh_auth_login_cancel() {
    if let Some((_, tx)) = CANCEL.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = tx.send(());
    }
    if let Some(mut job) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = job.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_switch_names_host_and_login_and_refuses_what_is_not_a_word() {
        assert_eq!(
            switch_args("github.com", "bob-work").unwrap(),
            vec!["auth", "switch", "--hostname", "github.com", "--user", "bob-work"]
        );
        for (host, login) in [("github.com", "-u"), ("github.com", ""), ("github.com", "bob work"), ("", "bob"), ("-h", "bob"), ("git hub.com", "bob")] {
            assert!(switch_args(host, login).is_err(), "{host:?} {login:?} must be refused");
        }
    }

    const STATUS: &str = "github.com\n  \u{1b}[32m✓\u{1b}[0m Logged in to github.com account alice-dev (keyring)\n  - Active account: true\n  - Git operations protocol: ssh\n  - Token: gho_************************************\n  - Token scopes: 'gist', 'read:org', 'repo'\n\n  ✓ Logged in to github.com account bob-work (keyring)\n  - Active account: false\n  - Git operations protocol: https\n  - Token: gho_************************************\n  - Token scopes: none\n\n  ✓ Logged in to github.com account carol-fg (GH_TOKEN)\n  - Active account: false\n  - Token: github_pat_************************************\n\nghe.example.com\n  ✓ Logged in to ghe.example.com account alice-dev (keyring)\n  X Failed to log in to github.com account dave (keyring)\n";

    #[test]
    fn gh_status_names_each_account_with_its_host_state_and_scopes() {
        let a = parse_auth_status(STATUS);
        let brief: Vec<(&str, &str, Option<bool>)> = a.iter().map(|x| (x.host.as_str(), x.login.as_str(), x.active)).collect();
        assert_eq!(
            brief,
            vec![
                ("github.com", "alice-dev", Some(true)),
                ("github.com", "bob-work", Some(false)),
                ("github.com", "carol-fg", Some(false)),
                ("ghe.example.com", "alice-dev", None),
                ("github.com", "dave", None),
            ]
        );
        assert_eq!(a[0].scopes, Some(vec!["gist".into(), "read:org".into(), "repo".into()]));
        assert_eq!(a[1].scopes, Some(vec![]));
        assert_eq!(a[2].scopes, None);
        assert_eq!(logins(&a), vec!["alice-dev", "bob-work", "carol-fg", "dave"]);
        assert!(parse_auth_status("  - Active account: true\n").is_empty());
    }

    #[test]
    fn a_sign_in_names_its_host_keeps_git_on_ssh_and_uploads_no_key() {
        let a = login_args("github.com").unwrap();
        assert_eq!(&a[..4], ["auth", "login", "--hostname", "github.com"]);
        for flag in ["--web", "--skip-ssh-key"] {
            assert!(a.iter().any(|x| x == flag), "{flag} missing from {a:?}");
        }
        assert!(a.windows(2).any(|w| w == ["--git-protocol", "ssh"]), "{a:?}");
        let scopes = a.iter().position(|x| x == "--scopes").map(|i| a[i + 1].clone()).unwrap();
        let granted: Vec<String> = scopes.split(',').map(str::to_string).chain(["repo".to_string()]).collect();
        assert!(missing_scopes(&granted, true).is_empty(), "a new login must not be flagged by Doctor: {scopes}");
        for host in ["", "-h", "git hub.com"] {
            assert!(login_args(host).is_err(), "{host:?} must be refused");
        }
    }

    #[test]
    fn the_code_and_the_address_are_read_from_what_gh_prints() {
        assert_eq!(device_code_in("! One-time code (C027-AF8F) copied to clipboard").as_deref(), Some("C027-AF8F"));
        assert_eq!(device_code_in("\u{1b}[33m!\u{1b}[0m First copy your one-time code: \u{1b}[1m6E2B-91D0\u{1b}[0m").as_deref(), Some("6E2B-91D0"));
        assert_eq!(device_code_in("Open this URL to continue in your web browser: https://github.com/login/device"), None);
        assert_eq!(device_code_in("- gh config set -h github.com git_protocol ssh"), None);
        assert_eq!(device_code_in("lower-case abcd-1234 is not a code"), None);
        assert_eq!(
            device_url_in("Open this URL to continue in your web browser: https://github.com/login/device").as_deref(),
            Some("https://github.com/login/device")
        );
        assert_eq!(device_url_in("- Press Enter to open https://ghe.example.com/login/device in your browser...").as_deref(), Some("https://ghe.example.com/login/device"));
        assert_eq!(logged_in_as("\u{1b}[32m✓\u{1b}[0m Logged in as \u{1b}[1mKshitijkb28\u{1b}[0m").as_deref(), Some("Kshitijkb28"));
        assert_eq!(logged_in_as("✓ Configured git protocol"), None);
    }

    #[test]
    fn a_failed_sign_in_is_said_in_words() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(login_failure(&l(&["! One-time code (C6C5-CFFF) copied to clipboard", "failed to authenticate via web browser: context deadline exceeded", ""])).contains("code expired"));
        assert_eq!(login_failure(&l(&["The value of the GH_TOKEN environment variable is being used for authentication."])), "gh could not sign in: The value of the GH_TOKEN environment variable is being used for authentication.");
        assert_eq!(login_failure(&[]), "gh stopped without signing in.");
    }

    #[test]
    fn the_scopes_a_token_lacks_are_named_with_what_needs_them() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let names = |m: Vec<(&str, &str)>| m.into_iter().map(|(n, _)| n.to_string()).collect::<Vec<_>>();
        assert_eq!(names(missing_scopes(&s(&["write:public_key", "repo"]), false)), Vec::<String>::new());
        assert_eq!(names(missing_scopes(&s(&["read:public_key", "repo"]), false)), vec!["admin:public_key"]);
        assert_eq!(names(missing_scopes(&s(&["admin:public_key", "repo", "admin:ssh_signing_key"]), true)), Vec::<String>::new());
        assert_eq!(names(missing_scopes(&s(&["admin:public_key", "repo"]), true)), vec!["write:ssh_signing_key"]);
        assert_eq!(names(missing_scopes(&s(&[]), false)), vec!["admin:public_key", "repo"]);
    }
}
