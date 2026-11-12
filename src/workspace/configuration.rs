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
        self.poll_interval_ms = match options
            .get("pollIntervalMs")
            .filter(|value| !value.is_null())
        {
            Some(value) => value.as_u64().ok_or((
                crate::error::INVALID_PARAMS,
                "pollIntervalMs must be a non-negative integer".into(),
            ))?,
            None => std::env::var("GOQUERY_POLL_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(1000),
        };
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
