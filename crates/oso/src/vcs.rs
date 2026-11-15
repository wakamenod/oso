//! Source control (ADR 0127): a repository's status buffer, as magit's, with its sections,
//! files and hunks, and staging and unstaging by section, file and hunk. Step 1a of the ADR.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands requests in
//! and takes reports out, and supplies the wake-up.
//!
//! - Each status buffer has its own thread, `oso-vcs` ([`work`]), which runs git only for a
//!   trusted project's root, through `git`'s runner and `oso-platform::process`, and lays
//!   out the whole text there ([`layout`]), for the UI to put in at O(1).
//! - What git writes is untrusted: [`status`] and [`patch`] read it as bytes, bounded, and
//!   skip what does not parse.
//! - What a line names is the table's ([`layout::Built`]), never the text's; [`stage`] turns
//!   the lines commands name into git commands.
//! - Files that may hold secrets (ADR 0028) show their name and counts: their hunks are never
//!   read, and they stage as a whole.

pub(crate) mod layout;
pub(crate) mod patch;
pub(crate) mod stage;
pub(crate) mod status;
pub(crate) mod work;

pub(crate) use layout::{Built, View};
pub(crate) use work::{Report, Request, Worker};
