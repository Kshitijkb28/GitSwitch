//! Publishing a repository made here, without leaving GitSwitch: a new,
//! empty repository on GitHub, created through the API as one of GitSwitch's
//! accounts, connected as the remote the push goes to, ready to publish. The
//! browser is only a fallback, for when GitHub refuses the app.

use crate::error::AppError;
use crate::gh_api::{check_new_repo_name, check_username, login_of, Gh};
use crate::git_ops::{self, Intent, StatusFacts};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// An account that can create the repository.
#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct PublishAccount {
    /// The key commands take: "gh:<login>" or "signin:<login>".
    pub account: String,
    pub login: String,
    pub source: String,
}

/// Where the new repository would be connected.
#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct PublishTarget {
    /// "add": no remote yet, origin is added. "change": the push goes to a
    /// remote nothing was ever fetched from or pushed to, which gets the new
    /// address. "none": the push goes to a remote that has been reached, so
    /// publishing goes there and nothing is created.
    pub kind: String,
    pub remote: Option<String>,
    /// That remote's address, as the status shows it (tokens masked).
    pub url: Option<String>,
    /// "owner/name" when that address is a GitHub repository's.
    pub repo: Option<String>,
}

/// What the "Create on GitHub" form starts from.
#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct PublishContext {
    /// One per login, GitHub CLI logins first.
    pub accounts: Vec<PublishAccount>,
    /// The account this folder pushes as, when GitSwitch has it.
    pub suggested: Option<String>,
    /// The GitHub login this folder pushes as: its profile's account, else its key's.
    pub push_login: Option<String>,
    pub target: PublishTarget,
    /// The folder's name, made into a name GitHub takes as it is.
    pub default_name: String,
    /// The branch that will be published: an empty repository's default branch.
    pub branch: Option<String>,
    /// Why this folder can't be published right now, before anything is created.
    pub blocked: Option<String>,
}

/// The folder's name as GitHub would take it: anything but letters, digits,
/// '_' and '.' becomes '-', runs of '-' become one, none at the ends, no
/// ".git" at the end, at most 100 characters.
pub(crate) fn repo_name_from(folder: &str) -> String {
    let mut out = String::new();
    for c in folder.chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '_' | '.') { c } else { '-' };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let mut name = out.trim_matches('-').to_string();
    while name.to_ascii_lowercase().ends_with(".git") {
        name.truncate(name.len() - 4);
        name = name.trim_end_matches('-').to_string();
    }
    let name: String = name.chars().take(100).collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() || name == "." || name == ".." {
        "repository".to_string()
    } else {
        name
    }
}

fn folder_name(repo_path: &str) -> String {
    std::path::Path::new(repo_path.trim_end_matches(['/', '\\']))
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Where a push from this folder would go, and whether a new repository may
/// be connected there.
async fn target(repo_path: &str, status: &crate::git_status::RepoStatus) -> PublishTarget {
    if status.push.remotes.is_empty() {
        return PublishTarget { kind: "add".into(), remote: Some("origin".into()), ..Default::default() };
    }
    let Some(branch) = status.branch.as_deref() else {
        return PublishTarget { kind: "none".into(), ..Default::default() };
    };
    let inputs = git_ops::push_inputs(repo_path, status, branch, true).await;
    let Ok(t) = git_ops::resolve_push_target(&inputs) else {
        return PublishTarget { kind: "none".into(), ..Default::default() };
    };
    let url = status.push.remotes.iter().find(|r| r.name == t.remote).map(|r| r.fetch_url.clone());
    let repo = match url.as_deref() {
        Some(u) => crate::peek::github_slug_resolved(u).await.map(|(o, n)| format!("{}/{}", o, n)),
        None => None,
    };
    let kind = if git_ops::never_reached(repo_path, &t.remote).await { "change" } else { "none" };
    PublishTarget { kind: kind.into(), remote: Some(t.remote), url, repo }
}

/// What the form starts from: the accounts, the one this folder pushes as,
/// where the repository would be connected, and a name for it.
pub async fn context(repo_path: &str) -> Result<PublishContext, AppError> {
    let status = git_ops::snapshot(repo_path).await?;
    let target = target(repo_path, &status).await;
    let push_login = Box::pin(crate::gh_repo::profile_login(repo_path)).await;
    let mut seen = HashSet::new();
    let accounts: Vec<PublishAccount> = crate::remote_repos::accounts()
        .await
        .into_iter()
        .filter(|a| seen.insert(login_of(&a.key).to_lowercase()))
        .map(|a| PublishAccount { login: login_of(&a.key).to_string(), account: a.key, source: a.source })
        .collect();
    let suggested = push_login
        .as_deref()
        .and_then(|l| accounts.iter().find(|a| a.login.eq_ignore_ascii_case(l)))
        .or(if accounts.len() == 1 { accounts.first() } else { None })
        .map(|a| a.account.clone());
    Ok(PublishContext {
        blocked: git_ops::check(&Intent::Push, &StatusFacts::from(&status)).map(|r| r.message),
        branch: status.branch.clone(),
        default_name: repo_name_from(&folder_name(repo_path)),
        accounts,
        suggested,
        push_login,
        target,
    })
}

/// Where the person chose to connect it, as the dialog saw it.
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TargetArg {
    /// No remote yet: add origin.
    Add,
    /// The remote the push goes to, never reached, with the address it had.
    Change { remote: String, from: String },
}

/// What happened.
#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct Published {
    /// What became of the remote: "added", "changed", "kept" (it already
    /// pointed there) or "no" (the repository exists but isn't connected).
    pub saved: String,
    pub full_name: String,
    pub html_url: String,
    pub private: bool,
    pub renamed: bool,
    pub remote: String,
    /// The address saved, or the one to connect by hand.
    pub url: String,
    /// What still stands between the repository and the publish, in words.
    /// Empty when it is ready.
    pub problem: String,
}

fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// One creation per folder at a time: a second click is refused, never sent.
struct InFlight(String);

impl InFlight {
    fn claim(repo_path: &str) -> Result<InFlight, AppError> {
        let key = std::fs::canonicalize(repo_path).map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_else(|_| repo_path.to_lowercase());
        let mut set = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        if !set.insert(key.clone()) {
            return Err(AppError::Command("GitSwitch is already creating a repository for this folder.".into()));
        }
        Ok(InFlight(key))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        in_flight().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// An error's words, without the kind in front ("Command execution error: …").
fn words(e: &AppError) -> String {
    match e {
        AppError::Config(s) | AppError::NotFound(s) | AppError::Ssh(s) | AppError::Keyring(s) | AppError::Command(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The remote is still what the dialog saw: none at all (add), or the
/// never-reached remote with the address shown (change, whose current
/// address comes back).
async fn guard(repo_path: &str, target: &TargetArg) -> Result<Option<String>, AppError> {
    match target {
        TargetArg::Add => git_ops::add_origin_guard(repo_path).await.map(|_| None),
        TargetArg::Change { remote, from } => git_ops::change_remote_guard(repo_path, remote, from).await.map(Some),
    }
}

/// The address to save: the remote's own when it already points at the new
/// repository; its form (host alias, certificate user, port) pointed there
/// when it is a GitHub address; else GitHub's SSH address, or for an
/// organization this computer reaches with SSH certificates, that form (as
/// Clone does). The bool: kept as it was.
async fn address_for(made: &crate::gh_repo::CreatedRepo, current: Option<&str>, own: bool) -> (String, bool) {
    if let Some(cur) = current {
        if let Some((o, n)) = crate::peek::github_slug_resolved(cur).await {
            if o.eq_ignore_ascii_case(&made.owner) && n.eq_ignore_ascii_case(&made.name) {
                return (cur.to_string(), true);
            }
            if let Some(u) = git_ops::retarget_url(cur, &made.owner, &made.name).filter(|u| git_ops::remote_url_problem(u).is_none()) {
                return (u, false);
            }
        }
    }
    if !own {
        let profs = crate::profiles::load_profiles().map(|s| s.profiles).unwrap_or_default();
        let idx = Box::pin(crate::remote_repos::local_index_stale_ok(&profs)).await;
        return (crate::remote_repos::clone_link(&made.owner, &made.name, &made.ssh_url, &idx.org_ssh_users), false);
    }
    (made.ssh_url.clone(), false)
}

/// Create the repository on GitHub as `account` and connect it, ready to
/// publish. Nothing is created for a folder that couldn't be published, or
/// whose remote changed since the dialog read it; once the repository
/// exists, the answer says what happened rather than failing.
pub async fn create_and_connect(repo_path: &str, account: &str, owner: &str, name: &str, description: &str, private: bool, target: TargetArg) -> Result<Published, AppError> {
    check_username(owner)?;
    check_new_repo_name(name)?;
    let status = git_ops::snapshot(repo_path).await?;
    if let Some(r) = git_ops::check(&Intent::Push, &StatusFacts::from(&status)) {
        return Err(AppError::Command(r.message));
    }
    let _turn = InFlight::claim(repo_path)?;
    guard(repo_path, &target).await?;
    let gh = Gh::for_account(account).await?;
    let made = Box::pin(crate::gh_repo::create(&gh, owner, name, description, private)).await?;
    let remote = match &target {
        TargetArg::Add => "origin".to_string(),
        TargetArg::Change { remote, .. } => remote.clone(),
    };
    let mut out = Published {
        saved: "no".into(),
        full_name: made.full_name.clone(),
        html_url: made.html_url.clone(),
        private: made.private,
        renamed: made.renamed,
        remote,
        url: made.ssh_url.clone(),
        problem: String::new(),
    };
    let not_connected = |why: String| format!("{} was created on GitHub, but it wasn't connected: {} Connect it with \"Use an address\".", made.full_name, why);
    let current = match guard(repo_path, &target).await {
        Ok(c) => c,
        Err(e) => {
            out.problem = not_connected(words(&e));
            return Ok(out);
        }
    };
    let (url, kept) = address_for(&made, current.as_deref(), owner.eq_ignore_ascii_case(&gh.login)).await;
    out.url = url.clone();
    if let Some(p) = git_ops::remote_url_problem(&url) {
        out.problem = not_connected(p);
        return Ok(out);
    }
    let saved = if kept {
        Ok("kept")
    } else {
        match &target {
            TargetArg::Add => git_ops::save_origin(repo_path, &url).await.map(|_| "added"),
            TargetArg::Change { remote, .. } => git_ops::save_remote_url(repo_path, remote, &url).await.map(|_| "changed"),
        }
    };
    match saved {
        Ok(s) => out.saved = s.to_string(),
        Err(e) => {
            out.problem = not_connected(words(&e));
            return Ok(out);
        }
    }
    if let Err(why) = Box::pin(git_ops::wait_reachable(repo_path, &url)).await {
        out.problem = format!("{} was created on GitHub and connected, but git can't reach it yet: {} Then press Publish branch again.", made.full_name, why);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_name_becomes_a_name_github_takes_as_it_is() {
        assert_eq!(repo_name_from("SubtitleMaker"), "SubtitleMaker");
        assert_eq!(repo_name_from("Angular Assesment"), "Angular-Assesment");
        assert_eq!(repo_name_from("my  app!!"), "my-app");
        assert_eq!(repo_name_from("site.git"), "site");
        assert_eq!(repo_name_from("--x--"), "x");
        assert_eq!(repo_name_from("ñandú"), "and");
        assert_eq!(repo_name_from("!!!"), "repository");
        assert_eq!(repo_name_from(".."), "repository");
        assert!(check_new_repo_name(&repo_name_from(&"x y".repeat(80))).is_ok());
    }

    #[test]
    fn one_creation_per_folder_at_a_time() {
        let d = std::env::temp_dir().to_string_lossy().to_string();
        let first = InFlight::claim(&d).unwrap();
        assert!(InFlight::claim(&d).is_err());
        drop(first);
        assert!(InFlight::claim(&d).is_ok());
    }

    fn made(owner: &str, name: &str) -> crate::gh_repo::CreatedRepo {
        crate::gh_repo::CreatedRepo {
            owner: owner.into(),
            name: name.into(),
            full_name: format!("{}/{}", owner, name),
            ssh_url: format!("git@github.com:{}/{}.git", owner, name),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn the_address_saved_keeps_what_already_works() {
        let m = made("me", "app");
        assert_eq!(address_for(&m, Some("git@github.com:me/app.git"), true).await, ("git@github.com:me/app.git".to_string(), true));
        assert_eq!(address_for(&m, Some("git@github.com:Me/App.git"), true).await, ("git@github.com:Me/App.git".to_string(), true));
        assert_eq!(address_for(&m, Some("git@github.com:me/typo.git"), true).await, ("git@github.com:me/app.git".to_string(), false));
        assert_eq!(address_for(&m, Some("https://github.com/me/typo"), true).await, ("https://github.com/me/app.git".to_string(), false));
        assert_eq!(address_for(&m, Some("C:/repos/typo.git"), true).await, ("git@github.com:me/app.git".to_string(), false));
        assert_eq!(address_for(&m, None, true).await, ("git@github.com:me/app.git".to_string(), false));
        let org = made("Acme", "tool");
        assert_eq!(address_for(&org, Some("org-123@github.com:Acme/old.git"), false).await, ("org-123@github.com:Acme/tool.git".to_string(), false));
    }

    #[test]
    fn the_target_is_read_as_the_dialog_sends_it() {
        let add: TargetArg = serde_json::from_str(r#"{"kind":"add"}"#).unwrap();
        assert_eq!(add, TargetArg::Add);
        let change: TargetArg = serde_json::from_str(r#"{"kind":"change","remote":"origin","from":"git@github.com:me/x.git"}"#).unwrap();
        assert_eq!(change, TargetArg::Change { remote: "origin".into(), from: "git@github.com:me/x.git".into() });
        assert!(serde_json::from_str::<TargetArg>(r#"{"kind":"delete"}"#).is_err());
    }
}
