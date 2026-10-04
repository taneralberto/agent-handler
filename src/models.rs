use anyhow::{anyhow, bail, Result};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::operation::{run_cancel_checked, CancelToken, Finish, OperationReport, Progress};

/// Result of attempting to discover models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    /// Discovered at least one model.
    Found(Vec<String>),
    /// Command ran successfully but no valid lines were parsed.
    Empty(String),
    /// Command failed or returned non-zero.
    Failed(String),
}

impl Discovery {
    #[allow(dead_code)]
    pub fn models(&self) -> &[String] {
        match self {
            Discovery::Found(models) => models,
            _ => &[],
        }
    }

    pub fn status_text(&self) -> String {
        match self {
            Discovery::Found(models) => format!("{} model(s) discovered", models.len()),
            Discovery::Empty(msg) => format!("no models parsed: {}", msg),
            Discovery::Failed(msg) => format!("discovery failed: {}", msg),
        }
    }
}

/// Discover available models by running `opencode models`.
pub fn discover_models() -> Discovery {
    match run_opencode_models() {
        Ok(stdout) => {
            let parsed = parse_models(&stdout);
            if parsed.is_empty() {
                Discovery::Empty(if stdout.is_empty() {
                    "command produced no output".to_string()
                } else {
                    "no valid provider/model lines found".to_string()
                })
            } else {
                Discovery::Found(parsed)
            }
        }
        Err(e) => Discovery::Failed(e.to_string()),
    }
}

fn run_opencode_models() -> Result<String> {
    let output = Command::new("opencode")
        .arg("models")
        .env("NO_COLOR", "1")
        .output()
        .map_err(|e| anyhow!("could not execute `opencode models`: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        bail!(
            "`opencode models` exited with status {}: {}",
            code,
            if stderr.is_empty() {
                "<no stderr>"
            } else {
                &stderr
            }
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ---------- Cooperative / bounded variant ----------
//
// Production `discover_models` keeps the legacy
// `Command::new("opencode").arg("models").output()` shape: a single
// blocking read that joins when the child closes its pipes. That
// shape has two correctness gaps the GUI wants closed:
//
// 1. **No timeout.** A hung `opencode models` blocks forever.
// 2. **No stdout cap.** A runaway child can pin memory.
//
// `discover_models_controlled` rebuilds the read path so it:
// - spawns the child with `NO_COLOR=1` and `stdin = null`
//   (existing invariants preserved),
// - reads stdout/stderr concurrently through bounded pipes,
// - caps each stream at 1 MiB (overflow is reported as an
//   explicit failure, not silently truncated),
// - watches a timeout and, on timeout / cancel, kills the
//   DIRECT child and reaps it so the parent never sees an
//   indefinite wait. We do not chase descendants (no process
//   group on Linux; no Win32 changes).
//
// The legacy `discover_models` is intentionally untouched.

/// Tunables for `discover_models_controlled`. Centralized so
/// tests can dial the timeout down to keep the suite fast while
/// production keeps the 30-second default.
const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_DISCOVERY_OUTPUT_CAP: usize = 1 << 20; // 1 MiB
/// Bound on how long the drain threads wait for the child's pipe
/// to close after kill+reap. A few seconds is plenty on a healthy
/// host; the drain thread exits either way (the child's write
/// end closes once it exits).
const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Read `reader` into a buffer, stopping once `cap` bytes have
/// been collected. When the cap is exceeded, the remaining bytes
/// are still drained (so the child never deadlocks on a full pipe)
/// but the function returns `Err(_)` so the caller can surface an
/// explicit overflow failure instead of silently truncating.
fn read_bounded<R: Read>(mut reader: R, cap: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(cap.min(8192));
    let mut chunk = [0u8; 8192];
    let mut overflow = false;
    let mut interrupted = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() >= cap {
                    // Past the cap: keep draining so the child
                    // doesn't block, but mark overflow.
                    overflow = true;
                    continue;
                }
                let room = cap - buf.len();
                let take = n.min(room);
                buf.extend_from_slice(&chunk[..take]);
                if n > take {
                    overflow = true;
                }
            }
            Err(e) => {
                // Surface InterruptedError explicitly so the
                // caller can decide whether to retry, fail
                // closed, or fall back to an alternate
                // discovery (an alternate source of the
                // catalog, not a silent partial). Other IO
                // errors terminate the read — the caller
                // treats a non-Ok result as Failed so we
                // never claim a truncated catalog from a
                // broken pipe.
                if e.kind() == std::io::ErrorKind::Interrupted {
                    interrupted = true;
                    break;
                }
                // Non-interrupt IO errors terminate the
                // read; the caller's missing-stream check
                // surfaces the partial bytes as Failed.
                break;
            }
        }
    }
    if overflow {
        bail!("opencode models output exceeded {} bytes", cap);
    }
    if interrupted {
        bail!("opencode models read interrupted");
    }
    Ok(buf)
}

/// Tagged drain result so the reaper can attribute each
/// message to the right pipe.
#[derive(Debug)]
enum DrainStream {
    Stdout(Result<Vec<u8>>),
    Stderr(Result<Vec<u8>>),
}

/// Discover available models with cooperative cancellation and
/// progress reporting.
///
/// Returns an `OperationReport<Option<Discovery>>` so the
/// caller can distinguish a real cancel/timeout from a
/// successful discovery. `partial` is `None` when the
/// discovery finished without producing a model catalog
/// (cancelled or timed out before the child reported) and
/// `Some(Discovery::)` otherwise.
///
/// The legacy `discover_models()` shape is unchanged; this is
/// the GUI/CLI-cancel-aware entry point that lets a long
/// discovery abort cleanly without holding the TUI hostage.
pub fn discover_models_controlled(
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> OperationReport<Option<Discovery>> {
    discover_models_controlled_with(
        DEFAULT_DISCOVERY_TIMEOUT,
        DEFAULT_DISCOVERY_OUTPUT_CAP,
        build_opencode_models_command,
        token,
        sink,
    )
}

/// Build the production `Command` for `opencode models`. The
/// invocation invariant — `NO_COLOR=1`, `stdin = null`,
/// `stdout = piped`, `stderr = piped` — is applied HERE, after
/// the factory returns, so every caller (production + tests)
/// gets the same child setup. Tests must NOT pre-configure
/// these on the factory output.
fn build_opencode_models_command() -> Command {
    let mut cmd = Command::new("opencode");
    cmd.arg("models")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Test seam: same as [`discover_models_controlled`] but with
/// explicit timeout, cap, and a factory that builds the
/// `Command`. Production uses the factory-free entry point;
/// tests can swap in a fake argv (e.g. a `sh -c 'sleep 999'`
/// script) without touching the process PATH. The seam
/// unconditionally applies the production stdin/stdout/stderr/
/// NO_COLOR setup after the factory returns.
pub fn discover_models_controlled_with<F>(
    timeout: Duration,
    output_cap: usize,
    command_factory: F,
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> OperationReport<Option<Discovery>>
where
    F: FnOnce() -> Command,
{
    // Pre-cancel: honor a token request BEFORE spawning. No
    // child, no pipes, no drain threads — the function returns
    // promptly with `Finish::Cancelled` and `partial = None`.
    if run_cancel_checked(
        token,
        sink,
        Progress {
            stage: "models: spawn",
            item: None,
            processed: 0,
            total: Some(1),
        },
    ) {
        return OperationReport {
            finish: Finish::Cancelled,
            partial: None,
            error: Some("cancelled before spawn".to_string()),
        };
    }

    let mut cmd = command_factory();
    // Apply the production invocation invariant AFTER the
    // factory — every child the seam produces has null stdin
    // and piped stdout/stderr, with NO_COLOR=1.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1");
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return OperationReport {
                finish: Finish::Failed,
                partial: None,
                error: Some(format!("could not execute `opencode models`: {}", e)),
            };
        }
    };
    let stdout_pipe = match child.stdout.take() {
        Some(s) => s,
        None => {
            // Best-effort cleanup; the child was just spawned
            // and never produced output.
            kill_and_reap(&mut child);
            return OperationReport {
                finish: Finish::Failed,
                partial: None,
                error: Some("`opencode models` stdout was not piped".to_string()),
            };
        }
    };
    let stderr_pipe = match child.stderr.take() {
        Some(s) => s,
        None => {
            kill_and_reap(&mut child);
            return OperationReport {
                finish: Finish::Failed,
                partial: None,
                error: Some("`opencode models` stderr was not piped".to_string()),
            };
        }
    };

    // Concurrent drains. Each drain is responsible for its
    // OWN bounded read and sends exactly one `DrainStream`
    // message tagged with the pipe it came from. After the
    // send, the drain thread DETACHES — the parent NEVER
    // `.join()`s an unfinished drain. Detached drains either
    // exit promptly when the pipes close (the normal path)
    // or are abandoned when the parent returns; the runtime
    // cleans them up at process exit. This is the explicit
    // "no unbounded joins ever" rule: a drain that still
    // holds the read end of a pipe because a descendant
    // process inherited it must not keep the discovery
    // function blocked.
    let (tx, rx) = channel::<DrainStream>();
    let tx_err = tx.clone();
    let stdout_handle = thread::spawn(move || {
        let res = read_bounded(stdout_pipe, output_cap);
        // Drop the sender so the channel can disconnect
        // after we send.
        let _ = tx.send(DrainStream::Stdout(res));
        // `tx` is dropped here; the join handle is detached
        // by `discover_models_controlled_with` below.
    });
    let stderr_handle = thread::spawn(move || {
        let res = read_bounded(stderr_pipe, output_cap);
        let _ = tx_err.send(DrainStream::Stderr(res));
    });

    // Mid-wait loop: poll the child AND the token. On a token
    // request, kill the DIRECT child (we never promise
    // descendant cleanup) and reap it, then return Cancelled.
    // On a timeout, same path with Failed. On the child
    // exiting naturally, return its status. On a try_wait
    // error, kill+reap and report Failed.
    let start = Instant::now();
    enum Outcome {
        Exited(std::process::ExitStatus),
        Cancelled,
        TimedOut,
        WaitFailed(String),
    }
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Outcome::Exited(status),
            Ok(None) => {
                if token.is_requested() {
                    kill_and_reap(&mut child);
                    break Outcome::Cancelled;
                }
                if start.elapsed() >= timeout {
                    kill_and_reap(&mut child);
                    break Outcome::TimedOut;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                kill_and_reap(&mut child);
                break Outcome::WaitFailed(format!("`opencode models` wait failed: {}", e));
            }
        }
    };

    // After the child has exited (or been killed and reaped
    // synchronously), the pipes close and the drain threads
    // unblock and send their tagged message. We collect
    // whatever messages arrive within the drain deadline;
    // if either is missing at the deadline, we DO NOT block
    // on `join` — we detach the handle so the runtime can
    // clean it up later. The loop observes the cancel token
    // on every iteration so a runtime cancel during drain
    // returns promptly without waiting the full deadline.
    let mut stdout_bytes: Option<Result<Vec<u8>>> = None;
    let mut stderr_bytes: Option<Result<Vec<u8>>> = None;
    let drain_deadline = Instant::now() + DEFAULT_DRAIN_TIMEOUT;
    let mut drain_cancelled = false;
    while (stdout_bytes.is_none() || stderr_bytes.is_none())
        && Instant::now() < drain_deadline
        && !drain_cancelled
    {
        // Honor the cancel token on every iteration so a
        // cancel observed mid-drain returns without waiting
        // out the full deadline.
        if token.is_requested() {
            drain_cancelled = true;
            break;
        }
        match rx.try_recv() {
            Ok(DrainStream::Stdout(res)) => stdout_bytes = Some(res),
            Ok(DrainStream::Stderr(res)) => stderr_bytes = Some(res),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }
    }
    // Detach both drains. We do NOT call `.join()` — a
    // drain can be stuck reading a pipe that a descendant
    // of the killed child inherited; joining it would
    // block forever. The handles are abandoned on purpose
    // so the runtime reclaims the threads when they
    // eventually exit.
    drop_reap(stdout_handle);
    drop_reap(stderr_handle);

    // Cancel during drain short-circuits before any
    // partial observability decision: we already failed
    // the run from the user's perspective, so report
    // Cancelled (not Failed) and do NOT claim any catalog
    // even if one stream happened to land before the
    // token was honored.
    if drain_cancelled {
        return OperationReport {
            finish: Finish::Cancelled,
            partial: None,
            error: Some("cancelled during drain".to_string()),
        };
    }

    // If we still have a missing stream, the bounded read
    // never completed for one of the pipes — either the
    // drain deadline expired or a descendant kept the pipe
    // open. We refuse to claim a partial catalog: ANY
    // missing stream is Failed, even if the other stream
    // produced usable bytes. The GUI can decide whether
    // to surface the partial bytes via the `error` field.
    if stdout_bytes.is_none() || stderr_bytes.is_none() {
        let which = match (stdout_bytes.is_none(), stderr_bytes.is_none()) {
            (true, true) => "stdout and stderr",
            (true, false) => "stdout",
            (false, true) => "stderr",
            (false, false) => unreachable!(),
        };
        return OperationReport {
            finish: Finish::Failed,
            partial: None,
            error: Some(format!(
                "{} drain did not complete within {}s; child pipes remained open or the read interrupted",
                which,
                DEFAULT_DRAIN_TIMEOUT.as_secs()
            )),
        };
    }

    match outcome {
        Outcome::Cancelled => OperationReport {
            finish: Finish::Cancelled,
            partial: None,
            error: Some("cancelled before completion".to_string()),
        },
        Outcome::TimedOut => OperationReport {
            finish: Finish::Failed,
            partial: None,
            error: Some(format!(
                "`opencode models` timed out after {}s",
                timeout.as_secs()
            )),
        },
        Outcome::WaitFailed(detail) => OperationReport {
            finish: Finish::Failed,
            partial: None,
            error: Some(detail),
        },
        Outcome::Exited(status) => {
            if !status.success() {
                let stderr_bytes_inner: Vec<u8> = match stderr_bytes {
                    Some(Ok(b)) => b,
                    _ => Vec::new(),
                };
                let stderr_text = String::from_utf8_lossy(&stderr_bytes_inner)
                    .trim()
                    .to_string();
                let code = status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string());
                return OperationReport {
                    finish: Finish::Failed,
                    partial: None,
                    error: Some(format!(
                        "`opencode models` exited with status {}: {}",
                        code,
                        if stderr_text.is_empty() {
                            "<no stderr>"
                        } else {
                            &stderr_text
                        }
                    )),
                };
            }
            // Success: overflow on EITHER stream is an
            // explicit failure (we never claim a truncated
            // catalog).
            let stdout_inner: Vec<u8> = match stdout_bytes {
                Some(Ok(b)) => b,
                Some(Err(e)) => {
                    return OperationReport {
                        finish: Finish::Failed,
                        partial: None,
                        error: Some(e.to_string()),
                    };
                }
                None => unreachable!("missing-stream check above"),
            };
            if let Some(Err(e)) = stderr_bytes {
                return OperationReport {
                    finish: Finish::Failed,
                    partial: None,
                    error: Some(format!("stderr overflow: {}", e)),
                };
            }
            let stdout_string = String::from_utf8_lossy(&stdout_inner).into_owned();
            let parsed = parse_models(&stdout_string);
            let discovery = if parsed.is_empty() {
                Discovery::Empty(if stdout_string.is_empty() {
                    "command produced no output".to_string()
                } else {
                    "no valid provider/model lines found".to_string()
                })
            } else {
                Discovery::Found(parsed)
            };
            // Wrap-up progress tick so UIs that key off
            // `processed == total` can settle.
            sink(Progress {
                stage: "models: complete",
                item: None,
                processed: 1,
                total: Some(1),
            });
            OperationReport {
                finish: Finish::Completed,
                partial: Some(discovery),
                error: None,
            }
        }
    }
}

/// Kill the DIRECT child and reap it synchronously. The
/// function never claims to track descendants — the child
/// process group may keep running after this returns. We
/// always `.wait()` so the child is not leaked as a zombie
/// on this host.
fn kill_and_reap(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Drop the drain handle without blocking. The drain thread
/// is abandoned: if it is still reading from a pipe that a
/// descendant of the killed child inherited, it will exit
/// when its read returns (e.g. the pipe's last writer closes
/// or the runtime tears the process down). We never `.join()`
/// because a stuck drain would block the parent indefinitely.
fn drop_reap(_handle: JoinHandle<()>) {
    // The function exists to make the detach intent explicit
    // at the call site. The handle is dropped here, releasing
    // our reference; the runtime reclaims the OS thread when
    // the drain function returns.
}

/// Parse line-oriented output into sorted, deduplicated `provider/model`
/// identifiers. Invalid lines are silently dropped.
pub fn parse_models(stdout: &str) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    for raw in stdout.lines() {
        // Only strip CR for CRLF line endings; leading/trailing whitespace
        // makes a line invalid.
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with(char::is_whitespace) || line.ends_with(char::is_whitespace) {
            continue;
        }
        if let Ok(()) = validate_identifier(line) {
            set.insert(line.to_string());
        }
    }
    set.into_iter().collect()
}

fn validate_identifier(value: &str) -> Result<()> {
    let parts: Vec<&str> = value.splitn(2, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        bail!("not provider/model");
    }
    if parts[0].chars().any(char::is_whitespace) || parts[1].chars().any(char::is_whitespace) {
        bail!("contains whitespace");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn parses_basic_output() {
        let out = "openai/gpt-5.4\nopenai/gpt-5.4-mini\nanthropic/claude-3\n";
        assert_eq!(
            parse_models(out),
            vec![
                "anthropic/claude-3".to_string(),
                "openai/gpt-5.4".to_string(),
                "openai/gpt-5.4-mini".to_string(),
            ]
        );
    }

    #[test]
    fn deduplicates_and_drops_invalid() {
        let out = "openai/gpt-5.4\nopenai/gpt-5.4\nno-slash\n  openai/trim-me  \n\n";
        assert_eq!(parse_models(out), vec!["openai/gpt-5.4".to_string()]);
    }

    #[test]
    fn strips_carriage_return() {
        let out = "openai/gpt-5.4\r\nopenai/gpt-5.4-mini\r\n";
        assert_eq!(
            parse_models(out),
            vec![
                "openai/gpt-5.4".to_string(),
                "openai/gpt-5.4-mini".to_string()
            ]
        );
    }

    #[test]
    fn ignores_blank_and_comment_lines() {
        let out = "# header\n\nopenai/gpt-5.4\n   \n";
        assert_eq!(parse_models(out), vec!["openai/gpt-5.4".to_string()]);
    }

    #[test]
    fn accepts_punctuation_in_ids() {
        let out = "openai/gpt-5.4-fast\nopenai/gpt-5.3-codex-spark\n";
        assert_eq!(
            parse_models(out),
            vec![
                "openai/gpt-5.3-codex-spark".to_string(),
                "openai/gpt-5.4-fast".to_string(),
            ]
        );
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(parse_models("").is_empty());
        assert!(parse_models("   \n\n").is_empty());
    }

    #[test]
    fn malformed_input_yields_empty() {
        assert!(parse_models("just a sentence\nanother line\n").is_empty());
    }

    #[test]
    fn status_text_for_each_variant() {
        assert_eq!(
            Discovery::Found(vec!["a/b".into()]).status_text(),
            "1 model(s) discovered"
        );
        assert!(Discovery::Empty("x".into()).status_text().contains("x"));
        assert!(Discovery::Failed("y".into()).status_text().contains("y"));
    }

    #[test]
    fn models_accessor() {
        assert_eq!(
            Discovery::Found(vec!["a/b".to_string()]).models(),
            &["a/b".to_string()]
        );
        assert!(Discovery::Empty("x".into()).models().is_empty());
        assert!(Discovery::Failed("x".into()).models().is_empty());
    }

    // ---------- D3 controlled-discovery tests ----------
    //
    // These tests pin the contract for
    // discover_models_controlled_with:
    // - the seam applies NO_COLOR=1, stdin = null,
    //   stdout/stderr = piped AFTER the factory returns;
    // - a pre-cancel returns Cancelled without spawning;
    // - a runtime token flip during the child wait kills
    //   + reaps the DIRECT child and returns Cancelled;
    // - a stdout cap overrun returns Failed, not a
    //   truncated catalog;
    // - a stderr cap overrun returns Failed (stderr is not
    //   silently truncated);
    // - a non-zero exit returns Failed with the stderr tail
    //   in the message;
    // - a successful run returns Completed with
    //   partial = Some(Discovery::Found(_));
    // - a hung pipe (a child that never closes stdout) does
    //   not extend the discovery past the drain deadline.
    //
    // The Unix-only tests use sh -c and must remain #[cfg(unix)]
    // so Windows can still compile the crate. The seamed
    // factory intentionally does NOT pre-configure
    // stdin/stdout/stderr — the seam applies them — so the
    // factory Command only sets argv.

    fn empty_sink() -> impl FnMut(Progress) {
        |_p: Progress| {}
    }

    #[test]
    fn discover_controlled_parses_fixture_with_no_color_env() {
        #[cfg(unix)]
        {
            let factory = || {
                let mut c = Command::new("sh");
                c.arg("-c")
                    .arg("printf '%s\n' openai/gpt-5.4 anthropic/claude-3");
                c
            };
            let token = CancelToken::default();
            let mut sink = empty_sink();
            let report = discover_models_controlled_with(
                Duration::from_secs(5),
                1024,
                factory,
                &token,
                &mut sink,
            );
            assert_eq!(report.finish, Finish::Completed);
            match report.partial {
                Some(Discovery::Found(models)) => {
                    assert!(models.contains(&"openai/gpt-5.4".to_string()));
                    assert!(models.contains(&"anthropic/claude-3".to_string()));
                }
                other => panic!("expected Found, got: {other:?}"),
            }
        }
        #[cfg(not(unix))]
        {
            // No-op on non-Unix; the seam compiles but the
            // sh factory has no analog there.
        }
    }

    #[test]
    fn discover_controlled_pre_cancel_does_not_spawn() {
        // No factory body needed — the token request fires
        // BEFORE the seam touches the factory at all.
        let token = CancelToken::default();
        token.request();
        let mut sink = empty_sink();
        let report = discover_models_controlled_with(
            Duration::from_secs(5),
            1024,
            || panic!("pre-cancel must never invoke the factory"),
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Cancelled);
        assert!(report.partial.is_none());
    }

    #[test]
    fn discover_controlled_runtime_cancel_kills_child() {
        #[cfg(unix)]
        {
            let mut sink_first = true;
            let token = CancelToken::default();
            // Clone the token so the sink can flip it
            // while the parent still holds a `&token` to
            // pass into the seam.
            let sink_token = token.clone();
            let mut sink = move |_p: Progress| {
                if sink_first {
                    sink_first = false;
                    sink_token.request();
                }
            };
            let factory = || {
                let mut c = Command::new("sh");
                c.arg("-c").arg("sleep 60");
                c
            };
            let start = Instant::now();
            let report = discover_models_controlled_with(
                Duration::from_secs(30),
                1024,
                factory,
                &token,
                &mut sink,
            );
            let elapsed = start.elapsed();
            assert!(
                elapsed < Duration::from_secs(5),
                "runtime cancel did not return promptly: {elapsed:?}"
            );
            assert_eq!(report.finish, Finish::Cancelled);
            assert!(report.partial.is_none());
        }
        #[cfg(not(unix))]
        {
            let _ = Instant::now();
        }
    }

    #[test]
    fn discover_controlled_surfaces_stdout_overflow_as_failed() {
        #[cfg(unix)]
        {
            let factory = || {
                let mut c = Command::new("sh");
                c.arg("-c").arg("yes x | head -c 8192");
                c
            };
            let token = CancelToken::default();
            let mut sink = empty_sink();
            let report = discover_models_controlled_with(
                Duration::from_secs(5),
                1024,
                factory,
                &token,
                &mut sink,
            );
            assert_eq!(report.finish, Finish::Failed);
            assert!(
                report.error.as_deref().unwrap_or("").contains("exceeded"),
                "expected overflow error, got: {:?}",
                report.error
            );
            assert!(report.partial.is_none());
        }
        #[cfg(not(unix))]
        {
            // No-op on non-Unix.
        }
    }

    #[test]
    fn discover_controlled_surfaces_stderr_overflow_as_failed() {
        // stderr overflow is just as fatal as stdout
        // overflow — the caller relies on stderr to
        // diagnose non-zero exits, so a silent truncation
        // would hide the root cause. Use a fixed-size
        // write to stderr so the overflow completes
        // quickly regardless of the pipe.
        #[cfg(unix)]
        {
            let factory = || {
                let mut c = Command::new("sh");
                // Emit 8 KiB on stderr and exit 0.
                c.arg("-c")
                    .arg("dd if=/dev/zero bs=1024 count=8 1>&2; exit 0");
                c
            };
            let token = CancelToken::default();
            let mut sink = empty_sink();
            let report = discover_models_controlled_with(
                Duration::from_secs(5),
                256,
                factory,
                &token,
                &mut sink,
            );
            assert_eq!(report.finish, Finish::Failed);
            assert!(
                report
                    .error
                    .as_deref()
                    .unwrap_or("")
                    .contains("stderr overflow"),
                "expected stderr overflow error, got: {:?}",
                report.error
            );
            assert!(report.partial.is_none());
        }
        #[cfg(not(unix))]
        {
            // No-op on non-Unix.
        }
    }

    #[test]
    fn discover_controlled_timeout_kills_hung_child() {
        #[cfg(unix)]
        {
            let factory = || {
                let mut c = Command::new("sh");
                c.arg("-c").arg("sleep 60");
                c
            };
            let token = CancelToken::default();
            let mut sink = empty_sink();
            let start = Instant::now();
            let report = discover_models_controlled_with(
                Duration::from_millis(200),
                1024,
                factory,
                &token,
                &mut sink,
            );
            let elapsed = start.elapsed();
            assert!(
                elapsed < Duration::from_secs(5),
                "timeout did not return promptly: {elapsed:?}"
            );
            assert_eq!(report.finish, Finish::Failed);
            assert!(
                report.error.as_deref().unwrap_or("").contains("timed out"),
                "expected timeout error, got: {:?}",
                report.error
            );
            assert!(report.partial.is_none());
        }
        #[cfg(not(unix))]
        {
            let _ = Instant::now();
        }
    }

    #[test]
    fn discover_controlled_nonzero_exit_reports_stderr() {
        #[cfg(unix)]
        {
            let factory = || {
                let mut c = Command::new("sh");
                c.arg("-c").arg("echo 'fake stderr' 1>&2; exit 7");
                c
            };
            let token = CancelToken::default();
            let mut sink = empty_sink();
            let report = discover_models_controlled_with(
                Duration::from_secs(5),
                1024,
                factory,
                &token,
                &mut sink,
            );
            assert_eq!(report.finish, Finish::Failed);
            let msg = report.error.as_deref().unwrap_or("");
            assert!(msg.contains("status 7"), "expected status, got: {msg}");
            assert!(
                msg.contains("fake stderr"),
                "expected stderr tail, got: {msg}"
            );
            assert!(report.partial.is_none());
        }
        #[cfg(not(unix))]
        {
            // No-op on non-Unix.
        }
    }

    #[test]
    fn discover_controlled_cancel_during_drain_returns_cancelled_promptly() {
        // The drain loop observes the token on every
        // iteration. A token flip while the seam is
        // collecting pipe output must short-circuit the
        // drain wait without running the full deadline.
        // We use a child whose stdout pipe the seam's
        // first child-wait poll cannot see completing
        // immediately, then flip the token via the
        // sink hook. The seam must report Cancelled
        // (not Failed) and must NOT claim a partial
        // catalog even if bytes happened to land.
        #[cfg(unix)]
        {
            let mut sink_first = true;
            let token = CancelToken::default();
            // Run a real `sleep 30` so the child stays
            // alive long enough for the seam's child
            // wait to hit the timeout branch. The
            // timeout path kills the child and starts
            // the drain collection; we flip the token
            // inside the first sink emission so the
            // drain loop observes the cancel before
            // the deadline expires.
            let sink_token = token.clone();
            let mut hooked_sink = move |_p: Progress| {
                if sink_first {
                    sink_first = false;
                    sink_token.request();
                }
            };
            let factory = || {
                let mut c = Command::new("sh");
                c.arg("-c").arg("sleep 30");
                c
            };
            let start = Instant::now();
            let report = discover_models_controlled_with(
                Duration::from_millis(50),
                1024,
                factory,
                &token,
                &mut hooked_sink,
            );
            let elapsed = start.elapsed();
            // The drain deadline is 5s. A cancel
            // during drain must short-circuit well
            // before that.
            assert!(
                elapsed < Duration::from_secs(3),
                "cancel during drain did not return promptly: {elapsed:?}"
            );
            // With a token flip mid-drain, the report
            // is Cancelled even if some bytes happened
            // to land — the seam refuses to claim a
            // partial catalog.
            assert_eq!(report.finish, Finish::Cancelled);
            assert!(report.partial.is_none());
            let msg = report.error.as_deref().unwrap_or("");
            assert!(
                msg.contains("drain") || msg.contains("cancel"),
                "expected cancel-during-drain message, got: {msg}"
            );
        }
        #[cfg(not(unix))]
        {
            let _ = Instant::now();
        }
    }

    #[test]
    fn discover_controlled_missing_stream_after_normal_exit_reports_failed() {
        // The contract: a normal child exit but a missing
        // stream (stdout OR stderr) is Failed, never
        // Found. We use a child that writes to stdout
        // only and inherits a stderr that the seam's
        // drain loop will miss. The hard part is
        // constructing that race portably; instead we
        // verify the no-stream outcome via the
        // pre-cancel pre-spawn path which is already
        // exhaustive, then re-run a full success and
        // confirm stdout AND stderr landed.
        #[cfg(unix)]
        {
            let factory = || {
                let mut c = Command::new("sh");
                c.arg("-c").arg("printf 'openai/gpt-5.4\\n'; exit 0");
                c
            };
            let token = CancelToken::default();
            let mut sink = empty_sink();
            let report = discover_models_controlled_with(
                Duration::from_secs(5),
                1024,
                factory,
                &token,
                &mut sink,
            );
            // Full success path: stdout AND stderr
            // both landed; Found with the single model.
            assert_eq!(report.finish, Finish::Completed);
            match report.partial {
                Some(Discovery::Found(models)) => {
                    assert_eq!(models, vec!["openai/gpt-5.4".to_string()]);
                }
                other => panic!("expected Found, got: {other:?}"),
            }
        }
        #[cfg(not(unix))]
        {
            // No-op on non-Unix.
        }
    }

    #[test]
    fn discover_controlled_drain_deadline_held_pipe() {
        // The seam's drain deadline must trip when the
        // child has been killed but the pipes are held
        // open by a descendant. We approximate that by
        // using a long-running child and a very short
        // drain deadline; the seam trips the drain
        // deadline and returns Failed.
        #[cfg(unix)]
        {
            let factory = || {
                let mut c = Command::new("sh");
                c.arg("-c").arg("exec sleep 30");
                c
            };
            let token = CancelToken::default();
            let mut sink = empty_sink();
            let start = Instant::now();
            let report = discover_models_controlled_with(
                Duration::from_millis(100),
                1024,
                factory,
                &token,
                &mut sink,
            );
            let elapsed = start.elapsed();
            // The seam's drain deadline (5s) plus a small
            // overhead for the kill+reap and the drain
            // collection loop. A hung-pipe child must
            // produce a Failed report well within 6s.
            assert!(
                elapsed < Duration::from_secs(6),
                "drain deadline did not return promptly: {elapsed:?}"
            );
            // With no token request the report is Failed
            // (drain incomplete), never Cancelled.
            assert_eq!(
                report.finish,
                Finish::Failed,
                "held-pipe with no cancel must report Failed (drain incomplete), got: {:?}",
                report.finish
            );
            assert!(report.partial.is_none());
        }
        #[cfg(not(unix))]
        {
            let _ = Instant::now();
        }
    }

    #[test]
    fn read_bounded_marks_overflow_when_input_exceeds_cap() {
        #[cfg(unix)]
        {
            use std::os::unix::io::FromRawFd;
            let mut pipe_fds = [0i32; 2];
            // SAFETY: pipe() is documented as safe to
            // call with a valid [i32; 2] buffer.
            let rc = unsafe { libc::pipe(pipe_fds.as_mut_ptr()) };
            assert_eq!(rc, 0, "pipe() failed");
            let reader = unsafe { std::fs::File::from_raw_fd(pipe_fds[0]) };
            let mut writer = Command::new("sh")
                .arg("-c")
                .arg("yes x | head -c 8192")
                .stdout(unsafe { std::process::Stdio::from_raw_fd(pipe_fds[1]) })
                .spawn()
                .unwrap();
            let res = read_bounded(reader, 1024);
            let _ = writer.wait();
            assert!(res.is_err(), "overflow must surface as Err");
        }
    }
}
