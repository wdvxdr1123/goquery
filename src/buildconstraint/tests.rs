//! Go-oracle checks for build constraints, filename selection and experiments.
use super::Target;
use super::expression::parse;
use super::header::from_header;
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

fn official_acceptance(cases: &[String], mode: &str) -> Vec<bool> {
    serde_json::from_value(official_output(cases, mode)).expect("oracle acceptance results")
}

fn official_output(cases: &[String], mode: &str) -> Value {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new("go");
    command.arg("run").arg(base.join("tests/oracle.go"));
    if !mode.is_empty() {
        command.arg(mode);
    }
    let mut oracle = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Go SDK required for syntax oracle");
    oracle
        .stdin
        .take()
        .expect("oracle stdin")
        .write_all(&serde_json::to_vec(&cases).expect("encode corpus"))
        .expect("send corpus");
    let output = oracle.wait_with_output().expect("wait for Go oracle");
    assert!(
        output.status.success(),
        "Go oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle output")
}

#[test]
fn constraints_match_go_oracle() {
    let cases: Vec<String> = [
        "linux && amd64",
        "windows || linux && !amd64",
        "(windows || linux) && feature",
        "!(!linux)",
        "linux ||",
        "linux & amd64",
        "",
        "linux other",
        "linux && (amd64 || arm64)",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let expected = official_acceptance(&cases, "--constraints");
    for (source, expected) in cases.iter().zip(expected) {
        let result = parse(source)
            .is_ok_and(|c| c.matches(|tag| matches!(tag, "linux" | "amd64" | "feature")));
        assert_eq!(result, expected, "constraint differs from Go: {source}");
    }
    assert!(
        parse("linux ||").is_err(),
        "invalid constraints must not be treated as false expressions"
    );
    assert!(
        from_header("//go:build linux\n//go:build windows\npackage p").is_err(),
        "duplicate directives must be rejected"
    );
    assert!(
        from_header("/* //go:build windows */\npackage p")
            .expect("comment header")
            .is_none(),
        "block comments must not introduce constraints"
    );
    assert!(
        from_header("package p\n//go:build windows")
            .expect("package terminates header")
            .is_none(),
        "body comments must not introduce constraints"
    );
}

#[test]
fn build_selection_matches_go_oracle() {
    let mut cases = Vec::new();
    for platform in [
        "windows/amd64",
        "linux/arm64",
        "android/arm64",
        "ios/arm64",
        "illumos/amd64",
        "wasip1/wasm",
        "unknown/amd64",
    ] {
        for name in [
            "main.go",
            "linux.go",
            "main_linux.go",
            "main_windows_amd64.go",
            "main_arm64_test.go",
            "main_darwin.go",
            "main_solaris.go",
            "main_linux_test.go",
            "main_unix.go",
            "main_mips64p32le.go",
            "main_linux.extra.go",
            "_main.go",
            ".main.go",
            "main.txt",
            "main_amd64_linux.go",
            "main_linux_unknown.go",
        ] {
            for expression in [
                "",
                "unix && feature",
                "linux || darwin || solaris",
                "go1.27 && gc",
            ] {
                cases.push(format!("{platform}|{name}|{expression}"));
            }
        }
    }
    let accepted = official_acceptance(&cases, "--match-files");
    for (case, expected) in cases.iter().zip(accepted) {
        let mut parts = case.splitn(3, '|');
        let (goos, goarch) = parts
            .next()
            .expect("target")
            .split_once('/')
            .expect("platform pair");
        let name = parts.next().expect("filename");
        let expression = parts.next().expect("build expression");
        let source = if expression.is_empty() {
            "package p".to_owned()
        } else {
            format!("//go:build {expression}\n\npackage p")
        };
        let mut target = Target::new(goos, goarch);
        for tag in ["feature", "gc", "go1.27"] {
            target.enable_tag(tag);
        }
        assert_eq!(
            target
                .matches_file(name, &source)
                .expect("valid constraint"),
            expected,
            "build selection differs from Go for {case}"
        );
    }
}

#[test]
fn experiment_selection_matches_go_oracle() {
    let version = Command::new("go")
        .arg("version")
        .output()
        .expect("Go experiment oracle");
    assert!(
        version.status.success()
            && String::from_utf8_lossy(&version.stdout)
                .split_whitespace()
                .any(|part| part == "go1.27.3"),
        "experiment oracle requires Go 1.27.3"
    );
    for (os, arch, configuration) in [
        ("windows", "amd64", ""),
        ("linux", "386", ""),
        ("darwin", "arm64", ""),
        ("aix", "ppc64", ""),
        ("windows", "amd64", "none,noregabi"),
        ("linux", "386", "regabi,arenas,boringcrypto"),
        ("linux", "arm64", "nojsonv2,nogreenteagc"),
        ("linux", "386", "none,jsonv2,nojsonv2,jsonv2,simd"),
    ] {
        let output = Command::new("go")
            .args(["list", "-json", "internal/goexperiment"])
            .env("GOOS", os)
            .env("GOARCH", arch)
            .env("GOEXPERIMENT", configuration)
            .env("GOWORK", "off")
            .env("GOTOOLCHAIN", "local")
            .output()
            .expect("Go experiment file selection");
        let mut target = Target::new(os, arch);
        target.configure_experiments("");
        target.configure_experiments(configuration);
        let report: Value = serde_json::from_slice(&output.stdout).expect("Go experiment package");
        let mut compared = 0;
        for file in report["GoFiles"].as_array().expect("selected Go files") {
            let Some(name) = file.as_str().expect("Go filename").strip_prefix("exp_") else {
                continue;
            };
            let (experiment, enabled) = if let Some(name) = name.strip_suffix("_on.go") {
                (name, true)
            } else if let Some(name) = name.strip_suffix("_off.go") {
                (name, false)
            } else {
                continue;
            };
            assert_eq!(
                target.matches_tag(&format!("goexperiment.{experiment}")),
                enabled,
                "{os}/{arch}/{configuration}: {experiment}"
            );
            compared += 1;
        }
        assert!(compared >= 20, "oracle must check every pinned experiment");
    }
    // Unknown experiment names are accepted as tags instead of rejecting the
    // configuration; `no<name>` removes a previously enabled name from that
    // configuration's fresh default set.
    let mut target = Target::new("linux", "386");
    target.configure_experiments("madeup");
    assert!(target.matches_tag("goexperiment.madeup"));
    target.configure_experiments("madeup,nomadeup");
    assert!(!target.matches_tag("goexperiment.madeup"));
}
