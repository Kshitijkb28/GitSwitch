//! A record of every git command this app ran, so a person can see it.
//!
//! The terminal drawer shows you a shell, but a shell only knows what *you*
//! typed. Everything the app does on your behalf — the fetch behind a button,
//! the eighteen reads behind a status refresh — was invisible. This is the
//! other half: the exact argv, the exit code, how long it took, and what git
//! said, for each one.
//!
//! Two decisions worth stating.
//!
//! **In memory, bounded.** `push_lock::log_event` writes JSONL and rewrites the
//! whole file on every append; that is right for a handful of lock events and
//! wrong here, where a single status refresh adds a dozen entries. A
//! `VecDeque` capped at `KEEP` drops the oldest instead.
//!
//! **Redacted on the way in, never on the way out.** A token can reach this log
//! three ways: a person pastes a credentialed URL into Clone, which lands in
//! argv verbatim; git quotes that URL back in stderr when authentication
//! fails; and `git config --show-origin --list` prints whatever the user's own
//! config holds. Scrubbing at the display layer would mean the secret still sat
//! in memory and in every future accessor, so `record` scrubs before storing.

use serde::Serialize;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Entries kept. A status refresh is ~18 commands and a repository scan is one
/// per repository, so this is minutes of work, not hours.
const KEEP: usize = 1000;
/// Output kept per entry. Enough for a real error; short of a `git log` dump.
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub at: String,
    pub repo: String,
    /// Exactly what ran, already redacted. `argv[0]` is the program.
    pub argv: Vec<String>,
    /// `-1` when git could not be started, `-9` when it was killed on a timeout.
    pub exit_code: i32,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    /// A read rather than a change. The UI hides these by default: they are the
    /// overwhelming majority and none of them did anything.
    pub is_read: bool,
    /// Output was longer than `MAX_OUTPUT` and was cut.
    pub truncated: bool,
}

fn log() -> &'static Mutex<VecDeque<Entry>> {
    static L: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn next_id() -> u64 {
    static N: OnceLock<Mutex<u64>> = OnceLock::new();
    let n = N.get_or_init(|| Mutex::new(0));
    let Ok(mut v) = n.lock() else { return 0 };
    *v += 1;
    *v
}

/// git subcommands that only look. Everything else is treated as a change,
/// because being wrong in that direction merely shows a row that could have
/// been hidden, while the reverse hides something that altered the repository.
const READS: &[&str] = &[
    "status", "rev-parse", "ls-files", "ls-tree", "cat-file", "log", "diff",
    "show", "var", "for-each-ref", "rev-list", "branch", "remote", "grep",
    "describe", "name-rev", "check-ref-format", "count-objects", "version",
];

/// Is this a read? `config` is a read only when it is getting something.
pub fn is_read(argv: &[String]) -> bool {
    let mut it = argv.iter().skip(1).map(|s| s.as_str());
    // Skip the global flags that come before the subcommand.
    let mut verb = None;
    while let Some(a) = it.next() {
        match a {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path" => {
                it.next();
            }
            _ if a.starts_with('-') => continue,
            _ => {
                verb = Some(a);
                break;
            }
        }
    }
    let Some(verb) = verb else { return true };
    if verb == "config" {
        // `config --get x` reads; `config x y` writes.
        return argv.iter().any(|a| a.starts_with("--get") || a == "--list" || a == "-l");
    }
    if verb == "remote" {
        return argv.iter().any(|a| a == "get-url") || argv.len() <= 3;
    }
    if verb == "branch" {
        // A bare `branch` or `branch --list` lists; anything else changes a ref.
        return !argv.iter().any(|a| {
            matches!(a.as_str(), "-d" | "-D" | "-m" | "-M" | "-c" | "-C" | "--delete" | "--move")
        });
    }
    READS.contains(&verb)
}

/// Replace anything that looks like a credential with `***`.
///
/// Deliberately blunt. A log is something people screenshot and paste into an
/// issue, so a false positive costs a `***` in a place nobody minded, while a
/// false negative costs a live token. Every pattern below is paired with a test
/// asserting that ordinary text survives it.
pub fn redact(s: &str) -> String {
    let mut out = redact_urls(s);
    out = redact_prefixed_tokens(&out);
    out = redact_headers(&out);
    out = redact_credential_lines(&out);
    out
}

/// `password=…` (what `git credential fill` prints) and `token=…` lines, the
/// value gone whatever its shape — a stored password has no prefix to match.
fn redact_credential_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        // Everything the credential protocol carries besides protocol, host,
        // path and username is a secret or describes one: password,
        // oauth_refresh_token, credential, authtype… — judged by the key's
        // shape, so a new key git adds later is masked too.
        let key = lower.split('=').next().unwrap_or("");
        let secret = lower.contains('=')
            && !key.is_empty()
            && !key.contains(' ')
            && (matches!(key, "password" | "credential" | "authtype" | "password_expiry_utc") || key.contains("token") || key.contains("secret"));
        if secret {
            let (k, _) = trimmed.split_once('=').unwrap_or((trimmed, ""));
            out.push_str(&line[..line.len() - trimmed.len()]);
            out.push_str(k);
            out.push_str("=***");
        } else {
            out.push_str(line);
        }
    }
    out
}

/// `scheme://user:secret@host` and `scheme://secret@host`, anywhere in a
/// string — not just at the start, because git quotes URLs back mid-sentence.
fn redact_urls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let (before, after) = rest.split_at(i + 3);
        out.push_str(before);
        // The authority ends at the first delimiter; credentials end at '@'.
        let end = after
            .find(['/', ' ', '\'', '"', '\n'])
            .unwrap_or(after.len());
        let authority = &after[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***@");
                out.push_str(&authority[at + 1..]);
            }
            None => out.push_str(authority),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// GitHub's own token shapes. These are recognisable on sight and never appear
/// in ordinary prose.
pub(crate) const TOKEN_PREFIXES: &[&str] = &["ghp_", "gho_", "ghs_", "ghu_", "ghr_", "github_pat_"];

fn redact_prefixed_tokens(s: &str) -> String {
    let mut out = s.to_string();
    for prefix in TOKEN_PREFIXES {
        // A cursor rather than a restart: a bare prefix in prose ("ghp_...")
        // must be stepped past, or a real token later on the same line is
        // never reached.
        let mut from = 0;
        while let Some(rel) = out[from..].find(prefix) {
            let i = from + rel;
            let tail = &out[i + prefix.len()..];
            let len = tail
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(tail.len());
            if len == 0 {
                from = i + prefix.len();
                continue;
            }
            out.replace_range(i..i + prefix.len() + len, "***");
            from = i + 3;
        }
    }
    out
}

/// `Authorization: Bearer x`, `http.extraheader=AUTHORIZATION: basic x`, and
/// the `-c` form of the same.
fn redact_headers(s: &str) -> String {
    let needles = ["authorization:", "extraheader="];
    // ASCII-only lowering: both needles are ASCII, and `to_lowercase` on
    // Unicode can change byte lengths ("İ" → 3 bytes), which would make the
    // offsets below slice `s` off a character boundary — a panic reachable
    // from any commit subject, since `log -1 --format=%s` runs every snapshot.
    let lower = s.to_ascii_lowercase();
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    for needle in needles {
        let mut from = 0;
        while let Some(rel) = lower[from..].find(needle) {
            let start = from + rel + needle.len();
            let tail = &s[start..];
            let end = tail.find('\n').unwrap_or(tail.len());
            if end > 0 {
                cuts.push((start, start + end));
            }
            from = start;
        }
    }
    if cuts.is_empty() {
        return s.to_string();
    }
    cuts.sort_unstable();
    let mut out = String::with_capacity(s.len());
    let mut at = 0;
    for (start, end) in cuts {
        if start < at {
            continue;
        }
        out.push_str(&s[at..start]);
        out.push_str(" ***");
        at = end;
    }
    out.push_str(&s[at..]);
    out
}

/// Cut long output at a line boundary. A line is kept whole or dropped, so a
/// credentialed URL can never be split with its secret half left on the kept
/// side; and this runs BEFORE redaction, so a multi-megabyte `ls-files` is
/// not scanned three times in full on every status refresh.
fn clip(s: &str) -> (&str, bool) {
    if s.len() <= MAX_OUTPUT {
        return (s, false);
    }
    // A character boundary first: slicing at a byte inside a multi-byte
    // character panics, and this runs on every command's output.
    let mut cut = MAX_OUTPUT;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let end = match s[..cut].rfind('\n') {
        Some(nl) if nl > 0 => nl,
        _ => cut,
    };
    (&s[..end], true)
}

/// Record one command. Never fails, never blocks on anything but its own lock,
/// and never holds that lock across an await — it is called from the hot path.
pub fn record(repo: &Path, argv: Vec<String>, exit_code: i32, dur: Duration, stdout: &str, stderr: &str) {
    let argv: Vec<String> = argv.iter().map(|a| redact(a)).collect();
    let read = is_read(&argv);
    let (stdout, cut_a) = clip(stdout);
    let (stderr, cut_b) = clip(stderr);
    let stdout = redact(stdout);
    let stderr = redact(stderr);
    let entry = Entry {
        id: next_id(),
        at: chrono::Utc::now().to_rfc3339(),
        repo: repo.to_string_lossy().to_string(),
        argv,
        exit_code,
        duration_ms: dur.as_millis().min(u64::MAX as u128) as u64,
        stdout,
        stderr,
        is_read: read,
        truncated: cut_a || cut_b,
    };
    let Ok(mut l) = log().lock() else { return };
    if l.len() >= KEEP {
        l.pop_front();
    }
    l.push_back(entry);
}

/// Everything newer than `since_id`, oldest first. `repo` filters to one
/// folder; `None` returns every repository's commands.
pub fn since(since_id: u64, repo: Option<&str>) -> Vec<Entry> {
    let Ok(l) = log().lock() else { return Vec::new() };
    l.iter()
        .filter(|e| e.id > since_id)
        .filter(|e| repo.is_none_or(|r| r.is_empty() || e.repo == r))
        .cloned()
        .collect()
}

/// Forget everything. The log is a convenience, not a record of account.
pub fn clear() {
    if let Ok(mut l) = log().lock() {
        l.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(|x| x.to_string()).collect()
    }

    /// The log is one global buffer and cargo runs tests in parallel, so the
    /// ones that write to it take turns. Each also uses its own repository
    /// path and filters by it, so a neighbour's entries can never be counted.
    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        static M: OnceLock<Mutex<()>> = OnceLock::new();
        M.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn a_credentialed_url_is_scrubbed_wherever_it_appears() {
        // In argv, as Clone puts it there.
        assert_eq!(
            redact("https://user:ghp_secret@github.com/o/r.git"),
            "https://***@github.com/o/r.git"
        );
        // Token-as-username, the other form GitHub documents.
        assert_eq!(redact("https://ghp_abc@github.com/o/r"), "https://***@github.com/o/r");
        // Mid-sentence, as git quotes it back on an auth failure.
        let said = redact("fatal: Authentication failed for 'https://u:p@github.com/o/r.git/'");
        assert!(said.contains("'https://***@github.com/o/r.git/'"), "{}", said);
        assert!(!said.contains("u:p"));
        // ssh:// too — the authority is the same shape.
        assert_eq!(redact("ssh://git:pw@host/x"), "ssh://***@host/x");
    }

    #[test]
    fn an_ordinary_url_is_left_exactly_as_it_was() {
        for benign in [
            "https://github.com/owner/repo.git",
            "git@github.com:owner/repo.git",
            "Cloning into 'repo'… done.",
            "see https://git-scm.com/docs/git-push for more",
            "remote: Compressing objects: 100% (12/12), done.",
        ] {
            assert_eq!(redact(benign), benign, "mangled a benign string");
        }
    }

    #[test]
    fn a_bare_token_is_scrubbed_but_a_word_that_merely_starts_the_same_is_not() {
        assert_eq!(redact("token ghp_16C7e42F292c6912E7710c838347Ae178B4a"), "token ***");
        assert_eq!(redact("github_pat_11ABC_xyz123"), "***");
        assert_eq!(redact("gho_aaa and ghs_bbb"), "*** and ***");
        // Not tokens: the prefix with nothing after it, and ordinary words.
        assert_eq!(redact("ghp_"), "ghp_");
        assert_eq!(redact("the ghost of a branch"), "the ghost of a branch");
        // `github_pat` without the trailing underscore is not the prefix, so an
        // ordinary filename that starts the same way is left alone.
        assert_eq!(redact("github_patch.diff"), "github_patch.diff");
    }

    #[test]
    fn a_unicode_case_change_before_a_header_cannot_panic_or_under_redact() {
        // "İ" lowercases to two code points; a byte offset taken from the
        // lowered copy used to slice the original off a character boundary.
        let out = redact("İ authorization: Bearer ghp_zzz\nnext");
        assert!(out.contains("İ"), "{}", out);
        assert!(!out.contains("ghp_zzz") && !out.contains("Bearer"), "{}", out);
        assert!(out.ends_with("next"), "{}", out);
        // The Kelvin sign is the other classic: its lowercase is ASCII 'k'.
        let out = redact("\u{212A}elvin authorization: x");
        assert!(out.contains("\u{212A}") && !out.ends_with("x"), "{}", out);
    }

    #[test]
    fn a_bare_prefix_in_prose_does_not_hide_a_real_token_after_it() {
        // Exactly the shape a hook's stderr takes: a rule, then a finding.
        let out = redact("rule: GitHub PAT (ghp_...)  found: ghp_16C7e42F292c6912E7710c838347Ae178B4a");
        assert!(!out.contains("ghp_16C7"), "{}", out);
        assert!(out.contains("(ghp_...)"), "the bare prefix in prose is left as it was: {}", out);
    }

    #[test]
    fn long_output_is_cut_on_a_line_so_a_secret_is_never_split_into_the_kept_half() {
        let _lock = exclusive();
        clear();
        // A line of filler, then a credentialed line straddling the cap.
        let filler = "x".repeat(MAX_OUTPUT - 20);
        let out = format!("{}\nhttps://user:ghp_SECRETSECRET@host/r.git\n", filler);
        record(Path::new("/split"), argv("git remote -v"), 0, Duration::from_millis(1), &out, "");
        let e = &since(0, Some("/split"))[0];
        assert!(e.truncated);
        assert!(!e.stdout.contains("SECRET"), "{}", &e.stdout[e.stdout.len().saturating_sub(80)..]);
        clear();
    }

    #[test]
    fn a_credential_helpers_answer_keeps_its_user_name_and_loses_the_password() {
        let out = redact("protocol=https\nhost=github.com\nusername=octocat\npassword=hunter2 with spaces\n");
        assert!(out.contains("username=octocat"), "{out}");
        assert!(out.contains("password=***") && !out.contains("hunter2"), "{out}");
        // A word that merely contains "password=" mid-line is left alone.
        assert_eq!(redact("the old password=field style"), "the old password=field style");
        // Every other secret-bearing key of the credential protocol.
        let out = redact("oauth_refresh_token=rt_abc\ncredential=xyz\nauthtype=bearer\nusername=u\nprotocol=https\n");
        assert!(out.contains("oauth_refresh_token=***") && out.contains("credential=***") && out.contains("authtype=***"), "{out}");
        assert!(out.contains("username=u") && out.contains("protocol=https"), "{out}");
    }

    #[test]
    fn output_cut_inside_a_multi_byte_character_does_not_panic() {
        let _lock = exclusive();
        clear();
        // 65535 ASCII bytes, then a 3-byte character straddling the cap.
        let mut s = "x".repeat(MAX_OUTPUT - 1);
        s.push('€');
        s.push_str("tail");
        record(Path::new("/utf8"), argv("git log"), 0, Duration::from_millis(1), &s, "");
        let e = &since(0, Some("/utf8"))[0];
        assert!(e.truncated && e.stdout.len() <= MAX_OUTPUT);
        clear();
    }

    #[test]
    fn an_authorization_header_keeps_its_name_and_loses_its_value() {
        let out = redact("http.extraheader=AUTHORIZATION: basic aGVsbG86d29ybGQ=");
        assert!(out.starts_with("http.extraheader="), "{}", out);
        assert!(!out.contains("aGVsbG8"), "{}", out);
        let out = redact("Authorization: Bearer ghp_xyz\nnext line survives");
        assert!(out.contains("next line survives"), "{}", out);
        assert!(!out.contains("Bearer ghp_xyz"), "{}", out);
        // A sentence merely mentioning the word is untouched.
        assert_eq!(redact("no authorization was needed"), "no authorization was needed");
    }

    #[test]
    fn a_read_is_told_apart_from_a_change() {
        assert!(is_read(&argv("git -C /r status --porcelain=v2")));
        assert!(is_read(&argv("git rev-parse HEAD")));
        assert!(is_read(&argv("git -c color.ui=never log --oneline")));
        assert!(is_read(&argv("git config --get user.email")));
        assert!(is_read(&argv("git remote get-url origin")));
        assert!(is_read(&argv("git branch --list")));

        assert!(!is_read(&argv("git commit -m hello")));
        assert!(!is_read(&argv("git push origin main")));
        assert!(!is_read(&argv("git -C /r fetch --all")));
        assert!(!is_read(&argv("git config user.email me@x.test")));
        assert!(!is_read(&argv("git remote set-url origin git@host:o/r")));
        assert!(!is_read(&argv("git branch -D old")));
        assert!(!is_read(&argv("git switch main")));
        assert!(!is_read(&argv("git stash push")));
    }

    #[test]
    fn a_flag_before_the_subcommand_does_not_hide_it() {
        // -c takes a value; the value must not be read as the subcommand.
        assert!(!is_read(&argv("git -c rebase.backend=merge rebase --continue")));
        assert!(!is_read(&argv("git -C /some/path push")));
    }

    #[test]
    fn the_log_keeps_the_newest_and_drops_the_oldest() {
        let _lock = exclusive();
        clear();
        for i in 0..(KEEP + 25) {
            record(
                Path::new("/cap"),
                argv(&format!("git status {}", i)),
                0,
                Duration::from_millis(1),
                "",
                "",
            );
        }
        let all = since(0, None);
        assert_eq!(all.len(), KEEP, "the cap holds");
        assert!(all[0].argv.last().unwrap() != "0", "the oldest was dropped");
        assert_eq!(all.last().unwrap().argv.last().unwrap(), &format!("{}", KEEP + 24));
        clear();
    }

    #[test]
    fn since_returns_only_what_is_new_and_can_be_filtered_to_one_repository() {
        let _lock = exclusive();
        clear();
        record(Path::new("/a"), argv("git status"), 0, Duration::from_millis(2), "", "");
        let first = since(0, Some("/a"));
        assert_eq!(first.len(), 1);
        let mark = first[0].id;
        record(Path::new("/b"), argv("git fetch"), 1, Duration::from_millis(3), "", "boom");
        assert_eq!(since(mark, None).len(), 1, "only the newer one");
        assert_eq!(since(0, Some("/a")).len(), 1);
        assert_eq!(since(0, Some("/b"))[0].exit_code, 1);
        assert_eq!(since(0, Some("/nowhere")).len(), 0);
        // An empty filter means every repository, so the UI can pass "".
        assert_eq!(since(0, Some("")).len(), 2);
        clear();
    }

    #[test]
    fn what_a_command_printed_is_recorded_and_redacted_too() {
        let _lock = exclusive();
        clear();
        record(
            Path::new("/said"),
            argv("git remote get-url origin"),
            0,
            Duration::from_millis(7),
            "https://u:ghp_zzz@github.com/o/r.git",
            "",
        );
        let e = &since(0, Some("/said"))[0];
        assert_eq!(e.stdout, "https://***@github.com/o/r.git");
        assert_eq!(e.duration_ms, 7);
        assert!(e.is_read);
        assert!(!e.truncated);
        clear();
    }

    #[test]
    fn output_too_long_to_read_is_cut_and_says_so() {
        let _lock = exclusive();
        clear();
        let huge = "x".repeat(MAX_OUTPUT + 500);
        record(Path::new("/big"), argv("git log"), 0, Duration::from_millis(1), &huge, "");
        let e = &since(0, Some("/big"))[0];
        assert!(e.truncated, "the cut is reported, not silent");
        assert!(e.stdout.len() <= MAX_OUTPUT);
        clear();
    }
}
