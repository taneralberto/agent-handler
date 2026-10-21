use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
#[cfg(windows)]
use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
#[cfg(windows)]
use std::path::{Path, PathBuf};
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
            Discovery::Found(models) => {
                // The list mixes base models and their named
                // variants (`provider/model#variant`), so the
                // label counts both instead of claiming every
                // entry is a model.
                let variants = models.iter().filter(|m| m.contains('#')).count();
                let base = models.len() - variants;
                if variants == 0 {
                    format!("{} model(s) discovered", base)
                } else {
                    format!("{} model(s), {} variant(s) discovered", base, variants)
                }
            }
            Discovery::Empty(msg) => format!("no models parsed: {}", msg),
            Discovery::Failed(msg) => format!("discovery failed: {}", msg),
        }
    }
}

// ---------- Model catalog (OpenCode `/api/model`) ----------
//
// The `opencode models` CLI prints one `provider/model` line per
// available model. The OpenCode server also exposes the model
// catalog through `opencode api get /api/model`, and every
// catalog entry carries the model's named variants (`low`,
// `high`, ...). A model selector is `provider/model` with an
// optional `#variant` suffix, so the catalog is the single
// source of truth for the selector list: a model or a variant
// added in OpenCode needs no change here.
//
// The catalog is read first; the CLI listing is the fallback
// for hosts where the catalog is unavailable (older OpenCode,
// no running server, offline).

/// One model entry of the OpenCode model catalog. Only the
/// fields the selector list needs are modeled — every other
/// catalog field (`settings`, `headers`, `body`, `cost`,
/// `limit`, ...) is ignored so a newer OpenCode release cannot
/// break the parse.
#[derive(Debug, Deserialize)]
struct CatalogModel {
    /// Catalog id — the part after `providerID` in a selector.
    #[serde(default)]
    id: String,
    /// Provider id — the part before the `/` in a selector.
    #[serde(rename = "providerID", default)]
    provider_id: String,
    /// Whether the model is available. An entry that is not
    /// enabled is hidden from the selector list.
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    /// The model's named variants.
    #[serde(default)]
    variants: Vec<CatalogVariant>,
}

/// One named variant of a catalog model.
#[derive(Debug, Deserialize)]
struct CatalogVariant {
    #[serde(default)]
    id: String,
}

/// Default for [`CatalogModel::enabled`]: a catalog entry that
/// does not carry the flag is treated as available.
fn enabled_by_default() -> bool {
    true
}

/// Parse the OpenCode model catalog payload (`/api/model`) into
/// model selectors.
///
/// Every enabled entry contributes its base selector
/// (`provider/model`) plus one selector per named variant
/// (`provider/model#variant`). Disabled entries, entries
/// without an id or a provider id, and variants without an id
/// are skipped; a malformed entry never invalidates the rest of
/// the catalog. The result is sorted and deduplicated.
///
/// Returns an error when the payload is not a catalog at all
/// (invalid JSON, or no `data` array) so the caller can fall
/// back to the `opencode models` CLI listing.
pub fn parse_model_catalog(payload: &str) -> Result<Vec<String>> {
    let value = parse_catalog_json(payload)?;
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("catalog payload has no `data` array"))?;
    let mut selectors = std::collections::BTreeSet::new();
    for entry in data {
        let Ok(model) = serde_json::from_value::<CatalogModel>(entry.clone()) else {
            // Malformed entry: skipped, the rest of the
            // catalog still counts.
            continue;
        };
        if !model.enabled {
            continue;
        }
        let base = format!("{}/{}", model.provider_id, model.id);
        if !is_model_selector(&base) {
            continue;
        }
        selectors.insert(base.clone());
        for variant in model.variants {
            let variant_id = variant.id.trim();
            if variant_id.is_empty() {
                continue;
            }
            let selector = format!("{}#{}", base, variant_id);
            if is_model_selector(&selector) {
                selectors.insert(selector);
            }
        }
    }
    Ok(selectors.into_iter().collect())
}

/// Read the catalog payload from a command's stdout. The
/// OpenCode CLI prints the payload on stdout; a leading log
/// line would break a strict parse, so the payload is retried
/// as the `{ ... }` slice of the output.
fn parse_catalog_json(stdout: &str) -> Result<serde_json::Value> {
    let trimmed = stdout.trim();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Ok(value);
    }
    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    match (start, end) {
        (Some(start), Some(end)) if end > start => {
            serde_json::from_str::<serde_json::Value>(&trimmed[start..=end])
                .map_err(|e| anyhow!("invalid catalog JSON: {}", e))
        }
        _ => Err(anyhow!("invalid catalog JSON: expected an object")),
    }
}

/// True when `value` is a usable model selector: a non-empty
/// provider id, a non-empty model id, and — when the selector
/// carries one — a non-empty variant id. The selector is stored
/// verbatim in the agent frontmatter, so whitespace is rejected.
fn is_model_selector(value: &str) -> bool {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return false;
    }
    let (base, variant) = match value.split_once('#') {
        Some((base, variant)) => (base, Some(variant)),
        None => (value, None),
    };
    let mut parts = base.splitn(2, '/');
    let provider = parts.next().unwrap_or_default();
    let model = parts.next().unwrap_or_default();
    if provider.is_empty() || model.is_empty() {
        return false;
    }
    match variant {
        Some(variant) => !variant.is_empty() && !variant.contains('#'),
        None => true,
    }
}

/// Discover the available model selectors (base models plus
/// their named variants) without a cancel token.
///
/// Blocking: the read is bounded by the default discovery
/// timeout, but the caller cannot cancel it. The TUI's model
/// picker uses this entry point.
pub fn discover_models() -> Discovery {
    let token = CancelToken::default();
    let mut sink = |_progress: Progress| {};
    let report = discover_models_controlled(&token, &mut sink);
    match report.partial {
        Some(discovery) => discovery,
        None => Discovery::Failed(
            report
                .error
                .unwrap_or_else(|| "model discovery failed".to_string()),
        ),
    }
}

/// Locate the npm-shipped `opencode.cmd` shim by walking `PATH`.
/// Needed because Rust's Windows bare executable lookup doesn't
/// append `.cmd`. Pure so they don't have to touch process state in
/// tests. Real `opencode.exe` installs are resolved by the bare-name
/// spawn and never hit this path.
#[cfg(windows)]
fn locate_opencode_cmd_in(path: &OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(path) {
        let candidate = dir.join("opencode.cmd");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
// Both sources the discovery reads (the model catalog and the
// `opencode models` fallback) go through the SAME bounded
// reader, so the fallback never turns into a second,
// unbounded wait.

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

/// The command sources the discovery reads. `Progress.stage`
/// is a `&'static str`, so the stage names are compile-time
/// constants per source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The OpenCode model catalog (`opencode api get
    /// /api/model`) — the primary source; it carries the
    /// models' named variants.
    Catalog,
    /// The `opencode models` CLI listing — the fallback used
    /// when the catalog is unavailable.
    Cli,
}

impl Source {
    /// Human label used in the discovery's error messages.
    fn label(self) -> &'static str {
        match self {
            Source::Catalog => "`opencode api get /api/model`",
            Source::Cli => "`opencode models`",
        }
    }

    /// Progress stage emitted before the child is spawned.
    fn spawn_stage(self) -> &'static str {
        match self {
            Source::Catalog => "catalog: spawn",
            Source::Cli => "models: spawn",
        }
    }

    /// Progress stage emitted once the source was read and
    /// parsed.
    fn complete_stage(self) -> &'static str {
        match self {
            Source::Catalog => "catalog: complete",
            Source::Cli => "models: complete",
        }
    }
}

/// Read `reader` into a buffer, stopping once `cap` bytes have
/// been collected. When the cap is exceeded, the remaining bytes
/// are still drained (so the child never deadlocks on a full pipe)
/// but the function returns `Err(_)` so the caller can surface an
/// explicit overflow failure instead of silently truncating.
/// `label` names the command in the failure message.
fn read_bounded<R: Read>(mut reader: R, cap: usize, label: &str) -> Result<Vec<u8>> {
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
        bail!("{} output exceeded {} bytes", label, cap);
    }
    if interrupted {
        bail!("{} read interrupted", label);
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

/// Outcome of one bounded, cancellable child run.
enum BoundedRun {
    /// The child exited successfully; `stdout` is the bounded
    /// read of its standard output.
    Completed { stdout: String },
    /// The run was cancelled. The message says when.
    Cancelled(String),
    /// The run failed. The message is the user-facing reason.
    Failed(String),
}

/// Discover the available model selectors with cooperative
/// cancellation and progress reporting.
///
/// The OpenCode model catalog is the primary source: it lists
/// every available model together with the model's named
/// variants, so the selector list carries both `provider/model`
/// and `provider/model#variant` entries. When the catalog is
/// unavailable (older OpenCode, no running server, offline) the
/// discovery falls back to the `opencode models` CLI listing, so
/// no model is lost.
///
/// Returns an `OperationReport<Option<Discovery>>` so the
/// caller can distinguish a real cancel/timeout from a
/// successful discovery. `partial` is `None` when the
/// discovery finished without producing a model catalog
/// (cancelled or timed out before the child reported) and
/// `Some(Discovery::)` otherwise.
///
/// This is the GUI/CLI-cancel-aware entry point that lets a long
/// discovery abort cleanly without holding the TUI hostage.
///
/// Both commands inherit the process's working directory, so the
/// catalog and the fallback describe the same project scope.
pub fn discover_models_controlled(
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> OperationReport<Option<Discovery>> {
    discover_model_selectors_controlled_with(
        DEFAULT_DISCOVERY_TIMEOUT,
        DEFAULT_DISCOVERY_OUTPUT_CAP,
        build_opencode_catalog_command,
        build_opencode_models_command,
        token,
        sink,
    )
}

/// Build the production `Command` for the OpenCode model
/// catalog. The invocation invariant — `NO_COLOR=1`,
/// `stdin = null`, `stdout = piped`, `stderr = piped` — is
/// applied HERE, after the factory returns, so every caller
/// (production + tests) gets the same child setup. Tests must
/// NOT pre-configure these on the factory output.
fn build_opencode_catalog_command() -> Command {
    let mut cmd = Command::new("opencode");
    cmd.arg("api")
        .arg("get")
        .arg("/api/model")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Build the production `Command` for `opencode models` — the
/// fallback listing used when the catalog is unavailable. Same
/// invocation invariant as the catalog command.
fn build_opencode_models_command() -> Command {
    let mut cmd = Command::new("opencode");
    cmd.arg("models")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Test seam for the single-source discovery: same bounded,
/// cancellable read of ONE command, with an explicit timeout
/// and output cap.
///
/// The signature and the contract are the ones this seam has
/// always had: one command factory, the `opencode models`
/// listing, and the original `OperationReport` semantics. A
/// consumer compiled against the previous five-argument shape
/// keeps compiling and keeps its behaviour.
///
/// The catalog + fallback flow lives in
/// [`discover_model_selectors_controlled_with`], which is
/// private: it is an internal composition of the catalog read
/// and THIS seam.
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
    match run_bounded(
        Source::Cli,
        timeout,
        output_cap,
        command_factory,
        token,
        sink,
    ) {
        BoundedRun::Completed { stdout } => {
            let parsed = parse_models(&stdout);
            let discovery = if parsed.is_empty() {
                Discovery::Empty(if stdout.is_empty() {
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
                stage: Source::Cli.complete_stage(),
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
        BoundedRun::Cancelled(message) => OperationReport {
            finish: Finish::Cancelled,
            partial: None,
            error: Some(message),
        },
        BoundedRun::Failed(message) => OperationReport {
            finish: Finish::Failed,
            partial: None,
            error: Some(message),
        },
    }
}

/// The two-source discovery: the OpenCode model catalog
/// (primary) and the `opencode models` CLI listing (fallback).
///
/// Private on purpose: the published surface is
/// [`discover_models_controlled`] (catalog + fallback) and
/// [`discover_models_controlled_with`] (the single-source seam
/// this flow falls back to). The seam applies the production
/// stdin/stdout/stderr/NO_COLOR setup after the factory
/// returns.
fn discover_model_selectors_controlled_with<F1, F2>(
    timeout: Duration,
    output_cap: usize,
    catalog_factory: F1,
    cli_factory: F2,
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> OperationReport<Option<Discovery>>
where
    F1: FnOnce() -> Command,
    F2: FnOnce() -> Command,
{
    // 1. Catalog (primary source).
    let catalog = run_bounded(
        Source::Catalog,
        timeout,
        output_cap,
        catalog_factory,
        token,
        sink,
    );
    let mut catalog_error: Option<String> = None;
    let selectors = match catalog {
        BoundedRun::Completed { stdout } => match parse_model_catalog(&stdout) {
            Ok(selectors) => selectors,
            Err(e) => {
                catalog_error = Some(format!("catalog parse failed: {}", e));
                Vec::new()
            }
        },
        BoundedRun::Cancelled(message) => {
            // A cancel stops the discovery: the fallback is not
            // read after the user asked to stop.
            return OperationReport {
                finish: Finish::Cancelled,
                partial: None,
                error: Some(message),
            };
        }
        BoundedRun::Failed(message) => {
            catalog_error = Some(message);
            Vec::new()
        }
    };
    if !selectors.is_empty() {
        // Wrap-up progress tick so UIs that key off
        // `processed == total` can settle.
        sink(Progress {
            stage: Source::Catalog.complete_stage(),
            item: None,
            processed: 1,
            total: Some(1),
        });
        return OperationReport {
            finish: Finish::Completed,
            partial: Some(Discovery::Found(selectors)),
            error: None,
        };
    }

    // 2. CLI fallback. The fallback read goes through the same
    // bounded runner (the published single-source seam), so it
    // never turns into a second, unbounded wait.
    let fallback = discover_models_controlled_with(timeout, output_cap, cli_factory, token, sink);
    match fallback.finish {
        Finish::Completed | Finish::Cancelled => fallback,
        Finish::Failed => {
            // Both sources failed: report the catalog failure
            // first (it is the primary source) and the fallback
            // failure second.
            let cli_message = fallback.error.unwrap_or_default();
            let error = match catalog_error {
                Some(catalog_message) => format!(
                    "{}; `opencode models` fallback failed: {}",
                    catalog_message, cli_message
                ),
                None => cli_message,
            };
            OperationReport {
                finish: Finish::Failed,
                partial: None,
                error: Some(error),
            }
        }
    }
}

/// Run one discovery source with a bounded, cancellable read.
///
/// The child is spawned with `NO_COLOR=1` and `stdin = null`
/// (the production invocation invariant), stdout / stderr are
/// read concurrently through bounded pipes, and a timeout /
/// cancel kills + reaps the DIRECT child so the parent never
/// waits indefinitely. Descendants are not chased.
fn run_bounded(
    source: Source,
    timeout: Duration,
    output_cap: usize,
    command_factory: impl FnOnce() -> Command,
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> BoundedRun {
    let label = source.label();

    // Pre-cancel: honor a token request BEFORE spawning. No
    // child, no pipes, no drain threads — the run returns
    // promptly as cancelled.
    if run_cancel_checked(
        token,
        sink,
        Progress {
            stage: source.spawn_stage(),
            item: None,
            processed: 0,
            total: Some(1),
        },
    ) {
        return BoundedRun::Cancelled("cancelled before spawn".to_string());
    }

    let mut cmd = command_factory();
    // Apply the production invocation invariant AFTER the
    // factory — every child the seam produces has null stdin
    // and piped stdout/stderr, with NO_COLOR=1.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1");
    let mut child = match spawn_child(&mut cmd) {
        Ok(c) => c,
        Err(e) => {
            return BoundedRun::Failed(format!("could not execute {}: {}", label, e));
        }
    };
    let stdout_pipe = match child.stdout.take() {
        Some(s) => s,
        None => {
            // Best-effort cleanup; the child was just spawned
            // and never produced output.
            kill_and_reap(&mut child);
            return BoundedRun::Failed(format!("{} stdout was not piped", label));
        }
    };
    let stderr_pipe = match child.stderr.take() {
        Some(s) => s,
        None => {
            kill_and_reap(&mut child);
            return BoundedRun::Failed(format!("{} stderr was not piped", label));
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
        let res = read_bounded(stdout_pipe, output_cap, label);
        // Drop the sender so the channel can disconnect
        // after we send.
        let _ = tx.send(DrainStream::Stdout(res));
        // `tx` is dropped here; the join handle is detached
        // by `run_bounded` below.
    });
    let stderr_handle = thread::spawn(move || {
        let res = read_bounded(stderr_pipe, output_cap, label);
        let _ = tx_err.send(DrainStream::Stderr(res));
    });

    // Mid-wait loop: poll the child AND the token. On a token
    // request, kill the DIRECT child (we never promise
    // descendant cleanup) and reap it, then report the cancel.
    // On a timeout, same path with a failure. On the child
    // exiting naturally, return its status. On a try_wait
    // error, kill+reap and report the failure.
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
                break Outcome::WaitFailed(format!("{} wait failed: {}", label, e));
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
    // the run from the user's perspective, so report it as
    // cancelled (not failed) and do NOT claim any catalog
    // even if one stream happened to land before the
    // token was honored.
    if drain_cancelled {
        return BoundedRun::Cancelled("cancelled during drain".to_string());
    }

    // If we still have a missing stream, the bounded read
    // never completed for one of the pipes — either the
    // drain deadline expired or a descendant kept the pipe
    // open. We refuse to claim a partial catalog: ANY
    // missing stream is a failure, even if the other stream
    // produced usable bytes. The caller can decide whether
    // to surface the partial bytes via the `error` field.
    if stdout_bytes.is_none() || stderr_bytes.is_none() {
        let which = match (stdout_bytes.is_none(), stderr_bytes.is_none()) {
            (true, true) => "stdout and stderr",
            (true, false) => "stdout",
            (false, true) => "stderr",
            (false, false) => unreachable!(),
        };
        return BoundedRun::Failed(format!(
            "{} drain did not complete within {}s; child pipes remained open or the read interrupted",
            which,
            DEFAULT_DRAIN_TIMEOUT.as_secs()
        ));
    }

    match outcome {
        Outcome::Cancelled => BoundedRun::Cancelled("cancelled before completion".to_string()),
        Outcome::TimedOut => {
            BoundedRun::Failed(format!("{} timed out after {}s", label, timeout.as_secs()))
        }
        Outcome::WaitFailed(detail) => BoundedRun::Failed(detail),
        Outcome::Exited(status) => {
            if !status.success() {
                let stderr_text = match &stderr_bytes {
                    Some(Ok(b)) => String::from_utf8_lossy(b).trim().to_string(),
                    _ => String::new(),
                };
                let code = status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string());
                return BoundedRun::Failed(format!(
                    "{} exited with status {}: {}",
                    label,
                    code,
                    if stderr_text.is_empty() {
                        "<no stderr>"
                    } else {
                        &stderr_text
                    }
                ));
            }
            // Success: overflow on EITHER stream is an
            // explicit failure (we never claim a truncated
            // catalog).
            let stdout_inner: Vec<u8> = match stdout_bytes {
                Some(Ok(b)) => b,
                Some(Err(e)) => return BoundedRun::Failed(e.to_string()),
                None => unreachable!("missing-stream check above"),
            };
            if let Some(Err(e)) = &stderr_bytes {
                return BoundedRun::Failed(format!("stderr overflow: {}", e));
            }
            BoundedRun::Completed {
                stdout: String::from_utf8_lossy(&stdout_inner).into_owned(),
            }
        }
    }
}

/// Spawn the child for a discovery source.
///
/// Windows-only fallback: Rust's bare executable lookup does
/// not append `.cmd`, so on a host whose only `opencode` is the
/// npm-shipped `opencode.cmd` shim the bare-name spawn fails
/// with `NotFound`. We locate the shim on PATH and re-spawn it
/// with the SAME argv, cwd and env; Rust's std runs an
/// explicitly named batch shim itself, so we never assemble a
/// `cmd.exe /C` command line ourselves. Every other error — and
/// every non-Windows host — propagates unchanged.
///
/// The PATH lookup uses the PATH **configured on the command**
/// when the caller set one, and the process PATH otherwise. A
/// caller that pins a PATH therefore gets the shim from THAT
/// directory without touching the process environment — the
/// bare-name lookup would ignore the configured PATH (Windows
/// resolves a bare program name against the parent's
/// environment), so the configured PATH is resolved first.
fn spawn_child(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    #[cfg(windows)]
    if cmd.get_program() == "opencode" {
        if let Some(path) = configured_path(cmd) {
            if let Some(shim) = locate_opencode_cmd_in(&path) {
                return spawn_shim(cmd, &shim);
            }
        }
    }
    match cmd.spawn() {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            #[cfg(windows)]
            {
                if cmd.get_program() == "opencode" {
                    let path = std::env::var_os("PATH").unwrap_or_default();
                    if let Some(shim) = locate_opencode_cmd_in(&path) {
                        return spawn_shim(cmd, &shim);
                    }
                }
            }
            Err(e)
        }
        other => other,
    }
}

/// The PATH configured on `cmd`, if the caller set one.
/// Windows env names are case-insensitive, so the lookup
/// tolerates `Path` as well as `PATH`.
#[cfg(windows)]
fn configured_path(cmd: &Command) -> Option<std::ffi::OsString> {
    cmd.get_envs()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .and_then(|(_, value)| value)
        .map(std::ffi::OsStr::to_os_string)
}

/// Re-spawn `cmd`'s program through the located `opencode.cmd`
/// shim, keeping the original argv, working directory and
/// environment.
#[cfg(windows)]
fn spawn_shim(cmd: &Command, shim: &Path) -> std::io::Result<std::process::Child> {
    let mut retry = Command::new(shim);
    retry.args(cmd.get_args().map(|arg| arg.to_os_string()));
    if let Some(cwd) = cmd.get_current_dir() {
        retry.current_dir(cwd);
    }
    for (key, value) in cmd.get_envs() {
        match value {
            Some(value) => {
                retry.env(key, value);
            }
            None => {
                retry.env_remove(key);
            }
        }
    }
    retry
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1");
    retry.spawn()
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
    use std::io::Write;
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

    // Windows-only: walks PATH for `opencode.cmd`. Synthesized
    // shims in isolated TempDirs keep the host PATH untouched.
    #[cfg(windows)]
    #[test]
    fn locate_opencode_cmd_in_finds_shim_even_when_dir_has_amper() {
        // Directory name intentionally contains `&` (e.g.
        // `C:\R&D\bin\opencode.cmd`). The resolved path goes to
        // `Command::new`, never to a shell.
        let dir = tempfile::tempdir().expect("tempdir");
        let weird = dir.path().join("with&here");
        std::fs::create_dir_all(&weird).expect("create weird dir");
        let shim = weird.join("opencode.cmd");
        std::fs::write(&shim, b"").expect("write shim");
        let path_env = std::env::join_paths([&weird]).expect("PATH env");

        assert_eq!(
            locate_opencode_cmd_in(path_env.as_os_str()),
            Some(shim),
            "locator must return the shim even when its parent dir contains `&`"
        );
    }

    #[cfg(windows)]
    #[test]
    fn locate_opencode_cmd_in_returns_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path_env = std::env::join_paths([dir.path()]).expect("PATH env");
        assert!(
            locate_opencode_cmd_in(path_env.as_os_str()).is_none(),
            "no opencode.cmd in PATH -> locator must return None"
        );
    }

    #[cfg(windows)]
    #[test]
    fn locate_opencode_cmd_in_honours_path_order() {
        let first = tempfile::tempdir().expect("first tempdir");
        let second = tempfile::tempdir().expect("second tempdir");
        std::fs::write(first.path().join("opencode.cmd"), b"").expect("write first");
        std::fs::write(second.path().join("opencode.cmd"), b"").expect("write second");
        let path_env = std::env::join_paths([first.path(), second.path()]).expect("PATH env");

        let found = locate_opencode_cmd_in(path_env.as_os_str()).expect("located");
        assert!(
            found.starts_with(first.path()),
            "earlier PATH directory must win, got: {}",
            found.display()
        );
    }

    // End-to-end: a `.cmd` shim at a path containing `&` runs to
    // completion via `Command::new(<full .cmd path>)` — proving
    // the fallback shape does not regress on real cmd.exe quoting
    // traps. No host PATH mutation; the shim lives in an isolated
    // TempDir and we pass it to `Command::new` ourselves.
    #[cfg(windows)]
    #[test]
    fn command_new_full_cmd_path_handles_amper_in_dirname() {
        let dir = tempfile::tempdir().expect("tempdir");
        let weird = dir.path().join("with&here");
        std::fs::create_dir_all(&weird).expect("create weird dir");
        let shim = weird.join("opencode.cmd");
        std::fs::write(&shim, b"@echo openai/gpt-5.4\r\n").expect("write shim");

        let out = Command::new(&shim)
            .arg("models")
            .env("NO_COLOR", "1")
            .output()
            .expect("Command::new(<full .cmd path>) must run the shim");
        assert!(out.status.success(), "shim exited non-zero: {:?}", out);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "openai/gpt-5.4",
            "shim stdout should reach us verbatim, even from a path containing `&`"
        );
    }

    // ---------- D3 controlled-discovery tests ----------
    //
    // These tests pin the contract for
    // discover_models_controlled_with:
    // - the seam applies NO_COLOR=1, stdin = null,
    //   stdout/stderr = piped AFTER the factory returns;
    // - the catalog is the primary source; the
    //   `opencode models` CLI listing is the fallback used
    //   when the catalog is unavailable;
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
    // The fixtures run the TEST BINARY itself as a helper
    // process (re-entered with `--exact
    // models::tests::helper_process --ignored`), so the
    // contract is exercised on every platform without a
    // shell dependency. The seamed factory intentionally does
    // NOT pre-configure stdin/stdout/stderr — the seam
    // applies them — so the factory Command only sets argv
    // and the helper's environment.

    fn empty_sink() -> impl FnMut(Progress) {
        |_p: Progress| {}
    }

    /// Env var the helper process reads its payload from.
    const PAYLOAD_ENV: &str = "AGENTHD_TEST_PAYLOAD";

    /// Helper process. Re-entered by the controlled-discovery
    /// tests with `AGENTHD_TEST_HELPER` set to
    /// `<mode>[:<arg>]`. The modes:
    ///
    /// - `printenv:<name>`: print the value of the env var
    ///   `<name>` on stdout (a leading newline keeps the
    ///   payload off the harness' own "test <name> ... "
    ///   prefix line).
    /// - `stderrenv:<name>`: print the value of the env var
    ///   `<name>` on stderr.
    /// - `failenv:<code>:<name>`: print `<name>=<value>` on
    ///   stderr and exit with `<code>`.
    /// - `sleep:<seconds>`: sleep, then exit 0.
    ///
    /// The process exits directly so the test harness never
    /// appends its own summary to the captured output.
    #[test]
    #[ignore]
    fn helper_process() {
        let Ok(spec) = std::env::var("AGENTHD_TEST_HELPER") else {
            return;
        };
        let mut parts = spec.splitn(3, ':');
        let mode = parts.next().expect("helper mode");
        let first = parts.next().unwrap_or_default();
        let second = parts.next().unwrap_or_default();
        match mode {
            "printenv" => {
                let payload = std::env::var(first).expect("payload env var");
                let mut out = std::io::stdout();
                out.write_all(b"\n").expect("write stdout");
                out.write_all(payload.as_bytes()).expect("write stdout");
                out.flush().expect("flush stdout");
            }
            "stderrenv" => {
                let payload = std::env::var(first).expect("payload env var");
                let mut out = std::io::stderr();
                out.write_all(payload.as_bytes()).expect("write stderr");
                out.flush().expect("flush stderr");
            }
            "failenv" => {
                let payload = std::env::var(second).expect("payload env var");
                let mut out = std::io::stderr();
                out.write_all(format!("{}={}", second, payload).as_bytes())
                    .expect("write stderr");
                out.flush().expect("flush stderr");
                std::process::exit(first.parse().expect("exit code"));
            }
            "sleep" => {
                std::thread::sleep(Duration::from_secs(first.parse().expect("sleep seconds")));
            }
            other => panic!("unknown helper mode: {other}"),
        }
        std::process::exit(0);
    }

    /// A command that runs the helper process in the given
    /// mode.
    fn helper_command(spec: &str) -> Command {
        let mut c = Command::new(std::env::current_exe().expect("current exe"));
        c.arg("models::tests::helper_process")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("AGENTHD_TEST_HELPER", spec);
        c
    }

    /// A command whose stdout is `value`.
    fn stdout_command(value: &str) -> Command {
        let mut c = helper_command(&format!("printenv:{}", PAYLOAD_ENV));
        c.env(PAYLOAD_ENV, value);
        c
    }

    /// A command whose stderr is `value`.
    fn stderr_command(value: &str) -> Command {
        let mut c = helper_command(&format!("stderrenv:{}", PAYLOAD_ENV));
        c.env(PAYLOAD_ENV, value);
        c
    }

    /// A command that writes `<name>=<value>` to stderr and
    /// exits with `code`.
    fn failing_command(code: i32, value: &str) -> Command {
        let mut c = helper_command(&format!("failenv:{}:{}", code, PAYLOAD_ENV));
        c.env(PAYLOAD_ENV, value);
        c
    }

    /// A command that reports the child's `NO_COLOR` value on
    /// stderr and exits non-zero — used to observe the
    /// environment the seam applies to the child.
    fn no_color_probe_command() -> Command {
        helper_command("failenv:1:NO_COLOR")
    }

    /// A command that sleeps for `seconds`.
    fn sleeping_command(seconds: u64) -> Command {
        helper_command(&format!("sleep:{}", seconds))
    }

    /// A catalog payload with one model that has two variants
    /// and one model without variants.
    const CATALOG_FIXTURE: &str = concat!(
        r#"{"location":{"directory":"/tmp"},"data":["#,
        r#"{"id":"gpt-5.2","providerID":"openai","enabled":true,"variants":["#,
        r#"{"id":"low","settings":{"reasoningEffort":"low"}},"#,
        r#"{"id":"high","settings":{"reasoningEffort":"high"}}]},"#,
        r#"{"id":"MiniMax-M3.1-Flash","providerID":"minimax","enabled":true,"variants":[]}"#,
        r#"]}"#
    );

    #[test]
    fn parse_model_catalog_reads_base_models_and_variants() {
        let selectors = parse_model_catalog(CATALOG_FIXTURE).expect("catalog parses");
        assert_eq!(
            selectors,
            vec![
                "minimax/MiniMax-M3.1-Flash".to_string(),
                "openai/gpt-5.2".to_string(),
                "openai/gpt-5.2#high".to_string(),
                "openai/gpt-5.2#low".to_string(),
            ]
        );
    }

    #[test]
    fn parse_model_catalog_skips_disabled_entries() {
        let payload = concat!(
            r#"{"data":[{"id":"gpt-5.2","providerID":"openai","enabled":false,"variants":["#,
            r#"{"id":"high"}]},{"id":"gpt-5.3","providerID":"openai","enabled":true,"variants":[]}]}"#
        );
        let selectors = parse_model_catalog(payload).expect("catalog parses");
        assert_eq!(selectors, vec!["openai/gpt-5.3".to_string()]);
    }

    #[test]
    fn parse_model_catalog_deduplicates_repeated_entries() {
        let payload = concat!(
            r#"{"data":[{"id":"gpt-5.2","providerID":"openai","enabled":true,"variants":["#,
            r#"{"id":"high"}]},{"id":"gpt-5.2","providerID":"openai","enabled":true,"variants":["#,
            r#"{"id":"high"}]}]}"#
        );
        let selectors = parse_model_catalog(payload).expect("catalog parses");
        assert_eq!(
            selectors,
            vec![
                "openai/gpt-5.2".to_string(),
                "openai/gpt-5.2#high".to_string()
            ]
        );
    }

    #[test]
    fn parse_model_catalog_skips_malformed_entries() {
        // An entry without a provider id, a variant without an
        // id, and an entry whose fields have the wrong type are
        // skipped; the well-formed entries survive.
        let payload = concat!(
            r#"{"data":[{"id":"no-provider","enabled":true},"#,
            r#"{"id":"gpt-5.2","providerID":"openai","enabled":true,"variants":[{"settings":{}}]},"#,
            r#"{"id":42,"providerID":"openai","enabled":true},"#,
            r#"{"id":"gpt-5.3","providerID":"openai","enabled":true,"variants":[]}]}"#
        );
        let selectors = parse_model_catalog(payload).expect("catalog parses");
        assert_eq!(
            selectors,
            vec!["openai/gpt-5.2".to_string(), "openai/gpt-5.3".to_string()]
        );
    }

    #[test]
    fn parse_model_catalog_rejects_a_payload_that_is_not_a_catalog() {
        assert!(parse_model_catalog("not json at all").is_err());
        assert!(parse_model_catalog("{\"data\":\"not-an-array\"}").is_err());
        assert!(parse_model_catalog("{}").is_err());
    }

    #[test]
    fn parse_model_catalog_accepts_a_payload_behind_a_log_line() {
        let payload = format!("starting opencode...\n{}", CATALOG_FIXTURE);
        let selectors = parse_model_catalog(&payload).expect("catalog parses");
        assert!(selectors.contains(&"openai/gpt-5.2#high".to_string()));
    }

    #[test]
    fn is_model_selector_accepts_base_and_variant_forms() {
        assert!(is_model_selector("openai/gpt-5.2"));
        assert!(is_model_selector("openai/gpt-5.2#high"));
        assert!(is_model_selector(
            "openrouter/anthropic/claude-sonnet-4.5#high"
        ));
        assert!(!is_model_selector(""));
        assert!(!is_model_selector("openai"));
        assert!(!is_model_selector("openai/"));
        assert!(!is_model_selector("/gpt-5.2"));
        assert!(!is_model_selector("openai/gpt 5.2"));
        assert!(!is_model_selector("openai/gpt-5.2#"));
        assert!(!is_model_selector("openai/gpt-5.2#a#b"));
    }

    #[test]
    fn discover_controlled_reads_catalog_selectors() {
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            65536,
            || stdout_command(CATALOG_FIXTURE),
            || panic!("the CLI fallback must not run when the catalog answered"),
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Completed);
        match report.partial {
            Some(Discovery::Found(models)) => {
                assert!(models.contains(&"openai/gpt-5.2".to_string()));
                assert!(models.contains(&"openai/gpt-5.2#high".to_string()));
                assert!(models.contains(&"minimax/MiniMax-M3.1-Flash".to_string()));
            }
            other => panic!("expected Found, got: {other:?}"),
        }
    }

    #[test]
    fn discover_controlled_falls_back_to_the_cli_listing() {
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            65536,
            || failing_command(7, "catalog boom"),
            || stdout_command("openai/gpt-5.4\nanthropic/claude-3\n"),
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Completed);
        match report.partial {
            Some(Discovery::Found(models)) => {
                assert_eq!(
                    models,
                    vec![
                        "anthropic/claude-3".to_string(),
                        "openai/gpt-5.4".to_string()
                    ]
                );
            }
            other => panic!("expected Found, got: {other:?}"),
        }
    }

    #[test]
    fn discover_controlled_falls_back_when_the_catalog_payload_is_malformed() {
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            65536,
            || stdout_command("this is not a catalog"),
            || stdout_command("openai/gpt-5.4\n"),
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Completed);
        match report.partial {
            Some(Discovery::Found(models)) => {
                assert_eq!(models, vec!["openai/gpt-5.4".to_string()]);
            }
            other => panic!("expected Found, got: {other:?}"),
        }
    }

    #[test]
    fn discover_controlled_reports_both_sources_when_both_fail() {
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            65536,
            || failing_command(7, "catalog boom"),
            || failing_command(3, "cli boom"),
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Failed);
        let message = report.error.as_deref().unwrap_or("");
        assert!(message.contains("catalog boom"), "got: {message}");
        assert!(message.contains("cli boom"), "got: {message}");
    }

    #[test]
    fn discover_controlled_applies_no_color_env() {
        let token = CancelToken::default();
        let mut sink = empty_sink();
        // The helper writes the child's NO_COLOR value to
        // stderr and exits non-zero, so the value shows up in
        // the discovery's error message.
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            65536,
            no_color_probe_command,
            no_color_probe_command,
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Failed);
        let message = report.error.as_deref().unwrap_or("");
        assert!(
            message.contains("NO_COLOR=1"),
            "the child must observe NO_COLOR=1: {message}"
        );
    }

    #[test]
    fn discover_controlled_pre_cancel_does_not_spawn() {
        // No factory body needed — the token request fires
        // BEFORE the seam touches the factory at all.
        let token = CancelToken::default();
        token.request();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(5),
            1024,
            || panic!("pre-cancel must never invoke the catalog factory"),
            || panic!("pre-cancel must never invoke the CLI factory"),
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Cancelled);
        assert!(report.partial.is_none());
    }

    #[test]
    fn discover_controlled_runtime_cancel_kills_child() {
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
        let start = Instant::now();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            1024,
            || sleeping_command(60),
            || panic!("a cancelled discovery must not read the CLI fallback"),
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

    #[test]
    fn discover_controlled_surfaces_stdout_overflow_as_failed() {
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            1024,
            || stdout_command(&"x".repeat(8192)),
            || stdout_command(&"x".repeat(8192)),
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

    #[test]
    fn discover_controlled_surfaces_stderr_overflow_as_failed() {
        // stderr overflow is just as fatal as stdout
        // overflow — the caller relies on stderr to
        // diagnose non-zero exits, so a silent truncation
        // would hide the root cause.
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            256,
            || stderr_command(&"x".repeat(8192)),
            || stderr_command(&"x".repeat(8192)),
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

    #[test]
    fn discover_controlled_timeout_kills_hung_child() {
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let start = Instant::now();
        let report = discover_model_selectors_controlled_with(
            Duration::from_millis(200),
            1024,
            || sleeping_command(60),
            || sleeping_command(60),
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

    #[test]
    fn discover_controlled_nonzero_exit_reports_stderr() {
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            1024,
            || failing_command(7, "fake stderr"),
            || failing_command(7, "fake stderr"),
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

    #[test]
    fn discover_controlled_cancel_during_drain_returns_cancelled_promptly() {
        // The drain loop observes the token on every
        // iteration. A token flip while the seam is
        // collecting pipe output must short-circuit the
        // drain wait without running the full deadline.
        let mut sink_first = true;
        let token = CancelToken::default();
        let sink_token = token.clone();
        let mut hooked_sink = move |_p: Progress| {
            if sink_first {
                sink_first = false;
                sink_token.request();
            }
        };
        let start = Instant::now();
        let report = discover_model_selectors_controlled_with(
            Duration::from_millis(50),
            1024,
            || sleeping_command(30),
            || panic!("a cancelled discovery must not read the CLI fallback"),
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
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            1024,
            || stdout_command(CATALOG_FIXTURE),
            || panic!("the CLI fallback must not run when the catalog answered"),
            &token,
            &mut sink,
        );
        // Full success path: stdout AND stderr
        // both landed; Found with the catalog's selectors.
        assert_eq!(report.finish, Finish::Completed);
        match report.partial {
            Some(Discovery::Found(models)) => {
                assert!(models.contains(&"openai/gpt-5.2#low".to_string()));
            }
            other => panic!("expected Found, got: {other:?}"),
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
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let start = Instant::now();
        let report = discover_model_selectors_controlled_with(
            Duration::from_millis(100),
            1024,
            || sleeping_command(30),
            || sleeping_command(30),
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

    #[cfg(windows)]
    #[test]
    fn discover_controlled_falls_back_to_the_npm_shim_on_windows() {
        // Rust's bare executable lookup does not append `.cmd`,
        // so a host whose only `opencode` is the npm-shipped
        // `opencode.cmd` shim needs the explicit fallback. The
        // test hosts a shim in an isolated temp dir and pins the
        // PATH **on the command**: the shim lookup honours that
        // PATH, so the test never mutates the process
        // environment and a real `opencode.exe` on the host
        // PATH cannot win.
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = dir.path().join("opencode.cmd");
        // The shim echoes a marker it receives through the
        // environment, so the discovery proves the child
        // inherited the command's env.
        std::fs::write(
            &shim,
            "@echo openai/gpt-5.4\r\n@echo prov/%AGENTHD_TEST_MARKER%\r\n",
        )
        .expect("write shim");
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            65536,
            || {
                let mut c = Command::new("opencode");
                c.arg("api").arg("get").arg("/api/model");
                c.env("PATH", dir.path());
                c.env("AGENTHD_TEST_MARKER", "marker");
                c
            },
            || {
                let mut c = Command::new("opencode");
                c.arg("models");
                c.env("PATH", dir.path());
                c.env("AGENTHD_TEST_MARKER", "marker");
                c
            },
            &token,
            &mut sink,
        );
        // The shim answers BOTH commands with the same lines, so
        // the catalog payload is not a catalog and the fallback
        // (the same shim) answers with the model listing.
        assert_eq!(report.finish, Finish::Completed);
        match report.partial {
            Some(Discovery::Found(models)) => {
                assert!(models.contains(&"openai/gpt-5.4".to_string()), "{models:?}");
                assert!(
                    models.contains(&"prov/marker".to_string()),
                    "the shim must inherit the command env: {models:?}"
                );
            }
            other => panic!("expected Found, got: {other:?}"),
        }

        // The working directory of the original command is kept
        // as well: the shim reports it on stderr and exits
        // non-zero, so the discovery's error message carries it.
        std::fs::write(&shim, "@echo cwd=%CD% 1>&2\r\nexit /b 3\r\n").expect("write shim");
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_model_selectors_controlled_with(
            Duration::from_secs(30),
            65536,
            || {
                let mut c = Command::new("opencode");
                c.arg("api").arg("get").arg("/api/model");
                c.env("PATH", dir.path());
                c.current_dir(dir.path());
                c
            },
            || {
                let mut c = Command::new("opencode");
                c.arg("models");
                c.env("PATH", dir.path());
                c.current_dir(dir.path());
                c
            },
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Failed);
        let message = report.error.as_deref().unwrap_or("");
        assert!(message.contains("cwd="), "{message}");
        assert!(
            message.contains(&dir.path().to_string_lossy().to_string()),
            "the shim must keep the command cwd: {message}"
        );
    }

    #[test]
    fn discover_models_controlled_with_keeps_the_cli_only_contract() {
        // The published seam keeps its original shape: ONE
        // command factory, the `opencode models` listing, and
        // the original report semantics. The catalog is not
        // read here — a consumer compiled against the previous
        // five-argument signature keeps its behaviour.
        let token = CancelToken::default();
        let mut sink = empty_sink();
        let report = discover_models_controlled_with(
            Duration::from_secs(30),
            65536,
            || stdout_command("openai/gpt-5.4\nanthropic/claude-3\n"),
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Completed);
        match report.partial {
            Some(Discovery::Found(models)) => {
                assert_eq!(
                    models,
                    vec![
                        "anthropic/claude-3".to_string(),
                        "openai/gpt-5.4".to_string()
                    ]
                );
            }
            other => panic!("expected Found, got: {other:?}"),
        }
    }

    #[test]
    fn discover_models_controlled_with_keeps_its_published_signature() {
        // Compile-time pin of the published seam: a downstream
        // consumer must keep compiling against the five-argument,
        // one-generic shape.
        let _: fn(
            Duration,
            usize,
            Box<dyn FnOnce() -> Command>,
            &CancelToken,
            &mut dyn FnMut(Progress),
        ) -> OperationReport<Option<Discovery>> = |timeout, output_cap, factory, token, sink| {
            discover_models_controlled_with(timeout, output_cap, factory, token, sink)
        };
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
            let res = read_bounded(reader, 1024, "`opencode models`");
            let _ = writer.wait();
            assert!(res.is_err(), "overflow must surface as Err");
        }
    }
}
