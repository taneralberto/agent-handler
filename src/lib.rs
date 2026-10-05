//! Shared library surface for the `agenthd` crate.
//!
//! This library exposes the UI-independent modules that back the
//! `agenthd` TUI binary (`store`, `workflows`, `agent`, `models`,
//! `tools`, `launcher`) so a future GUI client (Tauri or otherwise)
//! can depend on `agenthd` by path and reuse the same resolution,
//! store, workflows, and CLI parser without duplicating them.
//!
//! The TUI-specific `app` module (ratatui / crossterm wiring,
//! `TerminalGuard`, panic hook, the `App::run` loop) stays in the
//! binary crate (`src/main.rs`) — it depends on `ratatui` and
//! `crossterm`, which the GUI candidate will replace, so it is not
//! part of the shared surface. The seam that lets the TUI consume
//! the shared modules is the `use agenthd::...` imports the binary
//! pulls in here; the `app` module is declared locally in
//! `main.rs`.
//!
//! Cargo treats the `agenthd` package as both a library and a
//! binary. The binary is still built via `cargo build` (default
//! target) and the library via `cargo build --lib`. Adding this
//! file does not change the CLI surface, the TUI behavior, or the
//! `agenthd gui` rejection contract: the boot path runs the same
//! code through the same `use agenthd::...` paths. Only the module
//! declarations moved (from `mod` in `main.rs` to `pub mod` in
//! `lib.rs`).

pub mod agent;
pub mod launcher;
pub mod models;
pub mod operation;
pub mod store;
pub mod tools;
pub mod workflows;
