use crate::error::AppError;
use serde::{Deserialize, Serialize};
use crate::profiles::{self, Profile};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct SparseInfo {
    pub path: String,
    pub branch: String,
    pub is_sparse: bool,
    /// Folders currently in the sparse-checkout set (git sparse-checkout list).
    pub sparse_dirs: Vec<String>,
    /// Top-level folders that exist in the repo (git ls-tree -d).
    pub available_dirs: Vec<String>,
}

// ---------------------------------------------------------------------------
// Clone progress and size
// ---------------------------------------------------------------------------

/// One line of `git clone --progress`, as the page shows it.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CloneProgress {
    /// "Counting objects", "Receiving objects", "Resolving deltas", "Updating files", "Submodules", "Large files"…
    pub phase: String,
    pub percent: Option<u8>,
    pub done: Option<u64>,
    pub total: Option<u64>,
    /// "10.00 MiB" — what has arrived so far, for the receiving phase.
    pub bytes: Option<String>,
    /// "2.00 MiB/s"
    pub rate: Option<String>,
    /// What is being worked on right now — the submodule being fetched, the
    /// large file being downloaded. Left out of the JSON when there is none,
    /// so a reader written before this field existed still parses the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// What the host says the repository being downloaded weighs, in
    /// kilobytes — "10.00 MiB of about 1.2 GB". Only when the host says, and
    /// only while what has arrived is still below it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_kb: Option<u64>,
    /// Seconds left at the current rate: against the host's size when it is
    /// known, otherwise against the share of objects that has arrived. Only
    /// for a download that reports bytes and a rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
}

/// `done` out of `total`, as a whole percentage; `None` with no total to
/// divide by. Capped at 100 so a count that overshoots never shows 112%.
fn percent_of(done: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    Some((done.min(total) * 100 / total) as u8)
}

impl CloneProgress {
    fn phase(name: &str) -> Self {
        CloneProgress { phase: name.to_string(), percent: None, done: None, total: None, bytes: None, rate: None, detail: None, estimate_kb: None, eta_secs: None }
    }

    /// `done` submodules checked out of `total`; `current` is the one being
    /// fetched now, `None` once the loop is through.
    fn submodules(done: usize, total: usize, current: Option<&str>) -> Self {
        CloneProgress {
            percent: percent_of(done as u64, total as u64),
            done: Some(done as u64),
            total: Some(total as u64),
            detail: current.map(str::to_string),
            ..CloneProgress::phase("Submodules")
        }
    }

    /// What git-lfs itself reported: the file it is on, out of how many, and
    /// the bytes so far — relayed, not reinterpreted.
    fn large_files(p: &crate::lfs::LfsProgress) -> Self {
        CloneProgress {
            percent: percent_of(p.done as u64, p.total as u64),
            done: Some(p.done as u64),
            total: Some(p.total as u64),
            bytes: Some(format!("{} of {}", human_bytes(p.bytes), human_bytes(p.total_bytes))),
            detail: Some(p.file.clone()),
            ..CloneProgress::phase("Large files")
        }
    }

    /// The large-files phase at its two measured ends: nothing downloaded yet
    /// out of the pointer stubs counted, and afterwards how many arrived.
    fn large_files_counted(done: usize, total: usize) -> Self {
        CloneProgress {
            percent: percent_of(done as u64, total as u64),
            done: Some(done as u64),
            total: Some(total as u64),
            ..CloneProgress::phase("Large files")
        }
    }
}

/// "1.0 MiB", as git prints sizes.
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", n)
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

/// `Receiving objects:  45% (556/1234), 10.00 MiB | 2.00 MiB/s` and its
/// siblings (`remote: Compressing objects: …`, `Resolving deltas: …, done.`).
pub fn parse_clone_progress(line: &str) -> Option<CloneProgress> {
    const PHASES: &[&str] = &["Enumerating objects", "Counting objects", "Compressing objects", "Receiving objects", "Resolving deltas", "Updating files", "Filtering content", "Checking out files"];
    let line = line.trim().trim_start_matches("remote: ").trim();
    let phase = PHASES.iter().find(|p| line.starts_with(*p))?;
    let rest = line[phase.len()..].trim_start_matches(':').trim();
    let mut p = CloneProgress::phase(phase);
    // "45% (556/1234), 10.00 MiB | 2.00 MiB/s, done." — pieces separated by commas.
    for (i, piece) in rest.split(',').enumerate() {
        let piece = piece.trim();
        if i == 0 {
            if let Some((pct, counts)) = piece.split_once('%') {
                p.percent = pct.trim().parse().ok();
                let counts = counts.trim().trim_start_matches('(').trim_end_matches(')');
                if let Some((d, t)) = counts.split_once('/') {
                    p.done = d.trim().parse().ok();
                    p.total = t.trim().parse().ok();
                }
            } else if let Some(n) = piece.split_whitespace().next().and_then(|n| n.parse().ok()) {
                // "Enumerating objects: 1234, done."
                p.total = Some(n);
            }
        } else if piece.contains("iB") {
            let (bytes, rate) = piece.split_once('|').unwrap_or((piece, ""));
            p.bytes = Some(bytes.trim().to_string()).filter(|s| !s.is_empty());
            p.rate = Some(rate.trim().to_string()).filter(|s| !s.is_empty());
        }
    }
    Some(p)
}

/// "10.00 MiB" / "2.00 MiB/s", as git prints them, in bytes.
fn git_size_bytes(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches("/s").trim();
    let (number, unit) = s.split_once(' ')?;
    let n: f64 = number.trim().parse().ok()?;
    let scale = match unit.trim() {
        "bytes" | "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    (n >= 0.0).then_some(n * scale)
}

/// How long a download has left, and the size to show beside what arrived.
///
/// With the host's size for the repository (`estimate_kb`), the rest of it at
/// the current rate. Without one — any host that does not say, or a
/// repository nobody signed in here may ask about — the share of objects that
/// has arrived stands in for the share of bytes; that is rough (history comes
/// first and is small), so it is only used once a twentieth has arrived.
/// A size the download has already passed was wrong: it is dropped, not shown.
/// Returns (estimate to show, seconds left).
pub(crate) fn download_estimate(bytes: Option<&str>, rate: Option<&str>, percent: Option<u8>, estimate_kb: Option<u64>) -> (Option<u64>, Option<u64>) {
    let Some(have) = bytes.and_then(git_size_bytes) else { return (None, None) };
    let estimate = estimate_kb.filter(|kb| (*kb as f64) * 1024.0 > have);
    let Some(per_sec) = rate.and_then(git_size_bytes).filter(|r| *r > 0.0) else { return (estimate, None) };
    let left = match estimate {
        Some(kb) => Some((kb as f64) * 1024.0 - have),
        None => percent.filter(|p| (5..100).contains(p)).map(|p| have * (100.0 - p as f64) / p as f64),
    };
    (estimate, left.map(|bytes_left| (bytes_left / per_sec).round() as u64))
}

/// The size the host reports and the time left, added to a line of a
/// download in flight. Other phases (counting, deltas, checkout) pass through.
fn with_estimate(mut p: CloneProgress, estimate_kb: Option<u64>) -> CloneProgress {
    if p.phase == "Receiving objects" {
        let (estimate, eta) = download_estimate(p.bytes.as_deref(), p.rate.as_deref(), p.percent, estimate_kb);
        p.estimate_kb = estimate;
        p.eta_secs = eta;
    }
    p
}

/// A cell the host's answer lands in while the download is already running:
/// asking must never hold the download up.
type EstimateCell = std::sync::Arc<std::sync::Mutex<Option<u64>>>;

fn ask_size(url: &str, accounts: Option<Vec<String>>) -> (EstimateCell, Option<tokio::task::JoinHandle<()>>) {
    let cell: EstimateCell = Default::default();
    // Only a host that can be asked — GitHub, by its name or through an SSH
    // alias ssh resolves inside the task: for every other address there is
    // nothing to wait for, and nothing leaves this computer.
    if crate::peek::github_slug(url).is_none() && crate::ssh_config::ssh_host_of_url(url).is_none() {
        return (cell, None);
    }
    let (into, url) = (cell.clone(), url.to_string());
    let task = tokio::spawn(async move {
        if let Some(e) = size_estimate(&url, accounts.as_deref()).await {
            if let Ok(mut c) = into.lock() {
                *c = Some(e.kb);
            }
        }
    });
    (cell, Some(task))
}

type RunningClones = std::collections::HashMap<String, String>;
fn running_clones() -> &'static std::sync::Mutex<RunningClones> {
    static R: std::sync::OnceLock<std::sync::Mutex<RunningClones>> = std::sync::OnceLock::new();
    R.get_or_init(Default::default)
}

/// While a clone writes into its folder — git, then its submodules, then its
/// large files — the folder is not a repository yet, though git wrote `.git`
/// and an origin before downloading anything. Held for the whole clone;
/// dropped however it ends.
pub(crate) struct CloneGuard(Vec<String>);

impl CloneGuard {
    pub(crate) fn hold(dest: &Path, url: &str) -> Self {
        let mut keys = vec![crate::paths::norm(&dest.to_string_lossy())];
        // And as the disk spells it (the folder itself may not exist yet).
        if let (Some(parent), Some(name)) = (dest.parent(), dest.file_name()) {
            if let Some(r) = crate::paths::real_form(&parent.to_string_lossy()) {
                keys.push(crate::paths::norm(&format!("{}/{}", r, name.to_string_lossy())));
            }
        }
        keys.dedup();
        if let Ok(mut m) = running_clones().lock() {
            for k in &keys {
                m.insert(k.clone(), url.to_string());
            }
        }
        CloneGuard(keys)
    }
}

impl Drop for CloneGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = running_clones().lock() {
            for k in &self.0 {
                m.remove(k);
            }
        }
    }
}

/// Is a clone still writing this folder?
pub(crate) fn clone_running_at(path: &str) -> bool {
    let Ok(m) = running_clones().lock() else { return false };
    if m.is_empty() {
        return false;
    }
    let mut keys = vec![crate::paths::norm(path)];
    if let Some(r) = crate::paths::real_form(path) {
        keys.push(crate::paths::norm(&r));
    }
    keys.iter().any(|k| m.contains_key(k))
}

/// Every clone still running: (destination, address).
pub(crate) fn running_clone_list() -> Vec<(String, String)> {
    running_clones().lock().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
}

type ProgressMap = std::collections::HashMap<String, CloneProgress>;
fn progress_map() -> &'static std::sync::Mutex<ProgressMap> {
    static M: std::sync::OnceLock<std::sync::Mutex<ProgressMap>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn set_progress(dest: &str, p: CloneProgress) {
    #[cfg(test)]
    if let Ok(mut t) = progress_trace().lock() {
        t.push(p.clone());
    }
    if let Ok(mut m) = progress_map().lock() {
        m.insert(crate::paths::norm(dest), p);
    }
}

/// Every progress entry set in this process, in order — the verification
/// suites read it back through the probe to prove the counts were set, since
/// against a local remote a phase lasts milliseconds and a poller would miss
/// it. Test binary only.
#[cfg(test)]
pub(crate) fn progress_trace() -> &'static std::sync::Mutex<Vec<CloneProgress>> {
    static T: std::sync::OnceLock<std::sync::Mutex<Vec<CloneProgress>>> = std::sync::OnceLock::new();
    T.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// How often the LFS progress file is read while a download runs: half the
/// page's own poll, and only a file read.
const LFS_PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// Relay what `git lfs pull` writes to its progress file into the clone's
/// progress entry until the returned task is aborted. Starts the phase at
/// "0 of `total`" so the line changes the moment the download begins.
fn relay_lfs_progress(dest: &str, repo: &str, total: usize) -> tokio::task::JoinHandle<()> {
    set_progress(dest, CloneProgress::large_files_counted(0, total));
    let dest = dest.to_string();
    let repo = repo.to_string();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(LFS_PROGRESS_EVERY).await;
            if let Some(p) = crate::lfs::read_progress(&repo) {
                set_progress(&dest, CloneProgress::large_files(&p));
            }
        }
    })
}

/// Removes the entry when the clone or download is over, however it ended —
/// so the next one does not open on this one's last line.
pub(crate) struct ProgressGuard(String);

impl ProgressGuard {
    /// Hold the entry for `dest` (a path, normalised as the poll reads it).
    pub(crate) fn hold(dest: &str) -> Self {
        ProgressGuard(crate::paths::norm(dest))
    }
}
impl Drop for ProgressGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = progress_map().lock() {
            m.remove(&self.0);
        }
    }
}

pub fn clone_progress(dest: &str) -> Option<CloneProgress> {
    progress_map().lock().ok().and_then(|m| m.get(&crate::paths::norm(dest)).cloned())
}

/// How long a submodule download may report nothing before it is stopped.
/// Long: a server packing a large repository is quiet for a while before
/// the first byte comes. A download that is merely slow keeps reporting, and
/// is never cut short for taking long.
const SUBMODULE_STALL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

fn submodule_stall() -> std::time::Duration {
    // The suites prove the stop with a stuck server; waiting ten minutes for
    // it would prove the same thing slowly.
    #[cfg(test)]
    if let Some(secs) = std::env::var("GITSWITCH_STALL_SECS").ok().and_then(|v| v.parse::<u64>().ok()) {
        return std::time::Duration::from_secs(secs);
    }
    SUBMODULE_STALL
}

/// `git … --progress` with stderr read as it comes: each progress line git
/// writes (with `\r` between updates, and only when asked, since there is no
/// terminal) goes to `on_progress` for the page's bar. With `stall` set, a
/// command that reports nothing for that long is stopped, its whole process
/// tree with it — never one that is still reporting, however long it takes.
/// `cfg` are `-c key=value` pairs for this one command. `armed`: the stall
/// clock runs from the start (a command that may sit on the network before it
/// says anything) — otherwise only from the first "Cloning into".
async fn run_git_streaming(
    repo: Option<&Path>,
    cfg: &[String],
    args: &[&str],
    stall: Option<std::time::Duration>,
    armed: bool,
    on_progress: &(dyn Fn(CloneProgress) + Sync),
) -> Result<String, AppError> {
    use tokio::io::AsyncReadExt;
    // The one runner's arguments and environment, its whole process tree
    // killable (a stalled download's helpers go with it); only the clock and
    // the reading of progress are this function's own.
    let mut git = match repo {
        Some(dir) => crate::git_exec::GitCmd::at(dir),
        None => crate::git_exec::GitCmd::without_repo(),
    };
    for kv in cfg {
        git = git.cfg_raw(kv);
    }
    let git = git.args(args.iter().copied()).settled().await?;
    // As `run` does: what was read of this folder while a write ran (a
    // half-downloaded submodule) is forgotten once it is over.
    let _forget = git.forget_when_done();
    let (mut cmd, argv) = git.prepare();
    let started = std::time::Instant::now();
    let mut child = cmd.spawn().map_err(|e| AppError::Command(format!("Failed to run git: {}", e)))?;
    #[cfg(windows)]
    let job = crate::spawn::JobObject::adopt(&child);
    let stderr = child.stderr.take();
    let stdout = child.stdout.take();
    // Every line as it arrives, for the bar and for the stall clock; the task
    // keeps one copy of each distinct line for the error message and the log.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let err_task = tokio::spawn(async move {
        let mut full = String::new();
        let Some(mut pipe) = stderr else { return full };
        let mut buf = [0u8; 4096];
        let mut acc: Vec<u8> = Vec::new();
        loop {
            let n = match pipe.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                if b == b'\r' || b == b'\n' {
                    let line = String::from_utf8_lossy(&acc).to_string();
                    acc.clear();
                    if line.trim().is_empty() {
                        continue;
                    }
                    if !full.ends_with(&format!("{}\n", line)) {
                        full.push_str(&line);
                        full.push('\n');
                    }
                    let _ = tx.send(line);
                } else {
                    acc.push(b);
                }
            }
        }
        if !acc.is_empty() {
            full.push_str(&String::from_utf8_lossy(&acc));
        }
        full
    });
    let out_task = tokio::spawn(async move {
        let mut s = String::new();
        if let Some(mut pipe) = stdout {
            let _ = pipe.read_to_string(&mut s).await;
        }
        s
    });
    let mut last_word = std::time::Instant::now();
    let mut last_line = String::new();
    let mut lines_open = true;
    let mut stalled = false;
    // The stall clock runs only while a transfer is in flight: from
    // "Cloning into" (printed before the transport connects, so a server
    // that never answers is still caught) to git's own "done." — what
    // follows is local work that says nothing while it works (a long
    // checkout, a smudge filter downloading large files) and must never
    // be called a stall. A nested submodule's next "Cloning into" re-arms.
    let mut transferring = stall.is_some() && armed;
    let status = loop {
        tokio::select! {
            s = child.wait() => break Some(s),
            line = rx.recv(), if lines_open => match line {
                Some(l) => {
                    last_word = std::time::Instant::now();
                    let t = l.trim();
                    if t.starts_with("Cloning into") {
                        transferring = true;
                    } else if !t.starts_with("remote:") && t.ends_with("done.") {
                        transferring = false;
                    }
                    if let Some(p) = parse_clone_progress(&l) {
                        on_progress(p);
                    }
                    last_line = l;
                }
                None => lines_open = false,
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)), if stall.is_some() => {
                if transferring && last_word.elapsed() > stall.unwrap_or_default() {
                    stalled = true;
                    break None;
                }
            }
        }
    };
    let sub = args.iter().find(|a| !a.starts_with('-') && !a.contains('=')).copied().unwrap_or("clone");
    if stalled {
        #[cfg(windows)]
        if let Some(j) = &job {
            j.terminate();
        }
        crate::git_exec::terminate_tree(&mut child).await;
        let stderr_text = tokio::time::timeout(std::time::Duration::from_secs(2), err_task).await.ok().and_then(|r| r.ok()).unwrap_or_default();
        let secs = stall.unwrap_or_default().as_secs();
        crate::cmdlog::record(
            repo.unwrap_or(Path::new(".")),
            argv,
            -9,
            started.elapsed(),
            "",
            &format!("stopped: no progress for {}s\n{}", secs, stderr_text),
        );
        let far = if last_line.is_empty() { String::new() } else { format!(" It had got as far as: {}", last_line.trim()) };
        return Err(AppError::Command(format!(
            "git {} made no progress for {}s and was stopped — the remote may be unreachable, or the connection very slow.{}",
            sub, secs, far
        )));
    }
    let status = status.unwrap_or_else(|| Err(std::io::Error::other("stopped")));
    let status = status.map_err(|e| AppError::Command(format!("git {} failed: {}", sub, e)))?;
    let stderr_text = err_task.await.unwrap_or_default();
    let stdout_text = out_task.await.unwrap_or_default();
    crate::cmdlog::record(
        repo.unwrap_or(Path::new(".")),
        argv,
        status.code().unwrap_or(-1),
        started.elapsed(),
        &stdout_text,
        &stderr_text,
    );
    if !status.success() {
        return Err(AppError::Command(format!("git {} failed: {}", sub, stderr_text.trim())));
    }
    Ok(stdout_text.trim().to_string())
}

/// What GitHub reports as the repository's size, before a clone.
#[derive(Debug, Serialize, Clone)]
pub struct CloneSizeEstimate {
    pub repo: String,
    /// GitHub's `size` field: kilobytes of the repository's git data, which is
    /// what a clone downloads. Large files kept in LFS are not included.
    pub kb: u64,
}

/// Ask GitHub how big `url`'s repository is: with each signed-in account's
/// token first (private repositories), then anonymously. None when the URL is
/// not GitHub's, or GitHub will not say.
pub async fn clone_size_estimate(url: &str) -> Option<CloneSizeEstimate> {
    size_estimate(url, None).await
}

/// The account keys that may look up a repository for a profile: its own
/// GitSwitch sign-in, and its GitHub login in the gh CLI. None for no profile.
fn account_keys(profile_id: &str, github_login: Option<&str>) -> Vec<String> {
    let mut keys = vec![format!("profile:{}", profile_id)];
    if let Some(login) = github_login.filter(|l| !l.trim().is_empty()) {
        keys.push(format!("gh:{}", login.trim()));
    }
    keys
}

/// The accounts a profile acts as, by its id.
fn accounts_of(profile_id: Option<&str>) -> Vec<String> {
    let Some(id) = profile_id else { return Vec::new() };
    let login = profiles::load_profiles().ok().and_then(|st| st.profiles.into_iter().find(|p| p.id == id)).and_then(|p| p.github_login);
    account_keys(id, login.as_deref())
}

/// `accounts`: `None` tries every signed-in account (the address the person
/// typed on the Clone page, before choosing whom to clone as); `Some(keys)`
/// only those — the account a folder acts as — so one account's token never
/// looks up a repository on another's behalf. Anonymously after either.
async fn size_estimate(url: &str, accounts: Option<&[String]>) -> Option<CloneSizeEstimate> {
    let (owner, repo) = crate::peek::github_slug_resolved(url).await?;
    // The page asks when the address is typed and the clone asks again a
    // moment later: the answer is kept for a few minutes, not asked twice.
    static KNOWN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, u64)>>> = std::sync::OnceLock::new();
    let slug = format!("{}/{}", owner, repo).to_lowercase();
    let known = KNOWN.get_or_init(Default::default);
    if let Some((at, kb)) = known.lock().ok().and_then(|m| m.get(&slug).copied()) {
        if at.elapsed() < std::time::Duration::from_secs(600) {
            return Some(CloneSizeEstimate { repo: format!("{}/{}", owner, repo), kb });
        }
    }
    let client = crate::github::http_client().ok()?;
    let api = format!("{}/repos/{}/{}", crate::remote_repos::api_base(), owner, repo);
    let mut tokens: Vec<Option<String>> = Vec::new();
    let keys: Vec<String> = match accounts {
        None => crate::remote_repos::accounts().await.into_iter().map(|a| a.key).collect(),
        Some(k) => k.to_vec(),
    };
    for key in keys {
        if let Ok(t) = crate::remote_repos::token_for(&key).await {
            tokens.push(Some(t));
        }
    }
    tokens.push(None);
    for token in tokens {
        let mut req = client.get(&api).header("User-Agent", "GitSwitch").header("Accept", "application/vnd.github+json");
        if let Some(t) = &token {
            req = req.header("Authorization", format!("Bearer {}", t));
        }
        let Ok(resp) = req.timeout(std::time::Duration::from_secs(6)).send().await else { continue };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(v) = resp.json::<serde_json::Value>().await else { continue };
        if let Some(kb) = v.get("size").and_then(|s| s.as_u64()) {
            if let Ok(mut m) = known.lock() {
                m.insert(slug.clone(), (std::time::Instant::now(), kb));
            }
            return Some(CloneSizeEstimate { repo: format!("{}/{}", owner, repo), kb });
        }
    }
    None
}

async fn run_git(repo: Option<&Path>, args: &[&str]) -> Result<String, AppError> {
    // The one runner: no prompt can hang it (no terminal exists for a GUI
    // app), and it has a clock. Materialising a work tree can be a download
    // — a partial clone fetches the blobs it now needs, Git LFS's smudge
    // filter the large files — so `sparse-checkout` and `checkout` get the
    // large-file ceiling, like the clone they finish; the rest is local.
    let cmd = match repo {
        Some(dir) => crate::git_exec::GitCmd::at(dir),
        None => crate::git_exec::GitCmd::without_repo(),
    };
    let timeout = match args.first() {
        Some(&"sparse-checkout") | Some(&"checkout") => crate::git_exec::LFS_TIMEOUT,
        _ => crate::git_exec::LOCAL_TIMEOUT,
    };
    // Boxed: this is awaited deep in the submodule download chain, and the
    // runner's state (child, pipes, clock, tree kill) inline in that chain
    // is the shape that has overflowed a 2 MiB stack before.
    let out = Box::pin(cmd.args(args.iter().copied()).timeout(timeout).run()).await?;
    if !out.ok() {
        return Err(AppError::Command(format!("git {} failed: {}", args.first().unwrap_or(&""), out.stderr)));
    }
    Ok(out.trimmed())
}

/// Derive the repo folder name from a clone URL (strip .git, take last segment).
fn repo_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let last = trimmed.rsplit(['/', ':']).next()?;
    if last.is_empty() {
        None
    } else {
        Some(last.to_string())
    }
}

/// Where a clone of `url` would land: (parent dir, folder name, full path).
fn destination_path(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
) -> Result<(PathBuf, String, PathBuf), AppError> {
    let parent = PathBuf::from(parent_dir);
    if !parent.is_dir() {
        return Err(AppError::Config(format!("Folder not found: {}", parent_dir)));
    }
    let name = match folder_name {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => repo_name_from_url(url)
            .ok_or_else(|| AppError::Config("Could not derive a folder name from the URL".into()))?,
    };
    // A plain folder name only — never a path that escapes the chosen parent.
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(AppError::Config(format!("\"{}\" isn't a valid folder name", name)));
    }
    let dest = parent.join(&name);
    Ok((parent, name, dest))
}

/// Resolve and validate where a clone will land — it must not exist yet.
fn clone_destination(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
) -> Result<(PathBuf, String, PathBuf), AppError> {
    let (parent, name, dest) = destination_path(url, parent_dir, folder_name)?;
    if dest.exists() {
        return Err(AppError::Config(format!("{} already exists", dest.display())));
    }
    Ok((parent, name, dest))
}

#[derive(Debug, Serialize)]
pub struct DestinationStatus {
    pub path: String,
    pub exists: bool,
    pub is_repo: bool,
    /// The existing repo's origin, credentials masked.
    pub origin: Option<String>,
    /// That origin is the same GitHub repository as the URL being cloned.
    pub same_repo: bool,
    /// ...and already exactly this URL.
    pub same_url: bool,
    pub origin_is_https: bool,
    /// A repo folder with no commit: either a clone that was interrupted or an
    /// empty repository. Either way it can't be cloned into or switched.
    pub incomplete: bool,
}

fn same_github_repo(a: &str, b: &str) -> bool {
    match (
        crate::repo_scan::parse_github_owner_name(a),
        crate::repo_scan::parse_github_owner_name(b),
    ) {
        (Some((ao, an)), Some((bo, bn))) => ao.eq_ignore_ascii_case(&bo) && an.eq_ignore_ascii_case(&bn),
        _ => false,
    }
}

fn is_http_url(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("https://") || u.starts_with("http://")
}

/// Is the clone destination already taken — and if so, by this same repo?
/// Local reads only: no fetch, no network.
pub async fn destination_status(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
) -> Result<DestinationStatus, AppError> {
    let (_, _, dest) = match destination_path(url, parent_dir, folder_name) {
        Ok(d) => d,
        // no usable destination yet — nothing is "taken"
        Err(_) => {
            return Ok(DestinationStatus {
                path: String::new(), exists: false, is_repo: false, origin: None,
                same_repo: false, same_url: false, origin_is_https: false, incomplete: false,
            })
        }
    };
    let exists = dest.exists();
    let is_repo = dest.join(".git").exists();
    let origin = if is_repo {
        run_git(Some(&dest), &["remote", "get-url", "origin"]).await.ok()
    } else {
        None
    };
    let incomplete = is_repo && run_git(Some(&dest), &["rev-parse", "--verify", "HEAD"]).await.is_err();
    Ok(DestinationStatus {
        path: dest.to_string_lossy().to_string(),
        exists,
        is_repo,
        same_repo: origin.as_deref().is_some_and(|o| same_github_repo(o, url)),
        same_url: origin.as_deref().is_some_and(|o| o.trim() == url.trim()),
        origin_is_https: origin.as_deref().is_some_and(is_http_url),
        origin: origin.as_deref().map(crate::repo_scan::mask_token),
        incomplete,
    })
}

/// Point an existing clone's `origin` at `url` — e.g. from HTTPS to the SSH
/// link, so pulls and pushes go through the profile's SSH key. Deliberately
/// narrow: only `origin`'s URL changes, and only to another address of the
/// SAME repository, so this can never silently retarget a repo.
pub async fn switch_origin(repo_path: &str, url: &str) -> Result<String, AppError> {
    let repo = PathBuf::from(repo_path);
    let url = url.trim();
    if !repo.join(".git").exists() {
        return Err(AppError::Config(format!("{} is not a git repository", repo_path)));
    }
    if url.is_empty() || url.starts_with('-') {
        return Err(AppError::Config("That isn't a valid repository URL.".into()));
    }
    if is_http_url(url) {
        return Err(AppError::Config(
            "Switching to an HTTPS link wouldn't use your SSH key — use the SSH link (Code → SSH).".into(),
        ));
    }
    if run_git(Some(&repo), &["rev-parse", "--verify", "HEAD"]).await.is_err() {
        return Err(AppError::Config(
            "That folder has no commits — it looks like a clone that was interrupted. Delete it and clone again.".into(),
        ));
    }
    let current = run_git(Some(&repo), &["remote", "get-url", "origin"])
        .await
        .map_err(|_| AppError::Config("This repo has no \"origin\" remote to switch.".into()))?;
    if !same_github_repo(&current, url) {
        return Err(AppError::Config(format!(
            "Refusing to switch: origin is {}, which is a different repository than {}.",
            crate::repo_scan::mask_token(&current),
            url
        )));
    }
    run_git(Some(&repo), &["remote", "set-url", "--", "origin", url]).await?;
    Ok(format!(
        "origin switched from {} to {}",
        crate::repo_scan::mask_token(&current),
        url
    ))
}

#[derive(Debug, Serialize)]
pub struct CloneResult {
    pub path: String,
    /// Profile whose SSH key authenticated the clone (None = no profile at all).
    pub profile_name: Option<String>,
    /// Set when the new folder was added to that profile, so later pulls and
    /// pushes in it keep using the same key.
    pub mapped_to: Option<String>,
    /// Present when submodules were requested.
    pub submodules: Option<SubmoduleReport>,
    /// Present whenever the repository uses Git LFS — whether or not the
    /// download was requested, so the result can say what state the large
    /// files are in either way.
    pub lfs: Option<LfsReport>,
    /// The same for each populated submodule that uses Git LFS.
    pub submodule_lfs: Vec<SubmoduleLfs>,
}

/// One submodule's large files after a clone or a download.
#[derive(Debug, Serialize, Clone)]
pub struct SubmoduleLfs {
    pub path: String,
    pub report: LfsReport,
}

/// The large files of the populated submodules — all of them, or only the
/// named ones and those nested in them — handled as `lfs_after_checkout`
/// handles the repository's own: downloaded when asked and possible, said
/// either way.
pub(crate) async fn lfs_in_submodules(repo: &Path, only: Option<&[String]>, requested: bool, progress_for: Option<&str>) -> Vec<SubmoduleLfs> {
    let mut out = Vec::new();
    for rel in crate::push_guard::populated_submodules(repo).await {
        if let Some(only) = only {
            if !only.iter().any(|p| rel == *p || rel.starts_with(&format!("{}/", p))) {
                continue;
            }
        }
        if let Some(report) = Box::pin(lfs_after_checkout(&repo.join(&rel), requested, progress_for)).await {
            out.push(SubmoduleLfs { path: rel, report });
        }
    }
    out
}

/// What happened to the large files after a checkout. Measured by counting
/// pointer stubs before and after, like the Changes page's LFS card.
#[derive(Debug, Serialize, Clone)]
pub struct LfsReport {
    pub installed: bool,
    pub requested: bool,
    pub tracked: usize,
    pub fetched: usize,
    pub pointers_left: usize,
    /// `git lfs install --local` had to be run first (a fresh clone with no
    /// global LFS filters is exactly the state in which `git lfs pull` does nothing).
    pub configured_now: bool,
    pub error: Option<String>,
    /// One sentence, written here so the Clone page and its result say the same thing.
    pub note: String,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {}", one)
    } else {
        format!("{} {}", n, many)
    }
}

/// After a clone or a sparse checkout: download the LFS content when asked
/// and possible, and in every case say what state the large files are in.
/// `None` when the repository does not use LFS. Never an error — the
/// repository is usable and the Changes page can retry.
/// `progress_for` is the clone destination whose progress line should show
/// the download ("Large files · 2 of 5"): this repository while a clone of it
/// is in flight. `None` from the sparse "Apply selection" step, which has no
/// clone running and nothing polling for one.
pub(crate) async fn lfs_after_checkout(repo: &Path, requested: bool, progress_for: Option<&str>) -> Option<LfsReport> {
    let repo_str = repo.to_string_lossy().to_string();
    let before = crate::lfs::lfs_status(&repo_str).await.ok()?;
    if !before.uses_lfs {
        return None;
    }
    let stubs = |n: usize| plural(n, "large file is a pointer stub", "large files are pointer stubs");
    let go_to_changes = "open the repository in Changes → Git LFS → Pull LFS files";
    if requested && before.installed && before.pointers > 0 {
        // The counts appear only while a download really runs.
        let relay = progress_for.map(|dest| relay_lfs_progress(dest, &repo_str, before.pointers));
        let fetched = Box::pin(crate::git_ops::fetch_lfs_content(&repo_str)).await;
        if let Some(task) = relay {
            task.abort();
        }
        return Some(match fetched {
            Ok(f) => {
                // The last word is measured: pointer stubs counted before and
                // after, the same numbers the result card shows.
                if let Some(dest) = progress_for {
                    set_progress(dest, CloneProgress::large_files_counted(f.fetched, before.pointers));
                }
                let note = if let Some(a) = &f.error {
                    format!("The large files could not be downloaded: {} To retry, {}.", a.headline, go_to_changes)
                } else if f.after.pointers == 0 {
                    format!("Downloaded {} (Git LFS).", plural(f.fetched, "large file", "large files"))
                } else {
                    format!("Downloaded {}; {} — {}.", plural(f.fetched, "large file", "large files"), stubs(f.after.pointers), go_to_changes)
                };
                LfsReport {
                    installed: true,
                    requested,
                    tracked: f.after.tracked,
                    fetched: f.fetched,
                    pointers_left: f.after.pointers,
                    configured_now: f.configured_now,
                    error: f.error.map(|a| a.headline),
                    note,
                }
            }
            Err(e) => LfsReport {
                installed: true,
                requested,
                tracked: before.tracked,
                fetched: 0,
                pointers_left: before.pointers,
                configured_now: false,
                error: Some(e.to_string()),
                note: format!("The large files could not be downloaded ({}). To retry, {}.", e, go_to_changes),
            },
        });
    }
    let note = if !before.installed {
        format!(
            "This repository uses Git LFS, but git-lfs isn't installed on this computer, so {}. Install it ({}) and then {}.",
            stubs(before.pointers),
            crate::lfs::install_hint(),
            go_to_changes
        )
    } else if before.pointers == 0 {
        format!("All {} present (Git LFS).", plural(before.tracked, "large file is", "large files are"))
    } else {
        format!("{} — {} to download {}.", stubs(before.pointers), go_to_changes, if before.pointers == 1 { "it" } else { "them" })
    };
    Some(LfsReport {
        installed: before.installed,
        requested,
        tracked: before.tracked,
        fetched: 0,
        pointers_left: before.pointers,
        configured_now: false,
        error: None,
        note,
    })
}

#[derive(Debug, Serialize, Clone)]
pub struct SubmoduleReport {
    /// Submodules that have an address in .gitmodules.
    pub listed: usize,
    /// Of those, how many are now checked out.
    pub downloaded: usize,
    /// Submodule entries with NO address in .gitmodules — git can't fetch
    /// these, so they stay empty (as in any clone).
    pub unlisted: usize,
    /// Which listed submodules failed, by path. The loop below already knows;
    /// collapsing them into one error string threw the detail away.
    pub failed_paths: Vec<String>,
    pub error: Option<String>,
    /// Submodules whose interrupted download (a `.git` entry, no commit) was
    /// cleared before downloading again.
    #[serde(default)]
    pub cleared: Vec<String>,
}

/// Download the submodules a repo lists in `.gitmodules` — the same end state
/// as `git clone --recurse-submodules`, but sturdier when one of them fails.
///
/// Why not just pass `--recurse-submodules` to the clone: if one submodule is
/// refused, git stops before checking out the others it already cloned, and
/// no later `git submodule update` fills those folders in. So each listed
/// path gets its own call. (A bare `git submodule update --init` is no good
/// either: it aborts on the first entry with no address in .gitmodules —
/// some repos have dozens — and downloads nothing.)
/// `only`: restrict the update to these .gitmodules paths. The Changes page
/// passes the submodules it downloads, and a row's "Update to <commit>" one
/// that is populated: that one is checked out at its record whatever
/// `submodule.<name>.update` says (`--checkout` — under `update = merge` the
/// plain command writes a merge commit inside it, the tangle a superproject
/// with CI pointer bumps keeps producing), and a nested one set to merge can
/// only fast-forward. The clone path passes `None` (nothing is populated).
/// `progress_for` is the path whose progress line should name each submodule
/// as it is fetched ("Submodules · 3 of 8 · libs/sub") — this same `repo`:
/// the clone's destination while a clone is in flight, or the repository the
/// Repository page's strip is polling. Whoever passes it holds a
/// `ProgressGuard` for it, so the entry goes when the work is over and the
/// next download does not open on this one's last line.
pub(crate) async fn update_submodules(repo: &Path, ssh_override: Option<&str>, key: Option<&str>, url: &str, only: Option<&[String]>, progress_for: Option<&str>, acting_profile: Option<&str>) -> SubmoduleReport {
    // `-z` (`key\nvalue\0` records): a submodule's name defaults to its path,
    // and a path with a space would otherwise split the key in the middle.
    // (name, path): the name finds the module directory under .git/modules.
    let listed: Vec<(String, String)> = run_git(Some(repo), &["config", "-f", ".gitmodules", "--get-regexp", "-z", r"\.path$"])
        .await
        .map(|out| {
            out.split('\0')
                .filter_map(|rec| {
                    let (k, p) = rec.split_once('\n')?;
                    let name = k.trim().strip_prefix("submodule.")?.strip_suffix(".path")?.to_string();
                    let p = p.trim().to_string();
                    (!p.is_empty()).then_some((name, p))
                })
                .collect()
        })
        .unwrap_or_default();
    let gitlinks = run_git(Some(repo), &["ls-files", "-s"])
        .await
        .map(|out| out.lines().filter(|l| l.starts_with("160000 ")).count())
        .unwrap_or(0);
    let unlisted = gitlinks.saturating_sub(listed.len());
    let listed: Vec<(String, String)> = match only {
        Some(want) => listed.into_iter().filter(|(_, p)| want.iter().any(|w| crate::paths::norm(w) == crate::paths::norm(p))).collect(),
        None => listed,
    };
    // The name and path are the repository author's text, and both are joined
    // onto the repository below, to find a module directory and to clear an
    // interrupted download's leftovers: a name like `../../..` put that delete
    // outside the repository. Such an entry is refused, never downloaded
    // (git itself ignores a submodule name with `..` in it), and the report
    // says so.
    let (listed, refused): (Vec<_>, Vec<_>) =
        listed.into_iter().partition(|(n, p)| crate::paths::is_plain_relative(n) && crate::paths::is_plain_relative(p));
    let mut report = SubmoduleReport {
        listed: listed.len() + refused.len(),
        downloaded: 0,
        unlisted,
        failed_paths: refused.iter().map(|(_, p)| p.clone()).collect(),
        error: (!refused.is_empty()).then(|| refused_entries(&refused)),
        cleared: Vec::new(),
    };
    // Leave the repo exactly as `git clone --recurse-submodules` does: every
    // submodule counts as active, so later `git pull --recurse-submodules` /
    // `git submodule update` also fetch submodules the team adds afterwards.
    // (Set even when there are none yet — the team command does too.)
    let _ = run_git(Some(repo), &["config", "submodule.active", "."]).await;
    if listed.is_empty() {
        return report;
    }
    // A modules folder that is there but leads out of the repository's git
    // directory (`.git/modules` a link or a junction; an archive can carry
    // one): git's own `submodule update` would write through it — it removes
    // `index` and writes `core.worktree` into a module directory it finds
    // there — and clear_incomplete clears nothing below it. Nothing is
    // downloaded, and the report says why. One not made yet is git's to make.
    if let Some((base, false)) = modules_folder(repo).await {
        if std::fs::symlink_metadata(&base).is_ok() {
            let paths: Vec<String> = listed.iter().map(|(_, p)| p.clone()).collect();
            let said = format!(
                "Not downloaded: {}. This repository's .git/modules leads outside its git folder (a link or a junction), so GitSwitch will not let git write there. Replace the link with a folder of its own, then Download again.",
                paths.join(", ")
            );
            report.failed_paths.extend(paths);
            report.error = Some(match report.error.take() {
                Some(refusal) => format!("{}\n\n{}", refusal, said),
                None => said,
            });
            return report;
        }
    }

    // One submodule per git call. In a single batch, one refused submodule
    // makes git stop before checking out the ones it DID clone — and a retry
    // then thinks they're up to date, leaving empty folders behind.
    let mut failed: Vec<String> = Vec::new();
    let mut first_error: Option<String> = None;
    for (finished, (name, path)) in listed.iter().enumerate() {
        // Before, not after: the line should name the submodule while it is
        // the one taking the time.
        if let Some(dest) = progress_for {
            set_progress(dest, CloneProgress::submodules(finished, listed.len(), Some(path)));
        }
        if let Some(p) = clear_incomplete(repo, name, path).await {
            report.cleared.push(p);
        }
        // Streamed, with the inner clone's progress on the page's line, and
        // stopped only when it reports nothing for a long while — never for
        // taking long: a big submodule on an ordinary connection needs more
        // than any fixed limit. No credential prompt can hang it, and the
        // ssh override is a `-c` for this command only.
        let cfg: Vec<String> = ssh_override.map(|kv| vec![kv.to_string()]).unwrap_or_default();
        let total = listed.len();
        let sub_dir = repo.join(path);
        let recorded = run_git(Some(repo), &["ls-files", "-s", "--", path])
            .await
            .ok()
            .and_then(|o| o.lines().next().and_then(|l| l.split_whitespace().nth(1)).map(str::to_string));
        let have_recorded = |sha: String| {
            let dir = sub_dir.clone();
            async move { run_git(Some(&dir), &["cat-file", "-e", &format!("{}^{{commit}}", sha)]).await.is_ok() }
        };
        // Downloaded before, and the commit the superproject records is not
        // here: `submodule update` would fetch it itself — without a word of
        // progress, so a long fetch could not be shown and looked like a
        // stall. It is fetched first, here, with git's progress on.
        let downloaded = own_repository(&sub_dir).await;
        let must_fetch = match &recorded {
            Some(sha) if downloaded => !have_recorded(sha.clone()).await,
            _ => false,
        };
        // Shallow (`submodule.<name>.shallow`): git downloads one commit, a
        // fraction of what the host reports for the whole history.
        let shallow = run_git(Some(repo), &["config", "-f", ".gitmodules", "--bool", "--get", &format!("submodule.{}.shallow", name)])
            .await
            .map(|v| v.trim() == "true")
            .unwrap_or(false);
        // What its host says it weighs, for "of about 1.2 GB" and the time
        // left — for a first, full download only: a submodule already here
        // either has its commit (a local checkout, nothing to ask about) or
        // fetches what is new, a fraction of it; a shallow one downloads one
        // commit. Asked as the account the work is done as (the clone's, or
        // the folder's profile) and no other, anonymously after it. Its address as git resolves it
        // (a relative one against the superproject's): `init` is what
        // `update --init` does first.
        let (size, asking) = if progress_for.is_some() && !downloaded && !shallow {
            let _ = run_git(Some(repo), &["submodule", "init", "--", path]).await;
            let address = run_git(Some(repo), &["config", "--get", &format!("submodule.{}.url", name)]).await.unwrap_or_default();
            ask_size(address.trim(), Some(accounts_of(acting_profile)))
        } else {
            (EstimateCell::default(), None)
        };
        // `--recursive` downloads the submodules nested inside this one in
        // the same command; the host's size is for this one alone, so it
        // applies to the first download only.
        let first_done = std::sync::atomic::AtomicBool::new(false);
        let on_progress = |inner: CloneProgress| {
            if let Some(dest) = progress_for {
                // The bar is the submodules' share, filling once per
                // submodule as its download comes in; the other inner phases
                // (counting, compressing, deltas) do not move it back.
                let receiving = inner.phase == "Receiving objects";
                let percent = if receiving {
                    Some(((finished as u64 * 100 + inner.percent.unwrap_or(0) as u64) / total.max(1) as u64) as u8)
                } else {
                    percent_of(finished as u64, total as u64)
                };
                let nested = first_done.load(std::sync::atomic::Ordering::Relaxed);
                let known = if nested { None } else { size.lock().ok().and_then(|c| *c) };
                let (estimate_kb, eta_secs) = if receiving { download_estimate(inner.bytes.as_deref(), inner.rate.as_deref(), inner.percent, known) } else { (None, None) };
                if receiving && inner.percent == Some(100) {
                    first_done.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                set_progress(dest, CloneProgress { percent, bytes: inner.bytes, rate: inner.rate, estimate_kb, eta_secs, ..CloneProgress::submodules(finished, total, Some(path)) });
            }
        };
        let mut fetched = false;
        let mut stalled: Option<AppError> = None;
        if must_fetch {
            // Kept as a pack (`fetch.unpackLimit=1`): git says nothing while
            // it unpacks a fetch of under a hundred objects, however big.
            let mut fetch_cfg = cfg.clone();
            fetch_cfg.push("fetch.unpackLimit=1".into());
            match run_git_streaming(Some(&sub_dir), &fetch_cfg, &["fetch", "--progress", "--no-recurse-submodules"], Some(submodule_stall()), true, &on_progress).await {
                Ok(_) => fetched = true,
                // Stopped for silence: the update would fetch from the same
                // remote and wait as long again. Any other failure is left to
                // git's own update, exactly as before.
                Err(e) if is_stall(&e) => stalled = Some(e),
                Err(_) => {}
            }
        }
        let mut res = match stalled {
            Some(e) => Err(e),
            None => {
                // With the commit already fetched there is nothing left to
                // download but nested submodules: the clock waits for one of
                // them to start ("Cloning into") — a long silent checkout
                // (a smudge filter fetching large files) is not a stall.
                let nothing_to_fetch = fetched && match &recorded {
                    Some(sha) => have_recorded(sha.clone()).await,
                    None => false,
                };
                // What is recorded is checked out whatever
                // `submodule.<name>.update` says. Already here, merge, rebase
                // or none would make "Update to" a merge commit inside it, a
                // rebase, or nothing at all; on a first download git turns
                // those into a checkout by itself but would still run an
                // `update = !command` from the repository's own config.
                let mut update = vec!["submodule", "update", "--init", "--recursive", "--progress", "--checkout"];
                let mut update_cfg = cfg.clone();
                if downloaded {
                    // And a nested one set to `update = merge` can only
                    // fast-forward: one with commits of its own fails with
                    // git's words instead of getting a merge commit.
                    update_cfg.push("merge.ff=only".into());
                }
                update.extend(["--", path.as_str()]);
                run_git_streaming(Some(repo), &update_cfg, &update, Some(submodule_stall()), !nothing_to_fetch, &on_progress).await
            }
        };
        if let Some(t) = asking {
            t.abort();
        }
        // A module directory that holds the commits but has nothing checked
        // out (HEAD pointing nowhere) makes `submodule update` give up
        // ("Unable to find current revision"): when the recorded commit is
        // there, check it out ourselves.
        if !is_checked_out(&repo.join(path)).await {
            let sub = repo.join(path);
            let recorded = run_git(Some(repo), &["ls-files", "-s", "--", path]).await.ok().and_then(|o| o.split_whitespace().nth(1).map(|s| s.to_string()));
            if let Some(sha) = recorded {
                if run_git(Some(&sub), &["cat-file", "-e", &format!("{}^{{commit}}", sha)]).await.is_ok()
                    && run_git(Some(&sub), &["checkout", "-q", "--detach", &sha]).await.is_ok()
                {
                    res = Ok(String::new());
                }
            }
        }
        if let Err(e) = res {
            // A download the app stopped leaves what git's inner clone had
            // made so far. On unix git removes that itself (SIGTERM, then the
            // group's grace); Windows ends the whole job object at once, so
            // the empty module directory and the folder's `.git` stay — the
            // row then reads "interrupted", and the `git submodule update
            // --init` the error suggests fails with "Unable to find current
            // revision". Cleared as git would have; anything holding a commit
            // is kept (clear_incomplete's rule). Not reported as `cleared`:
            // that names leftovers found before a download.
            let _ = clear_incomplete(repo, name, path).await;
            failed.push(path.clone());
            if first_error.is_none() {
                first_error = Some(match e {
                    AppError::Command(msg) => explain_transport_error(&msg, key, url, false),
                    other => other.to_string(),
                });
            }
        }
    }
    // The loop is through — a failed one still counts as attempted here; the
    // report's `downloaded` below is the measured number.
    if let Some(dest) = progress_for {
        set_progress(dest, CloneProgress::submodules(listed.len(), listed.len(), None));
    }
    report.failed_paths.extend(failed.iter().cloned());
    if let Some(err) = first_error {
        let said = format!(
            "Couldn't download: {}.\n\n{}\n\nOnce that's sorted, Download on the repository's Submodules tab fetches {} — what already arrived is kept (in a terminal: `git submodule update --init --recursive`).",
            failed.join(", "),
            err,
            if failed.len() == 1 { "it" } else { "them" }
        );
        // After the refusal of an entry that is not plain, when there was one.
        report.error = Some(match report.error.take() {
            Some(refusal) => format!("{}\n\n{}", refusal, said),
            None => said,
        });
    }
    let mut downloaded = 0;
    for (_, path) in &listed {
        if is_checked_out(&repo.join(path)).await {
            downloaded += 1;
        }
    }
    report.downloaded = downloaded;
    report
}

/// What the report says of `.gitmodules` entries whose name or path is not
/// plain folder names: (name, path) each.
fn refused_entries(refused: &[(String, String)]) -> String {
    let each: Vec<String> = refused.iter().map(|(n, p)| if n == p { format!("`{}`", p) } else { format!("`{}` (named `{}`)", p, n) }).collect();
    let (entry, is) = if refused.len() == 1 { ("Its name or path", "is") } else { ("Their names or paths", "are") };
    format!(
        "Not downloaded: {}. {} in .gitmodules {} not plain folder names (it has `..`, `.`, an empty part, a leading `/`, a drive or a backslash), so GitSwitch will not use it as a folder; git itself refuses a name with `..`. Ask whoever maintains the repository to fix .gitmodules.",
        each.join(", "),
        entry,
        is
    )
}

/// The repository's modules folder as git names it (`rev-parse --git-path
/// modules`), and whether it lies strictly inside the repository's common git
/// directory (`rev-parse --git-common-dir`, git 2.5; resolved against the
/// repository when relative), as the disk resolves both. False when either
/// cannot be resolved (a folder not made yet, a link that leads nowhere).
/// `None` when git cannot say.
async fn modules_folder(repo: &Path) -> Option<(PathBuf, bool)> {
    let resolve = |m: String| {
        let m = m.trim().to_string();
        if Path::new(&m).is_absolute() {
            PathBuf::from(m)
        } else {
            repo.join(m)
        }
    };
    let base = resolve(run_git(Some(repo), &["rev-parse", "--git-path", "modules"]).await.ok()?);
    let common = resolve(run_git(Some(repo), &["rev-parse", "--git-common-dir"]).await.ok()?);
    let inside = crate::paths::strictly_inside(&base, &common);
    Some((base, inside))
}

/// A submodule counts as downloaded only if its files are actually there:
/// cloned-but-never-checked-out leaves `.git` with an empty folder.
/// An interrupted download leaves a `.git` entry in the folder and a module
/// directory with no commit in it; `git submodule update` cannot finish such
/// a folder. When nothing of it holds a commit — nothing to lose — the
/// leftovers are removed so the download starts afresh. The path, when it did.
async fn clear_incomplete(repo: &Path, name: &str, path: &str) -> Option<String> {
    // Both from .gitmodules: update_submodules refuses an entry that is not
    // plain names before it gets here; checked again because what follows
    // deletes, and only ever inside the repository.
    if !crate::paths::is_plain_relative(name) || !crate::paths::is_plain_relative(path) {
        return None;
    }
    let dir = repo.join(path);
    let gitfile = dir.join(".git");
    if !gitfile.exists() {
        return None;
    }
    // A folder (or a `.git` in it) that is a link to somewhere else resolves
    // outside the work tree: nothing there is ours to clear.
    if !crate::paths::strictly_inside(&gitfile, repo) {
        return None;
    }
    if run_git(Some(&dir), &["rev-parse", "--verify", "--quiet", "HEAD"]).await.is_ok() {
        return None;
    }
    // A git directory holds no commit when git can read it and counts none —
    // or when git cannot read it (no HEAD, say) and it holds neither refs nor
    // finished objects on disk: an interrupted transfer leaves only temporary
    // pack files. Anything with a ref or a finished object is kept.
    let commitless = |d: PathBuf| async move {
        // Named outright (`--git-dir`), never discovered: a module directory
        // with no HEAD makes git walk up into the superproject's own .git
        // and count ITS commits. When git cannot read it at all, the disk
        // decides.
        let gd = if d.join(".git").is_dir() { d.join(".git") } else { d.clone() };
        let gd_s = gd.to_string_lossy().to_string();
        if let Some(n) = run_git(None, &["--git-dir", &gd_s, "rev-list", "--all", "--count"]).await.ok().and_then(|s| s.trim().parse::<u64>().ok()) {
            return n == 0;
        }
        // A ref is a file somewhere under refs/ — the empty `heads/` and
        // `remotes/origin/` folders git leaves behind are not refs.
        fn any_file(dir: &Path) -> bool {
            std::fs::read_dir(dir)
                .map(|it| it.flatten().any(|e| e.path().is_dir() && any_file(&e.path()) || e.path().is_file()))
                .unwrap_or(false)
        }
        let has_refs = gd.join("packed-refs").exists() || any_file(&gd.join("refs"));
        let has_objects = std::fs::read_dir(gd.join("objects"))
            .map(|it| {
                it.flatten().any(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.len() == 2 && n.chars().all(|c| c.is_ascii_hexdigit())
                })
            })
            .unwrap_or(false)
            || std::fs::read_dir(gd.join("objects/pack"))
                .map(|it| it.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".pack")))
                .unwrap_or(false);
        !has_refs && !has_objects
    };
    let git_path = |rel: String| async move {
        let m = run_git(Some(repo), &["rev-parse", "--git-path", &rel]).await.ok()?.trim().to_string();
        Some(if Path::new(&m).is_absolute() { PathBuf::from(&m) } else { repo.join(&m) })
    };
    if let Some(mp) = git_path(format!("modules/{}", name)).await {
        if mp.exists() {
            // git prints `modules/<name>` as given, `..` and all: the module
            // directory is cleared only when the disk puts it inside the
            // repository's own modules folder, and that folder inside the
            // repository's git directory. The repository decides what
            // `.git/modules` is: a link there resolves both paths through the
            // same link, so the first check alone holds wherever it points.
            // The common git directory (`--git-common-dir`, git 2.5) holds the
            // modules folder of a linked worktree, of a superproject that is
            // itself a submodule (under its parent's `.git/modules`) and of a
            // `.git` file naming a separate git directory alike.
            let inside = match modules_folder(repo).await {
                Some((base, true)) => crate::paths::strictly_inside(&mp, &base),
                _ => false,
            };
            if !inside || !commitless(mp.clone()).await {
                return None;
            }
            let _ = std::fs::remove_dir_all(&mp);
        }
    }
    if gitfile.is_dir() {
        if !commitless(dir.clone()).await {
            return None;
        }
        let _ = std::fs::remove_dir_all(&gitfile);
    } else {
        let _ = std::fs::remove_file(&gitfile);
    }
    Some(path.to_string())
}

/// The folder is a repository of its own — never the superproject above it,
/// which git finds when the folder has no `.git` of its own.
pub(crate) async fn own_repository(dir: &Path) -> bool {
    if !dir.join(".git").exists() {
        return false;
    }
    let Ok(top) = run_git(Some(dir), &["rev-parse", "--show-toplevel"]).await else { return false };
    let canonical = |p: &Path| std::fs::canonicalize(p).ok();
    canonical(Path::new(top.trim())).is_some() && canonical(Path::new(top.trim())) == canonical(dir)
}

fn is_stall(e: &AppError) -> bool {
    matches!(e, AppError::Command(m) if m.contains("made no progress for"))
}

pub(crate) async fn is_checked_out(dir: &Path) -> bool {
    if !dir.join(".git").exists() || run_git(Some(dir), &["rev-parse", "--verify", "HEAD"]).await.is_err() {
        return false;
    }
    let head_has_files = run_git(Some(dir), &["ls-tree", "--name-only", "HEAD"])
        .await
        .map(|o| !o.trim().is_empty())
        .unwrap_or(false);
    if !head_has_files {
        return true; // an empty commit has nothing to check out
    }
    // A checked-out tree has a populated index; `--no-checkout` leaves none.
    match run_git(Some(dir), &["rev-parse", "--absolute-git-dir"]).await {
        Ok(gitdir) => std::fs::metadata(PathBuf::from(gitdir.trim()).join("index"))
            .map(|m| m.len() > 32)
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// The profile git applies to `path`: longest matching folder, else the default.
pub(crate) fn resolve_profile<'a>(path: &str, profs: &'a [Profile]) -> Option<&'a Profile> {
    crate::commit_audit::owning_profile(path, profs).or_else(|| profs.iter().find(|p| p.is_default))
}

/// Same command GitSwitch writes into the per-profile gitconfig.
fn ssh_command_for(key: &str) -> String {
    crate::git_config::ssh_command_for(key)
}

/// GitHub shows `org-<ID>@github.com:org/repo.git` instead of `git@…` when an
/// organization uses SSH certificate authorities.
pub(crate) fn uses_org_certificate_url(url: &str) -> bool {
    let u = url.trim();
    let u = u.strip_prefix("ssh://").unwrap_or(u);
    u.split_once('@')
        .map(|(user, rest)| user.starts_with("org-") && rest.starts_with("github.com"))
        .unwrap_or(false)
}

/// A failure of the network itself — the server never reached, its name not
/// found, the connection dropped part way — as a sentence saying so, or
/// `None` when the server answered and refused.
///
/// Read before anything else. ssh ends EVERY failure with "Could not read
/// from remote repository", a timed-out connection included; taking that
/// line for a refusal sends a person to their keys and their account when
/// their network dropped.
/// This computer's own disk refused what arrived — not the network, and not
/// the server. The sentence to show, or `None`.
pub(crate) fn disk_failure(stderr: &str) -> Option<String> {
    let s = stderr.to_lowercase();
    if s.contains("no space left on device") || s.contains("disk quota exceeded") {
        return Some(
            "This computer's disk is full (or this account's disk quota is used up), so git could not write what arrived. Nothing is wrong with the network, the key or the account. Free some space, then try again."
                .to_string(),
        );
    }
    None
}

pub(crate) fn network_failure(stderr: &str) -> Option<String> {
    let s = stderr.to_lowercase();
    let has = |needle: &str| s.contains(needle);
    // A full disk ends with the same index-pack lines as a dropped
    // connection: it is its own failure (`disk_failure`), never this one.
    if disk_failure(stderr).is_some() {
        return None;
    }
    // The server answered — with a refusal, or an HTTP error of its own
    // ("The requested URL returned error: 400", a size limit): that is not the
    // network either, whatever the connection did next.
    if has("the requested url returned error") || has("remote: fatal:") || has("remote: error:") {
        return None;
    }
    // The server answered and said no: that is not the network.
    if has("permission denied")
        || has("repository not found")
        || has("saml sso")
        || has("host key verification failed")
        || has("could not read username")
        || has("terminal prompts disabled")
        || has("authentication failed")
        || has("invalid username or password")
    {
        return None;
    }
    let blameless = "Nothing is wrong with the key or the account.";
    if has("could not resolve host") || has("name or service not known") || has("nodename nor servname") || has("temporary failure in name resolution") {
        return Some(format!(
            "This computer could not look up the server's name: it is offline, or its DNS is not answering. {} Check the connection (and the VPN, if the repository needs one), then try again.",
            blameless
        ));
    }
    let unreachable = (has("connect to host") && (has("timed out") || has("connection refused") || has("network is unreachable") || has("no route to host")))
        || has("failed to connect to")
        || has("couldn't connect to server")
        || has("connection timed out")
        || has("kex_exchange_identification");
    if unreachable {
        let mut m = format!(
            "The server could not be reached: the connection was never made. {} Check the connection (and the VPN, if the repository needs one), then try again.",
            blameless
        );
        // Port 22 itself, not 2222 or 22022.
        let port_22 = s.match_indices("port 22").any(|(i, m)| !s[i + m.len()..].starts_with(|c: char| c.is_ascii_digit()));
        if port_22 {
            m.push_str(" Some networks block port 22, the one SSH uses");
            if has("github.com") {
                m.push_str(": GitHub also answers SSH on port 443 — in ~/.ssh/config, under `Host github.com`, set `HostName ssh.github.com` and `Port 443` — or use the repository's HTTPS address.");
            } else {
                m.push_str("; the repository's HTTPS address goes through where SSH does not.");
            }
        }
        return Some(m);
    }
    let dropped = has("unexpected disconnect")
        || has("early eof")
        || has("unexpected eof")
        || has("remote end hung up unexpectedly")
        || has("connection reset")
        || has("broken pipe")
        || has("closed by remote host")
        || has("connection closed by")
        || has("rpc failed")
        || has("outstanding read data remaining")
        || has("was not closed cleanly")
        || has("ssl_error_syscall")
        || has("not responding")
        || has("operation timed out")
        || has("invalid index-pack output")
        || has("index-pack failed");
    if dropped {
        return Some(format!(
            "The connection dropped while data was moving. {} Try again — git starts the transfer over from the beginning, so a big repository needs a connection that holds for the whole of it.",
            blameless
        ));
    }
    None
}

/// Turn git/ssh's terse failures into something that says what to do — for
/// the Clone page, where "Clone as" chooses the account.
pub(crate) fn explain_clone_error(stderr: &str, key: Option<&str>, url: &str) -> String {
    explain_transport_error(stderr, key, url, true)
}

/// The same for any operation on the network. `for_clone`: the advice may
/// point at the Clone page's "Clone as"; otherwise the folder's own profile
/// decides the account, and the advice says so.
pub(crate) fn explain_transport_error(stderr: &str, key: Option<&str>, url: &str, for_clone: bool) -> String {
    let key_name = key.map(crate::paths::base_name);
    let the_key = match &key_name {
        Some(k) => format!("the SSH key {}", k),
        None => "the SSH key ssh offered".to_string(),
    };
    let then = if for_clone { "clone again" } else { "try again" };
    let network = network_failure(stderr);
    let disk = disk_failure(stderr);
    let refused = network.is_none()
        && disk.is_none()
        && (stderr.contains("Permission denied (publickey)")
            || stderr.contains("Repository not found")
            || stderr.contains("Could not read from remote repository"));

    let mut hint = if let Some(d) = disk {
        d
    } else if let Some(net) = network {
        net
    } else if stderr.contains("SAML SSO") {
        format!(
            "This organization uses single sign-on, and {} hasn't been authorized for it yet. \
             On GitHub (signed in as the account that belongs to the organization): Settings → SSH and GPG keys → Configure SSO next to this key → Authorize for the organization. Then {}.",
            the_key, then
        )
    } else if stderr.contains("Host key verification failed") {
        format!("This computer doesn't trust the server's SSH host key yet. Run `ssh -T git@github.com` (or the same for your server) once in Terminal, answer \"yes\", then {}.", then)
    } else if stderr.contains("Permission denied (publickey)") {
        if for_clone {
            format!("GitHub refused {}. Either it isn't registered on the GitHub account, or the organization doesn't accept it (see below). Register it from SSH Keys, or pick another profile under \"Clone as\".", the_key)
        } else {
            format!("GitHub refused {}. Either it isn't registered on the GitHub account, or the organization doesn't accept it (see below). Register it from SSH Keys, or check in Profiles which profile this folder belongs to — its key is the one used here.", the_key)
        }
    } else if stderr.contains("Repository not found") || stderr.contains("Could not read from remote repository") {
        if for_clone {
            format!("GitHub accepted {}, but that account can't see this repository. Check the URL, or under \"Clone as\" pick the profile whose account has access (e.g. your company profile for a company repo).", the_key)
        } else {
            format!("GitHub accepted {}, but that account can't see this repository. Check the remote's address, or check in Profiles that this folder belongs to the profile whose account has access.", the_key)
        }
    } else if stderr.contains("could not read Username") || stderr.contains("terminal prompts disabled") {
        "This HTTPS URL needs a username and password, which GitSwitch can't type in for you. Use the SSH URL (git@github.com:owner/repo.git) instead.".to_string()
    } else {
        String::new()
    };

    // The org-<ID>@ link means the organization uses SSH certificates. If it
    // *requires* them, GitHub refuses plain keys (and HTTPS) outright, so the
    // generic "not registered / no access" advice above would mislead.
    if refused && uses_org_certificate_url(url) && !stderr.contains("SAML SSO") {
        let (key_ref, cert) = match &key_name {
            Some(k) => (k.clone(), format!("{}-cert.pub", k)),
            None => ("your SSH key".to_string(), "<the key's name>-cert.pub".to_string()),
        };
        hint.push_str(&format!(
            "\n\nThis link (org-…@github.com) means the organization uses SSH certificates. If it requires them, a plain key is refused even when it's registered: \
             ask your company admin for a certificate for {}, and save it next to the key as {} — ssh picks it up automatically.",
            key_ref, cert
        ));
    }

    if hint.is_empty() {
        stderr.trim().to_string()
    } else {
        format!("{}\n\ngit said: {}", hint.trim_start(), stderr.trim())
    }
}

#[derive(Debug, Serialize)]
pub struct CertInfo {
    /// Where ssh looks for it: `<key>-cert.pub`.
    pub path: String,
    pub exists: bool,
    /// "forever" or an end timestamp, as ssh-keygen prints it.
    pub valid_until: Option<String>,
    pub expired: bool,
}

/// Is there a CA-signed certificate next to this private key? ssh loads
/// `<key>-cert.pub` automatically whenever `-i <key>` is used. Reads only the
/// public certificate file — no private key, no network.
pub async fn certificate_info(key_path: &str) -> CertInfo {
    let path = format!("{}-cert.pub", key_path);
    let mut info = CertInfo { exists: PathBuf::from(&path).is_file(), path, valid_until: None, expired: false };
    if !info.exists {
        return info;
    }
    if let Ok(out) = run_git_like("ssh-keygen", &["-L", "-f", &info.path]).await {
        if let Some(line) = out.lines().map(str::trim).find(|l| l.starts_with("Valid:")) {
            let v = line.trim_start_matches("Valid:").trim();
            if v == "forever" {
                info.valid_until = Some("forever".into());
            } else if let Some(end) = v.rsplit(" to ").next() {
                info.valid_until = Some(end.to_string());
                // ssh-keygen prints local time as YYYY-MM-DDTHH:MM:SS
                if let Ok(t) = chrono::NaiveDateTime::parse_from_str(end, "%Y-%m-%dT%H:%M:%S") {
                    info.expired = t < chrono::Local::now().naive_local();
                }
            }
        }
    }
    info
}

async fn run_git_like(bin: &str, args: &[&str]) -> Result<String, AppError> {
    let out = crate::spawn::async_command(bin)
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::Command(format!("Failed to run {}: {}", bin, e)))?;
    if !out.status.success() {
        return Err(AppError::Command(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Clone `url` into `parent_dir`, authenticating either as the profile that
/// owns the destination folder (`clone_as` = None — exactly what a terminal
/// `git clone` would do) or as an explicitly chosen profile.
///
/// With an explicit profile the key is passed via `git -c core.sshCommand=…`,
/// which applies to this one command only. (`git clone -c …` would instead
/// store the key in the new repo's own config, where it would silently beat
/// GitSwitch's rules forever after.) The new folder is then added to that
/// profile, so pulls and pushes in it keep using the same account.
async fn clone_as_profile(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
    clone_as: Option<&str>,
    extra_args: &[&str],
    with_submodules: bool,
    with_lfs: bool,
) -> Result<CloneResult, AppError> {
    let (parent, name, dest) = clone_destination(url, parent_dir, folder_name)?;
    let dest_str = dest.to_string_lossy().to_string();

    let store = profiles::load_profiles()?;
    let folder_profile = resolve_profile(&dest_str, &store.profiles).cloned();
    let chosen = match clone_as.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => Some(
            store
                .profiles
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .ok_or_else(|| AppError::NotFound(format!("Profile {} not found", id)))?,
        ),
        None => None,
    };
    let acting = chosen.clone().or_else(|| folder_profile.clone());
    let acting_id = acting.as_ref().map(|p| p.id.clone());
    let key = acting.as_ref().and_then(|p| p.ssh_key_path.clone());
    if let (Some(p), None) = (chosen.as_ref(), key.as_ref()) {
        return Err(AppError::Config(format!(
            "Profile \"{}\" has no SSH key, so it can't authenticate an SSH clone. Add a key to it first.",
            p.name
        )));
    }

    // Only when a profile was chosen explicitly; otherwise the folder's
    // includeIf rule already picks the key, exactly like a terminal clone.
    let ssh_override = chosen
        .as_ref()
        .map(|_| format!("core.sshCommand={}", ssh_command_for(key.as_deref().unwrap_or_default())));

    let mut args: Vec<String> = Vec::new();
    if let Some(cmd) = ssh_override.as_ref() {
        args.push("-c".into());
        args.push(cmd.clone());
    }
    args.push("clone".into());
    // Progress is only written when asked: there is no terminal here.
    args.push("--progress".into());
    args.extend(extra_args.iter().map(|a| a.to_string()));
    // `--` so a URL or folder name starting with "-" can never be read as an option.
    args.push("--".into());
    args.push(url.trim().to_string());
    args.push(name);

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let _progress = ProgressGuard(crate::paths::norm(&dest_str));
    let _running = CloneGuard::hold(Path::new(&dest_str), url.trim());
    set_progress(&dest_str, CloneProgress::phase("Connecting"));
    let progress_key = crate::paths::norm(&dest_str);
    // The host's size is for a whole repository: a partial clone downloads a
    // fraction of it, so there the time left comes from the objects alone.
    let (size, asking) = if extra_args.iter().any(|a| a.starts_with("--filter")) { (EstimateCell::default(), None) } else { ask_size(url.trim(), None) };
    let cloned = run_git_streaming(Some(&parent), &[], &arg_refs, None, true, &|p| {
        let known = size.lock().ok().and_then(|c| *c);
        set_progress(&progress_key, with_estimate(p, known))
    })
    .await;
    if let Some(t) = asking {
        t.abort();
    }
    cloned.map_err(|e| match e {
        AppError::Command(msg) => AppError::Command(explain_clone_error(&msg, key.as_deref(), url)),
        other => other,
    })?;

    // The main clone succeeded; submodule trouble is reported, not fatal —
    // the repo is usable and `git submodule update` can be retried.
    let submodules = if with_submodules {
        set_progress(&dest_str, CloneProgress::phase("Submodules"));
        Some(Box::pin(update_submodules(&dest, ssh_override.as_deref(), key.as_deref(), url, None, Some(&dest_str), acting_id.as_deref())).await)
    } else {
        None
    };
    // Same rule for the large files: reported, never fatal. A `--no-checkout`
    // clone has no work tree yet, so its LFS step waits for `sparse_set`.
    let lfs = if extra_args.contains(&"--no-checkout") {
        None
    } else {
        set_progress(&dest_str, CloneProgress::phase("Large files"));
        lfs_after_checkout(&dest, with_lfs, Some(&dest_str)).await
    };
    // The submodules' large files, the same way: a submodule's own are as much
    // the clone as the repository's.
    let submodule_lfs = if with_submodules && !extra_args.contains(&"--no-checkout") {
        Box::pin(lfs_in_submodules(&dest, None, with_lfs, Some(&dest_str))).await
    } else {
        Vec::new()
    };

    let mut mapped_to = None;
    if let Some(p) = chosen.as_ref() {
        let already = folder_profile.as_ref().is_some_and(|f| f.id == p.id);
        if !already {
            // The store as it is now, not as it was when the clone began: a
            // folder added to this profile meanwhile must stay.
            profiles::add_folders(&p.id, std::slice::from_ref(&dest_str))
                .and_then(|_| crate::git_config::apply_git_config())
                .map_err(|e| {
                    AppError::Config(format!(
                        "Cloned into {}, but couldn't add it to profile \"{}\" ({}). Add the folder in Profiles so pulls and pushes use the same account.",
                        dest_str, p.name, e
                    ))
                })?;
            mapped_to = Some(p.name.clone());
        }
    }

    Ok(CloneResult {
        path: dest_str,
        profile_name: acting.map(|p| p.name),
        mapped_to,
        submodules,
        lfs,
        submodule_lfs,
    })
}

/// Partial clone (metadata only): `git clone --filter=blob:none --no-checkout`.
/// Nothing is materialized until folders are selected.
pub async fn sparse_clone(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
    clone_as: Option<&str>,
) -> Result<CloneResult, AppError> {
    clone_as_profile(url, parent_dir, folder_name, clone_as, &["--filter=blob:none", "--no-checkout"], false, false).await
}

/// Ordinary full clone — what `git clone <url>` does in a terminal.
pub async fn full_clone(
    url: &str,
    parent_dir: &str,
    folder_name: Option<&str>,
    clone_as: Option<&str>,
    with_submodules: bool,
    with_lfs: bool,
) -> Result<CloneResult, AppError> {
    clone_as_profile(url, parent_dir, folder_name, clone_as, &[], with_submodules, with_lfs).await
}

/// Inspect a repo: default branch, top-level folders, and current sparse config.
pub async fn repo_info(repo_path: &str) -> Result<SparseInfo, AppError> {
    let repo = PathBuf::from(repo_path);
    if !repo.join(".git").exists() {
        return Err(AppError::Config(format!("Not a git repository: {}", repo_path)));
    }

    let branch = run_git(Some(&repo), &["rev-parse", "--abbrev-ref", "HEAD"]).await?;

    // Tree objects are fetched even with --filter=blob:none, so this works
    // offline right after a partial clone. NUL-separated: otherwise git
    // escapes a name (`"caf\303\251"`), and a folder ticked in that spelling
    // checks out nothing.
    let available_dirs: Vec<String> = match crate::git_exec::GitCmd::at(&repo).args(["ls-tree", "-d", "-z", "--name-only", "HEAD"]).run().await {
        Ok(out) if out.ok() => out.stdout.split(|b| *b == 0).filter(|n| !n.is_empty()).map(|n| String::from_utf8_lossy(n).to_string()).collect(),
        _ => Vec::new(),
    };

    let is_sparse = run_git(Some(&repo), &["config", "--get", "core.sparseCheckout"])
        .await
        .map(|v| v == "true")
        .unwrap_or(false);

    // `list` has no -z: with core.quotePath off only a name holding a quote,
    // a backslash or a control character is still escaped, and that is undone.
    let sparse_dirs: Vec<String> = if is_sparse {
        match crate::git_exec::GitCmd::at(&repo).cfg("core.quotePath", "false").args(["sparse-checkout", "list"]).run().await {
            Ok(out) if out.ok() => out
                .stdout
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .map(|l| {
                    let raw = if l.first() == Some(&b'"') { unquote_c_style(l).unwrap_or_else(|| l.to_vec()) } else { l.to_vec() };
                    String::from_utf8_lossy(&raw).to_string()
                })
                .collect(),
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };

    Ok(SparseInfo {
        path: repo.to_string_lossy().to_string(),
        branch,
        is_sparse,
        sparse_dirs,
        available_dirs,
    })
}

/// Undo git's C-style quoting of a path (`"say\"hi"`, `"caf\303\251"`): the
/// raw bytes, or None for anything that is not a well-formed quoted name.
fn unquote_c_style(s: &[u8]) -> Option<Vec<u8>> {
    let inner = s.strip_prefix(b"\"")?.strip_suffix(b"\"")?;
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let c = inner[i];
        if c != b'\\' {
            out.push(c);
            i += 1;
            continue;
        }
        let n = *inner.get(i + 1)?;
        let (byte, used) = match n {
            b'a' => (0x07, 2),
            b'b' => (0x08, 2),
            b'f' => (0x0c, 2),
            b'n' => (b'\n', 2),
            b'r' => (b'\r', 2),
            b't' => (b'\t', 2),
            b'v' => (0x0b, 2),
            b'\\' => (b'\\', 2),
            b'"' => (b'"', 2),
            b'0'..=b'3' => {
                let d = inner.get(i + 1..i + 4)?;
                if !d.iter().all(|x| (b'0'..=b'7').contains(x)) {
                    return None;
                }
                ((d[0] - b'0') * 64 + (d[1] - b'0') * 8 + (d[2] - b'0'), 4)
            }
            _ => return None,
        };
        out.push(byte);
        i += used;
    }
    Some(out)
}

#[derive(Debug, Serialize)]
pub struct SparseSetResult {
    /// Present when the repository uses Git LFS (see `lfs_after_checkout`).
    pub lfs: Option<LfsReport>,
}

/// Set the sparse folder selection (replaces the current set) and materialize:
/// `sparse-checkout init --cone` → `sparse-checkout set <dirs>` → `checkout <branch>`,
/// then the LFS step for what is now checked out.
pub async fn sparse_set(repo_path: &str, dirs: Vec<String>, with_lfs: bool) -> Result<SparseSetResult, AppError> {
    if dirs.is_empty() {
        return Err(AppError::Config("Select at least one folder".into()));
    }
    let repo = PathBuf::from(repo_path);

    run_git(Some(&repo), &["sparse-checkout", "init", "--cone"]).await?;

    let mut args: Vec<&str> = vec!["sparse-checkout", "set"];
    args.extend(dirs.iter().map(String::as_str));
    run_git(Some(&repo), &args).await?;

    // Materialize the working tree (no-op if already checked out).
    let branch = run_git(Some(&repo), &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    run_git(Some(&repo), &["checkout", &branch]).await?;

    // No clone is in flight here; the page shows its own "Checking out…".
    Ok(SparseSetResult { lfs: lfs_after_checkout(&repo, with_lfs, None).await })
}

/// Add folders to an existing sparse checkout: `git sparse-checkout add <dirs>`.
pub async fn sparse_add(repo_path: &str, dirs: Vec<String>) -> Result<(), AppError> {
    if dirs.is_empty() {
        return Err(AppError::Config("Select at least one folder to add".into()));
    }
    let repo = PathBuf::from(repo_path);
    let mut args: Vec<&str> = vec!["sparse-checkout", "add"];
    args.extend(dirs.iter().map(String::as_str));
    run_git(Some(&repo), &args).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both clone-side runners are the one runner: the plain one entirely,
    /// the progress reader through its arguments and environment.
    #[tokio::test]
    async fn the_clone_runners_go_through_gitcmd() {
        use crate::git_exec::test_support::{assert_hardened, SHOW_ENV};
        let dir = std::env::temp_dir();
        assert_hardened(&run_git(Some(&dir), &SHOW_ENV).await.expect("git runs"));
        let cfg = [SHOW_ENV[1].to_string()];
        let env = run_git_streaming(Some(&dir), &cfg, &[SHOW_ENV[2]], None, true, &|_| {}).await.expect("git runs");
        assert_hardened(&env);
    }

    #[test]
    fn progress_counts_are_the_finished_share() {
        assert_eq!(percent_of(0, 8), Some(0));
        assert_eq!(percent_of(3, 8), Some(37));
        assert_eq!(percent_of(9, 8), Some(100), "a count that overshoots is capped");
        assert_eq!(percent_of(1, 0), None, "nothing to divide by");
        let p = CloneProgress::submodules(3, 8, Some("libs/sub"));
        assert_eq!((p.phase.as_str(), p.done, p.total, p.percent, p.detail.as_deref()), ("Submodules", Some(3), Some(8), Some(37), Some("libs/sub")));
        let last = CloneProgress::submodules(8, 8, None);
        assert_eq!((last.done, last.percent, last.detail), (Some(8), Some(100), None));
        let lfs = crate::lfs::parse_progress_line("download 2/5 1048576/4194304 media/video/intro.mp4").expect("parsed");
        let p = CloneProgress::large_files(&lfs);
        assert_eq!((p.phase.as_str(), p.done, p.total, p.percent), ("Large files", Some(2), Some(5), Some(40)));
        assert_eq!(p.bytes.as_deref(), Some("1.0 MiB of 4.0 MiB"));
        assert_eq!(p.detail.as_deref(), Some("media/video/intro.mp4"));
        // Left out of the JSON when there is nothing in flight; still read when present.
        let json = serde_json::to_string(&CloneProgress::large_files_counted(0, 3)).unwrap();
        assert!(!json.contains("detail"), "{}", json);
        let back: CloneProgress = serde_json::from_str(r#"{"phase":"Submodules","percent":37,"done":3,"total":8,"bytes":null,"rate":null,"detail":"libs/sub"}"#).unwrap();
        assert_eq!(back.detail.as_deref(), Some("libs/sub"));
    }

    #[test]
    fn a_name_git_quoted_is_read_back_whole() {
        assert_eq!(unquote_c_style(b"\"caf\\303\\251\""), Some("café".as_bytes().to_vec()));
        assert_eq!(unquote_c_style(b"\"say\\\"hi\""), Some(b"say\"hi".to_vec()));
        assert_eq!(unquote_c_style(b"\"a\\\\b\\tc\""), Some(b"a\\b\tc".to_vec()));
        assert_eq!(unquote_c_style(b"plain"), None, "not quoted");
        assert_eq!(unquote_c_style(b"\"bad\\q\""), None, "an unknown escape");
        assert_eq!(unquote_c_style(b"\"cut\\30\""), None, "a short octal");
    }

    #[test]
    fn a_profile_looks_up_repositories_with_its_own_accounts_only() {
        assert_eq!(account_keys("p1", Some("octo")), vec!["profile:p1".to_string(), "gh:octo".to_string()]);
        assert_eq!(account_keys("p1", None), vec!["profile:p1".to_string()]);
        assert_eq!(account_keys("p1", Some("  ")), vec!["profile:p1".to_string()]);
        assert!(accounts_of(None).is_empty(), "no profile: no account's token at all");
    }

    #[test]
    fn the_time_left_comes_from_the_hosts_size_or_from_the_objects() {
        // 10 MiB of a 1.2 GB repository at 2 MiB/s: the rest of it at that rate.
        let (est, eta) = download_estimate(Some("10.00 MiB"), Some("2.00 MiB/s"), Some(45), Some(1_258_291));
        assert_eq!(est, Some(1_258_291));
        assert_eq!(eta, Some(((1_258_291.0f64 * 1024.0 - 10.0 * 1024.0 * 1024.0) / (2.0 * 1024.0 * 1024.0)).round() as u64));
        assert!((600..=612).contains(&eta.unwrap()), "about ten minutes: {:?}", eta);
        // No size from the host: 45% of the objects stand in for 45% of the bytes.
        let (est, eta) = download_estimate(Some("10.00 MiB"), Some("2.00 MiB/s"), Some(45), None);
        assert_eq!(est, None);
        assert_eq!(eta, Some(6), "10 MiB is 45%, so about 12.2 MiB are left: 6 s at 2 MiB/s");
        // Too early to say from the objects alone.
        assert_eq!(download_estimate(Some("1.00 MiB"), Some("2.00 MiB/s"), Some(2), None), (None, None));
        // A size the download has already passed was wrong: dropped, and the objects answer instead.
        let (est, eta) = download_estimate(Some("2.00 GiB"), Some("1.00 MiB/s"), Some(50), Some(1_258_291));
        assert_eq!(est, None);
        assert_eq!(eta, Some(2048));
        // Nothing to divide by, or nothing arrived: no time is claimed.
        assert_eq!(download_estimate(Some("10.00 MiB"), None, Some(45), Some(1_258_291)), (Some(1_258_291), None));
        assert_eq!(download_estimate(Some("10.00 MiB"), Some("0 bytes/s"), Some(45), None), (None, None));
        assert_eq!(download_estimate(None, Some("2.00 MiB/s"), Some(45), Some(1_258_291)), (None, None));
        // Every unit git prints.
        assert_eq!(git_size_bytes("512 bytes"), Some(512.0));
        assert_eq!(git_size_bytes("1.50 KiB"), Some(1536.0));
        assert_eq!(git_size_bytes("3.00 GiB"), Some(3.0 * 1024.0 * 1024.0 * 1024.0));
        assert_eq!(git_size_bytes("2.00 MiB/s"), Some(2.0 * 1024.0 * 1024.0));
        assert_eq!(git_size_bytes("fast"), None);
        // On the line itself: a download carries both, any other phase neither.
        let p = with_estimate(parse_clone_progress("Receiving objects:  45% (556/1234), 10.00 MiB | 2.00 MiB/s").unwrap(), Some(1_258_291));
        assert_eq!((p.estimate_kb, p.eta_secs.is_some()), (Some(1_258_291), true));
        let p = with_estimate(parse_clone_progress("Resolving deltas:  45% (5/11)").unwrap(), Some(1_258_291));
        assert_eq!((p.estimate_kb, p.eta_secs), (None, None));
        // Left out of the JSON when there is nothing to say; read when present.
        let json = serde_json::to_string(&CloneProgress::submodules(3, 8, Some("libs/sub"))).unwrap();
        assert!(!json.contains("eta_secs") && !json.contains("estimate_kb"), "{}", json);
        let back: CloneProgress = serde_json::from_str(r#"{"phase":"Submodules","percent":37,"done":3,"total":8,"bytes":"10.00 MiB","rate":"2.00 MiB/s","detail":"libs/sub","estimate_kb":1258291,"eta_secs":603}"#).unwrap();
        assert_eq!((back.estimate_kb, back.eta_secs), (Some(1_258_291), Some(603)));
    }

    #[test]
    fn clone_progress_lines_are_read_in_every_shape_git_prints() {
        let p = parse_clone_progress("Receiving objects:  45% (556/1234), 10.00 MiB | 2.00 MiB/s").unwrap();
        assert_eq!(p.phase, "Receiving objects");
        assert_eq!((p.percent, p.done, p.total), (Some(45), Some(556), Some(1234)));
        assert_eq!(p.bytes.as_deref(), Some("10.00 MiB"));
        assert_eq!(p.rate.as_deref(), Some("2.00 MiB/s"));
        let p = parse_clone_progress("remote: Compressing objects:   7% (12/160)").unwrap();
        assert_eq!((p.phase.as_str(), p.percent), ("Compressing objects", Some(7)));
        let p = parse_clone_progress("Resolving deltas: 100% (900/900), done.").unwrap();
        assert_eq!((p.percent, p.done, p.total, p.bytes), (Some(100), Some(900), Some(900), None));
        let p = parse_clone_progress("remote: Enumerating objects: 1234, done.").unwrap();
        assert_eq!((p.phase.as_str(), p.total), ("Enumerating objects", Some(1234)));
        assert!(parse_clone_progress("Cloning into 'x'...").is_none());
        assert!(parse_clone_progress("fatal: repository not found").is_none());
    }

    fn prof(id: &str, dirs: &[&str], default: bool) -> Profile {
        Profile {
            id: id.into(),
            name: id.into(),
            git_name: id.into(),
            git_email: format!("{id}@example.com"),
            ssh_key_path: Some(format!("/keys/{id}")),
            is_default: default,
            allow_push: true,
            signing_enabled: false,
            remote_patterns: Vec::new(),
            github_login: None,
            directories: dirs.iter().map(|d| d.to_string()).collect(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            push_over_ssh: None,
        }
    }

    #[test]
    fn destination_folder_decides_the_profile_not_its_parent() {
        // company owns /w/Work; personal owns a more specific path inside it
        let profs = vec![
            prof("personal", &["/w/Work/side-project"], true),
            prof("company", &["/w/Work"], false),
        ];
        let pick = |p: &str| resolve_profile(p, &profs).map(|x| x.id.clone());
        assert_eq!(pick("/w/Work/new-repo").as_deref(), Some("company"));
        assert_eq!(pick("/w/Work/side-project").as_deref(), Some("personal"));
        // not covered by anything -> the default profile, like git's global config
        assert_eq!(pick("/w/Elsewhere/repo").as_deref(), Some("personal"));
        // a name that merely shares a prefix is not "inside"
        assert_eq!(pick("/w/Work2/repo").as_deref(), Some("personal"));
    }

    #[test]
    fn rejects_folder_names_that_escape_the_parent() {
        let dir = std::env::temp_dir();
        let d = dir.to_string_lossy();
        for bad in ["..", ".", "a/b", "..\\x"] {
            assert!(
                clone_destination("git@github.com:o/r.git", &d, Some(bad)).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn clone_errors_say_what_to_do() {
        let e = explain_clone_error(
            "ERROR: Repository not found.\nfatal: Could not read from remote repository.",
            Some("/Users/me/.ssh/id_ed25519_company"),
            "git@github.com:acme/api.git",
        );
        assert!(e.contains("id_ed25519_company"), "{e}");
        assert!(e.contains("Clone as"), "{e}");

        let e = explain_clone_error("git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.", None, "git@github.com:o/r.git");
        assert!(e.contains("isn't registered"), "publickey must win over the generic message: {e}");
        assert!(!e.contains("certificate"), "plain git@ links must not talk about certificates: {e}");

        let e = explain_clone_error("Host key verification failed.", None, "git@github.com:o/r.git");
        assert!(e.contains("ssh -T git@github.com"), "{e}");

        let e = explain_clone_error("fatal: could not read Username for 'https://github.com': terminal prompts disabled", None, "https://github.com/o/r");
        assert!(e.contains("SSH URL"), "{e}");

        assert_eq!(explain_clone_error("fatal: something else", None, "git@github.com:o/r.git"), "fatal: something else");
    }

    #[test]
    fn a_network_failure_is_not_blamed_on_the_key_or_the_account() {
        let tail = "\nfatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights\nand the repository exists.";
        // ssh ends a timeout with the very line it ends a refusal with.
        let timed_out = format!("ssh: connect to host github.com port 22: Operation timed out{}", tail);
        for url in ["git@github.com:acme/api.git", "org-1234567@github.com:AcmeOrg/widgets.git"] {
            let e = explain_clone_error(&timed_out, Some("/Users/me/.ssh/id_company"), url);
            assert!(e.contains("could not be reached"), "{e}");
            assert!(e.contains("Nothing is wrong with the key or the account"), "{e}");
            assert!(e.contains("ssh.github.com") && e.contains("Port 443"), "the way round a blocked port 22: {e}");
            assert!(!e.contains("can't see this repository") && !e.contains("isn't registered"), "not an access problem: {e}");
            assert!(!e.contains("SSH certificates"), "nor a certificate one: {e}");
            assert!(e.contains("git said: ssh: connect to host github.com port 22: Operation timed out"), "git's own words stay: {e}");
        }
        // Linux's wording, another host: no GitHub-only advice.
        let e = explain_clone_error(&format!("ssh: connect to host git.example.test port 22: Connection timed out{}", tail), None, "git@git.example.test:o/r.git");
        assert!(e.contains("could not be reached") && e.contains("block port 22") && !e.contains("ssh.github.com"), "{e}");
        let e = explain_clone_error(&format!("ssh: connect to host git.example.test port 2222: Connection refused{}", tail), None, "ssh://git@git.example.test:2222/o/r.git");
        assert!(e.contains("could not be reached") && !e.contains("block port 22"), "port 2222 is not port 22: {e}");
        // The name could not be looked up — both systems' wording.
        for stderr in [
            format!("ssh: Could not resolve hostname github.com: nodename nor servname provided, or not known{}", tail),
            format!("ssh: Could not resolve hostname github.com: Name or service not known{}", tail),
            "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host: github.com".to_string(),
        ] {
            let e = explain_clone_error(&stderr, None, "git@github.com:o/r.git");
            assert!(e.contains("could not look up the server's name"), "{e}");
            assert!(!e.contains("can't see this repository"), "{e}");
        }
        // HTTPS that never connects.
        let e = explain_clone_error("fatal: unable to access 'https://github.com/o/r.git/': Failed to connect to github.com port 443 after 75003 ms: Couldn't connect to server", None, "https://github.com/o/r.git");
        assert!(e.contains("could not be reached") && !e.contains("port 22"), "{e}");
        // Dropped part way through the download.
        for stderr in [
            "fetch-pack: unexpected disconnect while reading sideband packet\nfatal: early EOF\nfatal: fetch-pack: invalid index-pack output",
            "Connection to github.com closed by remote host.\nfatal: the remote end hung up unexpectedly\nfatal: early EOF\nfatal: index-pack failed",
            "error: RPC failed; curl 92 HTTP/2 stream 5 was not closed cleanly: CANCEL (err 8)\nerror: 6017 bytes of body are still expected\nfetch-pack: unexpected disconnect while reading sideband packet",
            "client_loop: send disconnect: Broken pipe\nfatal: the remote end hung up unexpectedly",
            "Timeout, server github.com not responding.\nfatal: the remote end hung up unexpectedly",
        ] {
            let e = explain_clone_error(stderr, None, "org-1234567@github.com:AcmeOrg/widgets.git");
            assert!(e.contains("The connection dropped while data was moving"), "{e}");
            assert!(e.contains("git starts the transfer over"), "{e}");
            assert!(!e.contains("is kept"), "a dropped transfer keeps nothing: {e}");
            assert!(!e.contains("SSH certificates") && !e.contains("can't see this repository"), "{e}");
        }
        // A refusal is still a refusal, whatever else the output says.
        assert_eq!(network_failure("git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository."), None);
        assert_eq!(network_failure("ERROR: Repository not found.\nfatal: Could not read from remote repository."), None);
        assert_eq!(network_failure("git@github.com: Permission denied (publickey).\nfatal: the remote end hung up unexpectedly"), None);
        assert_eq!(network_failure("fatal: Authentication failed for 'https://github.com/o/r.git/'"), None);
        assert_eq!(network_failure("fatal: something else"), None);
        // A full disk ends with index-pack's lines too: named as the disk.
        let full = "fatal: write error: No space left on device\nfatal: fetch-pack: invalid index-pack output";
        assert_eq!(network_failure(full), None);
        let e = explain_clone_error(full, None, "git@github.com:o/r.git");
        assert!(e.contains("disk is full") && !e.contains("connection dropped"), "{e}");
        // The server answered with an HTTP error: not the network.
        let http = "error: RPC failed; HTTP 400 curl 22 The requested URL returned error: 400\nsend-pack: unexpected disconnect while reading sideband packet\nfatal: the remote end hung up unexpectedly";
        assert_eq!(network_failure(http), None);
        assert_eq!(network_failure("remote: fatal: pack exceeds maximum allowed size\nfatal: the remote end hung up unexpectedly"), None);
        // A genuine drop over HTTPS is still one.
        assert!(network_failure("error: RPC failed; curl 92 HTTP/2 stream 5 was not closed cleanly: CANCEL (err 8)\nfatal: early EOF").is_some());
    }

    #[test]
    fn refusals_outside_the_clone_page_point_at_the_folders_profile() {
        let refused = "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.";
        let e = explain_transport_error(refused, None, "git@github.com:o/r.git", false);
        assert!(e.contains("check in Profiles") && !e.contains("Clone as"), "{e}");
        assert!(e.contains("the SSH key ssh offered") && !e.contains("SSH key your"), "no doubled words without a key: {e}");
        let e = explain_transport_error("ERROR: The `Org' organization has enabled or enforced SAML SSO.", None, "git@github.com:o/r.git", false);
        assert!(e.contains("try again") && !e.contains("clone again"), "{e}");
        let e = explain_transport_error("ERROR: Repository not found.\nfatal: Could not read from remote repository.", Some("/k/id_work"), "git@github.com:o/r.git", false);
        assert!(e.contains("id_work") && e.contains("check in Profiles") && !e.contains("Clone as"), "{e}");
        // The Clone page keeps its own words.
        let e = explain_clone_error(refused, None, "git@github.com:o/r.git");
        assert!(e.contains("Clone as"), "{e}");
    }

    #[test]
    fn same_repository_across_url_forms() {
        let ssh = "org-1234567@github.com:AcmeOrg/widgets.git";
        assert!(same_github_repo("https://github.com/AcmeOrg/widgets.git", ssh));
        assert!(same_github_repo("https://github.com/acmeorg/WIDGETS", ssh), "GitHub names are case-insensitive");
        assert!(same_github_repo("git@github.com:AcmeOrg/widgets.git", ssh));
        assert!(!same_github_repo("https://github.com/AcmeOrg/widgets-docs.git", ssh));
        assert!(!same_github_repo("https://gitlab.com/AcmeOrg/widgets.git", ssh));
        assert!(is_http_url(" HTTPS://github.com/o/r"));
        assert!(!is_http_url(ssh));
    }

    #[test]
    fn org_certificate_links_are_recognised() {
        assert!(uses_org_certificate_url("org-1234567@github.com:AcmeOrg/widgets.git"));
        assert!(uses_org_certificate_url("ssh://org-1@github.com/o/r.git"));
        assert!(!uses_org_certificate_url("git@github.com:o/r.git"));
        assert!(!uses_org_certificate_url("https://github.com/o/r"));
        assert!(!uses_org_certificate_url("org-1@gitlab.com:o/r.git"));
        // the folder name still comes from the repo, not the org-… user
        assert_eq!(repo_name_from_url("org-1234567@github.com:AcmeOrg/widgets.git").as_deref(), Some("widgets"));
        assert_eq!(
            crate::repo_scan::parse_github_owner_name("org-1234567@github.com:AcmeOrg/widgets.git"),
            Some(("AcmeOrg".into(), "widgets".into()))
        );
    }

    #[test]
    fn refusals_on_certificate_orgs_mention_certificates() {
        let url = "org-1234567@github.com:AcmeOrg/widgets.git";
        let key = Some("/Users/me/.ssh/id_company");
        for stderr in [
            "org-1234567@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
            "ERROR: Repository not found.\nfatal: Could not read from remote repository.",
        ] {
            let e = explain_clone_error(stderr, key, url);
            assert!(e.contains("SSH certificates"), "{e}");
            assert!(e.contains("id_company-cert.pub"), "{e}");
        }
    }

    #[test]
    fn saml_sso_error_points_at_configure_sso() {
        let e = explain_clone_error(
            "ERROR: The 'AcmeOrg' organization has enabled or enforced SAML SSO. To access this repository, you must use the HTTPS remote with a personal access token or SSH with an SSH key and passphrase that has been authorized for this organization.\nfatal: Could not read from remote repository.",
            Some("/Users/me/.ssh/id_company"),
            "org-1234567@github.com:AcmeOrg/widgets.git",
        );
        assert!(e.contains("Configure SSO"), "{e}");
        assert!(!e.contains("SSH certificates"), "SSO is its own fix: {e}");
    }

    #[test]
    fn ssh_command_matches_what_gitconfig_writes() {
        assert_eq!(
            ssh_command_for(r"C:\Users\me\.ssh\id_key"),
            "ssh -i C:/Users/me/.ssh/id_key -o IdentitiesOnly=yes"
        );
    }

    #[test]
    fn derives_repo_name_from_common_url_forms() {
        assert_eq!(
            repo_name_from_url("git@github.com:acme/widgets.git").as_deref(),
            Some("widgets")
        );
        assert_eq!(
            repo_name_from_url("https://github.com/acme/widgets.git").as_deref(),
            Some("widgets")
        );
        assert_eq!(
            repo_name_from_url("https://github.com/o/repo/").as_deref(),
            Some("repo")
        );
        assert_eq!(repo_name_from_url(""), None);
    }
}
