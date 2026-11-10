//! Go language-server library. The server binary is a thin stdio entry point;
//! the reusable Go syntax frontend and gopls-compatible fuzzy matchers are
//! exposed as [`syntax`] and [`fuzzy`] for centralized integration tests and
//! external probes.

mod analysis;
mod bloom;
mod buildconstraint;
mod error;
mod features;
pub mod fuzzy;
mod gomod;
mod index;
mod server;
pub mod syntax;
mod text;
mod workspace;

/// Run the language server over stdin/stdout until LSP exit.
pub fn run() -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(2)
        .build()?;
    let code = runtime.block_on(server::serve(tokio::io::stdin(), tokio::io::stdout()));
    // Tokio's stdin can have a pending blocking read after the LSP exit. The
    // server has drained/flushed its output; do not wait for stdin to close.
    std::process::exit(code);
}
