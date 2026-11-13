//! Translate initialization options once, before scanning metadata.
use super::{Workspace, dependencies, goenv, path, sdk};
use serde_json::Value;

impl Workspace {
    pub(crate) fn integer_bits(&self) -> Option<u32> {
        match self.build_target.goarch() {
            "386" | "arm" | "mips" | "mipsle" | "wasm" => Some(32),
            "amd64" | "arm64" | "loong64" | "mips64" | "mips64le" | "ppc64" | "ppc64le"
            | "riscv64" | "s390x" => Some(64),
            _ => None,
        }
    }

    /// A view target for an orphaned file: same compiler/release tags and cgo
    /// policy as the workspace, with the port chosen from gopls' preferred list.
    pub(super) fn view_target(&self, goos: &str, goarch: &str) -> crate::buildconstraint::Target {
        let mut target = crate::buildconstraint::Target::new(goos, goarch);
        target.enable_tag("gc");
        for version in 1..=27 {
            target.enable_tag(&format!("go1.{version}"));
        }
        if !self.cgo_disabled {
            target.enable_tag("cgo");
        }
        target.configure_experiments("");
        target
    }
    pub fn initialize(&mut self, params: &Value) -> Result<(), (i32, String)> {
        self.completion_config = crate::features::CompletionConfig::from_initialize(params)?;
        self.semantic_config = crate::features::SemanticConfig::from_initialize(params);
        self.hierarchical_document_symbols = params["capabilities"]["textDocument"]
            ["documentSymbol"]["hierarchicalDocumentSymbolSupport"]
            .as_bool()
            .unwrap_or(false);
        self.watched_files_dynamic =
            params["capabilities"]["workspace"]["didChangeWatchedFiles"]["dynamicRegistration"]
                .as_bool()
                .unwrap_or(false);
        let options = &params["initializationOptions"];
        self.file_watch_enabled = match options.get("fileWatch").filter(|value| !value.is_null()) {
            Some(value) => value.as_bool().ok_or((
                crate::error::INVALID_PARAMS,
                "fileWatch must be a boolean".into(),
            ))?,
            None => true,
        };
        let defaults = super::file_events::WatchLimits::default();
        self.file_watch_limits = super::file_events::WatchLimits {
            roots: watch_limit(options, "fileWatchMaxRoots", defaults.roots)?,
            directories: watch_limit(options, "fileWatchMaxDirectories", defaults.directories)?,
            entries: watch_limit(options, "fileWatchMaxEntries", defaults.entries)?,
        };
        self.file_watch_excludes = std::sync::Arc::new(watch_excludes(options)?);
        // One local probe fills both defaults; it is not cached across requests.
        let go_env = if needs_go_env(options) {
            goenv::probe()
        } else {
            goenv::GoEnv::default()
        };
        self.module_cache = dependencies::cache_root(&go_env);
        let os = options["goos"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| std::env::var("GOOS").ok())
            .unwrap_or_else(|| {
                match std::env::consts::OS {
                    "macos" => "darwin",
                    other => other,
                }
                .to_owned()
            });
        let arch = options["goarch"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| std::env::var("GOARCH").ok())
            .unwrap_or_else(|| {
                match std::env::consts::ARCH {
                    "x86_64" => "amd64",
                    "aarch64" => "arm64",
                    "x86" => "386",
                    other => other,
                }
                .to_owned()
            });
        self.build_target = crate::buildconstraint::Target::new(&os, &arch);
        self.cgo_disabled = std::env::var("CGO_ENABLED").is_ok_and(|value| value == "0");
        if !self.cgo_disabled {
            self.build_target.enable_tag("cgo");
        }
        let experiments = match options.get("goexperiment").filter(|value| !value.is_null()) {
            Some(value) => value
                .as_str()
                .ok_or((
                    crate::error::INVALID_PARAMS,
                    "goexperiment must be a string".into(),
                ))?
                .to_owned(),
            None => std::env::var("GOEXPERIMENT").unwrap_or_default(),
        };
        self.build_target.configure_experiments(&experiments);
        self.build_target.enable_tag("gc");
        for version in 1..=27 {
            self.build_target.enable_tag(&format!("go1.{version}"));
        }
        if let Some(tags) = options["buildTags"].as_array() {
            for tag in tags.iter().filter_map(Value::as_str) {
                self.build_target.enable_tag(tag);
            }
        }
        self.sdk_root = sdk::discover(options, &go_env)
            .map_err(|error| (crate::error::INVALID_PARAMS, error))?;
        self.started_at = Some(crate::index::DiskStamp::now_nanos());
        let roots = params["workspaceFolders"]
            .as_array()
            .map(|folders| {
                folders
                    .iter()
                    .filter_map(|f| path(&f["uri"]))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| path(&params["rootUri"]).into_iter().collect());
        self.scan_roots(roots)
            .map_err(|error| (crate::error::INVALID_PARAMS, error))?;
        Ok(())
    }
}

/// `go env` is only consulted when initialization options and the environment do
/// not already settle the SDK root and the module cache. The SDK option is
/// explicit only when present and non-null (`""` disables SDK discovery); the
/// module cache has no option, and a nonempty `GOMODCACHE` already matches the
/// toolchain's answer because `go env` honors the process environment.
fn needs_go_env(options: &Value) -> bool {
    let explicit_goroot = options.get("goroot").is_some_and(|value| !value.is_null());
    let explicit_gomodcache = std::env::var_os("GOMODCACHE").is_some_and(|value| !value.is_empty());
    !explicit_goroot || !explicit_gomodcache
}

fn watch_limit(options: &Value, key: &str, default: usize) -> Result<usize, (i32, String)> {
    let Some(value) = options.get(key).filter(|value| !value.is_null()) else {
        return Ok(default);
    };
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            (
                crate::error::INVALID_PARAMS,
                format!("{key} must be a non-negative integer fitting this platform's usize"),
            )
        })
}

fn watch_excludes(options: &Value) -> Result<super::file_events::WatchExcludes, (i32, String)> {
    let invalid = |message: String| {
        (
            crate::error::INVALID_PARAMS,
            format!("fileWatchExcludeDirectories: {message}"),
        )
    };
    let Some(value) = options
        .get("fileWatchExcludeDirectories")
        .filter(|value| !value.is_null())
    else {
        return Ok(super::file_events::WatchExcludes::default());
    };
    let values = value
        .as_array()
        .ok_or_else(|| invalid("must be an array of regex strings".into()))?;
    if values.len() > 128 {
        return Err(invalid("at most 128 patterns are supported".into()));
    }
    let mut patterns = Vec::with_capacity(values.len());
    let mut bytes = 0usize;
    for value in values {
        let pattern = value
            .as_str()
            .ok_or_else(|| invalid("every pattern must be a string".into()))?;
        bytes = bytes.saturating_add(pattern.len());
        if bytes > 65536 {
            return Err(invalid("patterns exceed 65536 bytes".into()));
        }
        patterns.push(pattern);
    }
    if patterns.is_empty() {
        return Ok(super::file_events::WatchExcludes::default());
    }
    let compiled = regex::RegexSetBuilder::new(patterns)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|error| invalid(error.to_string()))?;
    Ok(super::file_events::WatchExcludes(Some(compiled)))
}
