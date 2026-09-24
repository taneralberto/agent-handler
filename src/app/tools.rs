//! Generic Tools list UI: render / open / refresh / handle / install and
//! the pure install-row helper.
//!
//! The screen is driven entirely by `crate::tools::DEFAULT_CATALOG`; no
//! per-tool view exists in v1 (a future `src/tools/<tool>/view.rs`
//! would be the home for tool-specific UI). The UI here is intentionally
//! shape-agnostic: the catalog may grow, but this module only needs to
//! know how to draw a list, dispatch Esc / arrows / `r` / `i`, and run
//! the installer. Every method on `App` declared here is `pub(super)`
//! so the parent `app` module (and its test submodule) can call them
//! while keeping the visibility footprint minimal.

use super::{panel, render_popup, selected_style, truncate, App, Screen, DANGER, MUTED, SUCCESS};
use crate::tools::{self, ToolItem, ToolOutcome, ToolStatus};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, Paragraph};

impl App {
    pub(super) fn render_tools(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        entries: &[ToolItem],
        selected: usize,
        status: Option<&str>,
        installing: bool,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(if installing { 1 } else { 0 }),
            ])
            .split(area);
        let header = format!("{:<18} {:<22} {}", "NAME", "STATUS", "DESTINATION");
        let mut items: Vec<ListItem> = Vec::new();
        items.push(ListItem::new(Line::from(header.bold())));
        for (idx, item) in entries.iter().enumerate() {
            let style = if idx == selected {
                selected_style()
            } else {
                match item.status {
                    ToolStatus::Installed => Style::default().fg(SUCCESS),
                    ToolStatus::NotInstalled => Style::default(),
                    ToolStatus::PrerequisitesMissing
                    | ToolStatus::IdentityMismatch
                    | ToolStatus::InstallFailed
                    | ToolStatus::Conflict => Style::default().fg(DANGER),
                }
            };
            let line = Line::from(format!(
                "{:<18} {:<22} {}",
                truncate(item.entry.skill_name, 18),
                truncate(item.status.label(), 22),
                truncate(&item.destination.display().to_string(), 60),
            ));
            items.push(ListItem::new(line).style(style));
        }
        let list = List::new(items).block(panel("Tools"));
        frame.render_widget(list, chunks[0]);
        if installing {
            let progress = Paragraph::new("installing…").style(Style::default().fg(MUTED));
            frame.render_widget(progress, chunks[1]);
        }
        if let Some(text) = status {
            render_popup(frame, area, "Notice", text);
        }
    }

    pub(super) fn open_tools(&mut self) {
        self.refresh_tools();
    }

    /// Build the current Tools view: one `ToolItem` per catalog entry, with
    /// the destination status read off the filesystem. Pre-flight (git/node/npm)
    /// is NOT run here — it runs lazily when the user actually presses `i`.
    pub(super) fn refresh_tools(&mut self) {
        let mut entries = Vec::with_capacity(tools::DEFAULT_CATALOG.len());
        let mut first_error: Option<String> = None;
        for entry in tools::DEFAULT_CATALOG {
            match tools::tool_status(&self.paths, entry) {
                Ok(item) => entries.push(item),
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(format!("error: {}", e));
                    }
                }
            }
        }
        if let Some(err) = first_error {
            self.status_bar = Some(err);
            return;
        }
        let len = entries.len();
        let (selected, status) = if let Screen::Tools {
            selected, status, ..
        } = &self.screen
        {
            (*selected, status.clone())
        } else {
            (0, None)
        };
        self.status_bar = None;
        self.screen = Screen::Tools {
            entries,
            selected: if len == 0 { 0 } else { selected.min(len - 1) },
            status,
            installing: false,
        };
    }

    /// Pick the row index the `i` key should install. Lifted out of
    /// `handle_tools_key` so the dispatch decision can be pinned by a
    /// unit test without invoking the installer — `install_selected_tool`
    /// calls `tools::install_tool` directly with no injection seam, so
    /// running it from a test would spawn real `git` / `node` / `npm` and
    /// touch the network. The conflict outcome of an `Installed` target
    /// is covered by the `tools::install_tool_with` tests using injected
    /// spawn / rename runners; this helper only asserts that the UI
    /// dispatches into the installer regardless of pre-install row
    /// state.
    pub(super) fn tools_screen_install_target(
        items: &[ToolItem],
        selected: usize,
    ) -> Option<usize> {
        if items.get(selected).is_some() {
            Some(selected)
        } else {
            None
        }
    }

    pub(super) fn handle_tools_key(&mut self, key: KeyEvent) {
        enum Op {
            Pop,
            Move(i32),
            Refresh,
            Install(usize),
        }
        let op = {
            if let Screen::Tools {
                entries,
                selected,
                installing,
                ..
            } = &mut self.screen
            {
                if *installing {
                    // Block dispatch while an install is running so the
                    // user cannot queue more work or race the spawn loop.
                    return;
                }
                match key.code {
                    KeyCode::Esc => Op::Pop,
                    KeyCode::Up | KeyCode::Char('k') => Op::Move(-1),
                    KeyCode::Down | KeyCode::Char('j') => Op::Move(1),
                    KeyCode::Char('r') => Op::Refresh,
                    KeyCode::Char('i') => {
                        // `i` always dispatches into the installer, per
                        // TOOL_INSTALLER_PLAN.md: the row's pre-install
                        // status is informational only. Any existing
                        // target dir/file/symlink (including a row that
                        // reads `Installed`) is a Conflict the OS
                        // no-replace primitive must refuse, surfaced
                        // as the status message. We therefore do not
                        // branch on `ToolStatus::Installed` here —
                        // `install_selected_tool` is the single source
                        // of truth for the destination state.
                        if let Some(idx) = Self::tools_screen_install_target(entries, *selected) {
                            Op::Install(idx)
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
            }
            Op::Move(delta) => {
                if let Screen::Tools {
                    entries, selected, ..
                } = &mut self.screen
                {
                    if entries.is_empty() {
                        return;
                    }
                    if delta < 0 {
                        *selected = selected.saturating_sub(1);
                    } else if *selected + 1 < entries.len() {
                        *selected += 1;
                    }
                }
            }
            Op::Refresh => self.refresh_tools(),
            Op::Install(idx) => self.install_selected_tool(idx),
        }
    }

    /// Run `tools::install_tool` for the row at `idx`. The installer is
    /// synchronous and runs inside the TUI event loop, so the screen marks
    /// `installing = true` for the duration to suppress other key input.
    pub(super) fn install_selected_tool(&mut self, idx: usize) {
        let entry = match &self.screen {
            Screen::Tools { entries, .. } => match entries.get(idx) {
                Some(item) => item.entry,
                None => return,
            },
            _ => return,
        };
        if let Screen::Tools { installing, .. } = &mut self.screen {
            *installing = true;
        }
        let outcome: ToolOutcome = match tools::install_tool(&self.paths, entry) {
            Ok(outcome) => outcome,
            Err(e) => {
                if let Screen::Tools {
                    installing, status, ..
                } = &mut self.screen
                {
                    *installing = false;
                    *status = Some(format!("error: {}", e));
                }
                self.status_bar = Some(format!("error: {}", e));
                return;
            }
        };
        let status_message = format!("{}: {}", outcome.status.label(), outcome.detail);
        // Only re-read the destination when the install actually mutated
        // it. On Conflict (or any non-Installed outcome that did not
        // touch the destination) the screen's pre-install row state is
        // already accurate — re-reading would, for example, render a
        // pre-existing directory as a green "installed" row alongside
        // the conflict message. The user can press `r` to refresh
        // explicitly when they want the disk-truth view.
        if matches!(outcome.status, ToolStatus::Installed) {
            self.refresh_tools();
        }
        if let Screen::Tools {
            installing, status, ..
        } = &mut self.screen
        {
            *installing = false;
            *status = Some(status_message.clone());
        }
        self.status_bar = Some(status_message);
    }
}
