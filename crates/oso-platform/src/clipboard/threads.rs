//! The clipboard's two threads: a worker that owns the system's clipboard, and the front,
//! which answers the host in time (ADR 0074).
//!
//! Must not depend on: the backends' own modules; a worker gets its backend from the
//! factory it is given.
//!
//! The worker does what it is given in order, and may wait on another process. The front
//! hands it one job at a time: a write while no read is out, or the newest write held
//! until the read is answered, so writes never queue behind a stuck read. A read gets an
//! answer within the answer time, [`Contents::NoAnswer`] if the worker has none. A worker
//! still on a read after the give-up time is left to finish on its own, and a new one
//! takes over, so one read that never returns costs one [`Event::NotAnswering`], not the
//! clipboard for the rest of the session. What a worker left behind says afterwards is
//! dropped.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::{Backend, Contents, Event, Fingerprint, Found, MakeText};

/// A write the host asked for.
pub(super) struct WriteJob {
    pub(super) id: u64,
    pub(super) concealed: bool,
    pub(super) text: MakeText,
}

/// What the front gives a worker.
enum Job {
    Write(WriteJob),
    Read,
}

/// What reaches the front: from the host, or from a worker of a given generation.
pub(super) enum Message {
    Write(WriteJob),
    Read,
    Written {
        id: u64,
        fingerprint: Fingerprint,
    },
    Found {
        worker: u64,
        contents: Contents,
    },
    /// The host's handle went: end once the worker has done its work.
    Close,
}

/// How long the front waits before it answers for a read, and before it leaves a worker
/// behind.
#[derive(Clone, Copy)]
pub(super) struct Timing {
    pub(super) answer: Duration,
    pub(super) give_up: Duration,
}

/// Makes a backend for each new worker.
pub(super) type Factory = Box<dyn FnMut() -> Box<dyn Backend + Send> + Send>;

/// Starts the front's thread, which starts the first worker. Returns where messages go.
pub(super) fn start(
    make: Factory,
    max_read: usize,
    timing: Timing,
    report: impl Fn(Event) + Send + 'static,
) -> io::Result<Sender<Message>> {
    let (messages, inbox) = mpsc::channel();
    let mut front = Front {
        make,
        max_read,
        timing,
        keys: RandomState::new(),
        messages: messages.clone(),
        jobs: None,
        worker: 0,
        reading: None,
        pending: None,
    };
    front.new_worker()?;
    thread::Builder::new()
        .name("clipboard answers".into())
        .spawn(move || front.run(&inbox, &report))?;
    Ok(messages)
}

/// A read the worker is on: when it began, and whether the host has had its answer.
struct Reading {
    since: Instant,
    answered: bool,
}

struct Front {
    make: Factory,
    max_read: usize,
    timing: Timing,
    keys: RandomState,
    messages: Sender<Message>,
    /// Where the current worker takes jobs.
    jobs: Option<Sender<Job>>,
    /// The current worker's generation.
    worker: u64,
    reading: Option<Reading>,
    /// The newest write, held while the worker is on a read.
    pending: Option<WriteJob>,
}

impl Front {
    /// Starts a worker over a new backend. The one before, if any, ends once its jobs are
    /// done, and what it says is dropped.
    fn new_worker(&mut self) -> io::Result<()> {
        self.worker += 1;
        let (jobs, work) = mpsc::channel();
        let worker = self.worker;
        let backend = (self.make)();
        let (max_read, keys, answers) = (self.max_read, self.keys.clone(), self.messages.clone());
        thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || run_worker(backend, worker, max_read, &keys, &work, &answers))?;
        self.jobs = Some(jobs);
        Ok(())
    }

    /// When the front must act next with no message: answer for a read, or give up on it.
    fn deadline(&self) -> Option<Instant> {
        self.reading.as_ref().map(|reading| {
            if reading.answered {
                reading.since + self.timing.give_up
            } else {
                reading.since + self.timing.answer
            }
        })
    }

    fn run(mut self, inbox: &Receiver<Message>, report: &impl Fn(Event)) {
        loop {
            let message = match self.deadline() {
                Some(at) => {
                    match inbox.recv_timeout(at.saturating_duration_since(Instant::now())) {
                        Ok(message) => message,
                        Err(RecvTimeoutError::Timeout) => {
                            self.late(report);
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match inbox.recv() {
                    Ok(message) => message,
                    Err(_) => return,
                },
            };
            match message {
                Message::Write(write) if self.reading.is_some() => self.pending = Some(write),
                Message::Write(write) => self.send(Job::Write(write)),
                Message::Read if self.reading.is_some() => {
                    report(Event::Read(Contents::NoAnswer));
                }
                Message::Read => {
                    self.send(Job::Read);
                    self.reading = Some(Reading {
                        since: Instant::now(),
                        answered: false,
                    });
                }
                Message::Written { id, fingerprint } => report(Event::Written { id, fingerprint }),
                Message::Found { worker, contents } if worker == self.worker => {
                    if self.reading.take().is_some_and(|reading| !reading.answered) {
                        report(Event::Read(contents));
                    }
                    self.send_pending();
                }
                // From a worker left behind.
                Message::Found { .. } => {}
                Message::Close => return,
            }
        }
    }

    /// A read's time has come: answer for it, or, past the give-up time, leave its worker
    /// behind.
    fn late(&mut self, report: &impl Fn(Event)) {
        let Some(reading) = &mut self.reading else {
            return;
        };
        if !reading.answered {
            reading.answered = true;
            report(Event::Read(Contents::NoAnswer));
            return;
        }
        self.reading = None;
        report(Event::NotAnswering);
        if self.new_worker().is_err() {
            // No thread to replace it: writes go to the stuck one, behind its read.
            return;
        }
        self.send_pending();
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            // A worker ends only when its channel goes, so a send does not fail.
            let _ = jobs.send(job);
        }
    }

    fn send_pending(&mut self) {
        if let Some(write) = self.pending.take() {
            self.send(Job::Write(write));
        }
    }
}

/// A worker: does each job in order, and sends what came of it.
fn run_worker(
    mut backend: Box<dyn Backend + Send>,
    worker: u64,
    max_read: usize,
    keys: &RandomState,
    work: &Receiver<Job>,
    answers: &Sender<Message>,
) {
    let fingerprint = |text: &str| Fingerprint {
        len: text.len(),
        hash: keys.hash_one(text),
    };
    let mut own = None;
    // A job taken from the queue while looking for a newer write.
    let mut next = None;
    loop {
        let Some(job) = next.take().or_else(|| work.recv().ok()) else {
            return;
        };
        let message = match job {
            Job::Write(mut write) => {
                // A newer write replaces one not yet done, so kills in a row cost one
                // write, and a read waits behind one at most.
                while let Ok(later) = work.try_recv() {
                    match later {
                        Job::Write(newer) => write = newer,
                        read @ Job::Read => {
                            next = Some(read);
                            break;
                        }
                    }
                }
                let text = (write.text)();
                own = backend.write(&text, write.concealed);
                Message::Written {
                    id: write.id,
                    fingerprint: fingerprint(&text),
                }
            }
            Job::Read => Message::Found {
                worker,
                contents: match backend.change_count() {
                    Some(count) if Some(count) == own => Contents::Own,
                    _ => match backend.read(max_read) {
                        Found::Text { text, concealed } => Contents::Text {
                            fingerprint: fingerprint(&text),
                            text,
                            concealed,
                        },
                        Found::Nothing => Contents::Nothing,
                        Found::TooLarge => Contents::TooLarge,
                    },
                },
            },
        };
        if answers.send(message).is_err() {
            return;
        }
    }
}
