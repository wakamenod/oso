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
use crate::saver::SaveReport;
use draw::View;
use loads::Load;
use saves::Leaving;

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
    /// The exit waiting for running saves, once this instance has agreed to go.
    leaving: Option<Leaving>,
    /// A save outlived a leave, so the next exit does not wait for it again.
    save_stuck: bool,
    /// When the process started, until the first frame is presented.
    started: Option<Instant>,
    /// Trackpad scrolling not yet worth a whole row, in pixels.
    wheel_pixels: f64,
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
    pub fn new(proxy: EventLoopProxy<UserEvent>, started: Instant) -> Self {
        Self {
            view: None,
            editor: Editor::default(),
            dispatcher: Dispatcher::new(),
            modifiers: ModifiersState::default(),
            proxy,
            loads: Vec::new(),
            leaving: None,
            save_stuck: false,
            started: Some(started),
            wheel_pixels: 0.0,
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
