//! Terminals going over `oso dev`'s restart, in the instance being replaced (ADR 0120, step
//! 11): `export-state` asks each running terminal's thread to stop reading and write its state,
//! a thread puts each state in base64 for the items, `hand-over-terminals` gives the
//! supervisor a second descriptor for each master, and the `exit` that follows lets the
//! terminals go, nothing hung up. A restart that does not come to that has every terminal
//! read again: an exit refused, no request for a while, or a hand-over not followed by its
//! exit. Dev builds only.
//!
//! Must not depend on: sockets; the server passes the descriptors. Nothing here reads a
//! terminal's text: its state goes from its thread to the items as bytes.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::rope::Rope;
use oso_term::{Exported, NotExported};
use serde_json::{Value, json};

use crate::app::{App, UserEvent};
use crate::dev;
use crate::dev::handoff::carry::{self, Item, MAX_CARRIED};
use crate::dev::handoff::terminals::{self, MAX_TERMINALS, TerminalState};

mod release;

/// How long the terminals' states may take: less than the dev socket waits for a reply.
const EXPORT_TIME: Duration = Duration::from_secs(8);

/// How long exported terminals read nothing with no request about the restart.
const QUIET_TIME: Duration = Duration::from_secs(10);

/// How long handed-over terminals wait for the exit that lets them go.
const EXIT_TIME: Duration = Duration::from_secs(5);

/// What a restart that carries terminals keeps in the instance being replaced.
#[derive(Default)]
pub(in crate::app) struct Going {
    /// An export waiting for its terminals' states.
    exporting: Option<Exporting>,
    /// The terminals the latest export carries, until the restart goes or is over.
    exported: Option<Outgoing>,
}

/// An export whose reply waits for its terminals.
struct Exporting {
    reply: mpsc::Sender<dev::Reply>,
    /// The running terminals it carries, in the state's order.
    carried: Vec<BufferId>,
    /// The bytes the texts leave for the terminals' states, in base64, before kills.
    room: usize,
    asked: Vec<(BufferId, mpsc::Receiver<Result<Exported, NotExported>>)>,
    done: Vec<(BufferId, Exported)>,
    /// The thread that puts the states in base64, once every one came.
    encoding: Option<mpsc::Receiver<Vec<Item>>>,
    since: Instant,
}

/// The terminals an export carries, with their masters.
struct Outgoing {
    export: u64,
    terminals: Vec<Carried>,
    /// The last request about the restart.
    touched: Instant,
    /// When the masters went to the supervisor.
    handed: Option<Instant>,
}

/// A carried terminal: its buffer, its master, and how often it had read again by then.
struct Carried {
    buffer: BufferId,
    master: crate::dev::fds::Fd,
    resumes: u64,
}

/// What a thread of the restart found.
#[derive(Debug)]
pub(crate) enum CarryEvent {
    /// The terminals' states of the export waiting, in base64.
    Encoded,
    /// A carried terminal's thread started, or it could not be restored.
    Adopted(BufferId, std::io::Result<oso_term::Terminal>),
}

impl App {
    /// `export-state` with terminals: the state, once every running terminal has written
    /// its own, for the new instance reading `library`. Only the parent may ask. Returns the
    /// reply if it is ready now.
    pub(in crate::app) fn export_with_terminals(
        &mut self,
        library: &str,
        parent: bool,
        reply: mpsc::Sender<dev::Reply>,
    ) -> Option<(mpsc::Sender<dev::Reply>, dev::Reply)> {
        if !parent {
            return Some((reply, Err(NOT_PARENT.into())));
        }
        self.resume_terminals();
        let carried: Vec<BufferId> = self.editor.running_terminals().collect();
        if let Err(why) = self.terminals_may_go(&carried) {
            return Some((reply, Err(why)));
        }
        // What the texts take but the kills, which fill only what the terminals leave: the
        // state itself is made again once the terminals' states are in.
        let state = match self.export_texts(Some((&carried, MAX_CARRIED))) {
            Ok((state, texts)) => {
                carry::drop_later(texts);
                state
            }
            Err(why) => return Some((reply, Err(why))),
        };
        let used: usize = state["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|len| usize::try_from(len).unwrap_or(usize::MAX))
            .sum();
        let room = MAX_CARRIED.saturating_sub(used);
        // A state in base64 takes 4 bytes for 3. The terminals share the room: the one that
        // passes it stops writing at once.
        let budget = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(room / 4 * 3));
        let asked = carried
            .iter()
            .filter_map(|&buffer| {
                let terminal = self.terminals.map.get(buffer)?.terminal.as_ref()?;
                Some((
                    buffer,
                    terminal.export(library, std::sync::Arc::clone(&budget)),
                ))
            })
            .collect();
        self.handover.going.exporting = Some(Exporting {
            reply,
            carried,
            room,
            asked,
            done: Vec::new(),
            encoding: None,
            since: Instant::now(),
        });
        self.carry_terminals();
        None
    }

    /// Why the running terminals `carried` cannot go, if they cannot: too many, one still
    /// starting, or one whose program reads a password (ADR 0120, Q17; the author's choice).
    pub(in crate::app) fn terminals_may_go(&self, carried: &[BufferId]) -> Result<(), String> {
        if carried.len() > MAX_TERMINALS {
            return Err(format!(
                "cannot hand over more than {MAX_TERMINALS} running terminals"
            ));
        }
        for &buffer in carried {
            let name = self.editor.buffer(buffer).map_or("", |b| b.name());
            let starting = self
                .terminals
                .map
                .get(buffer)
                .is_none_or(|hosted| hosted.terminal.is_none());
            if starting {
                return Err(format!("cannot hand over {name}: it is starting"));
            }
            if self
                .editor
                .buffer(buffer)
                .is_some_and(super::reads_a_password)
            {
                return Err(format!(
                    "cannot hand over {name}: its program reads a password"
                ));
            }
        }
        Ok(())
    }

    /// Takes what the terminals' threads and the base64 thread sent for the export waiting,
    /// and replies once it is all there, or once something fails.
    pub(in crate::app) fn carry_terminals(&mut self) {
        let Some(exporting) = self.handover.going.exporting.as_mut() else {
            return;
        };
        let mut failed = None;
        exporting
            .asked
            .retain(|(buffer, answer)| match answer.try_recv() {
                Ok(Ok(exported)) => {
                    exporting.done.push((*buffer, exported));
                    false
                }
                Ok(Err(why)) => {
                    failed.get_or_insert((*buffer, Some(why)));
                    false
                }
                Err(mpsc::TryRecvError::Empty) => true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    failed.get_or_insert((*buffer, Some(NotExported::Ended)));
                    false
                }
            });
        if let Some((buffer, why)) = failed {
            let name = self.buffer_name(buffer);
            let why = match why {
                Some(NotExported::TooLarge) => format!(
                    "cannot hand over {name}: the terminals' scrollback and the unsaved text \
                     take more than the 64 MiB a restart carries; clear some or end a terminal"
                ),
                Some(NotExported::Ended) | None => format!("cannot hand over {name}: it ended"),
                Some(NotExported::Failed(error)) => format!("cannot hand over {name}: {error}"),
            };
            return self.export_failed(why);
        }
        if !exporting.asked.is_empty() {
            return;
        }
        if let Some(why) = self.exported_fault() {
            return self.export_failed(why);
        }
        let Some(exporting) = self.handover.going.exporting.as_mut() else {
            return;
        };
        match &exporting.encoding {
            None => self.start_encoding(),
            Some(encoding) => match encoding.try_recv() {
                Ok(items) => self.export_done(items),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.export_failed("cannot hand over the terminals: no thread".into());
                }
            },
        }
    }

    /// Puts the states of the export waiting in base64 on a thread of its own, in the state's
    /// order, whatever order the threads answered in: the windows name the terminals by their
    /// place.
    fn start_encoding(&mut self) {
        let Some(exporting) = self.handover.going.exporting.as_mut() else {
            return;
        };
        let carried = &exporting.carried;
        exporting
            .done
            .sort_by_key(|(buffer, _)| carried.iter().position(|c| c == buffer));
        let states: Vec<Vec<u8>> = exporting
            .done
            .iter_mut()
            .map(|(_, exported)| std::mem::take(&mut exported.state))
            .collect();
        let (sent, got) = mpsc::channel();
        exporting.encoding = Some(got);
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-export-terminals".into())
            .spawn(move || {
                // Each state goes once it is in base64, so that few copies are held.
                let items = states
                    .into_iter()
                    .map(|state| {
                        let text = terminals::base64(&state);
                        drop(state);
                        Item(vec![Rope::from(text.as_str())])
                    })
                    .collect();
                let _ = sent.send(items);
                waker.send(UserEvent::Carry(CarryEvent::Encoded));
            });
        if spawned.is_err() {
            self.export_failed("cannot hand over the terminals: no thread".into());
        }
    }

    /// What is wrong with the states that came, if anything: a program that started reading
    /// a password, or states past the room left.
    fn exported_fault(&self) -> Option<String> {
        let exporting = self.handover.going.exporting.as_ref()?;
        if let Some((buffer, _)) = exporting.done.iter().find(|(_, e)| e.echo_off) {
            let name = self.buffer_name(*buffer);
            return Some(format!(
                "cannot hand over {name}: its program reads a password"
            ));
        }
        let total: usize = exporting
            .done
            .iter()
            .map(|(_, e)| terminals::base64_len(e.state.len()))
            .sum();
        if total > exporting.room {
            let largest = exporting
                .done
                .iter()
                .max_by_key(|(_, e)| e.state.len())
                .map(|(buffer, _)| self.buffer_name(*buffer))
                .unwrap_or_default();
            return Some(format!(
                "cannot hand over the terminals: with the unsaved text they take more than \
                 the 64 MiB a restart carries; {largest} takes the most"
            ));
        }
        None
    }

    /// Replies to the export waiting with the state and its terminals.
    fn export_done(&mut self, items: Vec<Item>) {
        let Some(mut exporting) = self.handover.going.exporting.take() else {
            return;
        };
        // The state now, the kills filling what the texts and the terminals leave.
        let taken = items.iter().map(Item::len).sum();
        let (mut state, mut texts) = match self.export_texts(Some((&exporting.carried, taken))) {
            Ok(exported) => exported,
            Err(why) => return self.export_given_up(exporting, items, why),
        };
        let first = state["items"].as_array().map_or(0, Vec::len);
        let done = std::mem::take(&mut exporting.done);
        let mut entries = Vec::with_capacity(done.len());
        let mut carried = Vec::with_capacity(done.len());
        let mut failed = None;
        for (at, (buffer, exported)) in done.into_iter().enumerate() {
            let terminal = self.terminal_state(buffer, &exported, first + at);
            match terminals::entry(&terminal) {
                Ok(entry) => entries.push(entry),
                Err(why) => failed = failed.or(Some(why)),
            }
            let resumes = self
                .terminals
                .map
                .get(buffer)
                .and_then(|hosted| hosted.terminal.as_ref())
                .map_or(0, oso_term::Terminal::resumes);
            carried.push(Carried {
                buffer,
                master: exported.master,
                resumes,
            });
        }
        if let Some(why) = failed {
            carry::drop_later((texts, carried));
            return self.export_given_up(exporting, items, why);
        }
        let lengths: Vec<Value> = items.iter().map(|item| json!(item.len())).collect();
        if let Some(all) = state["items"].as_array_mut() {
            all.extend(lengths);
        }
        state["terminals"] = Value::Array(entries);
        // The masters' instance, and those before it, for a bridge that looks for its oso.
        state["pid"] = json!(std::process::id());
        state["follows"] = json!(self.handover.follows);
        if let Err(why) = crate::dev::handoff::fits(&state) {
            carry::drop_later(texts);
            return self.export_given_up(exporting, items, why);
        }
        let export = texts.number();
        texts.extend_with_terminals(items);
        self.keep_export(texts);
        self.handover.going.exported = Some(Outgoing {
            export,
            terminals: carried,
            touched: Instant::now(),
            handed: None,
        });
        let _ = exporting.reply.send(Ok(state));
    }

    /// What the state says of the carried terminal `buffer`.
    fn terminal_state(&self, buffer: BufferId, exported: &Exported, item: usize) -> TerminalState {
        TerminalState {
            item,
            snapshot: exported.encoding == oso_term::Encoding::Snapshot,
            name: self.buffer_name(buffer),
            rows: exported.size.rows,
            columns: exported.size.columns,
            pid: exported.pid,
            started: exported.started,
            agent_reads: self.editor.buffer(buffer).is_some_and(|b| b.agent_reads()),
            char_mode: self.dispatcher.terminal_input(buffer)
                == crate::terminal::route::Input::Char,
            session: self.carried_session_state(buffer),
        }
    }

    /// Replies `why` to the export, has its terminals read again, and frees what it held
    /// off this thread.
    fn export_given_up(&mut self, exporting: Exporting, items: Vec<Item>, why: String) {
        let _ = exporting.reply.send(Err(why));
        self.resume_carried(&exporting.carried);
        carry::drop_later((exporting, items));
    }

    fn export_failed(&mut self, why: String) {
        if let Some(exporting) = self.handover.going.exporting.take() {
            let _ = exporting.reply.send(Err(why));
            self.resume_carried(&exporting.carried);
            // States of up to 64 MiB are freed off this thread.
            carry::drop_later(exporting);
        }
        self.resume_terminals();
    }

    fn buffer_name(&self, buffer: BufferId) -> String {
        self.editor
            .buffer(buffer)
            .map(|b| b.name().to_owned())
            .unwrap_or_default()
    }
}

/// Why a client that is not this instance's parent gets no terminals: their states and
/// masters go only to `oso dev` (ADR 0120, step 11; the author's choice).
pub(in crate::app) const NOT_PARENT: &str =
    "terminals go only to the process that started this instance, oso dev";
