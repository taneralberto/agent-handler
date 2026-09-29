//! Agents list UI: render / open / handle key / delete.
//!
//! The screen is driven by `crate::store::load_canonical`; no
//! per-agent view exists in v1. The UI here is intentionally
//! shape-agnostic: the `AgentSummary` row carries the four fields
//! the table renders, and the handler dispatches Esc / arrows /
//! `n` / `e` / Enter / `d` into the editor or the delete gate.
//! Every method on `App` declared here is `pub(super)` so the
//! parent `app` module (and its test submodule) can call them
//! while keeping the visibility footprint minimal. `AgentSummary`
//! is `pub(super)` so the parent's `Screen::Agents` variant and
//! `editor.rs`'s `super::AgentSummary` reference (used by
//! `open_editor_existing`) keep resolving through the parent's
//! `use agents::AgentSummary;`.

use super::{panel, render_popup, selected_style, truncate, App, PendingDelete, Screen};
use crate::agent::Mode;
use crate::store::{delete_canonical, Paths};
use crate::workflows;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem};
use ratatui::Frame;

/// A compact view of an agent used to populate the Agents list.
/// Mirrors the fields that the list renders, so the editor and
/// other screens can build or pass summaries around without
/// loading the full canonical agent.
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
    ) {
        let header = format!(
            "{:<20} {:<10} {:<14} {}",
            "NAME", "MODE", "MODEL", "DESCRIPTION"
        );
        let mut items: Vec<ListItem> = Vec::new();
        items.push(ListItem::new(Line::from(header.bold())));
        if agents.is_empty() {
            items.push(ListItem::new::<Line>(
                Line::from("(no agents in this checkout; press `n` to create)")
                    .italic()
                    .into(),
            ));
        }
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
        if let Some(text) = status {
            render_popup(frame, area, "Notice", text);
        }
    }

    pub(super) fn open_agents(&mut self) {
        match workflows::list_canonical_agents(&self.paths) {
            Ok(agents) => {
                let agents: Vec<AgentSummary> = agents
                    .into_iter()
                    .map(|a| AgentSummary {
                        name: a.name,
                        description: a.description,
                        mode: a.mode,
                        model: a.model,
                    })
                    .collect();
                self.pending_delete = PendingDelete::default();
                self.status_bar = None;
                self.screen = Screen::Agents {
                    agents,
                    selected: 0,
                    status: None,
                };
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    pub(super) fn handle_agents_key(&mut self, key: KeyEvent, _paths: &Paths) {
        enum Op {
            Pop,
            Move(i32),
            New,
            Edit(AgentSummary),
            ArmDelete(String),
            ConfirmDelete(String),
        }
        let op = {
            if let Screen::Agents {
                agents,
                ref mut selected,
                ..
            } = &mut self.screen
            {
                match key.code {
                    KeyCode::Esc => Op::Pop,
                    KeyCode::Up | KeyCode::Char('k') => Op::Move(-1),
                    KeyCode::Down | KeyCode::Char('j') => Op::Move(1),
                    KeyCode::Char('n') => Op::New,
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
                    ..
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
}
