//! Starting a terminal (ADR 0120): `terminal`'s choice between the last one running and a new
//! one, where it starts, and what the start thread found, carried out on the UI thread.
//!
//! Must not depend on: how a frame is drawn, beyond the size of a cell.

use std::path::PathBuf;

use oso_core::buffer::BufferId;
use oso_platform::process::pty::Size;

use super::{NAME, palette_of};
use crate::app::{App, UserEvent};
use crate::terminal::hosted::{self, Request, Started};
use crate::terminal::shell;

impl App {
    /// `terminal`: the last terminal still running, or a new one (ADR 0120, Q2).
    pub(super) fn open_terminal(&mut self, new: bool) {
        if !new && let Some(last) = self.terminals.map.last_running() {
            self.editor.show(last);
            self.request_redraw();
            return;
        }
        let cwd = self.terminal_directory();
        let project = self.terminal_project();
        let id = self.editor.add_terminal_buffer(NAME);
        self.editor.show(id);
        let window = self.editor.window().id();
        self.terminals.map.add(id);
        self.terminals.map.selected(id, window);
        // From now on, until it ends or fails: a restart or an exit started before the shell
        // is found would end the child about to start.
        self.editor.set_running(id, true);
        let size = self.window_size(window).unwrap_or(Size {
            rows: 24,
            columns: 80,
            width: 0,
            height: 0,
        });
        if let Some(hosted) = self.terminals.map.get_mut(id) {
            hosted.sized = Some(size);
        }
        let configured = self.configuration.terminal();
        let defaults = shell::Settings::default();
        let settings = shell::Settings {
            shell: configured.shell.clone(),
            login: configured.login.unwrap_or(defaults.login),
            env: configured.env.clone().unwrap_or(defaults.env),
            scrollback: configured.scrollback.unwrap_or(defaults.scrollback),
        };
        let request = Request {
            buffer: id,
            settings,
            cwd,
            project,
            size,
            palette: self.terminal_palette(),
        };
        let started = self.waker.clone();
        let ready = self.waker.clone();
        let outcome = hosted::start(
            request,
            Box::new(move |started_terminal| {
                started.send(UserEvent::TerminalStarted(Box::new(started_terminal)));
            }),
            Box::new(move || {
                ready.send(UserEvent::Terminal(id));
            }),
        );
        if let Err(error) = outcome {
            self.terminal_failed(id, &error.to_string());
        }
        self.request_redraw();
    }

    /// Where a new terminal starts: the selected buffer's project root, else its file's
    /// directory, else the home directory (ADR 0120, Q18).
    fn terminal_directory(&self) -> Option<PathBuf> {
        if let Some(root) = self.terminal_project() {
            return Some(root);
        }
        let buffer = self.editor.current().1;
        buffer
            .path()
            .and_then(|path| path.parent())
            .map(std::path::Path::to_path_buf)
            .or_else(|| self.editor.home().map(std::path::Path::to_path_buf))
    }

    fn terminal_project(&self) -> Option<PathBuf> {
        let project = self.editor.current().1.project()?;
        Some(self.editor.project(project)?.root().to_path_buf())
    }

    /// The theme's colours as a terminal's engine takes them.
    fn terminal_palette(&self) -> oso_term::Palette {
        palette_of(&self.terminals.palettes, self.editor.theme())
    }

    /// The start thread found the shell and started the terminal, or could not.
    pub(in crate::app) fn terminal_started(&mut self, started: Started) {
        let Started { buffer, outcome } = started;
        let Some(hosted) = self.terminals.map.get_mut(buffer) else {
            // Its buffer went while it started: dropping it hangs it up.
            return;
        };
        match outcome {
            Ok(launched) => {
                // A theme switched while it started is the one it answers colour queries from.
                launched
                    .terminal
                    .set_palette(palette_of(&self.terminals.palettes, self.editor.theme()));
                hosted.terminal = Some(launched.terminal);
                let waiting = std::mem::take(&mut hosted.waiting);
                self.editor.set_running(buffer, true);
                for send in waiting {
                    self.terminal_send(buffer, send);
                }
                for passed in &launched.passed_over {
                    self.editor
                        .note(format!("Terminal shell passed over: {passed}"));
                }
                let dir = (launched.cwd.as_ref())
                    .map_or_else(String::new, |dir| format!(" in {}", dir.display()));
                // The path only: the environment's values are never shown (ADR 0120).
                self.editor
                    .note(format!("Started {}{dir}", launched.shell.display()));
            }
            Err(error) => self.terminal_failed(buffer, &error.to_string()),
        }
        self.sync_terminals();
        self.request_redraw();
    }

    fn terminal_failed(&mut self, buffer: BufferId, why: &str) {
        if let Some(hosted) = self.terminals.map.get_mut(buffer) {
            hosted.ended = Some("failed".to_owned());
        }
        self.editor.set_running(buffer, false);
        self.editor
            .message(format!("The terminal could not start: {why}"));
    }
}
