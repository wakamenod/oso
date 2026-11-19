//! `space_add_repo` (ADR 0161): a space's orchestrator adds a repository to its own space, as
//! the person's `space-add-repo` does. The path is looked at on a thread of its own, as the
//! person's is, and must also name the main checkout with no link on the way. The person is
//! asked, or it goes without asking by ADR 0135's rules, past no limit; one oso does not trust
//! always asks, and joins restricted. At the person's `y` the repository is looked at again, and
//! must be the same, by its real path, device and inode. Once added and trusted, oso offers the
//! person to paste `/add-dir` into the orchestrator, as for any repository added while it runs.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};

use super::super::super::App;
use super::asks::{Plan, Words};
use super::question::details;
use super::{Caller, Pending, shown};
use crate::channel::Connection;
use crate::sessions::spaces::edit::NotAdded;
use crate::sessions::spaces::stored::{MAX_MEMBERS, is_plain, same};
use crate::sessions::spaces::{Member, SpaceId};
use crate::sessions::tools::{self as rules, Refusal, Tool};
use crate::sessions::worker::SessionKey;

/// What `?` shows of an add, after the question.
const ADD_DETAILS: &str = "The repository is listed in the space. Once oso trusts it, the \
orchestrator may start workers in its worktrees and give them it to read, and oso offers to \
paste /add-dir PATH into the orchestrator's input, not sent: Claude Code then loads that \
repository's CLAUDE.md, skills, commands and subagents into it. One oso does not trust stays \
restricted until you trust it with trust-project.";

/// An agent's look at the repository its call names, on a thread of its own.
#[derive(Debug)]
pub(in crate::app) struct AddLook {
    pending: Pending,
    /// The path as the call gave it.
    path: PathBuf,
    stage: Stage,
}

/// What the look is for.
#[derive(Debug)]
enum Stage {
    /// Before the person is asked: the calling orchestrator and its space.
    First { caller: SessionKey, space: SpaceId },
    /// At the person's `y`: the repository as the first look found it.
    Again(Box<Adding>),
}

/// What the person's `y`, or an add without asking, adds.
#[derive(Debug)]
pub(in crate::app) struct Adding {
    /// The orchestrator that asked, and its space.
    caller: SessionKey,
    caller_name: String,
    space: SpaceId,
    /// The repository as the first look found it.
    member: Member,
}

impl AddLook {
    /// The path it looks at.
    pub(in crate::app) fn path(&self) -> &Path {
        &self.path
    }

    /// The connection of the call it is for.
    pub(in crate::app) fn connection(&self) -> Connection {
        self.pending.connection
    }

    /// Whether the person has not allowed it yet, so that it ends with its connection.
    pub(in crate::app) fn before_question(&self) -> bool {
        matches!(self.stage, Stage::First { .. })
    }
}

impl App {
    /// `space_add_repo` of `path` from `caller`.
    pub(super) fn tool_space_add_repo(
        &mut self,
        pending: Pending,
        tool: &Tool,
        caller: &Caller,
        path: PathBuf,
    ) {
        let Some(space) = (caller.space.as_ref()).filter(|_| caller.orchestrator()) else {
            return pending.answer(Err(
                "only a space's orchestrator adds repositories; the person adds one with \
                 space-add-repo"
                    .into(),
            ));
        };
        let space = space.id.clone();
        let Some(text) = path.to_str().filter(|text| text.chars().all(is_plain)) else {
            return pending.answer(Err(
                "the path holds a character oso does not take in a repository's path".into(),
            ));
        };
        if let Err(why) = self.admit_tool(tool, caller, text) {
            return pending.answer(Err(why));
        }
        if self.sessions.is_none() {
            return pending.answer(Err(
                "spaces need the sessions worker, which did not start".into()
            ));
        }
        let stage = Stage::First {
            caller: caller.key,
            space,
        };
        self.check_repository_for_agent(AddLook {
            pending,
            path,
            stage,
        });
    }

    /// The look at an agent's repository ended with `member`.
    pub(in crate::app) fn agent_repository_checked(
        &mut self,
        look: AddLook,
        member: Result<Member, String>,
    ) {
        let AddLook { pending, stage, .. } = look;
        let member = match member {
            Ok(member) => member,
            Err(why) => return pending.answer(Err(why)),
        };
        match stage {
            Stage::First { caller, space } => self.ask_to_add(pending, caller, space, member),
            Stage::Again(adding) => {
                let changed = !same(&member.root, &adding.member.root)
                    || member.identity != adding.member.identity;
                if changed {
                    return pending.answer(Err(format!(
                        "the repository at {} changed since the person was asked; nothing was \
                         added",
                        shown(&adding.member.root)
                    )));
                }
                self.add_for_agent(pending, *adding, None);
            }
        }
    }

    /// The first look found repository `member` for orchestrator `caller` of `space`: the
    /// person is asked, or it is added without asking as ADR 0135's rules let it.
    fn ask_to_add(&mut self, pending: Pending, caller: SessionKey, space: SpaceId, member: Member) {
        let caller = match self.still_orchestrates(caller, &space) {
            Ok(caller) => caller,
            Err(why) => return pending.answer(Err(why.into())),
        };
        if let Err(why) = self.may_add(&space, &member) {
            return pending.answer(Err(why));
        }
        let target = shown(&member.root);
        let refused =
            self.hosting
                .tools
                .guard
                .was_refused(&caller.id, pending.tool, &target, Instant::now());
        if refused {
            return pending.answer(Err(Refusal::Person.why().into()));
        }
        let name = self.space_name(&space);
        let trusted = self.member_trust(&member).is_some();
        let label = rules::add_repo_question(&caller.name, &target, &name, trusted);
        let said = rules::add_repo_said(&caller.name, &target, &name);
        let words = Words(
            label.clone(),
            details(&caller.name, &label, ADD_DETAILS),
            said,
        );
        let adding = Adding {
            caller: caller.key,
            caller_name: caller.name.clone(),
            space,
            member,
        };
        let plan = Plan::AddRepo(Box::new(adding));
        self.ask_or_start(pending, &caller, (target, trusted), words, plan);
    }

    /// The person's `y`, or an add without asking: without asking it is added now, as the
    /// first look found it; after a `y` it is looked at again first, and must be the same.
    pub(super) fn carry_out_add(&mut self, pending: Pending, adding: Adding, said: Option<String>) {
        if pending.unasked {
            return self.add_for_agent(pending, adding, said);
        }
        if self.sessions.is_none() {
            return pending.answer(Err(
                "spaces need the sessions worker, which did not start".into()
            ));
        }
        let path = adding.member.root.clone();
        let stage = Stage::Again(Box::new(adding));
        self.check_repository_for_agent(AddLook {
            pending,
            path,
            stage,
        });
    }

    /// Adds `adding`'s repository to its space, if its caller is still the space's
    /// orchestrator and the space still takes it, and replies with what the agent can do next.
    fn add_for_agent(&mut self, pending: Pending, adding: Adding, said: Option<String>) {
        if let Err(why) = self.still_orchestrates(adding.caller, &adding.space) {
            return pending.answer(Err(why.into()));
        }
        if let Err(why) = self.may_add(&adding.space, &adding.member) {
            return pending.answer(Err(why));
        }
        let Some(at) = self.watch.spaces().find(&adding.space) else {
            return pending.answer(Err("your space is gone".into()));
        };
        let root = adding.member.root.clone();
        let trusted = self.member_trust(&adding.member).is_some();
        let now = crate::sessions::spaces::stored::now_ms();
        match self.watch.spaces_mut().add_member(at, adding.member, now) {
            Ok(()) => {}
            Err(NotAdded::Already) => {
                return pending.answer(Err("your space has that repository already".into()));
            }
            Err(NotAdded::Full) => return pending.answer(Err(full())),
        }
        let space = self.space_name(&adding.space);
        let message = said.unwrap_or_else(|| {
            format!(
                "session {} added repository {} to space {}",
                rules::quoted(&adding.caller_name),
                rules::quoted(&shown(&root)),
                rules::quoted(&space)
            )
        });
        self.editor.message(format!("Agent: {message}"));
        self.agent_log(&format!("{}: added", pending.tool));
        // A trusted one is offered to the running orchestrator after this event (ADR 0136, Q5).
        let reply = self.added_reply(at, &adding.space, &root, trusted, pending.unasked);
        self.sessions_touched();
        self.request_redraw();
        pending.answer(Ok(reply));
    }

    /// The caller `key` as it is now, if it is still `space`'s orchestrator.
    fn still_orchestrates(&self, key: SessionKey, space: &SpaceId) -> Result<Caller, &'static str> {
        let caller = (self.hosting.table.get(key)).map(|session| self.caller_of(session));
        caller
            .filter(|caller| {
                caller.orchestrator() && (caller.space.as_ref()).is_some_and(|at| at.id == *space)
            })
            .ok_or("your session is no longer its space's orchestrator")
    }

    /// Whether `space` takes repository `member`: it is open, has it not, and is not full.
    fn may_add(&self, space: &SpaceId, member: &Member) -> Result<(), String> {
        let spaces = self.watch.spaces();
        let stored = (spaces.find(space))
            .and_then(|at| spaces.stored(at))
            .filter(|stored| !stored.archived)
            .ok_or("your space is closed or gone")?;
        if stored.has_member(&member.root) {
            return Err("your space has that repository already".into());
        }
        if stored.members.len() >= MAX_MEMBERS {
            return Err(full());
        }
        Ok(())
    }

    /// The slug of `space`, which its orchestrator answers to.
    fn space_name(&self, space: &SpaceId) -> String {
        let spaces = self.watch.spaces();
        (spaces.find(space))
            .and_then(|at| spaces.stored(at))
            .map_or_else(String::new, |stored| stored.name.clone())
    }

    /// What `space_add_repo` replies once `root` is in space `at`: the repository as
    /// `space_status` names it, and whether the orchestrator can read it yet (Q5).
    fn added_reply(
        &self,
        at: usize,
        space: &SpaceId,
        root: &Path,
        trusted: bool,
        unasked: bool,
    ) -> Value {
        let spaces = self.watch.spaces();
        let name = (spaces.member_names(at).into_iter())
            .find(|(_, member)| same(member, root))
            .map(|(name, _)| name);
        let given = (self.hosting.spaces.given.get(space))
            .is_some_and(|given| given.iter().any(|was| same(was, root)));
        let path = shown(root);
        let (add_dir, note) = match (trusted, given) {
            (false, _) => (
                "waits for trust",
                format!(
                    "oso does not trust it: it is restricted, with no worktrees, workers or \
                     --add-dir, until the person trusts it with trust-project in oso; oso then \
                     offers them to paste /add-dir {path} into your input."
                ),
            ),
            (true, true) => (
                "given",
                "your claude was given it with --add-dir already.".to_owned(),
            ),
            (true, false) => (
                "offered",
                format!(
                    "oso asks the person whether to paste /add-dir {path} into your input, not \
                     sent. You cannot read it until they accept it and send it: ask them to. \
                     Workers you start with this repo can use it now."
                ),
            ),
        };
        let note = if unasked {
            format!("{note} Added without asking, as the person's config.toml allows.")
        } else {
            note
        };
        json!({
            "repository": { "name": name, "path": path, "trusted": trusted },
            "asked": !unasked,
            "add_dir": add_dir,
            "note": note,
        })
    }
}

/// Why a full space takes no more.
fn full() -> String {
    format!("a space holds at most {MAX_MEMBERS} repositories")
}
