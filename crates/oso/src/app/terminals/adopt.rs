//! Terminals coming over `oso dev`'s restart, in the new instance (ADR 0120, step 11): the
//! import makes each carried terminal's buffer, or takes the one its session came back in,
//! and a thread restores its engine from its state; `import-terminals` then brings the
//! masters, once the instance before has gone, and each terminal reads where that one
//! stopped. A hosted session keeps its id, name, space, parent and baseline, and the agent
//! channel is on for its bridge (ADR 0121, Q20). Masters that never come leave the terminals
//! ended, their last screen kept. Dev builds only.
//!
//! Must not depend on: sockets; the server passes the descriptors. Nothing here reads a
//! terminal's text: its state goes from the items to its thread as bytes.

use std::io;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::rope::Rope;
use oso_platform::process::pty::Size;
use oso_term::{Adopt, Encoding, Terminal};
use serde_json::{Value, json};

use super::carry::{CarryEvent, NOT_PARENT};
use crate::app::{App, UserEvent};
use crate::dev;
use crate::dev::handoff::terminals::{self, TerminalState};
use crate::sessions::hosted::Run;
use crate::sessions::identity::SessionId;
use crate::sessions::record::Remembered;

/// How long carried terminals wait for their masters after the import.
const MASTERS_TIME: Duration = Duration::from_secs(30);

/// What an import that carried terminals keeps until their masters come.
#[derive(Default)]
pub(in crate::app) struct Coming {
    /// Each carried terminal's buffer, in the state's order, and its master once it came.
    terminals: Vec<(BufferId, Option<OwnedFd>)>,
    since: Option<Instant>,
}

impl App {
    /// Makes the buffers of the terminals an import carries, `texts` their states in
    /// base64, and starts a thread for each that restores it. Returns each one's buffer, in
    /// order, for the windows that show them.
    pub(in crate::app) fn adopt_terminals(
        &mut self,
        carried: Vec<TerminalState>,
        texts: Vec<Rope>,
    ) -> Vec<BufferId> {
        let scrollback = self.terminal_settings().scrollback;
        let palette = self.terminal_palette();
        let mut buffers = Vec::with_capacity(carried.len());
        let mut sessions = false;
        for (terminal, text) in carried.into_iter().zip(texts) {
            let buffer = self.carried_buffer(&terminal);
            sessions |= terminal.session.is_some();
            self.editor.set_running(buffer, true);
            self.editor.set_agent_reads(buffer, terminal.agent_reads);
            if terminal.char_mode {
                self.dispatcher.toggle_char_mode(buffer);
            }
            let size = Size {
                rows: terminal.rows,
                columns: terminal.columns,
                width: 0,
                height: 0,
            };
            if let Some(hosted) = self.terminals.map.get_mut(buffer) {
                // The size it had: a window of another size resizes it, and the program
                // redraws.
                hosted.sized = Some(size);
            }
            let adopt = Adopt {
                state: Vec::new(),
                encoding: if terminal.snapshot {
                    Encoding::Snapshot
                } else {
                    Encoding::Escapes
                },
                size,
                scrollback,
                palette,
                pid: terminal.pid,
                started: terminal.started,
            };
            self.restore_terminal(buffer, adopt, text);
            buffers.push(buffer);
        }
        self.handover.coming = Coming {
            terminals: buffers.iter().map(|&buffer| (buffer, None)).collect(),
            since: (!buffers.is_empty()).then(Instant::now),
        };
        if sessions {
            self.turn_agent_channel_on_for_a_session();
            self.remember_sessions();
        }
        buffers
    }

    /// The buffer a carried terminal comes back in: its session's, if that came back
    /// stopped already, else a new one, its session hosted as it was.
    fn carried_buffer(&mut self, terminal: &TerminalState) -> BufferId {
        // A session whose id is not one comes back as a plain terminal.
        let ids = terminal.session.as_ref().and_then(|session| {
            let parent = match &session.parent {
                Some(parent) => Some(SessionId::parse(parent)?),
                None => None,
            };
            Some((session, SessionId::parse(&session.id)?, parent))
        });
        let Some((session, id, parent)) = ids else {
            let buffer = self.editor.add_terminal_buffer(&terminal.name);
            self.terminals.map.add(buffer);
            return buffer;
        };
        let known = self
            .hosting
            .table
            .iter()
            .find(|s| s.id == id)
            .map(|s| (s.key, s.buffer));
        let key = match known {
            Some((key, _)) => key,
            None => {
                let back = self.hosting.table.recall(vec![Remembered {
                    id,
                    name: session.name.clone(),
                    directory: session.directory.clone(),
                    root: session.root.clone(),
                    parent,
                    pinned: session.pinned,
                    archived: false,
                    changed: session.changed,
                    baseline: session.baseline.clone(),
                }]);
                match back.first() {
                    Some(&key) => key,
                    None => {
                        let buffer = self.editor.add_terminal_buffer(&terminal.name);
                        self.terminals.map.add(buffer);
                        return buffer;
                    }
                }
            }
        };
        let stopped = known.and_then(|(_, buffer)| buffer);
        let buffer = match stopped {
            Some(buffer) if self.editor.buffer(buffer).is_some() => {
                self.terminals.map.restart(buffer);
                buffer
            }
            _ => {
                let buffer = self.editor.add_terminal_buffer(&terminal.name);
                self.terminals.map.add(buffer);
                buffer
            }
        };
        if let Some(hosted) = self.hosting.table.get_mut(key) {
            hosted.buffer = Some(buffer);
            hosted.run = Run::Running {
                child: terminal.pid,
            };
            hosted.started = terminal.started;
        }
        buffer
    }

    /// Restores the terminal of `buffer` from `adopt` and `text`, its state in base64, on a
    /// thread: decoding and the engine's restore cost more than a frame.
    fn restore_terminal(&self, buffer: BufferId, mut adopt: Adopt, text: Rope) {
        let waker = self.waker.clone();
        let ready = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-adopt-terminal".into())
            .spawn(move || {
                let adopted = match terminals::unbase64(&text.to_string()) {
                    Some(state) => {
                        adopt.state = state;
                        let wake = Box::new(move || {
                            ready.send(UserEvent::Terminal(buffer));
                        });
                        Terminal::adopt(adopt, wake)
                    }
                    None => Err(io::Error::other("the terminal's state is not base64")),
                };
                waker.send(UserEvent::Carry(CarryEvent::Adopted(buffer, adopted)));
            });
        if spawned.is_err() {
            log::warn!("oso dev: no thread to restore a carried terminal");
        }
    }

    /// A carried terminal's thread started, or it could not be restored.
    pub(in crate::app) fn terminal_adopted(
        &mut self,
        buffer: BufferId,
        adopted: io::Result<Terminal>,
    ) {
        match adopted {
            Ok(terminal) => {
                if !self.install_terminal(buffer, terminal, &[]) {
                    return;
                }
                self.attach_masters();
            }
            Err(error) => self.terminal_failed(buffer, &error.to_string()),
        }
    }

    /// `import-terminals`: the masters of the terminals the import carried, in their order,
    /// or, with `None`, word that they stay with the instance before. Only the parent may
    /// bring them.
    pub(in crate::app) fn import_terminals(
        &mut self,
        count: Option<usize>,
        passed: dev::Passed,
    ) -> dev::Reply {
        if !passed.parent {
            return Err(NOT_PARENT.into());
        }
        let Some(count) = count else {
            self.abandon_terminals();
            return Ok(Value::Null);
        };
        let coming = &mut self.handover.coming;
        if count != coming.terminals.len() || passed.brought.len() != count {
            return Err(format!(
                "the import carried {} terminals, not {count}, and {} masters came",
                coming.terminals.len(),
                passed.brought.len()
            ));
        }
        if coming.terminals.iter().any(|(_, master)| master.is_some()) {
            return Err("the terminals' masters came already".into());
        }
        for ((_, master), fd) in coming.terminals.iter_mut().zip(passed.brought) {
            *master = Some(fd);
        }
        self.attach_masters();
        self.write_follows();
        Ok(json!({ "terminals": count }))
    }

    /// Gives each carried terminal whose thread has started its master, once it came.
    fn attach_masters(&mut self) {
        let terminals = &mut self.terminals.map;
        self.handover
            .coming
            .terminals
            .retain_mut(|(buffer, master)| {
                let Some(terminal) = terminals.get(*buffer).and_then(|h| h.terminal.as_ref())
                else {
                    // Gone with its buffer: its master closes here.
                    return terminals.get(*buffer).is_some();
                };
                match master.take() {
                    Some(fd) => {
                        terminal.attach(fd);
                        false
                    }
                    None => true,
                }
            });
        if self.handover.coming.terminals.is_empty() {
            self.handover.coming.since = None;
        }
    }

    /// The carried terminals' masters will not come: each ends, its last screen kept.
    fn abandon_terminals(&mut self) {
        let coming = std::mem::take(&mut self.handover.coming);
        for (buffer, _) in coming.terminals {
            match self
                .terminals
                .map
                .get(buffer)
                .and_then(|hosted| hosted.terminal.as_ref())
            {
                Some(terminal) => terminal.abandon(),
                None => self.terminal_failed(buffer, "it stayed with the instance before"),
            }
        }
    }

    /// Before the event loop waits: masters that never came leave their terminals ended.
    pub(in crate::app) fn coming_terminals_due(&mut self) {
        if self
            .handover
            .coming
            .since
            .is_some_and(|since| since.elapsed() >= MASTERS_TIME)
        {
            log::warn!("oso dev: the carried terminals' masters did not come");
            self.abandon_terminals();
        }
    }

    /// When the event loop wakes for the carried terminals.
    pub(in crate::app) fn coming_terminals_deadline(&self) -> Option<Instant> {
        self.handover.coming.since.map(|since| since + MASTERS_TIME)
    }

    /// Whether carried terminals still wait for their masters, which holds a restart.
    pub(in crate::app) fn terminals_coming(&self) -> bool {
        !self.handover.coming.terminals.is_empty()
    }

    /// Writes the record of the instances this one took over from, for a session's bridge
    /// pinned to one of them (ADR 0121; the author's choice).
    fn write_follows(&self) {
        if let Some(dev) = &self.dev {
            dev.write_follows(&self.handover.follows);
        }
    }
}
