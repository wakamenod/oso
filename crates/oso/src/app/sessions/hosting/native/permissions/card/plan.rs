//! A plan's card in the app (ADR 0165, part 3d, the author's (a) on Q-D1 and Q-D2): `y`
//! approves the plan as Claude wrote it, `a` with accept edits after it and `A` with auto, as
//! the TUI offers; `e` edits it in `*plan: NAME*`, in the session's window in place of its
//! transcript, where `C-c C-c` approves the edit and `C-c C-k` gives it up, back to the card,
//! which waits all the while.
//!
//! Must not depend on: key presses; the card's and the edit's commands ask, and this carries
//! out.
//!
//! - `*plan: NAME*` is the person's draft, as a composer is: editable, withheld from agents,
//!   which may not change it, in `Mode::Plan`, which it keeps, highlighted as Markdown. It
//!   starts as the request's plan, Claude's text, which goes back only as the person's edit:
//!   the CLI is given an edit only if the text differs, never cut, at most
//!   [`permission::MAX_PLAN`] bytes, and never once a yank made it secret.
//! - Only a key in it sends the edit: `C-c C-c`, manual after it, `C-c C-a` with accept edits
//!   and `C-c C-A` with auto, and only for the call it was made for, never from a macro, and
//!   only after [`QUIET`] since the edit became the selected buffer, as a card's keys (`focus`;
//!   not since the key before, as the person writes the edit and then approves it).
//! - While an edit that differs waits, the card's approvals are refused, as they would approve
//!   the plan the person changed.
//! - The card closing otherwise (denied, its call gone) takes an edit the person never
//!   changed with it; a changed one stays, stale, for the person to copy, and says so once. A
//!   later plan's `e` is refused until `C-c C-k` gave the stale one up: it is never lost
//!   unasked.
//! - Auto, and an edit that differs, ask y or n first (`ask`).

mod ask;

use std::time::Instant;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::{Approval, NativeAction};
use oso_core::window::DockSlot;

use super::{Answer, Card, Mark};
use crate::app::App;
use crate::app::agent::QUIET;
use crate::app::sessions::withheld_draft;
use crate::channel::protocol::mode::PermissionMode;
use crate::channel::protocol::permission;
use crate::sessions::worker::SessionKey;

/// What the echo area says while a plan is edited.
const EDITING: &str = "Edit the plan: C-c C-c approves it, C-c C-a with accept edits, C-c C-A with auto; C-c C-k gives the edit up";

/// The plans being edited, one a session at most, and the question asked last before an
/// approval.
#[derive(Debug, Default)]
pub(in crate::app::sessions::hosting::native) struct Plans {
    edits: Vec<Edit>,
    asked: Option<ask::Asked>,
}

/// Session `key`'s plan's edit in `buffer`, made for its call `of` from Claude's `plan`; `stale`
/// once that call's card closed with the edit changed.
#[derive(Debug)]
struct Edit {
    key: SessionKey,
    buffer: BufferId,
    of: String,
    plan: String,
    stale: bool,
}

impl Edit {
    /// Whether `editor`'s buffer of it holds other than Claude's plan, compared with no copy.
    fn changed(&self, editor: &oso_core::editor::Editor) -> bool {
        editor.buffer(self.buffer).is_some_and(|buffer| {
            let text = buffer.text();
            text.len() != self.plan.len() || text.to_string() != self.plan
        })
    }
}

impl Plans {
    fn of(&self, key: SessionKey) -> Option<&Edit> {
        self.edits.iter().find(|edit| edit.key == key)
    }

    /// The session whose plan's edit `buffer` is.
    pub(super) fn session(&self, buffer: BufferId) -> Option<SessionKey> {
        (self.edits.iter()).find_map(|edit| (edit.buffer == buffer).then_some(edit.key))
    }

    fn take(&mut self, key: SessionKey) -> Option<Edit> {
        let at = self.edits.iter().position(|edit| edit.key == key)?;
        Some(self.edits.remove(at))
    }
}

/// The session's mode after a plan approved so, when it is not the CLI's own `default`.
pub(super) fn mode_after(approval: Approval) -> Option<PermissionMode> {
    match approval {
        Approval::Manual => None,
        Approval::AcceptEdits => Some(PermissionMode::AcceptEdits),
        Approval::Auto => Some(PermissionMode::Auto),
    }
}

/// Whether an answer at `now` counts for what was put before the person `at`, a question or
/// a card: [`QUIET`] passed since then and since `before`, the person's key or text before
/// the answer.
pub(super) fn answered_in_time(at: Instant, before: Option<Instant>, now: Instant) -> bool {
    now >= at + QUIET && before.is_none_or(|before| now >= before + QUIET)
}

/// The reply that approves a plan with `plan`, the person's edit if any, and `approval`, and
/// what its row says: a denial, said so, for an edit too long to send, which is never cut.
pub(super) fn answered(
    plan: Option<&str>,
    approval: Approval,
) -> (serde_json::Value, Mark, &'static str) {
    match permission::allow_plan(plan, mode_after(approval)) {
        Some(reply) => (reply, Mark::Approved(plan.is_some(), approval), "approved"),
        None => (
            permission::deny("Denied: the person's edit of the plan was too long to send."),
            Mark::Denied("the edit was too long to send".to_owned()),
            "denied",
        ),
    }
}

impl App {
    /// `y`, `a` or `A` on session `key`'s plan card: the plan approved as Claude wrote it,
    /// unless an edit of it that differs waits; with auto, once `y` answered the question.
    pub(super) fn approve_plan_card(&mut self, key: SessionKey, approval: Approval) {
        self.approve_card_plan(key, approval, false);
    }

    fn approve_card_plan(&mut self, key: SessionKey, approval: Approval, confirmed: bool) {
        if self.plan_edited(key) {
            let name = self.plan_name(key);
            return self.editor.message(format!(
                "Your edit of the plan waits in *plan: {name}*: \
                 C-c C-c there approves it, C-c C-k gives it up"
            ));
        }
        if !confirmed && ask::asks(false, approval) {
            return self.ask_approval(key, approval, None);
        }
        self.answer_card(key, Answer::Plan(None, approval));
    }

    /// `e` on session `key`'s plan card: its edit, made the first time for this call, shown in
    /// the window that carries the card, in place of the transcript.
    pub(super) fn edit_plan(&mut self, key: SessionKey) {
        let Some((of, plan, cut)) = self.waiting_plan(key) else {
            return self.editor.message("This card holds no plan");
        };
        if cut {
            return self
                .editor
                .message("The plan is longer than oso keeps: approve or deny it as it is");
        }
        let plans = &self.hosting.natives.permissions.plans;
        let alive = |edit: &&Edit| {
            (self.editor.buffer(edit.buffer)).is_some_and(|b| b.mode() == Mode::Plan)
        };
        let earlier = plans.of(key).filter(alive);
        if earlier.is_some_and(|edit| edit.of != of && edit.changed(&self.editor)) {
            let name = self.plan_name(key);
            return self.editor.message(format!(
                "Your earlier edit waits in *plan: {name}*: C-c C-k there gives it up first"
            ));
        }
        let kept = earlier.filter(|edit| edit.of == of).map(|edit| edit.buffer);
        let buffer = match kept {
            Some(id) => id,
            None => {
                // An edit of another call that the person never changed gives way.
                if let Some(edit) = self.hosting.natives.permissions.plans.take(key) {
                    self.editor.kill_buffer(edit.buffer);
                }
                let name = format!("*plan: {}*", self.plan_name(key));
                let id = withheld_draft(&mut self.editor, &name, Mode::Plan, &plan);
                (self.hosting.natives.permissions.plans.edits).push(Edit {
                    key,
                    buffer: id,
                    of,
                    plan,
                    stale: false,
                });
                id
            }
        };
        // The card's window is docked in the one that shows the transcript.
        self.composer_go(key, false);
        let transcript = (self.hosting.natives.map.get(&key)).map(|native| native.buffer);
        let window = self.editor.window();
        if window.docked().is_some() || Some(window.buffer()) != transcript {
            return self.editor.message("The session's window is not shown");
        }
        self.editor.show(buffer);
        self.note_current_card();
        self.editor.message(EDITING);
        self.request_redraw();
    }

    /// Whether `buffer` is a plan's edit, and whose.
    pub(in crate::app::sessions::hosting::native) fn plan_session(
        &self,
        buffer: BufferId,
    ) -> Option<SessionKey> {
        self.hosting.natives.permissions.plans.session(buffer)
    }

    /// The session whose plan's edit `buffer` is, if its call still waits on its card.
    pub(super) fn waiting_edit(&self, buffer: BufferId) -> Option<SessionKey> {
        let plans = &self.hosting.natives.permissions.plans.edits;
        let edit = plans.iter().find(|edit| edit.buffer == buffer)?;
        self.edit_waits(edit).then_some(edit.key)
    }

    /// Whether the call `edit` was made for still waits on its card.
    fn edit_waits(&self, edit: &Edit) -> bool {
        let shown = self.hosting.natives.permissions.asks.shown(edit.key);
        !edit.stale && shown.is_some_and(|ask| ask.call.tool_use_id == edit.of)
    }

    /// A key of session `key`'s plan's edit: `action`.
    pub(in crate::app::sessions::hosting::native) fn plan_key(
        &mut self,
        key: SessionKey,
        action: NativeAction,
    ) {
        match action {
            NativeAction::Approve(approval) => {
                // A macro, and an edit nothing waits for, are told so first, by `approve_edit`.
                let edit = self.hosting.natives.permissions.plans.of(key);
                let waiting = edit.filter(|edit| self.edit_waits(edit));
                let place = waiting.map(|edit| edit.buffer);
                if place.is_none_or(|place| self.edit_admits_approval(key, place)) {
                    self.approve_edit(key, approval, false);
                }
            }
            NativeAction::Cancel => self.give_up_edit(key),
            _ => self.editor.message(EDITING),
        }
    }

    /// `C-c C-c`, `C-c C-a` or `C-c C-A` in session `key`'s plan's edit: the plan approved as
    /// the person edited it, if its call still waits; with auto, or an edit that differs, once
    /// `y` answered the question.
    fn approve_edit(&mut self, key: SessionKey, approval: Approval, confirmed: bool) {
        if self.editor.macro_replaying() {
            return self.editor.message("A macro does not approve a plan");
        }
        let Some(edit) = self.hosting.natives.permissions.plans.of(key) else {
            return;
        };
        if !self.edit_waits(edit) {
            return self
                .editor
                .message("No plan waits for this edit any more: C-c C-k leaves it");
        }
        let Some(buffer) = self.editor.buffer(edit.buffer) else {
            return;
        };
        if buffer.mode() != Mode::Plan {
            return self
                .editor
                .message("This buffer is no longer the plan's edit");
        }
        // A yank of a secret entry turns it secret: it never goes to the session.
        if buffer.is_secret() {
            return self
                .editor
                .message("The plan holds secret text, which is not sent: take it out");
        }
        // Measured before it is copied, and never cut.
        if buffer.text().len() > permission::MAX_PLAN {
            return self.editor.message("The plan is over 64 KiB");
        }
        let text = buffer.text().to_string();
        if text.trim().is_empty() {
            return self
                .editor
                .message("The plan is empty: write it, or C-c C-k gives the edit up");
        }
        let edited = (text != edit.plan).then_some(text);
        if !confirmed && ask::asks(edited.is_some(), approval) {
            let asked = ask::EditToApprove {
                lines: edited.map(|text| ask::changed_lines(&edit.plan, &text)),
                revision: buffer.revision(),
            };
            return self.ask_approval(key, approval, Some(asked));
        }
        let Some(edit) = self.hosting.natives.permissions.plans.take(key) else {
            return;
        };
        self.answer_card(key, Answer::Plan(edited, approval));
        self.leave_edit(key, edit.buffer);
        self.composer_go(key, true);
    }

    /// `C-c C-k` in session `key`'s plan's edit: the edit goes, and point to the card, if its
    /// call still waits.
    fn give_up_edit(&mut self, key: SessionKey) {
        let Some(edit) = self.hosting.natives.permissions.plans.take(key) else {
            return;
        };
        self.leave_edit(key, edit.buffer);
        let card = (self.hosting.natives.map.get(&key))
            .and_then(|native| self.editor.dock_window(native.buffer, DockSlot::Card));
        match card {
            Some(window) if self.plan_waits(key) => {
                self.editor.select_window(window);
                self.note_current_card();
            }
            _ => self.composer_go(key, true),
        }
        self.request_redraw();
    }

    /// The selected window shows session `key`'s transcript again in place of its plan's edit
    /// `buffer`, which goes.
    fn leave_edit(&mut self, key: SessionKey, buffer: BufferId) {
        let transcript = (self.hosting.natives.map.get(&key)).map(|native| native.buffer);
        if let Some(transcript) = transcript.filter(|_| self.editor.window().buffer() == buffer) {
            self.editor.show(transcript);
        }
        self.editor.kill_buffer(buffer);
    }

    /// Session `key`'s card closed: an edit of its plan the person never changed goes with it;
    /// a changed one stays, stale, and the echo area says where, once.
    pub(super) fn plan_card_closed(&mut self, key: SessionKey) {
        let Some(edit) = self.hosting.natives.permissions.plans.of(key) else {
            return;
        };
        if edit.stale {
            return;
        }
        if edit.changed(&self.editor) {
            let name = self.plan_name(key);
            let plans = &mut self.hosting.natives.permissions.plans.edits;
            if let Some(edit) = plans.iter_mut().find(|edit| edit.key == key) {
                edit.stale = true;
            }
            return self.editor.message(format!(
                "Session {name}'s plan no longer waits: your edit stays in *plan: {name}*"
            ));
        }
        let Some(edit) = self.hosting.natives.permissions.plans.take(key) else {
            return;
        };
        log::debug!(
            "native sessions: the edit of plan {} goes with its card",
            edit.of
        );
        self.leave_edit(key, edit.buffer);
    }

    /// Whether session `key`'s plan has an edit, for the call its card asks about, that
    /// differs from Claude's.
    fn plan_edited(&self, key: SessionKey) -> bool {
        let Some(edit) = self.hosting.natives.permissions.plans.of(key) else {
            return false;
        };
        self.edit_waits(edit) && edit.changed(&self.editor)
    }

    /// Session `key`'s shown call's id, plan and whether the request cut it, if it asks to
    /// approve a plan.
    fn waiting_plan(&self, key: SessionKey) -> Option<(String, String, bool)> {
        let ask = self.hosting.natives.permissions.asks.shown(key)?;
        match &ask.question.as_ref()?.card {
            Card::Plan { plan, cut } => Some((ask.call.tool_use_id.clone(), plan.clone(), *cut)),
            Card::Call { .. } | Card::Questions(_) => None,
        }
    }

    /// Whether session `key`'s shown call asks to approve a plan.
    fn plan_waits(&self, key: SessionKey) -> bool {
        (self.hosting.natives.permissions.asks.shown(key))
            .and_then(|ask| ask.question.as_ref())
            .is_some_and(|question| matches!(question.card, Card::Plan { .. }))
    }

    fn plan_name(&self, key: SessionKey) -> String {
        (self.hosting.table.get(key)).map_or_else(String::new, |s| s.name.clone())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::time::{Duration, Instant};

    use oso_core::editor::Approval;
    use serde_json::json;

    use super::super::Mark;
    use oso_core::buffer::BufferId;

    use super::{Edit, Plans, QUIET, SessionKey, answered, answered_in_time};
    use crate::channel::protocol::permission::MAX_PLAN;

    /// Session `key`'s plan edited in `buffer`.
    pub(in super::super) fn editing(key: SessionKey, buffer: BufferId) -> Plans {
        Plans {
            edits: vec![Edit {
                key,
                buffer,
                of: "p1".to_owned(),
                plan: "Do it.".to_owned(),
                stale: false,
            }],
            asked: None,
        }
    }

    #[test]
    fn an_approval_says_its_edit_and_mode_and_a_long_edit_is_denied_never_cut() {
        let (reply, mark, said) = answered(None, Approval::Manual);
        assert_eq!(reply, json!({"behavior": "allow"}));
        assert!(matches!(mark, Mark::Approved(false, Approval::Manual)));
        assert_eq!(said, "approved");
        let (reply, mark, _) = answered(Some("Do it."), Approval::Auto);
        assert_eq!(
            reply,
            json!({"behavior": "allow", "plan": "Do it.", "mode": "auto"})
        );
        assert!(matches!(mark, Mark::Approved(true, Approval::Auto)));
        let (reply, _, _) = answered(None, Approval::AcceptEdits);
        assert_eq!(reply["mode"], "acceptEdits");
        let long = "x".repeat(MAX_PLAN + 1);
        let (reply, mark, said) = answered(Some(&long), Approval::Auto);
        assert_eq!(reply["behavior"], "deny");
        assert!(reply.get("mode").is_none());
        assert!(matches!(mark, Mark::Denied(_)));
        assert_eq!(said, "denied");
    }

    #[test]
    fn autos_y_counts_only_after_a_quiet_moment_since_the_question_and_the_key_before() {
        let at = Instant::now();
        let late = at + QUIET;
        assert!(answered_in_time(at, None, late));
        assert!(answered_in_time(at, Some(at), late));
        // Too soon after the question opened: `A` then `y` typed on.
        assert!(!answered_in_time(at, None, at + Duration::from_millis(80)));
        // Soon after another key typed into the question: "Are y…".
        let typed = at + QUIET + Duration::from_millis(200);
        assert!(!answered_in_time(
            at,
            Some(typed),
            typed + Duration::from_millis(90)
        ));
        assert!(answered_in_time(at, Some(typed), typed + QUIET));
    }
}
