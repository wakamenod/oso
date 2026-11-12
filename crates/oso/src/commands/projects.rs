//! Commands of the project a buffer's file is in (ADR 0114): trusting it to run its code, and
//! taking that back.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::TrustRefused;
use oso_core::project::ProjectId;

/// How many characters of a root's path a question or a message shows, its end, so that its
/// own words always do.
const ROOT_CELLS: usize = 60;

/// The commands of projects.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "trust-project",
        doc: "Trust the project of the current buffer's file to run its code.\n\nIt asks first, naming the project's root, and trusts it on `y`, if the buffer's project is still the one asked about. A trusted project's code may run through language servers, formatters and git hooks; a restricted one's never does. The record is `trusted-projects` in oso's configuration directory, by the root's path and its device and inode, so a directory made again at the same path is restricted again. A git worktree is trusted while its main repository is.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: trust_project,
    },
    Command {
        name: "untrust-project",
        doc: "Stop trusting the project of the current buffer's file, without asking.\n\nIts line goes from `trusted-projects`, and the project is restricted again. A worktree trusted through its main repository stays trusted until that one is untrusted.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: untrust_project,
    },
];

/// Trusts the current buffer's project once the person said `y` to a question about it, which
/// this command asks and its `y` runs again. Only that `y` confirms (ADR 0072): run by a key,
/// `M-x` or anything else, it asks.
fn trust_project(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let id = current_project(ctx)?;
    let root = shown_root(ctx, id);
    match ctx.editor.trust_project(id, ctx.confirmed()) {
        Ok(()) => {
            ctx.editor.message(format!("Trusted {root}"));
            ctx.request_redraw();
            Ok(())
        }
        Err(TrustRefused::Unconfirmed) => {
            let question = format!(
                "Trust {root}? Its code may run through language servers, formatters and git hooks"
            );
            super::session::ask(ctx, question, "trust-project", ["trust", "keep restricted"])
        }
        Err(refused) => Err(refusal(ctx, &root, refused)),
    }
}

fn untrust_project(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let id = current_project(ctx)?;
    let root = shown_root(ctx, id);
    ctx.editor
        .untrust_project(id)
        .map_err(|refused| refusal(ctx, &root, refused))?;
    if ctx.editor.project_trusted(id) {
        ctx.editor.message(format!(
            "{root} is still trusted, as a worktree of a trusted repository"
        ));
    } else {
        ctx.editor.message(format!("{root} is restricted again"));
    }
    ctx.request_redraw();
    Ok(())
}

/// The project of the current buffer's file.
fn current_project(ctx: &Ctx<'_>) -> Result<ProjectId, CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor
        .current()
        .1
        .project()
        .ok_or_else(|| CommandError::new("Not in a project"))
}

/// The root of `id` as a question or a message shows it.
fn shown_root(ctx: &Ctx<'_>, id: ProjectId) -> String {
    ctx.editor.project(id).map_or_else(String::new, |project| {
        super::shown_place(project.root(), ctx.editor.home(), ROOT_CELLS)
    })
}

/// What a command says when trust did not change.
fn refusal(ctx: &Ctx<'_>, root: &str, refused: TrustRefused) -> CommandError {
    CommandError::new(match refused {
        TrustRefused::NoProject => "Not in a project".to_owned(),
        TrustRefused::Unread => {
            "trusted-projects is not read, so trust cannot change now".to_owned()
        }
        TrustRefused::Already => format!("{root} is trusted already"),
        // The path is not quoted: it cannot be written as a line, so it may hold controls.
        TrustRefused::NotRecordable(why) => format!("This project cannot be trusted: {why}"),
        TrustRefused::NotTrusted => format!("{root} is not trusted"),
        TrustRefused::ThroughMain(main) => {
            let main = super::shown_place(&main, ctx.editor.home(), ROOT_CELLS);
            format!("{root} is trusted as a worktree of {main}; untrust that one")
        }
        // Asked about above.
        TrustRefused::Unconfirmed => format!("{root} was not trusted"),
    })
}
