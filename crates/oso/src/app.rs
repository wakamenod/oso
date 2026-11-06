//! The application: the window, the event loop handler and the glue
//! between them and the editor (ADR 0004). It turns winit events into key
//! presses, wheel scrolling and frames, starts file loads and saves and takes
//! in what they report. This file holds the state and what the concerns share;
//! each concern has a module.
//!
//! Must not depend on: wgpu or OS bindings. Editor logic belongs in oso-core
//! and commands, not here.

use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::window::WheelRemainder;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;

#[cfg(feature = "dev")]
mod control;
mod draw;
mod events;
mod input;
mod loads;
mod saves;

#[cfg(feature = "dev")]
use crate::dev;
use crate::dispatch::Dispatcher;
use crate::emergency::Emergency;
use crate::saver::{Leaving, SaveReport};
use draw::View;
use loads::Load;

/// Events sent to the event loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// A loading thread has an update for this buffer.
    Load(BufferId),
    /// A saving thread has ended.
    Save(SaveReport),
    /// A control socket request, in dev builds.
    #[cfg(feature = "dev")]
    Control(dev::UiRequest),
}

/// Top-level application state.
///
/// - `view`: the window and what draws into it.
/// - `editor`: the editor state that commands operate on.
/// - `dispatcher`: keymaps and commands, which turn keys into changes.
/// - `modifiers`: the modifier keys held, as winit last reported them.
/// - `loads`: files loading in the background.
/// - `dev`: the control socket, in dev builds where it started.
pub struct App {
    view: Option<View>,
    editor: Editor,
    dispatcher: Dispatcher,
    modifiers: ModifiersState,
    proxy: EventLoopProxy<UserEvent>,
    loads: Vec<Load>,
    /// An exit waiting for running saves, once this instance has agreed to go.
    leaving: Leaving,
    /// What the panic hook and the end of the event loop write (ADR 0062).
    emergency: Emergency,
    /// When the process started, until the first frame is presented.
    started: Option<Instant>,
    /// Wheel and trackpad scrolling not yet worth a whole row or column.
    wheel: WheelRemainder,
    #[cfg(feature = "dev")]
    dev: Option<dev::DevState>,
    /// Key latency, startup and load times, for benchmarks (ADR 0025).
    #[cfg(feature = "dev")]
    timings: dev::Timings,
    /// Positions handed over from the previous instance, until their file has
    /// loaded.
    #[cfg(feature = "dev")]
    restore: Option<dev::handoff::Restore>,
}

impl App {
    /// An app with no window yet, showing `*scratch*`. Loading threads
    /// report through `proxy`.
    pub fn new(proxy: EventLoopProxy<UserEvent>, started: Instant, emergency: Emergency) -> Self {
        let mut dispatcher = Dispatcher::new();
        dispatcher.set_publisher(emergency.publisher());
        let leaving = Leaving::with_cell(emergency.agreement_cell());
        Self {
            view: None,
            editor: Editor::default(),
            dispatcher,
            modifiers: ModifiersState::default(),
            proxy,
            loads: Vec::new(),
            leaving,
            emergency,
            started: Some(started),
            wheel: WheelRemainder::default(),
            #[cfg(feature = "dev")]
            dev: None,
            #[cfg(feature = "dev")]
            timings: dev::Timings::new(started),
            #[cfg(feature = "dev")]
            restore: None,
        }
    }

    /// Answers control socket requests through `dev`.
    #[cfg(feature = "dev")]
    pub fn set_dev(&mut self, dev: Option<dev::DevState>) {
        self.dev = dev;
    }

    /// Does what commands asked for.
    fn apply(&mut self, event_loop: &ActiveEventLoop, effects: Effects) {
        self.start_saves();
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
}
