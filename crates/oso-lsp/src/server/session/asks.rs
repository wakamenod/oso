//! The UI's asks about documents on the session thread (ADR 0138, steps 8 and 10): each becomes
//! a request at the position its offset is in the text the server has, a newer one cancelling
//! the older of its kind for the document, and each answer, the places of a definition or the
//! text of a description, or the end of its wait or of the run, becomes an [`Answer`] for the
//! UI. Code actions' are in `actions`.
//!
//! Must not depend on: the process or the pipes; the run sends, and the reporter hands over.

use std::time::Instant;

use oso_core::rope::Rope;
use serde_json::{Value, json};

use crate::position;
use crate::protocol::{self, Outcome};
use crate::uri;

use super::super::{
    Answer, Answered, Ask, AskContext, AskFailure, AskKind, Completion, Completions, Location,
    Resolved, Target,
};
use super::run::{LastItems, Run};
use super::{Asked, Session};

/// Why there is no answer while no server runs.
const NOT_RUNNING: &str = "it does not run";

impl Session {
    /// An ask with no run: answered at once.
    pub(super) fn ask_idle(&mut self, ask: Ask) {
        self.fail(ask, NOT_RUNNING.to_owned());
    }

    /// An ask during a run: a request, or an answer at once saying why not.
    pub(super) fn ask(&mut self, run: &mut Run, ask: Ask) -> Result<(), String> {
        if !run.ready || run.stopping.is_some() {
            // A run asked to stop before it was ready is not starting.
            let why = if run.ready || run.stopping.is_some() {
                NOT_RUNNING
            } else {
                "it is starting"
            };
            self.fail(ask, why.to_owned());
            return Ok(());
        }
        let Some((uri, text)) = self.documents.as_sent(ask.document, ask.revision) else {
            self.fail(ask, "the server does not have this text yet".to_owned());
            return Ok(());
        };
        let params = match self.ask_params(run, &ask, (uri, text)) {
            Ok(params) => params,
            Err(why) => {
                self.fail(ask, why);
                return Ok(());
            }
        };
        // The older of its kind for the document is moot.
        let older = run.pending.iter().find_map(|(id, (asked, _))| match asked {
            Asked::Ask(kept) if (kept.document, kept.kind) == (ask.document, ask.kind) => Some(*id),
            _ => None,
        });
        if let Some(older) = older {
            run.pending.remove(&older);
            run.actions.edit_bases.remove(&older);
            run.actions.executing.remove(&older);
            self.reporter.log(format!("cancelled request {older}"));
            run.send(&protocol::notification(
                "$/cancelRequest",
                json!({"id": older}),
            ))?;
        }
        self.reporter.log(format!(
            "asked {} (request {})",
            ask.kind.method(),
            run.next_id
        ));
        // What its edits are placed against (step 13).
        if matches!(ask.kind, AskKind::CodeAction | AskKind::ResolveAction) {
            run.actions
                .edit_bases
                .insert(run.next_id, self.documents.sent_revisions());
        }
        // While it waits, the server's `workspace/applyEdit` is the UI's to answer (13b).
        if ask.kind == AskKind::ExecuteCommand {
            run.actions.executing.insert(run.next_id);
        }
        run.request(Asked::Ask(ask), ask.kind.method(), params, ask.kind.wait())
    }

    /// The parameters of `ask`'s request, about `text`, the document's at `uri`, or why it
    /// cannot be asked.
    fn ask_params(
        &self,
        run: &Run,
        ask: &Ask,
        (uri, text): (&str, &Rope),
    ) -> Result<Value, String> {
        let at = position::from_offset(text, ask.offset, run.encoding);
        Ok(match (ask.kind, ask.context) {
            (AskKind::Definition | AskKind::Hover, AskContext::None) => {
                protocol::position_params(uri, at)
            }
            (AskKind::References, AskContext::None) => protocol::references_params(uri, at),
            (AskKind::Completion, AskContext::Completion(invoked)) => {
                protocol::completion_params(uri, at, invoked)
            }
            (AskKind::CodeAction, AskContext::Range { start, end }) => {
                self.action_params(run, ask, (start, end), (uri, text))?
            }
            (AskKind::ExecuteCommand, AskContext::Item { answer, index }) => {
                self.command_params(run, ask.document, (answer, index))?
            }
            (AskKind::ResolveAction, AskContext::Item { answer, index }) => {
                let action = (run.actions.last_actions.as_ref())
                    .filter(|last| (last.document, last.serial) == (ask.document, answer))
                    .and_then(|last| last.items.get(index).cloned());
                action.ok_or("its code actions are gone")?
            }
            // The item as the server gave it, if it is one of its last completions'.
            (AskKind::Resolve, AskContext::Item { answer, index }) => {
                let item = (run.last_items.as_ref())
                    .filter(|last| (last.document, last.serial) == (ask.document, answer))
                    .and_then(|last| last.items.get(index).cloned());
                item.ok_or("its completions are gone")?
            }
            _ => return Err("an ask of this kind takes another context".to_owned()),
        })
    }

    /// The answer to `ask`, request `id`, which was due by `deadline`.
    pub(super) fn answered(
        &mut self,
        run: &mut Run,
        (id, ask): (i64, Ask),
        outcome: Outcome,
        deadline: Instant,
        now: Instant,
    ) {
        let took = (now + ask.kind.wait())
            .saturating_duration_since(deadline)
            .as_millis();
        // A command that answered no longer takes a `workspace/applyEdit` (13b).
        run.actions.executing.remove(&id);
        let result = match outcome {
            Outcome::Result(result) => result,
            Outcome::Error { code, message } => {
                self.reporter.log(format!(
                    "request {id} failed with error {code} in {took} ms"
                ));
                let why = if message.is_empty() {
                    format!("error {code}")
                } else {
                    message
                };
                self.fail(ask, why);
                return;
            }
        };
        match ask.kind {
            AskKind::Completion => return self.completed(run, (id, ask), result, took),
            AskKind::Resolve => return self.resolved(run, (id, ask), &result, took),
            AskKind::Hover => return self.answered_hover(id, ask, &result, took),
            AskKind::CodeAction => return self.offered_actions(run, (id, ask), result, took),
            AskKind::ResolveAction => {
                return self.resolved_action(run, (id, ask), &result, took);
            }
            AskKind::ExecuteCommand => return self.executed(id, ask, took),
            AskKind::Definition | AskKind::References => {}
        }
        let (located, left) = protocol::located(&result);
        drop(result);
        let mut locations = Vec::with_capacity(located.len());
        let mut unread = left;
        // One look at the documents for the whole answer, not one for each of its places.
        let sent = self.documents.sent_by_path();
        for place in located {
            let Some(path) = uri::to_path(&place.uri).filter(|path| !automounted(path)) else {
                unread += 1;
                continue;
            };
            let target = match sent.get(path.as_path()).copied() {
                Some((document, revision, text)) => Target::Open {
                    document,
                    revision,
                    range: position::to_range(text, place.start, place.end, run.encoding),
                    start: place.start,
                    encoding: run.encoding,
                },
                None => Target::Lines {
                    start: place.start,
                    end: place.end,
                    encoding: run.encoding,
                },
            };
            locations.push(Location { path, target });
        }
        let count = match locations.len() {
            1 => "1 place".to_owned(),
            count => format!("{count} places"),
        };
        self.reporter.log(format!(
            "request {id} answered with {count} in {took} ms{}",
            if unread > 0 {
                format!(", {unread} more left out")
            } else {
                String::new()
            }
        ));
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Ok(Answered::Places(locations)),
        });
    }

    /// The answer to the description `ask`, request `id`, which took `took` ms: its text, which
    /// only its size reaches `*LSP*`, as it may quote what the server read.
    fn answered_hover(&mut self, id: i64, ask: Ask, result: &serde_json::Value, took: u128) {
        let hover = protocol::hover(result);
        self.reporter.log(format!(
            "request {id} answered with {} bytes of description in {took} ms",
            hover.as_ref().map_or(0, |hover| hover.text.len())
        ));
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Ok(Answered::Hover(hover)),
        });
    }

    /// The completions `result` offers for `ask`, request `id`, their ranges placed in the text
    /// the server has of the document.
    fn completed(
        &mut self,
        run: &mut Run,
        (id, ask): (i64, Ask),
        result: serde_json::Value,
        took: u128,
    ) {
        let completed = protocol::completed(&result);
        // Kept for their resolve, taken out of the answer, while they are the document's last;
        // the last ones go first.
        run.last_items = None;
        if run.resolves {
            run.last_items = Some(LastItems {
                document: ask.document,
                serial: ask.serial,
                items: protocol::take_items(result, &completed.at),
            });
        }
        let Some((revision, text)) = self.documents.sent(ask.document) else {
            self.fail(ask, "the document is no longer open with it".to_owned());
            return;
        };
        // A range is kept only around the position asked about, and an edit besides an item's
        // own at all, in the text asked about.
        let asked = revision == ask.revision;
        let at = asked.then(|| position::from_offset(text, ask.offset, run.encoding));
        let encoding = run.encoding;
        let items: Vec<Completion> = (completed.items.into_iter())
            .map(|offered| Completion {
                range: (at.and_then(|at| offered.range_around(at)))
                    .map(|(start, end)| position::to_range(text, start, end, encoding)),
                extra: (offered.extra.into_iter())
                    .filter(|_| asked)
                    .map(|(start, end, new)| (position::to_range(text, start, end, encoding), new))
                    .collect(),
                label: offered.label,
                kind: offered.kind,
                detail: offered.detail,
                filter: offered.filter,
                text: offered.text,
            })
            .collect();
        self.reporter.log(format!(
            "request {id} answered with {} completions in {took} ms{}{}",
            items.len(),
            if completed.incomplete {
                ", incomplete"
            } else {
                ""
            },
            if completed.left > 0 {
                format!(", {} more left out", completed.left)
            } else {
                String::new()
            }
        ));
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Ok(Answered::Completion(Completions {
                revision,
                items,
                incomplete: completed.incomplete,
            })),
        });
    }

    /// An item's edits besides its own, which `result` resolves for `ask`, request `id`, their
    /// ranges placed in the text the server has of the document.
    fn resolved(
        &mut self,
        run: &Run,
        (id, ask): (i64, Ask),
        result: &serde_json::Value,
        took: u128,
    ) {
        let AskContext::Item { answer, index } = ask.context else {
            self.fail(ask, "a resolve names no item".to_owned());
            return;
        };
        // Placed against the text the resolve was asked in, which the server's ranges are of.
        let Some(text) = self.documents.given_text(ask.document, ask.revision) else {
            self.fail(ask, "the text it was asked in is gone".to_owned());
            return;
        };
        let revision = ask.revision;
        let extra: Vec<(std::ops::Range<usize>, String)> = (protocol::resolved(result).into_iter())
            .map(|(start, end, new)| (position::to_range(text, start, end, run.encoding), new))
            .collect();
        self.reporter.log(format!(
            "request {id} resolved with {} more edits in {took} ms",
            extra.len()
        ));
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Ok(Answered::Resolved(Resolved {
                answer,
                index,
                revision,
                extra,
            })),
        });
    }

    /// An ask whose wait ended: cancelled, and failed.
    pub(super) fn overdue_ask(&mut self, run: &mut Run, id: i64, ask: Ask) -> Result<(), String> {
        run.actions.edit_bases.remove(&id);
        run.actions.executing.remove(&id);
        self.reporter.log(format!(
            "request {id} not answered within {} s, cancelled",
            ask.kind.wait().as_secs()
        ));
        self.answer_failed(ask, AskFailure::Overdue);
        run.send(&protocol::notification(
            "$/cancelRequest",
            json!({"id": id}),
        ))
    }

    /// The run ended: what waits fails.
    pub(super) fn fail_asks(&mut self, run: &mut Run) {
        run.actions.edit_bases.clear();
        run.actions.executing.clear();
        let waiting: Vec<Ask> = run
            .pending
            .drain()
            .filter_map(|(_, (asked, _))| match asked {
                Asked::Ask(ask) => Some(ask),
                _ => None,
            })
            .collect();
        for ask in waiting {
            self.fail(ask, "it stopped before it answered".to_owned());
        }
    }

    pub(super) fn fail(&mut self, ask: Ask, why: String) {
        self.answer_failed(ask, AskFailure::Other(why));
    }

    fn answer_failed(&mut self, ask: Ask, failure: AskFailure) {
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Err(failure),
        });
    }
}

/// Where the system mounts other computers' files on a first look: macOS's `/net` and
/// `/Network`, Linux's `/net`. A place there is left out, as opening it would reach another
/// computer, and a mount that hangs would hold the threads that look at it.
///
/// The system's own files are left out too: `/dev`, `/proc` and `/sys` hold devices and what the
/// system says of its processes, oso's own among them, never a definition (step 8b).
const AUTOMOUNTS: &[&str] = if cfg!(target_os = "macos") {
    &["net", "Network", "dev"]
} else if cfg!(target_os = "linux") {
    &["net", "dev", "proc", "sys"]
} else if cfg!(unix) {
    &["dev", "proc", "sys"]
} else {
    &[]
};

/// Whether `path` is under one of the [`AUTOMOUNTS`], in any case on macOS, whose file
/// system is usually blind to it, or has a `..` that could lead there.
pub(super) fn automounted(path: &std::path::Path) -> bool {
    use std::path::Component;
    if path.components().any(|part| part == Component::ParentDir) {
        return true;
    }
    let mut parts = path.components();
    let (Some(Component::RootDir), Some(Component::Normal(first))) = (parts.next(), parts.next())
    else {
        return false;
    };
    let first = first.to_string_lossy();
    AUTOMOUNTS.iter().any(|mount| {
        if cfg!(target_os = "macos") {
            mount.eq_ignore_ascii_case(&first)
        } else {
            *mount == first
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_place_under_an_automount_is_left_out() {
        let unix = cfg!(any(target_os = "macos", target_os = "linux"));
        assert_eq!(automounted(Path::new("/net/host.example/x.rs")), unix);
        assert_eq!(
            automounted(Path::new("/Network/Servers/x.rs")),
            cfg!(target_os = "macos")
        );
        assert_eq!(
            automounted(Path::new("/NET/host/x.rs")),
            cfg!(target_os = "macos"),
            "macOS's file system is blind to case"
        );
        assert!(
            automounted(Path::new("/w/../net/host/x.rs")),
            "a `..` could lead there"
        );
        assert_eq!(
            automounted(Path::new("/proc/self/environ")),
            cfg!(target_os = "linux"),
            "the system's own files"
        );
        assert_eq!(automounted(Path::new("/dev/stdin")), unix);
        assert!(!automounted(Path::new("/home/me/net/x.rs")));
        assert!(!automounted(Path::new("/netx/x.rs")));
    }
}
