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

use std::os::fd::OwnedFd;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::rope::Rope;
use oso_term::{Exported, NotExported};
use serde_json::{Value, json};

use crate::app::{App, UserEvent};
use crate::dev;
use crate::dev::handoff::carry::{self, Item, MAX_CARRIED};
use crate::dev::handoff::terminals::{self, MAX_TERMINALS, SessionState, TerminalState};

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
    state: Value,
    texts: carry::Exported,
    /// The items the texts take, before the terminals'.
    items: usize,
    /// The bytes left for the terminals' states, in base64.
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
    /// The first item that holds a terminal's state: only the parent reads those.
    first_item: usize,
    /// The last request about the restart.
    touched: Instant,
    /// When the masters went to the supervisor.
    handed: Option<Instant>,
}

/// A carried terminal: its buffer, its master, and how often it had read again by then.
struct Carried {
    buffer: BufferId,
    master: OwnedFd,
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
        let (state, texts) = match self.export_texts(Some(&carried)) {
            Ok(exported) => exported,
            Err(why) => return Some((reply, Err(why))),
        };
        let items = state["items"].as_array().map_or(0, Vec::len);
        let used: usize = state["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|len| usize::try_from(len).unwrap_or(usize::MAX))
            .sum();
        let room = MAX_CARRIED.saturating_sub(used);
        // A state in base64 takes 4 bytes for 3.
        let budget = room / 4 * 3;
        let asked = carried
            .iter()
            .filter_map(|&buffer| {
                let terminal = self.terminals.map.get(buffer)?.terminal.as_ref()?;
                Some((buffer, terminal.export(library, budget)))
            })
            .collect();
        self.handover.going.exporting = Some(Exporting {
            reply,
            state,
            texts,
            items,
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
                    "cannot hand over {name}: the terminals' scrollback takes more than the \
                     64 MiB a restart carries; clear it or end the terminal"
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
            None => {
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
                        let items = states
                            .iter()
                            .map(|state| Item(vec![Rope::from(terminals::base64(state).as_str())]))
                            .collect();
                        let _ = sent.send(items);
                        waker.send(UserEvent::Carry(CarryEvent::Encoded));
                    });
                if spawned.is_err() {
                    self.export_failed("cannot hand over the terminals: no thread".into());
                }
            }
            Some(encoding) => match encoding.try_recv() {
                Ok(items) => self.export_done(items),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.export_failed("cannot hand over the terminals: no thread".into());
                }
            },
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
        let mut entries = Vec::with_capacity(exporting.done.len());
        let mut lengths = Vec::with_capacity(items.len());
        let mut carried = Vec::with_capacity(exporting.done.len());
        for (at, ((buffer, exported), item)) in exporting.done.drain(..).zip(&items).enumerate() {
            let state = self.terminal_state(buffer, &exported, exporting.items + at);
            match terminals::entry(&state) {
                Ok(entry) => entries.push(entry),
                Err(why) => {
                    self.handover.going.exporting = None;
                    self.resume_terminals();
                    let _ = exporting.reply.send(Err(why));
                    return;
                }
            }
            lengths.push(json!(item.len()));
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
        let mut state = exporting.state;
        if let Some(all) = state["items"].as_array_mut() {
            all.extend(lengths);
        }
        state["terminals"] = Value::Array(entries);
        // The masters' instance, and those before it, for a bridge that looks for its oso.
        state["pid"] = json!(std::process::id());
        state["follows"] = json!(self.handover.follows);
        if let Err(why) = crate::dev::handoff::fits(&state) {
            self.resume_terminals();
            let _ = exporting.reply.send(Err(why));
            return;
        }
        let export = exporting.texts.number();
        exporting.texts.extend(items);
        self.keep_export(exporting.texts);
        self.handover.going.exported = Some(Outgoing {
            export,
            terminals: carried,
            first_item: exporting.items,
            touched: Instant::now(),
            handed: None,
        });
        let _ = exporting.reply.send(Ok(state));
    }

    /// What the state says of the carried terminal `buffer`.
    fn terminal_state(&self, buffer: BufferId, exported: &Exported, item: usize) -> TerminalState {
        let session = self
            .hosting
            .table()
            .iter()
            .find(|s| s.buffer == Some(buffer));
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
            session: session.map(|s| SessionState {
                id: s.id.as_str().to_owned(),
                name: s.name.clone(),
                directory: s.directory.clone(),
                root: s.root.clone(),
                parent: s.parent.as_ref().map(|p| p.as_str().to_owned()),
                baseline: s.baseline.clone(),
                pinned: s.pinned,
                changed: s.changed,
            }),
        }
    }

    fn export_failed(&mut self, why: String) {
        if let Some(exporting) = self.handover.going.exporting.take() {
            let _ = exporting.reply.send(Err(why));
        }
        self.resume_terminals();
    }

    /// Whether `item` of the latest export holds a terminal's state, which only the parent
    /// may read.
    pub(in crate::app) fn is_terminal_item(&self, export: u64, item: usize) -> bool {
        self.handover
            .going
            .exported
            .as_ref()
            .is_some_and(|e| e.export == export && item >= e.first_item)
    }

    /// Notes a request about the restart, which keeps exported terminals waiting.
    pub(in crate::app) fn touch_carried(&mut self) {
        if let Some(exported) = self.handover.going.exported.as_mut() {
            exported.touched = Instant::now();
        }
    }

    /// `hand-over-terminals`: a second descriptor for each carried master, in the state's
    /// order, put to go with the reply, if nothing changed since the export.
    pub(in crate::app) fn hand_over_terminals(
        &mut self,
        generation: u64,
        passed: &dev::Passed,
    ) -> dev::Reply {
        if !passed.parent {
            return Err(NOT_PARENT.into());
        }
        crate::dev::handoff::exit_allowed(&self.editor, Some(generation))?;
        self.carried_unchanged()?;
        let exported = self
            .handover
            .going
            .exported
            .as_mut()
            .ok_or("no terminals were exported")?;
        let fds = exported
            .terminals
            .iter()
            .map(|t| t.master.try_clone())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("cannot hand over the terminals: {e}"))?;
        let count = fds.len();
        passed.passing.put(fds);
        exported.handed = Some(Instant::now());
        exported.touched = Instant::now();
        Ok(json!({ "terminals": count }))
    }

    /// Whether every carried terminal is still there and has read nothing since the export.
    fn carried_unchanged(&self) -> Result<(), String> {
        let Some(exported) = self.handover.going.exported.as_ref() else {
            return Ok(());
        };
        let changed = exported.terminals.iter().any(|carried| {
            self.terminals
                .map
                .get(carried.buffer)
                .and_then(|hosted| hosted.terminal.as_ref())
                .is_none_or(|terminal| terminal.resumes() != carried.resumes)
        });
        if changed {
            return Err(format!(
                "{}: a terminal has read again or ended since the export",
                crate::dev::EXIT_REFUSED
            ));
        }
        Ok(())
    }

    /// Before an `exit` with a generation: the carried terminals go if they were handed over,
    /// and an exit that would end terminals not handed over is refused.
    pub(in crate::app) fn exit_with_terminals(&mut self) -> Result<(), String> {
        let Some(exported) = self.handover.going.exported.as_ref() else {
            return Ok(());
        };
        if exported.handed.is_none() {
            self.resume_terminals();
            return Err(format!(
                "{}: the terminals were exported but not handed over",
                crate::dev::EXIT_REFUSED
            ));
        }
        if let Err(why) = self.carried_unchanged() {
            self.resume_terminals();
            return Err(why);
        }
        let Some(exported) = self.handover.going.exported.take() else {
            return Ok(());
        };
        for carried in exported.terminals {
            let terminal = self
                .terminals
                .map
                .get_mut(carried.buffer)
                .and_then(|hosted| hosted.terminal.take());
            if let Some(terminal) = terminal {
                terminal.release();
            }
        }
        Ok(())
    }

    /// Has every exported terminal read again, and forgets their masters here.
    pub(in crate::app) fn resume_terminals(&mut self) {
        if let Some(exporting) = self.handover.going.exporting.take() {
            let _ = exporting
                .reply
                .send(Err("the export was overtaken by another".into()));
        }
        let Some(exported) = self.handover.going.exported.take() else {
            return;
        };
        for carried in exported.terminals {
            if let Some(terminal) = self
                .terminals
                .map
                .get(carried.buffer)
                .and_then(|hosted| hosted.terminal.as_ref())
            {
                terminal.resume();
            }
        }
    }

    /// Before the event loop waits: terminals read again once the restart has gone quiet,
    /// and an export that takes too long fails.
    pub(in crate::app) fn carried_terminals_due(&mut self) {
        let now = Instant::now();
        if self
            .handover
            .going
            .exporting
            .as_ref()
            .is_some_and(|e| now.duration_since(e.since) >= EXPORT_TIME)
        {
            self.export_failed("cannot hand over the terminals: they took too long".into());
        }
        let over = self
            .handover
            .going
            .exported
            .as_ref()
            .is_some_and(|e| match e.handed {
                Some(at) => now.duration_since(at) >= EXIT_TIME,
                None => now.duration_since(e.touched) >= QUIET_TIME,
            });
        if over {
            log::info!("oso dev: the restart went quiet, so the terminals read again");
            self.resume_terminals();
        }
    }

    /// When the event loop wakes for the restart's terminals.
    pub(in crate::app) fn carried_terminals_deadline(&self) -> Option<Instant> {
        let going = &self.handover.going;
        let exporting = going.exporting.as_ref().map(|e| e.since + EXPORT_TIME);
        let exported = going.exported.as_ref().map(|e| match e.handed {
            Some(at) => at + EXIT_TIME,
            None => e.touched + QUIET_TIME,
        });
        exporting.into_iter().chain(exported).min()
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
