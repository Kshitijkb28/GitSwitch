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
