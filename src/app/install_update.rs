//! Install/Update screen: render / open / refresh / handle key / safe-install
//! and force-overwrite.
//!
//! Two-phase UI driven by `Screen::InstallUpdate.target`:
//! - `target = None`: the OpenCode/Pi selector is on screen. `Enter`
//!   plans that harness via `plan_for` and switches to the list view.
//!   `Esc` returns to the main menu.
//! - `target = Some(t)`: the per-file list for that harness is on screen.
//!   `i` / `o` / `r` all operate on `t` only — they call `plan_for` and
//!   `apply_safe` scoped to that harness and cannot reach the other
//!   target's files or ownership map.
//!
//! The screen is driven by `crate::store::plan_for`, which returns the
//! per-file `SyncItem` rows for one target. `force_overwrite` accepts a
//! `SyncTarget` and asserts it matches the current screen target before
//! touching disk, so a force-install cannot accidentally cross harnesses.
//! Every method on `App` declared here is `pub(super)` so the parent `app`
//! module (and its test submodule) can call them while keeping the
//! visibility footprint minimal. `Screen::InstallUpdate` stays in the
//! parent so the global key dispatch and the contextual footer can match
//! on its variants.

use super::{panel, render_popup, selected_style, truncate, App, Screen, DANGER, SUCCESS};
use crate::store::{
    apply_safe, force_install, load_canonical, plan_for, ApplyOutcome, State, SyncItem, SyncStatus,
    SyncTarget,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::Frame;

/// Static list of harnesses the Install/Update session can target.
/// Kept in source order so the selector's default selection (`OpenCode`)
/// matches the previous combined-view behavior.
const HARNESSES: &[SyncTarget] = &[SyncTarget::OpenCode, SyncTarget::Pi];

impl App {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_install_update(
        &self,
        frame: &mut Frame,
        area: Rect,
        items: &[SyncItem],
        selected: usize,
        last_outcomes: &[ApplyOutcome],
        status: Option<&str>,
        confirm_overwrite: Option<&(SyncTarget, String)>,
        target: Option<SyncTarget>,
    ) {
        // The confirm-overwrite popup is the highest-priority overlay:
        // it always wins over the selector, the list, and any transient
        // status message. Esc on an armed popup must cancel the popup,
        // not the list navigation underneath.
        if let Some((overwrite_target, filename)) = confirm_overwrite {
            let body = format!(
                "Overwrite `{}` on `{}` with current canonical? Press Y to confirm, N/Esc to cancel.",
                filename,
                overwrite_target.label()
            );
            render_popup(frame, area, "Overwrite?", &body);
            return;
        }
        match target {
            None => self.render_install_update_selector(frame, area, selected),
            Some(target) => {
                self.render_install_update_list(
                    frame,
                    area,
                    items,
                    selected,
                    last_outcomes,
                    status,
                    target,
                );
            }
        }
    }

    fn render_install_update_selector(&self, frame: &mut Frame, area: Rect, selected: usize) {
        let rows: Vec<ListItem> = HARNESSES
            .iter()
            .enumerate()
            .map(|(idx, target)| {
                let style = if idx == selected {
                    selected_style()
                } else {
                    Style::default()
                };
                ListItem::new(vec![
                    Line::from(target.label()),
                    Line::from(harness_detail(*target)),
                ])
                .style(style)
            })
            .collect();
        let list = List::new(rows)
            .block(panel("Install / Update — pick harness"))
            .highlight_style(selected_style())
            .highlight_symbol("▌ ");
        frame.render_widget(list, area);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_install_update_list(
        &self,
        frame: &mut Frame,
        area: Rect,
        items: &[SyncItem],
        selected: usize,
        last_outcomes: &[ApplyOutcome],
        status: Option<&str>,
        target: SyncTarget,
    ) {
        let outcome_height = if last_outcomes.is_empty() { 0 } else { 8 };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(outcome_height)])
            .split(area);
        let header = format!("{:<22} {:<20} {}", "file", "status", "reason");
        let mut list_items: Vec<ListItem> = Vec::new();
        list_items.push(ListItem::new(Line::from(header.bold())));
        for (idx, item) in items.iter().enumerate() {
            let line = Line::from(format!(
                "{:<22} {:<20} {}",
                truncate(&item.filename, 22),
                item.status.label(),
                truncate(&item.reason(), 50),
            ));
            let style = if idx == selected {
                selected_style()
            } else if matches!(item.status, SyncStatus::Conflict) {
                Style::default().fg(DANGER)
            } else if item.status.is_safe_action() {
                Style::default().fg(SUCCESS)
            } else {
                Style::default()
            };
            list_items.push(ListItem::new(line).style(style));
        }
        let title = format!("Install / Update — {}", target.label());
        let list = List::new(list_items).block(panel(&title));
        frame.render_widget(list, chunks[0]);

        if !last_outcomes.is_empty() {
            let mut lines: Vec<Line> = Vec::new();
            for outcome in last_outcomes {
                let line = format!(
                    "{}: {} ({})",
                    outcome.filename, outcome.action, outcome.detail
                );
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

    pub(super) fn open_install_update(&mut self) {
        // Land on the harness selector with no items loaded. Items are
        // computed only after the user picks a target so we never touch
        // a directory we don't need to read this session.
        self.status_bar = None;
        self.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: None,
            target: None,
        };
    }

    /// Re-plan the current target and update the on-screen items.
    /// `target` is the only harness that gets read or written by this
    /// call; the other harness's directory, manifest entries, and
    /// on-disk files are never touched.
    pub(super) fn refresh_install_update(&mut self) {
        let state = match State::load(&self.paths.state_file) {
            Ok(s) => s,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        self.state = state;
        let target = match &self.screen {
            Screen::InstallUpdate {
                target: Some(t), ..
            } => *t,
            // Refresh only makes sense once a target is picked; ignore
            // stray refreshes that arrive while the selector is on
            // screen. The selector has its own navigation keys.
            _ => return,
        };
        match plan_for(&self.paths, &self.state, target) {
            Ok(items) => {
                let len = items.len();
                let (selected, outcomes) = if let Screen::InstallUpdate {
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
                self.screen = Screen::InstallUpdate {
                    items,
                    selected: if len == 0 { 0 } else { selected.min(len - 1) },
                    last_outcomes: outcomes,
                    status: None,
                    confirm_overwrite: None,
                    target: Some(target),
                };
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    pub(super) fn handle_install_update_key(&mut self, key: KeyEvent) {
        enum Op {
            ConfirmForce(SyncTarget, String),
            CancelConfirm,
            Refresh,
            Install,
            BeginForce(SyncItem),
            MoveSelection(i32),
            MoveSelector(i32),
            PickTarget(SyncTarget),
            PopToSelector,
            PopToMain,
            MarkNonConflict(String),
        }
        let op: Op;
        if let Screen::InstallUpdate {
            items,
            selected,
            confirm_overwrite,
            target,
            ..
        } = &mut self.screen
        {
            if confirm_overwrite.is_some() {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let (t, filename) = confirm_overwrite.take().unwrap();
                        op = Op::ConfirmForce(t, filename);
                    }
                    KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                        *confirm_overwrite = None;
                        op = Op::CancelConfirm;
                    }
                    _ => return,
                }
            } else if target.is_none() {
                op = match key.code {
                    KeyCode::Esc => Op::PopToMain,
                    KeyCode::Up | KeyCode::Char('k') => Op::MoveSelector(-1),
                    KeyCode::Down | KeyCode::Char('j') => Op::MoveSelector(1),
                    KeyCode::Enter => Op::PickTarget(HARNESSES[*selected]),
                    _ => return,
                };
            } else {
                op = match key.code {
                    KeyCode::Esc => Op::PopToSelector,
                    KeyCode::Up | KeyCode::Char('k') => Op::MoveSelection(-1),
                    KeyCode::Down | KeyCode::Char('j') => Op::MoveSelection(1),
                    KeyCode::Char('r') => Op::Refresh,
                    KeyCode::Char('i') => Op::Install,
                    KeyCode::Char('o') => {
                        if let Some(item) = items.get(*selected) {
                            if matches!(item.status, SyncStatus::Conflict) {
                                Op::BeginForce(item.clone())
                            } else {
                                Op::MarkNonConflict(item.filename.clone())
                            }
                        } else {
                            return;
                        }
                    }
                    _ => return,
                };
            }
        } else {
            return;
        }
        match op {
            Op::ConfirmForce(target, filename) => self.force_overwrite(target, &filename),
            Op::CancelConfirm => self.status_bar = Some("overwrite cancelled".to_string()),
            Op::Refresh => self.refresh_install_update(),
            Op::Install => self.apply_safe_install(),
            Op::BeginForce(item) => {
                if let Screen::InstallUpdate {
                    confirm_overwrite, ..
                } = &mut self.screen
                {
                    *confirm_overwrite = Some((item.target, item.filename));
                }
            }
            Op::MoveSelection(delta) => {
                if let Screen::InstallUpdate {
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
            Op::MoveSelector(delta) => {
                if let Screen::InstallUpdate { selected, .. } = &mut self.screen {
                    if delta < 0 {
                        *selected = selected.saturating_sub(1);
                    } else if *selected + 1 < HARNESSES.len() {
                        *selected += 1;
                    }
                }
            }
            Op::PickTarget(target) => {
                // Bind the session to the chosen harness and plan it.
                // The list view, every safe-install action, and every
                // force-overwrite will be scoped to this target until
                // the user backs out to the selector.
                self.screen = Screen::InstallUpdate {
                    items: Vec::new(),
                    selected: 0,
                    last_outcomes: Vec::new(),
                    status: None,
                    confirm_overwrite: None,
                    target: Some(target),
                };
                self.refresh_install_update();
            }
            Op::PopToSelector => {
                // Drop the target binding so the next Enter on the
                // selector picks again. Items, last_outcomes, and the
                // selection are cleared because the user explicitly
                // backed out of the list.
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
            Op::PopToMain => {
                self.screen = Screen::Main { selected: 0 };
                self.status_bar = None;
            }
            Op::MarkNonConflict(name) => {
                // Match `CancelConfirm`: surface the notice through
                // the status bar only. Writing the screen popup too
                // makes the message stick across j/k navigation —
                // there's no code path that clears it on Move, and the
                // next action's popup would silently overwrite it
                // anyway. Status bar carries the message once and is
                // naturally replaced by the next key.
                self.status_bar = Some(format!("`{}` is not a conflict", name));
            }
        }
    }

    /// Apply all safe actions for the bound target only. The plan is
    /// computed via `plan_for`, so the other harness's files and
    /// ownership entries are guaranteed not to appear in `items`.
    /// `apply_safe`'s cleanup pass is scoped to the targets present in
    /// `items`, which is exactly the bound target here — the other
    /// harness's ownership map is left untouched even if it contains
    /// stale entries.
    pub(super) fn apply_safe_install(&mut self) {
        let state = match State::load(&self.paths.state_file) {
            Ok(s) => s,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        self.state = state;
        // Fail closed: every canonical must parse cleanly.
        if let Err(e) = load_canonical(&self.paths) {
            self.status_bar = Some(format!("error: {}", e));
            return;
        }
        let target = match &self.screen {
            Screen::InstallUpdate {
                target: Some(t), ..
            } => *t,
            _ => {
                self.status_bar = Some("pick a harness first".to_string());
                return;
            }
        };
        // Recompute the plan from disk before any writes so we never
        // act on stale items held in memory, and so the scope of the
        // plan is unambiguous.
        let plan = match plan_for(&self.paths, &self.state, target) {
            Ok(p) => p,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        if plan.is_empty() {
            // Empty plan is the safe no-op path: do not call apply_safe
            // (which would still walk cleanup, even though no targets
            // would qualify), and surface a clear message to the user.
            self.status_bar = Some(format!("nothing to install for {}", target.label()));
            return;
        }
        match apply_safe(&self.paths, self.state.clone(), plan) {
            Ok((state, outcomes)) => {
                self.state = state;
                let succeeded: usize = outcomes.iter().filter(|o| o.ok).count();
                let failed: usize = outcomes.iter().filter(|o| !o.ok).count();
                if let Screen::InstallUpdate {
                    last_outcomes,
                    status,
                    ..
                } = &mut self.screen
                {
                    *last_outcomes = outcomes;
                    *status = Some(format!(
                        "safe install {}: {} ok, {} failed",
                        target.label(),
                        succeeded,
                        failed
                    ));
                }
                self.status_bar = Some(format!(
                    "safe install {}: {} ok, {} failed",
                    target.label(),
                    succeeded,
                    failed
                ));
                self.refresh_install_update();
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    /// Force-overwrite a single conflict. `target` must equal the bound
    /// target; the assertion is the cross-harness guard the Install/Update
    /// session relies on, so a stale UI call (e.g. from a queued key event
    /// after the user backed out to the selector) cannot accidentally
    /// write the other harness.
    pub(super) fn force_overwrite(&mut self, target: SyncTarget, filename: &str) {
        let current = match &self.screen {
            Screen::InstallUpdate {
                target: Some(t), ..
            } => *t,
            _ => {
                self.status_bar = Some("overwrite refused: no harness is selected".to_string());
                return;
            }
        };
        if current != target {
            self.status_bar = Some(format!(
                "overwrite refused: bound to {}, cannot overwrite {}",
                current.label(),
                target.label()
            ));
            return;
        }
        let state = match State::load(&self.paths.state_file) {
            Ok(s) => s,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        self.state = state;
        match force_install(&self.paths, self.state.clone(), target, filename) {
            Ok((state, outcome)) => {
                self.state = state;
                if let Screen::InstallUpdate {
                    last_outcomes,
                    status,
                    ..
                } = &mut self.screen
                {
                    last_outcomes.push(outcome.clone());
                    *status = Some(format!("{}: {}", outcome.filename, outcome.action));
                }
                self.status_bar =
                    Some(format!("force installed {} `{}`", target.label(), filename));
                self.refresh_install_update();
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }
}

fn harness_detail(target: SyncTarget) -> &'static str {
    match target {
        SyncTarget::OpenCode => "Sync agents into the OpenCode targets directory",
        SyncTarget::Pi => "Sync agents into the Pi targets directory",
    }
}
