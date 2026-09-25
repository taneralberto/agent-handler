//! Agents list UI: render / open / handle key / delete / apply_update_bundled
//! and the pure status-bar formatter for the bundled-update summary.
//!
//! The screen is driven by `crate::store::load_canonical`; no per-agent view
//! exists in v1. The UI here is intentionally shape-agnostic: the
//! `AgentSummary` row carries the four fields the table renders, and the
//! handler dispatches Esc / arrows / `n` / `e` / Enter / `d` / `u` into the
//! editor, the delete gate, or the bundled-update gate. Every method on
//! `App` declared here is `pub(super)` so the parent `app` module (and its
//! test submodule) can call them while keeping the visibility footprint
//! minimal. `AgentSummary` is `pub(super)` so the parent's `Screen::Agents`
//! variant and `editor.rs`'s `super::AgentSummary` reference (used by
//! `open_editor_existing`) keep resolving through the parent's
//! `use agents::AgentSummary;`.

use super::{panel, render_popup, selected_style, truncate, App, PendingDelete, Screen};
use crate::agent::Mode;
use crate::store::{
    delete_canonical, load_canonical, update_bundled_prompts, Paths, UpdatePromptOutcome,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem};
use ratatui::Frame;

/// A compact view of an agent used to populate the Agents list. Mirrors the
/// fields that the list renders, so the editor and other screens can build or
/// pass summaries around without loading the full canonical agent.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct AgentSummary {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) mode: Mode,
    pub(super) model: Option<String>,
}

impl App {
    pub(super) fn render_agents(
        &self,
        frame: &mut Frame,
        area: Rect,
        agents: &[AgentSummary],
        selected: usize,
        status: Option<&str>,
        confirm_update_bundled: Option<&str>,
    ) {
        let header = format!(
            "{:<20} {:<10} {:<14} {}",
            "NAME", "MODE", "MODEL", "DESCRIPTION"
        );
        let mut items: Vec<ListItem> = Vec::new();
        items.push(ListItem::new(Line::from(header.bold())));
        for (idx, agent) in agents.iter().enumerate() {
            let line = Line::from(format!(
                "{:<20} {:<10} {:<14} {}",
                truncate(&agent.name, 20),
                truncate(agent.mode.as_str(), 10),
                truncate(agent.model.as_deref().unwrap_or("(inherit)"), 14),
                truncate(&agent.description, 60),
            ));
            let item = if idx == selected {
                ListItem::new(line).style(selected_style())
            } else {
                ListItem::new(line)
            };
            items.push(item);
        }
        let list = List::new(items).block(panel("Agents"));
        frame.render_widget(list, area);
        // The update confirmation takes precedence over a transient status
        // message: while the gate is armed, the screen should describe the
        // pending action instead of any older notice.
        if let Some(text) = confirm_update_bundled {
            render_popup(frame, area, "Update bundled prompts?", text);
        } else if let Some(text) = status {
            render_popup(frame, area, "Notice", text);
        }
    }

    pub(super) fn open_agents(&mut self) {
        match load_canonical(&self.paths) {
            Ok(map) => {
                let mut agents: Vec<AgentSummary> = map
                    .into_values()
                    .map(|(a, _)| AgentSummary {
                        name: a.name,
                        description: a.description,
                        mode: a.mode,
                        model: a.model,
                    })
                    .collect();
                agents.sort_by(|a, b| a.name.cmp(&b.name));
                self.pending_delete = PendingDelete::default();
                self.status_bar = None;
                self.screen = Screen::Agents {
                    agents,
                    selected: 0,
                    status: None,
                    confirm_update_bundled: None,
                };
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    pub(super) fn handle_agents_key(&mut self, key: KeyEvent, paths: &Paths) {
        enum Op {
            Pop,
            Move(i32),
            New,
            Edit(AgentSummary),
            ArmDelete(String),
            ConfirmDelete(String),
            ArmUpdateBundled,
            ConfirmUpdateBundled,
            CancelUpdateBundled,
        }
        let op = {
            if let Screen::Agents {
                agents,
                ref mut selected,
                confirm_update_bundled,
                ..
            } = &mut self.screen
            {
                // When the bundled-update confirmation is armed, only Y/N/Esc
                // are valid. Any other key is a no-op so users cannot
                // accidentally navigate or trigger destructive actions while
                // the gate is up.
                if confirm_update_bundled.is_some() {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => Op::ConfirmUpdateBundled,
                        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                            Op::CancelUpdateBundled
                        }
                        _ => return,
                    }
                } else {
                    match key.code {
                        KeyCode::Esc => Op::Pop,
                        KeyCode::Up | KeyCode::Char('k') => Op::Move(-1),
                        KeyCode::Down | KeyCode::Char('j') => Op::Move(1),
                        KeyCode::Char('n') => Op::New,
                        KeyCode::Char('u') => Op::ArmUpdateBundled,
                        KeyCode::Enter | KeyCode::Char('e') => {
                            if let Some(agent) = agents.get(*selected) {
                                Op::Edit(agent.clone())
                            } else {
                                return;
                            }
                        }
                        KeyCode::Char('d') => {
                            if let Some(agent) = agents.get(*selected) {
                                let name = agent.name.clone();
                                if self.pending_delete.name.as_deref() == Some(name.as_str()) {
                                    Op::ConfirmDelete(name)
                                } else {
                                    Op::ArmDelete(name)
                                }
                            } else {
                                return;
                            }
                        }
                        _ => return,
                    }
                }
            } else {
                return;
            }
        };
        match op {
            Op::Pop => {
                self.screen = Screen::Main { selected: 0 };
                self.status_bar = None;
                self.pending_delete = PendingDelete::default();
            }
            Op::Move(delta) => {
                if let Screen::Agents {
                    agents,
                    selected,
                    status,
                    confirm_update_bundled,
                } = &mut self.screen
                {
                    if agents.is_empty() {
                        return;
                    }
                    if delta < 0 {
                        *selected = selected.saturating_sub(1);
                    } else if *selected + 1 < agents.len() {
                        *selected += 1;
                    }
                    *status = None;
                    *confirm_update_bundled = None;
                }
                self.pending_delete = PendingDelete::default();
            }
            Op::New => self.open_editor_new(),
            Op::Edit(summary) => self.open_editor_existing(&summary),
            Op::ArmDelete(name) => {
                self.pending_delete = PendingDelete {
                    name: Some(name.clone()),
                };
                let msg = format!("Press `d` again to delete `{}`.", name);
                if let Screen::Agents { status, .. } = &mut self.screen {
                    *status = Some(msg.clone());
                }
                self.status_bar = Some(msg);
            }
            Op::ConfirmDelete(name) => {
                self.pending_delete = PendingDelete::default();
                self.delete_agent(&name);
            }
            Op::ArmUpdateBundled => {
                let msg = "Refresh the prompt body of every bundled canonical agent? Description, mode, model, and permissions are preserved. Y to confirm, N/Esc to cancel.".to_string();
                if let Screen::Agents {
                    confirm_update_bundled,
                    ..
                } = &mut self.screen
                {
                    *confirm_update_bundled = Some(msg);
                }
                // Suppress any prior transient status so the popup is the
                // only thing the screen communicates about this action.
                self.status_bar = None;
                self.pending_delete = PendingDelete::default();
            }
            Op::ConfirmUpdateBundled => {
                // Drop the gate before doing the work: the function call may
                // touch disk and we do not want the popup still armed while
                // results are reported. If it fails (very unlikely; only on
                // directory-level errors), restore it so the user can retry
                // or cancel cleanly.
                if let Screen::Agents {
                    confirm_update_bundled,
                    ..
                } = &mut self.screen
                {
                    *confirm_update_bundled = None;
                }
                self.apply_update_bundled(paths);
            }
            Op::CancelUpdateBundled => {
                if let Screen::Agents {
                    confirm_update_bundled,
                    ..
                } = &mut self.screen
                {
                    *confirm_update_bundled = None;
                }
                self.status_bar = Some("update bundled prompts cancelled".to_string());
                self.pending_delete = PendingDelete::default();
            }
        }
    }

    pub(super) fn delete_agent(&mut self, name: &str) {
        match delete_canonical(&self.paths, name) {
            Ok(()) => {
                self.status_bar = Some(format!("deleted canonical `{}`", name));
                self.open_agents();
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    /// Refresh the prompt body of every bundled canonical agent that
    /// currently exists. Called after the Agents screen's `u` gate is
    /// confirmed with `y`. Reports per-file outcomes via the status bar
    /// so the user sees what changed and what was skipped.
    ///
    /// `paths` is the snapshot of `self.paths` taken at the top of
    /// `handle_key`; using the snapshot avoids re-borrowing `self` while the
    /// status bar is being updated below.
    pub(super) fn apply_update_bundled(&mut self, paths: &Paths) {
        match update_bundled_prompts(paths) {
            Ok(outcomes) => {
                self.status_bar = Some(format_update_bundled_status(&outcomes));
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }
}

/// Summarize a `update_bundled_prompts` run into a single status-bar
/// message. Keeps the per-file detail out of the status line so the
/// bar stays readable, but stays expressive when something failed.
fn format_update_bundled_status(outcomes: &[UpdatePromptOutcome]) -> String {
    let updated = outcomes
        .iter()
        .filter(|o| o.action == "updated" && o.ok)
        .count();
    let kept = outcomes
        .iter()
        .filter(|o| o.action == "kept" && o.ok)
        .count();
    let skipped = outcomes
        .iter()
        .filter(|o| o.action == "skipped" && o.ok)
        .count();
    let failed: Vec<&UpdatePromptOutcome> = outcomes.iter().filter(|o| !o.ok).collect();
    if failed.is_empty() {
        if updated > 0 {
            format!(
                "updated bundled prompts: {} updated, {} kept, {} skipped",
                updated, kept, skipped
            )
        } else {
            // No rows needed a write; the user requested a no-op refresh.
            format!("bundled prompts already current ({} kept)", kept)
        }
    } else {
        let names: Vec<&str> = failed.iter().map(|o| o.name.as_str()).collect();
        format!(
            "updated bundled prompts: {} updated, {} failed ({})",
            updated,
            failed.len(),
            names.join(", ")
        )
    }
}
