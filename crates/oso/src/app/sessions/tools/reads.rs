//! The session tools that only read (ADR 0121, step 6): `session_list` and `session_state`
//! from the hosted table, and `worktree_list` from a listing job whose rows it gathers.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::super::super::App;
use super::{Caller, Job, Lister, Pending, session_json, shown};
use crate::channel::Reply;
use crate::sessions::identity::SessionId;
use crate::sessions::worktree::checks::{self, Looked};
use crate::sessions::worktree::jobs::{Report, Trust};
use crate::sessions::worktree::merged::Merged;

/// The most sessions one reply of `session_list` gives.
const PAGE: usize = 256;
/// The most worktree listings one caller runs a minute, and at once.
const LISTINGS_A_MINUTE: usize = 6;
/// How long a listing gathers before its reply goes with the rows so far.
pub(in crate::app) const LIST_WITHIN: Duration = Duration::from_secs(60);

/// A worktree listing an agent waits for.
#[derive(Debug)]
pub(in crate::app) struct Listing {
    pub(super) pending: Pending,
    started: Instant,
    /// How many rows the job found, once it says.
    expected: Option<usize>,
    rows: Vec<Looked>,
    /// What `gh` said, by the row's path.
    merged: HashMap<PathBuf, Merged>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

impl App {
    /// The selected window's project's root, for a connection that is no hosted session.
    fn outside_root(&self) -> Result<PathBuf, String> {
        let project = self.editor.current().1.project();
        project
            .and_then(|id| self.editor.project(id))
            .map(|project| project.root().to_path_buf())
            .ok_or_else(|| "the selected window shows no project".to_owned())
    }

    /// `session_list`: every hosted session for a session's agent (the author's choice), only
    /// the selected window's project's for another, from `offset`.
    pub(super) fn session_list(&self, caller: Option<&Caller>, offset: usize) -> Reply {
        let within = match caller {
            Some(_) => None,
            None => Some(self.outside_root()?),
        };
        let now = now_ms();
        let you = caller.map(|caller| &caller.id);
        let sessions: Vec<Value> = self
            .hosting
            .table
            .all()
            .iter()
            .filter(|s| {
                within
                    .as_deref()
                    .is_none_or(|root| s.directory.starts_with(root))
            })
            .skip(offset)
            .take(PAGE)
            .map(|session| session_json(session, you, now))
            .collect();
        let more = self.hosting.table.all().len() > offset.saturating_add(PAGE);
        Ok(json!({
            "you": you.map(SessionId::as_str),
            "sessions": sessions,
            "more": more,
        }))
    }

    /// `session_state`: one session, by its id or its name.
    pub(super) fn session_state(&self, caller: Option<&Caller>, session: &str) -> Reply {
        let within = match caller {
            Some(_) => None,
            None => Some(self.outside_root()?),
        };
        let all = self.hosting.table.all();
        let seen = |s: &&crate::sessions::hosted::Hosted| {
            within
                .as_deref()
                .is_none_or(|root| s.directory.starts_with(root))
        };
        let by_id = all.iter().filter(seen).find(|s| s.id.as_str() == session);
        let found = match by_id {
            Some(found) => found,
            None => {
                let mut named = all.iter().filter(seen).filter(|s| s.name == session);
                match (named.next(), named.next()) {
                    (Some(found), None) => found,
                    (Some(_), Some(_)) => {
                        return Err("several sessions have that name; give its id".into());
                    }
                    (None, _) => return Err("no session has that id or name".into()),
                }
            }
        };
        let mut state = session_json(found, caller.map(|c| &c.id), now_ms());
        // A review keyed to a session comes with ADR 0122's later steps.
        state["review"] = Value::Null;
        Ok(state)
    }

    /// `worktree_list`: the linked worktrees of the caller's repository, or of the selected
    /// window's project for another agent, gathered from a listing job.
    pub(super) fn tool_worktree_list(&mut self, pending: Pending, caller: Option<Caller>) {
        let (lister, trust) = match &caller {
            Some(caller) => (
                Lister::Session(caller.id.clone()),
                self.caller_trust(caller),
            ),
            None => (
                Lister::Outside(pending.connection),
                self.project_here().and_then(|id| self.trust_for(id)),
            ),
        };
        let trust: Trust = match trust {
            Ok(trust) => trust,
            Err(why) => return pending.answer(Err(why)),
        };
        let now = Instant::now();
        let busy = self.hosting.tools.jobs.values().any(|job| {
            matches!(job, Job::Listing(listing) if listing.pending.connection == pending.connection)
        });
        let recent = self.hosting.tools.listings.entry(lister).or_default();
        recent.retain(|&at| now.saturating_duration_since(at) < Duration::from_secs(60));
        if busy || recent.len() >= LISTINGS_A_MINUTE {
            return pending.answer(Err(format!(
                "at most one listing at a time and {LISTINGS_A_MINUTE} a minute; try again later"
            )));
        }
        recent.push(now);
        let token = self.worktree_token();
        let snapshot = self.worktree_snapshot();
        self.worktree_jobs().list_for_agent(token, trust, snapshot);
        self.wake_after(LIST_WITHIN);
        self.hosting.tools.jobs.insert(
            token,
            Job::Listing(Listing {
                pending,
                started: now,
                expected: None,
                rows: Vec::new(),
                merged: HashMap::new(),
            }),
        );
    }

    /// A listing job reported for `listing`.
    pub(super) fn tool_listing(&mut self, token: u64, mut listing: Listing, report: Report) {
        match report {
            Report::Listed { result: Ok(n), .. } => listing.expected = Some(n),
            Report::Listed {
                result: Err(why), ..
            } => return listing.pending.answer(Err(why)),
            Report::Row { looked, .. } => listing.rows.push(*looked),
            Report::Merged { path, merged, .. } => {
                listing.merged.insert(path, merged);
            }
            Report::ListEnded { .. } => {
                let reply = self.listing_reply(&listing, false);
                return listing.pending.answer(Ok(reply));
            }
            _ => {}
        }
        self.hosting.tools.jobs.insert(token, Job::Listing(listing));
    }

    /// Replies to listings past [`LIST_WITHIN`] with the rows they have.
    pub(super) fn end_slow_listings(&mut self) {
        let slow: Vec<u64> = self
            .hosting
            .tools
            .jobs
            .iter()
            .filter(|(_, job)| {
                matches!(job, Job::Listing(listing) if listing.started.elapsed() >= LIST_WITHIN)
            })
            .map(|(&token, _)| token)
            .collect();
        for token in slow {
            if let Some(Job::Listing(listing)) = self.hosting.tools.jobs.remove(&token) {
                let reply = self.listing_reply(&listing, true);
                listing.pending.answer(Ok(reply));
            }
        }
    }

    /// The reply for `listing`'s rows; `cut` when it ran out of time.
    fn listing_reply(&self, listing: &Listing, cut: bool) -> Value {
        let rows: Vec<Value> = listing
            .rows
            .iter()
            .map(|looked| {
                let mut looked = looked.clone();
                if let Some(merged) = listing.merged.get(&looked.listed.path) {
                    looked.merged = Some(merged.clone());
                    looked.gh_pending = None;
                }
                self.worktree_row_json(&looked, cut)
            })
            .collect();
        json!({
            "worktrees": rows,
            "complete": !cut && listing.expected == Some(listing.rows.len()),
        })
    }

    fn worktree_row_json(&self, looked: &Looked, cut: bool) -> Value {
        let path: &Path = &looked.listed.path;
        let held = self.held_at(path);
        let (merged, how) = match (&looked.merged, &looked.gh_pending) {
            (Some(merged), _) => (Some(merged.is_merged()), merged.how()),
            (None, Some(_)) if cut => (None, "unknown: gh was still asked".to_owned()),
            (None, _) => (None, "unknown".to_owned()),
        };
        let removable = checks::removable(looked, &held);
        json!({
            "path": shown(path),
            "branch": looked.listed.branch,
            "detached": looked.listed.detached,
            "uncommitted": looked.status.map(|status| status.changed),
            "untracked": looked.status.map(|status| status.untracked),
            "merged": merged,
            "merged_how": how,
            "sessions": self.sessions_in(path),
            "other_sessions": looked.external,
            "unsaved_buffers": held.unsaved,
            "removable": removable.is_ok(),
            "kept_because": removable.err().map(|check| check.why()),
        })
    }
}
