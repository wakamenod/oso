//! Source control (ADR 0127): a repository's status buffer, as magit's, with its sections,
//! files and hunks, staging and unstaging by section, file and hunk (step 1a), committing,
//! with git's output in a log (step 1b), staging lines, refined words, and discarding with
//! copies kept and written back (step 1c: [`cut`], [`discard`], [`keep`], [`discarded`]), and
//! asking the worktree's agent session for a message's draft or a failed commit's fix (step
//! 6a: [`agent`]); and from step 2 the unpushed, unpulled and recent commits ([`history`]),
//! fixup and squash, and pushes, pulls and fetches on [`run`], with what they need that runs no
//! git in [`remote`]; and from step 3a a stopped merge, rebase, pick, revert or `git am`
//! ([`sequence`]), which the head shows and `C-c C-c`, `C-c C-s` and `C-c C-k` go on with;
//! and from step 3b stashes ([`stash`]) and branches ([`branch`]).
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands requests in
//! and takes reports out, and supplies the wake-up.
//!
//! - Each status buffer has its own thread, `oso-vcs` ([`work`]), which runs git only for a
//!   trusted project's root, through `git`'s runner and `oso-platform::process`, its reads
//!   in [`read`] and its writes to the index in [`write`], and lays out the whole text there
//!   ([`layout`]), for the UI to put in at O(1).
//! - What git writes is untrusted: [`status`] and [`patch`] read it as bytes, bounded, and
//!   skip what does not parse.
//! - What a line names is the table's ([`layout::Built`]), never the text's; [`stage`] turns
//!   the lines commands name into git commands.
//! - Files that may hold secrets (ADR 0028) show their name and counts: their hunks are never
//!   read, and they stage as a whole.
//! - A commit runs on a thread of its own for the repository, `oso-vcs-run` ([`run`]), with the
//!   repository's hooks and no time limit, until it ends or the person stops it; what needs no
//!   git, its arguments, its message and the log's lines, is [`commit`]'s.

pub(crate) mod agent;
pub(crate) mod branch;
pub(crate) mod commit;
pub(crate) mod cut;
pub(crate) mod discard;
pub(crate) mod discarded;
pub(crate) mod history;
pub(crate) mod keep;
pub(crate) mod layout;
pub(crate) mod patch;
pub(crate) mod read;
pub(crate) mod remote;
pub(crate) mod run;
pub(crate) mod sequence;
pub(crate) mod stage;
pub(crate) mod stash;
pub(crate) mod status;
pub(crate) mod work;
pub(crate) mod write;

pub(crate) use layout::{Built, View};
pub(crate) use work::{Report, Request, Worker};
