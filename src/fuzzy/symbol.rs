// Copyright 2021 The Go Authors. All rights reserved.
// Adapted from gopls v0.24.0 internal/fuzzy/symbol.go under the BSD-3-Clause
// license in LICENSE-GO at the repository root. Byte offsets are corrected for
// Unicode input.

//! Symbol matcher adapted from gopls.
//!
//! The linear matcher uses at most 255 Unicode scalar values; see
//! [`crate::fuzzy::CompletionMatcher`] for the separate completion scorer.

const LIMIT: usize = 255;
const SEGMENT: u8 = 1;
const WORD: u8 = 2;
const SEPARATOR: u8 = 4;
const UPPER: u8 = 8;

/// A match's original UTF-8 byte offset and normalized relevance score.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Match {
    pub start: usize,
    pub score: f64,
}

/// A reusable pattern; matching uses fixed temporary buffers and no heap allocation.
#[derive(Debug)]
pub struct Matcher {
    pattern: Vec<char>,
}

impl Matcher {
    /// Creates a case-insensitive pattern, retaining at most 255 scalar values.
    pub fn new(pattern: &str) -> Self {
        Self {
            pattern: pattern.chars().take(LIMIT).map(lower).collect(),
        }
    }

    /// Finds the rightmost compact match in the concatenation of `chunks`.
    /// Empty patterns or inputs do not match. Chunk boundaries add no separators.
    pub fn score(&self, chunks: &[&str]) -> Option<Match> {
        if self.pattern.is_empty() {
            return None;
        }
        let mut input = ['\0'; LIMIT];
        let mut offsets = [0usize; LIMIT];
        let mut roles = [0u8; LIMIT];
        let mut segments = [0u8; LIMIT];
        let mut length = 0;
        let mut base = 0;
        let mut modifiers = WORD | SEGMENT;
        'input: for chunk in chunks {
            for (byte, rune) in chunk.char_indices() {
                if matches!(rune, '.' | '/') {
                    modifiers |= SEPARATOR;
                }
                let folded = lower(rune);
                if folded != rune {
                    modifiers |= UPPER;
                    if length == 0 || roles[length - 1] & UPPER == 0 {
                        modifiers |= WORD;
                    }
                }
                input[length] = folded;
                offsets[length] = base + byte;
                roles[length] = modifiers;
                modifiers = if modifiers & SEPARATOR != 0 {
                    WORD | SEGMENT
                } else {
                    0
                };
                length += 1;
                if length == LIMIT {
                    break 'input;
                }
            }
            base += chunk.len();
        }
        let mut pattern = self.pattern.len() - 1;
        let mut segment = 0;
        let mut start = None;
        for index in (0..length).rev() {
            if segment < 3 && roles[index] & SEPARATOR != 0 {
                segment += 1;
            }
            segments[index] = segment;
            if self.pattern[pattern] == input[index] {
                if pattern == 0 {
                    start = Some(index);
                    break;
                }
                pattern -= 1;
            }
        }
        let start = start?;
        pattern = 0;
        let mut total = 0.0;
        let mut previous = None;
        for index in start..length {
            if input[index] != self.pattern[pattern] {
                continue;
            }
            pattern += 1;
            let final_rune = pattern == self.pattern.len();
            let mut score = 0.6;
            if previous == index.checked_sub(1) {
                score = 0.9;
                if final_rune && index + 1 < length && roles[index + 1] & SEPARATOR == 0 {
                    score = if roles[index + 1] & WORD != 0 {
                        0.8
                    } else {
                        0.7
                    };
                }
            }
            previous = Some(index);
            if roles[index] & SEGMENT != 0 {
                score = 1.0;
            } else if roles[index] & WORD != 0 {
                score = 0.9;
            }
            total += score * (1.0 - f64::from(segments[index]) * 0.1);
            if final_rune {
                break;
            }
        }
        Some(Match {
            start: offsets[start],
            score: total / self.pattern.len() as f64,
        })
    }
}

fn lower(rune: char) -> char {
    rune.to_lowercase().next().expect("a lowercase scalar")
}
