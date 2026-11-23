//! What an orchestrator's or a worker's command line takes beyond any session's, found on the
//! start thread just before it starts (ADR 0136, step 3): the space's directory checked again,
//! its members' `--add-dir`s and its prompt, and a worker's directories to read.
//!
//! Must not depend on: `app` or the UI thread; it runs on the start thread and may read the
//! disk.

use std::path::{Path, PathBuf};

use super::SpaceStart;

/// What an orchestrator's command line takes beyond any session's.
#[derive(Debug, Default)]
pub(super) struct Extra {
    pub(super) add_dirs: Vec<PathBuf>,
    pub(super) system_prompt: Option<String>,
}

/// An orchestrator's directory checked again, just before it starts, and its `--add-dir`s and
/// prompt, with what the echo area says of a repository left out (ADR 0136, step 3). Nothing
/// for another session.
pub(super) fn space_extra(
    space: Option<&SpaceStart>,
    directory: &Path,
    state: Option<&Path>,
) -> Result<(Extra, Vec<String>), String> {
    use crate::sessions::spaces::orchestrator;
    let Some(space) = space else {
        return Ok((Extra::default(), Vec::new()));
    };
    if space.in_space_directory {
        orchestrator::check_before_start(directory, space.made, &space.members)
            .map_err(|why| format!("The orchestrator did not start: {why}"))?;
    }
    let (kept, gone) = orchestrator::still_there(&space.trusted);
    let mut notes: Vec<String> = gone
        .iter()
        .map(|root| {
            format!(
                "{} is no longer the directory oso checked: left out",
                root.display()
            )
        })
        .collect();
    let instructions = match space
        .instructions
        .as_deref()
        .map(orchestrator::read_instructions)
    {
        Some(Ok(text)) => text,
        Some(Err(why)) => {
            notes.push(format!("The space's instructions were not read: {why}"));
            String::new()
        }
        None => String::new(),
    };
    let mut prompt = space.prompt.clone();
    if !gone.is_empty() {
        let roots: Vec<String> = gone.iter().map(|root| root.display().to_string()).collect();
        prompt.push_str(&format!(
            "\nNot added after all, as each is no longer the directory the space recorded: {}\n",
            orchestrator::plain(&roots.join(", "))
        ));
    }
    prompt.push_str(&orchestrator::instructions_section(&instructions));
    // A session in the space's directory that is not its orchestrator gets the checks alone.
    let mut add_dirs: Vec<PathBuf> = kept.into_iter().map(|member| member.root).collect();
    let (reads, left) = reads_checked(&space.reads, state);
    add_dirs.extend(reads);
    notes.extend(left);
    let extra = Extra {
        add_dirs,
        system_prompt: (!prompt.is_empty()).then_some(prompt),
    };
    Ok((extra, notes))
}

/// A worker's directories to read, each checked again in its repository as the tool's job did:
/// those that pass, and what the echo area says of each one left out.
fn reads_checked(
    reads: &[(crate::sessions::worktree::jobs::Trust, PathBuf)],
    state: Option<&Path>,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut kept = Vec::new();
    let mut left = Vec::new();
    for (trust, dir) in reads {
        match crate::sessions::worktree::jobs::readable_in(trust, dir, state) {
            Ok(read) => kept.push(read.root),
            Err(why) => left.push(format!("Not given to read: {why}")),
        }
    }
    (kept, left)
}
