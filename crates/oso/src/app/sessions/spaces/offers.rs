//! A space's offers of a repository added to its running orchestrator (ADR 0136, step 3;
//! ADR 0161): pasting `/add-dir` into a terminal's, or resuming a native one with it
//! (ADR 0176, part O-2), when a repository it was not given becomes one it may have.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! An offer is a question the host asks with `space-confirm` as its `y`: what it would paste is
//! kept beside the question's id, dropped when another question is asked, and done only when
//! `y` answers that same question, as the person's paste, never submitted. The paste is oso's own
//! `/add-dir PATH`, made from the recorded path only while that is one plain line, and so not
//! escaped as composed text is (`sessions::send::Command`, ADR 0165, part 2f). A native
//! orchestrator, which runs no `/add-dir`, is offered a resume with the repository added
//! instead, once its turn ends (ADR 0176, part O-2). Every such question,
//! the space's and a review's offer to resume the orchestrator (ADR 0163), is asked through
//! [`App::ask_confirm`], which drops the others first, and `y` runs the one still kept.

use std::path::{Path, PathBuf};

use oso_core::minibuffer::PromptId;

use crate::app::App;
use crate::sessions::send::Command;
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::stored::same;
use crate::sessions::worker::SessionKey;

/// What a question the host asked would do on `y`: paste `/add-dir PATH` into an
/// orchestrator, or resume a native one with the repository added.
#[derive(Debug)]
pub(in crate::app) struct Offer {
    /// The question it was asked with.
    prompt: PromptId,
    session: SessionKey,
    /// The space and the repository it offers.
    space: SpaceId,
    root: PathBuf,
    act: Act,
}

/// What `y` to an offer does.
#[derive(Debug)]
enum Act {
    /// Pastes oso's own command into a terminal's input, not submitted.
    Paste(Command),
    /// Resumes a native orchestrator with the repository added, once it is free to stop.
    Resume,
}

impl App {
    /// Asks `question` with `space-confirm` as its `y`, after dropping every offer kept for
    /// such a question, the space's and a review's, so that a `y` does only what it answers.
    pub(in crate::app) fn ask_confirm(
        &mut self,
        question: String,
    ) -> Result<PromptId, oso_core::minibuffer::PromptError> {
        self.hosting.spaces.offer = None;
        self.hosting.spaces.replace_asked = None;
        self.forget_review_offer();
        self.editor.ask(question, "space-confirm")
    }

    /// Whether space `space`'s orchestrator was given `root`, at its start or pasted since.
    pub(in crate::app) fn was_given(&self, space: &SpaceId, root: &Path) -> bool {
        (self.hosting.spaces.given.get(space))
            .is_some_and(|given| given.iter().any(|was| same(was, root)))
    }

    /// Offers to paste `/add-dir root` into space `at`'s running orchestrator now, or to
    /// resume a native one with it, for a trusted repository an agent just added (ADR 0161), and notes it as seen, so that the
    /// look after this event does not offer it again. Whether the person is asked.
    pub(in crate::app) fn offer_added(&mut self, at: usize, root: &Path) -> bool {
        let asked = self.offer_add_dir(at, root);
        let space = self
            .watch
            .spaces()
            .stored(at)
            .map(|stored| stored.id.clone());
        if let (Some(seen), Some(space)) = (&mut self.hosting.spaces.trusted_seen, space) {
            seen.push((space, root.to_owned()));
        }
        asked && self.hosting.spaces.offer.is_some()
    }

    /// Offers to paste `/add-dir root` into space `at`'s running orchestrator, which was not
    /// given `root`: a trusted repository added, or trusted, since it started (Q5); a native
    /// one is offered a resume with it (ADR 0176, part O-2). Whether it asked.
    fn offer_add_dir(&mut self, at: usize, root: &Path) -> bool {
        let Some(key) = self.live_orchestrator(at) else {
            return false;
        };
        let Some(stored) = self.watch.spaces().stored(at) else {
            return false;
        };
        let space = stored.id.clone();
        if self.was_given(&space, root) {
            return false;
        }
        // oso's own command, not escaped, so that it runs as one: made from the recorded path
        // alone, and only while that is one plain line.
        let Some(command) = Command::add_dir(root) else {
            return false;
        };
        // The orchestrator answers to the space's slug.
        let name = stored.name.clone();
        // A native one takes no `/add-dir` (`claude -p` has none): it is resumed with the
        // repository given with `--add-dir` instead, once its turn ends (ADR 0176, Q-O4 (a)),
        // the path shown only once it is one plain line, as the paste's. Its question, whose
        // `y` stops a process, is never put over another the person may be answering.
        let native = self.hosting.natives.has(key);
        if native && self.editor.prompt().is_some() {
            return false;
        }
        let question = match native {
            true => format!(
                "Resume orchestrator {name} with {} added, once its turn ends? (y or n)",
                root.display()
            ),
            false => format!(
                "Paste {} into orchestrator {name}'s prompt, not sent? (y or n)",
                command.as_str()
            ),
        };
        // A question asked anew drops the one before, and what it offered.
        match self.ask_confirm(question) {
            Ok(prompt) => {
                self.hosting.spaces.offer = Some(Offer {
                    prompt,
                    session: key,
                    space,
                    root: root.to_owned(),
                    act: match native {
                        true => Act::Resume,
                        false => Act::Paste(command),
                    },
                });
            }
            Err(e) => self.editor.message(e.to_string()),
        }
        true
    }

    /// `y` to the space question asked last: its offer, if it still holds: the session is
    /// still the space's running orchestrator, and the repository still one of the space's
    /// that oso trusts; a native one's resume is judged so again until it goes. One whose question closed otherwise was dropped after that event, and
    /// one asked anew replaced it, so a `y` does only the question it answered; with nothing
    /// kept, it does nothing.
    pub(in crate::app) fn space_confirmed(&mut self) {
        // A review's offer to resume the orchestrator for a question (ADR 0163).
        if self.review_ask_confirmed() {
            return;
        }
        if self.hosting.spaces.replace_asked.is_some() {
            return self.replace_confirmed();
        }
        let Some(offer) = self.hosting.spaces.offer.take() else {
            return;
        };
        if !self.add_dir_holds(offer.session, &offer.space, &offer.root) {
            let not = match offer.act {
                Act::Paste(_) => "pasted",
                Act::Resume => "resumed",
            };
            return self.editor.message(format!(
                "Not {not}: the space or its orchestrator changed meanwhile"
            ));
        }
        let command = match offer.act {
            Act::Paste(command) => command,
            Act::Resume => return self.resume_with_dir(offer.session, offer.space, offer.root),
        };
        match self.send_command(offer.session, &command) {
            Ok(()) => (self.hosting.spaces.given.entry(offer.space))
                .or_default()
                .push(offer.root),
            Err(why) => self.editor.message(why),
        }
    }

    /// Whether repository `root` is still one of `space`'s that oso trusts, and session `key`
    /// still the space's running orchestrator, as an offer of it was made.
    pub(in crate::app) fn add_dir_holds(
        &self,
        key: SessionKey,
        space: &SpaceId,
        root: &Path,
    ) -> bool {
        let spaces = self.watch.spaces();
        let Some(at) = spaces.find(space) else {
            return false;
        };
        // As `trusted_members` judges, for this one space.
        let trusted = spaces.stored(at).is_some_and(|stored| {
            !stored.archived
                && (stored.members.iter())
                    .any(|member| same(&member.root, root) && self.trusts(member))
        });
        self.editor.trust_list().is_some() && trusted && self.live_orchestrator(at) == Some(key)
    }

    /// After an event: an offer whose question is no longer open, and was not answered `y`
    /// in it, is dropped, so that no later `y` does it.
    pub(in crate::app) fn forget_unanswered_offer(&mut self) {
        self.forget_unanswered_review_offer();
        let open = self.editor.prompt().map(|prompt| prompt.id());
        let replace = self
            .hosting
            .spaces
            .replace_asked
            .as_ref()
            .map(|asked| asked.prompt);
        if replace.is_some() && open != replace {
            self.hosting.spaces.replace_asked = None;
        }
        let asked = self.hosting.spaces.offer.as_ref().map(|offer| offer.prompt);
        if asked.is_some() && open != asked {
            self.hosting.spaces.offer = None;
        }
    }

    /// After an event: the repositories of open spaces that oso trusts, compared with those
    /// seen last; one trusted or added since is offered to its space's running orchestrator
    /// (Q5). Nothing is offered when the trust list was not read before, as when oso starts:
    /// every root would look new.
    pub(in crate::app) fn notice_trust(&mut self) {
        let now = self.trusted_members();
        let before = std::mem::replace(&mut self.hosting.spaces.trusted_seen, now.clone());
        let (Some(before), Some(now)) = (before, now) else {
            return;
        };
        for (space, root) in now {
            let seen = (before.iter()).any(|(id, was)| *id == space && same(was, &root));
            if seen {
                continue;
            }
            let at = self.watch.spaces().find(&space);
            if at.is_some_and(|at| self.offer_add_dir(at, &root)) {
                return;
            }
        }
    }

    /// Each open space's repositories that oso trusts; none while the trust list is unread.
    fn trusted_members(&self) -> Option<Vec<(SpaceId, PathBuf)>> {
        self.editor.trust_list()?;
        let spaces = self.watch.spaces();
        let open = spaces.iter().filter(|space| !space.stored.archived);
        let members = open.flat_map(|space| {
            (space.stored.members.iter())
                .filter(|member| self.trusts(member))
                .map(|member| (space.stored.id.clone(), member.root.clone()))
        });
        Some(members.collect())
    }
}
