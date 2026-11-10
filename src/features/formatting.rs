//! Document formatting delegates to the configured `gofmt` executable.
//!
//! Formatting is the only deliberate Go-tool invocation in the server: semantic
//! requests never invoke Go or gopls. The current open-buffer text is written to
//! a fresh child process on stdin and the formatted bytes are read back, so the
//! request follows gofmt (including import sorting and CRLF normalization).
//!
//! gopls formats with `format.Node`, which does not sort imports. For files that
//! are already gofmt-formatted the two agree exactly and this feature returns no
//! edits. For changed buffers the resulting text is gofmt-equivalent; the edit
//! list itself is a single replacement instead of gopls' internal diff.
//!
//! Resource bounds: at most 15 MiB of buffer text, at most 16 MiB of child
//! output, and a 5 second wall-clock budget after which the child is killed.
//! Nothing (source, output, handles or the child itself) is retained afterwards.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::analysis::query::Context;

/// Same single-file budget the workspace uses for closed-source reads.
const MAX_SOURCE_BYTES: usize = 15 * 1024 * 1024;
/// Formatted output cannot usefully exceed the input; keep the read bounded.
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound for diagnostics read from the child.
const MAX_STDERR_BYTES: usize = 8 * 1024;
/// Wall-clock budget for one formatter invocation, including pipe draining.
const TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(2);

enum Outcome {
    /// gofmt produced a complete formatted document.
    Formatted(String),
    /// gofmt rejected the source; the message is gofmt's parser diagnostic.
    Rejected(String),
    /// Spawn/IO/timeout/output-budget failure: callers report no edit result.
    Unavailable,
}

pub(super) fn execute(context: &mut Context<'_>, _params: &Value) -> Result<Value, (i32, String)> {
    let source = context.source();
    if source.len() > MAX_SOURCE_BYTES {
        return Ok(Value::Null);
    }
    let Some(gofmt) = gofmt_executable(context) else {
        return Ok(Value::Null);
    };
    match run(&gofmt, source) {
        Outcome::Formatted(formatted) => {
            if formatted == source {
                // gopls returns an empty edit list for already formatted files.
                Ok(json!([]))
            } else {
                Ok(json!([{
                    "range": whole_document_range(source),
                    "newText": formatted,
                }]))
            }
        }
        // gopls surfaces gofmt's parser diagnostic as a JSON-RPC error whose
        // code is 0 and whose message is the bare `file:line:col: ...` text.
        Outcome::Rejected(message) => Err((0, message)),
        Outcome::Unavailable => Ok(Value::Null),
    }
}

/// Position range spanning the complete document, in UTF-16 LSP coordinates.
fn whole_document_range(source: &str) -> Value {
    let line = source.bytes().filter(|byte| *byte == b'\n').count() as u64;
    let character = source
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .encode_utf16()
        .count() as u64;
    json!({
        "start": {"line": 0, "character": 0},
        "end": {"line": line, "character": character},
    })
}

/// Locates the formatter: configured SDK first, then `GOROOT`, then `PATH`.
fn gofmt_executable(context: &Context<'_>) -> Option<PathBuf> {
    if let Some(root) = context.workspace().sdk_root_path() {
        // The workspace stores the SDK source root (`<goroot>/src`); the
        // gofmt binary lives in `<goroot>/bin`.
        let bin = if root.file_name() == Some(std::ffi::OsStr::new("src")) {
            root.parent().map(|parent| parent.join("bin"))
        } else {
            Some(root.join("bin"))
        };
        if let Some(found) = bin.as_deref().and_then(executable_in) {
            return Some(found);
        }
    }
    if let Some(root) = std::env::var_os("GOROOT")
        && let Some(found) = executable_in(&PathBuf::from(root).join("bin"))
    {
        return Some(found);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|directory| executable_in(&directory))
}

fn executable_in(dir: &Path) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["gofmt.exe", "gofmt"]
    } else {
        &["gofmt"]
    };
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

/// Formats `source` with one bounded `gofmt` child process (`stdin` -> `stdout`).
fn run(gofmt: &Path, source: &str) -> Outcome {
    let Ok(mut child) = Command::new(gofmt)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Outcome::Unavailable;
    };

    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        let _ = child.wait();
        return Outcome::Unavailable;
    };
    if stdin.write_all(source.as_bytes()).is_err() || stdin.flush().is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Outcome::Unavailable;
    }
    drop(stdin);

    // Drain both pipes on a helper thread so a stuck child cannot block the
    // request thread past its deadline.
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut stderr_output = Vec::new();
        let read = stdout
            .take(MAX_OUTPUT_BYTES as u64 + 1)
            .read_to_end(&mut output)
            .and_then(|_| {
                stderr
                    .take(MAX_STDERR_BYTES as u64)
                    .read_to_end(&mut stderr_output)
            });
        let _ = sender.send((read.is_ok(), output, stderr_output));
    });

    let deadline = Instant::now() + TIMEOUT;
    let received = receiver.recv_timeout(TIMEOUT);
    let Ok((read_ok, output, stderr_output)) = received else {
        let _ = child.kill();
        let _ = child.wait();
        return Outcome::Unavailable;
    };
    let status = wait_with_deadline(&mut child, deadline);
    let _ = reader.join();
    if !read_ok || output.len() > MAX_OUTPUT_BYTES {
        return Outcome::Unavailable;
    }
    let Some(status) = status else {
        return Outcome::Unavailable;
    };
    if status.success() {
        return match String::from_utf8(output) {
            Ok(formatted) => Outcome::Formatted(formatted),
            Err(_) => Outcome::Unavailable,
        };
    }
    Outcome::Rejected(formatter_diagnostic(&stderr_output))
}

/// Reads gofmt's diagnostic without the `<standard input>:` prefix that gopls
/// does not include in its JSON-RPC error message.
fn formatter_diagnostic(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let first = text.lines().next().unwrap_or_default().trim();
    let message = first
        .strip_prefix("<standard input>:")
        .unwrap_or(first)
        .trim();
    if message.is_empty() {
        "gofmt rejected the document".to_string()
    } else {
        message.to_string()
    }
}

/// Waits for exit until `deadline`, killing the child when the budget is gone.
fn wait_with_deadline(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(POLL);
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{formatter_diagnostic, whole_document_range};

    #[test]
    fn whole_document_range_uses_utf16_positions() {
        let range = whole_document_range("package p\n\nvar x = \"e\"\n");
        assert_eq!(range["start"]["line"], 0);
        assert_eq!(range["start"]["character"], 0);
        assert_eq!(range["end"]["line"], 3);
        assert_eq!(range["end"]["character"], 0);

        let range = whole_document_range("var x = \"\u{1f600}\"");
        assert_eq!(range["end"]["line"], 0);
        assert_eq!(range["end"]["character"], 12);
    }

    #[test]
    fn formatter_diagnostic_strips_stdin_prefix() {
        assert_eq!(
            formatter_diagnostic(b"<standard input>:3:14: expected ')', found '{'\n"),
            "3:14: expected ')', found '{'"
        );
        assert_eq!(formatter_diagnostic(b""), "gofmt rejected the document");
    }
}
