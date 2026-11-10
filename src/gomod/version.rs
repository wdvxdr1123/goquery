//! Go canonical module version ordering and module-cache escaping.
use std::cmp::Ordering;

fn number(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}
fn numeric_cmp(left: &str, right: &str) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}
fn parts(value: &str) -> Option<([&str; 3], Option<&str>)> {
    let value = value.strip_prefix('v')?;
    let (value, build) = value
        .split_once('+')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    if build.is_some_and(|v| {
        v.is_empty()
            || !v
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    }) {
        return None;
    }
    let (base, pre) = value
        .split_once('-')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    let values: Vec<_> = base.split('.').collect();
    if values.len() != 3 || !values.iter().all(|v| number(v)) {
        return None;
    }
    if pre.is_some_and(|v| {
        v.split('.').any(|s| {
            s.is_empty()
                || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || s.bytes().all(|b| b.is_ascii_digit()) && !number(s)
        })
    }) {
        return None;
    }
    Some(([values[0], values[1], values[2]], pre))
}
/// Compare canonical three-component Go versions, including pseudo-versions.
/// Build metadata does not affect precedence. Invalid inputs return None.
pub fn compare_versions(left: &str, right: &str) -> Option<Ordering> {
    let (a, ap) = parts(left)?;
    let (b, bp) = parts(right)?;
    for (a, b) in a.into_iter().zip(b) {
        let order = numeric_cmp(a, b);
        if order != Ordering::Equal {
            return Some(order);
        }
    }
    Some(match (ap, bp) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => {
            let mut a = a.split('.');
            let mut b = b.split('.');
            loop {
                match (a.next(), b.next()) {
                    (None, None) => break Ordering::Equal,
                    (None, Some(_)) => break Ordering::Less,
                    (Some(_), None) => break Ordering::Greater,
                    (Some(a), Some(b)) => {
                        let order = match (number(a), number(b)) {
                            (true, true) => numeric_cmp(a, b),
                            (true, false) => Ordering::Less,
                            (false, true) => Ordering::Greater,
                            _ => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            break order;
                        }
                    }
                }
            }
        }
    })
}
/// Encode an ASCII module path or canonical version for the Go module cache.
/// Reject filesystem traversal/separators that are not module path separators.
pub fn escape_cache_path(value: &str) -> Option<String> {
    if value.is_empty()
        || value
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || !value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'-' | b'_' | b'+' | b'~')
        })
    {
        return None;
    }
    let mut result = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_uppercase() {
            result.push('!');
            result.push(byte.to_ascii_lowercase() as char);
        } else {
            result.push(byte as char);
        }
    }
    Some(result)
}
