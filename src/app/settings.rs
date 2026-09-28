//! Settings screen: configure the canonical checkout path.
//!
//! The screen keeps the path editor minimal — a single text field
//! plus the same `validate_checkout_path` helper used at startup. The
//! settings file lives at `$HOME/.agenthd/settings.json`; the runtime
//! reads it on every launch and re-points `Paths.canonical_dir` at
//! `<checkout>/agents`. There is no Local mode and no Compare mode:
//! the configured checkout is the only source of truth, and the
//! settings screen exists purely to change that one path.
//!
//! Key conventions:
//! - `↑/↓`: move the validation / status list (read-only rows that
//!   describe the current effective path).
//! - `Esc`: cancel the current step.
//!   - When the inline editor is open: cancel the edit, restoring the
//!     buffer to the on-disk / pre-edit value so a stray Esc does
//!     not destroy a typed path.
//!   - When the inline editor is closed: return to the main menu.
//!     First-run is gated, so `Esc` is a no-op there — the user must
//!     pick a checkout before any other screen is reachable.
//! - `Enter`: open the inline path editor from the summary; on the
//!   editor, validate + apply (and write `settings.json`).
//!
//! On the path editor:
//!   - printable characters append
//!   - `Backspace` deletes
//!   - `Ctrl+U` clears the buffer in one keystroke
//!   - `Enter` validates + applies (and writes `settings.json`)
//!   - `Esc` cancels the edit

use super::{panel, selected_style, App, Screen};
use crate::store::{
    canonical_dir_from, find_checkout_root_from, load_settings, save_settings,
    validate_checkout_path, Settings,
};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::path::PathBuf;

/// Editor state for the inline checkout-path input.
#[derive(Debug, Clone)]
pub(super) struct PathInputState {
    pub(super) buffer: String,
    /// Snapshot of the buffer as it was when the editor was opened
    /// (or the screen was opened, when editing started immediately).
    /// `Esc` restores the buffer to this value so the user does not
    /// lose a typed path on a stray cancel.
    pub(super) initial: String,
    pub(super) error: Option<String>,
}

/// Full screen state.
#[derive(Debug)]
pub(super) struct SettingsState {
    /// `true` when the screen gates the rest of the app on first run;
    /// `Esc` / `q` are no-ops in that case so the user must pick.
    pub(super) gated: bool,
    pub(super) path_input: PathInputState,
    /// `true` while the user is editing the inline path buffer.
    pub(super) path_editing: bool,
    /// Last success / failure message from the most recent apply.
    pub(super) status: Option<String>,
    /// Banner shown above the editor when the screen was opened to
    /// recover from a stale persisted checkout. The banner makes the
    /// failure visible; the user must type a new path (or `Esc` /
    /// `--repo` may repair) rather than have the runtime silently
    /// fall back. `None` for the first-run / menu entries.
    pub(super) recovery_error: Option<String>,
}

impl SettingsState {
    fn new(
        gated: bool,
        initial: Option<PathBuf>,
        initial_raw_invalid: Option<PathBuf>,
        recovery_error: Option<String>,
    ) -> Self {
        // Three buffer sources, in priority order:
        //   1. `initial_raw_invalid`: the persisted path that
        //      failed validation. We surface it as the editor
        //      buffer so the user can edit in place rather than
        //      retype the (now-broken) path from memory. The
        //      accompanying banner carries the validation error.
        //   2. `initial`: either a valid persisted path or, on the
        //      first run, a cwd-ancestor hint. The hint is a
        //      suggestion only — the user must still press Enter
        //      to apply and validate it.
        //   3. Empty buffer when nothing is available; the user
        //      must type one.
        let buffer = initial_raw_invalid
            .as_ref()
            .or(initial.as_ref())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        // The `initial` snapshot is what `Esc` restores the buffer
        // to. For the invalid-stored-path case the snapshot is the
        // raw invalid text (matching the buffer) so a stray Esc
        // does not destroy the user's chance to edit-in-place.
        let initial_snapshot = buffer.clone();
        // Drop into path-edit mode automatically when the user
        // must act: first run (gated, no valid persisted path),
        // or recovering from a stale persisted checkout. A
        // valid persisted path opened from the menu keeps the
        // screen in the summary view so the user can Esc back
        // out (and only Enter drops into the editor).
        //
        // On the first run, even when `find_checkout_root_from`
        // produced a cwd-ancestor hint, the editor opens so the
        // user actively confirms or replaces the suggestion. The
        // hint is buffer text only; it is NOT auto-applied.
        let path_editing = initial.is_none() || recovery_error.is_some() || gated;
        Self {
            gated,
            path_input: PathInputState {
                initial: initial_snapshot,
                buffer,
                error: recovery_error.clone(),
            },
            path_editing,
            status: None,
            recovery_error,
        }
    }

    /// `true` while the inline path editor is active. The parent
    /// reads this to swap the footer into the editor-specific keymap.
    pub(super) fn is_path_editing(&self) -> bool {
        self.path_editing
    }
}

impl App {
    /// Open the Settings screen. Called either on the first run
    /// (when `settings.json` is absent) or from the Settings main
    /// menu entry.
    pub fn open_settings(&mut self, gated: bool) {
        self.open_settings_with_error(gated, None);
    }

    /// Open the Settings screen with an optional banner explaining
    /// why the runtime dropped the user here. Used by `run` when the
    /// persisted `settings.json` points at a checkout that has moved
    /// or otherwise failed validation: the failure is surfaced
    /// visibly inside the screen rather than as a hard startup
    /// error, and `--repo` is still available to repair it.
    pub fn open_settings_with_error(&mut self, gated: bool, error: Option<String>) {
        // Pull the persisted path through the store loader so the
        // editor reflects the on-disk truth; an in-memory cached
        // value would be wrong if the user re-launched with --repo.
        // The raw (un-validated) value is what the recovery banner
        // is about, so we keep it in the buffer (so the user can see
        // the now-invalid path they had configured) and surface the
        // error above the editor. On the first-run branch (no
        // persisted settings), we still want a helpful starting
        // point: if the user launched agenthd from inside a tree
        // whose cwd ancestor carries an `agents/` directory, suggest
        // that path as the initial buffer text. The suggestion is
        // local to the editor — it is NOT persisted to
        // `settings.json`, and it does NOT auto-confirm: the user
        // must still press Enter (or type a different path) before
        // the runtime writes anything.
        let (initial, initial_raw_invalid) = match load_settings(&self.paths.settings_file) {
            Ok(Some(s)) => {
                let p = PathBuf::from(&s.checkout_path);
                if validate_checkout_path(&p).is_ok() {
                    (Some(p), None)
                } else {
                    // Stored path is invalid (e.g. the checkout was
                    // moved). Put the raw invalid text into the
                    // editor buffer so the user can edit it in
                    // place; the supplied banner above the editor
                    // carries the underlying validation error.
                    (None, Some(p))
                }
            }
            _ => {
                // No persisted settings yet — first run. Try to
                // prefill from a cwd ancestor with an `agents/`
                // child. The walk is cheap and side-effect free;
                // `find_checkout_root_from` is intentionally
                // permissive (it does not insist on absolute path
                // or non-symlink); the editor's submit step still
                // runs `validate_checkout_path` before persisting,
                // so a non-absolute or symlinked suggestion is
                // refused at apply time. We never persist the
                // suggestion automatically.
                let hint = std::env::current_dir().ok().and_then(|cwd| {
                    find_checkout_root_from(&cwd).map(|root| root.to_string_lossy().into_owned())
                });
                (hint.map(PathBuf::from), None)
            }
        };
        let state = SettingsState::new(gated, initial, initial_raw_invalid, error);
        self.screen = Screen::Settings { state };
    }

    pub(super) fn render_settings(
        &self,
        frame: &mut Frame,
        area: Rect,
        state: &SettingsState,
        status: Option<&str>,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Min(5),
            ])
            .split(area);
        let intro = if state.gated {
            "First-run setup: pick the absolute path to a checkout that contains an `agents/` directory. agenthd reads and writes agent definitions only inside that directory."
        } else {
            "Change the absolute path to the checkout that contains the `agents/` directory. agenthd reads and writes agent definitions only inside that directory."
        };
        frame.render_widget(
            Paragraph::new(intro)
                .wrap(Wrap { trim: false })
                .block(panel("Settings")),
            chunks[0],
        );
        // Body row: a single-list summary of the current effective
        // path. The list keeps the screen shape symmetric with the
        // rest of the app even though there is only one row — adding
        // a second persisted field later becomes one extra list row.
        let current = state.path_input.buffer.clone();
        let rows: Vec<ListItem> = vec![ListItem::new(Line::from(vec![
            "Checkout path: ".bold(),
            if current.is_empty() {
                "(type the absolute path to your checkout)".into()
            } else {
                current.into()
            },
        ]))];
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        let list = List::new(rows)
            .block(panel("Path"))
            .highlight_style(selected_style())
            .highlight_symbol("▌ ");
        frame.render_stateful_widget(list, chunks[1], &mut list_state);

        // Status panel: the recovery banner (when opened via the
        // stale-checkout recovery flow), validation errors, and the
        // optional success status share the same panel because they
        // are mutually exclusive in practice (the banner only
        // appears at open time, errors only appear after Enter, and
        // success banners live in the bottom status bar).
        let mut lines: Vec<Line> = Vec::new();
        if let Some(banner) = &state.recovery_error {
            lines.push(Line::from(format!("error: {banner}")).style(error_style()));
        }
        if let Some(err) = &state.path_input.error {
            lines.push(Line::from(format!("error: {err}")).style(error_style()));
        }
        if let Some(s) = status {
            lines.push(Line::from(s));
        }
        if lines.is_empty() {
            lines.push(Line::from(
                "Press Enter to edit the path; Esc to go back (only when opened from the menu).",
            ));
        }
        let status_area = chunks[2];
        let bordered_height_needed = lines.len() as u16 + 2;
        let can_border = status_area.height >= bordered_height_needed.max(3);
        let mut render_lines = lines;
        if !can_border
            && (state.recovery_error.is_some() || state.path_input.error.is_some())
            && render_lines.len() > 1
        {
            render_lines.remove(0);
        }
        let paragraph = Paragraph::new(render_lines).wrap(Wrap { trim: false });
        if can_border {
            frame.render_widget(paragraph.block(panel("Status")), status_area);
        } else {
            frame.render_widget(paragraph, status_area);
        }
    }

    /// Handle a key event while the Settings screen is on top.
    pub(super) fn handle_settings_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }
        let Screen::Settings { state } = &self.screen else {
            return Ok(());
        };
        let path_editing = state.path_editing;
        let gated = state.gated;
        match key.code {
            KeyCode::Esc => {
                if path_editing {
                    // Esc inside the path editor cancels the edit,
                    // not the screen. Restore the buffer to the
                    // on-disk / pre-edit value so a stray Esc does
                    // not destroy a typed path. On a gated screen
                    // (first run or recovery) the user MUST pick a
                    // checkout before leaving — Esc keeps them in
                    // edit mode and only restores the buffer; the
                    // next Esc leaves the editor open with the
                    // restored text. On a non-gated screen, Esc
                    // drops out of edit mode so a subsequent Esc
                    // can fall through to the screen-exit branch.
                    let Screen::Settings { state } = &mut self.screen else {
                        return Ok(());
                    };
                    let initial = state.path_input.initial.clone();
                    state.path_input.buffer = initial.clone();
                    state.path_input.error = None;
                    // The recovery banner keeps the user in edit
                    // mode (the banner is the reason we entered
                    // edit mode at open time). On the first run
                    // the gate itself keeps the user in edit mode
                    // even if the buffer (with the cwd-ancestor
                    // hint) is non-empty. On a non-gated menu
                    // entry with a valid persisted path, Esc drops
                    // out of edit mode so a subsequent Esc can
                    // close the screen.
                    state.path_editing = state.recovery_error.is_some() || state.gated;
                } else if !gated {
                    self.screen = Screen::Main { selected: 0 };
                }
            }
            KeyCode::Char('q') if !path_editing && !gated => {
                self.screen = Screen::Main { selected: 0 };
            }
            KeyCode::Enter => {
                if !path_editing {
                    let Screen::Settings { state } = &mut self.screen else {
                        return Ok(());
                    };
                    // Snapshot the buffer so a future Esc restores
                    // the value the editor opened with.
                    state.path_input.initial = state.path_input.buffer.clone();
                    state.path_editing = true;
                    state.path_input.error = None;
                } else {
                    self.apply_settings_path_input()?;
                }
            }
            KeyCode::Backspace if path_editing => {
                let Screen::Settings { state } = &mut self.screen else {
                    return Ok(());
                };
                state.path_input.buffer.pop();
                state.path_input.error = None;
            }
            KeyCode::Char('u') if path_editing && key.modifiers.contains(KeyModifiers::CONTROL) => {
                let Screen::Settings { state } = &mut self.screen else {
                    return Ok(());
                };
                state.path_input.buffer.clear();
                state.path_input.error = None;
            }
            KeyCode::Char(c) if path_editing && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let Screen::Settings { state } = &mut self.screen else {
                    return Ok(());
                };
                // First keystroke after a prefill: replace the
                // suggestion rather than appending to it. The
                // placeholder hint is meant to be either accepted
                // (Enter without typing) or replaced (start typing);
                // appending would produce an invalid path the user
                // would have to clear out by hand. Only fire when
                // the buffer still equals the on-open snapshot — a
                // buffer the user has already edited is theirs to
                // extend.
                if state.path_input.buffer == state.path_input.initial
                    && !state.path_input.initial.is_empty()
                {
                    state.path_input.buffer.clear();
                }
                state.path_input.buffer.push(c);
                state.path_input.error = None;
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_settings_path_input(&mut self) -> Result<()> {
        let buffer = match &self.screen {
            Screen::Settings { state } => state.path_input.buffer.trim().to_string(),
            _ => return Ok(()),
        };
        if buffer.is_empty() {
            if let Screen::Settings { state } = &mut self.screen {
                state.path_input.error =
                    Some("checkout path is empty; type the absolute path".to_string());
            }
            return Ok(());
        }
        let path = PathBuf::from(&buffer);
        if !path.is_absolute() {
            if let Screen::Settings { state } = &mut self.screen {
                state.path_input.error =
                    Some(format!("`{}` is not an absolute path", path.display()));
            }
            return Ok(());
        }
        if let Err(e) = validate_checkout_path(&path) {
            if let Screen::Settings { state } = &mut self.screen {
                state.path_input.error = Some(e.to_string());
            }
            return Ok(());
        }
        // Persist the validated path through the store loader so the
        // failure modes (write, mkdir, etc.) land in the status bar
        // rather than the screen-local error.
        let settings = Settings::new(path.to_string_lossy().into_owned());
        if let Err(e) = save_settings(&self.paths.settings_file, &settings) {
            if let Screen::Settings { state } = &mut self.screen {
                state.path_input.error = Some(format!("write settings: {e}"));
            }
            return Ok(());
        }
        // Re-point the runtime's canonical_dir at the new checkout.
        // Failures here are surfaced through the screen-local error
        // so the user sees them rather than seeing a stale
        // canonical. `Paths::ensure_dirs` is intentionally NOT
        // called here: it creates the output target trees but it
        // does NOT create `canonical_dir` (the configured checkout
        // is the source of truth for `agents/`, and it already
        // existed — validation required it). Calling ensure_dirs
        // here would also risk silently masking a real I/O error
        // by `.ok()`-swallowing it.
        let new_canonical = match canonical_dir_from(&self.paths.agenthd_root, &settings) {
            Ok(p) => p,
            Err(e) => {
                if let Screen::Settings { state } = &mut self.screen {
                    state.path_input.error = Some(format!("re-validate: {e}"));
                }
                return Ok(());
            }
        };
        self.paths.canonical_dir = new_canonical;
        if let Screen::Settings { state } = &mut self.screen {
            // A successful apply means the recovery banner has
            // been satisfied: drop it so the screen returns to
            // its normal non-error look. The new "initial" is the
            // just-saved path; a future Esc on this Settings
            // entry restores to it rather than to the stale one.
            state.path_input.initial = path.to_string_lossy().into_owned();
            state.path_input.buffer = state.path_input.initial.clone();
            state.path_editing = false;
            state.path_input.error = None;
            state.recovery_error = None;
            state.status = Some(format!("saved checkout path: {}", path.display()));
        }
        self.status_bar = Some(format!("saved checkout path: {}", path.display()));
        Ok(())
    }
}

/// Semantic style for a validation error: the screen-level error lives
/// inside the Status panel, not the bottom status bar, so it cannot
/// borrow the global `status_style_for` (which expects the bottom-bar
/// semantics). Keeping the helper here keeps the rendering site
/// self-contained.
fn error_style() -> Style {
    Style::default()
        .fg(Color::Rgb(251, 113, 133))
        .add_modifier(Modifier::BOLD)
}
