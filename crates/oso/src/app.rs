//! The application: the window, the event loop handler and the glue
//! between them and the editor (ADR 0004). It turns the window's events into key
//! presses, wheel scrolling and frames, starts file loads and saves and takes
//! in what they report. This file holds the state and what the concerns share;
//! each concern has a module, and `shell` alone knows the window system (ADR 0070).
//!
//! Must not depend on: wgpu or OS bindings. Editor logic belongs in oso-core
//! and commands, not here.

use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::command::Effects;
use oso_core::editor::{Editor, Status};
use oso_core::window::WheelRemainder;

mod agent;
mod clipboard;
mod completions;
mod configuration;
#[cfg(feature = "dev")]
mod control;
mod copies;
mod draw;
mod events;
mod files;
mod flash;
#[cfg(feature = "dev")]
mod handover;
mod ime;
mod input;
mod loads;
mod log_gate;
mod projects;
mod reloads;
mod saves;
mod searches;
mod syntax;
mod theme_file;
mod trims;
mod wraps;
// The window system: oso's own layer on macOS, and winit on Windows and Linux (ADR 0070,
// stage 3).
// Both give the same loop, window and waker, under the one name.
#[cfg(target_os = "macos")]
mod native;
#[cfg(not(target_os = "macos"))]
mod shell;
#[cfg(target_os = "macos")]
use native as shell;

#[cfg(feature = "dev")]
use crate::dev;
use crate::dispatch::Dispatcher;
use crate::emergency::{CopiesEvent, Emergency, Offer};
use crate::saver::{Leaving, SaveReport};
use draw::View;
use loads::Load;
pub use log_gate::LogGate;
use oso_platform::window::Held;
use shell::Loop;
pub use shell::{Waker, run};
pub use theme_file::ThemeRead;

/// Events sent to the event loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// A loading thread has an update for this buffer.
    Load(BufferId),
    /// A saving thread has ended.
    Save(SaveReport),
    /// The watcher saw open files change on disk (ADR 0083).
    Watch,
    /// The configuration's thread has found something new in the file (ADR 0108).
    Configuration,
    /// No emergency copy could be written, for this reason (ADR 0062).
    NoEmergencyCopies(String),
    /// A thread working on the emergency copies on offer has something to say (ADR 0064).
    Copies(CopiesEvent),
    /// The clipboard's threads wrote or read (ADR 0074).
    Clipboard(clipboard::ClipboardEvent),
    /// A whitespace scan ended (ADR 0113).
    Trim(oso_core::editor::TrimReport),
    /// isearch's worker found or counted something (ADR 0082).
    Search(oso_core::editor::SearchReport),
    /// Soft wrap's worker wrapped a window's rows (ADR 0100).
    Wrap(oso_core::display_map::wrap_map::WrapReport),
    /// A wrap job panicked, so its window asks for it again.
    WrapLost,
    /// The syntax worker highlighted a buffer, or stopped (ADR 0111).
    Syntax(oso_syntax::Report<BufferId>),
    /// The completion worker built a list or matched an input (ADR 0096).
    Completion(oso_core::complete::CompletionReport),
    /// A completion job panicked on the worker.
    CompletionFailed,
    /// The lister answered a prompt that reads a path (ADR 0087).
    Listed,
    /// The thread that finds projects answered (ADR 0114).
    Projects,
    /// The record of trusted projects was read, or written and read back (ADR 0114).
    Trust,
    /// A control socket request, in dev builds.
    #[cfg(feature = "dev")]
    Control(dev::UiRequest),
    /// Something from a connection of the agent channel (ADR 0085).
    Agent(crate::channel::Request),
    /// A deadline of the agent channel's came: a file it opens is dropped once it passes.
    AgentDeadline,
    /// The agent channel started, or could not, off the UI thread.
    AgentStarted(std::io::Result<crate::channel::Channel>),
}

/// Top-level application state.
///
/// - `view`: the window and what draws into it.
/// - `editor`: the editor state that commands operate on.
/// - `dispatcher`: keymaps and commands, which turn keys into changes.
/// - `modifiers`: the modifier keys held, as the window system last reported them.
/// - `loads`: files loading in the background.
/// - `disk`: what is known of open files on disk.
/// - `dev`: the control socket, in dev builds where it started.
pub struct App {
    view: Option<View>,
    editor: Editor,
    dispatcher: Dispatcher,
    modifiers: Held,
    /// Wakes the event loop from other threads.
    waker: Waker,
    loads: Vec<Load>,
    /// What is known of open files on disk, and the thread that looks at them.
    disk: reloads::Disk,
    /// The user configuration file's thread, and what has been said of the file (ADR 0108).
    configuration: configuration::Configuration,
    /// Drops what the UI thread is done with and would take long to drop: a reloaded file's
    /// text, a replaced user keymap.
    freer: Freer,
    /// An exit waiting for running saves, once this instance has agreed to go.
    leaving: Leaving,
    /// What the panic hook and the end of the event loop write (ADR 0062).
    emergency: Emergency,
    /// The emergency copies on offer (ADR 0064).
    offer: Offer,
    /// A letter pressed as an answer among the copies, whose repeats are dropped until the next
    /// press, wherever the answer took the window.
    answering: Option<oso_core::key::KeyPress>,
    /// A file has loaded, so copies may belong to it. Asked for here, as a load's end can
    /// come on the frame path, and looked into before the event loop waits.
    scan_for_copies: bool,
    /// When the process started, until the first frame is presented.
    started: Option<Instant>,
    /// Wheel and trackpad scrolling not yet worth a whole row or column.
    wheel: WheelRemainder,
    /// What the modeline and the echo area said when a frame was last drawn or asked
    /// for, and room to compare it with what they would say now.
    status: Status,
    next_status: Status,
    /// What oso has told the OS about the input method (ADR 0069).
    ime: ime::ImeState,
    /// The OS clipboard's threads, or none if they could not start (ADR 0074).
    clipboard: Option<oso_platform::clipboard::Clipboard>,
    /// isearch's worker (ADR 0082).
    searcher: searches::Searcher,
    /// Soft wrap's worker (ADR 0100).
    wrapper: wraps::Wrapper,
    /// Syntax highlighting's worker (ADR 0111).
    syntax: syntax::Syntax,
    /// The completion worker, for a list past 5000 candidates (ADR 0096).
    completer: completions::Completer,
    /// The theme remembered from before, being read until the window opens, what writes the
    /// theme shown last, and which one it was given, with the configuration's default theme
    /// then (ADR 0089, ADR 0108).
    theme_read: Option<ThemeRead>,
    theme_writer: theme_file::Writer,
    /// The renderer's themes, each a name and what it is, the default first: made once, as
    /// making them resolves every theme's colours.
    themes: Vec<(&'static str, &'static str)>,
    remembered_theme: ShownTheme,
    /// The threads that list directories for a prompt that reads a path (ADR 0087).
    lister: crate::lister::Lister,
    /// The thread that finds each file's project (ADR 0114).
    projects: crate::project::Detector,
    /// The thread that reads and writes the record of trusted projects, if there is one.
    trust_file: Option<crate::config::TrustFile>,
    /// What keeps the log to warnings and errors around a buffer that may hold secrets
    /// (ADR 0078).
    log_gate: LogGate,
    /// The agent channel, once the person turns it on (ADR 0085).
    agent: agent::Agent,
    /// The cursor flash fading after a large jump (ADR 0097).
    flash: flash::Flash,
    #[cfg(feature = "dev")]
    dev: Option<dev::DevState>,
    /// Key latency, startup and load times, for benchmarks (ADR 0025).
    #[cfg(feature = "dev")]
    timings: dev::Timings,
    /// Questions a control socket request opened, which keys from the window do not answer.
    #[cfg(feature = "dev")]
    remote_questions: Vec<oso_core::minibuffer::PromptId>,
    /// What a restart under `oso dev` keeps: positions still to restore, the latest export,
    /// and what an import has brought so far.
    #[cfg(feature = "dev")]
    handover: handover::Handover,
}

/// A theme shown, by its index among the renderer's, and the configuration's default theme
/// then (ADR 0108).
struct ShownTheme {
    index: usize,
    default: String,
}

/// The thread that drops what the UI thread is done with, or none if it could not start.
struct Freer(Option<std::sync::mpsc::Sender<Box<dyn Send>>>);

impl Freer {
    fn start() -> Self {
        let (freer, freed) = std::sync::mpsc::channel::<Box<dyn Send>>();
        // Freeing a large file's text can take longer than a frame, so it is done there.
        let started = std::thread::Builder::new()
            .name("oso-free".into())
            .spawn(move || freed.iter().for_each(drop))
            .is_ok();
        Self(started.then_some(freer))
    }

    /// Drops `value` on the freeing thread, or here if it could not start.
    fn free<T: Send + 'static>(&self, value: T) {
        if let Some(freer) = &self.0 {
            // A thread that has gone hands it back in the error, and it is dropped here.
            drop(freer.send(Box::new(value)));
        }
    }
}

/// The home directory as the environment names it: `HOME`, and `USERPROFILE` on Windows. It is
/// what `~` stands for, and never a project (ADR 0114).
fn home() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let name = "USERPROFILE";
    #[cfg(not(windows))]
    let name = "HOME";
    std::env::var_os(name).map(std::path::PathBuf::from)
}

impl App {
    /// An app with no window yet, showing `*scratch*`, with the clipboard of `source`.
    /// Loading threads and the clipboard report through `waker`, and `log_gate` is the
    /// installed logger's. It writes the theme chosen in `theme_directory`, and draws in the
    /// one `theme_read` brings, if it has by the time the window opens (ADR 0089).
    pub fn new(
        waker: Waker,
        started: Instant,
        emergency: Emergency,
        source: oso_platform::clipboard::Source,
        log_gate: LogGate,
        theme_directory: Option<std::path::PathBuf>,
        theme_read: ThemeRead,
    ) -> Self {
        let mut dispatcher = Dispatcher::new();
        dispatcher.set_publisher(emergency.publisher());
        let leaving = Leaving::with_cell(emergency.agreement_cell());
        let offer = emergency.offer();
        let clipboard = clipboard::start(source, waker.clone());
        let disk = reloads::Disk::start(waker.clone());
        let lister = files::lister(&waker);
        let projects = projects::detector(&waker);
        let mut editor = Editor::default();
        // Read once: `~` stands for it, and a path typed in a buffer with no file starts there.
        editor.set_home(home());
        if let Ok(directory) = std::env::current_dir() {
            editor.set_start_directory(directory);
        }
        let themes = theme_names();
        let configuration = configuration::Configuration::new(&themes);
        let remembered_theme = ShownTheme {
            index: 0,
            default: configuration.default_theme().to_owned(),
        };
        editor.set_themes(themes.clone(), 0);
        Self {
            view: None,
            editor,
            dispatcher,
            modifiers: Held::NONE,
            waker,
            loads: Vec::new(),
            disk,
            configuration,
            freer: Freer::start(),
            leaving,
            emergency,
            offer,
            answering: None,
            scan_for_copies: false,
            started: Some(started),
            wheel: WheelRemainder::default(),
            status: Status::default(),
            next_status: Status::default(),
            ime: ime::ImeState::default(),
            clipboard,
            searcher: searches::Searcher::default(),
            wrapper: wraps::Wrapper::default(),
            syntax: syntax::Syntax::default(),
            completer: completions::Completer::default(),
            theme_read: Some(theme_read),
            theme_writer: theme_file::Writer::start(theme_directory),
            themes,
            remembered_theme,
            lister,
            projects,
            trust_file: None,
            log_gate,
            agent: agent::Agent::default(),
            flash: flash::Flash::default(),
            #[cfg(feature = "dev")]
            dev: None,
            #[cfg(feature = "dev")]
            timings: dev::Timings::new(started),
            #[cfg(feature = "dev")]
            remote_questions: Vec::new(),
            #[cfg(feature = "dev")]
            handover: handover::Handover::default(),
        }
    }

    /// Draws in the theme remembered from before, if its read is done and nothing has been
    /// chosen since, and takes the configuration's default theme then as the one in force, for
    /// the file's to be compared with (ADR 0108). A name this build has no theme of leaves the
    /// default.
    fn take_remembered_theme(&mut self) {
        let Some(remembered) = self.theme_read.take().and_then(ThemeRead::take) else {
            return;
        };
        if self.editor.theme() != self.remembered_theme.index {
            return;
        }
        let name = &remembered.name;
        match self.themes.iter().position(|&(theme, _)| theme == name) {
            Some(index) => {
                self.editor.set_themes(self.themes.clone(), index);
                // A file of one line was written before the configuration set the theme.
                if let Some(default) = remembered.default {
                    self.configuration.set_default_theme(default);
                }
                let default = self.configuration.default_theme().to_owned();
                self.remembered_theme = ShownTheme { index, default };
            }
            None => log::warn!("no theme named {name}, as remembered; drawing the default"),
        }
    }

    /// Hands the theme shown now, with the configuration's default theme, to the writer, if
    /// they are not those last remembered: a choice, or a theme the configuration set.
    fn remember_theme(&mut self) {
        let index = self.editor.theme();
        let default = self.configuration.default_theme();
        let remembered = &self.remembered_theme;
        if (index, default) == (remembered.index, remembered.default.as_str()) {
            return;
        }
        self.remembered_theme = ShownTheme {
            index,
            default: default.to_owned(),
        };
        if let Some(name) = self.editor.theme_name() {
            self.theme_writer.remember(theme_file::Remembered {
                name: name.to_owned(),
                default: Some(default.to_owned()),
            });
        }
    }

    /// Answers control socket requests through `dev`.
    #[cfg(feature = "dev")]
    pub fn set_dev(&mut self, dev: Option<dev::DevState>) {
        self.dev = dev;
    }

    /// Does what commands asked for.
    fn apply(&mut self, event_loop: &Loop<'_>, effects: Effects) {
        let effects = effects.union(self.sync_clipboard());
        self.flash_after_input();
        self.forget_killed();
        self.serve_files();
        self.forget_listings();
        self.start_saves();
        self.start_trims();
        self.start_searches();
        self.start_reverts();
        self.answer_about_copies();
        self.remember_theme();
        if effects.redraw {
            self.request_redraw();
        }
        if effects.exit {
            self.leave(event_loop, false);
        }
    }

    fn request_redraw(&self) {
        if let Some(view) = &self.view {
            view.window.request_redraw();
        }
    }

    /// Starts handling an event: the log keeps to warnings and errors if a buffer that may hold
    /// secrets is shown, until the next event (ADR 0078).
    fn begin_logging(&self) {
        self.log_gate.begin(self.editor.shows_secret());
    }

    /// Keeps the log to warnings and errors until the next event if a buffer that may hold
    /// secrets is shown now.
    fn note_logging(&self) {
        self.log_gate.note(self.editor.shows_secret());
    }
}

/// The renderer's themes, each a name and what it is, the default first (ADR 0089).
fn theme_names() -> Vec<(&'static str, &'static str)> {
    oso_render::theme::themes()
        .iter()
        .map(|theme| {
            let kind = match theme.kind() {
                oso_render::theme::Kind::Dark => "dark",
                oso_render::theme::Kind::Light => "light",
            };
            (theme.name(), kind)
        })
        .collect()
}
