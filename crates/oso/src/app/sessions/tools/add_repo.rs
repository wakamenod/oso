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

use serde_json::json;

use super::super::super::App;
use super::asks::{Plan, Words};
use super::question::details;
use super::{Caller, Pending, shown};
use crate::channel::Connection;
use crate::sessions::spaces::stored::{is_plain, same};
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
    /// Its connection went before the person allowed it: it ends adding nothing.
    abandoned: bool,
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

    /// The orchestrator it is for.
    pub(in crate::app) fn caller(&self) -> SessionKey {
        match &self.stage {
            Stage::First { caller, .. } => *caller,
            Stage::Again(adding) => adding.caller,
        }
    }

    /// Answers its call with `why`: nothing was looked at.
    pub(in crate::app) fn refuse(self, why: &str) {
        self.pending.answer(Err(why.into()));
    }

    /// Whether the person has not allowed it yet: a first look, which also tells whether the
    /// repository is one oso knew.
    pub(in crate::app) fn before_question(&self) -> bool {
        matches!(self.stage, Stage::First { .. })
    }

    /// Its connection went: unless the person allowed it already, it ends adding nothing.
    pub(in crate::app) fn abandon(&mut self) {
        if self.before_question() {
            self.abandoned = true;
        }
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
        // One look a caller, so that a mount that does not answer holds one thread only.
        if self.agent_looks_at(caller.key) {
            return pending.answer(Err(
                "oso is still looking at a repository you named; try again".into(),
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
            abandoned: false,
        });
    }

    /// The look at an agent's repository ended with `member`.
    pub(in crate::app) fn agent_repository_checked(
        &mut self,
        look: AddLook,
        member: Result<Member, String>,
        known: bool,
    ) {
        let AddLook {
            pending,
            stage,
            abandoned,
            ..
        } = look;
        if abandoned {
            return;
        }
        let member = match member {
            Ok(member) => member,
            Err(why) => return pending.answer(Err(why)),
        };
        match stage {
            Stage::First { caller, space } => {
                self.ask_to_add(pending, (caller, space), member, known);
            }
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

    /// The first look found repository `member` for orchestrator `caller` of `space`, `known`
    /// when it is one oso knew when the call came, by its device and inode: the person is
    /// asked, or it is added without asking as ADR 0135's rules let it, only for a known
    /// repository oso trusts (Q2, Q3).
    fn ask_to_add(
        &mut self,
        pending: Pending,
        (caller, space): (SessionKey, SpaceId),
        member: Member,
        known: bool,
    ) {
        let caller = match self.still_orchestrates(caller, &space) {
            Ok(caller) => caller,
            Err(why) => return pending.answer(Err(why.into())),
        };
        if let Err(why) = self.may_add(&space, &member) {
            return pending.answer(Err(why));
        }
        let target = shown(&member.root);
        // A refusal holds for the repository, whatever the path that named it.
        let refusal = refusal_key(&member);
        let refused = self.hosting.tools.guard.was_refused(
            &caller.id,
            pending.tool,
            &refusal,
            Instant::now(),
        );
        if refused {
            return pending.answer(Err(Refusal::Person.why()));
        }
        let name = self.space_name(&space);
        let trusted = self.trusts(&member);
        let label = rules::add_repo_question(&caller.name, &target, &name, trusted);
        let said = rules::add_repo_said(&caller.name, &target, &name, true);
        // `?` shows the whole path, which the question may cut.
        let whole = rules::quoted_within(&target, usize::MAX);
        let more = format!("The repository: {whole}\n\n{ADD_DETAILS}");
        let words = Words(label.clone(), details(&caller.name, &label, &more), said);
        let unasked = trusted && known;
        let adding = Adding {
            caller: caller.key,
            caller_name: caller.name.clone(),
            space,
            member,
        };
        let plan = Plan::AddRepo(Box::new(adding));
        self.ask_or_start(pending, &caller, (refusal, unasked), words, plan);
    }

    /// The person's `y`, or an add without asking: without asking it is added now, as the
    /// first look found it; after a `y` it is looked at again first, and must be the same.
    pub(super) fn carry_out_add(&mut self, pending: Pending, adding: Adding, said: Option<String>) {
        if pending.unasked {
            return self.add_for_agent(pending, adding, said);
        }
        let path = adding.member.root.clone();
        let stage = Stage::Again(Box::new(adding));
        self.check_repository_for_agent(AddLook {
            pending,
            path,
            stage,
            abandoned: false,
        });
    }

    /// Adds `adding`'s repository to its space, if its caller is still the space's
    /// orchestrator and the space still takes it; a trusted one is offered to the running
    /// orchestrator now, and the reply says what the agent can do next.
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
        let trusted = self.trusts(&adding.member);
        let now = crate::sessions::spaces::stored::now_ms();
        // `may_add` checked it in this event.
        if self
            .watch
            .spaces_mut()
            .add_member(at, adding.member, now)
            .is_err()
        {
            return pending.answer(Err("your space takes no more repositories".into()));
        }
        let space = self.space_name(&adding.space);
        let path = shown(&root);
        let said =
            said.unwrap_or_else(|| rules::add_repo_said(&adding.caller_name, &path, &space, false));
        self.editor.message(format!("Agent: {said}"));
        self.agent_log(&format!("{}: added", pending.tool));
        let given = self.was_given(&adding.space, &root);
        let offered = trusted && !given && self.offer_added(at, &root);
        let native = (self.live_orchestrator(at)).is_some_and(|key| self.hosting.natives.has(key));
        let add_dir = rules::add_dir(trusted, given, offered, native);
        let name = (self.watch.spaces().member_names(at).into_iter())
            .find(|(_, member)| same(member, &root))
            .map(|(name, _)| name);
        let note = add_dir.note(&path);
        let note = if pending.unasked {
            format!("{note} Added without asking, as the person's config.toml allows.")
        } else {
            note
        };
        self.sessions_touched();
        self.request_redraw();
        let unasked = pending.unasked;
        pending.answer(Ok(json!({
            "repository": { "name": name, "path": path, "trusted": trusted },
            "asked": !unasked,
            "add_dir": add_dir.word(),
            "note": note,
        })));
    }

    /// Whether `space` takes repository `member` now ([`rules::addable`]).
    fn may_add(&self, space: &SpaceId, member: &Member) -> Result<(), String> {
        let spaces = self.watch.spaces();
        let at = spaces.find(space);
        let open = at.is_some_and(|at| spaces.get(at).is_some_and(|space| space.is_open()));
        let stored = at.and_then(|at| spaces.stored(at));
        // A repository named by another path is the same one, by its device and inode.
        let has_it = stored.is_some_and(|stored| {
            stored.has_member(&member.root)
                || (member.identity.is_some()
                    && (stored.members.iter()).any(|was| was.identity == member.identity))
        });
        let members = stored.map_or(0, |stored| stored.members.len());
        rules::addable(open, has_it, members)
    }

    /// The slug of `space`, which its orchestrator answers to.
    fn space_name(&self, space: &SpaceId) -> String {
        let spaces = self.watch.spaces();
        (spaces.find(space))
            .and_then(|at| spaces.stored(at))
            .map_or_else(String::new, |stored| stored.name.clone())
    }
}

/// What a refusal of `member` is kept for: its device and inode where the system has them,
/// so that another path to it is refused too, else its real path.
fn refusal_key(member: &Member) -> String {
    member.identity.map_or_else(
        || shown(&member.root),
        |(device, inode)| format!("{device}:{inode}"),
    )
}
