//! Agent editor + model picker UI: types (AgentDraft, EditorField,
//! EditorMode, EditorOp), render / open / handle / save / discard paths,
//! the pure field-navigation helpers, the external-prompt editor helper,
//! and the rename-then-save helper.
//!
//! This module implements both the editor and the model picker screens.
//! The parent `app` module retains the `Screen::Editor` and
//! `Screen::ModelPicker` variants and dispatches them through the
//! `Screen::Editor { .. } => self.handle_editor_key(...)` and
//! `Screen::ModelPicker { .. } => self.handle_model_picker_key(...)`
//! arms of `handle_key`; the parent also keeps the contextual footer
//! for these screens. `editor_draft`,
//! `editor_original_name`, and `editor_prior_hash` stay on `App` so the
//! model picker (which replaces `screen`) can still mutate the draft
//! and restore the editor with the picked model applied.
//!
//! Every method on `App` declared here is `pub(super)` so the parent
//! `app` module (and its test submodule) can call them while keeping
//! the visibility footprint minimal. `AgentDraft`, `EditorField`,
//! `EditorMode`, and `EditorOp` are `pub(super)` so the parent's
//! `Screen` variants and the existing tests in `mod tests` can match
//! on them.

use super::{
    border_style_for, centered_rect, panel, render_popup, selected_style, App, PendingDelete,
    Screen, ACCENT, MUTED,
};
use crate::agent::{Agent, Mode, PermissionAction, PERMISSION_KEYS};
use crate::models::{self, Discovery};
use crate::store::{hash_file, rename_canonical, save_canonical, Paths};
use anyhow::{anyhow, bail, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Editable view of an agent.
#[derive(Debug, Clone)]
pub(super) struct AgentDraft {
    pub(super) agent: Agent,
    pub(super) permissions_view: Vec<(String, Option<PermissionAction>)>,
    pub(super) prompt: String,
}

impl AgentDraft {
    pub(super) fn from_agent(agent: Agent) -> Self {
        let permissions_view = PERMISSION_KEYS
            .iter()
            .map(|key| {
                let value = agent.permissions.get(*key).copied();
                ((*key).to_string(), value)
            })
            .collect();
        let prompt = agent.prompt.clone();
        AgentDraft {
            agent,
            permissions_view,
            prompt,
        }
    }

    pub(super) fn materialize(&self) -> Agent {
        let mut agent = self.agent.clone();
        agent.prompt = self.prompt.clone();
        agent.permissions.clear();
        for (key, value) in &self.permissions_view {
            if let Some(action) = value {
                agent.permissions.insert(key.clone(), *action);
            }
        }
        agent
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.materialize().validate()
    }

    pub(super) fn is_dirty(&self, original: Option<&Agent>) -> bool {
        let material = self.materialize();
        match original {
            Some(orig) => &material != orig,
            None => !material.description.trim().is_empty() || !material.prompt.trim().is_empty(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorField {
    Name,
    Description,
    Mode,
    Model,
    Prompt,
    Permissions(usize),
}

impl EditorField {
    #[allow(dead_code)]
    fn label(&self) -> &'static str {
        match self {
            EditorField::Name => "Name",
            EditorField::Description => "Description",
            EditorField::Mode => "Mode",
            EditorField::Model => "Model",
            EditorField::Prompt => "Prompt",
            EditorField::Permissions(_) => "Permissions",
        }
    }

    /// Fields whose values are free-form text and therefore accept
    /// INSERT-mode typing. Mode, Model, and Permissions cycle a fixed set of
    /// values, so `i` is a no-op there.
    fn accepts_insert(&self) -> bool {
        matches!(
            self,
            EditorField::Name | EditorField::Description | EditorField::Prompt
        )
    }
}

/// Vim-style editor mode. Starts in NORMAL on every fresh editor session;
/// INSERT is only reachable from NORMAL via `i` on a text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorMode {
    Normal,
    Insert,
}

impl EditorMode {
    fn label(self) -> &'static str {
        match self {
            EditorMode::Normal => "NORMAL",
            EditorMode::Insert => "INSERT",
        }
    }
}

/// Outcomes of handling a single editor key. Holding this in a small enum
/// avoids double-borrowing `self` while destructuring the editor screen.
pub(super) enum EditorOp {
    Discard,
    RequestDiscard,
    Save {
        original_name: Option<String>,
        material: Agent,
    },
}

impl App {
    /// Whether `App` currently holds an editor draft. Used as a precondition
    /// for actions that need to read or mutate it.
    pub(super) fn has_editor_draft(&self) -> bool {
        self.editor_draft.is_some()
    }

    /// Re-enter the editor screen using the draft held on `App`. The picker
    /// uses this to hand control back after a model is applied or cancelled.
    /// Always returns to NORMAL so the editor never re-enters in INSERT.
    pub(super) fn restore_editor_screen(&mut self, status: Option<String>) {
        if self.editor_draft.is_some() {
            self.screen = Screen::Editor {
                field: EditorField::Model,
                mode: EditorMode::Normal,
                status,
                confirm_discard: false,
            };
        } else {
            // No draft to restore; fall back to the agents list so we never
            // leave the picker with no exit path.
            self.screen = Screen::Main { selected: 0 };
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_editor(
        &self,
        frame: &mut Frame,
        area: Rect,
        draft: &AgentDraft,
        field: EditorField,
        mode: EditorMode,
        status: Option<&str>,
        confirm_discard: bool,
    ) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Min(5),
            ])
            .split(area);
        self.render_mode_bar(frame, rows[0], field, mode);
        self.render_field_input(
            frame,
            rows[1],
            "Name",
            &draft.agent.name,
            field == EditorField::Name,
        );
        self.render_field_input(
            frame,
            rows[2],
            "Description",
            &draft.agent.description,
            field == EditorField::Description,
        );
        self.render_field_input(
            frame,
            rows[3],
            "Mode",
            draft.agent.mode.as_str(),
            field == EditorField::Mode,
        );
        let model_text = draft.agent.model.as_deref().unwrap_or("(inherit)");
        self.render_field_input(
            frame,
            rows[4],
            "Model",
            model_text,
            field == EditorField::Model,
        );

        let prompt_block =
            panel("Prompt").border_style(border_style_for(field == EditorField::Prompt));
        let prompt = Paragraph::new(draft.prompt.as_str())
            .block(prompt_block)
            .wrap(Wrap { trim: false });
        frame.render_widget(prompt, rows[5]);

        self.render_permissions(frame, rows[6], &draft.permissions_view, field);

        if confirm_discard {
            render_popup(
                frame,
                area,
                "Discard changes?",
                "Unsaved changes will be lost. Press Esc again to discard, or any other key to cancel.",
            );
        }
        if let Some(text) = status {
            render_popup(frame, area, "Notice", text);
        }
    }

    fn render_mode_bar(
        &self,
        frame: &mut Frame,
        area: Rect,
        _field: EditorField,
        mode: EditorMode,
    ) {
        // The footer is the sole shortcut reference. This row only keeps the
        // current Vim-style mode visible while editing.
        let line = Line::from(Span::styled(
            format!(" {} ", mode.label()),
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(line), area);
    }

    fn render_field_input(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        value: &str,
        active: bool,
    ) {
        let style = border_style_for(active);
        let block = panel(title).border_style(style);
        let paragraph = Paragraph::new(value).block(block);
        frame.render_widget(paragraph, area);
    }

    fn render_permissions(
        &self,
        frame: &mut Frame,
        area: Rect,
        view: &[(String, Option<PermissionAction>)],
        field: EditorField,
    ) {
        let active = matches!(field, EditorField::Permissions(_));
        let block = panel("Permissions").border_style(border_style_for(active));
        let inner = block.inner(area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                (0..view.len())
                    .map(|_| Constraint::Length(1))
                    .collect::<Vec<_>>(),
            )
            .split(inner);
        let active_idx = match field {
            EditorField::Permissions(idx) => Some(idx),
            _ => None,
        };
        for (i, (key, value)) in view.iter().enumerate() {
            let text = format!(
                "{:<18} {}",
                key,
                value.map(|a| a.as_str()).unwrap_or("inherit")
            );
            let line = if active_idx == Some(i) {
                Line::from(text).style(selected_style())
            } else {
                Line::from(text)
            };
            frame.render_widget(Paragraph::new(line), rows[i]);
        }
        frame.render_widget(block, area);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_model_picker(
        &self,
        frame: &mut Frame,
        area: Rect,
        discovery: &Discovery,
        manual: &str,
        selected: usize,
        manual_open: bool,
        status: Option<&str>,
    ) {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(
            Line::from(format!("Status · {}", discovery.status_text()))
                .style(Style::default().fg(MUTED)),
        );
        lines.push(Line::from(""));
        if let Discovery::Found(models) = discovery {
            for (i, m) in models.iter().enumerate() {
                let line = Line::from(format!("  {}", m));
                lines.push(if i == selected {
                    line.style(selected_style())
                } else {
                    line
                });
            }
        }
        let title = if manual_open {
            "Model picker (manual)"
        } else {
            "Model picker"
        };
        let block = panel(title);
        let para = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(para, area);

        if manual_open {
            let popup_area = centered_rect(60, 30, area);
            let manual_lines = vec![
                Line::from("Manual model (`provider/model`, blank = inherit). Tab to apply."),
                Line::from(""),
                Line::from(manual),
            ];
            let manual_block = panel("Manual model");
            let manual_para = Paragraph::new(manual_lines)
                .block(manual_block)
                .wrap(Wrap { trim: false });
            frame.render_widget(Clear, popup_area);
            frame.render_widget(manual_para, popup_area);
        }
        if let Some(text) = status {
            render_popup(frame, area, "Notice", text);
        }
    }

    pub(super) fn open_editor_new(&mut self) {
        let suggested = self.next_agent_name();
        match Agent::new_default(suggested.clone()) {
            Ok(agent) => {
                self.pending_delete = PendingDelete::default();
                self.status_bar = None;
                self.editor_draft = Some(AgentDraft::from_agent(agent));
                self.editor_original_name = None;
                self.editor_prior_hash = None;
                self.screen = Screen::Editor {
                    field: EditorField::Name,
                    mode: EditorMode::Normal,
                    status: None,
                    confirm_discard: false,
                };
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    pub(super) fn open_editor_existing(&mut self, summary: &super::AgentSummary) {
        let path = self
            .paths
            .canonical_dir
            .join(format!("{}.md", summary.name));
        let prior_hash = hash_file(&path).unwrap_or(None);
        match Agent::read(&path) {
            Ok(agent) => {
                self.editor_original_name = Some(agent.name.clone());
                self.editor_draft = Some(AgentDraft::from_agent(agent));
                self.editor_prior_hash = prior_hash;
                self.pending_delete = PendingDelete::default();
                self.status_bar = None;
                self.screen = Screen::Editor {
                    field: EditorField::Name,
                    mode: EditorMode::Normal,
                    status: None,
                    confirm_discard: false,
                };
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    fn next_agent_name(&self) -> String {
        let map = crate::store::load_canonical(&self.paths).unwrap_or_default();
        for i in 1..1000 {
            let candidate = format!("agent-{}", i);
            if !map.contains_key(&candidate) {
                return candidate;
            }
        }
        "agent".to_string()
    }

    pub(super) fn handle_editor_key(&mut self, key: KeyEvent, paths: &Paths) {
        // First-Esc-of-dirty edit: confirm-discard popup is active.
        let in_confirm = matches!(
            self.screen,
            Screen::Editor {
                confirm_discard: true,
                ..
            }
        );
        if in_confirm {
            if key.code == KeyCode::Esc {
                self.apply_editor_op(EditorOp::Discard);
            } else if let Screen::Editor {
                confirm_discard, ..
            } = &mut self.screen
            {
                *confirm_discard = false;
            }
            return;
        }

        // Snapshot the active mode and field. INSERT is only reachable from
        // NORMAL via `i` on a text field, and we restore the editor in
        // NORMAL after every model-picker round-trip, so these snapshots
        // always describe a consistent editor state.
        let (mode, field) = match &self.screen {
            Screen::Editor { mode, field, .. } => (*mode, *field),
            _ => unreachable!("handle_editor_key called outside the editor"),
        };

        if mode == EditorMode::Insert {
            self.handle_editor_key_insert(key, field);
            return;
        }

        // NORMAL: every action returns early so unrecognized keys can never
        // reach `edit_text_field`. The only free-text path is INSERT, via
        // `handle_editor_key_insert` above.
        match key.code {
            KeyCode::Esc => {
                self.editor_esc_or_discard(paths);
                return;
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.editor_try_save();
                return;
            }
            KeyCode::Char('q') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.editor_esc_or_discard(paths);
                return;
            }
            KeyCode::Char('w') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.editor_try_save();
                return;
            }
            KeyCode::Char('i') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if field.accepts_insert() {
                    if let Screen::Editor { mode, .. } = &mut self.screen {
                        *mode = EditorMode::Insert;
                    }
                }
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let perm_len = self
                    .editor_draft
                    .as_ref()
                    .map(|d| d.permissions_view.len())
                    .unwrap_or(0);
                if let Screen::Editor { field, .. } = &mut self.screen {
                    prev_field(field, perm_len);
                }
                return;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let perm_len = self
                    .editor_draft
                    .as_ref()
                    .map(|d| d.permissions_view.len())
                    .unwrap_or(0);
                if let Screen::Editor { field, .. } = &mut self.screen {
                    next_field(field, perm_len);
                }
                return;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.editor_apply_horizontal(field, true);
                return;
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.editor_apply_horizontal(field, false);
                return;
            }
            _ => {}
        }

        // Field-local NORMAL actions that depend on which field is active:
        // Enter on Model opens the picker; Tab/BackTab step between the
        // Prompt and the permission list; Space on a permission row cycles
        // the value. Each branch returns; there is no fall-through to
        // `edit_text_field`, so any other printable key is a no-op here.
        match field {
            EditorField::Model => {
                if key.code == KeyCode::Enter {
                    let model = self
                        .editor_draft
                        .as_ref()
                        .expect("editor screen implies draft")
                        .agent
                        .model
                        .clone();
                    self.open_model_picker(model);
                }
            }
            EditorField::Prompt => {
                if key.code == KeyCode::Char('e') {
                    self.edit_prompt_in_system_editor();
                } else if key.code == KeyCode::Tab || key.code == KeyCode::BackTab {
                    let mut field = EditorField::Prompt;
                    let perm_len = self
                        .editor_draft
                        .as_ref()
                        .map(|d| d.permissions_view.len())
                        .unwrap_or(0);
                    if key.code == KeyCode::Tab {
                        next_field(&mut field, perm_len);
                    } else {
                        prev_field(&mut field, perm_len);
                    }
                    if let Screen::Editor { field: slot, .. } = &mut self.screen {
                        *slot = field;
                    }
                }
                // All other keys in NORMAL on Prompt are explicit no-ops.
            }
            EditorField::Name | EditorField::Description => {
                // NORMAL never mutates text on Name or Description. INSERT
                // is the only path that calls `edit_text_field`.
            }
            EditorField::Permissions(idx) => match key.code {
                KeyCode::Tab | KeyCode::BackTab => {
                    let mut field = EditorField::Permissions(idx);
                    let perm_len = self
                        .editor_draft
                        .as_ref()
                        .map(|d| d.permissions_view.len())
                        .unwrap_or(0);
                    if key.code == KeyCode::Tab {
                        next_field(&mut field, perm_len);
                    } else {
                        prev_field(&mut field, perm_len);
                    }
                    if let Screen::Editor { field: slot, .. } = &mut self.screen {
                        *slot = field;
                    }
                }
                KeyCode::Char(' ') => {
                    let mut draft = self
                        .editor_draft
                        .take()
                        .expect("editor screen implies draft");
                    let (_, value) = &mut draft.permissions_view[idx];
                    *value = match value {
                        None => Some(PermissionAction::Allow),
                        Some(PermissionAction::Allow) => Some(PermissionAction::Ask),
                        Some(PermissionAction::Ask) => Some(PermissionAction::Deny),
                        Some(PermissionAction::Deny) => None,
                    };
                    self.editor_draft = Some(draft);
                }
                _ => {}
            },
            EditorField::Mode => {
                // Already handled above via editor_apply_horizontal (h/l/Left/Right).
            }
        }
    }

    /// Open the full prompt in the user's editor and replace the draft only
    /// after that editor exits successfully.
    fn edit_prompt_in_system_editor(&mut self) {
        let prompt = self
            .editor_draft
            .as_ref()
            .expect("editor screen implies draft")
            .prompt
            .clone();
        match edit_prompt_externally(&prompt) {
            Ok(edited) => {
                self.editor_draft
                    .as_mut()
                    .expect("editor screen implies draft")
                    .prompt = edited;
                self.status_bar = Some("prompt returned from system editor".to_string());
            }
            Err(e) => self.status_bar = Some(format!("error: prompt editor: {}", e)),
        }
    }

    /// Apply a Left/Right (or h/l) keypress: cycle Mode on the Mode field,
    /// step between permission rows on the Permissions field, no-op on the
    /// remaining fields. `back` selects `prev`/decrement; otherwise `next`/
    /// increment.
    fn editor_apply_horizontal(&mut self, field: EditorField, back: bool) {
        match field {
            EditorField::Mode => {
                let mut draft = self
                    .editor_draft
                    .take()
                    .expect("editor screen implies draft");
                draft.agent.mode = if back {
                    draft.agent.mode.prev()
                } else {
                    draft.agent.mode.next()
                };
                self.editor_draft = Some(draft);
            }
            EditorField::Permissions(idx) => {
                let len = self
                    .editor_draft
                    .as_ref()
                    .map(|d| d.permissions_view.len())
                    .unwrap_or(0);
                if back {
                    if idx > 0 {
                        if let Screen::Editor { field, .. } = &mut self.screen {
                            *field = EditorField::Permissions(idx - 1);
                        }
                    }
                } else if idx + 1 < len {
                    if let Screen::Editor { field, .. } = &mut self.screen {
                        *field = EditorField::Permissions(idx + 1);
                    }
                }
            }
            _ => {
                // Name, Description, Model, Prompt: explicit no-op so
                // h/l never leak into the text buffer.
            }
        }
    }

    /// INSERT-mode key handler: only character keys (including q, w, h, j,
    /// k, l), Backspace, and Esc reach here. Esc returns to NORMAL without
    /// touching the dirty flag; character keys append to the active text
    /// field. All other keys are ignored so navigation, save, and quit
    /// commands can never consume text in INSERT mode.
    fn handle_editor_key_insert(&mut self, key: KeyEvent, field: EditorField) {
        if key.code == KeyCode::Esc {
            if let Screen::Editor { mode, .. } = &mut self.screen {
                *mode = EditorMode::Normal;
            }
            return;
        }
        if !field.accepts_insert() {
            // Defensive: `i` is only honored on text fields, but if INSERT
            // is somehow active on a non-text field, swallow all keys rather
            // than mutate values that don't take free-form text.
            return;
        }
        let mut draft = self
            .editor_draft
            .take()
            .expect("editor screen implies draft");
        match field {
            EditorField::Name => edit_text_field(key, &mut draft.agent.name),
            EditorField::Description => edit_text_field(key, &mut draft.agent.description),
            EditorField::Prompt => edit_text_field(key, &mut draft.prompt),
            _ => {}
        }
        self.editor_draft = Some(draft);
    }

    /// Esc-in-NORMAL and `q`-in-NORMAL share the existing dirty-confirm
    /// behavior: clean draft discards immediately, dirty draft arms the
    /// confirmation popup.
    fn editor_esc_or_discard(&mut self, paths: &Paths) {
        let dirty = {
            let draft = self
                .editor_draft
                .as_ref()
                .expect("editor screen implies draft");
            let original_agent = self.editor_original_name.as_ref().and_then(|name| {
                Agent::read(&paths.canonical_dir.join(format!("{}.md", name))).ok()
            });
            draft.is_dirty(original_agent.as_ref())
        };
        if dirty {
            if let Screen::Editor {
                confirm_discard, ..
            } = &mut self.screen
            {
                *confirm_discard = true;
            }
            self.apply_editor_op(EditorOp::RequestDiscard);
        } else {
            self.apply_editor_op(EditorOp::Discard);
        }
    }

    /// Ctrl+S and `w`-in-NORMAL share the same validate-and-save path. On a
    /// validation failure the editor state stays intact and the error is
    /// surfaced through the status bar.
    fn editor_try_save(&mut self) {
        let op = {
            let draft = self
                .editor_draft
                .as_ref()
                .expect("editor screen implies draft");
            match draft.validate() {
                Ok(()) => EditorOp::Save {
                    original_name: self.editor_original_name.clone(),
                    material: draft.materialize(),
                },
                Err(e) => {
                    self.status_bar = Some(format!("error: {}", e));
                    return;
                }
            }
        };
        self.apply_editor_op(op);
    }

    pub(super) fn apply_editor_op(&mut self, op: EditorOp) {
        match op {
            EditorOp::Discard => {
                self.editor_draft = None;
                self.editor_original_name = None;
                self.editor_prior_hash = None;
                self.open_agents();
            }
            EditorOp::RequestDiscard => {
                self.status_bar = Some("Unsaved changes. Press Esc again to discard.".to_string());
            }
            EditorOp::Save {
                original_name,
                material,
            } => {
                // Clone, don't take: a failed save must leave the editor state
                // intact so the user can fix the conflict and retry without
                // re-opening.
                let prior_hash = self.editor_prior_hash.clone();
                match save_agent(&self.paths, original_name.clone(), prior_hash, material) {
                    Ok(name) => {
                        self.status_bar = Some(format!("saved `{}`", name));
                        self.editor_draft = None;
                        self.editor_original_name = None;
                        self.editor_prior_hash = None;
                        self.open_agents();
                    }
                    Err(e) => {
                        self.status_bar = Some(format!("error: {}", e));
                    }
                }
            }
        }
    }

    pub(super) fn open_model_picker(&mut self, current: Option<String>) {
        let discovery = models::discover_models();
        let manual = current.unwrap_or_default();
        self.status_bar = None;
        self.screen = Screen::ModelPicker {
            discovery,
            manual,
            selected: 0,
            manual_open: false,
            status: None,
        };
    }

    pub(super) fn handle_model_picker_key(&mut self, key: KeyEvent) {
        enum Action {
            Cancel,
            Refresh,
            ApplyDiscovered(String),
            ApplyManual(String),
            Type(char),
            Backspace,
            ToggleManual,
            None,
        }
        let action = {
            if let Screen::ModelPicker {
                discovery,
                manual,
                selected,
                manual_open,
                status,
                ..
            } = &mut self.screen
            {
                match key.code {
                    KeyCode::Esc => {
                        if *manual_open {
                            *manual_open = false;
                            Action::None
                        } else {
                            Action::Cancel
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Discovery::Found(models) = discovery {
                            if *selected > 0 {
                                *selected -= 1;
                            }
                            let _ = models;
                        }
                        Action::None
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Discovery::Found(models) = discovery {
                            if *selected + 1 < models.len() {
                                *selected += 1;
                            }
                        }
                        Action::None
                    }
                    KeyCode::Enter => {
                        if let Discovery::Found(models) = discovery {
                            if let Some(model) = models.get(*selected).cloned() {
                                Action::ApplyDiscovered(model)
                            } else {
                                Action::ApplyManual(manual.clone())
                            }
                        } else {
                            Action::ApplyManual(manual.clone())
                        }
                    }
                    KeyCode::Char('r') => {
                        *discovery = models::discover_models();
                        *selected = 0;
                        *status = Some("refreshed".to_string());
                        Action::Refresh
                    }
                    KeyCode::Char('m') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        Action::ToggleManual
                    }
                    KeyCode::Tab if *manual_open => Action::ApplyManual(manual.clone()),
                    KeyCode::Backspace if *manual_open => Action::Backspace,
                    KeyCode::Char(c)
                        if *manual_open && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        Action::Type(c)
                    }
                    _ => Action::None,
                }
            } else {
                Action::None
            }
        };
        match action {
            Action::ApplyDiscovered(value) => self.apply_model_value(Some(value)),
            Action::ApplyManual(manual) => self.apply_manual_model(&manual),
            Action::Cancel => self.cancel_model_picker(),
            Action::Refresh => {}
            Action::ToggleManual => {
                if let Screen::ModelPicker { manual_open, .. } = &mut self.screen {
                    *manual_open = !*manual_open;
                }
            }
            Action::Type(c) => {
                if let Screen::ModelPicker { manual, .. } = &mut self.screen {
                    manual.push(c);
                }
            }
            Action::Backspace => {
                if let Screen::ModelPicker { manual, .. } = &mut self.screen {
                    manual.pop();
                }
            }
            Action::None => {}
        }
    }

    fn cancel_model_picker(&mut self) {
        if !self.has_editor_draft() {
            // No draft to return to — drop straight to main so the user is
            // never trapped in the picker.
            self.screen = Screen::Main { selected: 0 };
            return;
        }
        self.restore_editor_screen(None);
    }

    fn apply_manual_model(&mut self, manual: &str) {
        let value = if manual.trim().is_empty() {
            None
        } else {
            Some(manual.trim().to_string())
        };
        self.apply_model_value(value);
    }

    pub(super) fn apply_model_value(&mut self, value: Option<String>) {
        if let Err(e) = Agent::validate_model_opt(&value) {
            self.status_bar = Some(format!("error: {}", e));
            return;
        }
        let draft = match self.editor_draft.as_mut() {
            Some(d) => d,
            None => {
                self.status_bar = Some("error: no editor draft to update".to_string());
                self.screen = Screen::Main { selected: 0 };
                return;
            }
        };
        draft.agent.model = value.clone();
        self.status_bar = Some(match &value {
            Some(v) => format!("model set to `{}`", v),
            None => "model cleared (inherit)".to_string(),
        });
        self.restore_editor_screen(self.status_bar.clone());
    }
}

/// Save an agent, performing a rename first if the name changed.
///
/// `prior_hash` is the SHA-256 captured when the editor opened the canonical
/// file (or `None` for a new agent). It is passed straight through to
/// `save_canonical` so an external edit made between open and save is
/// rejected. Recomputing it here would defeat the check.
fn save_agent(
    paths: &Paths,
    original_name: Option<String>,
    prior_hash: Option<String>,
    material: Agent,
) -> Result<String> {
    let target_name = material.name.clone();
    let needs_rename = original_name
        .as_ref()
        .map(|o| o != &target_name)
        .unwrap_or(false);
    if needs_rename {
        let old = original_name.clone().unwrap();
        let source_path = paths.canonical_dir.join(format!("{}.md", old));
        let current_hash = hash_file(&source_path)?;
        if current_hash.as_deref() != prior_hash.as_deref() {
            bail!(
                "`{}` changed on disk since this edit started; reload to pick up the latest version",
                source_path.display()
            );
        }
        rename_canonical(paths, &old, &target_name).map_err(|e| anyhow!("rename: {}", e))?;
    }
    save_canonical(paths, &material, prior_hash.as_deref()).map_err(|e| anyhow!("save: {}", e))?;
    Ok(target_name)
}

fn edit_prompt_externally(prompt: &str) -> Result<String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "agenthd-prompt-{}-{}.md",
        std::process::id(),
        stamp
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| anyhow!("create temporary file: {}", e))?;
        file.write_all(prompt.as_bytes())
            .map_err(|e| anyhow!("write temporary file: {}", e))?;
        drop(file);

        let editor = std::env::var_os("VISUAL")
            .or_else(|| std::env::var_os("EDITOR"))
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    "notepad.exe".into()
                } else {
                    "vi".into()
                }
            });
        let status = Command::new(editor)
            .arg(&path)
            .status()
            .map_err(|e| anyhow!("start editor: {}", e))?;
        if !status.success() {
            return Err(anyhow!("editor exited with {}", status));
        }
        fs::read_to_string(&path).map_err(|e| anyhow!("read edited prompt: {}", e))
    })();
    let _ = fs::remove_file(&path);
    result
}

pub(super) fn edit_text_field(key: KeyEvent, value: &mut String) {
    match key.code {
        KeyCode::Backspace => {
            value.pop();
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            value.push(c);
        }
        _ => {}
    }
}

pub(super) fn next_field(field: &mut EditorField, perm_len: usize) {
    *field = match *field {
        EditorField::Name => EditorField::Description,
        EditorField::Description => EditorField::Mode,
        EditorField::Mode => EditorField::Model,
        EditorField::Model => EditorField::Prompt,
        EditorField::Prompt => EditorField::Permissions(0),
        EditorField::Permissions(idx) => {
            if idx + 1 < perm_len {
                EditorField::Permissions(idx + 1)
            } else {
                EditorField::Name
            }
        }
    };
}

pub(super) fn prev_field(field: &mut EditorField, perm_len: usize) {
    *field = match *field {
        EditorField::Name => {
            if perm_len == 0 {
                EditorField::Name
            } else {
                EditorField::Permissions(perm_len - 1)
            }
        }
        EditorField::Description => EditorField::Name,
        EditorField::Mode => EditorField::Description,
        EditorField::Model => EditorField::Mode,
        EditorField::Prompt => EditorField::Model,
        EditorField::Permissions(0) => EditorField::Prompt,
        EditorField::Permissions(idx) => EditorField::Permissions(idx - 1),
    };
}

impl Mode {
    pub(super) fn prev(self) -> Self {
        match self {
            Mode::subagent => Mode::all,
            Mode::primary => Mode::subagent,
            Mode::all => Mode::primary,
        }
    }
}
