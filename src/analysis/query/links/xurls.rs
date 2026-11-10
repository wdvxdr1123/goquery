//! Leftmost-longest emulation of `xurls.Relaxed()` for gopls document links.
//!
//! gopls compiles the relaxed expression with `regexp.Longest`, so every scan
//! position picks the longest alternative match. The expression is a fixed
//! union (scheme URL, scheme-less web URL, e-mail, bare IPv6) built from the
//! grammar below; this module reproduces that grammar directly instead of
//! pulling a backtracking regex engine into the server:
//!
//! ```text
//! pathCont = (?:[mid]*(?: wellParen|wellBrack|wellBrace |[end]))+
//! domain   = (?:iri\.)+ tlds
//! hostName = domain | "[" ipv6Addr "]" | \b ipv4Addr \b
//! webURL   = hostName (":" [0-9]+)? (?: "/" pathCont | "/" )?
//! email    = [a-zA-Z0-9._%+-]+ "@" domain
//! relaxed  = strictExp | webURL | email | ipv6AddrMinusEmpty
//! ```
//!
//! Matching is bounded by a caller-owned [`Budget`]; exhausting it stops the
//! scan instead of allocating unbounded work.

use super::schemes::{MAX_SCHEME, SCHEMES, SCHEMES_NO_AUTHORITY};
use super::tlds::{ASCII_TLDS, MAX_ASCII_TLD, MAX_UNICODE_TLD_CHARS, UNICODE_TLDS};
use super::unicode::{ALLOWED_UCS, ALLOWED_UCS_MINUS_PUNC, LETTER, MARK, NUMBER};

/// Upper bound on explored match states for one request.
pub(super) struct Budget {
    remaining: u32,
}

impl Budget {
    pub(super) fn new(remaining: u32) -> Self {
        Self { remaining }
    }

    pub(super) fn exhausted(&self) -> bool {
        self.remaining == 0
    }

    pub(super) fn spend(&mut self) {
        self.remaining = self.remaining.saturating_sub(1);
    }
}

fn in_ranges(value: u32, ranges: &[(u32, u32)]) -> bool {
    ranges
        .binary_search_by(|&(low, high)| {
            if value < low {
                std::cmp::Ordering::Greater
            } else if value > high {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn is_private(value: char) -> bool {
    matches!(value as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
}

fn is_iri_char(value: char) -> bool {
    let code = value as u32;
    in_ranges(code, LETTER) || in_ranges(code, MARK) || in_ranges(code, NUMBER)
}

fn is_mid_char(value: char) -> bool {
    value.is_ascii_alphanumeric()
        || matches!(
            value,
            '-' | '.'
                | '_'
                | '~'
                | '%'
                | '!'
                | '$'
                | '&'
                | '\''
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | ':'
                | '@'
                | '/'
                | '?'
                | '#'
                | '\\'
        )
        || in_ranges(value as u32, ALLOWED_UCS)
        || is_private(value)
}

fn is_end_char(value: char) -> bool {
    value.is_ascii_alphanumeric()
        || matches!(
            value,
            '-' | '_' | '~' | '%' | '$' | '&' | '+' | '=' | '/' | '#'
        )
        || in_ranges(value as u32, ALLOWED_UCS_MINUS_PUNC)
        || is_private(value)
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn word_boundary_after(text: &str, offset: usize) -> bool {
    match text.as_bytes().get(offset) {
        None => true,
        Some(byte) => !is_word_byte(*byte),
    }
}

fn word_boundary_before(text: &str, offset: usize) -> bool {
    if offset == 0 {
        return true;
    }
    let mut previous = offset - 1;
    while previous > 0 && (text.as_bytes()[previous] & 0xc0) == 0x80 {
        previous -= 1;
    }
    !is_word_byte(text.as_bytes()[previous])
}

fn next_char(text: &str, offset: usize) -> Option<(char, usize)> {
    let value = text[offset..].chars().next()?;
    Some((value, value.len_utf8()))
}

fn dedupe(values: &mut Vec<usize>) {
    values.sort_unstable();
    values.dedup();
}

fn contains_scheme(list: &[&str], name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    list.binary_search(&lowered.as_str()).is_ok()
}

fn is_scheme_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')
}

/// `pathCont`, implemented as a reachability walk over repetition starts. All
/// mid characters are consumed once per run, so long paths stay linear.
fn match_path_cont(text: &str, start: usize, budget: &mut Budget) -> Option<usize> {
    if start >= text.len() {
        return None;
    }
    let mut reach = std::collections::BTreeSet::new();
    reach.insert(start);
    let mut best: Option<usize> = None;
    let mut run_done = 0usize;
    while let Some(position) = reach.pop_first() {
        if budget.exhausted() {
            return best;
        }
        if position < run_done {
            continue;
        }
        let mut index = position;
        let mut last_end: Option<usize> = None;
        loop {
            if budget.exhausted() {
                return best;
            }
            let Some((value, length)) = next_char(text, index) else {
                run_done = text.len();
                break;
            };
            budget.spend();
            if is_mid_char(value) {
                if is_end_char(value) {
                    last_end = Some(index + length);
                }
                index += length;
                continue;
            }
            run_done = index;
            if let Some(end) = match_well_group(text, index, budget) {
                best = Some(best.map_or(end, |best| best.max(end)));
                reach.insert(end);
            }
            break;
        }
        if index >= text.len() {
            run_done = text.len();
        }
        if let Some(end) = last_end {
            best = Some(best.map_or(end, |best| best.max(end)));
        }
    }
    best
}

/// One `wellParen`/`wellBrack`/`wellBrace` group: a single nested group of mid
/// characters is allowed, everything else must close the outer group.
fn match_well_group(text: &str, start: usize, budget: &mut Budget) -> Option<usize> {
    let (open, close) = match text.as_bytes().get(start) {
        Some(b'(') => ('(', ')'),
        Some(b'[') => ('[', ']'),
        Some(b'{') => ('{', '}'),
        _ => return None,
    };
    let mut index = start + 1;
    let mut nested = false;
    loop {
        if budget.exhausted() {
            return None;
        }
        let (value, length) = next_char(text, index)?;
        budget.spend();
        if value == close {
            if nested {
                nested = false;
            } else {
                return Some(index + length);
            }
        } else if value == open {
            if nested {
                return None;
            }
            nested = true;
        } else if !is_mid_char(value) {
            return None;
        }
        index += length;
    }
}

fn h4_ends(text: &str, start: usize) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(4);
    let mut end = start;
    while end < bytes.len() && end - start < 4 && bytes[end].is_ascii_hexdigit() {
        end += 1;
        result.push(end);
    }
    result
}

fn colon_end(text: &str, start: usize) -> Option<usize> {
    (text.as_bytes().get(start) == Some(&b':')).then_some(start + 1)
}

fn octet_ends(text: &str, start: usize) -> Vec<usize> {
    let bytes = text.as_bytes();
    let digit = |offset: usize| -> Option<u8> {
        bytes
            .get(offset)
            .copied()
            .filter(|byte| byte.is_ascii_digit())
            .map(|byte| byte - b'0')
    };
    let mut result = Vec::with_capacity(3);
    let Some(first) = digit(start) else {
        return result;
    };
    result.push(start + 1);
    if first != 0 && digit(start + 1).is_some() {
        result.push(start + 2);
        if let (Some(second), Some(third)) = (digit(start + 1), digit(start + 2)) {
            let value = first as u16 * 100 + second as u16 * 10 + third as u16;
            if value <= 255 {
                result.push(start + 3);
            }
        }
    }
    result
}

/// `ipv4Addr` candidates; a dotted quad can have at most a few decompositions.
fn ipv4_ends(text: &str, start: usize) -> Vec<usize> {
    let mut states = octet_ends(text, start);
    for _ in 0..3 {
        let mut next = Vec::new();
        for state in states.drain(..) {
            if text.as_bytes().get(state) != Some(&b'.') {
                continue;
            }
            next.extend(octet_ends(text, state + 1));
        }
        dedupe(&mut next);
        states = next;
        if states.is_empty() {
            break;
        }
    }
    states
}

fn colon_h4_reps(
    text: &str,
    states: Vec<usize>,
    min: usize,
    max: usize,
    budget: &mut Budget,
) -> Vec<usize> {
    let mut results = Vec::new();
    let mut current = states;
    for occurrence in 0..=max {
        if occurrence >= min {
            results.extend(current.iter().copied());
        }
        if occurrence == max || current.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for state in current.drain(..) {
            if let Some(colon) = colon_end(text, state) {
                next.extend(h4_ends(text, colon));
            }
        }
        dedupe(&mut next);
        current = next;
        if budget.exhausted() {
            break;
        }
        budget.spend();
    }
    dedupe(&mut results);
    results
}

fn h4_colon_exact(
    text: &str,
    mut states: Vec<usize>,
    count: usize,
    budget: &mut Budget,
) -> Vec<usize> {
    for _ in 0..count {
        let mut next = Vec::new();
        for state in states.drain(..) {
            for end in h4_ends(text, state) {
                if let Some(colon) = colon_end(text, end) {
                    next.push(colon);
                }
            }
        }
        dedupe(&mut next);
        states = next;
        if states.is_empty() || budget.exhausted() {
            break;
        }
        budget.spend();
    }
    states
}

fn ipv6_minus_empty_ends(text: &str, start: usize, budget: &mut Budget) -> Vec<usize> {
    let mut result = Vec::new();
    let initial = vec![start];
    // (h4:){7}(h4|:)
    for state in h4_colon_exact(text, initial.clone(), 7, budget) {
        result.extend(h4_ends(text, state));
        if let Some(colon) = colon_end(text, state) {
            result.push(colon);
        }
    }
    // (h4:){6}(ipv4Addr|:h4|:)
    for state in h4_colon_exact(text, initial.clone(), 6, budget) {
        result.extend(ipv4_ends(text, state));
        if let Some(colon) = colon_end(text, state) {
            result.extend(h4_ends(text, colon));
            result.push(colon);
        }
    }
    // (h4:){n}( (?:':h4){0,n2} ':' ipv4Addr | (?:':h4){1,k} | ':' )
    for (n, n2, k) in [
        (5usize, 0usize, 2usize),
        (4, 1, 3),
        (3, 2, 4),
        (2, 3, 5),
        (1, 4, 6),
    ] {
        for state in h4_colon_exact(text, initial.clone(), n, budget) {
            // (?:':h4){0,n2} ':' ipv4Addr
            for rep in colon_h4_reps(text, vec![state], 0, n2, budget) {
                if let Some(colon) = colon_end(text, rep) {
                    result.extend(ipv4_ends(text, colon));
                }
            }
            // (?:':h4){1,k}
            result.extend(colon_h4_reps(text, vec![state], 1, k, budget));
            // ':'
            if let Some(colon) = colon_end(text, state) {
                result.push(colon);
            }
        }
    }
    // ':' ( (?:':h4){0,5} ':' ipv4Addr | (?:':h4){1,7} )
    if let Some(colon) = colon_end(text, start) {
        for rep in colon_h4_reps(text, vec![colon], 0, 5, budget) {
            if let Some(inner) = colon_end(text, rep) {
                result.extend(ipv4_ends(text, inner));
            }
        }
        result.extend(colon_h4_reps(text, vec![colon], 1, 7, budget));
    }
    dedupe(&mut result);
    result
}

fn unicode_lower(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>()
}

/// `TLDs` alternatives: punycode run, ASCII TLD with `\b`, or Unicode TLD.
fn match_tld(text: &str, start: usize, budget: &mut Budget) -> Option<usize> {
    if start >= text.len() {
        return None;
    }
    let mut best: Option<usize> = None;
    let bytes = text.as_bytes();
    // Longest ASCII TLD prefix followed by an ASCII word boundary.
    let limit = (text.len() - start).min(MAX_ASCII_TLD);
    for length in (1..=limit).rev() {
        if !bytes[start..start + length].is_ascii() {
            continue;
        }
        let candidate = &text[start..start + length];
        let lowered = candidate.to_ascii_lowercase();
        if ASCII_TLDS.binary_search(&lowered.as_str()).is_ok()
            && word_boundary_after(text, start + length)
        {
            best = Some(start + length);
            break;
        }
        budget.spend();
    }
    // Longest Unicode TLD prefix; the generated list is already lowercased.
    let mut ends = Vec::with_capacity(MAX_UNICODE_TLD_CHARS);
    let mut end = start;
    while ends.len() < MAX_UNICODE_TLD_CHARS {
        let Some((_, length)) = next_char(text, end) else {
            break;
        };
        end += length;
        ends.push(end);
    }
    for end in ends.into_iter().rev() {
        if budget.exhausted() {
            break;
        }
        budget.spend();
        let lowered = unicode_lower(&text[start..end]);
        if UNICODE_TLDS.binary_search(&lowered.as_str()).is_ok() {
            best = Some(best.map_or(end, |best| best.max(end)));
            break;
        }
    }
    // (?i)xn--[a-z0-9-]+
    if text.len() >= start + 5 && bytes[start..start + 4].eq_ignore_ascii_case(b"xn--") {
        let mut run = start + 4;
        while run < bytes.len() && (bytes[run].is_ascii_alphanumeric() || bytes[run] == b'-') {
            run += 1;
            budget.spend();
        }
        if run > start + 4 {
            best = Some(best.map_or(run, |best| best.max(run)));
        }
    }
    best
}

/// `webURL` tail after the host: optional port, then optional slash path.
fn web_continuation(text: &str, host_end: usize, budget: &mut Budget) -> usize {
    let bytes = text.as_bytes();
    let mut end = host_end;
    if bytes.get(end) == Some(&b':') {
        let mut digits = end + 1;
        while digits < bytes.len() && bytes[digits].is_ascii_digit() {
            digits += 1;
        }
        if digits > end + 1 {
            end = digits;
        }
    }
    if bytes.get(end) == Some(&b'/') {
        if let Some(path_end) = match_path_cont(text, end + 1, budget) {
            return path_end;
        }
        return end + 1;
    }
    end
}

fn take_max(best: &mut Option<usize>, found: Option<usize>) {
    if let Some(found) = found {
        *best = Some(best.map_or(found, |current| current.max(found)));
    }
}

/// All `domain` = `(?:iri\.)+ tlds` ends, one per plausible TLD start.
fn domain_ends(text: &str, start: usize, budget: &mut Budget) -> Vec<usize> {
    let Some((first, first_length)) = next_char(text, start) else {
        return Vec::new();
    };
    if !is_iri_char(first) {
        return Vec::new();
    }
    let mut dots = Vec::new();
    let mut index = start + first_length;
    let mut previous_iri = true;
    while !budget.exhausted() {
        let Some((value, length)) = next_char(text, index) else {
            break;
        };
        budget.spend();
        if is_iri_char(value) {
            previous_iri = true;
            index += length;
        } else if value == '-' {
            previous_iri = false;
            index += length;
        } else if value == '.' {
            if !previous_iri {
                break;
            }
            dots.push(index);
            match next_char(text, index + 1) {
                Some((next, next_length)) if is_iri_char(next) => {
                    previous_iri = true;
                    index += 1 + next_length;
                }
                _ => break,
            }
        } else {
            break;
        }
        if dots.len() >= 64 {
            break;
        }
    }
    let mut ends = Vec::new();
    for dot in dots {
        if let Some(tld_end) = match_tld(text, dot + 1, budget) {
            ends.push(tld_end);
        }
    }
    ends
}

fn match_web_url(text: &str, start: usize, budget: &mut Budget) -> Option<usize> {
    let mut best: Option<usize> = None;
    for host_end in domain_ends(text, start, budget) {
        let end = web_continuation(text, host_end, budget);
        take_max(&mut best, Some(end));
    }
    if text.as_bytes().get(start) == Some(&b'[') {
        let mut ends = ipv6_minus_empty_ends(text, start + 1, budget);
        if text[start + 1..].starts_with("::") {
            ends.push(start + 3);
        }
        dedupe(&mut ends);
        for end in ends {
            if text.as_bytes().get(end) == Some(&b']') {
                let found = web_continuation(text, end + 1, budget);
                take_max(&mut best, Some(found));
            }
        }
    }
    if word_boundary_before(text, start) {
        for end in ipv4_ends(text, start) {
            if word_boundary_after(text, end) {
                let found = web_continuation(text, end, budget);
                take_max(&mut best, Some(found));
            }
        }
    }
    best
}

fn match_email(text: &str, start: usize, budget: &mut Budget) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'%' | b'-' | b'+') {
            index += 1;
            budget.spend();
        } else {
            break;
        }
    }
    if index == start || bytes.get(index) != Some(&b'@') {
        return None;
    }
    domain_ends(text, index + 1, budget).into_iter().max()
}

fn match_scheme(text: &str, start: usize, budget: &mut Budget) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut end = start;
    while end < bytes.len() && end - start < MAX_SCHEME && is_scheme_byte(bytes[end]) {
        end += 1;
    }
    if bytes.get(end) != Some(&b':') {
        return None;
    }
    let name = &text[start..end];
    let mut best: Option<usize> = None;
    if bytes.get(end + 1) == Some(&b'/')
        && bytes.get(end + 2) == Some(&b'/')
        && contains_scheme(SCHEMES, name)
        && let Some(found) = match_path_cont(text, end + 3, budget)
    {
        best = Some(found);
    }
    if contains_scheme(SCHEMES_NO_AUTHORITY, name)
        && let Some(found) = match_path_cont(text, end + 1, budget)
    {
        best = Some(best.map_or(found, |best| best.max(found)));
    }
    best
}

fn longest_match(text: &str, start: usize, budget: &mut Budget) -> Option<usize> {
    let mut best: Option<usize> = None;
    take_max(&mut best, match_scheme(text, start, budget));
    take_max(&mut best, match_web_url(text, start, budget));
    take_max(&mut best, match_email(text, start, budget));
    take_max(
        &mut best,
        ipv6_minus_empty_ends(text, start, budget).into_iter().max(),
    );
    if word_boundary_before(text, start) {
        let bounded = ipv4_ends(text, start)
            .into_iter()
            .filter(|end| word_boundary_after(text, *end))
            .max();
        take_max(&mut best, bounded);
    }
    best
}

/// Appends every `xurls.Relaxed()` match in `text` with byte spans.
pub(super) fn find(text: &str, budget: &mut Budget, links: &mut Vec<(usize, usize)>) {
    let mut position = 0;
    while position < text.len() {
        if budget.exhausted() {
            return;
        }
        if let Some(end) = longest_match(text, position, budget) {
            links.push((position, end));
            position = end;
        } else {
            position += next_char(text, position).map_or(1, |(_, length)| length);
        }
    }
}
