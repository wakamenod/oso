//! The y-or-n question before a plan's approval (ADR 0165, part 3d): before one with auto, `A`
//! on the card or `C-c C-A` in the edit, as `A` is the first letter of many a sentence typed
//! where point came to the card, and before one of an edit that differs from Claude's plan, with
//! whatever mode after it, which says how many lines differ.
//!
//! Must not depend on: key presses; `plan` asks it before it approves, and it has `plan` approve
//! on its `y`.
//!
//! - The question is kept for the call it asked about, with the approval it asked of, and its
//!   `y` approves only that call so, if it still waits and the edit, if any, is as it was asked
//!   about and still passes; the question closed otherwise is dropped.
//! - The `y` counts only after [`QUIET`](super::QUIET) since the question opened, or oso's
//!   window came in front again, and since the key before it, as the card's own keys do: a question takes every key but `y` and `n`
//!   without closing, so a sentence typed on ("Are you…") would otherwise answer it.

use std::convert::Infallible;
use std::time::{Duration, Instant};

use oso_core::editor::Approval;
use oso_core::minibuffer::PromptId;
use similar::Algorithm;
use similar::algorithms::{DiffHook, diff_slices_deadline};

use super::{Plans, answered_in_time};
use crate::app::App;
use crate::commands::agent_sessions::PLAN_CONFIRMED;
use crate::sessions::worker::SessionKey;

/// How long an edit's lines are compared with the plan's, at most 64 KiB of each, on the UI
/// thread, for one key of the person's, whose frame is to be drawn within 8 ms: past it, the
/// lines not compared yet count as changed.
const COMPARE_WITHIN: Duration = Duration::from_millis(2);

/// The question `prompt`, asked `at`, before session `key`'s plan, its call `of`, is approved
/// with `approval` after it, from its edit at `revision` if there is one.
#[derive(Debug)]
pub(super) struct Asked {
    prompt: PromptId,
    at: Instant,
    key: SessionKey,
    of: String,
    approval: Approval,
    revision: Option<u64>,
}

/// The edit a question is about: the lines it adds and takes out, if it differs from Claude's
/// plan, and its buffer's revision.
#[derive(Debug, Clone, Copy)]
pub(super) struct EditToApprove {
    pub(super) lines: Option<(usize, usize)>,
    pub(super) revision: u64,
}

impl Plans {
    /// What the question asks about is new to the person, as they come back to oso's window: its
    /// wait starts again at `now`.
    pub(in super::super) fn wait_again(&mut self, now: Instant) {
        if let Some(asked) = &mut self.asked {
            asked.at = asked.at.max(now);
        }
    }
}

/// Whether an approval with `approval` after it asks first, of an edit that differs if `edited`.
pub(super) fn asks(edited: bool, approval: Approval) -> bool {
    edited || approval == Approval::Auto
}

impl Asked {
    /// Why its `y` approves nothing, if it does not: `waiting` is the call whose plan waits now,
    /// and `edit` the call and the revision of the edit in front, if one is.
    fn refuses(&self, waiting: Option<&str>, edit: Option<(&str, u64)>) -> Option<&'static str> {
        if waiting != Some(self.of.as_str()) {
            return Some("Not approved: the plan asked about no longer waits");
        }
        let revision = self.revision?;
        match edit {
            Some((of, now)) if of == self.of => {
                (now != revision).then_some("Not approved: the edit changed after the question")
            }
            _ => Some("Not approved: the plan's edit is no longer in front"),
        }
    }
}

/// How many lines `edit` adds to `plan` and takes out of it, as a diff has them, line endings
/// among them: never none of both for two texts that differ.
pub(super) fn changed_lines(plan: &str, edit: &str) -> (usize, usize) {
    changed_lines_by(plan, edit, Instant::now() + COMPARE_WITHIN)
}

fn changed_lines_by(plan: &str, edit: &str, deadline: Instant) -> (usize, usize) {
    let lines = |text| -> Vec<&[u8]> {
        str::as_bytes(text)
            .split_inclusive(|&b| b == b'\n')
            .collect()
    };
    let (plan, edit) = (lines(plan), lines(edit));
    let mut changed = Changed::default();
    let Ok(()) = diff_slices_deadline(Algorithm::Myers, &mut changed, &plan, &edit, Some(deadline));
    (changed.added, changed.removed)
}

/// Counts a diff's lines as they come, so that no pass over its steps follows the deadline.
#[derive(Default)]
struct Changed {
    added: usize,
    removed: usize,
}

impl DiffHook for Changed {
    type Error = Infallible;

    fn delete(&mut self, _: usize, len: usize, _: usize) -> Result<(), Infallible> {
        self.removed += len;
        Ok(())
    }

    fn insert(&mut self, _: usize, _: usize, len: usize) -> Result<(), Infallible> {
        self.added += len;
        Ok(())
    }
}

/// The question before session `name`'s plan is approved with `approval` after it, as edited if
/// `lines` says the lines that differ.
fn question(name: &str, lines: Option<(usize, usize)>, approval: Approval) -> String {
    let edited = lines.map_or_else(String::new, |(added, removed)| {
        format!(" as you edited it (+{added} −{removed} lines)")
    });
    let after = match approval {
        Approval::Manual => "",
        Approval::AcceptEdits => " and leave it in accept edits",
        Approval::Auto => {
            " and leave it in auto mode, where calls its classifier judges safe run unasked"
        }
    };
    format!("Approve {name}'s plan{edited}{after}? (y or n)")
}

impl App {
    /// Asks whether to approve session `key`'s plan, from its edit if there is one, with
    /// `approval` after it; the question asked anew replaces the one before.
    pub(super) fn ask_approval(
        &mut self,
        key: SessionKey,
        approval: Approval,
        edit: Option<EditToApprove>,
    ) {
        let Some(of) = (self.hosting.natives.permissions.asks.shown(key))
            .map(|ask| ask.call.tool_use_id.clone())
        else {
            return;
        };
        let lines = edit.and_then(|edit| edit.lines);
        let question = question(&self.plan_name(key), lines, approval);
        self.hosting.natives.permissions.plans.asked = None;
        match self.editor.ask(question, PLAN_CONFIRMED) {
            Ok(prompt) => {
                self.hosting.natives.permissions.plans.asked = Some(Asked {
                    prompt,
                    at: Instant::now(),
                    key,
                    of,
                    approval,
                    revision: edit.map(|edit| edit.revision),
                });
            }
            Err(e) => self.editor.message(format!("Cannot ask: {e}")),
        }
        self.request_redraw();
    }

    /// `y` to the question asked last before an approval: the plan it asked about approved as
    /// it asked, if its call still waits on its card, from the edit it was asked in, which is
    /// checked again.
    pub(in crate::app::sessions::hosting::native) fn plan_confirmed(&mut self) {
        let Some(asked) = self.hosting.natives.permissions.plans.asked.take() else {
            return self.editor.message("Nothing was asked about a plan");
        };
        if self.editor.macro_replaying() {
            return self.editor.message("A macro does not approve a plan");
        }
        if !answered_in_time(asked.at, self.agent_key_before(), Instant::now()) {
            return self.editor.message(
                "Not approved: wait a moment after the question, then answer it; the approval's key asks again",
            );
        }
        let key = asked.key;
        let waiting = (self.hosting.natives.permissions.asks.shown(key))
            .filter(|_| self.plan_waits(key))
            .map(|ask| ask.call.tool_use_id.as_str());
        let shown = self.editor.window().buffer();
        let edit = (self.hosting.natives.permissions.plans.of(key))
            .filter(|edit| edit.buffer == shown)
            .and_then(|edit| Some((edit.of.as_str(), self.editor.buffer(shown)?.revision())));
        if let Some(why) = asked.refuses(waiting, edit) {
            return self.editor.message(why);
        }
        match asked.revision {
            Some(_) => self.approve_edit(key, asked.approval, true),
            None => self.approve_card_plan(key, asked.approval, true),
        }
    }

    /// After an event: the question before an approval, once it is no longer open, under
    /// another prompt or not, and was not answered `y` in it, is dropped, so that no later `y`
    /// approves.
    pub(in crate::app) fn drop_unanswered_plan_question(&mut self) {
        let Some(asked) = self.hosting.natives.permissions.plans.asked.as_ref() else {
            return;
        };
        let prompts = self.editor.prompts();
        if !prompts.iter().any(|prompt| prompt.id() == asked.prompt) {
            self.hosting.natives.permissions.plans.asked = None;
        }
    }

    /// Whether the question before an approval, if one is the open prompt, takes its `y` now.
    /// For end-to-end flows.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn plan_question_takes_answer(&self) -> Option<bool> {
        let asked = self.hosting.natives.permissions.plans.asked.as_ref()?;
        let open = self.editor.prompt().map(|prompt| prompt.id());
        (open == Some(asked.prompt))
            .then(|| answered_in_time(asked.at, self.agent_person_typed(), Instant::now()))
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    use oso_core::editor::Approval;

    use super::super::QUIET;
    use super::{Asked, Plans, answered_in_time, asks, changed_lines_by, question};

    #[test]
    fn the_question_waits_again_as_the_person_comes_back() {
        let mut editor = oso_core::editor::Editor::default();
        let at = Instant::now();
        let mut plans = Plans {
            asked: Some(Asked {
                prompt: editor.ask("Approve?", "keyboard-quit").unwrap(),
                at,
                key: 7,
                of: "p1".to_owned(),
                approval: Approval::Manual,
                revision: Some(3),
            }),
            ..Plans::default()
        };
        let back = at + Duration::from_secs(2);
        plans.wait_again(back);
        let asked = plans.asked.as_ref().unwrap();
        assert!(!answered_in_time(
            asked.at,
            None,
            back + Duration::from_millis(80)
        ));
        assert!(answered_in_time(asked.at, None, back + QUIET));
    }

    #[test]
    fn an_edit_that_differs_and_auto_ask_first_and_nothing_else_does() {
        for approval in [Approval::Manual, Approval::AcceptEdits, Approval::Auto] {
            assert!(asks(true, approval));
            assert_eq!(asks(false, approval), approval == Approval::Auto);
        }
    }

    #[test]
    fn a_yes_approves_only_the_call_and_the_edit_asked_about() {
        let mut editor = oso_core::editor::Editor::default();
        let mut asked = Asked {
            prompt: editor.ask("Approve?", "keyboard-quit").unwrap(),
            at: Instant::now(),
            key: 7,
            of: "p1".to_owned(),
            approval: Approval::AcceptEdits,
            revision: Some(3),
        };
        let edit = Some(("p1", 3));
        assert_eq!(asked.refuses(Some("p1"), edit), None);
        let says = |why: Option<&'static str>, what: &str| {
            assert!(why.is_some_and(|why| why.contains(what)), "{why:?}");
        };
        says(asked.refuses(None, edit), "no longer waits");
        says(asked.refuses(Some("p2"), edit), "no longer waits");
        says(asked.refuses(Some("p1"), Some(("p1", 4))), "changed");
        says(asked.refuses(Some("p1"), Some(("p2", 3))), "in front");
        says(asked.refuses(Some("p1"), None), "in front");
        // Asked from the card, of Claude's plan.
        asked.revision = None;
        assert_eq!(asked.refuses(Some("p1"), None), None);
        says(asked.refuses(Some("p2"), None), "no longer waits");
    }

    #[test]
    fn the_question_says_how_many_lines_differ_and_the_mode_after_it() {
        assert_eq!(
            question("proj", Some((2, 1)), Approval::Manual),
            "Approve proj's plan as you edited it (+2 −1 lines)? (y or n)"
        );
        assert_eq!(
            question("proj", Some((2, 1)), Approval::AcceptEdits),
            "Approve proj's plan as you edited it (+2 −1 lines) and leave it in accept edits? \
             (y or n)"
        );
        let unedited = question("proj", None, Approval::Auto);
        assert!(
            unedited.starts_with("Approve proj's plan and leave it in auto mode, where"),
            "{unedited}"
        );
        assert!(unedited.ends_with("run unasked? (y or n)"), "{unedited}");
        let edited = question("proj", Some((0, 3)), Approval::Auto);
        assert!(
            edited.starts_with("Approve proj's plan as you edited it (+0 −3 lines) and leave it"),
            "{edited}"
        );
    }

    #[test]
    fn an_edits_lines_are_counted_as_a_diff_has_them() {
        let counted =
            |plan, edit| changed_lines_by(plan, edit, Instant::now() + Duration::from_secs(5));
        assert_eq!(counted("a\nb\n", "a\nb\n"), (0, 0));
        assert_eq!(counted("a\nb\n", "a\nx\nb\n"), (1, 0));
        assert_eq!(counted("a\nb\nc\n", "a\nc\n"), (0, 1));
        assert_eq!(counted("a\nb\n", "b\na\n"), (1, 1), "moved");
        // What only a line's end or white space differs in counts too.
        assert_eq!(counted("a", "a\n"), (1, 1));
        assert_eq!(counted("a\n", "a \n"), (1, 1));
        assert_eq!(counted("a\n", "a\r\n"), (1, 1));
    }

    #[test]
    fn past_the_deadline_no_changed_line_goes_uncounted() {
        let mut plan = String::new();
        for n in 0..8000 {
            let _ = writeln!(plan, "{n}");
        }
        let edit = plan.replace("7\n", "seven\n") + "more\n";
        let changed = plan.matches("7\n").count();
        let (added, removed) = changed_lines_by(&plan, &edit, Instant::now());
        assert!(added > changed && removed >= changed, "+{added} −{removed}");
    }
}
