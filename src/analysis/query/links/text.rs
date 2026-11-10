//! Text-level link scanning shared by strings and comments.
//!
//! gopls scans each literal/comment with `xurls.Relaxed()`, re-parses accepted
//! matches with `net/url`, then appends the issue links of the `org/repo#N`
//! pattern. This module owns that per-piece pipeline; document orchestration
//! (imports, ordering, protocol encoding) stays in the parent module.

use super::url;
use super::xurls::{self, Budget};

/// URL links in xurls order followed by issue links, as gopls does.
pub(super) fn links(text: &str, budget: &mut Budget) -> Vec<(usize, usize, String)> {
    let mut result = Vec::new();
    let mut matches = Vec::new();
    xurls::find(text, budget, &mut matches);
    let mut issues = Vec::new();
    issue_matches(text, budget, &mut issues);
    let issue_target = issues.first().and_then(|first| issue_target(text, *first));
    for (start, end) in matches {
        if let Some(target) = url::target(&text[start..end]) {
            result.push((start, end, target));
        }
    }
    if let Some(target) = issue_target {
        for (start, end) in issues {
            result.push((start, end, target.clone()));
        }
    }
    result
}

/// `(\w+)/([\w-]+)#([0-9]+)` with Go's leftmost-first (greedy backtracking)
/// semantics. gopls reports each match's own range but takes the target from
/// the first `FindStringSubmatch` of the whole text, so one target is repeated
/// for every issue range in the piece.
fn issue_matches(text: &str, budget: &mut Budget, matches: &mut Vec<(usize, usize)>) {
    let bytes = text.as_bytes();
    let mut position = 0;
    while position < bytes.len() {
        if budget.exhausted() {
            return;
        }
        if let Some(found) = issue_match_at(bytes, position) {
            matches.push(found);
            position = found.1;
        } else {
            position += 1;
        }
        budget.spend();
    }
}

fn issue_match_at(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    if !is_word_byte(*bytes.get(start)?) {
        return None;
    }
    let mut word_end = start + 1;
    while word_end < bytes.len() && is_word_byte(bytes[word_end]) {
        word_end += 1;
    }
    for slash in (start + 1..=word_end).rev() {
        if *bytes.get(slash)? != b'/' {
            continue;
        }
        let mut repo_end = slash + 1;
        while repo_end < bytes.len() && (is_word_byte(bytes[repo_end]) || bytes[repo_end] == b'-') {
            repo_end += 1;
        }
        if repo_end == slash + 1 {
            continue;
        }
        for hash in (slash + 2..=repo_end).rev() {
            if *bytes.get(hash).unwrap_or(&0) != b'#' {
                continue;
            }
            let mut digits = hash + 1;
            while digits < bytes.len() && bytes[digits].is_ascii_digit() {
                digits += 1;
            }
            if digits > hash + 1 {
                return Some((start, digits));
            }
        }
    }
    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn issue_target(text: &str, span: (usize, usize)) -> Option<String> {
    let matched = &text[span.0..span.1];
    let slash = matched.find('/')?;
    let rest = &matched[slash + 1..];
    let hash = rest.find('#')?;
    Some(format!(
        "https://github.com/{}/{}/issues/{}",
        &matched[..slash],
        &rest[..hash],
        &rest[hash + 1..]
    ))
}
