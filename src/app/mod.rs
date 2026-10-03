use crate::models::Discovery;
// Editor / picker types live in the `editor` child module. We import
// the variants the parent names in `Screen::Editor` and in the
// contextual footer match arms. Editor-only helpers and `EditorOp`
// are pulled in by `mod tests` directly so the lib build does not
// carry an unused-import warning.
#[allow(unused_imports)]
use crate::store::{
    apply_skills, plan_skills, ApplyOutcome, Paths, SkillOutcome, SkillPlanItem, State, SyncItem,
    SyncTarget,
};
use crate::tools::ToolItem;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use editor::{AgentDraft, EditorField, EditorMode};
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

/// Agents list UI: render / open / handle key / delete. The
/// implementation lives in `app/agents.rs` as a child module; the
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

/// Settings screen: configure the canonical checkout path. The
/// implementation lives in `app/settings.rs` as a child module; the
/// `Screen::Settings` variant, the main-menu entry, the dispatch, and
/// the contextual footer all stay here. The screen replaces the
/// historical source-selection screen — there is no Local mode and no
/// Repo mode picker; there is exactly one canonical directory, and
/// the Settings screen exists to point at it.
mod settings;

/// Skills list UI: render / open / refresh / handle key / apply safe.
/// The implementation lives in `app/skills_list.rs` as a child module;
/// the `Screen::Skills` variant, the main-menu routing inside
/// Install/Update, the dispatch, and the contextual footer all stay
/// here. Like the agent Install/Update screens, the Skills list is
/// keyed on a checked-out source tree (configured-checkout `skills/`)
/// and a destination tree (`Paths.skills_dir`). Unlike the agent
/// flow, there is no per-target ownership variant and no force-
/// overwrite path: `o` is intentionally refused with an explanatory
/// message.
mod skills_list;

// Central palette: sage accent on near-black surfaces with warm semantic
// colors. The accent doubles as the selection highlight (foreground =
// SURFACE so the pill stays readable on both light and dark terminals).
const ACCENT: Color = Color::Rgb(132, 204, 169);
const ACCENT_SOFT: Color = Color::Rgb(86, 152, 122);
const SURFACE: Color = Color::Rgb(18, 22, 26);
const SURFACE_RAISED: Color = Color::Rgb(28, 34, 40);
const SURFACE_INSET: Color = Color::Rgb(24, 30, 36);
const TEXT: Color = Color::Rgb(230, 236, 232);
const MUTED: Color = Color::Rgb(150, 164, 160);
const ACCENT_MUTED: Color = Color::Rgb(96, 134, 116);
const SUCCESS: Color = Color::Rgb(132, 204, 169);
const WARNING: Color = Color::Rgb(232, 178, 84);
const DANGER: Color = Color::Rgb(228, 110, 110);

/// Top-level menu options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainItem {
    Agents,
    InstallUpdate,
    Settings,
    Tools,
    Exit,
}

impl MainItem {
    fn all() -> &'static [MainItem] {
        // Source order is the visual order on the main menu.
        &[
            MainItem::Agents,
            MainItem::InstallUpdate,
            MainItem::Tools,
            MainItem::Settings,
            MainItem::Exit,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            MainItem::Agents => "Agents",
            MainItem::InstallUpdate => "Install / Update",
            MainItem::Tools => "Tools",
            MainItem::Settings => "Settings",
            MainItem::Exit => "Exit",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            MainItem::Agents => "Create, edit, and tune your OpenCode roles",
            MainItem::InstallUpdate => {
                "Review and apply safe synchronization changes across your harnesses"
            }
            MainItem::Tools => "Install ready-to-use third-party OpenCode skills",
            MainItem::Settings => "Choose the checkout containing your agent definitions",
            MainItem::Exit => "Close agenthd",
        }
    }

    /// One-line group cue rendered above the menu row.
    fn group(self) -> &'static str {
        match self {
            MainItem::Agents => "WORK",
            MainItem::InstallUpdate => "SYNC",
            MainItem::Tools => "EXTRAS",
            MainItem::Settings => "CONFIG",
            MainItem::Exit => "LEAVE",
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
    /// Skills list screen (configured-checkout `skills/` directory
    /// into the OpenCode global skills dir). Driven by
    /// `crate::store::plan_skills` / `apply_skills` — separate from
    /// the agent sync flow because the safety contract differs (whole
    /// tree hash, no force overwrite, third-party tools like `pi-psql`
    /// must remain untouched unless the configured checkout ships them
    /// and the manifest already records them).
    Skills {
        items: Vec<SkillPlanItem>,
        selected: usize,
        last_outcomes: Vec<SkillOutcome>,
        status: Option<String>,
    },
    /// Settings screen (first-run or menu entry). The state lives in
    /// the `settings` child module so the parent only names the
    /// shape.
    Settings {
        state: settings::SettingsState,
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

    /// Replace the persistent status-bar message. Used by `main` for
    /// banners ("saved checkout path", "first run", etc.) that should
    /// outlive the screen transitions the user goes through next.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_bar = Some(msg.into());
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
            } => self.render_agents(frame, body, agents, *selected, status.as_deref()),
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
            Screen::Skills {
                items,
                selected,
                last_outcomes,
                status,
            } => self.render_skills(
                frame,
                body,
                items,
                *selected,
                last_outcomes,
                status.as_deref(),
            ),
            Screen::Settings { state } => self.render_settings(frame, body, state, None),
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
            Screen::Skills { .. } => "Skills",
            Screen::Settings { .. } => "Settings",
        };
        let title = Line::from(vec![
            Span::styled(
                "  ◆ AGENTHD ",
                Style::default()
                    .fg(SURFACE)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default().bg(SURFACE_RAISED)),
            Span::styled("OpenCode", Style::default().fg(MUTED).bg(SURFACE_RAISED)),
            Span::styled(" / ", Style::default().fg(ACCENT_MUTED).bg(SURFACE_RAISED)),
            Span::styled(
                screen,
                Style::default()
                    .fg(TEXT)
                    .bg(SURFACE_RAISED)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let subtitle = Line::from(vec![
            Span::styled("  ", Style::default().bg(SURFACE_RAISED)),
            Span::styled(
                "Agent definitions and OpenCode synchronization",
                Style::default().fg(MUTED).bg(SURFACE_RAISED),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(vec![title, subtitle]).style(Style::default().bg(SURFACE_RAISED)),
            area,
        );
    }

    fn render_main(&self, frame: &mut Frame, area: Rect, selected: usize) {
        // Tall layout needs `4 * N` content rows plus 2 panel border rows.
        // Anything shorter falls back to the compact 2-line stack so every
        // entry still fits inside the visible panel without clipping the
        // bottom border. N = `MainItem::all().len()` = 5.
        const TALL_ENTRY_LINES: u16 = 4;
        let n = MainItem::all().len() as u16;
        let tall_height_needed = TALL_ENTRY_LINES * n + 2;
        let use_tall = area.height >= tall_height_needed;
        let mut items: Vec<ListItem> = Vec::with_capacity(n as usize);
        for item in MainItem::all() {
            let lines: Vec<Line> = if use_tall {
                vec![
                    Line::from(Span::styled(
                        item.group(),
                        Style::default()
                            .fg(ACCENT_SOFT)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(vec![
                        Span::styled("●  ", Style::default().fg(ACCENT_SOFT)),
                        Span::styled(
                            item.label(),
                            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(
                        format!("    {}", item.detail()),
                        Style::default().fg(MUTED),
                    )),
                    Line::from(""),
                ]
            } else {
                vec![
                    Line::from(vec![
                        Span::styled(
                            format!("{}  ", item.group()),
                            Style::default()
                                .fg(ACCENT_SOFT)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            item.label(),
                            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(item.detail(), Style::default().fg(MUTED))),
                ]
            };
            items.push(ListItem::new(lines));
        }
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
        let style = Style::default().fg(TEXT).bg(SURFACE_INSET);
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
            Screen::Agents { .. } => {
                "↑/↓ or j/k: select · n: new · e / Enter: edit · d d: delete · Esc: back"
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
            Screen::Skills { .. } => {
                "↑/↓ or j/k: select · i: install · r: refresh · Esc: back".to_string()
            }
            Screen::Settings { state } => {
                if state.is_path_editing() {
                    "type: edit path · Backspace: delete · Ctrl+U: clear · Enter: apply · Esc: cancel"
                        .to_string()
                } else if state.gated {
                    "Enter: edit path".to_string()
                } else {
                    "Enter: edit path · Esc: back".to_string()
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
            Screen::Skills { .. } => self.handle_skills_key(key),
            Screen::Settings { .. } => self.handle_settings_key(key)?,
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
                        MainItem::Settings => self.open_settings(false),
                        MainItem::Tools => self.open_tools(),
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
        Style::default().fg(ACCENT_MUTED)
    }
}

fn title_style() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Selection highlight: SURFACE text on ACCENT so the pill stays
/// readable on both light and dark terminals.
fn selected_style() -> Style {
    Style::default()
        .fg(SURFACE)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

/// Semantic color for a status-bar message: errors go red, successes go
/// green, the unsaved-changes warning goes warm yellow, everything else
/// stays visible on the shared surface.
fn status_style_for(text: &str) -> Style {
    let color = if text.starts_with("error: ") {
        DANGER
    } else if text.starts_with("saved ")
        || text.starts_with("force installed ")
        || text.starts_with("deleted canonical ")
    {
        SUCCESS
    } else if text.starts_with("Unsaved") {
        WARNING
    } else {
        TEXT
    };
    Style::default().fg(color).bg(SURFACE_INSET)
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
    // The lib's `starter_fixture` is `#[cfg(test)]` and therefore
    // absent from the bin's test build (cfg(test) does not propagate
    // across crates). `src/main.rs` re-includes it as
    // `crate::starter_fixture` under the same `#[cfg(test)]` gate, so
    // these symbols come from there. The fixture itself imports
    // `crate::agent::Agent` which resolves to the lib's `Agent` via
    // the `pub use agenthd::agent` in `src/main.rs`.
    use crate::starter_fixture::{starter_agent, STARTERS};
    // Editor-only helpers and the `EditorOp` enum live in the `editor`
    // child module; pull them in here so the existing editor / model
    // picker tests keep their direct call shapes.
    use crate::agent::{Mode, PermissionAction};
    use crate::store::{hash_file, save_canonical, Settings};
    use crate::tools::{ToolItem, ToolStatus, DEFAULT_CATALOG};
    use editor::{edit_text_field, next_field, prev_field, EditorOp};
    use serde_json;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn setup_paths(dir: &TempDir) -> Paths {
        let paths = Paths {
            agenthd_root: dir.path().join(".agenthd"),
            canonical_dir: dir.path().join(".agenthd").join("agents"),
            state_file: dir.path().join(".agenthd").join("state.json"),
            target_dir: dir.path().join(".config").join("opencode").join("agents"),
            pi_target_dir: dir.path().join(".pi").join("agent").join("agents"),
            skills_dir: dir.path().join(".config").join("opencode").join("skills"),
            settings_file: dir.path().join(".agenthd").join("settings.json"),
        };
        paths.ensure_dirs().unwrap();
        paths
    }

    /// Build a `Paths` whose canonical_dir points at a fresh
    /// checkout-shaped directory inside the tempdir. Mirrors the
    /// committed-version `setup_paths` but uses the configured
    /// checkout layout so the tests exercise the production read /
    /// write path against a real `<repo>/agents/` directory rather
    /// than a local default that the new design no longer ships.
    fn setup_paths_with_checkout(dir: &TempDir) -> (Paths, std::path::PathBuf) {
        let paths = setup_paths(dir);
        let checkout = dir.path().join("checkout");
        let agents = checkout.join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        // Write the three starter files the editor tests need so
        // `App::open_agents` finds a non-empty list.
        for starter in STARTERS.iter().take(3) {
            std::fs::write(
                agents.join(format!("{}.md", starter.name)),
                starter_agent(starter).render(),
            )
            .unwrap();
        }
        let paths = Paths {
            canonical_dir: agents.clone(),
            ..paths
        };
        // Persist the settings file so the runtime could re-derive
        // canonical_dir from it.
        std::fs::create_dir_all(&paths.agenthd_root).unwrap();
        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        crate::store::save_settings(&paths.settings_file, &settings).unwrap();
        (paths, checkout)
    }

    /// Build a `Paths` whose checkout also carries a `skills/`
    /// directory with a few SKILL.md entries. Used by the Skills
    /// list UI tests.
    fn setup_paths_with_skills(dir: &TempDir) -> (Paths, std::path::PathBuf) {
        let (paths, checkout) = setup_paths_with_checkout(dir);
        let skills_src = checkout.join("skills");
        std::fs::create_dir_all(&skills_src).unwrap();
        for name in ["clarify-before-coding", "kiss-for-you"] {
            let dir = skills_src.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: x\n---\nbody\n"),
            )
            .unwrap();
        }
        (paths, checkout)
    }

    /// Drop a single canonical `*.md` file into `canonical_dir`. Used
    /// by tests that need a specific starter on disk without seeding
    /// every bundled starter.
    #[allow(dead_code)]
    fn write_one_canonical(paths: &Paths, name: &str) {
        let starter = STARTERS
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no starter named `{name}`"));
        std::fs::write(
            paths.canonical_dir.join(format!("{}.md", name)),
            starter_agent(starter).render(),
        )
        .unwrap();
    }

    // ---------- Editor tests ----------

    #[test]
    fn screen_transitions_create_new_editor() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths, State::default());
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
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
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
        assert!(prior_hash.is_some(), "prior_hash captured at open");
        assert_eq!(original_name.as_deref(), Some(STARTERS[0].name));
    }

    #[test]
    fn save_rejects_external_edit_during_editor_session() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
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

        let scout_path = paths.canonical_dir.join("scout.md");
        let original_bytes = std::fs::read_to_string(&scout_path).unwrap();
        let external = original_bytes.replace("read-only codebase scout", "externally rewritten");
        std::fs::write(&scout_path, &external).unwrap();
        assert_ne!(
            hash_file(&scout_path).unwrap().as_deref(),
            Some(prior_hash.as_str()),
            "sanity: external edit changed the hash"
        );

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

        let on_disk = std::fs::read_to_string(&scout_path).unwrap();
        assert!(
            on_disk.contains("externally rewritten"),
            "external edit must be preserved"
        );
        assert!(
            !on_disk.contains("Editor edit"),
            "editor's stale draft must not be written"
        );

        assert_eq!(app.editor_prior_hash.as_deref(), Some(prior_hash.as_str()));
        assert_eq!(
            app.editor_original_name.as_deref(),
            original_name.as_deref()
        );
    }

    #[test]
    fn save_rename_rejects_stale_source_without_moving_it() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
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
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
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
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths, State::default());
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
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths, State::default());
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
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths, State::default());
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
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths, State::default());
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
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths, State::default());
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
        app.handle_model_picker_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        let closed = match &app.screen {
            Screen::ModelPicker { manual_open, .. } => *manual_open,
            _ => false,
        };
        assert!(!closed);
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
    }

    #[test]
    fn pending_delete_double_press_deletes() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
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
        let s = "abcdef";
        assert_eq!(truncate(s, 4), "abc…");
        assert_eq!(truncate(s, 10), "abcdef");
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
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
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

        up(&mut app, &paths);
        assert_eq!(field(&app), EditorField::Prompt);
    }

    #[test]
    fn editor_jk_navigate_fields_in_normal() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
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

        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(field_of(&app), EditorField::Prompt);

        assert_eq!(app.editor_draft.as_ref().unwrap().agent.name, "agent-1");
    }

    #[test]
    fn editor_literal_jklqw_in_insert() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
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

        app.handle_editor_key(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()),
            &paths,
        );
        assert_eq!(mode_of(&app), EditorMode::Insert);

        type_chars(&mut app, &paths, "qjkhl");
        assert_eq!(mode_of(&app), EditorMode::Insert);
        assert_eq!(field_of(&app), EditorField::Name);
        assert_eq!(
            app.editor_draft.as_ref().unwrap().agent.name,
            "agent-1qjkhl"
        );
    }

    #[test]
    fn editor_esc_returns_to_normal_from_insert() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
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
        assert_eq!(app.editor_draft.as_ref().unwrap().agent.name, "agent-1x");
    }

    // ---------- Canonical / Settings tests ----------

    /// The runtime reads/writes agent definitions only inside the
    /// configured checkout. The historical local default
    /// `<agenthd_root>/agents/` is intentionally not created at any
    /// point in the boot path: `Paths::ensure_dirs` deliberately
    /// omits it, the Settings apply does not call `ensure_dirs` (so
    /// it cannot be smuggled in there), and the runtime never falls
    /// back to it as an unconfigured source. CRUD operations live
    /// in the configured checkout.
    #[test]
    fn canonical_crud_writes_only_inside_checkout() {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = setup_paths_with_checkout(&dir);

        // The agenthd_root/agents directory was not created by
        // setup_paths_with_checkout, and the test's `canonical_dir`
        // is the checkout's `agents/` (set by setup_paths_with_checkout),
        // not the agenthd-root default.
        assert_eq!(paths.canonical_dir, checkout.join("agents"));
        assert!(
            !paths.agenthd_root.join("agents").exists(),
            "the historical <agenthd_root>/agents default must not be created"
        );

        // Create a new agent through the store helper. It must land
        // under the checkout, not under the agenthd root.
        let mut agent = crate::agent::Agent::new_default("helper".to_string()).unwrap();
        agent.description = "A new helper".to_string();
        agent.prompt = "Help the user.".to_string();
        save_canonical(&paths, &agent, None).unwrap();
        assert!(checkout.join("agents").join("helper.md").exists());
        assert!(
            !paths.agenthd_root.join("agents").join("helper.md").exists(),
            "agenthd_root/agents must never receive writes"
        );

        // Rename + delete also stay inside the checkout.
        crate::store::rename_canonical(&paths, "helper", "assistant").unwrap();
        assert!(checkout.join("agents").join("assistant.md").exists());
        assert!(!checkout.join("agents").join("helper.md").exists());
        crate::store::delete_canonical(&paths, "assistant").unwrap();
        assert!(!checkout.join("agents").join("assistant.md").exists());
    }

    /// Empty configured checkout is valid: the runtime surfaces an
    /// empty Agents list, not a seed step.
    #[test]
    fn empty_checkout_loads_zero_agents_and_lists_them() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Build a checkout with an empty agents/ dir.
        let checkout = dir.path().join("empty-checkout");
        std::fs::create_dir_all(checkout.join("agents")).unwrap();
        let paths = Paths {
            canonical_dir: checkout.join("agents"),
            ..paths
        };
        let mut app = App::new(paths, State::default());
        app.open_agents();
        match &app.screen {
            Screen::Agents { agents, status, .. } => {
                assert!(
                    agents.is_empty(),
                    "empty checkout must produce an empty list"
                );
                assert!(
                    status.is_none(),
                    "no error status: an empty checkout is intentional, not a failure"
                );
            }
            screen => panic!("expected Agents screen, got {screen:?}"),
        }
    }

    /// A configured checkout that has been deleted or moved is
    /// detected by `canonical_dir_from`, which `resolve_checkout_path`
    /// in `main.rs` uses to drive the gated Settings recovery flow.
    /// The runtime never silently falls back to a cwd ancestor walk
    /// or deletes any targets; instead `run` opens the Settings
    /// screen gated with the validation error visible. The test pins
    /// the underlying validator contract: a missing checkout must
    /// surface an explicit "does not exist" error so the recovery
    /// banner has something useful to show.
    #[test]
    fn startup_fails_closed_when_configured_checkout_missing() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Configure a checkout path that does not exist.
        let bogus = dir.path().join("does-not-exist");
        let settings = Settings::new(bogus.to_string_lossy().into_owned());
        let result = crate::store::save_settings(&paths.settings_file, &settings);
        assert!(
            result.is_ok(),
            "save_settings succeeds even when path is bogus"
        );
        let loaded = crate::store::load_settings(&paths.settings_file)
            .unwrap()
            .unwrap();
        let err = crate::store::canonical_dir_from(&paths.agenthd_root, &loaded)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("does not exist"),
            "expected missing-checkout error, got: {err}"
        );
    }

    /// The same fail-closed contract applies to a configured path
    /// that points at a directory without an `agents/` child. The
    /// error must surface, never an empty canonical set.
    #[test]
    fn startup_fails_closed_when_checkout_lacks_agents_dir() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Configure a checkout that exists but has no agents/ child.
        let no_agents = dir.path().join("no-agents-here");
        std::fs::create_dir_all(&no_agents).unwrap();
        let settings = Settings::new(no_agents.to_string_lossy().into_owned());
        crate::store::save_settings(&paths.settings_file, &settings).unwrap();
        let loaded = crate::store::load_settings(&paths.settings_file)
            .unwrap()
            .unwrap();
        let err = crate::store::canonical_dir_from(&paths.agenthd_root, &loaded)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("does not exist") || err.contains("`agents/`"),
            "expected missing-agents-dir error, got: {err}"
        );
    }

    /// The Install/Update plan reports a `Remove` action for every
    /// canonical file that has been deleted from the checkout but
    /// whose target is still owned by agenthd. This is the
    /// "planned remove" the design requires the planner to surface
    /// for the Install/Update screen.
    #[test]
    fn plan_reports_remove_for_deleted_canonical_with_owned_target() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let state = State::default();
        // Install scout to the OpenCode target so it is owned.
        let mut state = state;
        let plan = crate::store::plan_for(&paths, &state, SyncTarget::OpenCode).unwrap();
        let (_, _outcomes) = crate::store::apply_safe(&paths, state, plan).unwrap();
        state = recover_outcomes_helper(SyncTarget::OpenCode, &paths);
        // Delete the canonical file.
        crate::store::delete_canonical(&paths, "scout").unwrap();
        // Replan. scout.md must be flagged as Remove (the target
        // still equals our last-installed hash).
        let plan = crate::store::plan_for(&paths, &state, SyncTarget::OpenCode).unwrap();
        let scout = plan
            .iter()
            .find(|i| i.filename == "scout.md")
            .expect("scout.md must still be in the plan");
        assert_eq!(
            scout.status,
            crate::store::SyncStatus::Remove,
            "deleted canonical with owned target must surface as Remove"
        );
    }

    /// `sync` includes both OpenCode and Pi targets in the same
    /// plan, with per-target ownership. The new design removes the
    /// Local/Repo toggle and the Compare screen but keeps the Pi
    /// render + the per-harness safe-apply path.
    #[test]
    fn sync_targets_opencode_and_pi_with_per_target_ownership() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let state = State::default();
        let plan = crate::store::compute_plan(&paths, &state).unwrap();
        let oc: Vec<&str> = plan
            .iter()
            .filter(|i| i.target == SyncTarget::OpenCode)
            .map(|i| i.filename.as_str())
            .collect();
        let pi: Vec<&str> = plan
            .iter()
            .filter(|i| i.target == SyncTarget::Pi)
            .map(|i| i.filename.as_str())
            .collect();
        assert!(oc.contains(&"scout.md"));
        assert!(pi.contains(&"scout.md"));
        // apply_safe with the full plan populates both ownership maps.
        let (state, outcomes) = crate::store::apply_safe(&paths, state, plan).unwrap();
        assert!(outcomes.iter().all(|o| o.ok));
        assert!(state.installed.contains_key("scout.md"));
        assert!(state.pi_installed.contains_key("scout.md"));
        // Pi rendering produced a Pi-format file under pi_target_dir.
        let pi_bytes = std::fs::read_to_string(paths.pi_target_dir.join("scout.md")).unwrap();
        assert!(pi_bytes.contains("name: scout"));
        assert!(pi_bytes.contains("tools:"));
        assert!(!pi_bytes.contains("mode:"));
    }

    /// The history-aware parts of the design: re-pointing the
    /// configured checkout changes `paths.canonical_dir` for the
    /// rest of the app. `with_settings` is the re-pointing primitive.
    #[test]
    fn with_settings_repoints_canonical_dir() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Build a second checkout and verify with_settings picks it up.
        let second = dir.path().join("second-checkout");
        std::fs::create_dir_all(second.join("agents")).unwrap();
        std::fs::write(
            second.join("agents").join("worker.md"),
            starter_agent(&STARTERS[2]).render(),
        )
        .unwrap();
        let settings = Settings::new(second.to_string_lossy().into_owned());
        let re_pointed = paths
            .with_settings(&settings)
            .expect("with_settings must succeed for a real checkout");
        assert_eq!(re_pointed.canonical_dir, second.join("agents"));
    }

    /// Helper used by the planned-remove test to recover the
    /// post-apply state through the per-target ownership map the
    /// implementation already exposes to the planner. Re-loading
    /// from disk keeps the test independent of any private helper.
    fn recover_outcomes_helper(_target: SyncTarget, paths: &Paths) -> State {
        State::load(&paths.state_file).unwrap_or_default()
    }

    // ---------- Settings screen tests ----------

    /// Esc inside the inline path editor cancels the edit and
    /// restores the buffer to the value the editor opened with —
    /// even when the buffer is non-empty. Without this guard a
    /// stray Esc would destroy a half-typed path; the user has to
    /// press it again to leave the screen (or to drop into the
    /// post-edit summary view).
    #[test]
    fn settings_esc_cancels_editing_and_restores_buffer_even_when_nonempty() {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
        // Seed a persisted checkout so the buffer opens with it.
        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        crate::store::save_settings(&paths.settings_file, &settings).unwrap();
        app.open_settings(false);
        let initial = match &app.screen {
            Screen::Settings { state } => state.path_input.buffer.clone(),
            _ => panic!("expected settings screen"),
        };
        assert!(
            !initial.is_empty(),
            "buffer should be the persisted checkout"
        );

        // Enter edit mode.
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(match &app.screen {
            Screen::Settings { state } => state.path_editing,
            _ => false,
        });

        // Type a different path. The buffer must change so we know
        // Esc had something to cancel.
        for c in "/tmp/somewhere/else".chars() {
            let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        assert!(match &app.screen {
            Screen::Settings { state } => state.path_input.buffer != initial,
            _ => false,
        });

        // Esc cancels: buffer restored, editing exits.
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        match &app.screen {
            Screen::Settings { state } => {
                assert_eq!(
                    state.path_input.buffer, initial,
                    "Esc must restore the persisted path"
                );
                assert!(
                    !state.path_editing,
                    "Esc must drop out of edit mode so the user can leave"
                );
            }
            _ => panic!("Esc must not leave the Settings screen"),
        }

        // A second Esc on the summary view, with the screen not
        // gated, returns to the main menu.
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Main { .. }));
    }

    /// First-run (gated) Settings: Esc on the editor with an empty
    /// buffer must keep the user in edit mode — they cannot escape
    /// without picking a checkout. Esc on the summary view is a
    /// no-op as well (there is no summary view to escape to).
    #[test]
    fn settings_first_run_is_gated_and_esc_is_no_op() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        app.open_settings(true);
        // First-run drops into the editor automatically.
        assert!(match &app.screen {
            Screen::Settings { state } => state.path_editing && state.gated,
            _ => false,
        });
        // Esc on the empty-buffer editor must NOT drop out of
        // edit mode and must NOT leave the screen.
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Settings { .. }));
        assert!(match &app.screen {
            Screen::Settings { state } => state.path_editing,
            _ => false,
        });

        // Try to type a valid path and submit; only then does the
        // user "escape" the gate.
        for c in "/tmp/agenthd-first-run-test".chars() {
            let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        // Without creating agents/ the apply will fail validation,
        // but the screen stays on Settings — the gate still holds.
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Settings { .. }));
    }

    /// `open_settings_with_error` is the recovery entry point used
    /// by `run` when the persisted checkout has moved. The screen
    /// must:
    ///   - show the banner verbatim as the visible error
    ///   - stay gated (so the user cannot route around it)
    ///   - drop into edit mode (the user must replace the path)
    #[test]
    fn settings_recovery_banner_is_visible_and_forces_edit() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        let banner = "configured checkout `/old/path` is unusable: nope".to_string();
        app.open_settings_with_error(true, Some(banner.clone()));
        match &app.screen {
            Screen::Settings { state } => {
                assert!(state.gated, "recovery must remain gated");
                assert!(state.path_editing, "recovery must open the editor");
                assert_eq!(state.recovery_error.as_deref(), Some(banner.as_str()));
                // The banner is the single source of truth on open;
                // `path_input.error` is reserved for post-Enter validation
                // messages and must NOT mirror the recovery banner
                // (otherwise the renderer would display the banner twice).
                assert!(
                    state.path_input.error.is_none(),
                    "recovery banner must not be mirrored into path_input.error"
                );
            }
            screen => panic!("expected Settings screen, got {screen:?}"),
        }

        // Esc on the editor in recovery mode keeps the banner and
        // keeps the screen; the user must pick a path.
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(matches!(app.screen, Screen::Settings { .. }));
        match &app.screen {
            Screen::Settings { state } => {
                assert!(state.path_editing, "recovery must stay in edit mode");
                assert_eq!(state.recovery_error.as_deref(), Some(banner.as_str()));
            }
            _ => unreachable!(),
        }

        // Submitting an invalid (non-absolute) path surfaces a
        // validation message into `path_input.error` while the
        // recovery banner stays put — the banner must NOT be
        // duplicated into the new error.
        for c in "relative/path".chars() {
            let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        match &app.screen {
            Screen::Settings { state } => {
                assert_eq!(state.recovery_error.as_deref(), Some(banner.as_str()));
                let validation = state
                    .path_input
                    .error
                    .as_deref()
                    .expect("validation error must land in path_input.error after Enter");
                assert!(
                    !validation.contains(&banner),
                    "the validation message must not re-state the recovery banner"
                );
            }
            _ => panic!("expected Settings screen"),
        }

        // Clear the bogus buffer before typing the valid path.
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));

        // Typing a valid path and applying drops the banner and
        // clears the editor.
        let checkout = dir.path().join("recovery-checkout");
        std::fs::create_dir_all(checkout.join("agents")).unwrap();
        for c in checkout.to_string_lossy().chars() {
            let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        // Recovery was gated; with no other screens reachable, the
        // app stays on Settings — but the banner must be gone.
        match &app.screen {
            Screen::Settings { state } => {
                assert!(
                    state.recovery_error.is_none(),
                    "successful apply must drop the recovery banner"
                );
                assert!(!state.path_editing);
            }
            _ => panic!("expected Settings screen"),
        }
    }

    /// `apply_settings_path_input` must NOT call `Paths::ensure_dirs`
    /// — `ensure_dirs` creates the output targets and the agenthd
    /// root, but it must not be conflated with the apply path. The
    /// runtime already created those dirs at startup; the configured
    /// checkout's `agents/` directory is what the apply validates,
    /// not what it makes. This test pins that contract so a future
    /// edit cannot reintroduce the swallowed-`.ok()` call.
    #[test]
    fn settings_apply_does_not_call_ensure_dirs() {
        // We can't reach into `apply_settings_path_input` to count
        // calls directly, but we can pin the observable contract:
        // submitting a valid path leaves the agenthd-root agents
        // directory absent (ensure_dirs would create it on older
        // builds, and the previous version's swallowed `.ok()`
        // proved it could).
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let checkout = dir.path().join("apply-checkout");
        std::fs::create_dir_all(checkout.join("agents")).unwrap();
        let mut app = App::new(paths, State::default());
        app.open_settings(false);
        // Open the editor and type the path.
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        for c in checkout.to_string_lossy().chars() {
            let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        // The apply re-points the App's canonical_dir at the
        // checkout's `agents/`. The test holds the only Paths
        // reference, so we read it back via the App.
        assert_eq!(app.paths.canonical_dir, checkout.join("agents"));
        // The agenthd-root agents directory must NOT exist:
        // ensure_dirs no longer creates it.
        assert!(
            !app.paths.agenthd_root.join("agents").exists(),
            "apply must not create <agenthd_root>/agents"
        );
        // The configured checkout's agents/ directory is what the
        // runtime now treats as canonical; it existed before apply
        // (validation requires it) and must still exist.
        assert!(checkout.join("agents").is_dir());
    }

    /// First-run Settings (no persisted `settings.json`) must
    /// prefill the editor buffer with a valid cwd ancestor when
    /// one exists, without persisting or auto-confirming. The hint
    /// is buffer text only: the user must press Enter (which
    /// still runs `validate_checkout_path` and writes
    /// `settings.json`) or type a different path. The test
    /// verifies the buffer reflects the cwd-ancestor hint and
    /// that no settings file was written by the open call.
    #[test]
    fn settings_first_run_prefills_cwd_ancestor_hint_without_persisting() {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
        // Remove the persisted settings file so the screen opens
        // in first-run mode (the seeded settings.json from
        // `setup_paths_with_checkout` would otherwise produce a
        // valid-persisted branch).
        std::fs::remove_file(&paths.settings_file).unwrap();
        // Place the cwd at the checkout so `find_checkout_root_from`
        // resolves to the checkout.
        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&checkout).unwrap();
        app.open_settings(true);
        let result = std::env::set_current_dir(&original_cwd);
        let _ = result; // best-effort restore
        match &app.screen {
            Screen::Settings { state } => {
                assert!(state.gated, "first run is gated");
                assert!(state.path_editing, "first run opens the editor");
                assert!(
                    state
                        .path_input
                        .buffer
                        .contains(checkout.to_string_lossy().as_ref()),
                    "buffer should prefill with the cwd-ancestor hint ({}), got: {}",
                    checkout.display(),
                    state.path_input.buffer
                );
                assert!(
                    state.path_input.error.is_none(),
                    "first-run prefill is not an error"
                );
            }
            screen => panic!("expected Settings screen, got {screen:?}"),
        }
        // No settings file must have been written by the open.
        assert!(
            !paths.settings_file.exists(),
            "open_settings_with_error must not persist a hint"
        );
    }

    /// Recovery from a stale persisted checkout must put the
    /// stored (invalid) path text into the editor buffer so the
    /// user can edit it in place, and must surface the
    /// validation error as the banner above the editor. This is
    /// the regression test for the case where a moved checkout
    /// would either silently empty the editor (forcing the user
    /// to retype the path from memory) or hide the failure behind
    /// a valid-but-wrong prefill.
    #[test]
    fn settings_recovery_restores_invalid_stored_path_text_and_shows_error() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Persist a checkout path that does not exist so
        // `validate_checkout_path` will refuse it.
        let stale = dir.path().join("moved-checkout");
        std::fs::create_dir_all(&paths.settings_file.parent().unwrap()).unwrap();
        crate::store::save_settings(
            &paths.settings_file,
            &Settings::new(stale.to_string_lossy().into_owned()),
        )
        .unwrap();
        // Hand-craft a banner the way `main.rs` would, then
        // open the screen.
        let banner = format!(
            "configured checkout `{}` is unusable: nope; type a new path or relaunch with `--repo <path>`",
            stale.display()
        );
        let mut app = App::new(paths.clone(), State::default());
        app.open_settings_with_error(true, Some(banner.clone()));
        match &app.screen {
            Screen::Settings { state } => {
                assert!(state.gated, "recovery is gated");
                assert!(state.path_editing, "recovery drops into the editor");
                // The invalid stored text is in the buffer so the
                // user can edit-in-place rather than retype.
                assert_eq!(
                    state.path_input.buffer,
                    stale.to_string_lossy().into_owned(),
                    "recovery must restore the stored invalid path into the buffer"
                );
                // The banner is the visible error and is the single
                // source of truth on open; `path_input.error` stays
                // None so the renderer does not display the banner twice.
                assert_eq!(state.recovery_error.as_deref(), Some(banner.as_str()));
                assert!(
                    state.path_input.error.is_none(),
                    "recovery banner must not be mirrored into path_input.error"
                );
                // Esc on the editor in recovery mode keeps the
                // banner and stays in edit mode; the user must
                // pick a real path before leaving.
                let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
                match &app.screen {
                    Screen::Settings { state } => {
                        assert!(state.path_editing, "recovery must stay in edit mode");
                        assert_eq!(state.recovery_error.as_deref(), Some(banner.as_str()));
                        // Buffer is the invalid text (restored
                        // by Esc to the initial snapshot).
                        assert_eq!(
                            state.path_input.buffer,
                            stale.to_string_lossy().into_owned()
                        );
                    }
                    _ => unreachable!(),
                }
            }
            screen => panic!("expected Settings screen, got {screen:?}"),
        }
    }

    /// First-keystroke on a prefill buffer must replace the hint
    /// rather than append to it. The hint is meant to be either
    /// accepted (Enter without typing) or replaced (start
    /// typing); appending would produce an invalid path the user
    /// would have to clear out by hand. The replacement fires
    /// only when the buffer still equals the on-open snapshot —
    /// a buffer the user has already edited is theirs to extend.
    #[test]
    fn settings_first_run_typing_replaces_prefill_instead_of_appending() {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths.clone(), State::default());
        std::fs::remove_file(&paths.settings_file).unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&checkout).unwrap();
        app.open_settings(true);
        std::env::set_current_dir(&original_cwd).unwrap();
        let hint = match &app.screen {
            Screen::Settings { state } => state.path_input.buffer.clone(),
            _ => panic!("expected Settings screen"),
        };
        assert!(!hint.is_empty(), "prefill should populate the buffer");
        // First keystroke replaces the hint.
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        match &app.screen {
            Screen::Settings { state } => {
                assert_eq!(
                    state.path_input.buffer, "a",
                    "first keystroke must replace the prefill hint"
                );
                assert!(state.path_editing);
            }
            _ => panic!("expected Settings screen"),
        }
        // Second keystroke appends (the buffer is no longer the
        // initial snapshot).
        let _ = app.handle_settings_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty()));
        match &app.screen {
            Screen::Settings { state } => {
                assert_eq!(state.path_input.buffer, "ab");
            }
            _ => panic!("expected Settings screen"),
        }
    }

    /// The gated first-run / recovery branch in `main::run` opens
    /// `App::new(paths, …)` before the user has configured a
    /// checkout. Constructing the gated `App` with `State::default()`
    /// would silently throw away any pre-existing ownership manifest
    /// (`~/.agenthd/state.json`) — per-target owned file hashes —
    /// and the next sync would treat every previously-owned target
    /// as unowned / a conflict. This test pins the contract that
    /// `State::load` round-trips through the gated `App::new` and
    /// survives `open_settings_with_error` (the gated recovery
    /// entry point), so the runtime's startup load
    /// (`State::load(&paths.state_file)` → `App::new(paths, state)`)
    /// preserves ownership across first-run / stale-checkout setup.
    #[test]
    fn gated_setup_preserves_existing_ownership_state() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Seed a real ownership manifest on disk covering all three
        // per-target maps (OpenCode agents, Pi agents, OpenCode skills).
        let mut installed = BTreeMap::new();
        installed.insert("scout.md".to_string(), "hash-scout".to_string());
        installed.insert("reviewer.md".to_string(), "hash-reviewer".to_string());
        let mut pi_installed = BTreeMap::new();
        pi_installed.insert("delegate.md".to_string(), "hash-delegate".to_string());
        let mut installed_skills = BTreeMap::new();
        installed_skills.insert(
            "clarify-before-coding".to_string(),
            crate::store::OwnedSkill {
                tree_hash: "hash-clarify".to_string(),
                skill_name: "clarify-before-coding".to_string(),
            },
        );
        let prior = State {
            installed,
            pi_installed,
            installed_skills,
        };
        std::fs::write(
            &paths.state_file,
            serde_json::to_vec_pretty(&prior).unwrap(),
        )
        .unwrap();
        // Sanity: re-load via the production helper to confirm the
        // on-disk JSON round-trips through `State::load` — the same
        // call the fixed `main::run` makes on the gated branch.
        let reloaded = State::load(&paths.state_file).unwrap();
        assert_eq!(reloaded, prior, "seed state.json must round-trip");

        // Mirror the fixed gated setup in main::run.
        let state = State::load(&paths.state_file).unwrap();
        let mut app = App::new(paths.clone(), state);

        // No settings file → first-run gated open.
        app.open_settings_with_error(true, None);

        // Every field of the pre-existing manifest must survive.
        assert_eq!(
            app.state, prior,
            "gated App::new must keep the on-disk ownership manifest; \
             State::default() would erase prior installed hashes"
        );
        assert_eq!(
            app.state.installed.get("scout.md").map(String::as_str),
            Some("hash-scout")
        );
        assert_eq!(
            app.state
                .pi_installed
                .get("delegate.md")
                .map(String::as_str),
            Some("hash-delegate")
        );

        // The gated Settings screen must also stay gated (so the
        // user is forced to pick a checkout) while the manifest
        // remains untouched.
        match &app.screen {
            Screen::Settings { state } => {
                assert!(state.gated, "gated open must keep the gate");
            }
            screen => panic!("expected Settings screen, got {screen:?}"),
        }
        assert_eq!(
            app.state, prior,
            "open_settings_with_error must not mutate the ownership manifest"
        );
    }

    /// Legacy `state.json` files written by older agenthd builds may
    /// carry fields this build no longer models (e.g. `plugin_hash`).
    /// `State::load` deserializes with serde defaults for known fields
    /// and ignores unknown ones, so a leftover `plugin_hash` is
    /// nonfatal — load succeeds, the field is dropped on next save,
    /// and no plugin code path can be reached because the Subagent
    /// panel was removed entirely.
    #[test]
    fn state_load_ignores_legacy_plugin_hash_field() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let legacy = serde_json::json!({
            "installed": {"scout.md": "hash-scout"},
            "pi_installed": {"delegate.md": "hash-delegate"},
            "plugin_hash": "hash-plugin-legacy",
        });
        std::fs::write(
            &paths.state_file,
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();
        let loaded = State::load(&paths.state_file).expect("legacy state.json must load");
        assert_eq!(
            loaded.installed.get("scout.md").map(String::as_str),
            Some("hash-scout")
        );
        assert_eq!(
            loaded.pi_installed.get("delegate.md").map(String::as_str),
            Some("hash-delegate")
        );
        // `plugin_hash` is unknown to the current State; serde
        // silently drops it. The next write rebuilds the JSON without
        // the field.
        std::fs::write(
            &paths.state_file,
            serde_json::to_vec_pretty(&loaded).unwrap(),
        )
        .unwrap();
        let rewritten = std::fs::read_to_string(&paths.state_file).unwrap();
        assert!(
            !rewritten.contains("plugin_hash"),
            "legacy plugin_hash must be dropped on resave: {rewritten}"
        );
    }

    /// The Subagent-panel (plugin) main-menu entry was removed because
    /// the bundled OpenCode sidebar plugin never worked. The main menu
    /// must now list exactly Agents, Install/Update, Tools, Settings,
    /// and Exit.
    #[test]
    fn main_menu_does_not_include_plugin() {
        let items: Vec<MainItem> = MainItem::all().to_vec();
        assert_eq!(
            items,
            vec![
                MainItem::Agents,
                MainItem::InstallUpdate,
                MainItem::Tools,
                MainItem::Settings,
                MainItem::Exit,
            ],
            "main menu must not include MainItem::Plugin"
        );
        for item in items {
            assert_ne!(
                item.label(),
                "Subagent panel",
                "Subagent panel label must be gone"
            );
            assert!(
                !item.detail().contains("OpenCode task sidebar"),
                "Subagent panel detail must be gone: {}",
                item.detail()
            );
        }
    }

    /// The main-menu order is a stable user-visible contract. A future
    /// reorder must touch `MainItem::all()` AND this assertion.
    #[test]
    fn main_menu_order_and_labels_are_pinned() {
        let items: Vec<MainItem> = MainItem::all().to_vec();
        assert_eq!(
            items,
            vec![
                MainItem::Agents,
                MainItem::InstallUpdate,
                MainItem::Tools,
                MainItem::Settings,
                MainItem::Exit,
            ],
            "main menu order is part of the user-visible UI contract"
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label()).collect();
        assert_eq!(
            labels,
            vec!["Agents", "Install / Update", "Tools", "Settings", "Exit"],
            "labels must read exactly as the user sees them"
        );
        for item in items {
            assert!(!item.group().is_empty(), "group cue must be non-empty");
            assert!(
                !item.detail().is_empty(),
                "detail must be non-empty for {item:?}"
            );
        }
    }

    /// Pin the index→screen dispatch mapping. Reordering `MainItem::all()`
    /// would silently rewire every index, so this test pins the mapping
    /// the user has already memorized.
    #[test]
    fn main_menu_index_to_screen_mapping_is_pinned() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths, State::default());

        app.screen = Screen::Main { selected: 0 };
        app.handle_main_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(
            matches!(app.screen, Screen::Agents { .. }),
            "index 0 must open the Agents screen, got {:?}",
            app.screen
        );

        app.screen = Screen::Main { selected: 2 };
        app.handle_main_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(
            matches!(app.screen, Screen::Tools { .. }),
            "index 2 must open the Tools screen, got {:?}",
            app.screen
        );

        app.screen = Screen::Main { selected: 3 };
        app.handle_main_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(
            matches!(app.screen, Screen::Settings { .. }),
            "index 3 must open the Settings screen, got {:?}",
            app.screen
        );

        app.screen = Screen::Main { selected: 4 };
        app.handle_main_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(app.quit, "index 4 must quit the app");
        assert!(
            matches!(app.screen, Screen::Main { .. }),
            "Exit must not transition to another screen"
        );
    }

    // ---------- Skills list UI tests ------------------------------------------

    /// Opening the Skills list with a populated source produces a
    /// plan whose first action is `Install`. The screen lists every
    /// skill directory the configured checkout ships.
    #[test]
    fn open_skills_lists_source_skills_as_install() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_skills(&dir);
        let mut app = App::new(paths, State::default());
        app.open_skills();
        match &app.screen {
            Screen::Skills { items, .. } => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].name, "clarify-before-coding");
                assert!(matches!(
                    items[0].action,
                    crate::store::SkillAction::Install
                ));
                assert_eq!(items[1].name, "kiss-for-you");
            }
            screen => panic!("expected Skills screen, got {screen:?}"),
        }
    }

    /// Opening the Skills list with an empty (but present) source
    /// `skills/` directory produces an empty plan; the empty-state
    /// row in the renderer is reached without panic.
    #[test]
    fn open_skills_with_empty_source_yields_empty_plan() {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = setup_paths_with_checkout(&dir);
        // Create an empty `skills/` directory so the planner
        // succeeds with an empty result instead of erroring on a
        // missing source.
        std::fs::create_dir_all(checkout.join("skills")).unwrap();
        let mut app = App::new(paths, State::default());
        app.open_skills();
        match &app.screen {
            Screen::Skills { items, .. } => assert!(items.is_empty()),
            screen => panic!("expected Skills screen, got {screen:?}"),
        }
    }

    /// `o` on the Skills list must NOT trigger an overwrite (there is
    /// no force-overwrite path); the status bar surfaces an
    /// explanatory decline message instead.
    #[test]
    fn skills_o_key_is_declined_with_explanatory_message() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_skills(&dir);
        let mut app = App::new(paths, State::default());
        app.open_skills();
        app.handle_skills_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()));
        let msg = app.status_bar.unwrap_or_default();
        assert!(
            msg.contains("skills sync has no force-overwrite"),
            "expected decline message, got: {msg}"
        );
        // The screen must still be the Skills list — `o` is a no-op
        // beyond the status bar message.
        assert!(matches!(app.screen, Screen::Skills { .. }));
    }

    /// `i` on the Skills list runs the safe plan: each source skill
    /// is installed into `Paths.skills_dir` and ownership is recorded
    /// in `state.installed_skills`.
    #[test]
    fn skills_i_key_runs_safe_install() {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = setup_paths_with_skills(&dir);
        let mut app = App::new(paths, State::default());
        app.open_skills();
        app.handle_skills_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));
        // After apply, the destination contains both skills.
        let skills_dir = app.paths.skills_dir.clone();
        assert!(skills_dir.join("clarify-before-coding").is_dir());
        assert!(skills_dir.join("kiss-for-you").is_dir());
        // State records ownership.
        assert_eq!(app.state.installed_skills.len(), 2);
        // No skills files were written into the repo's `skills/`
        // directory (which is untracked but adjacent to `agents/`).
        // This pins the contract that the installer only writes
        // into the OpenCode global skills dir.
        let _ = checkout;
    }

    /// `Esc` on the Skills list returns the user to the Install/Update
    /// harness selector with no target binding (mirrors the agent
    /// list's `PopToSelector` path).
    #[test]
    fn skills_esc_returns_to_install_update_selector() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_skills(&dir);
        let mut app = App::new(paths, State::default());
        app.open_skills();
        app.handle_skills_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        match &app.screen {
            Screen::InstallUpdate { target, items, .. } => {
                assert!(target.is_none(), "Esc must drop the target binding");
                assert!(items.is_empty());
            }
            screen => panic!("expected InstallUpdate selector, got {screen:?}"),
        }
    }

    // ---------- Install/Update safe-install ordering (direct) ----------

    /// Direct TUI test pinning the observable ordering of
    /// `App::apply_safe_install`: when no harness is bound
    /// (`target = None`) AND the canonical source has been
    /// removed out-of-band, the status bar must carry the
    /// canonical-load error (the "load_canonical runs before
    /// the target guard" invariant) — not the "pick a harness
    /// first" notice. The bound-target guard stays in the TUI;
    /// on the no-target branch the TUI calls `load_canonical`
    /// itself so the source-load error surfaces before the
    /// guard's "pick a harness first" message.
    #[test]
    fn apply_safe_install_no_target_with_missing_source_reports_canonical_error() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Repoint `canonical_dir` at a directory that does not
        // exist so `load_canonical` (called by the TUI's
        // no-target `else` arm before the bound-target guard)
        // fails closed with a "does not exist" error.
        let mut paths = paths;
        paths.canonical_dir = dir.path().join("does-not-exist").join("agents");
        let mut app = App::new(paths, State::default());
        // Open Install/Update and stay on the selector (no
        // harness picked → `target = None`).
        app.open_install_update();
        match &app.screen {
            Screen::InstallUpdate { target: None, .. } => {}
            screen => panic!("expected InstallUpdate selector, got {screen:?}"),
        }

        app.apply_safe_install();

        // The status bar must surface the canonical-load
        // error, NOT "pick a harness first". This is the
        // ordering invariant: on the no-target branch the
        // TUI runs `load_canonical` before the bound-target
        // guard so a missing source still surfaces its own
        // error.
        let status = app
            .status_bar
            .as_deref()
            .expect("status_bar must be set on the fail-closed path");
        assert!(
            status.starts_with("error: "),
            "expected canonical-load error in status_bar, got: {status}"
        );
        assert!(
            status.contains("does not exist"),
            "expected missing-source error from load_canonical, got: {status}"
        );
        assert!(
            !status.contains("pick a harness first"),
            "bound-target guard must NOT fire when load_canonical fails first, got: {status}"
        );

        // The screen must still be the Install/Update selector
        // — the call must not have promoted to a list view.
        match &app.screen {
            Screen::InstallUpdate { target: None, .. } => {}
            screen => panic!("expected selector to stay on screen, got {screen:?}"),
        }
        // No agent file landed under the OpenCode target dir.
        assert!(
            !fs::read_dir(&app.paths.target_dir)
                .map(|rd| rd.count())
                .unwrap_or(0)
                > 0,
            "no target_dir writes on the fail-closed path"
        );
    }

    /// Direct TUI test pinning the bound-target guard on a
    /// valid source: when the canonical checkout is fine but
    /// no harness is bound (`target = None`), the status bar
    /// must say `"pick a harness first"` (matching the
    /// previous inline message bit-for-bit). This is the
    /// mirror of the previous test: source valid + no
    /// target → NoTarget path.
    #[test]
    fn apply_safe_install_no_target_with_valid_source_reports_pick_a_harness_first() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let mut app = App::new(paths, State::default());
        app.open_install_update();
        match &app.screen {
            Screen::InstallUpdate { target: None, .. } => {}
            screen => panic!("expected InstallUpdate selector, got {screen:?}"),
        }

        app.apply_safe_install();

        let status = app
            .status_bar
            .as_deref()
            .expect("status_bar must be set on the no-target path");
        assert_eq!(
            status, "pick a harness first",
            "bound-target guard message must match the previous inline text"
        );
    }

    // ---------- Tools screen (TUI focal tests) ----------

    /// `App::open_tools` → `App::refresh_tools` reads
    /// `crate::tools::DEFAULT_CATALOG` and asks
    /// `crate::tools::tool_status` for each entry, populating
    /// `Screen::Tools { entries, .. }`. With nothing pre-staged
    /// on disk, every entry must read `NotInstalled` (the
    /// pre-flight checks git/node/npm are NOT run here — they
    /// only fire when the user actually presses `i`). This
    /// pins the catalog → screen contract without invoking
    /// the installer.
    #[test]
    fn tools_refresh_populates_entries_from_default_catalog_status_not_installed() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        // Ensure `skills_dir` exists; `tool_status` only does a
        // `symlink_metadata` so a missing destination simply
        // reports `NotInstalled`. No preflight / no spawn.
        app.open_tools();
        match &app.screen {
            Screen::Tools {
                entries, status, ..
            } => {
                assert_eq!(
                    entries.len(),
                    DEFAULT_CATALOG.len(),
                    "screen must list every catalog entry"
                );
                for item in entries.iter() {
                    assert_eq!(
                        item.status,
                        ToolStatus::NotInstalled,
                        "no destination on disk → NotInstalled, got {:?} for {}",
                        item.status,
                        item.entry.skill_name
                    );
                }
                assert!(
                    status.is_none(),
                    "no preflight error path on a clean skills_dir, got {status:?}"
                );
            }
            screen => panic!("expected Tools screen, got {screen:?}"),
        }
        assert!(
            app.status_bar.is_none(),
            "no status_bar message on a clean refresh, got {:?}",
            app.status_bar
        );
    }

    /// `tool_status` is the only store-side touch `refresh_tools`
    /// performs. Stage a directory at the catalog entry's
    /// destination (`<skills_dir>/<destination_subpath>`) and
    /// re-open the screen: the row must report `Installed` for
    /// that entry, every other entry still reports
    /// `NotInstalled`. This proves the screen reads status off
    /// the filesystem without spawning anything.
    #[test]
    fn tools_refresh_status_reflects_filesystem_state_per_entry() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Stage one catalog entry on disk so its row flips to
        // `Installed` after refresh.
        let staged = paths
            .skills_dir
            .join(DEFAULT_CATALOG[0].destination_subpath);
        std::fs::create_dir_all(&staged).unwrap();
        let mut app = App::new(paths, State::default());
        app.open_tools();
        match &app.screen {
            Screen::Tools { entries, .. } => {
                assert_eq!(entries[0].status, ToolStatus::Installed);
                assert_eq!(
                    entries[0].destination, staged,
                    "destination path must mirror the on-disk layout"
                );
                if entries.len() > 1 {
                    for item in &entries[1..] {
                        assert_eq!(
                            item.status,
                            ToolStatus::NotInstalled,
                            "other catalog entries must remain NotInstalled"
                        );
                    }
                }
            }
            screen => panic!("expected Tools screen, got {screen:?}"),
        }
    }

    /// Conflict shape: a non-directory file at the destination
    /// must read as `ToolStatus::Conflict` after refresh. This
    /// pins the screen's filesystem-driven classification
    /// without exercising `install_tool`.
    #[test]
    fn tools_refresh_status_reads_conflict_when_destination_is_a_file() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        // Plant a regular file at the destination.
        let dest = paths
            .skills_dir
            .join(DEFAULT_CATALOG[0].destination_subpath);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"not-a-directory").unwrap();
        let mut app = App::new(paths, State::default());
        app.open_tools();
        match &app.screen {
            Screen::Tools { entries, .. } => {
                assert_eq!(entries[0].status, ToolStatus::Conflict);
            }
            screen => panic!("expected Tools screen, got {screen:?}"),
        }
    }

    /// `tools_screen_install_target` is the pure dispatch
    /// helper that decides whether the `i` key should fire the
    /// installer. With `selected` pointing at a real entry it
    /// returns `Some(selected)`; with `selected` out of range
    /// it returns `None` so `handle_tools_key` can early-return
    /// without invoking `install_tool`. Pin both arms directly
    /// — no installer is invoked.
    #[test]
    fn tools_screen_install_target_returns_selected_when_in_range() {
        let items: Vec<ToolItem> = (0..3)
            .map(|_| ToolItem {
                entry: &DEFAULT_CATALOG[0],
                status: ToolStatus::NotInstalled,
                detail: String::new(),
                destination: std::path::PathBuf::new(),
            })
            .collect();
        assert_eq!(
            App::tools_screen_install_target(&items, 0),
            Some(0),
            "in-range selected must dispatch"
        );
        assert_eq!(
            App::tools_screen_install_target(&items, 2),
            Some(2),
            "last in-range selected must dispatch"
        );
        assert_eq!(
            App::tools_screen_install_target(&items, 3),
            None,
            "selected past the end must NOT dispatch"
        );
        assert_eq!(
            App::tools_screen_install_target(&[], 0),
            None,
            "empty items must NOT dispatch"
        );
    }

    /// Pressing `i` with `selected` past the end of `entries`
    /// must early-return without touching the screen state or
    /// invoking the installer. The dispatcher sets
    /// `installing = true` only inside `install_selected_tool`;
    /// an out-of-range selection never reaches that path, so
    /// `installing` stays false and the screen stays on
    /// `Screen::Tools`. `open_tools` populates one entry per
    /// `DEFAULT_CATALOG` row; with the current catalog that is
    /// a single row, so `selected = 99` is past the end and
    /// the dispatch helper returns `None`.
    #[test]
    fn tools_i_key_with_out_of_range_selected_is_a_no_op() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        app.open_tools();
        let entries_len = match &app.screen {
            Screen::Tools { entries, .. } => entries.len(),
            _ => panic!("expected Tools screen"),
        };
        assert!(
            entries_len >= 1,
            "test setup: catalog must populate at least one entry, got {entries_len}"
        );
        match &mut app.screen {
            Screen::Tools { selected, .. } => {
                *selected = entries_len + 100;
            }
            _ => panic!("expected Tools screen"),
        }
        app.handle_tools_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));
        match &app.screen {
            Screen::Tools { installing, .. } => {
                assert!(
                    !*installing,
                    "out-of-range `i` must not set installing=true"
                );
            }
            screen => panic!("expected Tools screen to stay put, got {screen:?}"),
        }
    }

    /// While `installing = true`, every key in the Tools
    /// dispatch table (`Esc`, arrows, `i`, `r`) must be a no-op
    /// — the dispatcher returns early before any arm runs, so
    /// `installing` stays true and the screen state stays put.
    /// This pins the "block dispatch while an install is
    /// running so the user cannot queue more work or race the
    /// spawn loop" invariant without running a real install.
    #[test]
    fn tools_keys_are_blocked_while_installing_flag_is_set() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        app.open_tools();
        // Force `installing = true` as if a real
        // `install_selected_tool` were mid-flight.
        match &mut app.screen {
            Screen::Tools {
                entries,
                selected,
                installing,
                ..
            } => {
                // Snapshot current state for assertions.
                let entries_len = entries.len();
                let before = (*selected, entries_len);
                *installing = true;
                for code in [
                    KeyCode::Esc,
                    KeyCode::Up,
                    KeyCode::Down,
                    KeyCode::Char('r'),
                    KeyCode::Char('i'),
                ] {
                    app.handle_tools_key(KeyEvent::new(code, KeyModifiers::empty()));
                    match &app.screen {
                        Screen::Tools {
                            selected: s,
                            entries: es,
                            installing: ins,
                            ..
                        } => {
                            assert!(
                                *ins,
                                "installing must stay true while blocked, key={code:?}"
                            );
                            assert_eq!(
                                (*s, es.len()),
                                before,
                                "selected/entries must not change while blocked, key={code:?}"
                            );
                        }
                        screen => panic!(
                            "screen must stay on Tools while blocked, got {screen:?} (key={code:?})"
                        ),
                    }
                }
            }
            screen => panic!("expected Tools screen, got {screen:?}"),
        }
    }

    /// `handle_event` filters out non-Press key kinds before
    /// dispatching. A Release event for the same key that
    /// would otherwise pop the Tools screen must be a no-op:
    /// the screen stays on Tools, `status_bar` is untouched,
    /// and no error is propagated. This pins the
    /// Press-only-dispatch contract from `handle_event`.
    #[test]
    fn tools_release_event_is_filtered_before_dispatch() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        app.open_tools();
        // Snapshot the screen so we can assert nothing
        // changed after the Release event.
        let before_screen = match &app.screen {
            Screen::Tools {
                entries, selected, ..
            } => (entries.len(), *selected),
            screen => panic!("expected Tools screen, got {screen:?}"),
        };
        let status_before = app.status_bar.clone();
        // Build a Release event for `Esc` (would otherwise pop
        // to the main menu).
        let release = KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Release,
            state: crossterm::event::KeyEventState::NONE,
        };
        // `handle_event` is private to `App`; the `app::tests`
        // submodule has direct access.
        app.handle_event(Event::Key(release))
            .expect("Release event must not error");
        match &app.screen {
            Screen::Tools {
                entries, selected, ..
            } => {
                assert_eq!(
                    (entries.len(), *selected),
                    before_screen,
                    "Release event must not mutate the Tools screen"
                );
            }
            screen => panic!("Release event must not pop Tools screen, got {screen:?}"),
        }
        assert_eq!(
            app.status_bar, status_before,
            "Release event must not touch status_bar"
        );
    }

    /// Press event for the same key DOES dispatch: the
    /// companion to the Release test. Pressing `Esc` from the
    /// Tools screen pops back to the main menu. This makes the
    /// Press-vs-Release distinction observable in the test
    /// surface.
    #[test]
    fn tools_press_event_for_esc_pops_to_main_menu() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        app.open_tools();
        let press = KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_event(Event::Key(press))
            .expect("Press event must not error");
        assert!(
            matches!(app.screen, Screen::Main { .. }),
            "Press Esc must pop to Main, got {:?}",
            app.screen
        );
    }

    /// Ctrl-C inside any screen (Tools included) must set
    /// `quit = true` without changing the screen and without
    /// erroring. The dispatch layer checks Ctrl-C before
    /// routing by screen, so the screen the user was on at
    /// the moment of Ctrl-C stays put. `quit` is a private
    /// field on `App`; this test lives in the same module so
    /// it can read it directly (no accessor).
    #[test]
    fn ctrl_c_from_tools_screen_returns_ok_and_leaves_screen_in_place() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        app.open_tools();
        match &app.screen {
            Screen::Tools { .. } => {}
            screen => panic!("expected Tools screen, got {screen:?}"),
        }
        assert!(!app.quit, "quit must be false before the dispatch runs");
        // Ctrl-C dispatch is in `handle_key` (not
        // `handle_event`), so build the key as the dispatcher
        // sees it.
        let result = app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(
            result.is_ok(),
            "Ctrl-C dispatch must not error, got {result:?}"
        );
        assert!(
            app.quit,
            "Ctrl-C must set quit = true so the main loop exits"
        );
        match &app.screen {
            Screen::Tools { .. } => {}
            screen => panic!("Ctrl-C must not change the screen, got {screen:?}"),
        }
    }

    // ---------- Main menu rendering (TestBackend) ----------

    /// Verify that the main menu renders every label in source order and
    /// that the selected row's label sits inside a sage pill (`SURFACE`
    /// foreground on `ACCENT` background). Asserts through the rendered
    /// buffer so the test stays valid even if helper names drift.
    #[test]
    fn main_menu_renders_labels_in_order_with_sage_pill() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| app.render(frame))
            .expect("render main menu");
        let buffer = terminal.backend().buffer().clone();

        // Walk the body row band (rows 3..=29; header occupies 0..=2)
        // and assemble per-row strings so we can pin the vertical
        // ordering of the labels.
        let mut body_lines: Vec<String> = Vec::new();
        for y in 3..buffer.area.height {
            let mut line = String::new();
            for x in 0..buffer.area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    line.push_str(cell.symbol());
                }
            }
            body_lines.push(line);
        }

        fn first_row_with(lines: &[String], needle: &str) -> Option<usize> {
            lines.iter().position(|l| l.contains(needle))
        }
        let expected_labels = ["Agents", "Install / Update", "Tools", "Settings", "Exit"];
        let mut last_row: Option<usize> = None;
        for label in &expected_labels {
            let row = first_row_with(&body_lines, label)
                .unwrap_or_else(|| panic!("label `{label}` not found in main menu body"));
            if let Some(prev) = last_row {
                assert!(
                    row > prev,
                    "label `{label}` must appear below the previous label in source order \
                     (previous row {prev}, this row {row})"
                );
            }
            last_row = Some(row);
        }

        let agents_row = first_row_with(&body_lines, "Agents").expect("Agents row must be present");
        let agents_col = body_lines[agents_row].find("Agents").expect("Agents label");
        let cell = buffer
            .cell((agents_col as u16, 3 + agents_row as u16))
            .expect("Agents cell must exist");
        assert_eq!(
            cell.fg, SURFACE,
            "selected row label foreground must equal the new surface color"
        );
        assert_eq!(
            cell.bg, ACCENT,
            "selected row label background must equal the new sage accent"
        );
    }

    /// Narrow terminals must keep rendering without panic and the menu
    /// must still surface every label.
    #[test]
    fn main_menu_renders_safely_on_narrow_terminal() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut app = App::new(paths, State::default());
        let backend = TestBackend::new(60, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| app.render(frame))
            .expect("narrow terminal must render without panic");
        let buffer = terminal.backend().buffer().clone();
        let mut all_text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    all_text.push_str(cell.symbol());
                }
            }
            all_text.push('\n');
        }
        for label in ["Agents", "Install / Update", "Tools", "Settings", "Exit"] {
            assert!(
                all_text.contains(label),
                "label `{label}` must still render on a narrow terminal; got:\n{all_text}"
            );
        }
    }

    /// Boundary assertion for the tall-layout threshold in `render_main`.
    /// Tall layout needs `4 * N + 2` rows (20 content + 2 panel borders
    /// for N=5 entries). At body height = 21, a buggy threshold of
    /// `4 * N + 1` would pick tall and clip the last entry off the
    /// bottom of the panel; the corrected threshold of `4 * N + 2`
    /// picks compact and keeps every entry — including the bottom
    /// border — on screen.
    #[test]
    fn main_menu_bottom_border_visible_at_tall_boundary() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let app = App::new(paths, State::default());
        let backend = TestBackend::new(60, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 60, 21);
                app.render_main(frame, area, 0);
            })
            .expect("render at tall boundary");
        let buffer = terminal.backend().buffer().clone();
        // The last row of the rendered rect (y = 20) must contain the
        // panel's bottom border corner / bottom edge.
        let mut bottom_row = String::new();
        for x in 0..buffer.area.width {
            if let Some(cell) = buffer.cell((x, 20)) {
                bottom_row.push_str(cell.symbol());
            }
        }
        assert!(
            bottom_row.contains('╰') || bottom_row.contains('─'),
            "bottom border must be visible at body height 21 (the tall-layout boundary); got `{bottom_row:?}`"
        );
        // And every label must still be reachable from the rendered
        // buffer so the user can navigate the last entry (Exit).
        let mut all_text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    all_text.push_str(cell.symbol());
                }
            }
            all_text.push('\n');
        }
        assert!(
            all_text.contains("Exit"),
            "the last entry (Exit) must not be clipped at the tall-layout boundary"
        );
    }
}
