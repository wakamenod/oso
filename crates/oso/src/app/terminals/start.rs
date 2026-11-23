//! Starting a terminal (ADR 0120): `terminal`'s choice between the last shell running and a
//! new one, where it starts, and what the start thread found, carried out on the UI thread.
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
    /// `terminal`: the last shell terminal still running, or a new one (ADR 0120, Q2). A
    /// hosted session's buffer is never the one it switches to (ADR 0156).
    pub(super) fn open_terminal(&mut self, new: bool) {
        if !new && let Some(last) = self.terminals.map.last_shell() {
            self.editor.show(last);
            self.request_redraw();
            return;
        }
        let cwd = self.terminal_directory();
        let project = self.terminal_project();
        let id = self.editor.add_terminal_buffer(NAME);
        self.terminals.map.add_shell(id);
        // It stands where it starts until its program says otherwise (ADR 0141).
        if let Some(cwd) = &cwd {
            self.editor.set_buffer_directory(id, cwd.clone());
        }
        let size = self.prepare_terminal(id, true);
        let settings = self.terminal_settings();
        let request = Request {
            buffer: id,
            settings,
            cwd,
            project,
            size,
            palette: self.terminal_palette(),
        };
        self.start_shell(request);
    }

    /// Starts the shell `request` asks for, or keeps it until the configuration's first look,
    /// its buffer shown meanwhile (ADR 0173).
    fn start_shell(&mut self, request: Request) {
        if self.starts_wait() {
            self.request_redraw();
            return self.hold_start(move |app| app.start_held_shell(request));
        }
        let id = request.buffer;
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

    /// Starts the shell `request` asked for, which waited, with `[terminal]` as it is in force
    /// now; nothing if its buffer was killed meanwhile.
    fn start_held_shell(&mut self, mut request: Request) {
        let id = request.buffer;
        if self.editor.buffer(id).is_none() || self.terminals.map.get(id).is_none() {
            return;
        }
        request.settings = self.terminal_settings();
        self.start_shell(request);
    }

    /// Shows the terminal buffer `id`, already in the map, in the selected window unless not
    /// `shown`, says it runs, and gives the size its terminal starts at: the selected tiled
    /// window's, also beneath a prompt. One not shown starts behind what the person is doing
    /// (ADR 0144): no window owns its size, and it is not the one selected last, until a window
    /// is selected on it.
    pub(in crate::app) fn prepare_terminal(&mut self, id: BufferId, shown: bool) -> Size {
        let window = self.editor.tiled().0.id();
        if shown {
            self.editor.show(id);
            self.terminals.map.selected(id, window);
        } else {
            self.terminals.map.behind(id);
        }
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
        size
    }

    /// Gives the terminal buffer `buffer` its started `terminal`, and sends it what was typed
    /// while it started. Returns `false`, dropping `terminal`, which hangs it up, if its
    /// buffer went meanwhile.
    pub(in crate::app) fn install_terminal(
        &mut self,
        buffer: BufferId,
        terminal: oso_term::Terminal,
        passed_over: &[String],
    ) -> bool {
        let Some(hosted) = self.terminals.map.get_mut(buffer) else {
            return false;
        };
        // A theme switched while it started is the one it answers colour queries from.
        terminal.set_palette(palette_of(&self.terminals.palettes, self.editor.theme()));
        hosted.install(terminal);
        let waiting = std::mem::take(&mut hosted.waiting);
        // Its first snapshot may be in flight already, its wake spent before it was installed.
        self.take_terminal_soon(buffer);
        self.editor.set_running(buffer, true);
        for send in waiting {
            self.terminal_send(buffer, send);
        }
        for passed in passed_over {
            self.editor
                .note(format!("Terminal shell passed over: {passed}"));
        }
        true
    }

    /// `[terminal]`'s settings in force, for the next terminal started.
    pub(in crate::app) fn terminal_settings(&self) -> shell::Settings {
        let configured = self.configuration.terminal();
        let defaults = shell::Settings::default();
        shell::Settings {
            shell: configured.shell.clone(),
            login: configured.login.unwrap_or(defaults.login),
            env: configured.env.clone().unwrap_or(defaults.env),
            scrollback: configured.scrollback.unwrap_or(defaults.scrollback),
        }
    }

    /// Where a new terminal starts: [`App::buffer_directory`], else the home directory
    /// (ADR 0120, Q18).
    fn terminal_directory(&self) -> Option<PathBuf> {
        self.buffer_directory()
            .or_else(|| self.editor.home().map(std::path::Path::to_path_buf))
    }

    /// Where a new terminal starts, and a new session (ADR 0121), from the selected buffer:
    /// its project root, else the directory it stands in (ADR 0141). From a terminal that is
    /// its own, where it started, as what its program says of its directory (OSC 7) never
    /// chooses where a process starts.
    pub(in crate::app) fn buffer_directory(&self) -> Option<PathBuf> {
        if let Some(root) = self.terminal_project() {
            return Some(root);
        }
        let (window, buffer) = self.editor.current();
        if buffer.is_terminal() && !buffer.hosts_session() {
            return buffer.directory().map(std::path::Path::to_path_buf);
        }
        Some(self.editor.default_directory(window.buffer()))
    }

    fn terminal_project(&self) -> Option<PathBuf> {
        let project = self.editor.current().1.project()?;
        Some(self.editor.project(project)?.root().to_path_buf())
    }

    /// The theme's colours as a terminal's engine takes them.
    pub(in crate::app) fn terminal_palette(&self) -> oso_term::Palette {
        palette_of(&self.terminals.palettes, self.editor.theme())
    }

    /// The start thread found the shell and started the terminal, or could not.
    pub(in crate::app) fn terminal_started(&mut self, started: Started) {
        let Started { buffer, outcome } = started;
        if self.terminals.map.get(buffer).is_none() {
            // Its buffer went while it started: dropping it hangs it up.
            return;
        }
        match outcome {
            Ok(launched) => {
                if !self.install_terminal(buffer, launched.terminal, &launched.passed_over) {
                    return;
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

    pub(in crate::app) fn terminal_failed(&mut self, buffer: BufferId, why: &str) {
        if let Some(hosted) = self.terminals.map.get_mut(buffer) {
            hosted.ended = Some("failed".to_owned());
        }
        self.editor.set_running(buffer, false);
        self.editor
            .message(format!("The terminal could not start: {why}"));
    }
}
