//! Edits a working-tree file from the diff: a review comment inserted after
//! some lines, or new text for them, refused if the lines changed meanwhile.
use anyhow::{Result, ensure};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Comment,
    Replace,
}

/// The comment markers of a file type; plain `//` when the type is unknown.
pub fn comment_markers(path: &Path) -> (&'static str, &'static str) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "py" | "rb" | "sh" | "bash" | "zsh" | "fish" | "yaml" | "yml" | "toml" | "pl" | "r"
        | "nix" | "ex" | "exs" | "cmake" | "conf" | "ini" | "tf" | "ps1" => ("#", ""),
        "sql" | "lua" | "hs" | "elm" => ("--", ""),
        "html" | "htm" | "xml" | "md" | "markdown" | "vue" | "svelte" | "svg" => ("<!--", " -->"),
        "css" | "scss" | "less" => ("/*", " */"),
        "clj" | "cljs" | "lisp" | "el" | "scm" | "asm" => (";", ""),
        "vim" => ("\"", ""),
        _ if matches!(
            name.as_str(),
            "makefile" | "dockerfile" | "gemfile" | "rakefile"
        ) || name.starts_with(".env") =>
        {
            ("#", "")
        }
        _ => ("//", ""),
    }
}

/// Applies an edit to lines `start..=end` (1-based) of `content`, which must
/// still read `expected`. Keeps the file's line endings and final newline.
pub fn apply(
    content: &str,
    path: &Path,
    start: usize,
    end: usize,
    expected: &[String],
    kind: EditKind,
    text: &str,
) -> Result<String> {
    let eol = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let trailing = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(str::to_owned).collect();
    ensure!(
        start >= 1 && start <= end && end <= lines.len() && lines[start - 1..end] == *expected,
        "{} changed since it was shown; reload it and try again",
        path.display()
    );
    let new: Vec<String> = match kind {
        EditKind::Replace => {
            let mut new: Vec<String> = text.lines().map(str::to_owned).collect();
            if text.is_empty() {
                new.clear();
            }
            lines.splice(start - 1..end, new);
            return Ok(join(&lines, eol, trailing));
        }
        EditKind::Comment => {
            let anchor = &lines[end - 1];
            let indent = &anchor[..anchor.len() - anchor.trim_start().len()];
            let (open, close) = comment_markers(path);
            text.lines()
                .map(|line| {
                    format!("{indent}{open} {line}{close}")
                        .trim_end()
                        .to_owned()
                })
                .collect()
        }
    };
    lines.splice(end..end, new);
    Ok(join(&lines, eol, trailing))
}

fn join(lines: &[String], eol: &str, trailing: bool) -> String {
    let mut text = lines.join(eol);
    if trailing && !lines.is_empty() {
        text.push_str(eol);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "fn main() {\r\n    let a = 1;\r\n    run(a);\r\n}\r\n";

    fn lines(range: std::ops::RangeInclusive<usize>) -> Vec<String> {
        FILE.lines()
            .skip(range.start() - 1)
            .take(range.count())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn comments_follow_the_lines_in_the_file_syntax_and_indentation() {
        let path = Path::new("src/main.rs");
        let edited = apply(
            FILE,
            path,
            2,
            3,
            &lines(2..=3),
            EditKind::Comment,
            "Why?\nRename a",
        )
        .unwrap();
        assert_eq!(
            edited,
            "fn main() {\r\n    let a = 1;\r\n    run(a);\r\n    // Why?\r\n    // Rename a\r\n}\r\n"
        );
        assert_eq!(comment_markers(Path::new("a/README.md")), ("<!--", " -->"));
        assert_eq!(comment_markers(Path::new("Makefile")), ("#", ""));
    }

    #[test]
    fn replacing_lines_keeps_the_rest_and_refuses_a_changed_file() {
        let path = Path::new("main.rs");
        let edited = apply(
            FILE,
            path,
            2,
            2,
            &lines(2..=2),
            EditKind::Replace,
            "    let a = 2;",
        )
        .unwrap();
        assert_eq!(edited, FILE.replace("a = 1", "a = 2"));
        let removed = apply(FILE, path, 2, 3, &lines(2..=3), EditKind::Replace, "").unwrap();
        assert_eq!(removed, "fn main() {\r\n}\r\n");
        let stale = vec!["    let a = 0;".to_owned()];
        assert!(apply(FILE, path, 2, 2, &stale, EditKind::Replace, "x").is_err());
    }
}
