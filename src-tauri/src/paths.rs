/// Path helpers that behave the same on Windows and Unix.
///
/// Windows paths use `\`, but git (and gh, and our own stored config) freely
/// mix in `/`. Comparing raw strings therefore fails on Windows — and a `\`
/// inside a git-config value is an ESCAPE character, so `gitdir:` patterns must
/// be written with forward slashes there too.

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

/// A `gitdir:` pattern: always forward slashes (backslashes would be read as
/// escapes by git-config), always a trailing slash so the match is recursive.
pub fn gitdir_pattern(dir: &str) -> String {
    format!("{}/", norm(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

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
