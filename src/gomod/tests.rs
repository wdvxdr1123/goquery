//! Semantic directive checks (Go modfile equivalents), not syntax-tree snapshots.
use std::cmp::Ordering;

#[test]
fn directives_and_cache_identities() {
    let parsed = crate::gomod::parse(
        r#"module "example.com/main"
go 1.25.0
toolchain go1.27.3
require (
    example.com/Old v1.2.3 // indirect
    example.com/lib/v2 v2.0.0-rc.1
)
replace (
    example.com/Old => example.com/New v1.10.0
    example.com/lib/v2 v2.0.0-rc.1 => "../local directory"
)
exclude example.com/Old v1.1.0
retract [v1.0.0, v1.0.1]
tool example.com/tools/cmd/check
godebug default=go1.25
"#,
    )
    .expect("valid Go module directives");
    assert_eq!(parsed.module.as_deref(), Some("example.com/main"));
    assert_eq!(parsed.go.as_deref(), Some("1.25.0"));
    assert_eq!(
        parsed.requires[0],
        ("example.com/Old".into(), "v1.2.3".into())
    );
    assert_eq!(parsed.replacements[0].target, "example.com/New");
    assert_eq!(parsed.replacements[1].target, "../local directory");
    assert_eq!(
        parsed.excludes,
        [("example.com/Old".into(), "v1.1.0".into())]
    );
    assert_eq!(parsed.tools, ["example.com/tools/cmd/check"]);
    let work =
        crate::gomod::parse("go 1.25\nuse (\n ./a\n \"./b b\"\n)\nreplace example.com/a => ./a\n")
            .expect("workspace directives");
    assert_eq!(work.uses, ["./a", "./b b"]);
    for source in [
        "module a\nmodule b",
        "require (\n a v1.0.0",
        ")",
        "replace x =>",
        "require x",
        "module \"unterminated",
        "go 1.25\ngo 1.26",
        "unknown x",
        "require a v1.0.0 /* comment */",
    ] {
        assert!(
            crate::gomod::parse(source).is_err(),
            "malformed module input: {source}"
        );
    }
    for (left, right) in [
        ("v1.9.0", "v1.10.0"),
        ("v1.0.0-rc.2", "v1.0.0-rc.10"),
        ("v1.0.0-rc.1", "v1.0.0"),
        (
            "v0.0.0-20200101000000-aaaaaaaaaaaa",
            "v0.0.0-20210101000000-bbbbbbbbbbbb",
        ),
        ("v1.0.0-2", "v1.0.0-a"),
    ] {
        assert_eq!(
            crate::gomod::compare_versions(left, right),
            Some(Ordering::Less),
            "Go semantic precedence: {left}, {right}"
        );
    }
    assert_eq!(
        crate::gomod::compare_versions("v2.0.0+incompatible", "v2.0.0"),
        Some(Ordering::Equal)
    );
    assert_eq!(crate::gomod::compare_versions("v1.02.0", "v1.2.0"), None);
    assert_eq!(
        crate::gomod::escape_cache_path("github.com/Azure/SDK").as_deref(),
        Some("github.com/!azure/!s!d!k")
    );
    for value in [
        "../escape",
        "/absolute",
        "example.com/a/../../b",
        "C:/escape",
        "a\\b",
        "x!y",
    ] {
        assert!(
            crate::gomod::escape_cache_path(value).is_none(),
            "unsafe cache key: {value}"
        );
    }
}
