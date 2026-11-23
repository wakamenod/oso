//! A dictation's life on its own thread (ADR 0175): it reads the discovery file, asks
//! TypeWhisper to start, waits while it records, asks it to stop, and polls until the text
//! comes, saying each step to the host. A call never runs on the UI thread.
//!
//! Must not depend on: `app`, the editor or how a call is made: the thread gets a
//! [`Transport`], which the tests fake, and the host gets [`Event`]s.
//!
//! - While it records, the thread looks once every [`Timing::recording_poll`] whether
//!   TypeWhisper still does, as its own hotkey or window can stop it. Past
//!   `[dictation] max_seconds` it stops it.
//! - Once it stopped, the thread polls every [`Timing::text_poll`], for up to
//!   [`Timing::text_limit`].
//! - Nothing is tried twice: a call that fails ends the dictation with what to say.
//! - When the host goes, as oso exits, the thread starts a stop and does not wait for it, if the
//!   dictation still records.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::api::{Answer, Call, DictationId, Failure, Transport, cleaned, said};
use super::discovery::{self, Discovery, Unread};

/// What the person is told when nothing answers on the port, or there is no discovery file.
pub const NOT_RUNNING: &str = "TypeWhisper isn't running, or is its API server on?";

/// How long the thread waits between its looks.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Between looks while it records.
    pub recording_poll: Duration,
    /// Between looks once it stopped.
    pub text_poll: Duration,
    /// How long it waits for the text once it stopped.
    pub text_limit: Duration,
    /// How long after the text the thread says it has settled, for the input method's wait.
    pub settle: Duration,
}

/// The thread's timing.
pub const TIMING: Timing = Timing {
    recording_poll: Duration::from_secs(1),
    text_poll: Duration::from_millis(300),
    text_limit: Duration::from_secs(120),
    settle: Duration::from_secs(10),
};

/// What a dictation is started with, from `[dictation]`.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Where TypeWhisper's discovery file is.
    pub discovery: PathBuf,
    /// The workflow to dictate in, a UUID, if any.
    pub workflow: Option<String>,
    /// The longest it records.
    pub max_recording: Duration,
}

/// What TypeWhisper heard, as it gave it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// The text, with its corrections made.
    pub text: String,
    /// What was recognised, before them, if it says.
    pub raw: Option<String>,
}

/// Why a dictation stopped recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The host asked.
    Asked,
    /// It recorded for `[dictation] max_seconds`.
    Limit,
    /// TypeWhisper stopped it itself, from its own hotkey or window.
    Elsewhere,
}

/// What the thread tells the host, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// TypeWhisper records.
    Recording,
    /// It stopped recording, and the text is on its way.
    Waiting(Stopped),
    /// The text TypeWhisper heard.
    Text(Heard),
    /// The dictation ended with no text, and what to say.
    Failed(String),
    /// [`Timing::settle`] has passed since the text came.
    Settled,
}

/// What the host tells the thread.
enum Order {
    /// Stop recording.
    Stop,
}

/// The host's handle on a dictation's thread. Dropping it lets the thread go: one that still
/// records starts a stop first.
pub struct Handle {
    orders: Sender<Order>,
    /// Says once the thread has started its stop at an exit, or ended.
    gone: Receiver<()>,
}

impl Handle {
    /// Asks the thread to stop recording. Nothing happens if it no longer records.
    pub fn stop(&self) {
        let _ = self.orders.send(Order::Stop);
    }

    /// Lets the thread go, as oso exits, waiting at most `wait` for it to have started its
    /// stop, if it still records.
    pub fn leave(self, wait: Duration) {
        let Self { orders, gone } = self;
        drop(orders);
        let _ = gone.recv_timeout(wait);
    }
}

/// What makes a dictation's [`Transport`], on its thread, as making one looks at the disk; or
/// why none can be made.
pub type MakeTransport = Box<dyn FnOnce() -> Result<Box<dyn Transport>, String> + Send>;

/// Starts a dictation's thread with `settings`, calling TypeWhisper through the transport
/// `make` makes there, with `timing`, and telling the host each [`Event`] through `report`.
///
/// # Errors
///
/// When the thread cannot be started.
pub fn start(
    settings: Settings,
    make: MakeTransport,
    timing: Timing,
    report: impl Fn(Event) + Send + 'static,
) -> io::Result<Handle> {
    let (orders, inbox) = mpsc::channel();
    let (went, gone) = mpsc::channel();
    thread::Builder::new()
        .name("oso-dictation".into())
        .spawn(move || {
            let transport = match make() {
                Ok(transport) => transport,
                Err(why) => {
                    report(Event::Failed(why));
                    let _ = went.send(());
                    return;
                }
            };
            let mut dictation = Dictation {
                transport,
                inbox,
                timing,
                report: &report,
                left: false,
            };
            dictation.run(&settings);
            let _ = went.send(());
        })?;
    Ok(Handle { orders, gone })
}

/// The thread's state.
struct Dictation<'r, R: Fn(Event)> {
    transport: Box<dyn Transport>,
    inbox: Receiver<Order>,
    timing: Timing,
    report: &'r R,
    /// The host has gone.
    left: bool,
}

/// Where TypeWhisper says a dictation is.
#[derive(Debug, PartialEq, Eq)]
enum Progress {
    Recording,
    Processing,
    Completed(Heard),
    Failed(String),
}

impl<R: Fn(Event)> Dictation<'_, R> {
    fn run(&mut self, settings: &Settings) {
        let discovery = match discovery::read(&settings.discovery) {
            Ok(discovery) => discovery,
            Err(Unread::Missing) => return self.fail(NOT_RUNNING.to_owned()),
            Err(Unread::Refused(why)) => {
                return self.fail(format!("TypeWhisper's discovery file is refused: {why}"));
            }
        };
        let start = Call::Start {
            workflow: settings.workflow.clone(),
        };
        let id = match self.transport.call(&discovery, &start) {
            Ok(answer) if answer.status == 200 => match started(&answer.body) {
                Some(id) => id,
                None => return self.fail("TypeWhisper's answer named no dictation".to_owned()),
            },
            Ok(answer) => return self.fail(refusal(&answer)),
            Err(failure) => return self.fail(unanswered(&failure, &start)),
        };
        (self.report)(Event::Recording);
        let Some(stopped) = self.record(&discovery, &id, settings.max_recording) else {
            return;
        };
        if self.left {
            return;
        }
        (self.report)(Event::Waiting(stopped));
        self.wait_for_text(&discovery, &id);
    }

    /// Waits while it records, until the host asks it to stop, the limit, or TypeWhisper
    /// stops on its own. Says why it stopped, or nothing once it has failed or the host left.
    fn record(
        &mut self,
        discovery: &Discovery,
        id: &DictationId,
        limit: Duration,
    ) -> Option<Stopped> {
        let began = Instant::now();
        loop {
            let left = limit.saturating_sub(began.elapsed());
            let stop = match self
                .inbox
                .recv_timeout(self.timing.recording_poll.min(left))
            {
                Ok(Order::Stop) => Some(Stopped::Asked),
                Err(RecvTimeoutError::Disconnected) => {
                    // oso exits: a stop goes, with no wait for it.
                    self.left = true;
                    self.transport.call_and_go(discovery, &Call::Stop);
                    return None;
                }
                Err(RecvTimeoutError::Timeout) if began.elapsed() >= limit => Some(Stopped::Limit),
                Err(RecvTimeoutError::Timeout) => None,
            };
            if let Some(stopped) = stop {
                return self.stop(discovery, stopped);
            }
            match self.progress(discovery, id) {
                Ok(Progress::Recording) => {}
                Ok(_) => return Some(Stopped::Elsewhere),
                Err(why) => {
                    self.fail(why);
                    return None;
                }
            }
        }
    }

    /// Asks TypeWhisper to stop. One that stopped already says so with a 409, which is no
    /// failure: the text comes all the same.
    fn stop(&mut self, discovery: &Discovery, stopped: Stopped) -> Option<Stopped> {
        match self.transport.call(discovery, &Call::Stop) {
            Ok(answer) if answer.status == 200 || answer.status == 409 => Some(stopped),
            Ok(answer) => {
                self.fail(refusal(&answer));
                None
            }
            Err(failure) => {
                self.fail(unanswered(&failure, &Call::Stop));
                None
            }
        }
    }

    /// Polls until the text comes, TypeWhisper fails, or the limit.
    fn wait_for_text(&mut self, discovery: &Discovery, id: &DictationId) {
        let began = Instant::now();
        loop {
            match self.progress(discovery, id) {
                Ok(Progress::Completed(heard)) => {
                    (self.report)(Event::Text(heard));
                    return self.settle();
                }
                Ok(Progress::Failed(why)) => return self.fail(why),
                Ok(Progress::Recording | Progress::Processing) => {}
                Err(why) => return self.fail(why),
            }
            if began.elapsed() >= self.timing.text_limit {
                let limit = self.timing.text_limit.as_secs();
                return self.fail(format!("No text came from TypeWhisper in {limit} s"));
            }
            if self.pause(self.timing.text_poll) {
                return;
            }
        }
    }

    /// Waits [`Timing::settle`] and says so, unless the host goes first.
    fn settle(&mut self) {
        if !self.pause(self.timing.settle) {
            (self.report)(Event::Settled);
        }
    }

    /// Waits `time`, taking no order: the host asks nothing more of a dictation that stopped.
    /// Says whether the host went.
    fn pause(&mut self, time: Duration) -> bool {
        let until = Instant::now() + time;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.inbox.recv_timeout(left) {
                Ok(Order::Stop) => {}
                Err(RecvTimeoutError::Timeout) => return false,
                Err(RecvTimeoutError::Disconnected) => {
                    self.left = true;
                    return true;
                }
            }
        }
    }

    /// Where TypeWhisper says the dictation `id` is, or why that is not known.
    fn progress(&mut self, discovery: &Discovery, id: &DictationId) -> Result<Progress, String> {
        let call = Call::Transcription(id.clone());
        match self.transport.call(discovery, &call) {
            Ok(answer) if answer.status == 200 => progress(&answer.body),
            Ok(answer) if answer.status == 404 => {
                Err("TypeWhisper no longer has the dictation".to_owned())
            }
            Ok(answer) => Err(refusal(&answer)),
            Err(Failure::NotRunning | Failure::Hung) => {
                Err("TypeWhisper stopped answering".to_owned())
            }
            Err(failure) => Err(unanswered(&failure, &call)),
        }
    }

    fn fail(&mut self, why: String) {
        if !self.left {
            (self.report)(Event::Failed(why));
        }
    }
}

/// The id in a start's answer, `{"id": ..., "status": "recording"}`.
fn started(body: &[u8]) -> Option<DictationId> {
    let value: Value = serde_json::from_slice(body).ok()?;
    DictationId::of(value.get("id")?.as_str()?)
}

/// Where a transcription's answer, `{"status": ..., "transcription": {"text": ...}}`, says the
/// dictation is.
fn progress(body: &[u8]) -> Result<Progress, String> {
    let unread = || "TypeWhisper's answer was not one oso reads".to_owned();
    let value: Value = serde_json::from_slice(body).map_err(|_| unread())?;
    match value.get("status").and_then(Value::as_str) {
        Some("recording") => Ok(Progress::Recording),
        Some("processing") => Ok(Progress::Processing),
        Some("completed") => value
            .get("transcription")
            .and_then(|transcription| {
                let text = |key: &str| Some(transcription.get(key)?.as_str()?.to_owned());
                Some(Progress::Completed(Heard {
                    text: text("text")?,
                    raw: text("raw_text"),
                }))
            })
            .ok_or_else(unread),
        Some("failed") => {
            let error = value.get("error").and_then(Value::as_str).and_then(cleaned);
            Ok(Progress::Failed(match error {
                Some(error) => format!("TypeWhisper failed: {error}"),
                None => "TypeWhisper failed".to_owned(),
            }))
        }
        _ => Err(unread()),
    }
}

/// What to say of an answer that refused a call.
fn refusal(answer: &Answer) -> String {
    let said = said(&answer.body);
    match (answer.status, said) {
        (401, _) => "TypeWhisper refused oso's token: is its discovery file current?".to_owned(),
        (409, Some(said)) => format!("TypeWhisper can't dictate now: {said}"),
        (409, None) => "TypeWhisper can't dictate now".to_owned(),
        (status, Some(said)) => format!("TypeWhisper answered {status}: {said}"),
        (status, None) => format!("TypeWhisper answered {status}"),
    }
}

/// What to say of a call that got no answer.
fn unanswered(failure: &Failure, call: &Call) -> String {
    match failure {
        Failure::NotRunning => NOT_RUNNING.to_owned(),
        Failure::NoAnswer => format!("TypeWhisper did not answer in {} s", call.limit().as_secs()),
        Failure::Hung => "TypeWhisper closed the connection with no answer".to_owned(),
        Failure::TooLarge => "TypeWhisper's answer was too large".to_owned(),
        Failure::Curl(why) => format!("Dictation could not call TypeWhisper: {why}"),
    }
}

#[cfg(test)]
mod tests;
