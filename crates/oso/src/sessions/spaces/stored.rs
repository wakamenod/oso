//! What oso keeps of an issue space (ADR 0136): its id, name, title, issue, member
//! repositories, directory, orchestrators and worktrees. Plain data; the `spaces` file
//! (`record`) reads and writes it.
//!
//! Must not depend on: threads, the UI or `app`; the caller reads the random bytes, off the UI
//! thread, and gives them here.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::sessions::identity::SessionId;

/// The longest name of a space, in bytes.
pub const MAX_NAME: usize = 48;
/// The longest title, in bytes.
pub const MAX_TITLE: usize = 200;
/// The longest issue reference, in bytes.
pub const MAX_ISSUE: usize = 200;
/// The most repositories a space holds.
pub const MAX_MEMBERS: usize = 16;
/// The most earlier orchestrators a space remembers.
pub const MAX_EARLIER: usize = 32;
/// The most worktrees a space records.
pub const MAX_WORKTREES: usize = 256;
/// A space's name when nothing is left of what it was made from.
const FALLBACK: &str = "space";

/// A space's id: 16 random bytes, as 32 lower-case hex digits.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpaceId(String);

impl SpaceId {
    /// The id made from `bytes`.
    pub fn from_random(bytes: [u8; 16]) -> Self {
        use fmt::Write as _;
        let mut text = String::with_capacity(32);
        for byte in bytes {
            let _ = write!(text, "{byte:02x}");
        }
        Self(text)
    }

    /// `text` as an id, if it is 32 lower-case hex digits.
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.len() == 32
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        hex.then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SpaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SpaceId({})", self.0)
    }
}

/// A repository of a space: its main checkout's root, and the root's device and inode where the
/// system has them and they were looked at, as `trusted-projects` keys trust (ADR 0114).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub root: PathBuf,
    pub identity: Option<(u64, u64)>,
}

impl Member {
    /// `root`, with no identity looked at yet.
    pub fn at(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            identity: None,
        }
    }

    /// Its name in a space: its root's last component.
    pub fn name(&self) -> String {
        last_component(&self.root)
    }
}

/// What the `spaces` file keeps of one space. No screen and no transcript text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub id: SpaceId,
    /// A slug, as [`slug`] leaves it, unique among the spaces oso keeps.
    pub name: String,
    /// The issue in the person's words, cleaned; at most [`MAX_TITLE`] bytes.
    pub title: String,
    /// The issue's reference, such as `#123`, shown and never fetched.
    pub issue: String,
    pub members: Vec<Member>,
    /// Its own directory, once it has one: a space made from repository spaces gets it at its
    /// first use, from the root in force then.
    pub directory: Option<PathBuf>,
    /// The directory's device and inode when oso made it.
    pub directory_identity: Option<(u64, u64)>,
    pub archived: bool,
    /// The orchestrator's session, while it has one (step 3).
    pub orchestrator: Option<SessionId>,
    /// Earlier orchestrators, the latest last.
    pub earlier: Vec<SessionId>,
    /// The worktrees it made or holds, each in one space only.
    pub worktrees: Vec<PathBuf>,
    /// When it was made and when it last changed, in ms since the epoch.
    pub created: u64,
    pub changed: u64,
}

impl Stored {
    /// A new open space of one repository, `member`, called `name`, as of `now`.
    pub fn of_repository(id: SpaceId, name: String, member: Member, now: u64) -> Self {
        Self {
            id,
            name,
            title: String::new(),
            issue: String::new(),
            members: vec![member],
            directory: None,
            directory_identity: None,
            archived: false,
            orchestrator: None,
            earlier: Vec::new(),
            worktrees: Vec::new(),
            created: now,
            changed: now,
        }
    }

    /// Whether `root` is one of its repositories.
    pub fn has_member(&self, root: &Path) -> bool {
        self.members.iter().any(|member| same(&member.root, root))
    }

    /// Whether it records the worktree at `path`.
    pub fn holds_worktree(&self, path: &Path) -> bool {
        self.worktrees.iter().any(|worktree| same(worktree, path))
    }
}

/// Whether `a` and `b` name one place by their words (ADR 0114's comparison).
pub fn same(a: &Path, b: &Path) -> bool {
    oso_core::project::same_place(a, b)
}

/// `raw` as a space's name: lower-case ASCII letters and digits, each run of anything else one
/// `-`, none at either end, at most [`MAX_NAME`] bytes; `space` if nothing is left. So a name
/// is always a directory's name on its own: never `.`, `..`, a path or an option.
pub fn slug(raw: &str) -> String {
    let mut name = String::with_capacity(raw.len().min(MAX_NAME));
    for c in raw.chars() {
        if name.len() == MAX_NAME {
            break;
        }
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    while name.ends_with('-') {
        name.pop();
    }
    if name.is_empty() {
        FALLBACK.to_owned()
    } else {
        name
    }
}

/// `base`, a slug, or `base-2`, `base-3` and on, the first that `taken` does not hold, within
/// [`MAX_NAME`] bytes.
pub fn unique(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (2u32..)
        .map(|n| {
            let suffix = format!("-{n}");
            let mut keep = MAX_NAME.saturating_sub(suffix.len()).min(base.len());
            while !base.is_char_boundary(keep) {
                keep -= 1;
            }
            format!("{}{suffix}", base[..keep].trim_end_matches('-'))
        })
        .find(|name| !taken(name))
        .unwrap_or_else(|| base.to_owned())
}

/// `path`'s last component, or the whole path when it has none.
pub fn last_component(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Now, in ms since the epoch, as `changed` times are kept.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
