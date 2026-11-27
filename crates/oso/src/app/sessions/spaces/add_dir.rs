//! A repository given to a space's running terminal orchestrator without asking (ADR 0185):
//! oso types its own `/add-dir PATH` into the orchestrator's input and submits it, once the
//! orchestrator is idle with an empty input box.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - **What is typed:** a [`Command`] alone, which only `Command::add_dir` makes, from the
//!   space's recorded path while that is one plain line; then one Enter, as a key of its own.
//!   No text an agent gave goes this way.
//! - **When:** one at a time, once all of these hold: the session is still the space's running
//!   orchestrator and the repository still one of the space's that oso trusts
//!   (`App::add_dir_holds`); the terminal takes a paste (`App::paste_target`: judged, no dialog
//!   or permission prompt, no full-screen view, no password, bracketed paste on); its turn
//!   ended; its input box is empty; the person typed nothing for [`TYPING_PAUSE`]; and no
//!   review question's Enter waits on it. Until then it waits, with no end, and says so once
//!   in `*Messages*`, and in the echo area while that says nothing else.
//! - **The Enter** goes as a review question's does (ADR 0163): at least `ENTER_AFTER` after the
//!   paste, once the screen changed since, within `ENTER_WAIT`, and only while the box holds the
//!   command and nothing else (`sessions::send::holds_command`) and the person typed nothing
//!   since the paste; else the command stays in the box, not sent, and the echo area says why.
//! - **Skipped,** saying so: a session gone or no longer running, which gets the repository with
//!   `--add-dir` at its next start, and an offer that no longer holds.
//! - **After the Enter:** Claude Code may ask its own question, to trust a folder its trust does
//!   not cover. oso never answers it: once it shows, the echo area says so, once.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;

use crate::app::App;
use crate::app::reviews::{ENTER_AFTER, ENTER_POLL, ENTER_RECHECK, ENTER_WAIT};
use crate::app::sessions::{Outgoing, Shown};
use crate::sessions::send::{Command, InputBox, holds_command};
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::stored::same;
use crate::sessions::state::State;
use crate::sessions::worker::SessionKey;

/// How long the person must not have typed before oso types into a terminal, so that no key of
/// theirs lands in the command, nor the command in what they type.
pub(in crate::app) const TYPING_PAUSE: Duration = Duration::from_secs(1);
/// How long after the Enter a question Claude Code asks of the command is looked for.
const ASKS_WITHIN: Duration = Duration::from_secs(5);
/// How long after the Enter nothing more is typed into any terminal, even once its box shows
/// empty: the TUI may draw an empty box for a frame before its question.
const SETTLE: Duration = Duration::from_secs(1);

/// The `/add-dir`s oso types into running terminal orchestrators.
#[derive(Debug, Default)]
pub(in crate::app) struct AddDirs {
    /// Those waiting for their orchestrator, in the order they came.
    due: Vec<Due>,
    /// The one typed, whose Enter waits.
    typed: Option<Typed>,
    /// The one sent last, while a question Claude Code asks of it is looked for.
    sent: Option<Sent>,
}

/// An `/add-dir` waiting for orchestrator `key` of `space` to take it.
#[derive(Debug)]
struct Due {
    key: SessionKey,
    space: SpaceId,
    root: PathBuf,
    command: Command,
    /// Whether the echo area said it waits, or a caller's echo did.
    told: bool,
}

/// An `/add-dir` typed into terminal `buffer`, whose screen was at `generation` then: its
/// Enter is looked at from `next`, given up at `until`.
#[derive(Debug)]
struct Typed {
    due: Due,
    buffer: BufferId,
    generation: Option<u64>,
    at: Instant,
    next: Instant,
    until: Instant,
}

/// An `/add-dir` sent to terminal `buffer`, its screen at `generation` then: looked after
/// until `until`, and at least until `settled`.
#[derive(Debug)]
struct Sent {
    key: SessionKey,
    root: PathBuf,
    buffer: BufferId,
    generation: Option<u64>,
    settled: Instant,
    until: Instant,
}

impl App {
    /// Queues `command`, `/add-dir root`, for terminal orchestrator `key` of `space`, typed and
    /// sent once it takes it; `told` when the caller's echo says it already. One queued or
    /// typed already for the same session and repository is not queued again.
    pub(in crate::app) fn queue_add_dir(
        &mut self,
        (key, space): (SessionKey, SpaceId),
        root: PathBuf,
        command: Command,
        told: bool,
    ) {
        let adds = &self.hosting.spaces.add_dirs;
        let has = |due: &Due| due.key == key && same(&due.root, &root);
        if adds.due.iter().any(has) || adds.typed.as_ref().is_some_and(|typed| has(&typed.due)) {
            return;
        }
        self.hosting.spaces.add_dirs.due.push(Due {
            key,
            space,
            root,
            command,
            told,
        });
    }

    /// Whether an `/add-dir` typed into session `key`'s terminal waits for its Enter, or was
    /// sent and its screen has not settled, so that a review's question is not pasted after it.
    pub(in crate::app) fn add_dir_typed_into(&self, key: SessionKey) -> bool {
        let adds = &self.hosting.spaces.add_dirs;
        adds.typed
            .as_ref()
            .is_some_and(|typed| typed.due.key == key)
            || adds.sent.as_ref().is_some_and(|sent| sent.key == key)
    }

    /// After each event, and when [`App::add_dirs_deadline`] passed: the typed `/add-dir`'s
    /// Enter is looked for, a question after one sent, and the next that may go is typed.
    pub(in crate::app) fn sync_add_dirs(&mut self) {
        let adds = &self.hosting.spaces.add_dirs;
        if adds.due.is_empty() && adds.typed.is_none() && adds.sent.is_none() {
            return;
        }
        let now = Instant::now();
        self.look_for_add_dir_enter(now);
        self.look_after_add_dir(now);
        self.type_next_add_dir(now);
    }

    /// When the `/add-dir`s next want a look: the Enter's next look, the end of the look for a
    /// question after one sent, or the end of the person's typing pause while one waits.
    pub(in crate::app) fn add_dirs_deadline(&self) -> Option<Instant> {
        let adds = &self.hosting.spaces.add_dirs;
        let enter = adds.typed.as_ref().map(|typed| typed.next);
        let now = Instant::now();
        let sent = (adds.sent.as_ref()).and_then(|sent| {
            [sent.settled, sent.until]
                .into_iter()
                .filter(|at| *at > now)
                .min()
        });
        let pause = (!adds.due.is_empty())
            .then(|| self.agent_person_typed().map(|at| at + TYPING_PAUSE))
            .flatten()
            .filter(|at| *at > now);
        [enter, sent, pause].into_iter().flatten().min()
    }

    /// Before the loop waits, after the screens of terminals no window shows were taken: the
    /// `/add-dir`s, as [`looks_before_waiting`] says.
    pub(in crate::app) fn add_dirs_due(&mut self, now: Instant) {
        let queued = !self.hosting.spaces.add_dirs.due.is_empty();
        if looks_before_waiting(self.add_dirs_deadline(), queued, now) {
            self.sync_add_dirs();
        }
    }

    /// Types the first `/add-dir` whose orchestrator takes it now, while none is typed and the
    /// screen after the last one sent has settled; drops, saying so, each that can no longer
    /// go; says once of each other that it waits.
    fn type_next_add_dir(&mut self, now: Instant) {
        let adds = &self.hosting.spaces.add_dirs;
        if adds.typed.is_some() || adds.sent.is_some() {
            return;
        }
        let mut at = 0;
        while let Some(due) = self.hosting.spaces.add_dirs.due.get(at) {
            let (key, shown) = (due.key, due.root.display().to_string());
            let session = self.hosting.table().get(key);
            let name = session.map_or_else(String::new, |session| session.name.clone());
            let dropped = match session {
                None => Some(format!(
                    "/add-dir {shown} was not typed: its orchestrator is gone"
                )),
                Some(session) if !session.runs() => Some(format!(
                    "{name} is not running: /add-dir {shown} was not typed; it gets {shown} with \
                     --add-dir when it starts"
                )),
                Some(_) if !self.add_dir_holds(key, &due.space, &due.root) => Some(format!(
                    "/add-dir {shown} was not typed: the space or its orchestrator changed \
                     meanwhile"
                )),
                Some(_) => None,
            };
            if let Some(why) = dropped {
                self.hosting.spaces.add_dirs.due.remove(at);
                self.editor.message(why);
                continue;
            }
            match self.takes_add_dir(key, now) {
                Some(buffer) => return self.type_add_dir(at, buffer, now),
                None => {
                    let due = &mut self.hosting.spaces.add_dirs.due[at];
                    if !std::mem::replace(&mut due.told, true) {
                        let said = format!(
                            "{name} gets /add-dir {shown} once its turn ends and its input box \
                             is empty"
                        );
                        // Never over news, as the add's or the move's own line.
                        self.editor.note(said.clone());
                        self.editor.echo_unless_news("", said);
                    }
                    at += 1;
                }
            }
        }
    }

    /// The terminal of session `key`, if an `/add-dir` may be typed into it now: it takes a
    /// paste, its turn ended, its input box is empty, the person paused typing, and no review
    /// question's Enter waits on it.
    fn takes_add_dir(&self, key: SessionKey, now: Instant) -> Option<BufferId> {
        let buffer = self.paste_target(key).ok()?;
        let ended = matches!(
            self.judged_state(key),
            Some(State::Idle | State::Done | State::ReadyForReview)
        );
        let paused = (self.agent_person_typed()).is_none_or(|at| now >= at + TYPING_PAUSE);
        let empty = self.input_box_of(buffer) == InputBox::Empty;
        (ended && paused && empty && !self.review_enter_waits(key)).then_some(buffer)
    }

    /// Types the `/add-dir` queued `at` into `buffer`, not shown, its Enter due later.
    fn type_add_dir(&mut self, at: usize, buffer: BufferId, now: Instant) {
        let due = self.hosting.spaces.add_dirs.due.remove(at);
        let generation = self.screen_generation(buffer);
        if let Err(why) =
            self.paste_into_session(due.key, Outgoing::Command(&due.command), Shown::Nothing)
        {
            return self.editor.message(format!(
                "/add-dir {} was not typed: {why}",
                due.root.display()
            ));
        }
        self.hosting.spaces.add_dirs.typed = Some(Typed {
            due,
            buffer,
            generation,
            at: now,
            next: now + ENTER_AFTER,
            until: now + ENTER_WAIT,
        });
    }

    /// Sends the typed `/add-dir`'s Enter once its time came and the screen changed since the
    /// paste, only while the offer still holds, the box holds the command alone and the person
    /// typed nothing since; else it is left in the box, not sent, saying why.
    fn look_for_add_dir_enter(&mut self, now: Instant) {
        let Some(typed) = &self.hosting.spaces.add_dirs.typed else {
            return;
        };
        if now < typed.next {
            return;
        }
        let (buffer, generation, until) = (typed.buffer, typed.generation, typed.until);
        let came = self.take_screen_now(buffer);
        let current = self.screen_generation(buffer) != generation && !came;
        if !current && now < until {
            if let Some(typed) = &mut self.hosting.spaces.add_dirs.typed {
                typed.next = now + if came { ENTER_RECHECK } else { ENTER_POLL };
            }
            return;
        }
        let Some(typed) = self.hosting.spaces.add_dirs.typed.take() else {
            return;
        };
        let Due {
            key, space, root, ..
        } = &typed.due;
        let name = (self.hosting.table().get(*key)).map_or_else(String::new, |s| s.name.clone());
        let rows = (self.terminals.map.get(buffer)).map(|hosted| hosted.grid.row_texts());
        let typed_since = self.agent_person_typed().is_some_and(|at| at >= typed.at);
        let why = if !current {
            Some("its screen showed nothing newer than the command".to_owned())
        } else if !self.add_dir_holds(*key, space, root) {
            Some("the space or its orchestrator changed meanwhile".to_owned())
        } else if !rows.is_some_and(|rows| holds_command(&rows, &typed.due.command)) {
            Some("its input box holds more than the command, or a menu shows".to_owned())
        } else if typed_since {
            Some("you typed meanwhile".to_owned())
        } else {
            self.submit_paste(*key, buffer).err()
        };
        let shown = root.display();
        if let Some(why) = why {
            return self.editor.message(format!(
                "/add-dir {shown} waits in {name}'s input box, not sent: {why}; press Enter there"
            ));
        }
        self.editor
            .message(format!("Typed /add-dir {shown} into {name} and sent it"));
        let (key, space, root) = (typed.due.key, typed.due.space, typed.due.root);
        let generation = self.screen_generation(buffer);
        self.hosting.spaces.add_dirs.sent = Some(Sent {
            key,
            root: root.clone(),
            buffer,
            generation,
            settled: now + SETTLE,
            until: now + ASKS_WITHIN,
        });
        (self.hosting.spaces.given.entry(space))
            .or_default()
            .push(root);
    }

    /// After an `/add-dir` was sent: once the screen changed, a question Claude Code asks of
    /// it, a dialog or no input box, is said once, never answered. An empty box ends the look
    /// once [`SETTLE`] passed, as [`ASKS_WITHIN`] does; until then nothing more is typed.
    fn look_after_add_dir(&mut self, now: Instant) {
        let Some(sent) = &self.hosting.spaces.add_dirs.sent else {
            return;
        };
        let (key, buffer, settled, until) = (sent.key, sent.buffer, sent.settled, sent.until);
        let changed = self.screen_generation(buffer) != sent.generation;
        let boxed = self.input_box_of(buffer);
        let asks = changed
            && (matches!(self.judged_state(key), Some(State::NeedsInput(_)))
                || matches!(boxed, InputBox::Other(_)));
        if asks {
            let Some(sent) = self.hosting.spaces.add_dirs.sent.take() else {
                return;
            };
            let name = (self.hosting.table().get(key)).map_or_else(String::new, |s| s.name.clone());
            return self.editor.message(format!(
                "{name} asks you to trust {}: answer it in {name}",
                sent.root.display()
            ));
        }
        let done = now >= until || (now >= settled && changed && boxed == InputBox::Empty);
        if done {
            self.hosting.spaces.add_dirs.sent = None;
        }
    }
}

/// Whether the `/add-dir`s are looked at as the loop is about to wait: past their `deadline`,
/// or while one is `queued`, whatever the deadline. A terminal no window shows takes its new
/// screen in that pass, which is no event, so an empty box there would wait for some unrelated
/// event else. It sets no wake of its own.
fn looks_before_waiting(deadline: Option<Instant>, queued: bool, now: Instant) -> bool {
    queued || deadline.is_some_and(|due| due <= now)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::looks_before_waiting;

    /// A queued `/add-dir` is looked at each time the loop is about to wait, as a terminal no
    /// window shows takes its new screen in that pass, which is no event; else only past the
    /// deadline.
    #[test]
    fn a_queued_add_dir_is_looked_at_before_every_wait() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        assert!(looks_before_waiting(None, true, now), "queued, no deadline");
        assert!(
            looks_before_waiting(Some(later), true, now),
            "queued, deadline ahead"
        );
        assert!(!looks_before_waiting(None, false, now));
        assert!(!looks_before_waiting(Some(later), false, now));
        assert!(
            looks_before_waiting(Some(now), false, now),
            "deadline passed"
        );
    }
}
