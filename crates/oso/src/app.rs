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
#[cfg(feature = "dev")]
mod control;
mod copies;
mod draw;
mod events;
#[cfg(feature = "dev")]
mod handover;
mod ime;
mod input;
mod loads;
mod log_gate;
mod reloads;
mod saves;
mod searches;
// The window system: winit, or oso's own layer on macOS with `native-window` (ADR 0070).
// Both give the same loop, window and waker, under the one name.
#[cfg(all(target_os = "macos", feature = "native-window"))]
mod native;
#[cfg(not(all(target_os = "macos", feature = "native-window")))]
mod shell;
#[cfg(all(target_os = "macos", feature = "native-window"))]
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

/// Events sent to the event loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// A loading thread has an update for this buffer.
    Load(BufferId),
    /// A saving thread has ended.
    Save(SaveReport),
    /// The watcher saw open files change on disk (ADR 0083).
    Watch,
    /// No emergency copy could be written, for this reason (ADR 0062).
    NoEmergencyCopies(String),
    /// A thread working on the emergency copies on offer has something to say (ADR 0064).
    Copies(CopiesEvent),
    /// The clipboard's threads wrote or read (ADR 0074).
    Clipboard(clipboard::ClipboardEvent),
    /// isearch's worker found or counted something (ADR 0082).
    Search(oso_core::editor::SearchReport),
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
    /// What keeps the log to warnings and errors around a buffer that may hold secrets
    /// (ADR 0078).
    log_gate: LogGate,
    /// The agent channel, once the person turns it on (ADR 0085).
    agent: agent::Agent,
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

impl App {
    /// An app with no window yet, showing `*scratch*`, with the clipboard of `source`.
    /// Loading threads and the clipboard report through `waker`, and `log_gate` is the
    /// installed logger's.
    pub fn new(
        waker: Waker,
        started: Instant,
        emergency: Emergency,
        source: oso_platform::clipboard::Source,
        log_gate: LogGate,
    ) -> Self {
        let mut dispatcher = Dispatcher::new();
        dispatcher.set_publisher(emergency.publisher());
        let leaving = Leaving::with_cell(emergency.agreement_cell());
        let offer = emergency.offer();
        let clipboard = clipboard::start(source, waker.clone());
        let disk = reloads::Disk::start(waker.clone());
        Self {
            view: None,
            editor: Editor::default(),
            dispatcher,
            modifiers: Held::NONE,
            waker,
            loads: Vec::new(),
            disk,
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
            log_gate,
            agent: agent::Agent::default(),
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

    /// Answers control socket requests through `dev`.
    #[cfg(feature = "dev")]
    pub fn set_dev(&mut self, dev: Option<dev::DevState>) {
        self.dev = dev;
    }

    /// Does what commands asked for.
    fn apply(&mut self, event_loop: &Loop<'_>, effects: Effects) {
        let effects = effects.union(self.sync_clipboard());
        self.forget_killed();
        self.start_saves();
        self.start_searches();
        self.start_reverts();
        self.answer_about_copies();
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
