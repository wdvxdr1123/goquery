// Copyright 2021 The Go Authors. All rights reserved.
// Matching policy adapted from gopls v0.24.0 internal/golang/workspace_symbol.go.
// BSD-3-Clause license: LICENSE-GO at the repository root.

use crate::fuzzy::Matcher;

pub(super) struct Score {
    pub start: usize,
    pub score: f64,
}

pub(super) struct Query {
    terms: Vec<Term>,
}

enum Term {
    Fuzzy(Matcher),
    Exact {
        text: String,
        mode: Mode,
        sensitive: bool,
    },
}

enum Mode {
    Prefix,
    Suffix,
    Substring,
}

impl Query {
    pub(super) fn new(text: &str) -> Result<Self, (i32, String)> {
        if text.len() > 4096 {
            return Err((
                crate::error::INVALID_PARAMS,
                "symbol query exceeds 4096 bytes".into(),
            ));
        }
        let mut terms = Vec::new();
        for field in text.split_whitespace() {
            if terms.len() == 32 {
                return Err((
                    crate::error::INVALID_PARAMS,
                    "symbol query exceeds 32 terms".into(),
                ));
            }
            let exact = if let Some(text) = field.strip_prefix('^') {
                Some((text, Mode::Prefix))
            } else if let Some(text) = field.strip_prefix('\'') {
                Some((text, Mode::Substring))
            } else {
                field.strip_suffix('$').map(|text| (text, Mode::Suffix))
            };
            terms.push(if let Some((text, mode)) = exact {
                Term::Exact {
                    text: text.to_owned(),
                    mode,
                    sensitive: text
                        .chars()
                        .any(|rune| rune.to_lowercase().next() != Some(rune)),
                }
            } else {
                Term::Fuzzy(Matcher::new(field))
            });
        }
        Ok(Self { terms })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub(super) fn score(&self, input: &str, lowercase: &mut String) -> Option<Score> {
        lowercase.clear();
        if self.terms.iter().any(|term| {
            matches!(
                term,
                Term::Exact {
                    sensitive: false,
                    ..
                }
            )
        }) {
            lowercase.extend(
                input
                    .chars()
                    .map(|rune| rune.to_lowercase().next().expect("lowercase scalar")),
            );
        }
        let mut start = 0;
        let mut score = 1.0;
        for term in &self.terms {
            let hit = match term {
                Term::Fuzzy(matcher) => matcher.score(&[input])?,
                Term::Exact {
                    text,
                    mode,
                    sensitive,
                } => {
                    let candidate = if *sensitive {
                        input
                    } else {
                        lowercase.as_str()
                    };
                    let offset = match mode {
                        Mode::Prefix => candidate.starts_with(text).then_some(0),
                        Mode::Suffix => candidate
                            .ends_with(text)
                            .then(|| candidate.len() - text.len()),
                        Mode::Substring => candidate.rfind(text),
                    }?;
                    let byte = if *sensitive {
                        offset
                    } else {
                        let index = candidate[..offset].chars().count();
                        input
                            .char_indices()
                            .nth(index)
                            .map_or(input.len(), |(byte, _)| byte)
                    };
                    start = byte;
                    continue;
                }
            };
            start = hit.start;
            score *= hit.score;
        }
        // gopls' combined matcher uses zero as the qualification offset.
        Some(Score {
            start: if self.terms.len() == 1 { start } else { 0 },
            score,
        })
    }
}
