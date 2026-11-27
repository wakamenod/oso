//! A space's questions with `space-confirm` as their `y` (ADR 0136, step 5; ADR 0163), and a
//! repository given to its running orchestrator without asking (ADR 0185, amending ADR 0161
//! and ADR 0176, part O-2) when one it was not given becomes one it may have: a terminal's gets
//! oso's own `/add-dir PATH`, typed and sent once it is idle (`add_dir`); a native one, which
//! runs no `/add-dir`, is resumed with the repository given with `--add-dir` once its turn ends
//! (`hosting::native::adding`).
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! The command is oso's own, made from the space's recorded path only while that is one plain
//! line (`sessions::send::Command`), and so not escaped as composed text is (ADR 0165, part
//! 2f). Every question with `space-confirm` as its `y`, a replacement's and a review's offer to
//! resume the orchestrator (ADR 0163), is asked through [`App::ask_confirm`], which drops the
//! others first, and `y` runs the one still kept.

use std::path::{Path, PathBuf};

use oso_core::minibuffer::PromptId;

use crate::app::App;
use crate::sessions::send::Command;
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::stored::same;
use crate::sessions::worker::SessionKey;

/// How a repository was given to a space's running orchestrator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Gave {
    /// None runs, it has it, or its path is no plain line: nothing.
    Nothing,
    /// A terminal's gets `/add-dir PATH` typed and sent once it is idle.
    Typed,
    /// A native one is resumed with it once its turn ends.
    Resumed,
}

impl App {
    /// Asks `question` with `space-confirm` as its `y`, after dropping every offer kept for
    /// such a question, a replacement's and a review's, so that a `y` does only what it answers.
    pub(in crate::app) fn ask_confirm(
        &mut self,
        question: String,
    ) -> Result<PromptId, oso_core::minibuffer::PromptError> {
        self.hosting.spaces.replace_asked = None;
        self.forget_review_offer();
        self.editor.ask(question, "space-confirm")
    }

    /// Whether space `space`'s orchestrator was given `root`, at its start or typed since.
    pub(in crate::app) fn was_given(&self, space: &SpaceId, root: &Path) -> bool {
        (self.hosting.spaces.given.get(space))
            .is_some_and(|given| given.iter().any(|was| same(was, root)))
    }

    /// Gives space `at`'s running orchestrator `root`, a trusted repository an agent just
    /// added (ADR 0161), whose reply and echo say how, and notes it as seen, so that the look
    /// after this event does not give it again.
    pub(in crate::app) fn give_added(&mut self, at: usize, root: &Path) -> Gave {
        let gave = self.give_dir(at, root, true);
        let space = self
            .watch
            .spaces()
            .stored(at)
            .map(|stored| stored.id.clone());
        if let (Some(seen), Some(space)) = (&mut self.hosting.spaces.trusted_seen, space) {
            seen.push((space, root.to_owned()));
        }
        gave
    }

    /// Gives space `at`'s running orchestrator `root`, which it was not given: a trusted
    /// repository added, or trusted, since it started (Q5), asking no one (ADR 0185). A
    /// terminal's gets `/add-dir root` queued, `told` when the caller's echo says so; a native
    /// one is resumed with it once its turn ends (ADR 0176, part O-2). A stopped one, which
    /// gets it with `--add-dir` at its next start, is said so.
    fn give_dir(&mut self, at: usize, root: &Path, told: bool) -> Gave {
        let Some(stored) = self.watch.spaces().stored(at) else {
            return Gave::Nothing;
        };
        let space = stored.id.clone();
        if self.was_given(&space, root) {
            return Gave::Nothing;
        }
        let Some(key) = self.live_orchestrator(at) else {
            let stopped = (self.orchestrator_of(at))
                .and_then(|key| self.hosting.table().get(key))
                .map(|session| session.name.clone());
            if let Some(name) = stopped {
                self.editor.message(format!(
                    "{name} is not running: it gets {} with --add-dir when it starts",
                    root.display()
                ));
            }
            return Gave::Nothing;
        };
        // oso's own command, not escaped, so that it runs as one: made from the recorded path
        // alone, and only while that is one plain line, which a native one's rows show too.
        let Some(command) = Command::add_dir(root) else {
            return Gave::Nothing;
        };
        // A native one takes no `/add-dir` (`claude -p` has none): it is resumed with the
        // repository given with `--add-dir` instead, once its turn ends (ADR 0176, Q-O4 (a)).
        if self.hosting.natives.has(key) {
            self.resume_with_dir(key, space, root.to_owned());
            return Gave::Resumed;
        }
        self.queue_add_dir((key, space), root.to_owned(), command, told);
        Gave::Typed
    }

    /// `y` to the space question asked last: a review's offer to resume the orchestrator
    /// (ADR 0163), or a replacement's, while kept; with nothing kept, it does nothing.
    pub(in crate::app) fn space_confirmed(&mut self) {
        if self.review_ask_confirmed() {
            return;
        }
        if self.hosting.spaces.replace_asked.is_some() {
            self.replace_confirmed();
        }
    }

    /// Whether repository `root` is still one of `space`'s that oso trusts, and session `key`
    /// still the space's running orchestrator, as when it was given it.
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
    }

    /// After an event: the repositories of open spaces that oso trusts, compared with those
    /// seen last; each trusted or added since is given to its space's running orchestrator
    /// (Q5, ADR 0185). Nothing is given when the trust list was not read before, as when oso
    /// starts: every root would look new. Then the `/add-dir`s are looked at, so that one given
    /// in this event goes in it.
    pub(in crate::app) fn notice_trust(&mut self) {
        self.give_trusted_since();
        self.sync_add_dirs();
    }

    /// [`App::notice_trust`]'s look at the repositories trusted or added since.
    fn give_trusted_since(&mut self) {
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
            if let Some(at) = self.watch.spaces().find(&space) {
                self.give_dir(at, &root, false);
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
