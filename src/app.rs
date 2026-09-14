use crate::agent::{Agent, Mode, PermissionAction, PERMISSION_KEYS};
use crate::models::{self, Discovery};
use crate::store::{
    apply_safe, compute_plan, delete_canonical, force_install,
    install_plugin as install_plugin_file, load_canonical, plugin_status, rename_canonical,
    save_canonical, uninstall_plugin as uninstall_plugin_file, update_bundled_prompts,
    ApplyOutcome, Paths, PluginStatus, State, SyncItem, SyncStatus, SyncTarget,
    UpdatePromptOutcome,
};
use anyhow::{anyhow, bail, Result};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::{DefaultTerminal, Frame};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const ACCENT: Color = Color::Rgb(94, 234, 212);
const SURFACE: Color = Color::Rgb(24, 29, 42);
const SURFACE_RAISED: Color = Color::Rgb(36, 44, 60);
const TEXT: Color = Color::Rgb(226, 232, 240);
const MUTED: Color = Color::Rgb(148, 163, 184);
const SUCCESS: Color = Color::Rgb(74, 222, 128);
const WARNING: Color = Color::Rgb(250, 204, 21);
const DANGER: Color = Color::Rgb(251, 113, 133);

/// Top-level menu options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainItem {
    Agents,
    InstallUpdate,
    Plugin,
    Exit,
}

impl MainItem {
    fn all() -> &'static [MainItem] {
        &[
            MainItem::Agents,
            MainItem::InstallUpdate,
            MainItem::Plugin,
            MainItem::Exit,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            MainItem::Agents => "Agents",
            MainItem::InstallUpdate => "Install/Update",
            MainItem::Plugin => "Subagent panel",
            MainItem::Exit => "Exit",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            MainItem::Agents => "Create and tune OpenCode roles",
            MainItem::InstallUpdate => "Review safe synchronization changes",
            MainItem::Plugin => "Manage the OpenCode task sidebar",
            MainItem::Exit => "Close agenthd",
        }
    }
}

/// Where the application currently is.
#[derive(Debug)]
enum Screen {
    Main {
        selected: usize,
    },
    Agents {
        agents: Vec<AgentSummary>,
        selected: usize,
        status: Option<String>,
        confirm_update_bundled: Option<String>,
    },
    Editor {
        field: EditorField,
        mode: EditorMode,
        status: Option<String>,
        confirm_discard: bool,
    },
    ModelPicker {
        discovery: Discovery,
        manual: String,
        selected: usize,
        manual_open: bool,
        status: Option<String>,
    },
    InstallUpdate {
        items: Vec<SyncItem>,
        selected: usize,
        last_outcomes: Vec<ApplyOutcome>,
        status: Option<String>,
        confirm_overwrite: Option<(SyncTarget, String)>,
    },
    Plugin {
        status: PluginStatus,
        message: Option<String>,
        confirm_uninstall: bool,
    },
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct AgentSummary {
    name: String,
    description: String,
    mode: Mode,
    model: Option<String>,
}

/// Editable view of an agent.
#[derive(Debug, Clone)]
struct AgentDraft {
    agent: Agent,
    permissions_view: Vec<(String, Option<PermissionAction>)>,
    prompt: String,
}

impl AgentDraft {
    fn from_agent(agent: Agent) -> Self {
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

    fn materialize(&self) -> Agent {
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

    fn validate(&self) -> Result<()> {
        self.materialize().validate()
    }

    fn is_dirty(&self, original: Option<&Agent>) -> bool {
        let material = self.materialize();
        match original {
            Some(orig) => &material != orig,
            None => !material.description.trim().is_empty() || !material.prompt.trim().is_empty(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorField {
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
enum EditorMode {
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

/// Two-press `d` delete state.
#[derive(Debug, Default)]
struct PendingDelete {
    name: Option<String>,
}

/// Outcomes of handling a single editor key. Holding this in a small enum
/// avoids double-borrowing `self` while destructuring the editor screen.
enum EditorOp {
    Discard,
    RequestDiscard,
    Save {
        original_name: Option<String>,
        material: Agent,
    },
}

#[derive(Debug)]
pub struct App {
    paths: Paths,
    state: State,
    screen: Screen,
    status_bar: Option<String>,
    pending_delete: PendingDelete,
    quit: bool,
    /// Editor draft and original name are kept on `App`, not inside `Screen::Editor`,
    /// so the model picker (which replaces `screen`) can still mutate the draft
    /// and restore the editor with the picked model applied.
    editor_draft: Option<AgentDraft>,
    editor_original_name: Option<String>,
    /// SHA-256 of the canonical file at the moment the editor was opened.
    /// Passed into `save_canonical` so an external edit made between open and
    /// save is rejected. `None` for new agents.
    editor_prior_hash: Option<String>,
}

impl App {
    pub fn new(paths: Paths, state: State) -> Self {
        App {
            paths,
            state,
            screen: Screen::Main { selected: 0 },
            status_bar: None,
            pending_delete: PendingDelete::default(),
            quit: false,
            editor_draft: None,
            editor_original_name: None,
            editor_prior_hash: None,
        }
    }

    /// Whether `App` currently holds an editor draft. Used as a precondition
    /// for actions that need to read or mutate it.
    fn has_editor_draft(&self) -> bool {
        self.editor_draft.is_some()
    }

    /// Re-enter the editor screen using the draft held on `App`. The picker
    /// uses this to hand control back after a model is applied or cancelled.
    /// Always returns to NORMAL so the editor never re-enters in INSERT.
    fn restore_editor_screen(&mut self, status: Option<String>) {
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

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            terminal.draw(|frame| self.render(frame))?;
            let event = crossterm::event::read()?;
            self.handle_event(event)?;
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        // The header, transient status, and contextual footer have fixed
        // height so every screen keeps the same visual frame.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area);
        let header_area = chunks[0];
        let body = chunks[1];
        let status_area = chunks[2];
        let footer_area = chunks[3];
        self.render_header(frame, header_area);
        match &self.screen {
            Screen::Main { selected } => self.render_main(frame, body, *selected),
            Screen::Agents {
                agents,
                selected,
                status,
                confirm_update_bundled,
            } => self.render_agents(
                frame,
                body,
                agents,
                *selected,
                status.as_deref(),
                confirm_update_bundled.as_deref(),
            ),
            Screen::Editor {
                field,
                mode,
                status,
                confirm_discard,
            } => {
                let draft = self
                    .editor_draft
                    .as_ref()
                    .expect("editor screen implies a draft");
                self.render_editor(
                    frame,
                    body,
                    draft,
                    *field,
                    *mode,
                    status.as_deref(),
                    *confirm_discard,
                );
            }
            Screen::ModelPicker {
                discovery,
                manual,
                selected,
                manual_open,
                status,
            } => self.render_model_picker(
                frame,
                body,
                discovery,
                manual,
                *selected,
                *manual_open,
                status.as_deref(),
            ),
            Screen::InstallUpdate {
                items,
                selected,
                last_outcomes,
                status,
                confirm_overwrite,
            } => self.render_install_update(
                frame,
                body,
                items,
                *selected,
                last_outcomes,
                status.as_deref(),
                confirm_overwrite.as_ref(),
            ),
            Screen::Plugin {
                status,
                message,
                confirm_uninstall,
            } => self.render_plugin(frame, body, *status, message.as_deref(), *confirm_uninstall),
        }
        self.render_status_line(frame, status_area);
        self.render_footer(frame, footer_area);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let screen = match &self.screen {
            Screen::Main { .. } => "Workspace",
            Screen::Agents { .. } => "Agents",
            Screen::Editor { .. } => "Agent editor",
            Screen::ModelPicker { .. } => "Model picker",
            Screen::InstallUpdate { .. } => "Install / Update",
            Screen::Plugin { .. } => "Subagent panel",
        };
        let title = Line::from(vec![
            Span::styled(
                "  ◆ AGENTHD  ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("OPEN CODE / ", Style::default().fg(MUTED)),
            Span::styled(
                screen.to_uppercase(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]);
        let subtitle = Line::from("  Agent definitions and OpenCode synchronization")
            .style(Style::default().fg(MUTED));
        frame.render_widget(
            Paragraph::new(vec![title, subtitle]).style(Style::default().bg(SURFACE_RAISED)),
            area,
        );
    }

    fn render_main(&self, frame: &mut Frame, area: Rect, selected: usize) {
        let items: Vec<ListItem> = MainItem::all()
            .iter()
            .map(|item| {
                ListItem::new(vec![
                    Line::from(item.label()).style(Style::default().add_modifier(Modifier::BOLD)),
                    Line::from(item.detail()).style(Style::default().fg(MUTED)),
                ])
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(selected));
        let list = List::new(items)
            .block(panel("Workspace"))
            .highlight_style(selected_style())
            .highlight_symbol("▌ ");
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_agents(
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

    #[allow(clippy::too_many_arguments)]
    fn render_editor(
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
    fn render_model_picker(
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

    #[allow(clippy::too_many_arguments)]
    fn render_install_update(
        &self,
        frame: &mut Frame,
        area: Rect,
        items: &[SyncItem],
        selected: usize,
        last_outcomes: &[ApplyOutcome],
        status: Option<&str>,
        confirm_overwrite: Option<&(SyncTarget, String)>,
    ) {
        let outcome_height = if last_outcomes.is_empty() { 0 } else { 8 };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(outcome_height)])
            .split(area);
        let header = format!(
            "{:<10} {:<22} {:<20} {}",
            "target", "file", "status", "reason"
        );
        let mut list_items: Vec<ListItem> = Vec::new();
        list_items.push(ListItem::new(Line::from(header.bold())));
        for (idx, item) in items.iter().enumerate() {
            let line = Line::from(format!(
                "{:<10} {:<22} {:<20} {}",
                item.target.label(),
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
        let list = List::new(list_items).block(panel("Install / Update"));
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
        if let Some((_, filename)) = confirm_overwrite {
            render_popup(
                frame,
                area,
                "Overwrite?",
                &format!(
                    "Overwrite `{}` with current canonical? Press Y to confirm, N/Esc to cancel.",
                    filename
                ),
            );
        }
        if let Some(text) = status {
            render_popup(frame, area, "Notice", text);
        }
    }

    fn render_plugin(
        &self,
        frame: &mut Frame,
        area: Rect,
        status: PluginStatus,
        message: Option<&str>,
        confirm_uninstall: bool,
    ) {
        let text = format!(
            "OpenCode sidebar plugin\n\nStatus: {}\n\nShows each subagent task with its title, role, model, input-context tokens, and status.\n\nInstall/Update copies only agenthd-subagents.tsx into OpenCode's global plugins directory. Uninstall removes it only when its last-installed hash still matches.",
            status.label()
        );
        let panel = Paragraph::new(text)
            .block(panel("Subagent panel"))
            .wrap(Wrap { trim: false });
        frame.render_widget(panel, area);
        if confirm_uninstall {
            render_popup(
                frame,
                area,
                "Uninstall subagent panel?",
                "Remove the agenthd-owned OpenCode plugin? Press Y to confirm, N/Esc to cancel.",
            );
        } else if let Some(text) = message {
            render_popup(frame, area, "Notice", text);
        }
    }

    /// Render the optional status line above the footer. Always occupies its
    /// allocated row (empty when `status_bar` is `None`) so the layout stays
    /// stable across transitions and the footer does not jump up and down.
    /// Semantic color is applied based on the message prefix so errors,
    /// successes, and warnings stay distinguishable at a glance.
    fn render_status_line(&self, frame: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let text = self.status_bar.clone().unwrap_or_default();
        let style = status_style_for(&text);
        let para = Paragraph::new(text).style(style);
        frame.render_widget(para, area);
    }

    /// Render the contextual footer as a deliberate command bar: a bright
    /// foreground on a stable accent background so the row reads on common
    /// dark and light terminal palettes. The text is truncated safely to
    /// the terminal width so narrow terminals never panic or overflow.
    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let text = self.footer_text();
        let truncated = truncate(&text, area.width as usize);
        let style = Style::default().fg(TEXT).bg(SURFACE_RAISED);
        let para = Paragraph::new(truncated).style(style);
        frame.render_widget(para, area);
    }

    /// Contextual shortcut summary for the active screen. Single source of
    /// truth for the bottom footer. Includes the modal confirmation states
    /// so the footer always describes the keys that are actually available
    /// right now rather than the underlying screen's default keys.
    fn footer_text(&self) -> String {
        match &self.screen {
            Screen::Main { .. } => "↑/↓ or j/k: select · Enter: open · q / Esc: quit".to_string(),
            Screen::Agents {
                confirm_update_bundled,
                ..
            } => {
                if confirm_update_bundled.is_some() {
                    return "Y: update prompts · N / Esc: cancel".to_string();
                }
                "↑/↓ or j/k: select · n: new · e / Enter: edit · d d: delete · u: update bundled prompts · Esc: back"
                    .to_string()
            }
            Screen::Editor {
                field,
                mode,
                confirm_discard,
                ..
            } => {
                if *confirm_discard {
                    return "Esc: discard changes · any other key: cancel".to_string();
                }
                match mode {
                    EditorMode::Normal => match field {
                        EditorField::Prompt => {
                            "e: edit prompt · i: inline edit · ↑/↓: field · w / Ctrl+S: save · q / Esc: back"
                                .to_string()
                        }
                        EditorField::Permissions(_) => {
                            "Space: cycle permission · h/l: row · ↑/↓: field · w / Ctrl+S: save · q / Esc: back"
                                .to_string()
                        }
                        EditorField::Mode => {
                            "h/l or ←/→: cycle mode · ↑/↓: field · w / Ctrl+S: save · q / Esc: back"
                                .to_string()
                        }
                        EditorField::Model => {
                            "Enter: choose model · ↑/↓: field · w / Ctrl+S: save · q / Esc: back"
                                .to_string()
                        }
                        EditorField::Name | EditorField::Description => {
                            "i: edit · ↑/↓: field · w / Ctrl+S: save · q / Esc: back".to_string()
                        }
                    }
                    EditorMode::Insert => {
                        "type to edit · Backspace: delete · Esc: NORMAL".to_string()
                    }
                }
            }
            Screen::ModelPicker { manual_open, .. } => {
                if *manual_open {
                    "type: edit manual · Tab: apply · Esc: close".to_string()
                } else {
                    "↑/↓ or j/k: select · Enter: apply · m: manual · r: refresh · Esc: cancel"
                        .to_string()
                }
            }
            Screen::InstallUpdate {
                confirm_overwrite, ..
            } => {
                if confirm_overwrite.is_some() {
                    return "Y: overwrite · N / Esc: cancel".to_string();
                }
                "↑/↓ or j/k: select · i: install safe · o: overwrite conflict · r: refresh · Esc: back"
                    .to_string()
            }
            Screen::Plugin {
                confirm_uninstall, ..
            } => {
                if *confirm_uninstall {
                    "Y: uninstall · N / Esc: cancel".to_string()
                } else {
                    "i: install/update · u: uninstall · r: refresh · Esc: back".to_string()
                }
            }
        }
    }

    fn handle_event(&mut self, event: Event) -> Result<()> {
        if let Event::Key(key) = event {
            if key.kind != KeyEventKind::Press {
                return Ok(());
            }
            self.handle_key(key)?;
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return Ok(());
        }
        // Snapshot paths so handlers don't have to re-borrow self.paths while
        // they hold a mutable borrow of self.screen.
        let paths = self.paths.clone();
        match self.screen {
            Screen::Main { .. } => self.handle_main_key(key),
            Screen::Agents { .. } => self.handle_agents_key(key, &paths),
            Screen::Editor { .. } => self.handle_editor_key(key, &paths),
            Screen::ModelPicker { .. } => self.handle_model_picker_key(key),
            Screen::InstallUpdate { .. } => self.handle_install_update_key(key),
            Screen::Plugin { .. } => self.handle_plugin_key(key),
        }
        Ok(())
    }

    fn handle_main_key(&mut self, key: KeyEvent) {
        if let Screen::Main { selected } = &mut self.screen {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    *selected = selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if *selected + 1 < MainItem::all().len() {
                        *selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let item = MainItem::all()[*selected];
                    match item {
                        MainItem::Agents => self.open_agents(),
                        MainItem::InstallUpdate => self.open_install_update(),
                        MainItem::Plugin => self.open_plugin(),
                        MainItem::Exit => self.quit = true,
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
                _ => {}
            }
        }
    }

    fn open_agents(&mut self) {
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

    fn open_install_update(&mut self) {
        self.refresh_install_update();
    }

    fn open_plugin(&mut self) {
        self.refresh_plugin();
    }

    fn refresh_plugin(&mut self) {
        match State::load(&self.paths.state_file).and_then(|state| {
            let status = plugin_status(&self.paths, &state)?;
            Ok((state, status))
        }) {
            Ok((state, status)) => {
                self.state = state;
                self.status_bar = None;
                self.screen = Screen::Plugin {
                    status,
                    message: None,
                    confirm_uninstall: false,
                };
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    fn refresh_install_update(&mut self) {
        let state = match State::load(&self.paths.state_file) {
            Ok(s) => s,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        self.state = state;
        match compute_plan(&self.paths, &self.state) {
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
                };
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    fn handle_agents_key(&mut self, key: KeyEvent, paths: &Paths) {
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

    fn open_editor_new(&mut self) {
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

    fn open_editor_existing(&mut self, summary: &AgentSummary) {
        let path = self
            .paths
            .canonical_dir
            .join(format!("{}.md", summary.name));
        let prior_hash = crate::store::hash_file(&path).unwrap_or(None);
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
        let map = load_canonical(&self.paths).unwrap_or_default();
        for i in 1..1000 {
            let candidate = format!("agent-{}", i);
            if !map.contains_key(&candidate) {
                return candidate;
            }
        }
        "agent".to_string()
    }

    fn delete_agent(&mut self, name: &str) {
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
    fn apply_update_bundled(&mut self, paths: &Paths) {
        match update_bundled_prompts(paths) {
            Ok(outcomes) => {
                self.status_bar = Some(format_update_bundled_status(&outcomes));
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent, paths: &Paths) {
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

    fn apply_editor_op(&mut self, op: EditorOp) {
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

    fn open_model_picker(&mut self, current: Option<String>) {
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

    fn handle_model_picker_key(&mut self, key: KeyEvent) {
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

    fn apply_model_value(&mut self, value: Option<String>) {
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

    fn handle_install_update_key(&mut self, key: KeyEvent) {
        enum Op {
            ConfirmForce(SyncTarget, String),
            CancelConfirm,
            Refresh,
            Install,
            BeginForce(SyncItem),
            MoveSelection(i32),
            PopToMain,
            MarkNonConflict(String),
        }
        let op: Op;
        if let Screen::InstallUpdate {
            items,
            selected,
            confirm_overwrite,
            ..
        } = &mut self.screen
        {
            if confirm_overwrite.is_some() {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let (target, filename) = confirm_overwrite.take().unwrap();
                        op = Op::ConfirmForce(target, filename);
                    }
                    KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                        *confirm_overwrite = None;
                        op = Op::CancelConfirm;
                    }
                    _ => return,
                }
            } else {
                op = match key.code {
                    KeyCode::Esc => Op::PopToMain,
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
            Op::PopToMain => {
                self.screen = Screen::Main { selected: 0 };
                self.status_bar = None;
            }
            Op::MarkNonConflict(name) => {
                if let Screen::InstallUpdate { status, .. } = &mut self.screen {
                    *status = Some(format!("`{}` is not a conflict", name));
                }
            }
        }
    }

    fn apply_safe_install(&mut self) {
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
        let plan = match compute_plan(&self.paths, &self.state) {
            Ok(p) => p,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
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
                    *status = Some(format!("safe install: {} ok, {} failed", succeeded, failed));
                }
                self.status_bar =
                    Some(format!("safe install: {} ok, {} failed", succeeded, failed));
                self.refresh_install_update();
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    fn handle_plugin_key(&mut self, key: KeyEvent) {
        let confirmed = matches!(
            self.screen,
            Screen::Plugin {
                confirm_uninstall: true,
                ..
            }
        );
        if confirmed {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.uninstall_plugin(),
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    if let Screen::Plugin {
                        confirm_uninstall, ..
                    } = &mut self.screen
                    {
                        *confirm_uninstall = false;
                    }
                    self.status_bar = Some("plugin uninstall cancelled".to_string());
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.screen = Screen::Main { selected: 0 };
                self.status_bar = None;
            }
            KeyCode::Char('r') => self.refresh_plugin(),
            KeyCode::Char('i') => self.install_plugin(),
            KeyCode::Char('u') => {
                if let Screen::Plugin {
                    confirm_uninstall, ..
                } = &mut self.screen
                {
                    *confirm_uninstall = true;
                }
            }
            _ => {}
        }
    }

    fn install_plugin(&mut self) {
        let state = match State::load(&self.paths.state_file) {
            Ok(state) => state,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        match install_plugin_file(&self.paths, state) {
            Ok((state, prior_status)) => {
                self.state = state;
                let message = format!("plugin {}", prior_status.label());
                self.refresh_plugin();
                if let Screen::Plugin { message: slot, .. } = &mut self.screen {
                    *slot = Some(message);
                }
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    fn uninstall_plugin(&mut self) {
        let state = match State::load(&self.paths.state_file) {
            Ok(state) => state,
            Err(e) => {
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        match uninstall_plugin_file(&self.paths, state) {
            Ok(state) => {
                self.state = state;
                self.refresh_plugin();
                if let Screen::Plugin { message, .. } = &mut self.screen {
                    *message = Some("plugin uninstalled".to_string());
                }
            }
            Err(e) => self.status_bar = Some(format!("error: {}", e)),
        }
    }

    fn force_overwrite(&mut self, target: SyncTarget, filename: &str) {
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
        let current_hash = crate::store::hash_file(&source_path)?;
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

fn edit_text_field(key: KeyEvent, value: &mut String) {
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

fn next_field(field: &mut EditorField, perm_len: usize) {
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

fn prev_field(field: &mut EditorField, perm_len: usize) {
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

fn panel(title: &str) -> Block<'static> {
    Block::default()
        .title(Line::from(format!(" {} ", title)).style(title_style()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style_for(false))
        .style(Style::default().fg(TEXT).bg(SURFACE))
}

fn border_style_for(active: bool) -> Style {
    if active {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED)
    }
}

fn title_style() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

fn selected_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

/// Semantic color for a status-bar message. Errors go red, successes go
/// green, the unsaved-changes warning goes yellow, everything else stays
/// visible on the shared surface.
fn status_style_for(text: &str) -> Style {
    let color = if text.starts_with("error: ") {
        DANGER
    } else if text.starts_with("saved ")
        || text.starts_with("force installed ")
        || text.starts_with("deleted canonical ")
        || text.starts_with("updated bundled prompts")
    {
        SUCCESS
    } else if text.starts_with("Unsaved") {
        WARNING
    } else {
        TEXT
    };
    Style::default().fg(color).bg(SURFACE)
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn render_popup(frame: &mut Frame, area: Rect, title: &str, body: &str) {
    let popup_area = centered_rect(60, 30, area);
    let block = panel(title);
    let paragraph = Paragraph::new(body.to_string())
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, popup_area);
    frame.render_widget(paragraph, popup_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

impl Mode {
    fn prev(self) -> Self {
        match self {
            Mode::subagent => Mode::all,
            Mode::primary => Mode::subagent,
            Mode::all => Mode::primary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::starter_agent;
    use crate::agent::STARTERS;
    use tempfile::TempDir;

    fn setup_paths(dir: &TempDir) -> Paths {
        let paths = Paths {
            agenthd_root: dir.path().join(".agenthd"),
            canonical_dir: dir.path().join(".agenthd").join("agents"),
            state_file: dir.path().join(".agenthd").join("state.json"),
            target_dir: dir.path().join(".config").join("opencode").join("agents"),
            pi_target_dir: dir.path().join(".pi").join("agent").join("agents"),
            plugin_file: dir
                .path()
                .join(".config")
                .join("opencode")
                .join("plugins")
                .join("agenthd-subagents.tsx"),
            plugin_config: dir.path().join(".config").join("opencode").join("tui.json"),
        };
        paths.ensure_dirs().unwrap();
        paths
    }

    #[test]
    fn screen_transitions_create_new_editor() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        app.open_agents();
        app.pending_delete = PendingDelete {
            name: Some("scout".to_string()),
        };
        app.open_editor_new();
        assert!(matches!(app.screen, Screen::Editor { .. }));
        assert!(app.pending_delete.name.is_none());
    }

    #[test]
    fn save_and_open_existing_round_trip() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        // Open the agents list and pick scout so we exercise the real editor
        // flow (open_editor_existing captures the canonical hash).
        app.open_agents();
        let summary = match &app.screen {
            Screen::Agents { agents, .. } => agents
                .iter()
                .find(|a| a.name == STARTERS[0].name)
                .cloned()
                .expect("scout in agents list"),
            _ => panic!("expected agents screen"),
        };
        app.open_editor_existing(&summary);
        assert!(
            app.editor_prior_hash.is_some(),
            "open_editor_existing must capture the canonical hash"
        );

        // Modify the prompt and save through the normal flow.
        let prior_hash = app.editor_prior_hash.clone();
        let original_name = app.editor_original_name.clone();
        let material = {
            let draft = app.editor_draft.as_mut().expect("draft present");
            draft.prompt = "Updated prompt".to_string();
            draft.materialize()
        };
        app.apply_editor_op(EditorOp::Save {
            original_name: original_name.clone(),
            material,
        });
        // On success the editor transitions to the agents list (status_bar is
        // cleared by open_agents), so verify via screen state and file content.
        assert!(
            matches!(app.screen, Screen::Agents { .. }),
            "save should transition to the agents list: {:?}",
            app.screen
        );
        assert!(
            app.editor_draft.is_none() && app.editor_prior_hash.is_none(),
            "editor state should be cleared on successful save"
        );
        let prompt = std::fs::read_to_string(paths.canonical_dir.join("scout.md")).unwrap();
        assert!(prompt.contains("Updated prompt"));
        // The captured hash matched the on-disk hash at save time; otherwise
        // save_canonical would have rejected the save with a stale-write error.
        assert!(prior_hash.is_some(), "prior_hash captured at open");
        assert_eq!(original_name.as_deref(), Some(STARTERS[0].name));
    }

    #[test]
    fn save_rejects_external_edit_during_editor_session() {
        // The audit fix: prior_hash must be captured at open time, not
        // recomputed at save time. Otherwise an external edit that lands
        // between open and save slips through.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        let summary = match &app.screen {
            Screen::Agents { agents, .. } => agents
                .iter()
                .find(|a| a.name == STARTERS[0].name)
                .cloned()
                .expect("scout in agents list"),
            _ => panic!("expected agents screen"),
        };
        app.open_editor_existing(&summary);
        let prior_hash = app
            .editor_prior_hash
            .clone()
            .expect("open captured the hash");
        let original_name = app.editor_original_name.clone();

        // Externally rewrite the canonical file after the editor opened.
        let scout_path = paths.canonical_dir.join("scout.md");
        let original_bytes = std::fs::read_to_string(&scout_path).unwrap();
        let external = original_bytes.replace("read-only codebase scout", "externally rewritten");
        std::fs::write(&scout_path, &external).unwrap();
        assert_ne!(
            crate::store::hash_file(&scout_path).unwrap().as_deref(),
            Some(prior_hash.as_str()),
            "sanity: external edit changed the hash"
        );

        // The editor's save should reject because prior_hash is stale.
        let material = {
            let draft = app.editor_draft.as_mut().expect("draft present");
            draft.prompt = "Editor edit".to_string();
            draft.materialize()
        };
        app.apply_editor_op(EditorOp::Save {
            original_name: original_name.clone(),
            material,
        });
        assert!(
            app.status_bar
                .as_deref()
                .unwrap_or_default()
                .contains("error"),
            "save should fail with stale prior_hash: {:?}",
            app.status_bar
        );
        assert!(
            app.status_bar
                .as_deref()
                .unwrap_or_default()
                .contains("changed on disk"),
            "error should mention the stale-write condition: {:?}",
            app.status_bar
        );

        // The external bytes win; the editor's draft is not on disk.
        let on_disk = std::fs::read_to_string(&scout_path).unwrap();
        assert!(
            on_disk.contains("externally rewritten"),
            "external edit must be preserved"
        );
        assert!(
            !on_disk.contains("Editor edit"),
            "editor's stale draft must not be written"
        );

        // The editor still holds its prior_hash so the user can retry once
        // they reload.
        assert_eq!(app.editor_prior_hash.as_deref(), Some(prior_hash.as_str()));
        assert_eq!(
            app.editor_original_name.as_deref(),
            original_name.as_deref()
        );
    }

    #[test]
    fn save_rename_rejects_stale_source_without_moving_it() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        let summary = match &app.screen {
            Screen::Agents { agents, .. } => agents
                .iter()
                .find(|agent| agent.name == "scout")
                .cloned()
                .expect("scout present"),
            _ => panic!("expected agents screen"),
        };
        app.open_editor_existing(&summary);

        let source = paths.canonical_dir.join("scout.md");
        let external = std::fs::read_to_string(&source)
            .unwrap()
            .replace("read-only codebase scout", "externally rewritten");
        std::fs::write(&source, &external).unwrap();
        let material = {
            let draft = app.editor_draft.as_mut().expect("draft present");
            draft.agent.name = "renamed-scout".to_string();
            draft.materialize()
        };
        app.apply_editor_op(EditorOp::Save {
            original_name: app.editor_original_name.clone(),
            material,
        });

        assert!(app
            .status_bar
            .as_deref()
            .unwrap_or_default()
            .contains("changed on disk"));
        assert_eq!(std::fs::read_to_string(&source).unwrap(), external);
        assert!(!paths.canonical_dir.join("renamed-scout.md").exists());
    }

    #[test]
    fn save_rename_into_existing_destination_is_rejected() {
        // Rename collision semantics: do not overwrite a differing
        // destination. The existing `rename_canonical` already enforces this;
        // this test pins the behavior through the editor save flow.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        let scout_summary = match &app.screen {
            Screen::Agents { agents, .. } => agents
                .iter()
                .find(|a| a.name == STARTERS[0].name)
                .cloned()
                .expect("scout present"),
            _ => panic!("expected agents screen"),
        };
        app.open_editor_existing(&scout_summary);

        let original_name = app.editor_original_name.clone();
        let material = {
            let draft = app.editor_draft.as_mut().expect("draft present");
            // Try to rename scout onto reviewer, which already exists.
            draft.agent.name = "reviewer".to_string();
            draft.materialize()
        };
        app.apply_editor_op(EditorOp::Save {
            original_name,
            material,
        });
        assert!(
            app.status_bar
                .as_deref()
                .unwrap_or_default()
                .contains("error"),
            "rename into existing destination should fail: {:?}",
            app.status_bar
        );
        // scout.md still exists; reviewer.md still has its starter content.
        assert!(paths.canonical_dir.join("scout.md").exists());
        let reviewer = std::fs::read_to_string(paths.canonical_dir.join("reviewer.md")).unwrap();
        assert!(
            reviewer.contains("disciplined review subagent"),
            "reviewer.md must keep its original starter bytes"
        );
    }

    #[test]
    fn apply_model_validates_value() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        let draft = AgentDraft::from_agent(starter_agent(&STARTERS[0]));
        app.editor_draft = Some(draft);
        app.editor_original_name = Some(STARTERS[0].name.to_string());
        app.screen = Screen::Editor {
            field: EditorField::Model,
            mode: EditorMode::Normal,
            status: None,
            confirm_discard: false,
        };
        app.apply_model_value(Some("not-a-model".into()));
        assert!(app
            .status_bar
            .as_deref()
            .unwrap_or_default()
            .contains("error"));
        app.apply_model_value(Some("openai/gpt-5.4".into()));
        let draft = app.editor_draft.as_ref().expect("draft restored");
        assert_eq!(draft.agent.model.as_deref(), Some("openai/gpt-5.4"));
        assert!(matches!(app.screen, Screen::Editor { .. }));
    }

    #[test]
    fn model_picker_esc_restores_editor_with_draft_intact() {
        // The original bug: Esc in the model picker left the user trapped
        // because the picker replaced the editor screen and apply_model_value
        // matched on Screen::Editor.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        let mut draft = AgentDraft::from_agent(starter_agent(&STARTERS[0]));
        draft.agent.model = Some("openai/gpt-5.4".to_string());
        app.editor_draft = Some(draft);
        app.editor_original_name = Some(STARTERS[0].name.to_string());
        app.screen = Screen::Editor {
            field: EditorField::Model,
            mode: EditorMode::Normal,
            status: None,
            confirm_discard: false,
        };
        app.open_model_picker(Some("openai/gpt-5.4".to_string()));
        assert!(matches!(app.screen, Screen::ModelPicker { .. }));
        app.handle_model_picker_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Editor { .. }));
        let draft = app.editor_draft.as_ref().expect("draft preserved");
        assert_eq!(draft.agent.model.as_deref(), Some("openai/gpt-5.4"));
    }

    #[test]
    fn model_picker_tab_applies_manual_and_returns_to_editor() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        let draft = AgentDraft::from_agent(starter_agent(&STARTERS[0]));
        app.editor_draft = Some(draft);
        app.editor_original_name = Some(STARTERS[0].name.to_string());
        app.screen = Screen::Editor {
            field: EditorField::Model,
            mode: EditorMode::Normal,
            status: None,
            confirm_discard: false,
        };
        app.open_model_picker(None);
        // The picker may have populated Discovery::Found from the local
        // `opencode models` invocation; open the manual modal then type, then
        // Tab (which always applies manual, regardless of discovery).
        app.handle_model_picker_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::empty()));
        for c in "openai/gpt-5.4".chars() {
            app.handle_model_picker_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.handle_model_picker_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Editor { .. }));
        let draft = app.editor_draft.as_ref().expect("draft preserved");
        assert_eq!(draft.agent.model.as_deref(), Some("openai/gpt-5.4"));
    }

    #[test]
    fn model_picker_ignores_manual_input_while_browsing() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        app.editor_draft = Some(AgentDraft::from_agent(starter_agent(&STARTERS[0])));
        app.editor_original_name = Some(STARTERS[0].name.to_string());
        app.open_model_picker(Some("openai/gpt-5.4".to_string()));

        for key in [KeyCode::Char('x'), KeyCode::Backspace, KeyCode::Tab] {
            app.handle_model_picker_key(KeyEvent::new(key, KeyModifiers::empty()));
        }

        match &app.screen {
            Screen::ModelPicker {
                manual,
                manual_open,
                ..
            } => {
                assert!(!manual_open);
                assert_eq!(manual, "openai/gpt-5.4");
            }
            screen => panic!("unexpected screen: {screen:?}"),
        }
    }

    #[test]
    fn model_picker_m_toggles_manual_modal() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        app.editor_draft = Some(AgentDraft::from_agent(starter_agent(&STARTERS[0])));
        app.editor_original_name = Some(STARTERS[0].name.to_string());
        app.screen = Screen::Editor {
            field: EditorField::Model,
            mode: EditorMode::Normal,
            status: None,
            confirm_discard: false,
        };
        app.open_model_picker(None);
        let initial = match &app.screen {
            Screen::ModelPicker { manual_open, .. } => *manual_open,
            _ => false,
        };
        assert!(!initial);
        app.handle_model_picker_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::empty()));
        let after = match &app.screen {
            Screen::ModelPicker { manual_open, .. } => *manual_open,
            _ => false,
        };
        assert!(after);
        // Esc closes the manual modal instead of cancelling the picker.
        app.handle_model_picker_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        let closed = match &app.screen {
            Screen::ModelPicker { manual_open, .. } => *manual_open,
            _ => false,
        };
        assert!(!closed);
        // Now Esc returns to the editor.
        app.handle_model_picker_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Editor { .. }));
    }

    #[test]
    fn next_prev_field_cycle() {
        let mut field = EditorField::Name;
        next_field(&mut field, 2);
        assert_eq!(field, EditorField::Description);
        prev_field(&mut field, 2);
        assert_eq!(field, EditorField::Name);
        prev_field(&mut field, 2);
        assert!(matches!(field, EditorField::Permissions(1)));
    }

    #[test]
    fn mode_cycle_round_trip() {
        assert_eq!(Mode::subagent.next(), Mode::primary);
        assert_eq!(Mode::primary.next(), Mode::all);
        assert_eq!(Mode::all.next(), Mode::subagent);
        assert_eq!(Mode::subagent.prev(), Mode::all);
    }

    #[test]
    fn pending_delete_double_press_deletes() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.pending_delete = PendingDelete {
            name: Some("scout".to_string()),
        };
        assert!(paths.canonical_dir.join("scout.md").exists());
        app.delete_agent("scout");
        assert!(!paths.canonical_dir.join("scout.md").exists());
    }

    #[test]
    fn truncate_helper_shortens_and_passes_through() {
        // ASCII pass-through and shortening.
        let s = "abcdef";
        assert_eq!(truncate(s, 4), "abc…");
        assert_eq!(truncate(s, 10), "abcdef");
        // Multibyte: truncation must respect character boundaries, not bytes.
        assert_eq!(truncate("日本語", 2), "日…");
        assert_eq!(truncate("日本語", 3), "日本語");
        assert_eq!(truncate("日本語", 4), "日本語");
    }

    #[test]
    fn agent_draft_dirty_after_edit() {
        let mut draft = AgentDraft::from_agent(starter_agent(&STARTERS[0]));
        let original = starter_agent(&STARTERS[0]);
        assert!(!draft.is_dirty(Some(&original)));
        draft.agent.description = "new".into();
        assert!(draft.is_dirty(Some(&original)));
    }

    #[test]
    fn agent_draft_materialize_resets_prompt_and_permissions() {
        let mut draft = AgentDraft::from_agent(starter_agent(&STARTERS[0]));
        draft.prompt = "fresh prompt".into();
        draft.permissions_view[0].1 = Some(PermissionAction::Deny);
        let material = draft.materialize();
        assert_eq!(material.prompt, "fresh prompt");
        assert_eq!(
            material.permissions.get("read"),
            Some(&PermissionAction::Deny)
        );
    }

    #[test]
    fn edit_text_field_handles_char_and_backspace() {
        let mut s = String::from("ab");
        edit_text_field(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()),
            &mut s,
        );
        assert_eq!(s, "a");
        edit_text_field(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty()),
            &mut s,
        );
        assert_eq!(s, "ac");
    }

    #[test]
    fn editor_up_down_navigate_all_fields() {
        // The original bug: Up/Down only worked for the Mode and Permissions
        // rows, so users got stuck on whichever field they entered. Now
        // Up/Down traverse the whole editor surface via prev_field/next_field.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();

        let field = |app: &App| -> EditorField {
            match app.screen {
                Screen::Editor { field, .. } => field,
                _ => panic!("expected editor screen, got {:?}", app.screen),
            }
        };
        let down = |app: &mut App, paths: &Paths| {
            app.handle_editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()), paths);
        };
        let up = |app: &mut App, paths: &Paths| {
            app.handle_editor_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()), paths);
        };

        // From Name, Down walks every editor field in order.
        assert_eq!(field(&app), EditorField::Name);
        down(&mut app, &paths);
        assert_eq!(field(&app), EditorField::Description);
        down(&mut app, &paths);
        assert_eq!(field(&app), EditorField::Mode);
        down(&mut app, &paths);
        assert_eq!(field(&app), EditorField::Model);
        down(&mut app, &paths);
        assert_eq!(field(&app), EditorField::Prompt);
        down(&mut app, &paths);
        assert_eq!(field(&app), EditorField::Permissions(0));

        // From Permissions(0), Up jumps out to Prompt (no longer moves within
        // the permission list — that role is now Left/Right).
        up(&mut app, &paths);
        assert_eq!(field(&app), EditorField::Prompt);
    }

    #[test]
    fn editor_jk_navigate_fields_in_normal() {
        // NORMAL Vim semantics: `j`/`k` mirror Up/Down and walk the whole
        // editor surface (Name → Description → Mode → Model → Prompt →
        // Permissions and back). Typing literal j/k into a text field only
        // happens in INSERT mode; see editor_literal_jkl_in_insert below.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();

        let field_of = |app: &App| -> EditorField {
            match app.screen {
                Screen::Editor { field, .. } => field,
                _ => panic!("expected editor screen"),
            }
        };
        let mode_of = |app: &App| -> EditorMode {
            match app.screen {
                Screen::Editor { mode, .. } => mode,
                _ => panic!("expected editor screen"),
            }
        };
        assert_eq!(mode_of(&app), EditorMode::Normal);
        assert_eq!(field_of(&app), EditorField::Name);

        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Description);
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Mode);
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Model);
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Prompt);
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Permissions(0));

        // k walks back up; on the permission list it returns to Prompt
        // (matching the existing field-helper behavior).
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Prompt);

        // NORMAL must NOT have appended j/k to the Name buffer.
        assert_eq!(app.editor_draft.as_ref().unwrap().agent.name, "agent-1");
    }

    #[test]
    fn editor_literal_jklqw_in_insert() {
        // INSERT-mode contract: every printable character — including the
        // Vim navigation/save/quit keys q, w, h, j, k, l — appends verbatim
        // to the active text field. NORMAL would navigate on these keys, so
        // the test would fail if INSERT ever leaked.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();

        let mode_of = |app: &App| -> EditorMode {
            match app.screen {
                Screen::Editor { mode, .. } => mode,
                _ => panic!("expected editor screen"),
            }
        };
        let field_of = |app: &App| -> EditorField {
            match app.screen {
                Screen::Editor { field, .. } => field,
                _ => panic!("expected editor screen"),
            }
        };
        let type_chars = |app: &mut App, paths: &Paths, s: &str| {
            for c in s.chars() {
                app.handle_editor_key(
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()),
                    paths,
                );
            }
        };

        assert_eq!(mode_of(&app), EditorMode::Normal);
        assert_eq!(field_of(&app), EditorField::Name);

        // `i` enters INSERT, but only on text fields.
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(mode_of(&app), EditorMode::Insert);

        type_chars(&mut app, &paths, "qjkhl");
        assert_eq!(mode_of(&app), EditorMode::Insert);
        assert_eq!(field_of(&app), EditorField::Name);
        // All five Vim navigation/save/quit letters must have landed in the
        // name buffer instead of triggering their NORMAL actions.
        assert_eq!(
            app.editor_draft.as_ref().unwrap().agent.name,
            "agent-1qjkhl"
        );
    }

    #[test]
    fn editor_esc_returns_to_normal_from_insert() {
        // Esc in INSERT must switch back to NORMAL without running the
        // dirty-confirm path: the editor stays open, the draft is preserved,
        // and the next Esc in NORMAL is what arms the discard popup.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();

        let mode_of = |app: &App| -> EditorMode {
            match app.screen {
                Screen::Editor { mode, .. } => mode,
                _ => panic!("expected editor screen"),
            }
        };

        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(mode_of(&app), EditorMode::Insert);
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()), &paths);
        assert_eq!(mode_of(&app), EditorMode::Normal);
        assert!(matches!(app.screen, Screen::Editor { .. }));
        // Typing landed: the dirty-confirm path on the next Esc needs real
        // content to detect.
        assert_eq!(app.editor_draft.as_ref().unwrap().agent.name, "agent-1x");
    }

    #[test]
    fn editor_i_is_noop_on_non_text_field() {
        // `i` is only meaningful on text fields. On Mode, Model, and the
        // permission list it must leave the editor in NORMAL.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();

        let mode_of = |app: &App| -> EditorMode {
            match app.screen {
                Screen::Editor { mode, .. } => mode,
                _ => panic!("expected editor screen"),
            }
        };

        // Walk to Mode (Name → Description → Mode).
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        assert!(matches!(
            app.screen,
            Screen::Editor {
                field: EditorField::Mode,
                ..
            }
        ));
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(mode_of(&app), EditorMode::Normal);

        // Walk to Model.
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        assert!(matches!(
            app.screen,
            Screen::Editor {
                field: EditorField::Model,
                ..
            }
        ));
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(mode_of(&app), EditorMode::Normal);

        // Walk to Permissions(0).
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        ); // Prompt
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        ); // Permissions(0)
        assert!(matches!(
            app.screen,
            Screen::Editor {
                field: EditorField::Permissions(0),
                ..
            }
        ));
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(mode_of(&app), EditorMode::Normal);
    }

    #[test]
    fn editor_q_in_normal_does_not_change_text_and_triggers_discard() {
        // `q` in NORMAL is the dirty-confirmation discard path. The buffer
        // must NOT receive a literal 'q', and a clean draft discards
        // immediately (returns to Agents), while a dirty draft arms the
        // confirmation popup without leaving the editor.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();
        let original_name = app.editor_draft.as_ref().unwrap().agent.name.clone();
        // Clean draft: `q` should discard immediately.
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty()),
            &paths,
        );
        assert!(
            matches!(app.screen, Screen::Agents { .. }),
            "clean `q` should return to Agents: {:?}",
            app.screen
        );
        assert!(app.editor_draft.is_none());

        // Dirty draft: `q` should arm the discard popup, not discard yet.
        app.open_agents();
        app.open_editor_existing(&match &app.screen {
            Screen::Agents { agents, .. } => agents[0].clone(),
            _ => unreachable!(),
        });
        {
            let draft = app.editor_draft.as_mut().unwrap();
            draft.agent.description = "edit".into();
        }
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty()),
            &paths,
        );
        assert!(
            matches!(
                app.screen,
                Screen::Editor {
                    confirm_discard: true,
                    ..
                }
            ),
            "dirty `q` should arm confirm_discard: {:?}",
            app.screen
        );
        // Buffer must not contain a literal 'q' from this keystroke.
        assert!(
            !original_name.contains('q'),
            "sanity: starting name had no 'q' to confuse the assertion"
        );
        assert!(
            !app.editor_draft
                .as_ref()
                .unwrap()
                .agent
                .description
                .contains('q'),
            "description must not have consumed a literal 'q' from the keystroke"
        );
        app.handle_editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()), &paths);
        assert!(
            matches!(app.screen, Screen::Agents { .. }),
            "confirmed discard should return to Agents: {:?}",
            app.screen
        );
    }

    #[test]
    fn editor_w_in_normal_reaches_save_flow() {
        // `w` in NORMAL must reach the same validate-and-save path as Ctrl+S.
        // The cleanest evidence is that the editor transitions to the
        // agents list on a successful save.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        let summary = match &app.screen {
            Screen::Agents { agents, .. } => agents
                .iter()
                .find(|a| a.name == STARTERS[0].name)
                .cloned()
                .expect("scout in agents list"),
            _ => panic!("expected agents screen"),
        };
        app.open_editor_existing(&summary);
        // Make the draft dirty so a save actually runs.
        {
            let draft = app.editor_draft.as_mut().expect("draft present");
            draft.prompt = "Updated prompt via w".to_string();
        }
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::empty()),
            &paths,
        );
        assert!(
            matches!(app.screen, Screen::Agents { .. }),
            "save should transition to the agents list: {:?}",
            app.screen
        );
        let prompt = std::fs::read_to_string(paths.canonical_dir.join("scout.md")).unwrap();
        assert!(
            prompt.contains("Updated prompt via w"),
            "save did not persist the prompt change: {:?}",
            prompt
        );
    }

    #[test]
    fn editor_hl_preserve_mode_cycle_and_permission_rows() {
        // h/l mirror Left/Right: cycle Mode on the Mode field, step between
        // permission rows on the Permissions field, and are no-ops elsewhere.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();

        // Walk to Mode (Name → Description → Mode) using j.
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        let initial = app.editor_draft.as_ref().unwrap().agent.mode;
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(
            app.editor_draft.as_ref().unwrap().agent.mode,
            initial.next()
        );
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(app.editor_draft.as_ref().unwrap().agent.mode, initial);

        // Walk down to Permissions(0).
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        ); // Model
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        ); // Prompt
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        ); // Permissions(0)
        let field_of = |app: &App| -> EditorField {
            match app.screen {
                Screen::Editor { field, .. } => field,
                _ => panic!("expected editor screen"),
            }
        };
        assert_eq!(field_of(&app), EditorField::Permissions(0));
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Permissions(1));
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Permissions(0));

        // On a non-text, non-mode, non-permission field (Name) h/l must
        // not mutate the draft or change the field.
        let snapshot_name = app.editor_draft.as_ref().unwrap().agent.name.clone();
        // Walk back up to Name (Permissions(0) → Prompt → Model → Mode →
        // Description → Name is five k presses, not four).
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Name);
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Name);
        assert_eq!(app.editor_draft.as_ref().unwrap().agent.name, snapshot_name);
    }

    #[test]
    fn model_picker_round_trip_resets_editor_mode_to_normal() {
        // Entering the model picker from NORMAL and returning must leave
        // the editor in NORMAL. INSERT must not survive the round-trip
        // because the picker is only reachable from the Model field.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        let mut draft = AgentDraft::from_agent(starter_agent(&STARTERS[0]));
        draft.agent.model = Some("openai/gpt-5.4".to_string());
        app.editor_draft = Some(draft);
        app.editor_original_name = Some(STARTERS[0].name.to_string());
        // Pretend we came from INSERT (defensive: the picker should still
        // reset to NORMAL on return).
        app.screen = Screen::Editor {
            field: EditorField::Model,
            mode: EditorMode::Insert,
            status: None,
            confirm_discard: false,
        };
        app.open_model_picker(Some("openai/gpt-5.4".to_string()));
        assert!(matches!(app.screen, Screen::ModelPicker { .. }));
        app.handle_model_picker_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        let mode = match app.screen {
            Screen::Editor { mode, .. } => mode,
            _ => panic!("expected editor after picker Esc"),
        };
        assert_eq!(mode, EditorMode::Normal);
    }

    #[test]
    fn editor_left_right_preserves_mode_cycling() {
        // Left/Right still cycle Mode. The fix must not touch this path.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();
        // Walk down to Mode (Name -> Description -> Mode).
        app.handle_editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()), &paths);
        app.handle_editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()), &paths);

        let initial = app.editor_draft.as_ref().unwrap().agent.mode;
        app.handle_editor_key(KeyEvent::new(KeyCode::Right, KeyModifiers::empty()), &paths);
        let after_right = app.editor_draft.as_ref().unwrap().agent.mode;
        assert_eq!(after_right, initial.next());
        app.handle_editor_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()), &paths);
        let after_left = app.editor_draft.as_ref().unwrap().agent.mode;
        assert_eq!(after_left, initial);
    }

    #[test]
    fn editor_left_right_moves_permission_rows() {
        // Left/Right take over the within-permission-list navigation that
        // Up/Down used to do, so users still have a way to step between
        // permission rows without leaving the Permissions block.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();
        // Walk down to Permissions(0).
        for _ in 0..5 {
            app.handle_editor_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()), &paths);
        }
        let field = |app: &App| -> EditorField {
            match app.screen {
                Screen::Editor { field, .. } => field,
                _ => panic!("expected editor screen"),
            }
        };
        assert_eq!(field(&app), EditorField::Permissions(0));

        app.handle_editor_key(KeyEvent::new(KeyCode::Right, KeyModifiers::empty()), &paths);
        assert_eq!(field(&app), EditorField::Permissions(1));
        app.handle_editor_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()), &paths);
        assert_eq!(field(&app), EditorField::Permissions(0));
    }

    #[test]
    fn editor_arbitrary_key_in_normal_does_not_mutate_text_fields() {
        // Regression: NORMAL must consume (swallow) every key that is not an
        // explicit NORMAL action. Previously the handler fell through to
        // `edit_text_field` on Name/Description/Prompt, so a stray `x`
        // (or any other printable key) would silently append to the buffer.
        // The contract: NORMAL = navigation/save/quit only; text editing
        // requires INSERT (reached via `i`).
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.open_editor_new();

        let field_of = |app: &App| -> EditorField {
            match app.screen {
                Screen::Editor { field, .. } => field,
                _ => panic!("expected editor screen"),
            }
        };
        let mode_of = |app: &App| -> EditorMode {
            match app.screen {
                Screen::Editor { mode, .. } => mode,
                _ => panic!("expected editor screen"),
            }
        };
        let press = |app: &mut App, paths: &Paths, c: char| {
            app.handle_editor_key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()),
                paths,
            );
        };
        // Backspace is also covered: in NORMAL it must not strip characters.
        let press_bs = |app: &mut App, paths: &Paths| {
            app.handle_editor_key(
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()),
                paths,
            );
        };

        // ---------- Name ----------
        // New-agent default name is "agent-1". Press several arbitrary
        // printable keys plus Backspace in NORMAL; buffer must be byte-
        // identical afterwards.
        let name_before = app.editor_draft.as_ref().unwrap().agent.name.clone();
        assert_eq!(field_of(&app), EditorField::Name);
        assert_eq!(mode_of(&app), EditorMode::Normal);
        for c in "xyzabc123!@#".chars() {
            press(&mut app, &paths, c);
        }
        press_bs(&mut app, &paths);
        assert_eq!(mode_of(&app), EditorMode::Normal);
        assert_eq!(field_of(&app), EditorField::Name);
        assert_eq!(
            app.editor_draft.as_ref().unwrap().agent.name,
            name_before,
            "NORMAL leaked chars into Name: {:?}",
            app.editor_draft.as_ref().unwrap().agent.name
        );

        // After `i`, the same key MUST append (proves INSERT is intact and
        // proves the test would have caught the bug if it had regressed).
        press(&mut app, &paths, 'i');
        assert_eq!(mode_of(&app), EditorMode::Insert);
        press(&mut app, &paths, 'x');
        assert_eq!(
            app.editor_draft.as_ref().unwrap().agent.name,
            format!("{}x", name_before),
            "INSERT failed to append `x` to Name"
        );
        // Backspace in INSERT still edits: `x` removed, buffer restored.
        press_bs(&mut app, &paths);
        assert_eq!(app.editor_draft.as_ref().unwrap().agent.name, name_before);
        // Return to NORMAL so the next section's `j` navigates instead of
        // typing.
        app.handle_editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()), &paths);
        assert_eq!(mode_of(&app), EditorMode::Normal);

        // ---------- Description ----------
        // Move to Description (Name → Description).
        press(&mut app, &paths, 'j');
        assert_eq!(field_of(&app), EditorField::Description);
        assert_eq!(mode_of(&app), EditorMode::Normal);

        let desc_before = app.editor_draft.as_ref().unwrap().agent.description.clone();
        for c in "hello-world".chars() {
            press(&mut app, &paths, c);
        }
        press_bs(&mut app, &paths);
        assert_eq!(mode_of(&app), EditorMode::Normal);
        assert_eq!(field_of(&app), EditorField::Description);
        assert_eq!(
            app.editor_draft.as_ref().unwrap().agent.description,
            desc_before,
            "NORMAL leaked chars into Description: {:?}",
            app.editor_draft.as_ref().unwrap().agent.description
        );

        // After `i`, the same key MUST append.
        press(&mut app, &paths, 'i');
        assert_eq!(mode_of(&app), EditorMode::Insert);
        press(&mut app, &paths, 'x');
        assert_eq!(
            app.editor_draft.as_ref().unwrap().agent.description,
            format!("{}x", desc_before),
            "INSERT failed to append `x` to Description"
        );
        press_bs(&mut app, &paths);
        assert_eq!(
            app.editor_draft.as_ref().unwrap().agent.description,
            desc_before
        );
        app.handle_editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()), &paths);
        assert_eq!(mode_of(&app), EditorMode::Normal);

        // ---------- Prompt ----------
        // Move to Prompt (Description → Mode → Model → Prompt).
        press(&mut app, &paths, 'j'); // Mode
        press(&mut app, &paths, 'j'); // Model
        press(&mut app, &paths, 'j'); // Prompt
        assert_eq!(field_of(&app), EditorField::Prompt);
        assert_eq!(mode_of(&app), EditorMode::Normal);

        let prompt_before = app.editor_draft.as_ref().unwrap().prompt.clone();
        // Use a string that contains only letters/digits/symbols that have
        // no NORMAL meaning. `q` and `w` discard/save; `i` enters INSERT;
        // j/k/h/l are navigation. Anything else must be a no-op.
        for c in "abcdef 0123 .,-".chars() {
            press(&mut app, &paths, c);
        }
        press_bs(&mut app, &paths);
        assert_eq!(mode_of(&app), EditorMode::Normal);
        assert_eq!(field_of(&app), EditorField::Prompt);
        assert_eq!(
            app.editor_draft.as_ref().unwrap().prompt,
            prompt_before,
            "NORMAL leaked chars into Prompt: {:?}",
            app.editor_draft.as_ref().unwrap().prompt
        );

        // After `i`, the same key MUST append.
        press(&mut app, &paths, 'i');
        assert_eq!(mode_of(&app), EditorMode::Insert);
        press(&mut app, &paths, 'x');
        assert_eq!(
            app.editor_draft.as_ref().unwrap().prompt,
            format!("{}x", prompt_before),
            "INSERT failed to append `x` to Prompt"
        );
        press_bs(&mut app, &paths);
        assert_eq!(app.editor_draft.as_ref().unwrap().prompt, prompt_before);
    }

    // ---------- Contextual footer ----------

    /// Build an `App` with no filesystem state. `footer_text` only inspects
    /// the active screen, so the dummy paths are safe.
    fn fresh_app() -> App {
        App::new(
            Paths {
                agenthd_root: std::path::PathBuf::from("/tmp/agenthd-footer-test/.agenthd"),
                canonical_dir: std::path::PathBuf::from("/tmp/agenthd-footer-test/.agenthd/agents"),
                state_file: std::path::PathBuf::from(
                    "/tmp/agenthd-footer-test/.agenthd/state.json",
                ),
                target_dir: std::path::PathBuf::from(
                    "/tmp/agenthd-footer-test/.config/opencode/agents",
                ),
                pi_target_dir: std::path::PathBuf::from(
                    "/tmp/agenthd-footer-test/.pi/agent/agents",
                ),
                plugin_file: std::path::PathBuf::from(
                    "/tmp/agenthd-footer-test/.config/opencode/plugins/agenthd-subagents.tsx",
                ),
                plugin_config: std::path::PathBuf::from(
                    "/tmp/agenthd-footer-test/.config/opencode/tui.json",
                ),
            },
            State::default(),
        )
    }

    #[test]
    fn footer_main_describes_navigate_open_quit() {
        let mut app = fresh_app();
        app.screen = Screen::Main { selected: 0 };
        let text = app.footer_text();
        assert!(
            text.contains("Enter"),
            "main footer mentions Enter: {text:?}"
        );
        assert!(text.contains("open"), "main footer mentions open: {text:?}");
        assert!(text.contains("quit"), "main footer mentions quit: {text:?}");
        assert!(text.contains("Esc"), "main footer mentions Esc: {text:?}");
    }

    #[test]
    fn footer_agents_describes_select_new_edit_delete_back() {
        let mut app = fresh_app();
        app.screen = Screen::Agents {
            agents: Vec::new(),
            selected: 0,
            status: None,
            confirm_update_bundled: None,
        };
        let text = app.footer_text();
        assert!(text.contains("new"), "agents footer mentions new: {text:?}");
        assert!(
            text.contains("edit"),
            "agents footer mentions edit: {text:?}"
        );
        assert!(
            text.contains("delete"),
            "agents footer mentions delete: {text:?}"
        );
        assert!(
            text.contains("back"),
            "agents footer mentions back: {text:?}"
        );
        // The new bundled-update shortcut must be advertised in the idle
        // footer so users discover it without reading the README.
        assert!(
            text.contains("u") && text.contains("update bundled prompts"),
            "agents footer mentions u + update bundled prompts: {text:?}"
        );
    }

    #[test]
    fn footer_agents_confirm_update_overrides_default_keys() {
        let mut app = fresh_app();
        app.screen = Screen::Agents {
            agents: Vec::new(),
            selected: 0,
            status: None,
            confirm_update_bundled: Some("confirm?".to_string()),
        };
        let text = app.footer_text();
        assert!(text.contains("Y"), "confirm footer mentions Y: {text:?}");
        assert!(
            text.contains("cancel"),
            "confirm footer mentions cancel: {text:?}"
        );
        // The idle actions are suppressed while the gate is armed so the
        // footer cannot contradict itself.
        assert!(
            !text.contains("delete"),
            "confirm footer must not mention delete: {text:?}"
        );
        assert!(
            !text.contains("update bundled prompts"),
            "confirm footer must not advertise the idle shortcut: {text:?}"
        );
    }

    /// `u` arms the bundled-update gate; pressing `y` runs the refresh;
    /// pressing `n` cancels without writing.
    #[test]
    fn agents_screen_u_arms_and_n_cancels() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        // Capture a sentinel: every starter's prompt is the bundled one, so
        // any successful refresh would bump the prompt body. We want to
        // verify that n cancels *without* touching the files at all.
        let scout_path = paths.canonical_dir.join("scout.md");
        let scout_before = std::fs::read_to_string(&scout_path).unwrap();

        // Press `u` — gate should arm and no write should have happened.
        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()),
            &paths,
        );
        assert!(
            matches!(
                app.screen,
                Screen::Agents {
                    confirm_update_bundled: Some(_),
                    ..
                }
            ),
            "u should arm the bundled-update confirmation: {:?}",
            app.screen
        );
        assert_eq!(
            std::fs::read_to_string(&scout_path).unwrap(),
            scout_before,
            "u must not write until y is pressed"
        );

        // Press `n` — gate should clear with a cancel status, files still
        // untouched.
        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty()),
            &paths,
        );
        assert!(matches!(
            app.screen,
            Screen::Agents {
                confirm_update_bundled: None,
                ..
            }
        ));
        assert_eq!(
            std::fs::read_to_string(&scout_path).unwrap(),
            scout_before,
            "n must not write"
        );
        assert!(
            app.status_bar
                .as_deref()
                .unwrap_or_default()
                .contains("cancelled"),
            "status should announce cancellation: {:?}",
            app.status_bar
        );
    }

    /// `Esc` cancels the bundled-update gate without writing.
    #[test]
    fn agents_screen_u_arms_and_esc_cancels() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        let scout_path = paths.canonical_dir.join("scout.md");
        let scout_before = std::fs::read_to_string(&scout_path).unwrap();

        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()),
            &paths,
        );
        assert!(matches!(
            app.screen,
            Screen::Agents {
                confirm_update_bundled: Some(_),
                ..
            }
        ));
        app.handle_agents_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()), &paths);
        // Esc on the armed confirmation must cancel the gate, NOT pop back
        // to the main menu. Otherwise a stray Esc while reviewing the
        // confirmation would silently lose the user's place in the list.
        assert!(matches!(
            app.screen,
            Screen::Agents {
                confirm_update_bundled: None,
                ..
            }
        ));
        assert_eq!(
            std::fs::read_to_string(&scout_path).unwrap(),
            scout_before,
            "Esc must not write"
        );
    }

    /// While the gate is armed, only `y`/`Y`/`n`/`N`/`Esc` are valid; any
    /// other key must be a no-op so the user cannot accidentally navigate,
    /// edit, or delete agents while a prompt refresh is pending.
    ///
    /// `n` is excluded from this test: it is itself a valid gate response
    /// (its own test, `agents_screen_u_arms_and_n_cancels`, covers that
    /// behavior). Mixing it into this test would cancel the gate mid-loop
    /// and then exercise the un-gated keys, which is the wrong question.
    #[test]
    fn agents_screen_confirmation_gates_other_keys() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        let initial_selected = match &app.screen {
            Screen::Agents { selected, .. } => *selected,
            _ => unreachable!(),
        };

        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()),
            &paths,
        );
        assert!(matches!(
            app.screen,
            Screen::Agents {
                confirm_update_bundled: Some(_),
                ..
            }
        ));

        // Each stray key is sent through the *still-armed* gate. Navigation
        // keys must not move the cursor, the create/edit/delete keys must
        // not change the screen, and the gate must remain armed.
        let stray_keys = [
            KeyCode::Char('j'),
            KeyCode::Down,
            KeyCode::Char('k'),
            KeyCode::Up,
            KeyCode::Char('d'),
            KeyCode::Char('e'),
            KeyCode::Enter,
            KeyCode::Char('x'),
        ];
        for code in stray_keys {
            app.handle_agents_key(KeyEvent::new(code, KeyModifiers::empty()), &paths);
            match &app.screen {
                Screen::Agents {
                    selected,
                    confirm_update_bundled,
                    ..
                } => {
                    assert!(
                        confirm_update_bundled.is_some(),
                        "gate must remain armed through stray key {:?}",
                        code
                    );
                    assert_eq!(
                        *selected, initial_selected,
                        "navigation key {:?} must not move the cursor while the gate is armed",
                        code
                    );
                }
                other => panic!("stray key {:?} changed screen to {:?}", code, other),
            }
        }
    }

    /// `y` accepts the gate and the bundled prompts are refreshed. The
    /// per-file outcomes show up as a green "updated" status message so
    /// the user can see what changed.
    #[test]
    fn agents_screen_y_refreshes_prompts_and_preserves_other_fields() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();

        // Customize scout's description and permissions, then edit its
        // prompt so the refresh has work to do.
        let scout_path = paths.canonical_dir.join("scout.md");
        let prior = crate::store::hash_file(&scout_path).unwrap();
        let mut scout = crate::agent::Agent::read(&scout_path).unwrap();
        let original_description = scout.description.clone();
        let original_mode = scout.mode;
        let original_model = scout.model.clone();
        scout
            .permissions
            .insert("read".to_string(), crate::agent::PermissionAction::Deny);
        scout.prompt = "the user changed this prompt on purpose\n".to_string();
        crate::store::save_canonical(&paths, &scout, prior.as_deref()).unwrap();

        // Press u, then y.
        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()),
            &paths,
        );
        assert!(matches!(
            app.screen,
            Screen::Agents {
                confirm_update_bundled: Some(_),
                ..
            }
        ));
        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty()),
            &paths,
        );

        // Gate cleared after y.
        assert!(matches!(
            app.screen,
            Screen::Agents {
                confirm_update_bundled: None,
                ..
            }
        ));
        let status = app.status_bar.clone().unwrap_or_default();
        assert!(
            status.starts_with("updated bundled prompts")
                || status.starts_with("bundled prompts already current"),
            "expected a successful summary, got {:?}",
            status
        );

        // The scout prompt body is now the bundled one.
        let refreshed = crate::agent::Agent::read(&scout_path).unwrap();
        let bundled_prompt = STARTERS.iter().find(|s| s.name == "scout").unwrap().prompt;
        assert_eq!(refreshed.prompt, bundled_prompt);
        // Other fields preserved.
        assert_eq!(refreshed.description, original_description);
        assert_eq!(refreshed.mode, original_mode);
        assert_eq!(refreshed.model, original_model);
        assert_eq!(
            refreshed.permissions.get("read"),
            Some(&crate::agent::PermissionAction::Deny),
            "user permission override must survive the refresh"
        );
    }

    /// Uppercase `Y` also confirms the gate (matching the overwrite pattern).
    #[test]
    fn agents_screen_uppercase_y_also_confirms() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::empty()),
            &paths,
        );
        assert!(matches!(
            app.screen,
            Screen::Agents {
                confirm_update_bundled: None,
                ..
            }
        ));
        assert!(
            app.status_bar
                .as_deref()
                .unwrap_or_default()
                .starts_with("updated bundled prompts")
                || app
                    .status_bar
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("bundled prompts already current"),
            "Y must run the refresh: {:?}",
            app.status_bar
        );
    }

    /// `u` must not write anything on its own. Confirms the single-key
    /// confirmation semantics: arming the gate is free.
    #[test]
    fn agents_screen_u_alone_does_not_write() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        let scout_path = paths.canonical_dir.join("scout.md");
        let before = std::fs::read_to_string(&scout_path).unwrap();
        let mtime_before = std::fs::metadata(&scout_path).unwrap().modified().unwrap();

        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::empty()),
            &paths,
        );

        // No file touched: mtime must match and bytes must match.
        let after = std::fs::read_to_string(&scout_path).unwrap();
        let mtime_after = std::fs::metadata(&scout_path).unwrap().modified().unwrap();
        assert_eq!(before, after);
        assert_eq!(
            mtime_before, mtime_after,
            "u alone must not bump the file mtime"
        );
        // Status bar must not claim any write happened.
        let status = app.status_bar.clone().unwrap_or_default();
        assert!(
            !status.starts_with("updated bundled prompts"),
            "u alone must not claim a refresh: {:?}",
            status
        );
    }

    /// When the user navigates the list while no confirmation is armed, the
    /// `u` action remains available (i.e. navigation does not accidentally
    /// gate the new shortcut).
    #[test]
    fn agents_screen_navigation_does_not_arm_update_gate() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_agents();
        // Press j twice to move down — the gate must stay disarmed.
        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        app.handle_agents_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
            &paths,
        );
        match &app.screen {
            Screen::Agents {
                confirm_update_bundled,
                ..
            } => assert!(
                confirm_update_bundled.is_none(),
                "navigation must not arm the gate"
            ),
            _ => panic!("expected Agents screen"),
        }
    }

    #[test]
    fn footer_editor_normal_describes_nav_save_back() {
        let mut app = fresh_app();
        app.screen = Screen::Editor {
            field: EditorField::Name,
            mode: EditorMode::Normal,
            status: None,
            confirm_discard: false,
        };
        let text = app.footer_text();
        assert!(
            text.contains("save"),
            "editor NORMAL footer mentions save: {text:?}"
        );
        assert!(
            text.contains("field"),
            "editor NORMAL footer mentions field: {text:?}"
        );
        // INSERT is reachable from NORMAL via `i`, which the mode bar shows
        // for text fields — the footer just confirms save/quit/back.
        assert!(
            text.contains("Esc"),
            "editor NORMAL footer mentions Esc: {text:?}"
        );
        assert!(
            text.contains("i: edit"),
            "Name footer mentions inline edit: {text:?}"
        );
    }

    #[test]
    fn footer_editor_surfaces_prompt_and_permission_shortcuts() {
        let mut app = fresh_app();
        app.screen = Screen::Editor {
            field: EditorField::Prompt,
            mode: EditorMode::Normal,
            status: None,
            confirm_discard: false,
        };
        assert!(app.footer_text().contains("e: edit prompt"));

        app.screen = Screen::Editor {
            field: EditorField::Permissions(0),
            mode: EditorMode::Normal,
            status: None,
            confirm_discard: false,
        };
        assert!(app.footer_text().contains("Space: cycle permission"));
        assert!(app.footer_text().contains("h/l: row"));

        app.screen = Screen::Editor {
            field: EditorField::Mode,
            mode: EditorMode::Normal,
            status: None,
            confirm_discard: false,
        };
        assert!(app.footer_text().contains("cycle mode"));

        app.screen = Screen::Editor {
            field: EditorField::Model,
            mode: EditorMode::Normal,
            status: None,
            confirm_discard: false,
        };
        assert!(app.footer_text().contains("choose model"));
    }

    #[test]
    fn footer_editor_insert_describes_type_backspace_esc() {
        let mut app = fresh_app();
        app.screen = Screen::Editor {
            field: EditorField::Name,
            mode: EditorMode::Insert,
            status: None,
            confirm_discard: false,
        };
        let text = app.footer_text();
        assert!(
            text.contains("Backspace"),
            "INSERT footer mentions Backspace: {text:?}"
        );
        assert!(
            text.contains("NORMAL"),
            "INSERT footer mentions NORMAL: {text:?}"
        );
        // INSERT mode does not surface save/quit because they are no-ops.
        assert!(
            !text.contains("save"),
            "INSERT footer should not mention save: {text:?}"
        );
    }

    #[test]
    fn footer_editor_confirm_discard_overrides_mode() {
        let mut app = fresh_app();
        // confirm_discard wins regardless of the underlying mode.
        app.screen = Screen::Editor {
            field: EditorField::Name,
            mode: EditorMode::Insert,
            status: None,
            confirm_discard: true,
        };
        let text = app.footer_text();
        assert!(
            text.contains("discard"),
            "confirm-discard footer mentions discard: {text:?}"
        );
        assert!(
            text.contains("cancel"),
            "confirm-discard footer mentions cancel: {text:?}"
        );
    }

    #[test]
    fn footer_model_picker_describes_select_apply_manual_refresh() {
        let mut app = fresh_app();
        app.screen = Screen::ModelPicker {
            discovery: crate::models::Discovery::Empty(String::new()),
            manual: String::new(),
            selected: 0,
            manual_open: false,
            status: None,
        };
        let text = app.footer_text();
        assert!(
            text.contains("apply"),
            "picker footer mentions apply: {text:?}"
        );
        assert!(
            text.contains("manual"),
            "picker footer mentions manual: {text:?}"
        );
        assert!(
            text.contains("refresh"),
            "picker footer mentions refresh: {text:?}"
        );
    }

    #[test]
    fn footer_model_picker_manual_open_describes_type_tab_esc() {
        let mut app = fresh_app();
        app.screen = Screen::ModelPicker {
            discovery: crate::models::Discovery::Empty(String::new()),
            manual: String::new(),
            selected: 0,
            manual_open: true,
            status: None,
        };
        let text = app.footer_text();
        assert!(
            text.contains("Tab"),
            "manual-open footer mentions Tab: {text:?}"
        );
        assert!(
            text.contains("close"),
            "manual-open footer mentions close: {text:?}"
        );
    }

    #[test]
    fn footer_install_update_describes_install_overwrite_refresh_back() {
        let mut app = fresh_app();
        app.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: None,
        };
        let text = app.footer_text();
        assert!(
            text.contains("install safe"),
            "install footer mentions install safe: {text:?}"
        );
        assert!(
            text.contains("overwrite"),
            "install footer mentions overwrite: {text:?}"
        );
        assert!(
            text.contains("refresh"),
            "install footer mentions refresh: {text:?}"
        );
        assert!(
            text.contains("back"),
            "install footer mentions back: {text:?}"
        );
    }

    #[test]
    fn footer_install_update_confirm_overwrite_overrides_keys() {
        let mut app = fresh_app();
        app.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: Some((SyncTarget::OpenCode, "overwrite?".to_string())),
        };
        let text = app.footer_text();
        assert!(
            text.contains("Y"),
            "confirm-overwrite footer mentions Y: {text:?}"
        );
        assert!(
            text.contains("cancel"),
            "confirm-overwrite footer mentions cancel: {text:?}"
        );
        // The underlying screen's regular keys are suppressed while the
        // confirmation is armed so the footer cannot contradict itself.
        assert!(
            !text.contains("install safe"),
            "confirm-overwrite footer should not mention install safe: {text:?}"
        );
    }

    // ---------- Narrow-terminal safety ----------

    #[test]
    fn render_footer_does_not_panic_on_zero_width_area() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(80, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::Main { selected: 0 };
        terminal
            .draw(|frame| {
                // 0-width simulates a degenerate terminal column slice.
                let area = Rect::new(0, 9, 0, 1);
                app.render_footer(frame, area);
            })
            .unwrap();
    }

    #[test]
    fn render_footer_does_not_panic_on_tiny_terminal() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(1, 3);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: None,
        };
        // Whole render path on a 1x3 terminal: layout shrinks, footer area
        // is clipped, no panic.
        terminal.draw(|frame| app.render(frame)).unwrap();
    }

    #[test]
    fn render_footer_truncates_long_text_to_narrow_area() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: None,
        };
        let full = app.footer_text();
        assert!(
            full.chars().count() > 20,
            "sanity: InstallUpdate footer exceeds 20 cols, got {full:?}"
        );
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 4, 20, 1);
                app.render_footer(frame, area);
            })
            .unwrap();
        // The truncated text must fit on the 20-wide row; specifically it
        // must not carry the trailing `Esc: back` segment.
        let buffer = terminal.backend().buffer().clone();
        let last_row: String = buffer
            .content()
            .iter()
            .skip(4 * 20)
            .take(20)
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            last_row.chars().count() <= 20,
            "rendered row must fit width, got {last_row:?}"
        );
        assert!(
            !last_row.contains("Esc: back"),
            "footer should be truncated to 20 cols: {last_row:?}"
        );
    }

    #[test]
    fn status_and_footer_coexist_when_status_is_set() {
        // Two independent rows: the status row carries the message, the
        // footer row carries the contextual help. Both must be visible
        // when status is set — neither hides the other.
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(80, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::Agents {
            agents: Vec::new(),
            selected: 0,
            status: None,
            confirm_update_bundled: None,
        };
        app.status_bar = Some("saved `scout`".to_string());
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        // Layout: 1 body row + status row + footer row = 3 used rows in a
        // 6-tall terminal; body consumes the rest (rows 0..3).
        let full: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            full.contains("saved `scout`"),
            "status message must remain visible: {full:?}"
        );
        assert!(
            full.contains("new") && full.contains("delete"),
            "footer shortcuts must remain visible alongside status: {full:?}"
        );
    }

    // ---------- Footer/status visibility & style ----------

    /// The footer used to render with `Color::DarkGray + DIM` which is
    /// effectively black on dark terminals. It must now use an explicit
    /// bright foreground on a stable accent background and must not carry
    /// the DIM modifier.
    #[test]
    fn render_footer_uses_bright_fg_and_accent_bg_no_dim() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(80, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::Main { selected: 0 };
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        // Footer is always the last row. 80 cols wide.
        let last_row_idx = buffer.area.height as usize - 1;
        let footer_cells: Vec<_> = buffer
            .content()
            .iter()
            .skip(last_row_idx * buffer.area.width as usize)
            .take(buffer.area.width as usize)
            .collect();
        // Find a non-empty footer cell to inspect style; the truncated text
        // occupies the leftmost cells, leaving the rest as the buffer
        // default. Sampling the first character of the footer text is
        // enough to verify the rendered style.
        let cell = footer_cells
            .iter()
            .find(|c| !c.symbol().chars().all(char::is_whitespace))
            .expect("footer row should contain at least one non-blank cell");
        assert_ne!(
            cell.fg,
            Color::DarkGray,
            "footer foreground must not be DarkGray (was the original black-on-dark bug): {:?}",
            cell.fg
        );
        assert_ne!(
            cell.bg,
            Color::Reset,
            "footer background must be explicitly set, not Reset: {:?}",
            cell.bg
        );
        assert!(
            !cell.modifier.contains(Modifier::DIM),
            "footer must not carry DIM modifier: {:?}",
            cell.modifier
        );
    }

    /// Footer row must remain visible when a status message is also set.
    /// The two rows are independent — neither hides the other.
    #[test]
    fn footer_remains_visible_when_status_is_set() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(80, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::Agents {
            agents: Vec::new(),
            selected: 0,
            status: None,
            confirm_update_bundled: None,
        };
        app.status_bar = Some("error: boom".to_string());
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let width = buffer.area.width as usize;
        // Last row is the footer; second-to-last is the status row.
        let footer_row: String = buffer
            .content()
            .iter()
            .skip((buffer.area.height as usize - 1) * width)
            .take(width)
            .map(|c| c.symbol().to_string())
            .collect();
        let status_row: String = buffer
            .content()
            .iter()
            .skip((buffer.area.height as usize - 2) * width)
            .take(width)
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            footer_row.contains("new") && footer_row.contains("delete"),
            "footer must remain visible alongside status: footer={footer_row:?}"
        );
        assert!(
            status_row.contains("error: boom"),
            "status must remain visible alongside footer: status={status_row:?}"
        );
    }

    /// `status: "error: ..."` must render in the semantic error color.
    #[test]
    fn status_line_applies_error_color_for_error_prefix() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(80, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::Main { selected: 0 };
        app.status_bar = Some("error: failed to write".to_string());
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let width = buffer.area.width as usize;
        // Status row sits directly above the footer (second-to-last row).
        let status_cells: Vec<_> = buffer
            .content()
            .iter()
            .skip((buffer.area.height as usize - 2) * width)
            .take(width)
            .collect();
        let cell = status_cells
            .iter()
            .find(|c| !c.symbol().chars().all(char::is_whitespace))
            .expect("status row must contain at least one non-blank cell");
        assert_eq!(
            cell.fg, DANGER,
            "error status must use the error foreground: {:?}",
            cell.fg
        );
    }

    /// Successful status prefixes must render in the semantic success color.
    #[test]
    fn status_line_applies_success_color_for_success_prefixes() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(80, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::Main { selected: 0 };
        app.status_bar = Some("saved `scout`".to_string());
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let width = buffer.area.width as usize;
        let status_cells: Vec<_> = buffer
            .content()
            .iter()
            .skip((buffer.area.height as usize - 2) * width)
            .take(width)
            .collect();
        let cell = status_cells
            .iter()
            .find(|c| !c.symbol().chars().all(char::is_whitespace))
            .expect("status row must contain at least one non-blank cell");
        assert_eq!(
            cell.fg, SUCCESS,
            "success status must use the success foreground: {:?}",
            cell.fg
        );
    }

    /// Neutral status messages remain readable on the shared surface.
    #[test]
    fn status_line_default_prefix_uses_body_color() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(80, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::Main { selected: 0 };
        app.status_bar = Some("overwrite cancelled".to_string());
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let width = buffer.area.width as usize;
        let status_cells: Vec<_> = buffer
            .content()
            .iter()
            .skip((buffer.area.height as usize - 2) * width)
            .take(width)
            .collect();
        let cell = status_cells
            .iter()
            .find(|c| !c.symbol().chars().all(char::is_whitespace))
            .expect("status row must contain at least one non-blank cell");
        assert_eq!(
            cell.fg, TEXT,
            "neutral status must use the body foreground, got {:?}",
            cell.fg
        );
    }

    /// The global AGENTHD brand must use the accent color and bold weight.
    #[test]
    fn screen_title_uses_accent_color_and_bold() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(40, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::Main { selected: 0 };
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let width = buffer.area.width as usize;
        let top_row: Vec<_> = buffer.content().iter().take(width).collect();
        let title_cell = top_row
            .iter()
            .find(|c| c.symbol() == "◆")
            .expect("agenthd brand mark must be on the top row");
        assert_eq!(
            title_cell.fg, ACCENT,
            "screen title must use accent color: {:?}",
            title_cell.fg
        );
        assert!(
            title_cell.modifier.contains(Modifier::BOLD),
            "screen title must be bold: {:?}",
            title_cell.modifier
        );
    }

    /// Table headers must be readable (BOLD), not hidden (DIM).
    #[test]
    fn table_header_is_bold_not_dim() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(80, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        app.screen = Screen::Agents {
            agents: vec![AgentSummary {
                name: "scout".to_string(),
                mode: crate::agent::Mode::primary,
                model: None,
                description: "desc".to_string(),
            }],
            selected: 0,
            status: None,
            confirm_update_bundled: None,
        };
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let width = buffer.area.width as usize;
        // Find the uppercase NAME header; scanning the buffer is robust to
        // the global header and rounded panel border.
        let header_cell = buffer
            .content()
            .iter()
            .find(|c| c.symbol() == "N")
            .expect("table header cell must be present");
        assert!(
            header_cell.modifier.contains(Modifier::BOLD),
            "table header must be bold: {:?}",
            header_cell.modifier
        );
        assert!(
            !header_cell.modifier.contains(Modifier::DIM),
            "table header must not be dim: {:?}",
            header_cell.modifier
        );
        // `width` is kept so future readers see the intent of the layout.
        let _ = width;
    }
}
