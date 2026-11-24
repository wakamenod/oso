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
//!   and in a window, a quiet moment after the report was whole, the question opened or oso's window came
//!   in front, and after the key before it; never from a macro, nor as the repeat of a held key.
//!   Everything is judged again then.
//! - It closes as oso's window loses the front or goes out of sight; closed any way, its report
//!   goes, and the transcript is back in its window.
//! - Nothing in the report but oso's own words is drawn unescaped.

use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::minibuffer::PromptId;

use super::super::permissions::request::gate::{Pressed, waited};
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

/// The rows of the frame's top the question's float may take.
const FLOAT_ROWS: usize = 6;

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
    /// What its report says, and since when it may be answered, once the report is whole.
    sent: Option<(Sent, Instant)>,
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
                match sent.addresses.is_empty() {
                    true => "none".to_owned(),
                    false => sent.addresses.join(", "),
                },
                sent.branch.clone().unwrap_or_else(|| "none".to_owned()),
            ),
        };
        // Below the question's float, at the frame's top.
        let below = "\n".repeat(FLOAT_ROWS);
        format!(
            "{below}Remote Control of {name}\n\
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
             Whoever holds your account can follow it at claude.ai, send it messages, stop it and \
             change its permission mode, to auto too, where calls run without asking you. They may \
             read its files. Permission questions are answered here alone.\n\
             \n\
             claude ends and resumes its conversation to connect. claude.ai may go on with a session \
             it lists for this conversation already. Turned off again, it keeps what it has.\n",
            name = self.name,
            directory = self.directory,
        )
    }
}

/// The most rows `text` takes where a window has `columns`, its lines wrapped.
fn rows(text: &str, columns: usize) -> usize {
    let cells = |line: &str| {
        line.chars()
            .map(|c| if c.is_ascii() { 1 } else { 2 })
            .sum::<usize>()
    };
    // A wrap may leave a part of a row empty, less than half of it.
    (text.lines())
        .map(|line| 1 + cells(line) * 2 / (columns.max(1) * 2 - 1))
        .sum()
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
        self.editor.set_wraps(report, true);
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
        natives.remotes.token += 1;
        let token = natives.remotes.token;
        natives.remotes.question = Some(Question {
            token,
            prompt,
            key,
            of,
            report,
            composer,
            sent: None,
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
        let Some(question) = (self.hosting.natives.remotes.question.as_ref())
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
                if let Some(question) = &mut self.hosting.natives.remotes.question {
                    question.sent = Some((sent, Instant::now().max(question.opened)));
                }
            }
        }
        self.request_redraw();
    }

    /// `y` to the question: the session it asked about is connected, if it still may be.
    pub(in crate::app::sessions::hosting::native) fn remote_confirmed(&mut self) {
        let Some(question) = self.hosting.natives.remotes.question.take() else {
            return self.editor.message("Nothing was asked");
        };
        let shown = self.report_shows_all(question.report);
        let Question {
            key,
            report,
            of,
            sent,
            composer,
            ..
        } = question;
        self.leave_report(key, report, composer);
        self.request_redraw();
        let still = (self.hosting.table.get(key)).is_some_and(|session| session.id == of);
        let since = (sent.as_ref()).map(|(_, since)| self.waits_after_agents(*since));
        let pressed = Pressed::last(self.person_typed_at(), Instant::now());
        let refused = match since {
            _ if self.editor.macro_replaying() => Some("a macro does not connect a session"),
            _ if !still => Some("the session asked about is gone"),
            None => Some("what is sent was not read yet; ask again"),
            Some(_) if !shown => {
                Some("what is sent was not shown whole; make the window larger and ask again")
            }
            Some(since) if !waited(since, pressed) => {
                Some("wait a moment after the question, then answer it; ask again")
            }
            Some(_) => None,
        };
        match (refused, sent) {
            (Some(why), _) => self.editor.message(format!("Not connected: {why}")),
            (None, Some((sent, _))) => self.remote_on(key, sent),
            (None, None) => {}
        }
    }

    /// Whether a window shows all of `report`, from its start.
    fn report_shows_all(&self, report: BufferId) -> bool {
        let Some(buffer) = self.editor.buffer(report) else {
            return false;
        };
        (self.editor.windows().filter(|w| w.buffer() == report)).any(|window| {
            let scroll = window.scroll();
            (self.editor.regions(window.id())).is_some_and(|regions| {
                let text = buffer.text().to_string();
                (scroll.start, scroll.left) == (0, 0)
                    && rows(&text, regions.text.columns) <= regions.text.rows
            })
        })
    }

    /// The question before Remote Control is turned on, if one is open, closes.
    pub(in crate::app) fn close_remote_question(&mut self) {
        if let Some(question) = self.hosting.natives.remotes.question.take() {
            self.close_question(question.prompt);
            self.leave_report(question.key, question.report, question.composer);
        }
    }

    /// The person came back to oso's window: the question is new to them.
    pub(in crate::app::sessions::hosting::native) fn remote_question_waits_again(&mut self) {
        if let Some(question) = &mut self.hosting.natives.remotes.question {
            let now = Instant::now();
            question.opened = question.opened.max(now);
            if let Some((_, since)) = &mut question.sent {
                *since = (*since).max(now);
            }
        }
    }

    /// After each event: the question closed otherwise is dropped, with its report.
    pub(in crate::app::sessions::hosting::native) fn sync_remote_question(&mut self) {
        let Some(question) = self.hosting.natives.remotes.question.as_ref() else {
            return;
        };
        let open = (self.editor.prompts().iter()).any(|prompt| prompt.id() == question.prompt);
        if open {
            return;
        }
        if let Some(question) = self.hosting.natives.remotes.question.take() {
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
        let question = self.hosting.natives.remotes.question.as_ref()?;
        let open = self.editor.prompt().map(|prompt| prompt.id());
        let pressed = Pressed::next(self.person_typed_at(), Instant::now());
        (open == Some(question.prompt)).then(|| {
            self.report_shows_all(question.report)
                && (question.sent.as_ref())
                    .is_some_and(|(_, since)| waited(self.waits_after_agents(*since), pressed))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{FLOAT_ROWS, READING, Report, Sent};

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
            "send it messages, stop it and \
             change its permission mode, to auto too",
            "Permission questions are answered here alone",
            "Turned off again, it keeps what it has",
        ] {
            assert!(text.contains(said), "{said}:\n{text}");
        }
        assert!(text.starts_with(&"\n".repeat(FLOAT_ROWS)));
        let none = report(Some(&Sent::default()));
        assert!(none.contains("addresses: none\n") && none.contains("branch: none\n"));
        assert_eq!(report(None).matches(READING).count(), 2);
    }
}
