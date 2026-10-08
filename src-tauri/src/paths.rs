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

#[cfg(test)]
mod tests {
    use super::*;

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
