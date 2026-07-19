//! Session auto-naming heuristics.
//!
//! Produces a short, human-readable title for a session from the first user
//! message (mirrors the Python `CodingSession` title heuristic).

use std::path::Path;

/// Derive a short session title from the first user message plus the working
/// directory.
///
/// Heuristic:
/// 1. take the first non-empty line of `first_user`, trimmed;
/// 2. keep at most the first 8 characters (Unicode-aware), appending an
///    ellipsis when truncated;
/// 3. if the input is empty/whitespace, fall back to the directory's
///    `file_name()` (or `"session"` as a last resort).
pub fn auto_title(first_user: &str, cwd: &Path) -> String {
    let first_line = first_user
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");

    if first_line.is_empty() {
        return cwd
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("session")
            .to_string();
    }

    let chars: Vec<char> = first_line.chars().collect();
    if chars.len() <= 8 {
        chars.iter().collect()
    } else if chars[7].is_whitespace() {
        // Cut lands on a word boundary — trim the trailing space, no ellipsis.
        chars
            .iter()
            .take(8)
            .collect::<String>()
            .trim_end()
            .to_string()
    } else {
        format!("{}…", chars.iter().take(8).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_takes_eight_chars() {
        assert_eq!(
            auto_title("  Fix bug in parser\nmore", Path::new("/tmp/foo")),
            "Fix bug"
        );
    }

    #[test]
    fn short_line_kept_verbatim() {
        assert_eq!(auto_title("hello", Path::new("/tmp/foo")), "hello");
    }

    #[test]
    fn empty_falls_back_to_cwd() {
        assert_eq!(auto_title("", Path::new("/tmp/foo")), "foo");
    }

    #[test]
    fn whitespace_only_falls_back_to_cwd() {
        assert_eq!(auto_title("   \n  \n", Path::new("/tmp/bar")), "bar");
    }

    #[test]
    fn last_resort_when_cwd_missing() {
        assert_eq!(auto_title("", Path::new("")), "session");
    }
}
