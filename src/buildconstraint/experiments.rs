//! Go 1.27.3 experiment defaults and explicit GOEXPERIMENT overrides.
use std::collections::BTreeSet;

pub(crate) fn tags(goos: &str, goarch: &str, configuration: &str) -> BTreeSet<String> {
    let regabi = matches!(
        goarch,
        "amd64" | "arm64" | "loong64" | "ppc64le" | "ppc64" | "riscv64" | "s390x"
    );
    let mut enabled = BTreeSet::from([
        "randomizedheapbase64",
        "greenteagc",
        "jsonv2",
        "sizespecializedmalloc",
    ]);
    if !matches!(goos, "darwin" | "ios" | "aix") {
        enabled.insert("dwarf5");
    }
    for item in configuration.split(',') {
        if item.is_empty() {
            continue;
        }
        if item == "none" {
            enabled.clear();
            continue;
        }
        let (name, value) = item
            .strip_prefix("no")
            .map_or((item, true), |name| (name, false));
        if name == "regabi" {
            for name in ["regabiargs", "regabiwrappers"] {
                if value {
                    enabled.insert(name);
                } else {
                    enabled.remove(name);
                }
            }
        } else if value {
            // Experiment names are not validated against a fixed KNOWN list:
            // every name is accepted as a `goexperiment.<name>` tag.
            enabled.insert(name);
        } else {
            enabled.remove(name);
        }
    }
    // Register ABI is forced on for supported targets, even after `none` or
    // `noregabi`; unsupported targets must never advertise either ABI tag.
    for name in ["regabiargs", "regabiwrappers"] {
        if regabi {
            enabled.insert(name);
        } else {
            enabled.remove(name);
        }
    }
    enabled
        .into_iter()
        .map(|name| format!("goexperiment.{name}"))
        .collect()
}
