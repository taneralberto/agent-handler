use crate::models::Discovery;
// Editor / picker types live in the `editor` child module. We import
// the variants the parent names in `Screen::Editor` and in the
// contextual footer match arms. Editor-only helpers and `EditorOp`
// are pulled in by `mod tests` directly so the lib build does not
// carry an unused-import warning.
use crate::store::{ApplyOutcome, Paths, PluginStatus, State, SyncItem, SyncTarget};
use crate::tools::ToolItem;
use editor::{AgentDraft, EditorField, EditorMode};
// `tools_lib` is only referenced from the test submodule below; gating the
// import on `#[cfg(test)]` keeps the production binary warning-free while
// preserving the natural short path inside the tests.
#[cfg(test)]
use crate::tools as tools_lib;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::{DefaultTerminal, Frame};
// `AgentSummary` lives in the `agents` child module. Re-import it here so
// the parent's `Screen::Agents` variant can name the type and so
// `editor.rs`'s `super::AgentSummary` reference (used by
// `open_editor_existing`) keeps resolving without a child->parent upcast.
use agents::AgentSummary;

/// Generic Tools list UI: render / open / refresh / handle / install and
/// the pure install-row helper. The implementation lives in `app/tools.rs`
/// as a child module; the screen state, main-menu entry, dispatch, and
/// footer all stay here.
mod tools;

/// Agents list UI: render / open / handle key / delete /
/// apply_update_bundled, and the bundled-update status-bar formatter.
/// The implementation lives in `app/agents.rs` as a child module; the
/// `Screen::Agents` variant, `pending_delete`, the main-menu entry, the
/// dispatch, and the contextual footer all stay here.
mod agents;

/// Install/Update screen: render / open / refresh / handle key /
/// safe-install / force-overwrite. The implementation lives in
/// `app/install_update.rs` as a child module; the `Screen::InstallUpdate`
/// variant, the main-menu entry, the dispatch, and the contextual
/// footer all stay here.
mod install_update;

/// Agent editor + model picker: render / open / handle / save / discard
/// paths, the field-navigation helpers, the external-prompt editor, and
/// the rename-then-save helper. The implementation lives in
/// `app/editor.rs` as a child module; the `Screen::Editor` and
/// `Screen::ModelPicker` variants, the editor fields on `App`, the
/// dispatch, and the contextual footer all stay here.
mod editor;

/// Subagent-panel (plugin) screen: render / open / refresh / handle key /
/// install / uninstall. The implementation lives in `app/plugin.rs` as a
/// child module; the `Screen::Plugin` variant, the main-menu entry, the
/// dispatch, and the contextual footer all stay here.
mod plugin;

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
    Tools,
    Plugin,
    Exit,
}

impl MainItem {
    fn all() -> &'static [MainItem] {
        &[
            MainItem::Agents,
            MainItem::InstallUpdate,
            MainItem::Tools,
            MainItem::Plugin,
            MainItem::Exit,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            MainItem::Agents => "Agents",
            MainItem::InstallUpdate => "Install/Update",
            MainItem::Tools => "Tools",
            MainItem::Plugin => "Subagent panel",
            MainItem::Exit => "Exit",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            MainItem::Agents => "Create and tune OpenCode roles",
            MainItem::InstallUpdate => "Review safe synchronization changes",
            MainItem::Tools => "Install bundled third-party OpenCode skills",
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
        /// Harness the session is bound to. `None` means the
        /// OpenCode/Pi selector is on screen; `Some(target)` means the
        /// per-file list for that target is on screen.
        target: Option<SyncTarget>,
    },
    Tools {
        entries: Vec<ToolItem>,
        selected: usize,
        status: Option<String>,
        /// `true` while the install is running; blocks key dispatch.
        installing: bool,
    },
    Plugin {
        status: PluginStatus,
        message: Option<String>,
        confirm_uninstall: bool,
    },
}

/// Two-press `d` delete state.
#[derive(Debug, Default)]
struct PendingDelete {
    name: Option<String>,
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
                target,
            } => self.render_install_update(
                frame,
                body,
                items,
                *selected,
                last_outcomes,
                status.as_deref(),
                confirm_overwrite.as_ref(),
                *target,
            ),
            Screen::Tools {
                entries,
                selected,
                status,
                installing,
            } => self.render_tools(
                frame,
                body,
                entries,
                *selected,
                status.as_deref(),
                *installing,
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
            Screen::Tools { .. } => "Tools",
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
                confirm_overwrite,
                target,
                ..
            } => {
                if confirm_overwrite.is_some() {
                    return "Y: overwrite · N / Esc: cancel".to_string();
                }
                match target {
                    None => {
                        "↑/↓ or j/k: pick harness · Enter: open · Esc: back".to_string()
                    }
                    Some(t) => format!(
                        "{} · ↑/↓ or j/k: select · i: install safe · o: overwrite conflict · r: refresh · Esc: harness",
                        t.label()
                    ),
                }
            }
            Screen::Tools { installing, .. } => {
                if *installing {
                    "installing (wait)…".to_string()
                } else {
                    "↑/↓ or j/k: select · i: install · r: refresh · Esc: back".to_string()
                }
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
            Screen::Tools { .. } => self.handle_tools_key(key),
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
                        MainItem::Tools => self.open_tools(),
                        MainItem::Plugin => self.open_plugin(),
                        MainItem::Exit => self.quit = true,
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
                _ => {}
            }
        }
    }
}

/// Shared visual helpers used by every screen: panel chrome, border,
/// title, selection, and status color styling.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::starter_agent;
    use crate::agent::STARTERS;
    // Editor-only helpers and the `EditorOp` enum live in the `editor`
    // child module; pull them in here so the existing editor / model
    // picker tests keep their direct call shapes.
    use crate::agent::{Mode, PermissionAction};
    use editor::{edit_text_field, next_field, prev_field, EditorOp};
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
            skills_dir: dir.path().join(".config").join("opencode").join("skills"),
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
        //
        // Note: `e` is intentionally excluded because it is a legitimate
        // NORMAL action on the Prompt field -- it opens the external
        // editor (`edit_prompt_in_system_editor` in this module), which
        // would block this headless test. The contract being verified
        // (NORMAL on Prompt must not call `edit_text_field`) is preserved:
        // those inert characters still prove no mutation happens.
        for c in "abcdf 0123 .,-".chars() {
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
                skills_dir: std::path::PathBuf::from(
                    "/tmp/agenthd-footer-test/.config/opencode/skills",
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
        // List view: the bound harness label appears up front and the
        // `Esc` shortcut takes the user back to the harness selector,
        // not the main menu.
        let mut app = fresh_app();
        app.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: None,
            target: Some(SyncTarget::OpenCode),
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
            text.contains("Esc"),
            "install footer mentions Esc: {text:?}"
        );
        assert!(
            text.contains("OpenCode"),
            "list footer must advertise the bound harness: {text:?}"
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
            target: Some(SyncTarget::OpenCode),
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

    #[test]
    fn main_menu_includes_tools_entry() {
        let labels: Vec<&str> = MainItem::all().iter().map(|m| m.label()).collect();
        assert!(
            labels.contains(&"Tools"),
            "main menu must include Tools entry: {:?}",
            labels
        );
        // Tools sits between Install/Update and the Subagent panel.
        let iu = labels.iter().position(|l| *l == "Install/Update").unwrap();
        let tools = labels.iter().position(|l| *l == "Tools").unwrap();
        let panel = labels.iter().position(|l| *l == "Subagent panel").unwrap();
        assert!(iu < tools && tools < panel, "Tools ordering: {:?}", labels);
    }

    #[test]
    fn footer_tools_describes_install_refresh_back() {
        let mut app = fresh_app();
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Build the Tools entries directly so we don't depend on disk state.
        let mut entries = Vec::new();
        for entry in tools_lib::DEFAULT_CATALOG {
            entries.push(tools_lib::tool_status(&paths, entry).unwrap_or_else(|_| {
                crate::tools::ToolItem {
                    entry,
                    status: crate::tools::ToolStatus::NotInstalled,
                    detail: String::new(),
                    destination: tools_lib::destination_for(&paths, entry),
                }
            }));
        }
        app.screen = Screen::Tools {
            entries,
            selected: 0,
            status: None,
            installing: false,
        };
        let text = app.footer_text();
        assert!(
            text.contains("install"),
            "tools footer mentions install: {text:?}"
        );
        assert!(
            text.contains("refresh"),
            "tools footer mentions refresh: {text:?}"
        );
        assert!(
            text.contains("back"),
            "tools footer mentions back: {text:?}"
        );
    }

    #[test]
    fn footer_tools_installing_suppresses_action_keys() {
        let mut app = fresh_app();
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entries: Vec<ToolItem> = tools_lib::DEFAULT_CATALOG
            .iter()
            .map(|entry| tools_lib::tool_status(&paths, entry).unwrap())
            .collect();
        app.screen = Screen::Tools {
            entries: entries.clone(),
            selected: 0,
            status: None,
            installing: true,
        };
        let text = app.footer_text();
        // While installing, the footer must not advertise the install
        // shortcut — the user cannot queue more work or race the spawn.
        assert!(
            !text.contains("install safe"),
            "installing footer must not advertise another install: {text:?}"
        );
        assert!(
            text.contains("installing"),
            "installing footer mentions state: {text:?}"
        );
    }

    #[test]
    fn render_tools_does_not_panic_on_tiny_terminal() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(1, 3);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = fresh_app();
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entries: Vec<ToolItem> = tools_lib::DEFAULT_CATALOG
            .iter()
            .map(|entry| tools_lib::tool_status(&paths, entry).unwrap())
            .collect();
        app.screen = Screen::Tools {
            entries,
            selected: 0,
            status: None,
            installing: false,
        };
        terminal.draw(|frame| app.render(frame)).unwrap();
    }

    #[test]
    fn open_tools_builds_entries_from_catalog() {
        let mut app = fresh_app();
        let dir = TempDir::new().unwrap();
        app.paths = setup_paths(&dir);
        app.open_tools();
        match &app.screen {
            Screen::Tools { entries, .. } => {
                assert_eq!(entries.len(), tools_lib::DEFAULT_CATALOG.len());
                for item in entries {
                    assert_eq!(
                        item.entry.skill_name, "pi-psql",
                        "the bundled catalog has one entry, pi-psql"
                    );
                    assert_eq!(item.status, crate::tools::ToolStatus::NotInstalled);
                }
            }
            other => panic!("expected Tools screen, got {:?}", other),
        }
    }

    #[test]
    fn handle_tools_key_i_on_installed_row_dispatches_install() {
        // Per TOOL_INSTALLER_PLAN.md, `i` must dispatch into the
        // installer regardless of the row's pre-install status. The
        // installer is the single source of truth: an existing target
        // dir/file/symlink is refused by the OS no-replace primitive
        // and reported as Conflict. The handler must therefore NOT
        // branch on `ToolStatus::Installed` and surface a UI no-op.
        //
        // We cannot exercise this through `App::handle_tools_key`
        // end-to-end here: that path calls `tools_lib::install_tool`,
        // which spawns real `git`, `node`, `npm`, and touches the
        // network. There is no injection seam for the installer in
        // `install_selected_tool` (it is a private method on `App`
        // that calls `tools_lib::install_tool` directly with the default
        // runners). Inventing one — a trait, a callback parameter, a
        // method override — solely for tests would violate the
        // "no extra interface" rule, so the smallest useful test
        // here pins the dispatch decision by calling the extracted
        // helper directly. The Conflict outcome itself is covered by
        // `tools_lib::install_tool_with` tests in `src/tools/tests.rs`,
        // which use the injected spawn / rename runners.
        let entry: &'static crate::tools::ToolCatalogEntry = &tools_lib::DEFAULT_CATALOG[0];
        let items = vec![ToolItem {
            entry,
            status: crate::tools::ToolStatus::Installed,
            detail: "installed".to_string(),
            destination: std::path::PathBuf::from("/tmp/agenthd-installed-target"),
        }];
        assert_eq!(
            App::tools_screen_install_target(&items, 0),
            Some(0),
            "i on an Installed row must dispatch into the install path",
        );
    }

    #[test]
    fn handle_tools_key_esc_returns_to_main() {
        let mut app = fresh_app();
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entries: Vec<ToolItem> = tools_lib::DEFAULT_CATALOG
            .iter()
            .map(|entry| tools_lib::tool_status(&paths, entry).unwrap())
            .collect();
        app.paths = paths;
        app.screen = Screen::Tools {
            entries,
            selected: 0,
            status: None,
            installing: false,
        };
        app.handle_tools_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Main { .. }));
    }

    #[test]
    fn handle_tools_key_blocks_input_during_install() {
        // The screen sets `installing = true` synchronously and the install
        // runs inside the event loop. Any key pressed while installing must
        // be a no-op so the user cannot queue more work.
        let mut app = fresh_app();
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entries: Vec<ToolItem> = tools_lib::DEFAULT_CATALOG
            .iter()
            .map(|entry| tools_lib::tool_status(&paths, entry).unwrap())
            .collect();
        app.paths = paths;
        app.screen = Screen::Tools {
            entries,
            selected: 0,
            status: None,
            installing: true,
        };
        app.handle_tools_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));
        assert!(matches!(
            app.screen,
            Screen::Tools {
                installing: true,
                ..
            }
        ));
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
            target: Some(SyncTarget::OpenCode),
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
            target: Some(SyncTarget::OpenCode),
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

    // ---------- Install/Update harness selection ----------

    /// Open the screen from main; the harness selector must be on
    /// screen with no items loaded.
    #[test]
    fn open_install_update_shows_selector() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        app.open_install_update();
        match &app.screen {
            Screen::InstallUpdate {
                items,
                target,
                selected,
                ..
            } => {
                assert!(
                    items.is_empty(),
                    "selector must not pre-load items: {items:?}"
                );
                assert!(
                    target.is_none(),
                    "selector must show when target is None: {target:?}"
                );
                assert_eq!(*selected, 0, "selector defaults to OpenCode (index 0)");
            }
            other => panic!("expected InstallUpdate screen, got {other:?}"),
        }
    }

    /// `j` / `Down` walk the selector; `k` / `Up` walk it back; bounds
    /// clamp at the edges so the user cannot scroll past the harness
    /// list.
    #[test]
    fn install_update_selector_jk_navigation() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        app.open_install_update();

        let press = |app: &mut App, code: KeyCode| {
            app.handle_install_update_key(KeyEvent::new(code, KeyModifiers::empty()));
        };
        // Helper returns (selected, target_is_none). Read each time to
        // avoid borrowing `app.screen` while a mutable borrow is held
        // by `press`.
        let snapshot = |app: &App| -> (usize, bool) {
            match &app.screen {
                Screen::InstallUpdate {
                    selected, target, ..
                } => (*selected, target.is_none()),
                _ => panic!("expected InstallUpdate screen: {:?}", app.screen),
            }
        };

        assert_eq!(snapshot(&app), (0, true));

        press(&mut app, KeyCode::Char('j'));
        assert_eq!(snapshot(&app).0, 1, "j moves down");

        press(&mut app, KeyCode::Char('j'));
        assert_eq!(
            snapshot(&app).0,
            1,
            "j clamps at the bottom (only 2 harnesses)"
        );

        press(&mut app, KeyCode::Down);
        assert_eq!(snapshot(&app).0, 1, "Down clamps too");

        press(&mut app, KeyCode::Char('k'));
        assert_eq!(snapshot(&app).0, 0, "k moves back up");

        press(&mut app, KeyCode::Up);
        assert_eq!(snapshot(&app).0, 0, "Up clamps at the top");

        // Other keys must be no-ops on the selector.
        press(&mut app, KeyCode::Char('i'));
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(snapshot(&app), (0, true));
    }

    /// Enter on the selector opens the list scoped to the chosen
    /// harness. The list only contains items for that target, never
    /// for the other.
    #[test]
    fn install_update_selector_enter_opens_scoped_list() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);
        app.open_install_update();

        // Pick OpenCode (default selection).
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        match &app.screen {
            Screen::InstallUpdate {
                target,
                items,
                selected,
                ..
            } => {
                assert_eq!(*target, Some(SyncTarget::OpenCode));
                assert!(!items.is_empty(), "fresh install must produce items");
                assert!(
                    items.iter().all(|i| i.target == SyncTarget::OpenCode),
                    "scoped list must only contain OpenCode items"
                );
                assert_eq!(*selected, 0);
            }
            other => panic!("expected InstallUpdate list, got {other:?}"),
        }

        // Back to selector, then pick Pi.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()));
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        match &app.screen {
            Screen::InstallUpdate { target, items, .. } => {
                assert_eq!(*target, Some(SyncTarget::Pi));
                assert!(
                    items.iter().all(|i| i.target == SyncTarget::Pi),
                    "scoped list must only contain Pi items, got {:?}",
                    items
                        .iter()
                        .map(|i| (i.target, &i.filename))
                        .collect::<Vec<_>>()
                );
            }
            other => panic!("expected InstallUpdate list for Pi, got {other:?}"),
        }
    }

    /// Esc on the selector returns to the main menu; Esc on the list
    /// returns to the selector (not the main menu).
    #[test]
    fn install_update_esc_walks_selector_then_list_then_main() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths, state);

        // Esc on selector -> main.
        app.open_install_update();
        assert!(matches!(
            app.screen,
            Screen::InstallUpdate { target: None, .. }
        ));
        app.handle_install_update_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Main { .. }));

        // Enter -> list, Esc -> selector (not main).
        app.open_install_update();
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(matches!(
            app.screen,
            Screen::InstallUpdate {
                target: Some(_),
                ..
            }
        ));
        app.handle_install_update_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        match &app.screen {
            Screen::InstallUpdate { target, items, .. } => {
                assert!(
                    target.is_none(),
                    "Esc on list must return to selector, not main"
                );
                assert!(items.is_empty(), "leaving the list clears items");
            }
            other => panic!("expected selector, got {other:?}"),
        }
        // A second Esc from the selector returns to main.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Main { .. }));
    }

    /// `Esc` on the confirm-overwrite popup must cancel the popup, not
    /// navigate the list underneath. The list bound to the chosen
    /// harness must remain visible after the cancel.
    #[test]
    fn install_update_esc_cancels_confirm_popup_then_lists_then_selector() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_install_update();
        // Pick OpenCode and install safe so the target file exists
        // before we mutate it into a conflict.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));
        let target_path = paths.target_dir.join("scout.md");
        assert!(target_path.exists(), "OpenCode scout.md must be installed");
        let before = std::fs::read_to_string(&target_path).unwrap();
        let mutated = before.replace("read-only", "tampered");
        std::fs::write(&target_path, &mutated).unwrap();
        app.refresh_install_update();

        // Find the scout row (it's a Conflict now) and arm `o`.
        let scout_idx = match &app.screen {
            Screen::InstallUpdate { items, .. } => items
                .iter()
                .position(|i| i.filename == "scout.md")
                .expect("scout row"),
            _ => unreachable!(),
        };
        for _ in 0..scout_idx {
            app.handle_install_update_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()));
        }
        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()));
        assert!(matches!(
            &app.screen,
            Screen::InstallUpdate {
                confirm_overwrite: Some(_),
                target: Some(SyncTarget::OpenCode),
                ..
            }
        ));

        // Esc cancels the popup only.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(
            &app.screen,
            Screen::InstallUpdate {
                confirm_overwrite: None,
                target: Some(SyncTarget::OpenCode),
                ..
            }
        ));
        assert!(
            app.status_bar
                .as_deref()
                .unwrap_or_default()
                .contains("cancelled"),
            "cancelled popup must surface a status: {:?}",
            app.status_bar
        );
        assert_eq!(
            std::fs::read_to_string(&target_path).unwrap(),
            mutated,
            "Esc on the popup must not overwrite the target"
        );

        // Esc again returns to the selector.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(
            app.screen,
            Screen::InstallUpdate { target: None, .. }
        ));

        // Esc a third time returns to main.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Main { .. }));
    }

    /// The footer text must describe the active sub-screen: the
    /// selector advertises pick/open/Esc, the list advertises
    /// install/overwrite/refresh/Esc with the bound harness label,
    /// and the confirm-overwrite popup overrides both.
    #[test]
    fn footer_install_update_selector_vs_list_vs_confirm() {
        let mut app = fresh_app();

        // Selector.
        app.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: None,
            target: None,
        };
        let selector_text = app.footer_text();
        assert!(
            selector_text.contains("pick harness") && selector_text.contains("Enter"),
            "selector footer: {selector_text:?}"
        );
        assert!(
            selector_text.contains("Esc"),
            "selector footer mentions Esc: {selector_text:?}"
        );
        assert!(
            !selector_text.contains("install safe"),
            "selector must not advertise install safe: {selector_text:?}"
        );

        // List bound to Pi.
        app.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: None,
            target: Some(SyncTarget::Pi),
        };
        let list_text = app.footer_text();
        assert!(
            list_text.contains("Pi"),
            "list footer shows bound target: {list_text:?}"
        );
        assert!(
            list_text.contains("install safe"),
            "list footer: {list_text:?}"
        );
        assert!(
            list_text.contains("overwrite"),
            "list footer: {list_text:?}"
        );
        assert!(list_text.contains("refresh"), "list footer: {list_text:?}");
        assert!(
            !list_text.contains("pick harness"),
            "list footer must not show selector shortcuts: {list_text:?}"
        );

        // Confirm-overwrite popup.
        app.screen = Screen::InstallUpdate {
            items: Vec::new(),
            selected: 0,
            last_outcomes: Vec::new(),
            status: None,
            confirm_overwrite: Some((SyncTarget::OpenCode, "scout.md".to_string())),
            target: Some(SyncTarget::OpenCode),
        };
        let confirm_text = app.footer_text();
        assert!(
            confirm_text.contains("Y"),
            "confirm footer: {confirm_text:?}"
        );
        assert!(
            confirm_text.contains("cancel"),
            "confirm footer: {confirm_text:?}"
        );
        assert!(
            !confirm_text.contains("install safe"),
            "confirm footer must not advertise install: {confirm_text:?}"
        );
    }

    /// `i` from the list installs safe actions only for the bound
    /// harness. Files in the other harness's directory must remain
    /// untouched and its manifest map must be byte-identical to the
    /// pre-install state.
    #[test]
    fn install_safe_only_targets_bound_harness() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_install_update();
        // Pick OpenCode.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));

        // Sentinel: write a known file to Pi's directory. After the
        // OpenCode-only install, that file must be untouched.
        let pi_sentinel = paths.pi_target_dir.join("sentinel.md");
        let pi_sentinel_body = "pi-only sentinel\n";
        std::fs::write(&pi_sentinel, pi_sentinel_body).unwrap();

        // Capture Pi's ownership map before the install so we can
        // verify it stays unchanged. The OpenCode map is expected to
        // grow, so we do not compare the whole manifest.
        let state_pre = State::load(&paths.state_file).unwrap();
        let pre_pi_installed = state_pre.pi_installed.clone();

        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));

        // OpenCode is installed.
        assert!(
            paths.target_dir.join("scout.md").exists(),
            "scout must be installed"
        );
        // Pi's directory must NOT have new agenthd-owned files
        // (starters seeded on the canonical side will appear in plan
        // for Pi but apply_safe was never asked to plan Pi).
        assert!(
            !paths.pi_target_dir.join("scout.md").exists(),
            "scout.md must not appear in Pi's directory after OpenCode-only install"
        );
        // The sentinel is preserved verbatim.
        assert_eq!(
            std::fs::read_to_string(&pi_sentinel).unwrap(),
            pi_sentinel_body
        );

        // Pi's ownership map is unchanged — same keys, same hashes,
        // and no new entries for OpenCode's files sneaking in. The
        // equality check above is strictly stronger than any
        // per-key cross-map loop: it proves the Pi map was not touched
        // at all, which already rules out Pi gaining an OpenCode
        // file. (See `install_safe_pi_only_does_not_touch_opencode`
        // for the symmetric direction with a planted ghost.)
        let state_post = State::load(&paths.state_file).unwrap();
        assert_eq!(
            state_post.pi_installed, pre_pi_installed,
            "Pi ownership map must be unchanged after OpenCode-only install"
        );
    }

    /// Mirror regression for the reverse direction: a Pi-bound `i` must
    /// install Pi only and leave OpenCode's directory and ownership
    /// map untouched. The planted ghost exercises the
    /// `apply_safe` cleanup pass: a buggy per-target cleanup that
    /// walked every `SyncTarget` would prune this OpenCode-side
    /// entry because the file is missing from both the canonical dir
    /// and the OpenCode target dir.
    #[test]
    fn install_safe_pi_only_does_not_touch_opencode() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);

        app.open_install_update();
        // Pick Pi (down once from OpenCode default).
        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()));
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        match &app.screen {
            Screen::InstallUpdate { target, .. } => {
                assert_eq!(*target, Some(SyncTarget::Pi));
            }
            other => panic!("expected InstallUpdate list for Pi, got {other:?}"),
        }

        // Sentinel in OpenCode's target dir — must be preserved
        // verbatim after the Pi-only install.
        let oc_sentinel = paths.target_dir.join("sentinel.md");
        let oc_sentinel_body = "opencode-only sentinel\n";
        std::fs::write(&oc_sentinel, oc_sentinel_body).unwrap();

        // Ghost ownership entry in the OpenCode map. After a full
        // install, `apply_safe`'s cleanup would prune this because
        // `ghost-oc.md` is absent from both the canonical dir and the
        // OpenCode target dir. The Pi-only install must leave it
        // alone because OpenCode is not in `items` and the cleanup
        // pass is scoped to targets present in `items`.
        const GHOST_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
        let mut state_pre = State::load(&paths.state_file).unwrap();
        state_pre
            .installed
            .insert("ghost-oc.md".to_string(), GHOST_HASH.to_string());
        let pre_installed = state_pre.installed.clone();
        std::fs::write(
            &paths.state_file,
            serde_json::to_vec_pretty(&state_pre).unwrap(),
        )
        .unwrap();

        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));

        // Pi was installed.
        let pi_scout = paths.pi_target_dir.join("scout.md");
        assert!(pi_scout.exists(), "Pi scout must be installed");
        let pi_scout_bytes = std::fs::read_to_string(&pi_scout).unwrap();
        assert!(
            pi_scout_bytes.contains("name: scout"),
            "Pi scout must be rendered in Pi format: {pi_scout_bytes}"
        );

        // OpenCode was NOT installed: no scout.md in OpenCode's dir.
        assert!(
            !paths.target_dir.join("scout.md").exists(),
            "OpenCode scout.md must not appear after Pi-only install"
        );
        // Sentinel is preserved verbatim.
        assert_eq!(
            std::fs::read_to_string(&oc_sentinel).unwrap(),
            oc_sentinel_body,
            "OpenCode sentinel must be preserved"
        );

        // OpenCode ownership map retained: the ghost is still there
        // with the same hash, and the whole map is byte-identical to
        // the pre-install state (no entries added, removed, or
        // rewritten by the Pi install).
        let state_post = State::load(&paths.state_file).unwrap();
        assert_eq!(
            state_post.installed.get("ghost-oc.md").map(String::as_str),
            Some(GHOST_HASH),
            "OpenCode ghost entry must survive a Pi-only install"
        );
        assert_eq!(
            state_post.installed, pre_installed,
            "OpenCode ownership map must be unchanged after Pi-only install"
        );
        // The Pi install only writes its own entries; the OpenCode
        // map's only key must not collide with any Pi map key (the
        // ghost is by construction absent from Pi's map).
        for name in state_post.installed.keys() {
            assert!(
                !state_post.pi_installed.contains_key(name),
                "OpenCode map must not gain a Pi file: {name}"
            );
        }
    }

    /// `o` on a selected conflict overwrites only the bound harness's
    /// target. The other harness's directory and manifest stay untouched.
    #[test]
    fn install_overwrite_only_targets_bound_harness() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        // Seed a baseline where both targets are installed and the
        // manifest reflects that ownership. `apply_safe` already
        // persists `state` to disk via `write_state`, so the manual
        // re-serialize that older revisions of this test carried is
        // redundant.
        let full_state = crate::store::State::load(&paths.state_file).unwrap();
        let full_plan = crate::store::compute_plan(&paths, &full_state).unwrap();
        let (_full_state, _) = crate::store::apply_safe(&paths, full_state, full_plan).unwrap();

        app.open_install_update();
        // Pick Pi.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()));
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        match &app.screen {
            Screen::InstallUpdate { target, .. } => assert_eq!(*target, Some(SyncTarget::Pi)),
            _ => panic!(),
        }

        // Mutate Pi's scout target so it is a Conflict.
        let pi_target = paths.pi_target_dir.join("scout.md");
        let original = std::fs::read_to_string(&pi_target).unwrap();
        let mutated = original.replace("read-only", "tampered");
        std::fs::write(&pi_target, &mutated).unwrap();
        // Also mutate OpenCode's scout target so we can verify it is
        // not overwritten by the Pi-only force_install.
        let oc_target = paths.target_dir.join("scout.md");
        let oc_original = std::fs::read_to_string(&oc_target).unwrap();
        let oc_mutated = oc_original.replace("read-only", "tampered-oc");
        std::fs::write(&oc_target, &oc_mutated).unwrap();

        // Capture pre-state.
        let pre_oc_target_bytes = std::fs::read_to_string(&oc_target).unwrap();
        let pre_state_bytes = std::fs::read_to_string(&paths.state_file).unwrap();

        app.refresh_install_update();
        let scout_idx = match &app.screen {
            Screen::InstallUpdate { items, .. } => items
                .iter()
                .position(|i| i.filename == "scout.md")
                .expect("scout row"),
            _ => unreachable!(),
        };
        for _ in 0..scout_idx {
            app.handle_install_update_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()));
        }
        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()));
        // Confirm overwrite.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty()));

        // Pi's scout is back to canonical.
        let pi_after = std::fs::read_to_string(&pi_target).unwrap();
        assert!(pi_after.contains("read-only"));
        // OpenCode's scout must NOT be rewritten — Pi's force_install
        // never touched it.
        assert_eq!(
            std::fs::read_to_string(&oc_target).unwrap(),
            pre_oc_target_bytes
        );
        // The OpenCode ownership entry's hash is the one we wrote
        // before the conflict; it is not bumped by the Pi install.
        let post_state: State =
            serde_json::from_str(&std::fs::read_to_string(&paths.state_file).unwrap()).unwrap();
        let post_oc_hash = post_state.installed.get("scout.md").cloned();
        // Manifest was rewritten by the Pi install (state.installed
        // for Pi got an update); the bytes won't be byte-equal, but
        // OpenCode's hash for scout must still equal what we wrote
        // before the test (the original canonical hash, since the
        // OpenCode target was never overwritten).
        // Verify OpenCode's hash equals what we wrote before.
        let pre_state: State = serde_json::from_str(&pre_state_bytes).unwrap();
        let pre_oc_hash = pre_state.installed.get("scout.md").cloned();
        assert_eq!(
            post_oc_hash, pre_oc_hash,
            "OpenCode ownership hash must be unchanged"
        );
    }

    /// `r` refreshes only the bound harness; the other harness's
    /// manifest map is byte-identical to before the refresh.
    #[test]
    fn install_refresh_only_targets_bound_harness() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        // First install everything via the store helper.
        let full_state = crate::store::State::load(&paths.state_file).unwrap();
        let full_plan = crate::store::compute_plan(&paths, &full_state).unwrap();
        let (full_state, _) = crate::store::apply_safe(&paths, full_state, full_plan).unwrap();
        std::fs::write(
            &paths.state_file,
            serde_json::to_vec_pretty(&full_state).unwrap(),
        )
        .unwrap();

        let pre_state_bytes = std::fs::read(&paths.state_file).unwrap();

        app.open_install_update();
        // Pick OpenCode.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        // External edit on the OpenCode target so refresh sees an
        // UpdateAvailable for OpenCode only.
        let oc_target = paths.target_dir.join("scout.md");
        let oc_before = std::fs::read_to_string(&oc_target).unwrap();
        std::fs::write(&oc_target, oc_before.replace("read-only", "tampered-oc")).unwrap();

        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::empty()));

        // The Pi ownership map in the manifest is byte-identical to
        // before the refresh — the OpenCode-scoped refresh must not
        // touch Pi's entries at all.
        let pre: State = serde_json::from_slice(&pre_state_bytes).unwrap();
        let post: State =
            serde_json::from_slice(&std::fs::read(&paths.state_file).unwrap()).unwrap();
        assert_eq!(
            pre.pi_installed, post.pi_installed,
            "Pi manifest entries must be untouched"
        );
    }

    /// Empty plan: when the chosen harness has no canonical and no
    /// targets, `i` is a no-op and a clear message is shown.
    #[test]
    fn install_safe_with_empty_plan_reports_and_does_not_write() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Seed no canonical — both target dirs are empty too.
        let mut app = App::new(paths.clone(), State::default());
        app.open_install_update();
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        // items is empty.
        match &app.screen {
            Screen::InstallUpdate { items, target, .. } => {
                assert_eq!(*target, Some(SyncTarget::OpenCode));
                assert!(items.is_empty());
            }
            _ => panic!("expected InstallUpdate list"),
        }

        // No state file should exist yet — but if it does, capture
        // its bytes so we can prove the empty install is a no-op.
        let pre_bytes = std::fs::read(&paths.state_file).ok();

        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));

        // Status surfaces the no-op message.
        assert!(
            app.status_bar
                .as_deref()
                .unwrap_or_default()
                .contains("nothing to install"),
            "empty plan must surface a clear message: {:?}",
            app.status_bar
        );
        // State file is untouched (either still absent or unchanged).
        let post_bytes = std::fs::read(&paths.state_file).ok();
        assert_eq!(
            pre_bytes, post_bytes,
            "empty plan must not touch the manifest"
        );
    }

    /// `o` on a non-conflict row is a no-op that surfaces a status
    /// message; it does not arm the popup or write anything.
    #[test]
    fn install_o_on_non_conflict_only_reports() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_install_update();
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));

        // Fresh install -> scout.md is NotInstalled, not a conflict.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()));
        match &app.screen {
            Screen::InstallUpdate {
                confirm_overwrite,
                status,
                ..
            } => {
                assert!(
                    confirm_overwrite.is_none(),
                    "non-conflict `o` must not arm the overwrite popup"
                );
                assert!(
                    status.is_none(),
                    "non-conflict `o` must not leave a screen popup behind: {:?}",
                    status
                );
            }
            other => panic!("expected InstallUpdate screen, got {other:?}"),
        }
        assert!(
            app.status_bar
                .as_deref()
                .unwrap_or_default()
                .contains("is not a conflict"),
            "non-conflict `o` must report: {:?}",
            app.status_bar
        );

        // After a j/k movement the notice should not re-appear as a
        // popup. (The screen.status was the original bug: writing it
        // here meant the message stuck across navigation.)
        app.handle_install_update_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()));
        match &app.screen {
            Screen::InstallUpdate { status, .. } => assert!(
                status.is_none(),
                "j must not surface a leftover popup: {status:?}"
            ),
            other => panic!("expected InstallUpdate screen, got {other:?}"),
        }
    }

    /// `force_overwrite` must refuse to write a target that is not the
    /// bound harness. This is the cross-harness guard the
    /// Install/Update session relies on.
    #[test]
    fn install_force_overwrite_refuses_cross_target() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let mut app = App::new(paths.clone(), state);
        app.open_install_update();
        // Bound to OpenCode.
        app.handle_install_update_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));

        // Try to force-overwrite Pi. The handler must refuse.
        app.force_overwrite(SyncTarget::Pi, "scout.md");
        assert!(
            app.status_bar
                .as_deref()
                .unwrap_or_default()
                .contains("overwrite refused"),
            "cross-target force must be refused: {:?}",
            app.status_bar
        );
        // Pi's target directory must be untouched.
        assert!(
            !paths.pi_target_dir.join("scout.md").exists(),
            "Pi scout.md must not be written by an OpenCode-bound session"
        );
    }

    /// `apply_safe` must prune stale ownership entries for the
    /// targets present in `items`, but must NOT prune entries for
    /// targets that did not appear in `items`.
    #[test]
    fn apply_safe_cleanup_does_not_touch_other_target_map() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Build a state where Pi has a stale ownership entry for a
        // file that exists on neither side.
        let mut state = State::default();
        state
            .pi_installed
            .insert("ghost-pi.md".to_string(), "abc".to_string());
        state
            .installed
            .insert("ghost-oc.md".to_string(), "def".to_string());

        // Seed canonical so OpenCode has at least one safe item.
        let (_, state) = crate::store::seed_starters(&paths, state).unwrap();

        // Plan OpenCode only.
        let oc_plan = crate::store::plan_for(&paths, &state, SyncTarget::OpenCode).unwrap();
        assert!(!oc_plan.is_empty());

        let (post_state, _) = crate::store::apply_safe(&paths, state, oc_plan).unwrap();

        // OpenCode's stale entry was cleaned up because OpenCode was
        // in items.
        assert!(
            !post_state.installed.contains_key("ghost-oc.md"),
            "OpenCode stale entry should be cleaned up"
        );
        // Pi's stale entry is untouched because Pi was NOT in items.
        assert!(
            post_state.pi_installed.contains_key("ghost-pi.md"),
            "Pi stale entry must NOT be cleaned up when items is OpenCode-only"
        );
    }

    /// Single-target planning: `plan_for` returns only items for the
    /// requested target, never the other one.
    #[test]
    fn plan_for_returns_only_target_items() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = crate::store::seed_starters(&paths, State::default()).unwrap();
        let oc = crate::store::plan_for(&paths, &state, SyncTarget::OpenCode).unwrap();
        assert!(oc.iter().all(|i| i.target == SyncTarget::OpenCode));
        let pi = crate::store::plan_for(&paths, &state, SyncTarget::Pi).unwrap();
        assert!(pi.iter().all(|i| i.target == SyncTarget::Pi));
        // Combined view is the sum of the two.
        let both = crate::store::compute_plan(&paths, &state).unwrap();
        assert_eq!(both.len(), oc.len() + pi.len());
    }
}
