//! Skills list UI: render / open / refresh / handle key / apply safe.
//!
//! Driven by `crate::store::plan_skills` / `apply_skills`. The UI is
//! intentionally shape-agnostic: the store produces a per-skill
//! `SkillPlanItem` row carrying the action label and reason, and the
//! handler dispatches `Esc` / arrows / `i` / `o` / `r` into the
//! appropriate plan / apply call.
//!
//! Conflict policy: there is no force-overwrite path. The `o` key is
//! surfaced as a status-bar decline so the user knows the omission
//! is intentional — `pi-psql` (or any other unrelated skill directory
//! in `Paths.skills_dir`) is never touched by this screen.

use super::{panel, render_popup, selected_style, truncate, App, Screen, DANGER, SUCCESS};
use crate::store::{apply_skills, plan_skills, SkillAction, SkillOutcome, SkillPlanItem};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, Paragraph};

impl App {
    pub(super) fn render_skills(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        items: &[SkillPlanItem],
        selected: usize,
        last_outcomes: &[SkillOutcome],
        status: Option<&str>,
    ) {
        let outcome_height = if last_outcomes.is_empty() { 0 } else { 8 };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(outcome_height)])
            .split(area);
        let header = format!("{:<26} {:<20} {}", "SKILL", "ACTION", "REASON");
        let mut list_items: Vec<ListItem> = Vec::new();
        list_items.push(ListItem::new(Line::from(header.bold())));
        if items.is_empty() {
            list_items.push(ListItem::new(
                Line::from("(no skills in the configured checkout's `skills/` directory)").italic(),
            ));
        }
        for (idx, item) in items.iter().enumerate() {
            let reason = match item.action {
                SkillAction::Install => "source present, target absent",
                SkillAction::Adopt => "destination already matches source",
                SkillAction::Update => "source changed since last install",
                SkillAction::UpToDate => "already in sync",
                SkillAction::Remove => "source removed, owned target remains",
                SkillAction::PreserveModified => {
                    "source removed, target modified externally; preserved"
                }
                SkillAction::Ghost => "stale manifest entry",
                SkillAction::Conflict => match (&item.source_tree_hash, &item.target_tree_hash) {
                    (Some(_), Some(_)) => "target has different bytes; refusing to overwrite",
                    (Some(_), None) => "target is not a regular directory",
                    (None, _) => "manifest drift; refresh to re-plan",
                },
            };
            let line = Line::from(format!(
                "{:<26} {:<20} {}",
                truncate(&item.name, 26),
                truncate(item.action.label(), 20),
                truncate(reason, 50),
            ));
            let style = if idx == selected {
                selected_style()
            } else if matches!(item.action, SkillAction::Conflict) {
                Style::default().fg(DANGER)
            } else if matches!(
                item.action,
                SkillAction::Install
                    | SkillAction::Adopt
                    | SkillAction::Update
                    | SkillAction::UpToDate
            ) {
                Style::default().fg(SUCCESS)
            } else {
                Style::default()
            };
            list_items.push(ListItem::new(line).style(style));
        }
        let list = List::new(list_items).block(panel("Skills"));
        frame.render_widget(list, chunks[0]);

        if !last_outcomes.is_empty() {
            let mut lines: Vec<Line> = Vec::new();
            for outcome in last_outcomes {
                let line = format!("{}: {} ({})", outcome.name, outcome.action, outcome.detail);
                let style = if outcome.ok {
                    Style::default()
                } else {
                    Style::default().fg(DANGER)
                };
                lines.push(Line::from(line).style(style));
            }
            let block = panel("Last action");
            frame.render_widget(Paragraph::new(lines).block(block), chunks[1]);
        }
        if let Some(text) = status {
            render_popup(frame, area, "Notice", text);
        }
    }

    /// Plan and render the Skills list. Items are computed fresh from
    /// disk and the on-disk manifest; the in-memory state on `App` is
    /// refreshed in-place so the next apply call sees the same view
    /// the user is looking at.
    pub(super) fn refresh_skills(&mut self) {
        let state = match crate::store::State::load(&self.paths.state_file) {
            Ok(s) => s,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        self.state = state;
        match plan_skills(&self.paths, &self.state) {
            Ok(items) => {
                let len = items.len();
                let (selected, outcomes) = if let Screen::Skills {
                    selected,
                    last_outcomes,
                    ..
                } = &self.screen
                {
                    (*selected, last_outcomes.clone())
                } else {
                    (0, Vec::new())
                };
                self.status_bar = None;
                self.screen = Screen::Skills {
                    items,
                    selected: if len == 0 { 0 } else { selected.min(len - 1) },
                    last_outcomes: outcomes,
                    status: None,
                };
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    pub(super) fn open_skills(&mut self) {
        self.refresh_skills();
    }

    pub(super) fn handle_skills_key(&mut self, key: KeyEvent) {
        enum Op {
            PopToSelector,
            MoveSelection(i32),
            Refresh,
            Install,
            OverwriteDeclined,
        }
        let op = match &mut self.screen {
            Screen::Skills { .. } => match key.code {
                KeyCode::Esc => Op::PopToSelector,
                KeyCode::Up | KeyCode::Char('k') => Op::MoveSelection(-1),
                KeyCode::Down | KeyCode::Char('j') => Op::MoveSelection(1),
                KeyCode::Char('r') => Op::Refresh,
                KeyCode::Char('i') => Op::Install,
                KeyCode::Char('o') => Op::OverwriteDeclined,
                _ => return,
            },
            _ => return,
        };
        match op {
            Op::PopToSelector => self.pop_skills_to_selector(),
            Op::MoveSelection(delta) => self.move_skills_selection(delta),
            Op::Refresh => self.refresh_skills(),
            Op::Install => self.apply_skills_install(),
            Op::OverwriteDeclined => self.decline_skills_overwrite(),
        }
    }

    fn pop_skills_to_selector(&mut self) {
        // Return to the Install/Update harness selector with no
        // target binding. Mirrors the agent list's `PopToSelector`
        // path: items and outcomes are cleared because the user
        // explicitly backed out of the list.
        self.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: None,
            target: None,
        };
        self.status_bar = None;
    }

    fn move_skills_selection(&mut self, delta: i32) {
        if let Screen::Skills {
            items, selected, ..
        } = &mut self.screen
        {
            if items.is_empty() {
                return;
            }
            if delta < 0 {
                *selected = selected.saturating_sub(1);
            } else if *selected + 1 < items.len() {
                *selected += 1;
            }
        }
    }

    fn apply_skills_install(&mut self) {
        // Re-read state and re-plan immediately before any writes so
        // we never act on stale items.
        let state = match crate::store::State::load(&self.paths.state_file) {
            Ok(s) => s,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        self.state = state;
        let plan = match plan_skills(&self.paths, &self.state) {
            Ok(p) => p,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        if plan.is_empty() {
            self.status_bar = Some("nothing to install for Skills".to_string());
            return;
        }
        match apply_skills(&self.paths, self.state.clone(), plan) {
            Ok((state, outcomes)) => {
                self.state = state;
                let succeeded = outcomes.iter().filter(|o| o.ok).count();
                let failed = outcomes.iter().filter(|o| !o.ok).count();
                if let Screen::Skills {
                    last_outcomes,
                    status,
                    ..
                } = &mut self.screen
                {
                    *last_outcomes = outcomes;
                    *status = Some(format!(
                        "skills install: {} ok, {} failed",
                        succeeded, failed
                    ));
                }
                self.status_bar = Some(format!(
                    "skills install: {} ok, {} failed",
                    succeeded, failed
                ));
                self.refresh_skills();
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    fn decline_skills_overwrite(&mut self) {
        // Skills sync has no force-overwrite path. The `o` key is
        // intentionally a no-op + an explanatory status message so
        // muscle-memory from the agent sync screen does not silently
        // overwrite a foreign skill directory the user wanted kept.
        self.status_bar =
            Some("skills sync has no force-overwrite; conflicts are kept as-is".to_string());
    }
}
