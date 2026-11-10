//! The subset of Go's net/url parse/String semantics that document links need.
//!
//! gopls re-parses every xurls match with `url.Parse`, defaults a missing scheme
//! to `https`, drops schemes an editor cannot open, and emits `(*URL).String()`.
//! This module mirrors that flow for the pinned Go toolchain, including the
//! percent-encoding bitmap (`escape.rs`) and the RawPath/RawFragment round
//! trips. Go URL components are byte strings; components are kept as bytes here
//! too, and only a target that is valid UTF-8 is emitted (LSP strings cannot
//! carry arbitrary bytes, so such exotic matches stay unlinked).

use super::escape::ESCAPE;

const ENCODE_PATH: u8 = 1;
const ENCODE_HOST: u8 = 4;
const ENCODE_ZONE: u8 = 8;
const ENCODE_USER_PASSWORD: u8 = 16;
const ENCODE_QUERY_COMPONENT: u8 = 32;
const ENCODE_FRAGMENT: u8 = 64;

fn should_escape(byte: u8, mode: u8) -> bool {
    ESCAPE[byte as usize] & mode == 0
}

fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => byte - b'A' + 10,
    }
}

fn is_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

/// Go's `unescape`; `None` stands for its `EscapeError`/`InvalidHostError`.
fn unescape(value: &str, mode: u8) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut has_escape = false;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                has_escape = true;
                if index + 2 >= bytes.len()
                    || !is_hex(bytes[index + 1])
                    || !is_hex(bytes[index + 2])
                {
                    return None;
                }
                if mode == ENCODE_HOST
                    && hex_value(bytes[index + 1]) < 8
                    && &bytes[index..index + 3] != b"%25"
                {
                    return None;
                }
                if mode == ENCODE_ZONE {
                    let decoded = hex_value(bytes[index + 1]) << 4 | hex_value(bytes[index + 2]);
                    if &bytes[index..index + 3] != b"%25"
                        && decoded != b' '
                        && should_escape(decoded, ENCODE_HOST)
                    {
                        return None;
                    }
                }
                index += 3;
            }
            b'+' => index += 1,
            byte => {
                if (mode == ENCODE_HOST || mode == ENCODE_ZONE)
                    && byte < 0x80
                    && should_escape(byte, mode)
                {
                    return None;
                }
                index += 1;
            }
        }
    }
    if !has_escape && !value.contains('+') {
        return Some(bytes.to_vec());
    }
    let plus = if mode == ENCODE_QUERY_COMPONENT {
        b' '
    } else {
        b'+'
    };
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                decoded.push(hex_value(bytes[index + 1]) << 4 | hex_value(bytes[index + 2]));
                index += 3;
            }
            b'+' => {
                decoded.push(plus);
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    Some(decoded)
}

/// Go's `escape`: every byte that needs escaping in `mode` becomes `%XX`.
fn escape(value: &[u8], mode: u8) -> String {
    if value.iter().all(|byte| !should_escape(*byte, mode)) {
        return String::from_utf8(value.to_vec()).expect("unescaped URL text is UTF-8");
    }
    let mut escaped = String::with_capacity(value.len() + 8);
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in value {
        if *byte == b' ' && mode == ENCODE_QUERY_COMPONENT {
            escaped.push('+');
        } else if should_escape(*byte, mode) {
            escaped.push('%');
            escaped.push(HEX[(*byte >> 4) as usize] as char);
            escaped.push(HEX[(*byte & 15) as usize] as char);
        } else {
            // Only bytes below 0x80 are never escaped.
            escaped.push(*byte as char);
        }
    }
    escaped
}

fn valid_encoded(value: &[u8], mode: u8) -> bool {
    value.iter().all(|byte| match *byte {
        b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':'
        | b'@' | b'[' | b']' | b'%' => true,
        byte => !should_escape(byte, mode),
    })
}

fn valid_optional_port(port: &[u8]) -> bool {
    match port.split_first() {
        None => true,
        Some((b':', digits)) => digits.iter().all(|byte| byte.is_ascii_digit()),
        Some(_) => false,
    }
}

fn valid_userinfo(value: &str) -> bool {
    value.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                '-' | '.'
                    | '_'
                    | ':'
                    | '~'
                    | '!'
                    | '$'
                    | '&'
                    | '\''
                    | '('
                    | ')'
                    | '*'
                    | '+'
                    | ','
                    | ';'
                    | '='
                    | '%'
                    | '@'
            )
    })
}

/// Splits `raw` the way `getScheme` does: `(scheme, rest)` or no scheme.
fn get_scheme(raw: &str) -> Option<(&str, &str)> {
    for (index, byte) in raw.bytes().enumerate() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                if index == 0 {
                    return Some(("", raw));
                }
            }
            b':' => {
                if index == 0 {
                    return None;
                }
                return Some((&raw[..index], &raw[index + 1..]));
            }
            _ => return Some(("", raw)),
        }
    }
    Some(("", raw))
}

/// Strict IPv6 validation equivalent to `netip.ParseAddr(addr)` accepting an
/// IPv6 address after the zone has been split off.
fn valid_ipv6(value: &[u8]) -> bool {
    if value.is_empty() {
        return false;
    }
    let body = match value.iter().position(|byte| *byte == b'%') {
        Some(index) => &value[..index],
        None => value,
    };
    let mut double = false;
    let mut groups: Vec<&[u8]> = Vec::new();
    if let Some(rest) = body.strip_prefix(b"::") {
        double = true;
        if !rest.is_empty() {
            groups.extend(rest.split(|byte| *byte == b':'));
        }
    } else if let Some(rest) = body.strip_suffix(b"::") {
        double = true;
        if !rest.is_empty() {
            groups.extend(rest.split(|byte| *byte == b':'));
        }
    } else if let Some(index) = body.windows(2).position(|window| window == b"::") {
        double = true;
        groups.extend(body[..index].split(|byte| *byte == b':'));
        groups.extend(body[index + 2..].split(|byte| *byte == b':'));
    } else {
        groups.extend(body.split(|byte| *byte == b':'));
    }
    if groups.iter().any(|group| group.is_empty()) {
        return false;
    }
    let mut count = 0usize;
    for group in &groups {
        if group.contains(&b'.') {
            if !valid_ipv4(group) {
                return false;
            }
            count += 2;
        } else {
            if group.len() > 4 || !group.iter().all(|byte| is_hex(*byte)) {
                return false;
            }
            count += 1;
        }
    }
    if double { count < 8 } else { count == 8 }
}

fn valid_ipv4(value: &[u8]) -> bool {
    let mut count = 0;
    for part in value.split(|byte| *byte == b'.') {
        if part.is_empty() || part.len() > 3 || !part.iter().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        if part.len() > 1 && part[0] == b'0' {
            return false;
        }
        if part
            .iter()
            .fold(0u16, |value, byte| value * 10 + (byte - b'0') as u16)
            > 255
        {
            return false;
        }
        count += 1;
    }
    count == 4
}

type UserInfo = Option<(Vec<u8>, Option<Vec<u8>>)>;

struct Url {
    scheme: String,
    opaque: Vec<u8>,
    user: UserInfo,
    host: Vec<u8>,
    path: Vec<u8>,
    raw_path: Vec<u8>,
    omit_host: bool,
    raw_query: Vec<u8>,
    force_query: bool,
    fragment: Vec<u8>,
    raw_fragment: Vec<u8>,
}

impl Url {
    fn set_path(&mut self, raw: &str) -> Option<()> {
        let path = unescape(raw, ENCODE_PATH)?;
        self.raw_path = if escape(&path, ENCODE_PATH).as_bytes() == raw.as_bytes() {
            Vec::new()
        } else {
            raw.as_bytes().to_vec()
        };
        self.path = path;
        Some(())
    }

    fn escaped_path(&self) -> Vec<u8> {
        if !self.raw_path.is_empty()
            && valid_encoded(&self.raw_path, ENCODE_PATH)
            && std::str::from_utf8(&self.raw_path)
                .ok()
                .and_then(|raw| unescape(raw, ENCODE_PATH))
                .as_deref()
                == Some(self.path.as_slice())
        {
            return self.raw_path.clone();
        }
        if self.path == b"*" {
            return b"*".to_vec();
        }
        escape(&self.path, ENCODE_PATH).into_bytes()
    }

    fn set_fragment(&mut self, raw: &str) -> Option<()> {
        let fragment = unescape(raw, ENCODE_FRAGMENT)?;
        self.raw_fragment = if escape(&fragment, ENCODE_FRAGMENT) == raw {
            Vec::new()
        } else {
            raw.as_bytes().to_vec()
        };
        self.fragment = fragment;
        Some(())
    }

    fn escaped_fragment(&self) -> Vec<u8> {
        if !self.raw_fragment.is_empty()
            && valid_encoded(&self.raw_fragment, ENCODE_FRAGMENT)
            && std::str::from_utf8(&self.raw_fragment)
                .ok()
                .and_then(|raw| unescape(raw, ENCODE_FRAGMENT))
                .as_deref()
                == Some(self.fragment.as_slice())
        {
            return self.raw_fragment.clone();
        }
        escape(&self.fragment, ENCODE_FRAGMENT).into_bytes()
    }

    fn userinfo(&self) -> Option<Vec<u8>> {
        let (name, password) = self.user.as_ref()?;
        let mut value = escape(name, ENCODE_USER_PASSWORD).into_bytes();
        if let Some(password) = password {
            value.push(b':');
            value.extend_from_slice(escape(password, ENCODE_USER_PASSWORD).as_bytes());
        }
        Some(value)
    }

    fn string(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.scheme.is_empty() {
            out.extend_from_slice(self.scheme.as_bytes());
            out.push(b':');
        }
        if !self.opaque.is_empty() {
            out.extend_from_slice(&self.opaque);
        } else {
            if !self.scheme.is_empty() || !self.host.is_empty() || self.user.is_some() {
                if self.omit_host && self.host.is_empty() && self.user.is_none() {
                    // Omit an empty host written as a bare empty authority.
                } else {
                    if !self.host.is_empty() || !self.path.is_empty() || self.user.is_some() {
                        out.extend_from_slice(b"//");
                    }
                    if let Some(userinfo) = self.userinfo() {
                        out.extend_from_slice(&userinfo);
                        out.push(b'@');
                    }
                    if !self.host.is_empty() {
                        out.extend_from_slice(escape(&self.host, ENCODE_HOST).as_bytes());
                    }
                }
            }
            let mut path = self.escaped_path();
            if self.omit_host
                && self.host.is_empty()
                && self.user.is_none()
                && path.starts_with(b"//")
            {
                out.extend_from_slice(b"%2F");
                path.remove(0);
            }
            if !path.is_empty() && path[0] != b'/' && !self.host.is_empty() {
                out.push(b'/');
            }
            if out.is_empty()
                && let Some(segment) = path.split(|byte| *byte == b'/').next()
                && segment.contains(&b':')
            {
                out.extend_from_slice(b"./");
            }
            out.extend_from_slice(&path);
        }
        if self.force_query || !self.raw_query.is_empty() {
            out.push(b'?');
            out.extend_from_slice(&self.raw_query);
        }
        if !self.fragment.is_empty() {
            out.push(b'#');
            out.extend_from_slice(&self.escaped_fragment());
        }
        out
    }
}

fn empty_url() -> Url {
    Url {
        scheme: String::new(),
        opaque: Vec::new(),
        user: None,
        host: Vec::new(),
        path: Vec::new(),
        raw_path: Vec::new(),
        omit_host: false,
        raw_query: Vec::new(),
        force_query: false,
        fragment: Vec::new(),
        raw_fragment: Vec::new(),
    }
}

fn parse_host(scheme: &str, host: &str) -> Option<Vec<u8>> {
    let brackets = host.rfind('[');
    if brackets.is_some_and(|index| index > 0) {
        return None;
    }
    if brackets == Some(0) {
        let close = host.rfind(']')?;
        let colon_port = &host[close + 1..];
        if !valid_optional_port(colon_port.as_bytes()) {
            return None;
        }
        let colon_port = unescape(colon_port, ENCODE_HOST)?;
        let hostname = &host[1..close];
        let hostname = match hostname.find("%25") {
            Some(index) => {
                let mut value = unescape(&hostname[..index], ENCODE_HOST)?;
                value.extend_from_slice(&unescape(&hostname[index..], ENCODE_ZONE)?);
                value
            }
            None => unescape(hostname, ENCODE_HOST)?,
        };
        if !valid_ipv6(&hostname) {
            return None;
        }
        let mut result = Vec::with_capacity(hostname.len() + colon_port.len() + 2);
        result.push(b'[');
        result.extend_from_slice(&hostname);
        result.push(b']');
        result.extend_from_slice(&colon_port);
        return Some(result);
    }
    if let Some(first) = host.find(':') {
        let mut index = first;
        let last = host.rfind(':').expect("colon present");
        if last != first && scheme != "http" && scheme != "https" {
            index = last;
        }
        if !valid_optional_port(&host.as_bytes()[index..]) {
            return None;
        }
    }
    unescape(host, ENCODE_HOST)
}

fn parse_authority(scheme: &str, authority: &str) -> Option<(UserInfo, Vec<u8>)> {
    let separator = authority.rfind('@');
    match separator {
        None => Some((None, parse_host(scheme, authority)?)),
        Some(index) => {
            let userinfo = &authority[..index];
            let host = parse_host(scheme, &authority[index + 1..])?;
            if !valid_userinfo(userinfo) {
                return None;
            }
            let user = match userinfo.find(':') {
                None => Some((unescape(userinfo, ENCODE_USER_PASSWORD)?, None)),
                Some(colon) => Some((
                    unescape(&userinfo[..colon], ENCODE_USER_PASSWORD)?,
                    Some(unescape(&userinfo[colon + 1..], ENCODE_USER_PASSWORD)?),
                )),
            };
            Some((user, host))
        }
    }
}

fn parse_without_fragment(raw: &str) -> Option<Url> {
    if raw.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return None;
    }
    let (scheme, rest) = get_scheme(raw)?;
    let mut url = empty_url();
    url.scheme = scheme.to_ascii_lowercase();
    let mut rest = rest;
    if rest.ends_with('?') && rest.matches('?').count() == 1 {
        url.force_query = true;
        rest = &rest[..rest.len() - 1];
    } else if let Some(index) = rest.find('?') {
        url.raw_query = rest.as_bytes()[index + 1..].to_vec();
        rest = &rest[..index];
    }
    if !rest.starts_with('/') {
        if !url.scheme.is_empty() {
            url.opaque = rest.as_bytes().to_vec();
            return Some(url);
        }
        let segment = rest.split('/').next().unwrap_or("");
        if segment.contains(':') {
            return None;
        }
    }
    if (!url.scheme.is_empty() || !rest.starts_with("///")) && rest.starts_with("//") {
        let after = &rest[2..];
        let (authority, remainder) = match after.find('/') {
            Some(index) => (&after[..index], &after[index..]),
            None => (after, ""),
        };
        let (user, host) = parse_authority(&url.scheme, authority)?;
        url.user = user;
        url.host = host;
        rest = remainder;
    } else if !url.scheme.is_empty() && rest.starts_with('/') {
        url.omit_host = true;
    }
    url.set_path(rest)?;
    Some(url)
}

fn parse(raw: &str) -> Option<Url> {
    let (body, fragment) = match raw.find('#') {
        Some(index) => (&raw[..index], &raw[index + 1..]),
        None => (raw, ""),
    };
    let mut url = parse_without_fragment(body)?;
    if !fragment.is_empty() {
        url.set_fragment(fragment)?;
    }
    Some(url)
}

/// gopls' `findLinksInString` URL handling: parse, default a missing scheme to
/// https, reject schemes an editor cannot open, then stringify.
pub(super) fn target(raw: &str) -> Option<String> {
    let mut url = match parse(raw) {
        Some(url) => url,
        None => parse(&format!("//{raw}"))?,
    };
    if url.scheme.is_empty() {
        url.scheme = "https".to_string();
    }
    if url.scheme != "http" && url.scheme != "https" {
        return None;
    }
    String::from_utf8(url.string()).ok()
}
