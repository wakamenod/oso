//! A server's session thread (ADR 0138, Threads and Lifetime): the gate, the spawn in a
//! process group of its own, `initialize`, the messages of the run, and its end. A crash ends
//! the run with `SIGTERM` to the group, then `SIGKILL` after [`EXIT_WAIT`], and starts it again
//! after the policy's wait, or stops it for the session; a stop the person asked for sends
//! `shutdown` and `exit` first. What the thread learns goes to the outbox.
//!
//! Must not depend on: the UI thread or the tip crate; this runs on its own thread, and only
//! the events and the outbox cross to the UI.

use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::{self, Child, Executable, Signal, Sink, Source, Spawn};

use crate::limits::{EXIT_WAIT, GROUP_GRACE, INITIALIZE_WAIT, LIVENESS_INTERVAL};
use crate::protocol::{self, Initialize, Severity};
use crate::uri;

use super::outbox::Reporter;
use super::pipes::{self};
use super::policy::{Crashes, Flood};
use super::{Event, Gate, Start, State};

mod asks;
mod incoming;
mod run;
mod sync;

use run::Run;

/// How often the writer's queue is looked at while whole texts wait for room in it.
const WAITING_POLL: Duration = Duration::from_millis(20);

/// How often the end of a process is looked for while waiting for it.
const POLL: Duration = Duration::from_millis(10);

/// What the session does after a run, or a wait.
enum Next {
    Start,
    After(Duration),
    Stopped,
    NotStarted,
    Ended,
    Quit,
}

/// How a run ended.
enum End {
    Crashed(String),
    /// Stopped as asked, then this.
    Stopped(After),
    Quit,
}

/// What a stop the person asked for leads to.
enum After {
    Ended,
    Restart(Box<(Start, Gate)>),
}

/// A request of oso's in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asked {
    Initialize,
    Shutdown,
    /// An ask of the UI's about a document (step 8).
    Place(super::Ask),
}

/// Where a stop the person asked for is: waiting for `shutdown`'s answer, for the process to
/// end after `exit`, or after `SIGTERM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Shutdown,
    Exit,
    Terminate,
}

pub(in crate::server) struct Session {
    start: Start,
    gate: Gate,
    reporter: Reporter,
    events: Receiver<Event>,
    sender: Sender<Event>,
    child: Arc<Mutex<Option<Child>>>,
    crashes: Crashes,
    run: u64,
    /// The documents the server knows, across its runs.
    documents: super::documents::Documents,
    /// The run's diagnostics reports, each document's last (step 6).
    diagnosed: super::diagnosed::Store,
}

impl Session {
    pub(in crate::server) fn new(
        start: Start,
        gate: Gate,
        reporter: Reporter,
        events: Receiver<Event>,
        sender: Sender<Event>,
        child: Arc<Mutex<Option<Child>>>,
    ) -> Self {
        Self {
            start,
            gate,
            reporter,
            events,
            sender,
            child,
            crashes: Crashes::default(),
            run: 0,
            documents: super::documents::Documents::default(),
            diagnosed: super::diagnosed::Store::default(),
        }
    }

    /// Runs the server until it is shut down or oso quits.
    pub(in crate::server) fn run(mut self) {
        let mut next = Next::Start;
        loop {
            next = match next {
                Next::Start => self.one_run(),
                Next::After(delay) => {
                    // The line first: the state's wake then carries it.
                    self.reporter
                        .log(format!("starting again in {} s", delay.as_secs()));
                    self.reporter.state(State::Waiting);
                    self.idle(Some(Instant::now() + delay))
                }
                Next::Stopped => {
                    self.reporter.state(State::Stopped);
                    self.idle(None)
                }
                Next::NotStarted => {
                    self.reporter.state(State::NotStarted);
                    self.idle(None)
                }
                Next::Ended => {
                    self.reporter.state(State::Ended);
                    return;
                }
                Next::Quit => return,
            };
        }
    }

    /// Waits with no process, until `until` if given, for an ask.
    fn idle(&mut self, until: Option<Instant>) -> Next {
        loop {
            let event = match until {
                Some(at) => match self
                    .events
                    .recv_timeout(at.saturating_duration_since(Instant::now()))
                {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout) => return Next::Start,
                    Err(RecvTimeoutError::Disconnected) => return Next::Ended,
                },
                None => match self.events.recv() {
                    Ok(event) => event,
                    Err(_) => return Next::Ended,
                },
            };
            match event {
                Event::Restart(again) => {
                    (self.start, self.gate) = *again;
                    self.crashes.forget();
                    return Next::Start;
                }
                Event::ShutDown => return Next::Ended,
                Event::Quit => return Next::Quit,
                Event::Open(..) | Event::Change(..) | Event::Close(..) | Event::Save(..) => {
                    if let Some(ask) = sync::Ask::of(event) {
                        let _ = self.document(None, ask);
                    }
                }
                Event::Ask(ask) => self.ask_idle(ask),
                // An earlier run's news.
                Event::Frame(..) | Event::ReadEnd(..) | Event::WriteFailed(..) => {}
            }
        }
    }

    /// One process's life, from the gate to its end.
    fn one_run(&mut self) -> Next {
        self.reporter.state(State::Starting);
        let found = match (self.gate)() {
            Ok(found) => found,
            Err(why) => {
                self.say(format!("not started: {why}"));
                return Next::NotStarted;
            }
        };
        for note in found.notes {
            self.reporter.log(note);
        }
        let run = match self.spawn(&found.executable, &found.args) {
            Ok(run) => run,
            Err(error) => {
                self.say(format!("could not start: {error}"));
                return Next::NotStarted;
            }
        };
        let end = self.drive(run);
        self.documents.forget_sent();
        self.diagnosed.clear();
        match end {
            End::Crashed(why) => {
                self.reporter.log(format!("crashed: {why}"));
                self.end_process();
                match self.crashes.record(Instant::now()) {
                    Some(delay) => Next::After(delay),
                    None => {
                        self.say(format!(
                            "stopped after crashing 3 times within 3 minutes, last as it {why}; \
                             lsp-restart starts it again"
                        ));
                        Next::Stopped
                    }
                }
            }
            End::Stopped(after) => {
                self.end_process();
                match after {
                    After::Ended => Next::Ended,
                    After::Restart(again) => {
                        (self.start, self.gate) = *again;
                        self.crashes.forget();
                        Next::Start
                    }
                }
            }
            End::Quit => Next::Quit,
        }
    }

    /// A message for the person, also in `*LSP*`.
    fn say(&mut self, text: String) {
        self.reporter.log(text.clone());
        self.reporter
            .show(Severity::Error, text, false, Instant::now());
    }

    /// Spawns the process and its pipe threads, and sends `initialize`.
    fn spawn(&mut self, executable: &Executable, args: &[std::ffi::OsString]) -> io::Result<Run> {
        let start = &self.start;
        let inherit: Vec<&str> = start.inherit.iter().map(String::as_str).collect();
        let mut child = process::spawn(&Spawn {
            executable,
            args,
            cwd: Some(&start.root),
            project: Some(&start.root),
            inherit: &inherit,
            stdin: Source::Piped,
            stdout: Sink::Piped,
            stderr: Sink::Piped,
            own_process_group: cfg!(unix),
            set: &[],
            terminal: None,
            pass_fd: None,
        })?;
        let pipes = match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
            (Some(stdin), Some(stdout), Some(stderr)) => (stdin, stdout, stderr),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("the process has no pipes"));
            }
        };
        self.run += 1;
        let started = pipes::start(start.number, self.run, pipes, &self.sender);
        let (writer, credit, stderr) = match started {
            Ok(threads) => threads,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        self.reporter.log(format!(
            "started {} (process {}) in {}",
            executable.path().display(),
            child.id(),
            start.root.display()
        ));
        *self.child.lock().unwrap_or_else(PoisonError::into_inner) = Some(child);
        let now = Instant::now();
        let mut run = Run {
            id: self.run,
            writer: Some(writer),
            credit,
            stderr,
            next_id: 1,
            pending: HashMap::new(),
            started: now,
            ready: false,
            tokens: HashMap::new(),
            percent: None,
            flood: Flood::new(now),
            stopping: None,
            output_ended: false,
            diagnostics_dropped: 0,
            diagnostics_said: now,
            look_at: now + LIVENESS_INTERVAL,
            encoding: crate::position::Encoding::Utf16,
            sync: protocol::Sync {
                open_close: false,
                change: protocol::SyncKind::None,
                save: false,
            },
        };
        let root_uri = uri::from_path(&start.root).unwrap_or_default();
        let root_name = start
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let params = protocol::initialize_params(&Initialize {
            root_uri: &root_uri,
            root_name: &root_name,
            options: start.options.as_ref(),
        });
        // A queue that is empty takes it.
        let _ = run.request(Asked::Initialize, "initialize", params, INITIALIZE_WAIT);
        Ok(run)
    }

    /// The run's loop, until it ends.
    fn drive(&mut self, mut run: Run) -> End {
        let end = loop {
            let now = Instant::now();
            if let Some(end) = self.tick(&mut run, now) {
                break end;
            }
            let wait = run
                .next_deadline()
                .min(self.reporter.flush(now).map_or(run.look_at, |d| now + d))
                .saturating_duration_since(now);
            let event = match self.events.recv_timeout(wait) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break End::Quit,
            };
            if let Some(end) = self.event(&mut run, event) {
                break end;
            }
        };
        run.credit.close();
        drop(run.writer.take());
        self.fail_asks(&mut run);
        if run.diagnostics_dropped > 0 {
            self.dropped_diagnostics(&mut run);
        }
        end
    }

    /// What is due at `now`: the stderr's lines, the waits of requests and of a stop, and a
    /// look at whether the process ended.
    fn tick(&mut self, run: &mut Run, now: Instant) -> Option<End> {
        // Whole texts that wait for room; while some do, the queue is looked at again soon.
        if let Err(full) = self.send_waiting(run) {
            return Some(End::Crashed(full));
        }
        if self.documents.waiting() {
            run.look_at = run.look_at.min(now + WAITING_POLL);
        }
        let (lines, dropped) = run.stderr.take();
        if dropped > 0 {
            self.reporter
                .log(format!("({dropped} lines of standard error dropped)"));
        }
        for line in lines {
            self.reporter.log(format!("stderr: {line}"));
        }
        if run.diagnostics_dropped > 0
            && now.duration_since(run.diagnostics_said) >= Duration::from_secs(1)
        {
            run.diagnostics_said = now;
            self.dropped_diagnostics(run);
        }
        for (id, ask) in run.overdue_asks(now) {
            if let Err(full) = self.overdue_ask(run, id, ask) {
                return Some(End::Crashed(full));
            }
        }
        if let Some((asked, _)) = run.overdue(now) {
            run.pending.retain(|_, (_, deadline)| *deadline > now);
            match asked {
                Asked::Place(_) => {}
                Asked::Initialize => {
                    return Some(End::Crashed(format!(
                        "did not answer initialize within {} s",
                        INITIALIZE_WAIT.as_secs()
                    )));
                }
                Asked::Shutdown => run.exit(now),
            }
        }
        if let Some((phase, deadline, _)) = &run.stopping
            && *deadline <= now
        {
            match phase {
                Phase::Shutdown => run.exit(now),
                Phase::Exit => {
                    self.signal(Signal::Terminate);
                    if let Some(stopping) = &mut run.stopping {
                        stopping.0 = Phase::Terminate;
                        stopping.1 = now + EXIT_WAIT;
                    }
                }
                Phase::Terminate => {
                    self.signal(Signal::Kill);
                    return run.stopping.take().map(|(.., after)| End::Stopped(after));
                }
            }
        }
        if run.look_at <= now {
            run.look_at = now + LIVENESS_INTERVAL;
            if self.exited() {
                return Some(match run.stopping.take() {
                    Some((.., after)) => End::Stopped(after),
                    None => End::Crashed("exited".into()),
                });
            }
        }
        None
    }

    /// Handles one event; `Some` when it ends the run.
    fn event(&mut self, run: &mut Run, event: Event) -> Option<End> {
        match event {
            Event::Restart(again) => run.stop(After::Restart(again), Instant::now()),
            Event::ShutDown => run.stop(After::Ended, Instant::now()),
            Event::Quit => {
                self.signal(Signal::Terminate);
                return Some(End::Quit);
            }
            Event::Open(..) | Event::Change(..) | Event::Close(..) | Event::Save(..) => {
                if let Some(ask) = sync::Ask::of(event) {
                    return self.document(Some(run), ask).err().map(End::Crashed);
                }
            }
            Event::Ask(ask) => return self.ask(run, ask).err().map(End::Crashed),
            Event::Frame(id, frame) if id == run.id => {
                return self.frame(run, frame).err().map(End::Crashed);
            }
            Event::ReadEnd(id, error) if id == run.id => {
                run.output_ended = true;
                if run.stopping.is_none() {
                    return Some(End::Crashed(pipes::ended(error.as_ref())));
                }
                // Asked to stop: the process's end is looked for at once.
                run.look_at = Instant::now();
            }
            Event::WriteFailed(id, kind) if id == run.id && run.stopping.is_none() => {
                return Some(End::Crashed(format!(
                    "its input could not be written: {kind}"
                )));
            }
            Event::Frame(..) | Event::ReadEnd(..) | Event::WriteFailed(..) => {}
        }
        None
    }

    /// Sends `signal` to the process's group, if it runs.
    fn signal(&self, signal: Signal) {
        if let Some(child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            let _ = child.signal_group(signal);
        }
    }

    /// Whether the process has ended, without reaping it.
    fn exited(&self) -> bool {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            // A look that fails is no end: the process is still signalled and waited for.
            .is_none_or(|child| child.exited().unwrap_or(false))
    }

    /// Ends the process and what it started: `SIGTERM` to its group, then `SIGKILL` to the
    /// group, after [`EXIT_WAIT`] if the process still runs, or after [`GROUP_GRACE`] once it
    /// ended, for what it left there that ignores `SIGTERM`. Both go before the process is
    /// reaped, while its group's number is still its own. One that does not end even then is
    /// reaped by a thread of its own, so that it never runs unreaped beside the next.
    fn end_process(&mut self) {
        self.signal(Signal::Terminate);
        let ended = self.wait_for_end(EXIT_WAIT);
        if ended {
            thread::sleep(GROUP_GRACE);
        }
        self.signal(Signal::Kill);
        let ended = ended || self.wait_for_end(EXIT_WAIT);
        let child = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(mut child) = child else {
            return;
        };
        if !ended {
            self.reporter
                .log("did not end after SIGKILL; it is reaped when it does".into());
            let reaper = thread::Builder::new()
                .name(format!("oso-lsp-{}-k", self.start.number))
                .spawn(move || {
                    let _ = child.wait();
                });
            if reaper.is_err() {
                self.reporter.log("no thread could wait for it".into());
            }
            return;
        }
        match child.try_wait() {
            Ok(Some(status)) => self.reporter.log(format!("ended: {status}")),
            Ok(None) => self.reporter.log("its end is not known".into()),
            Err(error) => self.reporter.log(format!("its end is not known: {error}")),
        }
    }

    /// Waits up to `wait` for the process to end; whether it did.
    fn wait_for_end(&self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            if self.exited() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(POLL);
        }
    }
}
