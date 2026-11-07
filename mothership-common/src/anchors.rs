//! Anchors are references to code written inside chat messages: `scripts/launcher.py:136` points
//! at one line, and `scripts/` points at a folder. Anchors are read from the message text each time,
//! so they always refer to the live file.

use serde::{Deserialize, Serialize};

use crate::normalize_selector;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// The text as it was written, without trailing punctuation
    pub text: String,
    /// Project-relative path, with `/` separators and no trailing `/`
    pub path: String,
    /// Line number for a file-and-line anchor. `None` for a file or a folder.
    pub line: Option<u32>,
    /// True when the text names a folder (it ends with `/`)
    pub folder: bool,
}

/// Every anchor in a message, in the order they appear
pub fn find_anchors(text: &str) -> Vec<Anchor> {
    text.split_whitespace().filter_map(parse_token).collect()
}

fn parse_token(raw: &str) -> Option<Anchor> {
    let token = raw
        .trim_start_matches(|c: char| matches!(c, '(' | '[' | '"' | '\''))
        .trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | '!' | '?' | ')' | ']' | '"' | '\''));
    if token.is_empty() {
        return None;
    }

    if let Some((path, line)) = token.rsplit_once(':') {
        let line: u32 = line.parse().ok()?;
        if line == 0 || path.is_empty() || path.ends_with('/') {
            return None;
        }
        return Some(Anchor {
            text: token.to_string(),
            path: normalize_selector(path).ok()?,
            line: Some(line),
            folder: false,
        });
    }

    // Plain paths must contain a dot or end with `/`, so ordinary words such as "and/or" are not anchors
    let folder = token.ends_with('/');
    if !folder && !token.contains('.') {
        return None;
    }
    let normalized = normalize_selector(token).ok()?;
    Some(Anchor {
        text: token.to_string(),
        path: normalized.trim_end_matches('/').to_string(),
        line: None,
        folder,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_file_and_line_anchors() {
        let anchors = find_anchors("look at scripts/launcher.py:136 please");
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].path, "scripts/launcher.py");
        assert_eq!(anchors[0].line, Some(136));
        assert!(!anchors[0].folder);
    }

    #[test]
    fn finds_folder_anchors_and_plain_file_anchors() {
        let anchors = find_anchors("see scripts/ and README.md");
        assert_eq!(anchors.len(), 2);
        assert!(anchors[0].folder);
        assert_eq!(anchors[0].path, "scripts");
        assert_eq!(anchors[1].path, "README.md");
        assert_eq!(anchors[1].line, None);
    }

    #[test]
    fn trailing_punctuation_and_brackets_are_not_part_of_the_anchor() {
        let anchors = find_anchors("(src/main.rs:10), and src/lib.rs:2.");
        let paths: Vec<(&str, Option<u32>)> = anchors.iter().map(|a| (a.path.as_str(), a.line)).collect();
        assert_eq!(paths, vec![("src/main.rs", Some(10)), ("src/lib.rs", Some(2))]);
    }

    #[test]
    fn ordinary_words_urls_and_unsafe_paths_are_ignored() {
        assert!(find_anchors("and/or this is fine").is_empty());
        assert!(find_anchors("https://example.com/page").is_empty());
        assert!(find_anchors("C:\\Users\\me\\x.rs:10").is_empty());
        assert!(find_anchors("../secret.txt:3").is_empty());
        assert!(find_anchors("src/main.rs:0").is_empty());
    }
}
