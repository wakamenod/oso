//! The question before Remote Control is turned on (ADR 0181): asked by the person's key, with a
//! report in the session's window that says what is sent, where it stays and who may drive the
//! session then.
//!
//! Must not depend on: key presses; the command asks it, and its `y` turns Remote Control on.
//!
//! - It opens from the command alone, and its report at once, with what git says of the repository
//!   filled in once a thread read it, under the trust judged then: what may not be sent closes the
//!   question, saying why.
//! - Its `y` counts only for the session and the conversation it asked about, with its report whole
//!   and in a window, [`QUIET`] after the report was whole, the question opened or oso's window came
//!   in front, and after the key before it; never from a macro, nor as the repeat of a held key.
//!   Everything is judged again then.
//! - It closes as oso's window loses the front or goes out of sight; closed any way, its report
//!   goes, and the transcript is back in its window.
//! - Nothing in the report but oso's own words is drawn unescaped.

use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::minibuffer::PromptId;

use crate::app::agent::{QUIET, Typed};
use crate::app::sessions::show_withheld;
use crate::app::{App, UserEvent};
use crate::commands::agent_sessions::REMOTE_CONFIRMED;
use crate::git::printable;
use crate::sessions::identity::SessionId;
use crate::sessions::start::connect::{self, Sent};
use crate::sessions::start::trust;
use crate::sessions::worker::SessionKey;

/// The most characters of a path in the report.
const MAX_PATH: usize = 400;

const READING: &str = "reading…";

/// What a thread read of a session's repository, for the question numbered `token`.
#[derive(Debug)]
pub(crate) struct Looked {
    token: u64,
    sent: Result<Sent, String>,
}

/// The question, while it is open.
#[derive(Debug)]
pub(in crate::app::sessions::hosting::native) struct Question {
    token: u64,
    prompt: PromptId,
    key: SessionKey,
    of: SessionId,
    report: BufferId,
    /// Asked from the composer, which is selected again after.
    composer: bool,
    /// Since when it may be answered, once its report is whole.
    since: Option<Instant>,
    opened: Instant,
}

/// What the report says.
struct Report<'a> {
    name: &'a str,
    directory: &'a str,
    sent: Option<&'a Sent>,
}

impl Report<'_> {
    fn text(&self) -> String {
        let (remotes, branch) = match self.sent {
            None => (READING.to_owned(), READING.to_owned()),
            Some(sent) => (
                match (sent.addresses.is_empty(), sent.more) {
                    (true, _) => "none".to_owned(),
                    (false, 0) => sent.addresses.join(", "),
                    (false, more) => format!("{} and {more} more", sent.addresses.join(", ")),
                },
                sent.branch.clone().unwrap_or_else(|| "none".to_owned()),
            ),
        };
        format!(
            "Remote Control of {name}\n\
             \n\
             Connected, this goes to Anthropic's servers and stays there:\n\
             \n\
             \x20 - all that was said in this session, so far and from now on\n\
             \x20 - all its tools gave and give, the files they read among it\n\
             \x20 - its name: {name}\n\
             \x20 - its git remotes' addresses: {remotes}\n\
             \x20 - its branch: {branch}\n\
             \x20 - its directory: {directory}\n\
             \n\
             Whoever holds your account can follow it at claude.ai, send it messages, stop it and\n\
             change its permission mode, to auto too, where calls run without asking you. They may\n\
             read its files. Permission questions are answered here alone.\n\
             \n\
             claude ends and resumes its conversation to connect. claude.ai may go on with a session\n\
             it lists for this conversation already. Turned off again, it keeps what it has.\n",
            name = self.name,
            directory = self.directory,
        )
    }
}

/// Whether a `y` the person pressed, as `typed` says, counts of what may be answered `since`.
fn counts(since: Instant, typed: Typed, now: Instant) -> bool {
    let pressed = typed.first.unwrap_or(now).min(now);
    let quiet = |from: Instant| pressed >= from + QUIET;
    quiet(since) && typed.before_first.is_none_or(quiet)
}

impl App {
    /// Asks whether native session `key` is connected to claude.ai, its report in its window.
    pub(super) fn ask_remote(&mut self, key: SessionKey, composer: bool) {
        self.close_remote_question();
        let name = self.remote_name(key);
        if let Some(refused) = self.remote_refused(key) {
            return self.editor.message(refused.message(&name));
        }
        let Some((of, directory, root)) = (self.hosting.table.get(key))
            .map(|s| (s.id.clone(), s.directory.clone(), s.root.clone()))
        else {
            return;
        };
        let judged = match self.native_trust_listed(&directory, root.as_deref(), true) {
            Ok(judged) => judged,
            Err(_) => return self.editor.message("Its project is not trusted now"),
        };
        // In the transcript's window, whose composer goes with it.
        self.composer_go(key, false);
        let transcript = (self.hosting.natives.map.get(&key)).map(|native| native.buffer);
        let window = self.editor.window();
        if window.docked().is_some() || Some(window.buffer()) != transcript {
            return self.editor.message("The session's window is not shown");
        }
        let text = Report {
            name: &name,
            directory: &printable(&directory.display().to_string(), MAX_PATH),
            sent: None,
        }
        .text();
        let mut report = None;
        let title = format!("*Remote Control: {name}*");
        if !show_withheld(&mut self.editor, &mut report, &title, &text) {
            return;
        }
        let Some(report) = report else {
            return;
        };
        let question =
            format!("Connect {name} to claude.ai, its conversation and tool output too? (y or n)");
        let prompt = match self.editor.ask(question, REMOTE_CONFIRMED) {
            Ok(prompt) => prompt,
            Err(e) => {
                self.leave_report(key, report, composer);
                return self.editor.message(format!("Cannot ask: {e}"));
            }
        };
        let natives = &mut self.hosting.natives;
        natives.remote_token += 1;
        let token = natives.remote_token;
        natives.remote_question = Some(Question {
            token,
            prompt,
            key,
            of,
            report,
            composer,
            since: None,
            opened: Instant::now(),
        });
        let waker = self.waker.clone();
        let state = crate::app::reviews::state();
        let spawned = std::thread::Builder::new()
            .name("oso-remote-look".into())
            .spawn(move || {
                let sent = trust::look(judged)
                    .and_then(|root| connect::look(&root, &directory, state.as_deref()))
                    .map(|(sent, _)| sent);
                waker.send(UserEvent::RemoteLooked(Looked { token, sent }));
            });
        if let Err(e) = spawned {
            log::warn!("cannot read a session's repository: {e}");
            self.close_remote_question();
            self.editor.message("Not connected: see the log");
        }
        self.request_redraw();
    }

    /// The thread read what is sent of the session's repository: it is in the report, or it may
    /// not be sent, and the question closes.
    pub(in crate::app) fn remote_looked(&mut self, looked: Looked) {
        let Some(question) = (self.hosting.natives.remote_question.as_ref())
            .filter(|question| question.token == looked.token)
        else {
            return;
        };
        let (key, report) = (question.key, question.report);
        let name = self.remote_name(key);
        match looked.sent {
            Err(why) => {
                self.close_remote_question();
                self.editor.message(format!(
                    "Session {name} is not connected to claude.ai: {why}"
                ));
            }
            Ok(sent) => {
                let directory = (self.hosting.table.get(key))
                    .map(|session| session.directory.display().to_string())
                    .unwrap_or_default();
                let text = Report {
                    name: &name,
                    directory: &printable(&directory, MAX_PATH),
                    sent: Some(&sent),
                }
                .text();
                if self.editor.buffer(report).is_some_and(Buffer::is_internal) {
                    self.editor.set_internal_text(report, &text);
                }
                if let Some(question) = &mut self.hosting.natives.remote_question {
                    question.since = Some(Instant::now().max(question.opened));
                }
            }
        }
        self.request_redraw();
    }

    /// `y` to the question: the session it asked about is connected, if it still may be.
    pub(in crate::app::sessions::hosting::native) fn remote_confirmed(&mut self) {
        let Some(question) = self.hosting.natives.remote_question.take() else {
            return self.editor.message("Nothing was asked");
        };
        let in_window = (self.editor.windows()).any(|window| window.buffer() == question.report);
        let Question {
            key, report, of, ..
        } = question;
        self.leave_report(key, report, question.composer);
        self.request_redraw();
        let still = (self.hosting.table.get(key)).is_some_and(|session| session.id == of);
        let since = question.since.map(|since| self.waits_after_agents(since));
        let typed = self.person_typed_at();
        let refused = match since {
            _ if self.editor.macro_replaying() => Some("a macro does not connect a session"),
            _ if !still => Some("the session asked about is gone"),
            None => Some("what is sent was not read yet; ask again"),
            Some(_) if !in_window => Some("what is sent was not shown; ask again"),
            Some(since) if !counts(since, typed, Instant::now()) => {
                Some("wait a moment after the question, then answer it; ask again")
            }
            Some(_) => None,
        };
        match refused {
            Some(why) => self.editor.message(format!("Not connected: {why}")),
            None => self.remote_on(key),
        }
    }

    /// The question before Remote Control is turned on, if one is open, closes.
    pub(in crate::app) fn close_remote_question(&mut self) {
        if let Some(question) = self.hosting.natives.remote_question.take() {
            self.close_question(question.prompt);
            self.leave_report(question.key, question.report, question.composer);
        }
    }

    /// The person came back to oso's window: the question is new to them.
    pub(in crate::app::sessions::hosting::native) fn remote_question_waits_again(&mut self) {
        if let Some(question) = &mut self.hosting.natives.remote_question {
            let now = Instant::now();
            question.opened = question.opened.max(now);
            question.since = question.since.map(|since| since.max(now));
        }
    }

    /// After each event: the question closed otherwise is dropped, with its report.
    pub(in crate::app::sessions::hosting::native) fn sync_remote_question(&mut self) {
        let Some(question) = self.hosting.natives.remote_question.as_ref() else {
            return;
        };
        let open = (self.editor.prompts().iter()).any(|prompt| prompt.id() == question.prompt);
        if open {
            return;
        }
        if let Some(question) = self.hosting.natives.remote_question.take() {
            self.leave_report(question.key, question.report, question.composer);
            self.request_redraw();
        }
    }

    /// Each window that shows `report` shows native session `key`'s transcript again, and the
    /// report goes; its composer is selected, if it was and the report's window is.
    fn leave_report(&mut self, key: SessionKey, report: BufferId, composer: bool) {
        let transcript = (self.hosting.natives.map.get(&key)).map(|native| native.buffer);
        let selected = self.editor.tiled().0.buffer() == report;
        if let Some(transcript) = transcript {
            let showing: Vec<_> = (self.editor.windows())
                .filter(|window| window.buffer() == report)
                .map(oso_core::window::Window::id)
                .collect();
            for id in showing {
                self.editor.show_in(id, transcript);
            }
        }
        if self.editor.buffer(report).is_some() {
            self.editor.kill_buffer(report);
        }
        if composer && selected && self.editor.prompt().is_none() {
            self.composer_go(key, true);
        }
    }

    /// Whether the question, if it is the open prompt, takes its `y` now. For end-to-end flows.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn remote_question_takes_answer(&self) -> Option<bool> {
        let question = self.hosting.natives.remote_question.as_ref()?;
        let open = self.editor.prompt().map(|prompt| prompt.id());
        (open == Some(question.prompt)).then(|| {
            let typed = self.person_typed_at();
            let now = Instant::now();
            let next = Typed {
                first: Some(now),
                before_first: typed.last,
                ..typed
            };
            (question.since).is_some_and(|since| counts(self.waits_after_agents(since), next, now))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{QUIET, READING, Report, Sent, Typed, counts};

    fn report(sent: Option<&Sent>) -> String {
        Report {
            name: "wa",
            directory: "/w/wa",
            sent,
        }
        .text()
    }

    #[test]
    fn the_report_says_what_is_sent_where_it_stays_and_who_drives_it() {
        let sent = Sent {
            addresses: vec!["https://h/r".into(), "git@h:f".into()],
            more: 0,
            branch: Some("main".into()),
        };
        let text = report(Some(&sent));
        for said in [
            "Anthropic's servers and stays there",
            "all that was said in this session, so far and from now on",
            "all its tools gave and give",
            "its name: wa\n",
            "its git remotes' addresses: https://h/r, git@h:f\n",
            "its branch: main\n",
            "its directory: /w/wa\n",
            "Whoever holds your account can",
            "send it messages, stop it and",
            "change its permission mode, to auto too",
            "Permission questions are answered here alone",
            "Turned off again, it keeps what it has",
        ] {
            assert!(text.contains(said), "{said}:\n{text}");
        }
        assert!(text.lines().all(|line| line.chars().count() <= 96));
        let none = report(Some(&Sent::default()));
        assert!(none.contains("addresses: none\n") && none.contains("branch: none\n"));
        let more = Sent { more: 3, ..sent };
        assert!(report(Some(&more)).contains("git@h:f and 3 more\n"));
        assert_eq!(report(None).matches(READING).count(), 2);
    }

    #[test]
    fn a_yes_counts_only_after_a_quiet_moment() {
        let at = Instant::now();
        let pressed = |first: Duration, before: Option<Duration>| Typed {
            last: Some(at + first),
            before: None,
            first: Some(at + first),
            before_first: before.map(|before| at + before),
        };
        let later = at + QUIET * 10;
        let ms = Duration::from_millis;
        assert!(counts(at, pressed(QUIET, None), later));
        assert!(!counts(at, pressed(QUIET - ms(1), None), later));
        // As one key of a sentence typed on.
        let typed_on = pressed(QUIET * 3, Some(QUIET * 3 - ms(50)));
        assert!(!counts(at, typed_on, later));
        assert!(counts(at, pressed(QUIET * 3, Some(ms(5))), later));
    }
}
