// Copyright 2019 The Go Authors. All rights reserved.
// Adapted from gopls v0.24.0 internal/fuzzy/{matcher,input}.go.
// BSD-3-Clause license: LICENSE-GO at the repository root.

//! Dynamic programming matcher used by gopls completion.
const INPUT: usize = 127;
const PATTERN: usize = 63;
const MIN: i16 = -10000;
const SEP: u8 = 1;
const TAIL: u8 = 2;
const UPPER_TAIL: u8 = 3;
const HEAD: u8 = 4;

/// Reusable completion scorer with a bounded 32 KiB table, allocated once.
/// Matching follows gopls' ASCII folding and byte-based limits (63 pattern
/// bytes, 127 candidate bytes), including for UTF-8 input.
pub struct CompletionMatcher {
    pattern: Vec<u8>,
    lower: Vec<u8>,
    roles: Vec<u8>,
    sensitive: bool,
    scores: Vec<[[i16; 2]; PATTERN + 1]>,
}

impl CompletionMatcher {
    /// Constructs a scorer, retaining the first 63 pattern bytes.
    pub fn new(pattern: &str) -> Self {
        let pattern = pattern.as_bytes()[..pattern.len().min(PATTERN)].to_vec();
        let lower: Vec<_> = pattern.iter().map(u8::to_ascii_lowercase).collect();
        let sensitive = lower != pattern;
        let mut roles = vec![0; pattern.len()];
        rune_roles(&pattern, &mut roles);
        Self {
            pattern,
            lower,
            roles,
            sensitive,
            scores: vec![[[0; 2]; PATTERN + 1]; INPUT + 1],
        }
    }

    /// Scores a candidate in [0, 1], returning zero for absent or weak matches.
    /// No heap allocations occur while scoring. Empty patterns score one.
    pub fn score(&mut self, candidate: &str) -> f32 {
        if self.pattern.is_empty() {
            return 1.0;
        }
        let candidate = &candidate.as_bytes()[..candidate.len().min(INPUT)];
        let mut lower = [0; INPUT];
        let mut roles = [0; INPUT];
        for (to, from) in lower.iter_mut().zip(candidate) {
            *to = from.to_ascii_lowercase();
        }
        let mut matched = 0;
        for byte in &lower[..candidate.len()] {
            if *byte == self.lower[matched] {
                matched += 1;
                if matched == self.pattern.len() {
                    break;
                }
            }
        }
        if matched != self.pattern.len() {
            return 0.0;
        }
        rune_roles(candidate, &mut roles);
        let score = self.compute(candidate, &lower, &roles);
        if score <= MIN / 2 || self.poor_match(candidate.len(), &roles) {
            return 0.0;
        }
        if candidate.len() == self.pattern.len() {
            return 1.0;
        }
        (f32::from(score.max(0)) * (1.0 / (4 * self.pattern.len()) as f32)).min(1.0)
    }

    fn best(&self, i: usize, j: usize) -> usize {
        usize::from(self.scores[i][j][0] >> 1 < self.scores[i][j][1] >> 1)
    }

    fn compute(&mut self, candidate: &[u8], lower: &[u8], roles: &[u8]) -> i16 {
        let length = self.pattern.len();
        self.scores[0][..=length].fill([MIN << 1; 2]);
        self.scores[0][0][0] = 0;
        let mut segments = 1;
        let mut last_segment = 0;
        for (i, role) in roles[..candidate.len()].iter().enumerate() {
            if *role == SEP {
                segments += 1;
                last_segment = i + 1;
            }
        }
        for i in 1..=candidate.len() {
            let role = roles[i - 1];
            if role == SEP && segments > 1 {
                segments -= 1;
            }
            let penalty = i16::from(i == 1 || i - 1 == last_segment);
            for j in 0..=length {
                self.scores[i][j][1] = MIN << 1;
                let k = self.best(i - 1, j);
                let skip = (self.scores[i - 1][j][k] >> 1) - if j != length { penalty } else { 0 };
                self.scores[i][j][0] = packed(skip, k);
                if j == 0 || lower[i - 1] != self.lower[j - 1] {
                    continue;
                }
                let pattern_role = self.roles[j - 1];
                if role == TAIL
                    && pattern_role == HEAD
                    && (j > 1
                        || !lower[i - 1..candidate.len()].starts_with(&self.lower[..length.min(3)]))
                {
                    continue;
                }
                let mut bonus = i16::from(segments <= 1)
                    + i16::from(
                        candidate[i - 1] == self.pattern[j - 1] || role == HEAD && !self.sensitive,
                    );
                if role == TAIL && pattern_role == HEAD {
                    bonus -= 1;
                }
                if j == 1 && role == TAIL {
                    bonus -= 4;
                }
                for previous in 0..2 {
                    let mut score = (self.scores[i - 1][j - 1][previous] >> 1) + bonus;
                    if previous == 1 || i == 1 || i - 1 == last_segment {
                        score += 2;
                    }
                    if previous == 0 && matches!(role, TAIL | UPPER_TAIL) {
                        score -= 3;
                    }
                    if score > self.scores[i][j][1] >> 1 {
                        self.scores[i][j][1] = packed(score, previous);
                    }
                }
            }
        }
        self.scores[candidate.len()][length][self.best(candidate.len(), length)] >> 1
    }

    fn poor_match(&self, mut i: usize, roles: &[u8]) -> bool {
        let mut j = self.pattern.len();
        if j < 2 {
            return false;
        }
        let mut k = self.best(i, j);
        let mut count = 0;
        let mut streak = 0;
        while i > 0 {
            let take = k == 1;
            k = usize::from((self.scores[i][j][k] & 1) != 0);
            if take {
                streak += 1;
                if k == 0 && streak < 3 && roles[i - 1] == TAIL {
                    count += 1;
                    if count > 1 {
                        return true;
                    }
                }
                j -= 1;
            } else {
                streak = 0;
            }
            i -= 1;
        }
        false
    }
}

fn packed(score: i16, previous: usize) -> i16 {
    (score << 1) + previous as i16
}

fn rune_roles(input: &[u8], output: &mut [u8]) {
    const TYPES: &[u8] = b"00000000000000000000000000000000000000000000001122222222221000000333333333333333333333333330000002222222222222222222222222200000";
    let mut previous = 0;
    let mut before_previous = 0;
    for (i, byte) in input.iter().enumerate() {
        let current = if byte.is_ascii() {
            TYPES[usize::from(*byte)] - b'0'
        } else {
            2
        };
        let role = match current {
            2 if previous == 0 || previous == 1 => HEAD,
            2 => TAIL,
            3 if previous == 3 && i + 1 == input.len() => UPPER_TAIL,
            3 => HEAD,
            1 if matches!(byte, b'.' | b':') => SEP,
            _ => 0,
        };
        if current != 2
            && i > 1
            && output[i - 1] == HEAD
            && before_previous == 3
            && matches!(output[i - 2], HEAD | UPPER_TAIL)
        {
            output[i - 1] = UPPER_TAIL;
        }
        output[i] = role;
        before_previous = previous;
        previous = current;
    }
}
