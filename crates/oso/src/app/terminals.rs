//! Terminals in the app (ADR 0120): what commands and keys ask of them, starting one, the
//! snapshots their threads send, at most one per terminal per frame, the size each PTY takes,
//! and the focus its program is told. Nothing here reads the PTY, parses or blocks: a
//! terminal's thread does that, and the shell is found on a thread of its own.
//!
//! Must not depend on: how a frame is drawn, beyond the size of a cell.

use std::path::PathBuf;

use oso_core::buffer::BufferId;
use oso_core::editor::TerminalRequest;
use oso_core::key::KeyPress;
use oso_platform::process::pty::Size;
use oso_term::{Event, QueueFull, Scroll};

use super::{App, UserEvent};

#[cfg(feature = "dev")]
mod dev_state;
mod ime;
use crate::dispatch::TerminalSend;
use crate::terminal::hosted::{self, Request, Started, Terminals};
use crate::terminal::{colors, paste, shell};

/// The name of a terminal's buffer; others are `*terminal*<2>` and on.
const NAME: &str = "*terminal*";

/// The terminals the app hosts, and what they share.
pub(super) struct Host {
    pub(super) map: Terminals,
    /// Each theme's colours for a terminal, by the renderer's index: made once.
    pub(super) palettes: Vec<oso_render::theme::TerminalPalette>,
    /// Whether oso's window has the keyboard's focus, for the programs that ask (mode 1004).
    pub(super) focused: bool,
}

impl Host {
    pub(super) fn new() -> Host {
        Host {
            map: Terminals::default(),
            palettes: oso_render::theme::themes()
                .iter()
                .map(oso_render::theme::Theme::terminal_palette)
                .collect(),
            focused: true,
        }
    }
}

impl App {
    /// After each event: carries out what commands and keys asked of terminals, drops the
    /// terminals of buffers that are gone, and keeps each one's size, focus and modeline.
    pub(super) fn sync_terminals(&mut self) {
        // What commands asked is bound to the terminal it was asked in first; only starting
        // one is left in the editor.
        let sends = self.dispatcher.take_terminal_sends(&mut self.editor);
        for request in self.editor.take_terminal_requests() {
            if let TerminalRequest::Open { new } = request {
                self.open_terminal(new);
            }
        }
        for (buffer, send) in sends {
            self.terminal_send(buffer, send);
        }
        if self.terminals.map.is_empty() {
            return;
        }
        self.drop_unasked_pastes();
        let editor = &self.editor;
        for gone in self.terminals.map.retain(|id| editor.buffer(id).is_some()) {
            // Dropping its handle hangs it up; its thread finishes on its own.
            drop(gone);
        }
        let editor = &self.editor;
        self.dispatcher
            .forget_terminals(|id| editor.buffer(id).is_some());
        self.note_terminal_selected();
        self.size_terminals();
        self.focus_terminals();
        if self.note_terminals() {
            self.request_redraw();
        }
    }

    fn terminal_send(&mut self, buffer: BufferId, send: TerminalSend) {
        match send {
            TerminalSend::Keys(keys) => self.send_keys(buffer, &keys),
            TerminalSend::Text(text) => self.send_text(buffer, &text),
            TerminalSend::Paste(text) => self.paste(buffer, text),
            TerminalSend::PasteLines => {
                let text = self
                    .terminals
                    .map
                    .get_mut(buffer)
                    .and_then(|hosted| hosted.pending_paste.take());
                if let Some((_, text)) = text {
                    self.send_paste(buffer, text, true);
                }
            }
        }
    }

    /// Drops a paste of lines whose question closed without a yes: a later
    /// `terminal-paste-lines` never pastes it unasked.
    fn drop_unasked_pastes(&mut self) {
        let prompts = self.editor.prompts();
        for (_, hosted) in self.terminals.map.iter_mut() {
            if hosted
                .pending_paste
                .as_ref()
                .is_some_and(|(asked, _)| !prompts.iter().any(|prompt| prompt.id() == *asked))
            {
                hosted.pending_paste = None;
            }
        }
    }

    /// `terminal`: the last terminal still running, or a new one (ADR 0120, Q2).
    fn open_terminal(&mut self, new: bool) {
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
    pub(super) fn terminal_started(&mut self, started: Started) {
        let Started { buffer, outcome } = started;
        let Some(hosted) = self.terminals.map.get_mut(buffer) else {
            // Its buffer went while it started: dropping it hangs it up.
            return;
        };
        match outcome {
            Ok(launched) => {
                hosted.terminal = Some(launched.terminal);
                self.editor.set_running(buffer, true);
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

    /// A terminal's thread has a snapshot ready: the next frame takes it.
    /// A terminal no window shows asks for no frame of its own: its snapshot waits for the
    /// next frame drawn anyway, while its thread goes on parsing.
    pub(super) fn terminal_ready(&mut self, buffer: BufferId) {
        if let Some(hosted) = self.terminals.map.get_mut(buffer) {
            hosted.ready = true;
            let shown = (self.editor.frame().windows().iter()).any(|(id, _)| {
                self.editor
                    .window_by_id(*id)
                    .is_some_and(|w| w.buffer() == buffer)
            });
            if shown {
                self.request_redraw();
            }
        }
    }

    /// Before a frame: takes each ready terminal's snapshot, one per terminal, merges its rows
    /// into what the UI keeps and gives its buffers back.
    pub(super) fn take_terminal_snapshots(&mut self) {
        let mut ended = Vec::new();
        let mut said = Vec::new();
        let mut again = false;
        for (id, hosted) in self.terminals.map.iter_mut() {
            if !hosted.ready {
                continue;
            }
            let Some(terminal) = &hosted.terminal else {
                hosted.ready = false;
                continue;
            };
            let Some(mut snapshot) = terminal.take_snapshot() else {
                hosted.ready = false;
                continue;
            };
            // Another may wait already, as the end does behind one in flight: the next frame
            // looks again, so that this one takes one only.
            again = true;
            hosted.grid.apply(&snapshot);
            for event in snapshot.events.drain(..) {
                match event {
                    Event::Title(title) => {
                        hosted.title = title;
                        hosted.note_stale = true;
                    }
                    Event::Ended(end) => {
                        hosted.ended = Some(hosted::ended(&end));
                        hosted.note_stale = true;
                        ended.push(id);
                    }
                    Event::PasteRefused { lines } => said.push(format!(
                        "{lines} lines not pasted: the program turned bracketed paste off"
                    )),
                    // Step 6 and after (ADR 0120).
                    Event::Bell
                    | Event::Directory(_)
                    | Event::Notification { .. }
                    | Event::Progress(_)
                    | Event::Prompt(_)
                    | Event::ClipboardWrite(_) => {}
                }
            }
            if hosted.ended.is_some() {
                // Nothing follows the end; its handle goes.
                hosted.terminal = None;
            } else {
                terminal.recycle(snapshot);
            }
        }
        for id in ended {
            self.editor.set_running(id, false);
        }
        for text in said {
            self.editor.message(text);
        }
        // The frame being drawn shows the notes.
        self.note_terminals();
        if again {
            self.request_redraw();
        }
    }

    fn send_keys(&mut self, buffer: BufferId, keys: &[KeyPress]) {
        let Some(terminal) = self
            .terminals
            .map
            .get(buffer)
            .and_then(|hosted| hosted.terminal.as_ref())
        else {
            return self.no_program(buffer);
        };
        for key in keys {
            if terminal.send_key(*key) == Err(QueueFull) {
                self.editor
                    .message("The terminal's input is full; key dropped");
                return;
            }
        }
    }

    /// Text the input method committed, or a key that types several characters, for the
    /// terminal `buffer`'s program, as UTF-8.
    fn send_text(&mut self, buffer: BufferId, text: &str) {
        let Some(terminal) = self
            .terminals
            .map
            .get(buffer)
            .and_then(|hosted| hosted.terminal.as_ref())
        else {
            return self.no_program(buffer);
        };
        if terminal.send(text.as_bytes().to_vec()).is_err() {
            self.editor
                .message("The terminal's input is full; text dropped");
        }
    }

    fn no_program(&mut self, buffer: BufferId) {
        let said = match self.terminals.map.get(buffer) {
            Some(hosted) if hosted.ended.is_some() => "No program runs in this terminal",
            Some(_) => "The terminal is starting",
            None => return,
        };
        self.editor.message(said);
    }

    /// `C-y` in a terminal: with bracketed paste off, text of several lines asks first
    /// (ADR 0120, Q13).
    fn paste(&mut self, buffer: BufferId, text: String) {
        let Some(hosted) = self.terminals.map.get_mut(buffer) else {
            return;
        };
        if paste::breaks(&text) && !hosted.grid.modes().bracketed_paste {
            let lines = paste::lines(&text);
            let question = format!("Paste {lines} lines into a program that may run them?");
            match self
                .editor
                .ask_with(question, "terminal-paste-lines", ["paste", "keep"])
            {
                Ok(asked) => {
                    if let Some(hosted) = self.terminals.map.get_mut(buffer) {
                        hosted.pending_paste = Some((asked, text));
                    }
                }
                Err(error) => self.editor.message(error.to_string()),
            }
            self.request_redraw();
            return;
        }
        self.send_paste(buffer, text, false);
    }

    fn send_paste(&mut self, buffer: BufferId, text: String, lines_allowed: bool) {
        let Some(terminal) = self
            .terminals
            .map
            .get(buffer)
            .and_then(|hosted| hosted.terminal.as_ref())
        else {
            return self.no_program(buffer);
        };
        if terminal.paste(text, lines_allowed).is_err() {
            self.editor
                .message("The terminal's input is full; nothing pasted");
        }
    }

    /// The wheel over a terminal scrolls its scrollback, `rows` down. Returns whether the
    /// window shows a terminal.
    pub(super) fn wheel_terminal(
        &mut self,
        window: oso_core::window::WindowId,
        rows: isize,
    ) -> bool {
        let Some(buffer) = self.editor.window_by_id(window).map(|w| w.buffer()) else {
            return false;
        };
        let Some(hosted) = self.terminals.map.get(buffer) else {
            return false;
        };
        if let Some(terminal) = &hosted.terminal
            && rows != 0
        {
            terminal.scroll(Scroll::Lines(rows));
        }
        true
    }

    /// The window selected now owns its terminal's size (ADR 0120).
    fn note_terminal_selected(&mut self) {
        let (window, buffer) = {
            let window = self.editor.tiled().0;
            (window.id(), window.buffer())
        };
        if self
            .terminals
            .map
            .get(buffer)
            .is_some_and(|hosted| hosted.owner != Some(window))
        {
            self.terminals.map.selected(buffer, window);
        }
    }

    /// The size, in cells and pixels, of `window`'s text.
    fn window_size(&self, window: oso_core::window::WindowId) -> Option<Size> {
        let regions = self.editor.frame().regions(window)?;
        let text = regions.text;
        let metrics = self.view.as_ref()?.render_state.cell_metrics();
        let cells = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
        // Whole pixels, as the grid has them; past `u16` the size only says less.
        let pixels = |n: usize, cell: f32| {
            let n = u16::try_from(n).unwrap_or(u16::MAX);
            let total = (f32::from(n) * cell).round();
            if total >= f32::from(u16::MAX) {
                u16::MAX
            } else if total > 0.0 {
                // In range and whole: the cast loses nothing.
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "checked above"
                )]
                let total = total as u16;
                total
            } else {
                0
            }
        };
        Some(Size {
            rows: cells(text.rows),
            columns: cells(text.columns),
            width: pixels(text.columns, metrics.width),
            height: pixels(text.rows, metrics.height),
        })
    }

    /// Gives each PTY the size of the window that last selected it, when that changed.
    fn size_terminals(&mut self) {
        let sizes: Vec<(BufferId, Size)> = self
            .terminals
            .map
            .iter()
            .filter_map(|(id, hosted)| {
                let window = hosted.owner?;
                // A window gone, or showing another buffer, leaves the size as it was.
                let shows = self.editor.window_by_id(window)?.buffer() == id;
                let size = shows.then(|| self.window_size(window))??;
                (hosted.sized != Some(size)).then_some((id, size))
            })
            .collect();
        for (id, size) in sizes {
            if let Some(hosted) = self.terminals.map.get_mut(id) {
                hosted.sized = Some(size);
                if let Some(terminal) = &hosted.terminal {
                    terminal.resize(size);
                }
            }
        }
    }

    /// Tells each program whether its terminal has the focus: shown in the selected window,
    /// with oso's window focused (mode 1004; the terminal's thread sends only when asked).
    fn focus_terminals(&mut self) {
        let selected = (self.editor.prompt().is_none() && self.terminals.focused)
            .then(|| self.editor.tiled().0.buffer());
        for (id, hosted) in self.terminals.map.iter_mut() {
            let focused = selected == Some(id);
            if hosted.focused != Some(focused)
                && let Some(terminal) = &hosted.terminal
            {
                terminal.set_focused(focused);
                hosted.focused = Some(focused);
            }
        }
    }

    /// Puts each terminal's mode, end and title in its modeline, when they changed.
    /// Puts each terminal's mode, end and title in its modeline, when one of them changed.
    /// Returns whether a note changed, which the frame being drawn already shows.
    fn note_terminals(&mut self) -> bool {
        let mut changed = false;
        let ids: Vec<BufferId> = self.terminals.map.iter().map(|(id, _)| id).collect();
        for id in ids {
            let input = self.dispatcher.terminal_input(id);
            let Some(hosted) = self.terminals.map.get_mut(id) else {
                continue;
            };
            if !hosted.note_stale && hosted.noted_input == input {
                continue;
            }
            hosted.note_stale = false;
            hosted.noted_input = input;
            let note = hosted.note_text(input);
            if note != hosted.note {
                hosted.note.clone_from(&note);
                self.editor.set_mode_note(id, note);
                changed = true;
            }
        }
        changed
    }
}

/// Theme `index`'s colours as a terminal's engine takes them.
fn palette_of(palettes: &[oso_render::theme::TerminalPalette], index: usize) -> oso_term::Palette {
    let theme = palettes
        .get(index)
        .or_else(|| palettes.first())
        .copied()
        .unwrap_or(colors::FALLBACK);
    colors::palette(&theme)
}

/// After a switch to theme `index`: each terminal answers colour queries from it.
pub(super) fn retheme(
    terminals: &Terminals,
    palettes: &[oso_render::theme::TerminalPalette],
    index: usize,
) {
    let palette = palette_of(palettes, index);
    for (_, hosted) in terminals.iter() {
        if let Some(terminal) = &hosted.terminal {
            terminal.set_palette(palette);
        }
    }
}
