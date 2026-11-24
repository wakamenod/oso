//! The rules of a session's rename (ADR 0155): who may rename which session, which names are
//! taken, and what the echo area and the reply say of it.
//!
//! Must not depend on: the app or threads; the rules are plain data and functions.

use super::quoted;

/// What a rename knows of the session it would rename and of who asks (ADR 0155).
#[derive(Clone, Copy, Debug, Default)]
pub struct Renamable {
    /// An agent's session asks, not the person.
    pub agent: bool,
    /// It is the calling session.
    pub caller: bool,
    /// It is a space's current orchestrator, whose name is the space's: as the record says,
    /// a closed space's among them.
    pub orchestrator: bool,
    /// It is an orchestrator no more, an earlier one, or a session in a space's own directory.
    pub former: bool,
    /// The caller is the current orchestrator of the space it is in.
    pub of_callers_space: bool,
    /// The caller started it, as the record's parent says.
    pub callers_child: bool,
    /// Its request waits for the person, who reads its name there (ADR 0182).
    pub asks: bool,
}

/// Whether a session may be renamed, or why not (ADR 0155): never a space's current
/// orchestrator, whose name the space's sessions message; for an agent, only itself, the
/// sessions it started, and a current orchestrator's own space's sessions, never an earlier
/// orchestrator, nor one whose request waits for the person (ADR 0182).
///
/// # Errors
///
/// Why it is refused, for the agent or the echo area.
pub fn renamable(session: Renamable) -> Result<(), &'static str> {
    if session.orchestrator {
        return Err(
            "that is its space's orchestrator: its name is the space's, which the space's sessions message; space-rename renames the space",
        );
    }
    if !session.agent {
        return Ok(());
    }
    if session.former {
        return Err("that is an earlier orchestrator of its space; only the person renames it");
    }
    if session.caller || session.of_callers_space || session.callers_child {
        return match session.asks {
            true => {
                Err("that session's request waits for the person; try again once it is answered")
            }
            false => Ok(()),
        };
    }
    Err(
        "a session renames itself and the sessions it started, and a space's orchestrator its space's sessions",
    )
}

/// Why `name` is taken for a rename, or `None` (ADR 0155), ASCII case aside: a name of another
/// session oso knows of, in `sessions`, hosted, remembered, being started or another program's;
/// a space's in `spaces`, which its orchestrator answers to; or an earlier orchestrator's of
/// one, `<space>-was-<n>`.
pub fn rename_clash<'a>(
    name: &str,
    sessions: impl IntoIterator<Item = &'a str>,
    spaces: impl IntoIterator<Item = &'a str> + Clone,
) -> Option<&'static str> {
    use crate::sessions::identity::same_name;
    use crate::sessions::spaces::orchestrator::is_earlier_name;
    if sessions.into_iter().any(|other| same_name(other, name)) {
        return Some("another session has that name");
    }
    if spaces
        .clone()
        .into_iter()
        .any(|space| same_name(space, name))
    {
        return Some("a space has that name, which its orchestrator answers to");
    }
    if spaces.into_iter().any(|space| is_earlier_name(space, name)) {
        return Some("that is how a space names an earlier orchestrator");
    }
    None
}

/// What the echo area says of a rename: by the agent `caller`, or by the person.
pub fn renamed_said(caller: Option<&str>, old: &str, new: &str) -> String {
    match caller {
        Some(caller) => format!(
            "session {} renamed session {} to {}",
            quoted(caller),
            quoted(old),
            quoted(new)
        ),
        None => format!("Session {} is now {}", quoted(old), quoted(new)),
    }
}

/// What a rename adds, if anything (ADR 0155): a session whose `claude` runs still answers to
/// `messaging`, the name Claude Code knows it by, until it is resumed, and `/rename` changes
/// that now; and the `children` sessions it started were told to report to that name.
pub fn rename_note(messaging: &str, new: &str, runs: bool, children: usize) -> Option<String> {
    let mut note = Vec::new();
    if runs && messaging != new {
        note.push(format!(
            "its claude answers to {} until it is resumed; /rename {new} in its terminal changes that now",
            quoted(messaging)
        ));
    }
    match children {
        0 => {}
        1 => note.push(format!(
            "1 session it started reports to {}",
            quoted(messaging)
        )),
        n => note.push(format!(
            "{n} sessions it started report to {}",
            quoted(messaging)
        )),
    }
    (!note.is_empty()).then(|| note.join("; "))
}
