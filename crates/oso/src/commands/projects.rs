//! Commands of the project a buffer's file is in (ADR 0114): visiting one of its files or a line
//! found by searching them, trusting it to run its code, and taking that back.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use std::path::PathBuf;

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::TrustRefused;
use oso_core::project::ProjectId;

/// How many characters of a root's path a question or a message shows, its end, so that its
/// own words always do.
const ROOT_CELLS: usize = 60;

/// The commands of projects.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "project-find-file",
        doc: "Visit a file of the current buffer's project, read with completion over its files.\n\nThe files show as paths from the project's root, and the list grows while the project is walked. Hidden files are listed, less what the project's ignore files leave out, as `rg` reads them; nothing inside `.git`, `.jj`, `.hg` or a project inside this one is. Links are not followed, but a link to a file is listed. Outside a project the buffer's directory stands in for one, but not the home directory. With `C-u`, it asks for the directory first. A file that may hold secrets opens as `C-x C-f` opens it.",
        args: ArgSpec::ProjectFile {
            history: "project-file",
        },
        reach: Reach::Person,
        run: project_find_file,
    },
    Command {
        name: "project-search",
        doc: "Search the files of the current buffer's project, line by line, and visit the line picked.\n\nWhat is typed is split at spaces, and a line matches when every part does, in any order: each part as typed or, if it holds a character an expression reads, as that expression too. `=` before a part keeps it as typed, and `!` before one leaves out the lines holding it. Case is ignored unless an upper-case letter is typed. A search starts once a part has two characters, and the rows come while it runs, each `path:line: text`. It searches the files `project-find-file` lists, on disk, and no link. A file that may hold secrets is searched, but its rows show no text. With `C-u`, it asks for the directory first. `RET` visits the line, with the place before on the mark ring.",
        args: ArgSpec::ProjectSearch {
            history: "project-search",
        },
        reach: Reach::Person,
        run: project_search,
    },
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

/// Visits the file the prompt gave, its whole path, through the loader's verdict.
fn project_find_file(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let path = PathBuf::from(ctx.answer().unwrap_or_default());
    super::files::visit(ctx, path)
}

/// Visits the file of the row the search's prompt gave, through the loader's verdict, and goes
/// to its line once it shows.
fn project_search(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let Some(hit) = ctx.editor.take_search_hit() else {
        return Err(CommandError::new("No line was picked"));
    };
    if ctx.editor.large_file(&hit.path).is_some() {
        return super::files::visit(ctx, hit.path);
    }
    ctx.editor
        .request_visit_at(hit.path, oso_core::load::UNASKED, hit.line, hit.column);
    ctx.request_redraw();
    Ok(())
}

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
