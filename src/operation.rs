//! Cooperative cancellation, progress reporting, and the
//! `OperationReport<T>` return type that fronts long-running store
//! and tools workflows.
//!
//! Shared D3 surface
//! -----------------
//!
//! The TUI and the future GUI both consume the same backend workflows,
//! so this module lives in `src/lib.rs` (not in `src/main.rs`'s TUI
//! `app` module). It deliberately avoids every UI type: there is no
//! `ratatui`, no `crossterm`, no `tauri`, no `serde_json`-derived
//! persistence. A caller is free to take an `OperationReport<T>` and
//! turn it into TUI rows, Tauri events, or CLI text — the backend
//! has no opinion.
//!
//! Cooperative cancellation model
//! ------------------------------
//!
//! Long-running store workflows (`apply_safe_controlled`,
//! `apply_skills_controlled`) and tool installs (`install_tool_at_controlled`,
//! `install_tool_controlled`)
//! receive a [`CancelToken`]. Cancellation is **cooperative**: the
//! workflow checks the token at the safe points documented by the
//! workflow, never inside an OS-level critical section (a running
//! `renameat2` / `MoveFileW`, an in-flight `Command::output`, an
//! `apply_one` row that has already mutated state). A request that
//! arrives mid-row is honored at the next safe checkpoint, not
//! immediately.
//!
//! Progress model
//! --------------
//!
//! [`Progress`] is a small value type the workflow emits before each
//! safe checkpoint. Callers hand in a [`ProgressSink`] (`&mut dyn
//! FnMut(Progress)`); the closure runs synchronously and may update
//! the UI, append to a log, or simply drop the value. The workflow
//! never stores the sink — every emission is a direct call.
//!
//! OperationReport model
//! ---------------------
//!
//! [`OperationReport<T>`] is the return type for every controlled
//! workflow. `partial` is the fresh in-memory `State` and the full
//! per-row `Vec<Outcome>` the workflow produced before stopping. The
//! caller is expected to persist its own UI state from the report;
//! the workflow itself persists per-row (after each row that
//! actually changed `state`) so a partial run still has every
//! completed row on disk. The full Completed path also runs the
//! legacy per-target `retain` cleanup pass before reporting.
//!
//! Public surface
//! --------------
//!
//! - [`CancelToken`] (cheap clone via `Arc<AtomicBool>`)
//! - [`Progress`]
//! - [`Finish`] / [`OperationReport`]
//! - [`ProgressSink`]
//!
//! Helpers like `run_cancel_checked` are internal: they encode the
//! "check token, return `Cancelled` if requested, otherwise emit a
//! progress line and continue" pattern every controlled workflow
//! uses, in one place.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Cheaply cloneable, single-shot cancellation flag.
///
/// `Default` produces a fresh, unrequested token. Callers that want
/// to share a token across threads construct it once via
/// `CancelToken::default()` and clone the `Arc`; the cancel
/// notification is then visible to every clone.
///
/// The flag is `AtomicBool` with `Relaxed` accessors. Relaxed is
/// sufficient for cooperative cancel: there is no payload or
/// happens-before relationship between the caller that calls
/// `request()` and the worker that calls `is_requested()` — the
/// worker only needs eventual visibility of the flag, which
/// `Relaxed` guarantees on every modern target. Acq/Rel is
/// unnecessary because the worker does not write back to the
/// canceller.
#[derive(Debug, Default, Clone)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    /// Build a fresh token. `CancelToken::default()` is identical.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent and safe to call from any
    /// thread. Does not block: the worker checks `is_requested()`
    /// at the next safe checkpoint.
    pub fn request(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    /// Returns `true` once [`CancelToken::request`] has been called.
    /// A `false` return is not a guarantee that cancel will not be
    /// requested later; workers must re-check before each unit of
    /// work that has a safe checkpoint.
    pub fn is_requested(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// One tick of cooperative progress.
///
/// `stage` names the phase the workflow is entering — for example
/// `"agents: row"`, `"skills: row"`, `"tools: publish"`. Callers
/// route on `stage` to update the visible status without re-parsing
/// detail strings.
///
/// `item` is the row identifier the workflow is about to touch (an
/// agent filename, a skill directory name, a tool catalog entry),
/// or `None` for stage ticks that are not bound to a single row.
///
/// `processed` and `total` describe progress along the unit of
/// work the stage owns. For row-based workflows (`agents: row`,
/// `skills: row`) `processed` is the count of rows the workflow
/// has finished, and the pre-row checkpoint emits `processed = idx`
/// (the index of the row about to start, 0-based). For
/// stage-based workflows (`tools: publish`, `models: complete`)
/// `processed` is the count of stages completed. `total` is `None`
/// when the workflow does not know the count up front; otherwise it
/// is the same unit as `processed`. UIs that key off
/// `processed == total` to settle must read `total != None` first.
#[derive(Debug, Clone)]
pub struct Progress {
    pub stage: &'static str,
    pub item: Option<String>,
    pub processed: usize,
    pub total: Option<usize>,
}

/// Terminal state of a controlled workflow. Each workflow decides
/// whether to map an internal failure (per-row error, manifest
/// persistence failure, …) onto `Failed` or onto `Cancelled`; the
/// two are distinct so a UI can render the difference without
/// re-parsing `error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    Completed,
    Cancelled,
    Failed,
}

/// Result of a controlled workflow. `partial` is whatever the
/// workflow produced before stopping — the fresh in-memory `State`
/// the caller should keep, and the per-row outcomes that were
/// already observable. `error` is the workflow's last failure
/// message for `Finish::Failed` / `Finish::Cancelled` paths; it is
/// `None` on `Finish::Completed`.
#[derive(Debug, Clone)]
pub struct OperationReport<T> {
    pub finish: Finish,
    pub partial: T,
    pub error: Option<String>,
}

/// Closure type a controlled workflow uses to surface progress.
/// `&mut dyn FnMut(Progress)` is the minimum that lets a UI caller
/// hold mutable UI state across calls without an extra trait.
pub type ProgressSink<'a> = &'a mut dyn FnMut(Progress);

/// Internal helper: check the token, return `Cancelled` if requested,
/// otherwise emit a progress line and continue. Centralized so every
/// safe checkpoint has the same observable behavior.
pub(crate) fn run_cancel_checked(
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
    progress: Progress,
) -> bool {
    if token.is_requested() {
        return true;
    }
    sink(progress);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_token_starts_unrequested() {
        let token = CancelToken::default();
        assert!(!token.is_requested());
    }

    #[test]
    fn request_flips_is_requested() {
        let token = CancelToken::default();
        token.request();
        assert!(token.is_requested());
    }

    #[test]
    fn clone_shares_state() {
        let token = CancelToken::default();
        let cloned = token.clone();
        cloned.request();
        assert!(token.is_requested());
        assert!(cloned.is_requested());
    }

    #[test]
    fn request_is_idempotent() {
        let token = CancelToken::default();
        token.request();
        token.request();
        assert!(token.is_requested());
    }

    #[test]
    fn operation_report_carries_partial_and_finish() {
        let report: OperationReport<u32> = OperationReport {
            finish: Finish::Completed,
            partial: 7,
            error: None,
        };
        assert_eq!(report.finish, Finish::Completed);
        assert_eq!(report.partial, 7);
        assert!(report.error.is_none());
    }

    #[test]
    fn run_cancel_checked_emits_when_unrequested() {
        let token = CancelToken::default();
        let captured = std::cell::RefCell::new(Vec::<Progress>::new());
        let mut closure = |p: Progress| captured.borrow_mut().push(p);
        let sink: &mut dyn FnMut(Progress) = &mut closure;
        let cancel = run_cancel_checked(
            &token,
            sink,
            Progress {
                stage: "test: stage",
                item: Some("row".into()),
                processed: 1,
                total: Some(3),
            },
        );
        assert!(!cancel);
        assert_eq!(captured.borrow().len(), 1);
    }

    #[test]
    fn run_cancel_checked_short_circuits_when_requested() {
        let token = CancelToken::default();
        token.request();
        let captured = std::cell::RefCell::new(Vec::<Progress>::new());
        let mut closure = |p: Progress| captured.borrow_mut().push(p);
        let sink: &mut dyn FnMut(Progress) = &mut closure;
        let cancel = run_cancel_checked(
            &token,
            sink,
            Progress {
                stage: "test: stage",
                item: None,
                processed: 0,
                total: None,
            },
        );
        assert!(cancel);
        assert!(captured.borrow().is_empty());
    }
}
