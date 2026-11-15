//! Path helpers that behave the same on Windows and Unix.
//!
//! Windows paths use `\`, but git (and gh, and our own stored config) freely
//! mix in `/`. Comparing raw strings therefore fails on Windows — and a `\`
//! inside a git-config value is an ESCAPE character, so `gitdir:` patterns must
//! be written with forward slashes there too.

/// Normalize for comparison: forward slashes, no trailing separator.
pub fn norm(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    let trimmed = slashed.trim_end_matches('/');
    if trimmed.is_empty() {
        slashed
    } else {
        trimmed.to_string()
    }
}

/// Is `child` the same directory as `parent`, or inside it?
pub fn is_within(child: &str, parent: &str) -> bool {
    let (c, p) = (norm(child), norm(parent));
    c == p || c.starts_with(&format!("{}/", p))
}

/// A relative path the repository itself supplies (a submodule's name or
/// path in `.gitmodules`) that can be joined onto one of our folders without
/// leaving it: plain names separated by `/`. Refused: `..` and `.` (and any
/// name of only dots and spaces, which Windows trims down to one of those),
/// a leading `/`, a drive (`C:`), a backslash (a separator on Windows), an
/// empty name (`a//b`) and NUL. One trailing `/` is allowed.
pub fn is_plain_relative(p: &str) -> bool {
    if p.is_empty() || p.starts_with('/') || p.contains(['\\', '\0']) {
        return false;
    }
    let p = p.strip_suffix('/').unwrap_or(p);
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return false;
    }
    p.split('/').all(|name| !name.is_empty() && !name.chars().all(|c| c == '.' || c == ' '))
}

/// `target` lies inside `base` and is not `base` itself, as the disk resolves
/// both (links, junctions and `..` followed). False when either cannot be
/// resolved: a path that cannot be checked is not inside.
pub fn strictly_inside(target: &std::path::Path, base: &std::path::Path) -> bool {
    match (std::fs::canonicalize(target), std::fs::canonicalize(base)) {
        (Ok(t), Ok(b)) => t != b && t.starts_with(&b),
        _ => false,
    }
}

/// A folder named by text the repository supplies (a saved state's part)
/// lies inside `base`: below it by plain names (no `..`), none of which is a
/// link or a junction as far as the folders exist. Below the first name not
/// made yet (a submodule that is not downloaded) it is still inside: there
/// is no link there to follow. Checked name by name, not by resolving the
/// whole path: resolving needs the disk to report a folder's final path,
/// which some Windows volumes (RAM disks, virtual drives) cannot do, and
/// every part there would then read as a link out. A name that cannot be
/// looked at for any other reason is not inside.
pub fn lies_inside(target: &std::path::Path, base: &std::path::Path) -> bool {
    use std::path::Component;
    let Ok(rel) = target.strip_prefix(base) else { return false };
    if rel.as_os_str().is_empty() || !base.is_dir() {
        return false;
    }
    let mut at = base.to_path_buf();
    for c in rel.components() {
        let Component::Normal(name) = c else { return false };
        at.push(name);
        match std::fs::symlink_metadata(&at) {
            // On Windows a junction is, like a symbolic link, a
            // name-surrogate reparse point: `is_symlink` is true for both.
            Ok(m) if m.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) => return false,
        }
    }
    true
}

/// For tests: `link` made a link to the folder `target` — a symbolic link on
/// macOS and Linux, a junction on Windows (a symbolic link needs developer
/// mode there; a junction does not).
#[cfg(test)]
pub(crate) fn test_link_dir(target: &std::path::Path, link: &std::path::Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("a symbolic link");
    #[cfg(windows)]
    {
        let ok = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(link).arg(target).output().map(|o| o.status.success()).unwrap_or(false);
        assert!(ok, "a junction {:?} -> {:?}", link, target);
    }
}

/// Last path component, whichever separator the platform used.
pub fn base_name(path: &str) -> String {
    norm(path)
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// The folder as the disk spells it: each component in the letter case its
/// parent directory lists it with. macOS and Windows disks ignore letter case,
/// so a folder typed as `~/documents/work` opens fine — but git matches a
/// `gitdir:` rule letter for letter against the real path of `.git`, so a rule
/// written in another case never applies to a repository there. Only letter
/// case changes: a symbolic link keeps its own name (it is listed through,
/// never resolved), and a path that is relative, missing, unreadable or
/// ambiguous (two entries differing only in case) comes back as given.
pub fn disk_spelling(dir: &str) -> String {
    use std::path::{Component, Path, PathBuf, Prefix};
    let path = Path::new(dir);
    if !path.is_absolute() || !path.exists() {
        return dir.to_string();
    }
    // Already the real path letter for letter (the usual case): nothing to
    // list. Otherwise a link or another letter case is in it somewhere.
    if real_form(dir).is_some_and(|r| r == norm(dir)) {
        return dir.to_string();
    }
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => match p.kind() {
                Prefix::Disk(l) => out.push(format!("{}:", (l as char).to_ascii_uppercase())),
                _ => out.push(p.as_os_str()),
            },
            Component::RootDir => out.push(c.as_os_str()),
            Component::Normal(name) => {
                let Ok(listing) = std::fs::read_dir(&out) else { return dir.to_string() };
                let names: Vec<std::ffi::OsString> = listing.filter_map(|e| e.ok().map(|e| e.file_name())).collect();
                if names.iter().any(|n| n == name) {
                    out.push(name);
                    continue;
                }
                let lower = name.to_string_lossy().to_lowercase();
                let mut same = names.iter().filter(|n| n.to_string_lossy().to_lowercase() == lower);
                match (same.next(), same.next()) {
                    (Some(one), None) => out.push(one),
                    _ => return dir.to_string(),
                }
            }
            Component::CurDir | Component::ParentDir => return dir.to_string(),
        }
    }
    let spelled = out.to_string_lossy().to_string();
    if norm(&spelled) == norm(dir) {
        return dir.to_string();
    }
    // Windows: keep the separators the person used.
    if cfg!(windows) && !dir.contains('\\') {
        return spelled.replace('\\', "/");
    }
    spelled
}

/// The folder with every symbolic link resolved, forward-slashed, without
/// Windows' verbatim prefix — the path git tries first when it matches a
/// `gitdir:` rule. None when the folder does not exist.
pub fn real_form(dir: &str) -> Option<String> {
    let real = norm(&std::fs::canonicalize(dir).ok()?.to_string_lossy());
    Some(match (real.strip_prefix("//?/UNC/"), real.strip_prefix("//?/")) {
        (Some(unc), _) => format!("//{}", unc),
        (None, Some(local)) => local.to_string(),
        (None, None) => real,
    })
}

/// What makes two folder entries one folder for git: the same letters once
/// each is spelled as the disk spells it, or the same directory once links
/// are resolved.
#[derive(Debug, Clone)]
pub struct FolderId {
    spelled: String,
    real: Option<String>,
}

impl FolderId {
    pub fn of(dir: &str) -> Self {
        FolderId { spelled: norm(&disk_spelling(dir)), real: real_form(dir) }
    }

    pub fn same(&self, other: &FolderId) -> bool {
        self.spelled == other.spelled || matches!((&self.real, &other.real), (Some(a), Some(b)) if a == b)
    }
}

/// A `gitdir:` pattern: always forward slashes (backslashes would be read as
/// escapes by git-config), always a trailing slash so the match is recursive.
pub fn gitdir_pattern(dir: &str) -> String {
    format!("{}/", norm(dir))
}

/// A git-config VALUE, written so that git reads back exactly this string.
///
/// Unquoted, git drops leading and trailing spaces, cuts the value at the
/// first `#` or `;` (a comment) and treats `\` as an escape — so a name like
/// `Ann #1` would be committed as `Ann`. Inside double quotes only `\\`,
/// `\"`, `\n` and `\t` are recognised; any other character after a `\` is a
/// "bad config line", which makes the WHOLE file unreadable for every git
/// command on the machine. Hence everything git could misread is escaped.
pub fn config_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The quoted part of a section header: `[includeIf "gitdir:<this>"]`. Only
/// `"` and `\` are special there, and a `\` before any other character is
/// silently dropped by git — which is why paths are forward-slashed first.
pub fn config_subsection(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Single-quoted for `sh -c`, which is how git runs `core.sshCommand` on
/// every platform (Git for Windows ships its own sh). A plain word is left
/// bare so the common case reads naturally.
pub fn shell_quote(v: &str) -> String {
    let plain = !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,".contains(c));
    if plain {
        v.to_string()
    } else {
        format!("'{}'", v.replace('\'', "'\\''"))
    }
}

/// The platform's data folder, which holds `com.gitswitch.app` (the profile
/// store, the per-profile include files, backups). On Windows `dirs` asks the
/// system for it and ignores HOME, so the verify suites — whose HOME is a
/// sandbox — would write their profiles into the person's real store; a
/// debug build honours `GITSWITCH_DATA_DIR` for them. A release build ignores it.
pub fn data_dir() -> Option<std::path::PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("GITSWITCH_DATA_DIR").filter(|d| !d.is_empty()) {
        return Some(dir.into());
    }
    dirs::data_dir()
}

/// The home folder `~/.ssh` is under, as `dirs` gives it (on Windows the
/// user profile folder, whatever HOME says). A debug build honours
/// `GITSWITCH_HOME`, for the same reason as `data_dir`.
pub fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("GITSWITCH_HOME").filter(|d| !d.is_empty()) {
        return Some(dir.into());
    }
    dirs::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_names_pass_as_a_path_the_repository_supplies() {
        for ok in ["lib", "libs/core", "vendor/x y", "lib/", "a.b/c..d", ".hidden/x"] {
            assert!(is_plain_relative(ok), "{ok} is a plain path");
        }
        for bad in ["", "..", "../../..", "a/../b", ".", "./a", "a/.", "...", ".. ", "a/. ./b", "/etc", "//x", "a//b", "C:", "c:/x", "C:x", "x:y", r"a\b", r"..\x", "a\0b", "/"] {
            assert!(!is_plain_relative(bad), "{bad:?} must be refused");
        }
    }

    #[test]
    fn inside_means_strictly_inside_as_the_disk_resolves_it() {
        let root = std::env::temp_dir().join(format!("gs-inside-{}", std::process::id()));
        let inner = root.join("repo").join(".git").join("modules").join("lib");
        std::fs::create_dir_all(&inner).unwrap();
        let repo = root.join("repo");
        assert!(strictly_inside(&inner, &repo));
        assert!(!strictly_inside(&repo, &repo), "the folder itself is not inside it");
        assert!(!strictly_inside(&repo.join(".git").join("modules").join("..").join("..").join(".."), &repo), "`..` is followed");
        assert!(!strictly_inside(&root, &repo));
        assert!(!strictly_inside(&repo.join("missing"), &repo), "what cannot be resolved is not inside");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_lies_inside_when_it_or_its_nearest_existing_ancestor_does() {
        let root = std::env::temp_dir().join(format!("gs-lies-inside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("lib")).unwrap();
        std::fs::create_dir_all(root.join("other").join("x")).unwrap();
        assert!(lies_inside(&repo.join("lib"), &repo));
        assert!(lies_inside(&repo.join("not-yet"), &repo), "a folder not made yet: the repository itself holds it");
        assert!(lies_inside(&repo.join("lib").join("deep").join("er"), &repo));
        assert!(!lies_inside(&repo, &repo), "the folder itself is not a part inside it");
        test_link_dir(&root.join("other"), &repo.join("out"));
        assert!(!lies_inside(&repo.join("out"), &repo), "a link (a junction on Windows) to a folder outside");
        assert!(!lies_inside(&repo.join("out").join("x"), &repo));
        assert!(!lies_inside(&repo.join("out").join("not-yet"), &repo), "a folder not made yet below such a link");
        assert!(!lies_inside(&root.join("other"), &repo));
        assert!(!lies_inside(&repo.join("lib").join("..").join("..").join("other"), &repo), "`..` is never inside");
        assert!(!lies_inside(&repo.join("..").join("repo").join("lib"), &repo));
        // A link that leads nowhere is still a link.
        test_link_dir(&root.join("gone"), &repo.join("dangling"));
        assert!(!lies_inside(&repo.join("dangling").join("x"), &repo));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_config_value_is_quoted_and_escaped_so_git_reads_back_the_same_string() {
        assert_eq!(config_value("Ann Example"), "\"Ann Example\"");
        // `#` and `;` start a comment when unquoted.
        assert_eq!(config_value("Ann #1; ok"), "\"Ann #1; ok\"");
        // The two characters that are special inside the quotes.
        assert_eq!(config_value(r#"Ann "The Hammer" O'Brien"#), r#""Ann \"The Hammer\" O'Brien""#);
        assert_eq!(config_value(r"C:\k"), r#""C:\\k""#);
        // Whitespace that git would otherwise trim or choke on.
        assert_eq!(config_value(" padded "), "\" padded \"");
        assert_eq!(config_value("a\nb\tc"), r#""a\nb\tc""#);
    }

    #[test]
    fn a_subsection_escapes_only_what_git_treats_specially_there() {
        assert_eq!(config_subsection("gitdir:/home/me/work/"), "gitdir:/home/me/work/");
        assert_eq!(config_subsection(r#"gitdir:/a/"b"/"#), r#"gitdir:/a/\"b\"/"#);
        assert_eq!(config_subsection(r"x\y"), r"x\\y");
    }

    #[test]
    fn shell_quoting_leaves_plain_paths_bare_and_protects_the_rest() {
        assert_eq!(shell_quote("/home/me/.ssh/id_ed25519"), "/home/me/.ssh/id_ed25519");
        assert_eq!(shell_quote("C:/Users/me/.ssh/id_ed25519"), "C:/Users/me/.ssh/id_ed25519");
        assert_eq!(shell_quote("/Users/me/My Keys/id"), "'/Users/me/My Keys/id'");
        assert_eq!(shell_quote("/k/o'brien/id"), r"'/k/o'\''brien/id'");
        assert_eq!(shell_quote("$HOME/id;rm"), "'$HOME/id;rm'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn normalizes_windows_and_unix_alike() {
        assert_eq!(norm(r"C:\Users\me\Work\"), "C:/Users/me/Work");
        assert_eq!(norm("/home/me/work/"), "/home/me/work");
        assert_eq!(norm("/"), "/");
    }

    #[test]
    fn containment_works_across_separator_styles() {
        // The exact case that broke Commit Audit + folder dedup on Windows.
        assert!(is_within(r"C:\Users\me\Work\repo", r"C:\Users\me\Work"));
        assert!(is_within(r"C:/Users/me/Work/repo", r"C:\Users\me\Work\"));
        assert!(is_within("/home/me/work/repo", "/home/me/work"));
        assert!(is_within("/home/me/work", "/home/me/work/"));
        // A sibling that merely shares a prefix must not match.
        assert!(!is_within(r"C:\Users\me\Workshop\repo", r"C:\Users\me\Work"));
        assert!(!is_within("/home/me/workshop", "/home/me/work"));
    }

    #[test]
    fn base_name_handles_both_separators() {
        assert_eq!(base_name(r"C:\Users\me\my-repo"), "my-repo");
        assert_eq!(base_name("/home/me/my-repo"), "my-repo");
        assert_eq!(base_name("/home/me/my-repo/"), "my-repo");
    }

    /// A temp folder under its real path, so links in the temp dir itself
    /// (macOS: /var -> /private/var) are not what the tests see.
    fn real_tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("gitswitch-paths-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("tmp");
        std::fs::canonicalize(&d).expect("canonical")
    }

    #[test]
    fn disk_spelling_corrects_letter_case_only() {
        let root = real_tmp("case");
        std::fs::create_dir_all(root.join("CaseWork").join("Inner")).expect("dirs");
        let real = root.join("CaseWork").join("Inner").to_string_lossy().to_string();
        let typed = root.join("casework").join("INNER").to_string_lossy().to_string();
        if std::path::Path::new(&typed).exists() {
            // A disk that ignores letter case (macOS, Windows): the disk's spelling.
            assert_eq!(norm(&disk_spelling(&typed)), norm(&real));
            #[cfg(windows)]
            {
                let lower_drive = format!("{}{}", real[..1].to_ascii_lowercase(), &real[1..]);
                assert_eq!(norm(&disk_spelling(&lower_drive)), norm(&real));
            }
        } else {
            // A disk that tells them apart: another folder, left alone.
            assert_eq!(disk_spelling(&typed), typed);
        }
        // Already right: returned exactly as given, separators and all.
        assert_eq!(disk_spelling(&real), real);
        let missing = root.join("not-there").join("x").to_string_lossy().to_string();
        assert_eq!(disk_spelling(&missing), missing);
        assert_eq!(disk_spelling("relative/casework"), "relative/casework");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_keeps_its_own_name_and_resolves_only_in_the_real_form() {
        let root = real_tmp("link");
        std::fs::create_dir_all(root.join("Real").join("CaseWork")).expect("dirs");
        std::os::unix::fs::symlink(root.join("Real"), root.join("link")).expect("symlink");
        let via_link = root.join("link").join("casework").to_string_lossy().to_string();
        let real = norm(&root.join("Real").join("CaseWork").to_string_lossy());
        if std::path::Path::new(&via_link).exists() {
            assert_eq!(norm(&disk_spelling(&via_link)), norm(&root.join("link").join("CaseWork").to_string_lossy()));
        }
        assert_eq!(real_form(&root.join("link").join("CaseWork").to_string_lossy()), Some(real.clone()));
        // One folder named two ways is one folder; a sibling is not.
        let a = FolderId::of(&root.join("link").join("CaseWork").to_string_lossy());
        assert!(a.same(&FolderId::of(&real)));
        std::fs::create_dir_all(root.join("Real").join("Other")).expect("other");
        assert!(!a.same(&FolderId::of(&root.join("Real").join("Other").to_string_lossy())));
        // Two folders that do not exist yet are one only when spelled alike.
        assert!(FolderId::of("/no/such/Place").same(&FolderId::of("/no/such/Place/")));
        assert!(!FolderId::of("/no/such/Place").same(&FolderId::of("/no/such/place")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gitdir_pattern_never_emits_backslashes() {
        // A backslash here would be an escape sequence inside git config.
        let p = gitdir_pattern(r"C:\Users\me\Work");
        assert_eq!(p, "C:/Users/me/Work/");
        assert!(!p.contains('\\'));
        assert!(p.ends_with('/'), "trailing slash makes the match recursive");
        assert_eq!(gitdir_pattern("/home/me/work/"), "/home/me/work/");
    }
}
