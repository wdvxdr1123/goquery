//! One bounded, initialization-time `go env` probe for default Go paths.
//!
//! Editors no longer have to duplicate machine-specific `GOROOT`/`GOMODCACHE`
//! paths in `initializationOptions`: the toolchain's own view is authoritative.
//! The probe is best-effort and never fails initialization. A missing, failing
//! or slow `go` binary yields no defaults, and callers fall back to the process
//! environment and the installed layout.
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Wall-clock budget for the whole probe, including pipe draining. `go env` is
/// a local configuration read; anything slower is treated as unavailable.
const TIMEOUT: Duration = Duration::from_secs(2);
/// The two requested path values are tiny; anything larger is not a value.
const MAX_STDOUT_BYTES: usize = 64 * 1024;
const MAX_STDERR_BYTES: usize = 8 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Defaults reported by the one `go env` invocation of an initialization.
#[derive(Default)]
pub(super) struct GoEnv {
    pub(super) goroot: Option<PathBuf>,
    pub(super) gomodcache: Option<PathBuf>,
}

/// Queries `GOROOT` and `GOMODCACHE` from the `go` binary on `PATH`.
///
/// The result is used only by the current initialization; nothing is cached
/// across requests, so changing the environment or toolchain takes effect on the
/// next initialization.
pub(super) fn probe() -> GoEnv {
    read_environment()
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output).ok())
        .map(|value| GoEnv {
            goroot: path(&value, "GOROOT"),
            gomodcache: path(&value, "GOMODCACHE"),
        })
        .unwrap_or_default()
}

fn path(value: &serde_json::Value, key: &str) -> Option<PathBuf> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Runs `go env -json GOROOT GOMODCACHE` with a deadline and bounded output.
///
/// `GOTOOLCHAIN=local` keeps the child from switching (or downloading) a
/// toolchain, and `GOWORK=off` keeps an unrelated or broken workspace file from
/// affecting the defaults. Both pipes are drained on a helper thread so a stuck
/// child cannot block initialization past the deadline.
fn read_environment() -> Option<Vec<u8>> {
    let mut child = Command::new("go")
        .args(["env", "-json", "GOROOT", "GOMODCACHE"])
        .env("GOTOOLCHAIN", "local")
        .env("GOWORK", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let read = stdout
            .take(MAX_STDOUT_BYTES as u64 + 1)
            .read_to_end(&mut output)
            .and_then(|_| {
                stderr
                    .take(MAX_STDERR_BYTES as u64)
                    .read_to_end(&mut errors)
            });
        let _ = sender.send((read.is_ok(), output));
    });
    let deadline = Instant::now() + TIMEOUT;
    let Ok((read_ok, output)) = receiver.recv_timeout(TIMEOUT) else {
        stop(&mut child);
        let _ = reader.join();
        return None;
    };
    let status = wait(&mut child, deadline);
    let _ = reader.join();
    (read_ok && output.len() <= MAX_STDOUT_BYTES && status.is_some_and(|status| status.success()))
        .then_some(output)
}

/// Waits for exit until `deadline`, killing the child when the budget is gone.
fn wait(child: &mut Child, deadline: Instant) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    stop(child);
                    return None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return None,
        }
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
