#[path = "stdio/ast_shape_differential.rs"]
mod ast_shape_differential;
#[path = "stdio/frontend.rs"]
mod frontend;
#[path = "stdio/go_corpus.rs"]
mod go_corpus;
#[path = "stdio/golden.rs"]
mod golden;
#[path = "stdio/harness.rs"]
mod harness;
#[path = "stdio/negative_syntax_differential.rs"]
mod negative_syntax_differential;
#[path = "stdio/recovered_structure_differential.rs"]
mod recovered_structure_differential;
#[path = "stdio/sema_e2e.rs"]
mod sema_e2e;
#[path = "stdio/watched.rs"]
mod watched;

#[test]
fn stdio_lifecycle_and_unsaved_navigation() {
    golden::check_tower_transport();
    for case in golden::GOLDEN_CASES {
        golden::run_case(case);
    }
    frontend::check_all();
}

/// Large external corpus gate, explicitly enabled after preparing the pinned source.
#[test]
#[ignore = "prepare the pinned Go source corpus and set GOQUERY_GO_SOURCE"]
fn go_source_package_corpus() {
    go_corpus::run_package_corpus();
}

/// External cross-package edits (agent or shell writes) must be visible without
/// the editor opening the changed files: the server registers file watching,
/// reindexes watched files, and republishes diagnostics for open documents in
/// the changed package and its direct importers.
#[test]
fn watched_file_changes_refresh_cross_package_state() {
    watched::run_all();
}
