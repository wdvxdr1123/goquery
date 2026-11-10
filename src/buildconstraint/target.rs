use super::experiments;
use super::expression::ParseError;
use super::header;
use std::collections::BTreeSet;

/// Explicit target context. Compiler/release/cgo/custom tags are supplied by the
/// caller rather than inferred from the machine running the parser.
#[derive(Default, Debug)]
pub struct Target {
    goos: String,
    goarch: String,
    tags: BTreeSet<String>,
}

impl Target {
    /// Configured architecture, independent of the machine running the server.
    pub fn goarch(&self) -> &str {
        &self.goarch
    }
    pub fn new(goos: &str, goarch: &str) -> Self {
        Self {
            goos: goos.to_owned(),
            goarch: goarch.to_owned(),
            tags: BTreeSet::new(),
        }
    }

    /// Applies Go 1.27.3 experiment defaults and comma-separated overrides.
    /// Experiment names are not validated: unknown names become
    /// `goexperiment.<name>` tags. Compiler/release/custom tags outside the
    /// `goexperiment.` namespace are preserved.
    pub fn configure_experiments(&mut self, configuration: &str) {
        let enabled = experiments::tags(&self.goos, &self.goarch, configuration);
        self.tags.retain(|tag| !tag.starts_with("goexperiment."));
        self.tags.extend(enabled);
    }

    /// Enables a caller-provided build, compiler, release or feature tag.
    pub fn enable_tag(&mut self, tag: &str) {
        self.tags.insert(tag.to_owned());
    }

    /// Matches Go platform aliases as well as explicitly enabled tags.
    pub fn matches_tag(&self, tag: &str) -> bool {
        let tag = if tag == "boringcrypto" && tag != self.goos && tag != self.goarch {
            "goexperiment.boringcrypto"
        } else {
            tag
        };
        tag == self.goos
            || tag == self.goarch
            || self.tags.contains(tag)
            || matches!(
                (self.goos.as_str(), tag),
                ("android", "linux") | ("ios", "darwin") | ("illumos", "solaris")
            )
            || tag == "unix" && UNIX.contains(&self.goos.as_str())
    }

    /// Selects a Go source basename using Go 1.27 suffix rules, including `_test`.
    /// Dot/underscore-prefixed names and non-Go files are excluded. This does not
    /// decide whether tests belong to a particular build configuration.
    pub fn matches_filename(&self, name: &str) -> bool {
        if name.starts_with(['.', '_']) || !name.ends_with(".go") {
            return false;
        }
        let stem = name.split('.').next().expect("split always has first part");
        let Some(first) = stem.find('_') else {
            return true;
        };
        let parts: Vec<_> = stem[first..].split('_').collect();
        let parts = if parts.last() == Some(&"test") {
            &parts[..parts.len() - 1]
        } else {
            &parts[..]
        };
        let Some(last) = parts.last() else {
            return true;
        };
        if parts.len() >= 2
            && KNOWN_OS.contains(&parts[parts.len() - 2])
            && KNOWN_ARCH.contains(last)
        {
            return self.matches_tag(parts[parts.len() - 2]) && self.matches_tag(last);
        }
        if KNOWN_OS.contains(last) || KNOWN_ARCH.contains(last) {
            return self.matches_tag(last);
        }
        true
    }

    /// Combines filename selection and leading `//go:build` expressions.
    pub fn matches_file(&self, name: &str, source: &str) -> Result<bool, ParseError> {
        if !self.matches_filename(name) {
            return Ok(false);
        }
        Ok(header::from_header(source)?
            .is_none_or(|constraint| constraint.matches(|tag| self.matches_tag(tag))))
    }
}

// These include historical/future names because Go uses them for suffix filtering.
const KNOWN_OS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "nacl",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];
const UNIX: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "linux",
    "netbsd",
    "openbsd",
    "solaris",
];
const KNOWN_ARCH: &[&str] = &[
    "386",
    "amd64",
    "amd64p32",
    "arm",
    "armbe",
    "arm64",
    "arm64be",
    "loong64",
    "mips",
    "mipsle",
    "mips64",
    "mips64le",
    "mips64p32",
    "mips64p32le",
    "ppc",
    "ppc64",
    "ppc64le",
    "riscv",
    "riscv64",
    "s390",
    "s390x",
    "sparc",
    "sparc64",
    "wasm",
];
