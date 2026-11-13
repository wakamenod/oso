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
use oso_core::window::WindowId;

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
mod greps;
#[cfg(feature = "dev")]
mod handover;
mod ime;
mod input;
mod jumps;
mod loads;
mod log_gate;
mod migemo;
mod projects;
mod reloads;
mod reviews;
mod saves;
mod searches;
mod sessions;
mod slices;
mod syntax;
mod terminals;
mod theme_file;
mod trims;
mod walks;
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
    /// A thread read the EditorConfig of a file `C-x C-w` named (ADR 0115).
    EditorConfig,
    /// The walker found a project's files, or ended (ADR 0114).
    Walked(crate::walk::Walked),
    /// A project's search found rows, or ended (ADR 0114).
    Grepped(crate::grep::Found),
    /// The lines around a search's row were read for its preview, or could not be (ADR 0114).
    Previewed(crate::grep::Previewed),
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
    /// The sessions worker found something (ADR 0121).
    Sessions(crate::sessions::worker::Out),
    /// A hosted session's start thread found something (ADR 0121).
    SessionStart(crate::sessions::start::Report),
    /// A look for a session's changes after its turn ended: whether its tree differs from its
    /// baseline, `None` if git failed (ADR 0121 step 3).
    SessionChanges(crate::sessions::worker::SessionKey, Option<bool>),
    /// A worktree job found something (ADR 0121 step 4).
    Worktree(Box<crate::sessions::worktree::jobs::Report>),
    /// The thread that reads migemo's dictionary read it, or could not (ADR 0116).
    Migemo(Box<migemo::DictionaryRead>),
    /// A review's threads, or the review menu's counts, reported (ADR 0122).
    Review(reviews::ReviewEvent),
    /// A terminal's thread has a snapshot ready (ADR 0120).
    Terminal(oso_core::buffer::BufferId),
    /// A terminal's start thread found the shell and started it, or could not.
    TerminalStarted(Box<crate::terminal::hosted::Started>),
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
    wheel: input::Wheeling,
    /// Wheel and trackpad scrolling held while `!` or an undo goes on, in rows and columns, a
    /// sum for each window, which scrolls after the keys held with it (ADR 0119).
    held_wheel: Vec<(WindowId, isize, isize)>,
    /// When work over turns last drew a frame (ADR 0119).
    pace: slices::Pace,
    /// What each tiled window's modeline and the echo area said when a frame was last drawn
    /// or asked for, and room to compare it with what they would say now (ADR 0112).
    statuses: Vec<(WindowId, Status)>,
    next_statuses: Vec<(WindowId, Status)>,
    /// What oso has told the OS about the input method (ADR 0069).
    ime: ime::ImeState,
    /// The OS clipboard's threads, or none if they could not start (ADR 0074).
    clipboard: Option<oso_platform::clipboard::Clipboard>,
    /// isearch's worker (ADR 0082).
    searcher: searches::Searcher,
    /// The thread that reads migemo's dictionary, and what became of it (ADR 0116).
    migemo_reader: migemo::MigemoReader,
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
    /// The home directory, after which no EditorConfig is read (ADR 0115).
    home: crate::loader::Home,
    /// The thread that reads EditorConfig after `C-x C-w` (ADR 0115).
    editorconfigs: crate::loader::EditorConfigReader,
    /// The thread that walks a project's files for `project-find-file` (ADR 0114).
    walker: crate::walk::Walker,
    /// When the rows a walk sent are added if it sends nothing more.
    quiet_walk: walks::Quiet,
    /// The threads that search a project's files for `project-search` (ADR 0114).
    grep: crate::grep::Grep,
    /// The thread that reads a search's row's file for its preview (ADR 0114).
    previewer: crate::grep::Previewer,
    /// The thread that reads and writes the record of trusted projects, if there is one.
    trust_file: Option<crate::config::TrustFile>,
    /// The sessions worker, which judges hosted sessions' screens (ADR 0121).
    sessions: Option<crate::sessions::worker::Worker>,
    /// The sessions oso hosts and remembers, and the internal `*Session explain*` buffer
    /// (ADR 0121).
    hosting: sessions::Hosting,
    /// What step 3 of ADR 0121 keeps of hosted sessions: the screen feed, their states, the
    /// `*Sessions*` list, spaces and attention.
    watch: sessions::Watch,
    /// The open reviews' threads and the review menu's counts (ADR 0122).
    reviews: reviews::Reviews,
    /// What keeps the log to warnings and errors around a buffer that may hold secrets
    /// (ADR 0078).
    log_gate: LogGate,
    /// The agent channel, once the person turns it on (ADR 0085).
    agent: agent::Agent,
    /// The cursor flash fading after a large jump (ADR 0097).
    flash: flash::Flash,
    /// The terminals, by their buffers, and what they share (ADR 0120).
    terminals: terminals::Host,
    /// When a jump reading its string shows its labels (ADR 0116).
    jump_timer: jumps::JumpTimer,
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
        let clipboard = clipboard::start(source, waker.clone());
        let disk = reloads::Disk::start(waker.clone());
        let lister = files::lister(&waker);
        let projects = projects::detector(&waker);
        let (home_dir, editorconfigs) = projects::editorconfig_reader(&waker);
        let walker = walks::walker(&waker);
        let grep = greps::grep(&waker);
        let previewer = greps::previewer(&waker);
        let sessions = sessions::worker(&waker, theme_directory.as_deref());
        let mut editor = starting_editor();
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
            leaving: Leaving::with_cell(emergency.agreement_cell()),
            offer: emergency.offer(),
            emergency,
            answering: None,
            scan_for_copies: false,
            started: Some(started),
            home: home_dir,
            editorconfigs,
            wheel: input::Wheeling::default(),
            held_wheel: Vec::new(),
            pace: slices::Pace::default(),
            statuses: Vec::new(),
            next_statuses: Vec::new(),
            ime: ime::ImeState::default(),
            clipboard,
            searcher: searches::Searcher::default(),
            migemo_reader: migemo::MigemoReader::default(),
            wrapper: wraps::Wrapper::default(),
            syntax: syntax::Syntax::default(),
            completer: completions::Completer::default(),
            theme_read: Some(theme_read),
            hosting: sessions::Hosting::new(theme_directory.clone()),
            watch: sessions::Watch::default(),
            theme_writer: theme_file::Writer::start(theme_directory),
            themes,
            remembered_theme,
            lister,
            projects,
            walker,
            quiet_walk: walks::Quiet::default(),
            grep,
            previewer,
            trust_file: None,
            sessions,
            reviews: reviews::Reviews::default(),
            log_gate,
            agent: agent::Agent::default(),
            flash: flash::Flash::default(),
            terminals: terminals::Host::new(),
            jump_timer: jumps::JumpTimer::default(),
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
        // A command that reads a branch waits for them as a yank does (ADR 0121).
        let effects = effects.union(self.sync_branch_wait());
        let input = self.flash_after_input();
        self.time_jump(input);
        self.forget_killed();
        if let Some(freed) = self.editor.take_crossing_freed() {
            self.freer.free(freed);
        }
        self.serve_files();
        self.forget_listings();
        self.start_walks();
        self.start_project_search();
        self.start_saves();
        self.start_trims();
        self.start_searches();
        self.start_reverts();
        self.sync_reviews();
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

/// An editor showing `*scratch*`, which knows the home directory and where oso started. Each
/// is read once: `~` stands for the home, and a path typed in a buffer with no file starts
/// in the start directory.
fn starting_editor() -> Editor {
    let mut editor = Editor::default();
    editor.set_home(home());
    if let Ok(directory) = std::env::current_dir() {
        editor.set_start_directory(directory);
    }
    editor
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
