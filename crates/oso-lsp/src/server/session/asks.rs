//! The UI's asks about documents on the session thread (ADR 0138, step 8): each becomes a
//! request at the position its offset is in the text the server has, a newer one cancelling
//! the older of its kind for the document, and each answer, or the end of its wait or of the
//! run, becomes an [`Answer`] for the UI.
//!
//! Must not depend on: the process or the pipes; the run sends, and the reporter hands over.

use std::time::Instant;

use serde_json::json;

use crate::position;
use crate::protocol::{self, Outcome};
use crate::uri;

use super::super::{Answer, Answered, Ask, AskKind, Location, Target};
use super::run::Run;
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
        let at = position::from_offset(text, ask.offset, run.encoding);
        let params = match ask.kind {
            AskKind::Definition | AskKind::Hover => protocol::position_params(uri, at),
            AskKind::References => protocol::references_params(uri, at),
        };
        // The older of its kind for the document is moot.
        let older = run.pending.iter().find_map(|(id, (asked, _))| match asked {
            Asked::Ask(kept) if (kept.document, kept.kind) == (ask.document, ask.kind) => Some(*id),
            _ => None,
        });
        if let Some(older) = older {
            run.pending.remove(&older);
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
        run.request(Asked::Ask(ask), ask.kind.method(), params, ask.kind.wait())
    }

    /// The answer to `ask`, request `id`, which was due by `deadline`.
    pub(super) fn answered(
        &mut self,
        run: &Run,
        (id, ask): (i64, Ask),
        outcome: Outcome,
        deadline: Instant,
        now: Instant,
    ) {
        let took = (now + ask.kind.wait())
            .saturating_duration_since(deadline)
            .as_millis();
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
        if ask.kind == AskKind::Hover {
            let hover = protocol::hover(&result);
            drop(result);
            // Its size only: the text may quote what the server read.
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
            return;
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

    /// An ask whose wait ended: cancelled, and failed.
    pub(super) fn overdue_ask(&mut self, run: &Run, id: i64, ask: Ask) -> Result<(), String> {
        self.reporter.log(format!(
            "request {id} not answered within {} s, cancelled",
            ask.kind.wait().as_secs()
        ));
        self.fail(
            ask,
            format!("no answer within {} s", ask.kind.wait().as_secs()),
        );
        run.send(&protocol::notification(
            "$/cancelRequest",
            json!({"id": id}),
        ))
    }

    /// The run ended: what waits fails.
    pub(super) fn fail_asks(&mut self, run: &mut Run) {
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

    fn fail(&mut self, ask: Ask, why: String) {
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Err(why),
        });
    }
}

/// Where the system mounts other computers' files on a first look: macOS's `/net` and
/// `/Network`, Linux's `/net`. A place there is left out, as opening it would reach another
/// computer, and a mount that hangs would hold the threads that look at it.
const AUTOMOUNTS: &[&str] = if cfg!(target_os = "macos") {
    &["net", "Network"]
} else if cfg!(target_os = "linux") {
    &["net"]
} else {
    &[]
};

/// Whether `path` is under one of the [`AUTOMOUNTS`], in any case on macOS, whose file
/// system is usually blind to it, or has a `..` that could lead there.
fn automounted(path: &std::path::Path) -> bool {
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
        assert!(!automounted(Path::new("/home/me/net/x.rs")));
        assert!(!automounted(Path::new("/netx/x.rs")));
    }
}
