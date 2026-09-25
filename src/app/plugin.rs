//! Subagent-panel (plugin) screen: render / open / refresh / handle key /
//! install / uninstall.
//!
//! The screen is driven by `crate::store::plugin_status`, `install_plugin`,
//! and `uninstall_plugin`. The store-level functions live under the
//! `store` module and are imported here under the `_file` alias to keep
//! them distinguishable from the `App` methods of the same name. Every
//! method on `App` declared here is `pub(super)` so the parent `app`
//! module (and its test submodule) can call them while keeping the
//! visibility footprint minimal. `Screen::Plugin` stays in the parent so
//! the global key dispatch and the contextual footer can match on its
//! variant.

use super::{panel, render_popup, App, Screen};
use crate::store::{
    install_plugin as install_plugin_file, plugin_status,
    uninstall_plugin as uninstall_plugin_file, PluginStatus, State,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

impl App {
    pub(super) fn render_plugin(
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

    pub(super) fn open_plugin(&mut self) {
        self.refresh_plugin();
    }

    pub(super) fn refresh_plugin(&mut self) {
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

    pub(super) fn handle_plugin_key(&mut self, key: KeyEvent) {
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

    pub(super) fn install_plugin(&mut self) {
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

    pub(super) fn uninstall_plugin(&mut self) {
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
}
