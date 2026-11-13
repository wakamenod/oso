//! The session tools that only read (ADR 0121, step 6): `session_list` and `session_state`
//! from the hosted table, and `worktree_list` from a listing job whose rows it gathers.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::super::App;
use super::super::hosting::now_ms;
use super::{Caller, Job, Lister, Pending, shown};
use crate::channel::Reply;
use crate::channel::protocol::sessions::JOB_ANSWER_WITHIN;
use crate::sessions::hosted::Hosted;
use crate::sessions::identity::SessionId;
use crate::sessions::worktree::checks::{self, Held, Looked};
use crate::sessions::worktree::jobs::{Report, Trust};

/// The most sessions one reply of `session_list` gives.
const PAGE: usize = 256;
/// The most worktree listings one caller runs a minute, and at once.
const LISTINGS_A_MINUTE: usize = 6;
/// How long a listing gathers before its reply goes with the rows so far, or a removal runs
/// before its reply says it goes on: within the server's wait for the reply.
pub(in crate::app) const JOB_WITHIN: Duration = JOB_ANSWER_WITHIN;

/// A worktree listing an agent waits for.
#[derive(Debug)]
pub(in crate::app) struct Listing {
    pub(super) pending: Pending,
    started: Instant,
    /// How many rows the job found, once it says.
    expected: Option<usize>,
    /// Each row with what the UI held of it when it came, so that the reply costs little.
    rows: Vec<ListedRow>,
}

#[derive(Debug)]
struct ListedRow {
    looked: Looked,
    held: Held,
    sessions: Vec<String>,
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
        let now = (Instant::now(), now_ms());
        let you = caller.map(|caller| &caller.id);
        let mut seen = self
            .hosting
            .table
            .all()
            .iter()
            .filter(|s| {
                within
                    .as_deref()
                    .is_none_or(|root| s.directory.starts_with(root))
            })
            .skip(offset);
        let sessions: Vec<Value> = seen
            .by_ref()
            .take(PAGE)
            .map(|session| self.session_json(session, you, now))
            .collect();
        let more = seen.next().is_some();
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
        let now = (Instant::now(), now_ms());
        let mut state = self.session_json(found, caller.map(|c| &c.id), now);
        // A review keyed to a session comes with ADR 0122's later steps.
        state["review"] = Value::Null;
        Ok(state)
    }

    /// A reply for a session, as `session_list` and `session_state` give it, with its state as
    /// `*Sessions*` and `*Worktrees*` have it: the taxonomy's, or `starting` or `running` while
    /// its live state is not known. `now` is the time as an instant and in ms since the epoch.
    fn session_json(
        &self,
        session: &Hosted,
        caller: Option<&SessionId>,
        now: (Instant, u64),
    ) -> Value {
        let live = self.live_state(session);
        let state = self
            .hosted_state(session)
            .map_or_else(|| session.run.label().to_owned(), |state| state.to_string());
        let age_s = match live {
            Some((_, since)) => now.0.saturating_duration_since(since).as_secs(),
            None => now.1.saturating_sub(session.changed) / 1000,
        };
        json!({
            "id": session.id.as_str(),
            "name": session.name,
            // The `--name` oso gave its claude, which Claude Code's SendMessage and ListAgents
            // know it by; the CLI may suffix a duplicate.
            "messaging_name": session.name,
            "directory": shown(&session.directory),
            "project": session.root.as_deref().map(shown),
            "space": self.session_space(session.key),
            "state": state,
            "age_s": age_s,
            "pinned": session.pinned,
            "archived": session.archived,
            "parent": session.parent.as_ref().map(SessionId::as_str),
            "external": false,
            "you": caller == Some(&session.id),
        })
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
        // One agent listing runs at a time, whoever asks, so that agents never fill the
        // threads the person's worktree commands use.
        if self.hosting.tools.listing.is_some() {
            return pending.answer(Err(
                "another agent's worktree listing is running; try again in a moment".into(),
            ));
        }
        let now = Instant::now();
        let recent = self.hosting.tools.listings.entry(lister).or_default();
        recent.retain(|&at| now.saturating_duration_since(at) < Duration::from_secs(60));
        if recent.len() >= LISTINGS_A_MINUTE {
            return pending.answer(Err(format!(
                "at most {LISTINGS_A_MINUTE} worktree listings a minute; try again later"
            )));
        }
        recent.push(now);
        let token = self.worktree_token();
        let snapshot = self.worktree_snapshot();
        let stop = Arc::new(AtomicBool::new(false));
        self.worktree_jobs()
            .list_for_agent(token, trust, snapshot, Arc::clone(&stop));
        self.hosting.tools.listing = Some((token, stop));
        self.wake_after(JOB_WITHIN);
        self.hosting.tools.jobs.insert(
            token,
            Job::Listing(Listing {
                pending,
                started: now,
                expected: None,
                rows: Vec::new(),
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
            Report::Row { looked, .. } => {
                let path = &looked.listed.path;
                let (held, sessions) = (self.held_at(path), self.sessions_in(path));
                listing.rows.push(ListedRow {
                    looked: *looked,
                    held,
                    sessions,
                });
            }
            Report::Merged { path, merged, .. } => {
                if let Some(row) = listing
                    .rows
                    .iter_mut()
                    .find(|r| r.looked.listed.path == path)
                {
                    row.looked.merged = Some(merged);
                    row.looked.gh_pending = None;
                }
            }
            Report::ListEnded { .. } => {
                let reply = self.listing_reply(&listing, false);
                return listing.pending.answer(Ok(reply));
            }
            _ => {}
        }
        self.hosting.tools.jobs.insert(token, Job::Listing(listing));
    }

    /// Replies to listings past [`JOB_WITHIN`] with the rows they have, and to removals past it
    /// that they go on. A removal still ends as its job says, and the echo area tells the
    /// person.
    pub(super) fn end_slow_jobs(&mut self) {
        let slow: Vec<u64> = self
            .hosting
            .tools
            .jobs
            .iter()
            .filter(|(_, job)| match job {
                Job::Listing(listing) => listing.started.elapsed() >= JOB_WITHIN,
                Job::Removing(removing) => {
                    removing.pending.is_some() && removing.started.elapsed() >= JOB_WITHIN
                }
                Job::Prepared(_) | Job::Creating(_) => false,
            })
            .map(|(&token, _)| token)
            .collect();
        for token in slow {
            match self.hosting.tools.jobs.get_mut(&token) {
                Some(Job::Removing(removing)) => {
                    if let Some(pending) = removing.pending.take() {
                        pending.answer(Err(
                            "the removal still runs: worktree_list shows whether it went; do not remove the worktree another way".into(),
                        ));
                    }
                }
                Some(Job::Listing(_)) => {
                    if let Some(Job::Listing(listing)) = self.hosting.tools.jobs.remove(&token) {
                        self.stop_agent_listing(token);
                        let reply = self.listing_reply(&listing, true);
                        listing.pending.answer(Ok(reply));
                    }
                }
                _ => {}
            }
        }
    }

    /// The reply for `listing`'s rows; `cut` when it ran out of time.
    fn listing_reply(&self, listing: &Listing, cut: bool) -> Value {
        let rows: Vec<Value> = listing
            .rows
            .iter()
            .map(|row| worktree_row_json(row, cut))
            .collect();
        json!({
            "worktrees": rows,
            "complete": !cut && listing.expected == Some(listing.rows.len()),
        })
    }

    /// Stops the agent listing `token`, if it runs, between its rows.
    pub(super) fn stop_agent_listing(&mut self, token: u64) {
        if let Some((running, stop)) = &self.hosting.tools.listing
            && *running == token
        {
            stop.store(true, Ordering::SeqCst);
        }
    }
}

fn worktree_row_json(row: &ListedRow, cut: bool) -> Value {
    let looked = &row.looked;
    let path: &Path = &looked.listed.path;
    let (merged, how) = match (&looked.merged, &looked.gh_pending) {
        (Some(merged), _) => (Some(merged.is_merged()), merged.how()),
        (None, Some(_)) if cut => (None, "unknown: gh was still asked".to_owned()),
        (None, _) => (None, "unknown".to_owned()),
    };
    let removable = checks::removable(looked, &row.held);
    json!({
        "path": shown(path),
        "branch": looked.listed.branch,
        "detached": looked.listed.detached,
        "uncommitted": looked.status.map(|status| status.changed),
        "untracked": looked.status.map(|status| status.untracked),
        "merged": merged,
        "merged_how": how,
        "sessions": row.sessions,
        "other_sessions": looked.external,
        "unsaved_buffers": row.held.unsaved,
        "removable": removable.is_ok(),
        "kept_because": removable.err().map(|check| check.why()),
    })
}
