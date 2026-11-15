//! `oso-restart` on the UI thread (ADR 0133): in the instance going, checking that it may hand
//! over, stopping the agent channel and starting the restart's thread, then answering what that
//! thread asks; in the new instance, starting the thread that reads the pair once the window is
//! up, and answering what it asks. Both answer with the same handover code as `oso dev`'s
//! restart (`handover`, `terminals/carry.rs`, `terminals/adopt.rs`), as the parent.
//!
//! Must not depend on: sockets or processes; the restart's threads hold the pair and the
//! child.

use oso_platform::control::ControlStream;

use super::shell::Loop;
use super::{App, UserEvent};
use crate::carry::{self, Asked, Op, Passed};

/// What a restart keeps on the UI thread.
#[derive(Default)]
pub(super) struct Restart {
    /// The binary and arguments a restart starts, as this instance started.
    pub(super) start: Option<carry::going::Start>,
    /// A restart of this instance is under way.
    under_way: bool,
    /// The agent channel was on when it began, so it comes back if the restart stops short.
    channel_was_on: bool,
    /// In a new instance: the pair from the instance before, and that instance's id, until
    /// the window is up.
    coming: Option<(ControlStream, u32)>,
}

impl App {
    /// What this instance starts on `oso-restart`, and, if it takes over from the instance
    /// `parent` through `pair`, those two, until the window is up.
    pub fn set_restart(
        &mut self,
        start: carry::going::Start,
        taking_over: Option<(ControlStream, u32)>,
    ) {
        self.restart.start = Some(start);
        self.restart.coming = taking_over;
    }

    /// After each event: in a new instance whose window is up, starts the thread that reads
    /// the pair.
    pub(super) fn sync_restart(&mut self) {
        if self.view.is_some()
            && let Some((pair, parent)) = self.restart.coming.take()
        {
            self.read_the_pair(pair, parent);
        }
    }

    fn read_the_pair(&mut self, pair: ControlStream, parent: u32) {
        // A new instance that ends before it speaks, for an end-to-end flow.
        #[cfg(feature = "dev")]
        if dev_fails_at("hello") {
            std::process::exit(3);
        }
        let pair = match carry::pair::Pair::new(pair) {
            Ok(pair) => pair,
            Err(e) => {
                log::warn!("oso-restart: cannot read the pair: {e}");
                return;
            }
        };
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-carried".into())
            .spawn(move || {
                carry::coming::serve(pair, parent, |asked| waker.send(UserEvent::Restart(asked)));
            });
        if let Err(e) = spawned {
            log::warn!("oso-restart: cannot start the thread that reads the pair: {e}");
        }
    }

    /// `oso-restart`: starts the restart's thread, if this instance may hand over.
    pub(super) fn begin_restart(&mut self) {
        let refused = if self.restart.under_way {
            Err("a restart is under way".to_owned())
        } else {
            self.check_handover(true, true).map(drop)
        };
        let Some(start) = self.restart.start.clone().filter(|_| refused.is_ok()) else {
            let why = refused
                .err()
                .unwrap_or_else(|| "this instance cannot restart".into());
            self.editor.message(format!("oso-restart: {why}"));
            self.request_redraw();
            return;
        };
        self.restart.channel_was_on = self.agent_channel_on();
        // An agent must not see or steer the restart (ADR 0133, T-agent).
        self.stop_agent_channel();
        let waker = self.waker.clone();
        match carry::going::spawn(start, move |asked| waker.send(UserEvent::Restart(asked))) {
            Ok(()) => {
                self.restart.under_way = true;
                self.editor.message("Restarting…");
            }
            Err(e) => self.restart_stays(format!("cannot start the restart: {e}")),
        }
        self.request_redraw();
    }

    /// What a thread of the restart asks.
    pub(super) fn restart_asked(&mut self, event_loop: &Loop<'_>, asked: Asked) {
        match asked {
            Asked::Op { op, passed, reply } => {
                if self.is_leaving(event_loop) {
                    let _ = reply.send(Err("the editor is shutting down".into()));
                    return;
                }
                self.drop_stale_texts();
                if let Some((reply, result)) = self.answer_restart(op, reply, passed) {
                    let _ = reply.send(result);
                }
            }
            Asked::Exit { generation, reply } => {
                // An exit refused at the last moment, for an end-to-end flow.
                #[cfg(feature = "dev")]
                let refused = dev_fails_at("exit");
                #[cfg(not(feature = "dev"))]
                let refused = false;
                let allowed = if refused {
                    Err(carry::EXIT_REFUSED.to_owned())
                } else {
                    carry::exit_allowed(&self.editor, Some(generation))
                        .and_then(|()| self.exit_with_terminals())
                };
                if allowed.is_ok() {
                    self.leave(event_loop, true);
                }
                let _ = reply.send(allowed.map(|()| serde_json::json!({ "exiting": true })));
            }
            Asked::Stays(why) => self.restart_stays(why),
            Asked::Left { generation } => self.left_by_the_one_before(event_loop, generation),
        }
        self.request_redraw();
    }

    /// The restart is over and this instance stays: its terminals read again, the channel comes
    /// back if it was on, and why is said.
    fn restart_stays(&mut self, why: String) {
        self.restart.under_way = false;
        self.resume_terminals();
        self.forget_export();
        if std::mem::take(&mut self.restart.channel_was_on) {
            self.start_agent_channel();
        }
        self.editor
            .message(format!("oso-restart: {why}; everything stays here"));
    }

    /// In a new instance: the one before stays. Its terminals are given up here, and this one
    /// closes unless it was changed since the import.
    fn left_by_the_one_before(&mut self, event_loop: &Loop<'_>, generation: Option<u64>) {
        let passed = Passed {
            parent: true,
            ..Passed::default()
        };
        let (reply, _) = std::sync::mpsc::channel();
        if let Some((_, Err(why))) =
            self.answer_restart(Op::ImportTerminals { count: None }, reply, passed)
        {
            log::debug!("oso-restart: the carried terminals were not given up: {why}");
        }
        if generation.is_some() && carry::exit_allowed(&self.editor, generation).is_ok() {
            self.leave(event_loop, true);
        } else {
            self.editor.message(
                "oso-restart: the oso before stays, and so does this one, as it was changed here",
            );
        }
    }
}

/// Whether a dev build's end-to-end flow asks the restart to stop short at `point`
/// (`OSO_DEV_RESTART_FAIL`): `hello`, the new instance ends before it speaks, or `exit`, the
/// instance going refuses to exit once the new one holds everything.
#[cfg(feature = "dev")]
fn dev_fails_at(point: &str) -> bool {
    std::env::var("OSO_DEV_RESTART_FAIL").is_ok_and(|asked| asked == point)
}
