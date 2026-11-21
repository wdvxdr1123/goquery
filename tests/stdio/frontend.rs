use crate::go_corpus::{official_acceptance, official_output};
use serde_json::Value;
use std::path::Path;

pub fn check_all() {
    check_symbol_matcher();
    check_completion_matcher();
    check_syntax_corpus();
    check_block_corpus();
    check_file_corpus();
    check_numeric_literals();
    check_identifier_categories();
    check_lexical_tokens();
    check_signature_corpus();
    check_type_corpus();
    check_aggregate_limits();
}

fn frontend_fixture(file: &str) -> String {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(base.join("tests/fixtures/frontend").join(file))
        .unwrap_or_else(|error| panic!("read frontend fixture {file}: {error}"))
}

fn check_syntax_corpus() {
    let mut cases = Vec::new();
    let mut expected_acceptance = Vec::new();
    for file in ["operators.go", "postfix.go", "composites.go", "invalid.go"] {
        for line in frontend_fixture(file).lines() {
            if let Some(expression) = line.strip_prefix("var _ = ") {
                cases.push(expression.to_owned());
                expected_acceptance.push(file != "invalid.go");
            }
        }
    }
    for source in ["\"\\\n\"", "`\0`", "\"\u{feff}\"", "'\0'"] {
        cases.push(source.to_owned());
        expected_acceptance.push(false);
    }
    let accepted = official_acceptance(&cases, "");
    assert_eq!(
        accepted, expected_acceptance,
        "Go source corpus validity must match its category"
    );
    assert_eq!(
        accepted.len(),
        cases.len(),
        "oracle must inspect every expression"
    );
    for (source, valid) in cases.iter().zip(accepted) {
        let tree = goquery::syntax::parse_expression(source);
        assert_eq!(
            tree.diagnostics().is_empty(),
            valid,
            "Go parser acceptance differs for {source:?}: {:?}",
            tree.diagnostics()
        );
        for span in tree
            .nodes()
            .iter()
            .map(|n| &n.span)
            .chain(tree.diagnostics().iter().map(|d| &d.span))
        {
            assert!(
                span.start <= span.end && source.get(span.clone()).is_some(),
                "parser span must refer to valid source boundaries: {span:?} in {source:?}"
            );
        }
    }
    let nested = format!("{}x{}", "(".repeat(256), ")".repeat(256));
    assert!(
        !goquery::syntax::parse_expression(&nested)
            .diagnostics()
            .is_empty(),
        "deep nesting must be diagnosed without exhausting the stack"
    );
}

fn check_block_corpus() {
    let mut cases = Vec::new();
    let mut expected = Vec::new();
    for (file, valid) in [("blocks.go", true), ("blocks_invalid.go", false)] {
        for line in frontend_fixture(file)
            .lines()
            .filter(|line| line.starts_with("func "))
        {
            cases.push(line[line.find('{').expect("corpus function body")..].to_owned());
            expected.push(valid);
        }
    }
    let accepted = official_acceptance(&cases, "--blocks");
    assert_eq!(
        accepted, expected,
        "official parser must confirm block corpus categories"
    );
    for (source, valid) in cases.iter().zip(accepted) {
        let tree = goquery::syntax::parse_block(source);
        assert_eq!(
            tree.diagnostics().is_empty(),
            valid,
            "block acceptance differs for {source:?}: {:?}",
            tree.diagnostics()
        );
        for entry in tree.type_parameters() {
            assert!(entry.declaration.index() < tree.statements().len());
            for parameter in &entry.parameters {
                assert!(source.get(parameter.annotation.clone()).is_some());
                assert!(
                    parameter
                        .name
                        .as_ref()
                        .is_some_and(|span| source.get(span.clone()).is_some())
                );
            }
        }
    }
    let deeply_nested = format!("{}{}", "{".repeat(256), "}".repeat(256));
    assert!(
        !goquery::syntax::parse_block(&deeply_nested)
            .diagnostics()
            .is_empty(),
        "block nesting must be bounded"
    );
}

fn check_file_corpus() {
    let mut cases: Vec<String> =
        serde_json::from_str(&frontend_fixture("files.json")).expect("Go source samples");
    for statement in [
        "go f",
        "defer 1",
        "go (f())",
        "defer (f())",
        "go int(0)",
        "defer []int(nil)",
        "go (f)()",
        "go f()",
        "defer f()",
        "_ = ~x",
        "if",
        "if }",
        "for",
        "for }",
        "switch",
        "switch }",
        "for range",
        "for range }",
    ] {
        cases.push(format!("package p\nfunc F() {{ {statement} }}"));
    }
    cases.push(format!(
        "package p\nfunc F() {{ if x {{}} {} else {{}} }}",
        "else if x {} ".repeat(400)
    ));
    let accepted = official_acceptance(&cases, "--files");
    assert!(
        accepted.contains(&true) && accepted.contains(&false),
        "file corpus needs positive and negative syntax"
    );
    for (source, accepted) in cases.iter().zip(accepted) {
        let diagnostics = goquery::syntax::validate_file(source);
        assert_eq!(
            diagnostics.is_empty(),
            accepted,
            "file acceptance differs for {source:?}: {diagnostics:?}"
        );
        for diagnostic in diagnostics {
            assert!(
                diagnostic.span.start <= diagnostic.span.end
                    && source.get(diagnostic.span).is_some(),
                "diagnostic must refer to this file"
            );
        }
    }
    // Truncate at every UTF-8 boundary, exercising editor snapshots independently
    // of acceptance. Check every public parser without snapshotting its trees.
    for source in [
        "package p\nfunc F(){ if x := f(); x { go f() } else if y { defer f() } }",
        "package p\nfunc F(){ for x := range values { switch x { case 1: select { case <-ch: } } } }",
        "package p\nfunc F(){ var 涓栫晫 = func(x map[string][]*T) { if x { } }; _ = 涓栫晫 }",
    ] {
        for end in source
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(source.len()))
        {
            let prefix = &source[..end];
            let diagnostics = std::panic::catch_unwind(|| {
                let mut diagnostics = goquery::syntax::validate_file(prefix);
                diagnostics.extend(goquery::syntax::parse_block(prefix).into_diagnostics());
                diagnostics.extend(goquery::syntax::parse_expression(prefix).into_diagnostics());
                diagnostics.extend(goquery::syntax::parse_type(prefix).into_diagnostics());
                diagnostics
            })
            .unwrap_or_else(|_| panic!("parser panic on {prefix:?}"));
            assert!(diagnostics.len() <= 4 * 4096);
            assert!(diagnostics.into_iter().all(|diagnostic| {
                diagnostic.span.start <= diagnostic.span.end
                    && prefix.get(diagnostic.span).is_some()
            }));
        }
    }
    assert!(!goquery::syntax::parse_type("~T").diagnostics().is_empty());
    let errors = goquery::syntax::validate_file(&format!("package p\n{}", "@;\n".repeat(5000)));
    assert!(
        !errors.is_empty() && errors.len() <= 4096,
        "malformed files must have bounded diagnostics"
    );
    let nested = format!(
        "package p\nfunc F() {{ {}return{} }}",
        "func(){".repeat(160),
        "}();".repeat(160)
    );
    assert!(
        !goquery::syntax::validate_file(&nested).is_empty(),
        "nested file validation must be bounded"
    );
}

fn check_lexical_tokens() {
    let streams: Vec<String> = [
        "0x1e+2; 1.foo; 3...; 123abc; 0b10+2; 0x1.fp-2; .5e+1i",
        "package p\nvar x = 1 /* comment\n */ +2\n",
        "\u{feff}package p\r\nvar 涓栫晫 = `raw\r\ntext`\n",
        "x++ // comment\n y := <-ch; z &^= 1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let mut sources: Vec<String> = "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var + - * / % & | ^ << >> &^ += -= *= /= %= &= |= ^= <<= >>= &^= && || <- ++ -- == < > = ! != <= >= := ... ( [ { , . ) ] } ; : ~"
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    sources.extend(
        [
            "value", "true", "0xdead", "1.0", "1e-3", "0x1.fp2", "3i", "'x'", "\"text\"", "`raw`",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    sources.extend(streams);
    let expected = official_output(&sources, "--token-streams");
    let expected = expected.as_array().expect("token streams");
    assert_eq!(sources.len(), expected.len());
    for (source, tokens) in sources.iter().zip(expected) {
        check_token_stream(source, tokens.as_array().expect("stream tokens"));
        let plain: Vec<_> = goquery::syntax::Lexer::new(source).collect();
        let with_comments: Vec<_> = goquery::syntax::Lexer::with_comments(source)
            .filter(|token| token.kind != goquery::syntax::TokenKind::Comment)
            .collect();
        assert_eq!(
            plain, with_comments,
            "comments changed semicolon insertion: {source:?}"
        );
    }
}

pub fn check_token_stream(source: &str, expected: &[Value]) {
    use goquery::syntax::TokenKind;
    let mut actual = goquery::syntax::Lexer::new(source);
    for expected in expected {
        let token = actual.next().expect("scanner token missing");
        let kind = token.kind.fixed_text().unwrap_or(match token.kind {
            TokenKind::Ident => "IDENT",
            TokenKind::Int => "INT",
            TokenKind::Float => "FLOAT",
            TokenKind::Imag => "IMAG",
            TokenKind::Char => "CHAR",
            TokenKind::String => "STRING",
            _ => "ILLEGAL",
        });
        assert_eq!(
            kind,
            expected["kind"].as_str().expect("token kind"),
            "token at byte {} in {source:?}",
            token.span.start
        );
        assert_eq!(
            token.span.start,
            expected["start"].as_u64().expect("token start") as usize
        );
        assert_eq!(
            token.span.end,
            expected["end"].as_u64().expect("token end") as usize
        );
        assert_eq!(
            token.inserted,
            expected["inserted"].as_bool().expect("semicolon insertion")
        );
    }
    assert!(actual.next().is_none(), "unexpected trailing scanner token");
}

fn check_signature_corpus() {
    let mut sources = Vec::new();
    let mut validity = Vec::new();
    for (file, valid) in [("signatures.go", true), ("invalid-signatures.go", false)] {
        for line in frontend_fixture(file)
            .lines()
            .filter(|line| line.starts_with("func "))
        {
            sources.push(line.to_owned());
            validity.push(valid);
        }
    }
    assert_eq!(official_acceptance(&sources, "--signatures"), validity);
    for (source, valid) in sources.iter().zip(validity) {
        let signature = goquery::syntax::parse_signature(source);
        assert_eq!(
            signature.diagnostics().is_empty(),
            valid,
            "signature {source:?}: {:?}",
            signature.diagnostics()
        );
        for parameter in signature
            .receiver
            .iter()
            .chain(&signature.type_parameters)
            .chain(&signature.parameters)
            .chain(&signature.results)
        {
            assert!(source.get(parameter.annotation.clone()).is_some());
            if let Some(name) = &parameter.name {
                assert!(source.get(name.clone()).is_some());
            }
        }
    }
    let nested = format!("func Deep(f {}int)", "func() ".repeat(256));
    assert!(
        !goquery::syntax::parse_signature(&nested)
            .diagnostics()
            .is_empty()
    );
}

fn check_type_corpus() {
    let mut cases = Vec::new();
    let mut validity = Vec::new();
    for (file, valid) in [("types.go", true), ("invalid-types.go", false)] {
        for line in frontend_fixture(file).lines() {
            if let Some(ty) = line.strip_prefix("type _ = ") {
                cases.push(ty.to_owned());
                validity.push(valid);
            }
        }
    }
    assert_eq!(official_acceptance(&cases, "--types"), validity);
    for (source, valid) in cases.iter().zip(validity) {
        let parsed = goquery::syntax::parse_type(source);
        assert_eq!(
            parsed.diagnostics().is_empty(),
            valid,
            "type {source:?}: {:?}",
            parsed.diagnostics()
        );
        check_type_spans(source, &parsed);
        if valid {
            let delimited = format!("{source}, bool");
            let prefix = goquery::syntax::parse_type_prefix(&delimited);
            assert!(prefix.diagnostics().is_empty(), "type prefix {source:?}");
            assert_eq!(prefix.span, parsed.span);
            check_type_spans(&delimited, &prefix);
            assert!(
                !goquery::syntax::parse_type(&delimited)
                    .diagnostics()
                    .is_empty()
            );
        }
    }
    let nested = format!("{}int{}", "struct{ Field ".repeat(256), "}".repeat(256));
    assert!(
        !goquery::syntax::parse_type(&nested)
            .diagnostics()
            .is_empty()
    );
}

pub fn check_type_spans(source: &str, parsed: &goquery::syntax::TypeSyntax) {
    for member in &parsed.interface_members {
        assert_eq!(source.as_bytes().get(member.container), Some(&b'{'));
        assert!(member.container < member.annotation.start);
        assert!(source.get(member.annotation.clone()).is_some());
        if let Some(name) = &member.name {
            assert!(member.container < name.start && name.end <= member.annotation.start);
            assert!(source.get(name.clone()).is_some());
        }
    }
    for reference in &parsed.references {
        assert!(source.get(reference.name.clone()).is_some());
        if let Some(qualifier) = &reference.qualifier {
            assert!(source.get(qualifier.clone()).is_some());
        }
    }
    for field in &parsed.fields {
        assert_eq!(source.as_bytes().get(field.container), Some(&b'{'));
        assert!(field.container < field.name.start);
        if field.embedded {
            assert!(field.annotation.start <= field.name.start);
            assert!(field.name.end <= field.annotation.end);
        } else {
            assert!(field.name.end <= field.annotation.start);
        }
        assert!(source.get(field.name.clone()).is_some());
        assert!(source.get(field.annotation.clone()).is_some());
        if let Some(tag) = &field.tag {
            assert!(field.annotation.end <= tag.start);
            assert!(goquery::syntax::decode_string_literal(&source[tag.clone()]).is_some());
        }
    }
}

// Exercise the matcher library's byte-offset and fixed-buffer boundaries;
// protocol cases above cover ranking, qualification, updates and result limits.
fn check_symbol_matcher() {
    let matcher = goquery::fuzzy::Matcher::new("srHS");
    assert!(matcher.score(&["SearchHTTPServer"]).is_some());
    assert!(matcher.score(&["unrelated"]).is_none());
    let matcher = goquery::fuzzy::Matcher::new("Method");
    let hit = matcher.score(&["涓栫晫.", "Box.", "Method"]).unwrap();
    assert_eq!(hit.start, "涓栫晫.Box.".len());
    assert!((hit.score - 5.5 / 6.0).abs() < 1e-12);
    assert!(goquery::fuzzy::Matcher::new("").score(&["Name"]).is_none());
    assert!(goquery::fuzzy::Matcher::new("Name").score(&[]).is_none());
    let long = "x".repeat(256);
    assert!(
        goquery::fuzzy::Matcher::new(&long)
            .score(&[&long])
            .is_some()
    );
    assert!(
        goquery::fuzzy::Matcher::new("Z")
            .score(&[&long, "Z"])
            .is_none()
    );
}

// Grouped nested anonymous fields can multiply the number of search paths.
fn check_aggregate_limits() {
    for declaration in [
        "= pkg.Box[struct { First int; Second string }]",
        "[T any] = Box[T]",
        "= *Box[int]",
    ] {
        let stub = goquery::syntax::stub(&format!("package p\ntype Alias {declaration}\n"));
        let target = stub.symbols[0]
            .type_info
            .as_ref()
            .and_then(|info| info.target.as_ref())
            .expect("named alias metadata");
        assert!(
            goquery::syntax::parse_type(&target.name)
                .diagnostics()
                .is_empty(),
            "{target:?}"
        );
    }
    for declaration in [
        "struct { Box[int] }",
        "[T any] struct { pkg.Box[T] }",
        "struct { Box[struct { First int; Second string }] }",
    ] {
        let stub = goquery::syntax::stub(&format!("package p\ntype Holder {declaration}\n"));
        let info = stub.symbols[0]
            .type_info
            .as_ref()
            .expect("aggregate metadata");
        assert_eq!(info.embedded.len(), 1, "{declaration}");
        for spelling in &info.embedded {
            assert!(
                goquery::syntax::parse_type(spelling)
                    .diagnostics()
                    .is_empty(),
                "{spelling}"
            );
        }
    }
    for source in [
        "struct { A, B }",
        "interface { Broken(",
        "struct { Field string `tag` }",
    ] {
        for member in goquery::syntax::aggregate_members(source) {
            assert!(source.get(member.selection).is_some());
            assert!(source.get(member.annotation).is_some());
        }
    }
    let mut source = "struct { Leaf int }".to_owned();
    for _ in 0..20 {
        source = format!("struct {{ A, B {source} }}");
    }
    let members = goquery::syntax::aggregate_members(&source);
    assert!(!members.is_empty() && members.len() <= 4096);
    assert!(
        members
            .iter()
            .map(|member| member.symbol.name.len())
            .sum::<usize>()
            <= 256 * 1024
    );
    for member in &members {
        assert!(source.get(member.selection.clone()).is_some());
        assert!(source.get(member.annotation.clone()).is_some());
        assert!(member.container <= member.selection.start);
    }
    let stub = goquery::syntax::stub(&format!("package p\ntype Bounded {source}\n"));
    let info = stub.symbols[0]
        .type_info
        .as_ref()
        .expect("aggregate metadata");
    assert!(info.search_members.len() <= 4096);
    assert!(
        info.search_members
            .iter()
            .map(|member| member.name.len())
            .sum::<usize>()
            <= 256 * 1024
    );
}

// Expected scores come from the pinned Go implementation, not the Rust port.
fn check_completion_matcher() {
    let oracle: Value =
        serde_json::from_str(include_str!("../fixtures/fuzzy/scores.json")).unwrap();
    for group in oracle["groups"].as_array().unwrap() {
        let pattern = group["pattern"].as_str().unwrap();
        let mut matcher = goquery::fuzzy::CompletionMatcher::new(pattern);
        for case in group["candidates"].as_array().unwrap() {
            let candidate = case["text"].as_str().unwrap();
            let actual = matcher.score(candidate);
            let expected = case["score"].as_f64().unwrap();
            assert!(
                (f64::from(actual) - expected).abs() < 1e-7,
                "{pattern:?}/{candidate:?}: {actual} vs {expected}"
            );
        }
    }
}

// Compare acceptance in expression, declaration, type and lazy nested-body paths.
fn check_numeric_literals() {
    let mut samples: Vec<String> = [
        "0",
        "00",
        "0755",
        "089",
        "08i",
        "0b10",
        "0B10i",
        "0b0190",
        "0b2i",
        "0o755",
        "0O755i",
        "0o9",
        "0o9i",
        "0xabc",
        "0XABi",
        "0x",
        "0b",
        "0o",
        "0x_",
        "0b_",
        "0o_",
        "1_",
        "1__0",
        "0x__1",
        "0_1",
        "0x_1",
        "0b_1",
        "0o_7",
        "1.25",
        ".5",
        "1.",
        "08.5",
        "0x1.fp2",
        "0x.8p-2",
        "0x1p0i",
        "0x1.0",
        "0x.p1",
        "0b1.0",
        "0o7.1i",
        "0b1e2",
        "0o1P-1",
        "1p2",
        "1e",
        "1E+",
        "1e-",
        "1e_2",
        "1e+_2",
        "1e2_",
        "1e2__3",
        "1e2i",
        "0x1p",
        "0x1p+",
        "0x1p_2",
        "0x1p+_2",
        "0x1p2_",
        "0x1p2__3",
        "1_.0",
        "1._0",
        "0x_1p0",
        "0x1_p0",
        "0x1p0_i",
        "0x1g",
        "0b01a0",
        "1+2i",
        "0x1e+2",
        "0x1e-2",
        "1...",
        "1..2",
        "0x1.2p3+4",
        "0x1.2e3",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for literal in [
        "0", "0755", "0b101", "0o755", "0xabc", "1.25", "1e+3", "0x1.fp-2", "10i",
    ] {
        for offset in 0..=literal.len() {
            let mut sample = literal.to_owned();
            sample.insert(offset, '_');
            samples.push(sample);
        }
    }
    samples.sort();
    samples.dedup();
    let accepted = official_acceptance(&samples, "");
    let mut files = Vec::new();
    let mut valid_tokens = Vec::new();
    for (sample, accepted) in samples.iter().zip(accepted) {
        let parsed = goquery::syntax::parse_expression(sample);
        assert_eq!(
            parsed.diagnostics().is_empty(),
            accepted,
            "numeric expression {sample:?}: {:?}",
            parsed.diagnostics()
        );
        if accepted {
            valid_tokens.push(sample.clone());
        }
        files.push(format!("package p\nvar X = {sample}\n"));
        files.push(format!("package p\ntype T [{sample}]byte\n"));
        files.push(format!(
            "package p\nfunc F() {{ func(){{ _ = {sample} }}() }}\n"
        ));
    }
    let accepted = official_acceptance(&files, "--files");
    for (source, accepted) in files.iter().zip(accepted) {
        let diagnostics = goquery::syntax::validate_file(source);
        assert_eq!(
            diagnostics.is_empty(),
            accepted,
            "numeric file {source:?}: {diagnostics:?}"
        );
        for diagnostic in diagnostics {
            assert!(source.get(diagnostic.span).is_some());
        }
    }
    let streams = official_output(&valid_tokens, "--token-streams");
    for (source, tokens) in valid_tokens
        .iter()
        .zip(streams.as_array().expect("numeric token streams"))
    {
        check_token_stream(source, tokens.as_array().expect("numeric tokens"));
    }
}

// Check the public lexer against Go categories for every Unicode scalar value.
fn check_identifier_categories() {
    let categories = official_output(&[], "--identifier-categories");
    assert_eq!(categories["Version"], "17.0.0", "pinned Go Unicode version");
    let mut expected = vec![0u8; 0x110000];
    for (category, bit) in [("Letters", 1), ("Digits", 2), ("Uppercase", 4)] {
        for width in ["R16", "R32"] {
            for range in categories[category][width]
                .as_array()
                .expect("Go category ranges")
            {
                let low = range["Lo"].as_u64().expect("lower bound") as usize;
                let high = range["Hi"].as_u64().expect("upper bound") as usize;
                let stride = range["Stride"].as_u64().expect("range stride") as usize;
                for value in (low..=high).step_by(stride) {
                    expected[value] |= bit;
                }
            }
        }
    }
    let mut bytes = [0u8; 5];
    bytes[0] = b'a';
    for (value, category) in expected.iter().enumerate() {
        let Some(character) = char::from_u32(value as u32) else {
            continue;
        };
        let length = 1 + character.encode_utf8(&mut bytes[1..]).len();
        let source = std::str::from_utf8(&bytes[..length]).expect("encoded scalar");
        assert_eq!(
            goquery::syntax::is_exported_name(&source[1..]),
            category & 4 != 0,
            "exported U+{value:04X}"
        );
        let start = goquery::syntax::Lexer::new(&source[1..])
            .next()
            .is_some_and(|token| {
                token.kind == goquery::syntax::TokenKind::Ident && token.span.end == length - 1
            });
        let continuation = goquery::syntax::Lexer::new(source)
            .next()
            .is_some_and(|token| {
                token.kind == goquery::syntax::TokenKind::Ident && token.span.end == length
            });
        assert_eq!(
            start,
            character == '_' || category & 1 != 0,
            "identifier start U+{value:04X}"
        );
        assert_eq!(
            continuation,
            character == '_' || *category != 0,
            "identifier continuation U+{value:04X}"
        );
    }
    let mut files = Vec::new();
    for value in [
        0xaa, 0xb2, 0x345, 0x5b0, 0x660, 0x203f, 0x2160, 0x10400, 0x104a0, 0x1d400, 0x31350,
    ] {
        let character = char::from_u32(value).expect("boundary scalar");
        files.push(format!("package p\nvar {character}Name = 1\n"));
        files.push(format!("package p\nvar a{character}Name = 1\n"));
    }
    let accepted = official_acceptance(&files, "--files");
    let mut valid = Vec::new();
    for (source, accepted) in files.iter().zip(accepted) {
        let diagnostics = goquery::syntax::validate_file(source);
        assert_eq!(
            diagnostics.is_empty(),
            accepted,
            "identifier file {source:?}: {diagnostics:?}"
        );
        if accepted {
            valid.push(source.clone());
        }
    }
    let streams = official_output(&valid, "--token-streams");
    for (source, stream) in valid
        .iter()
        .zip(streams.as_array().expect("identifier streams"))
    {
        check_token_stream(source, stream.as_array().expect("identifier tokens"));
    }
}
