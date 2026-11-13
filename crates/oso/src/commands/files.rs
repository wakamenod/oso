//! Commands that open a file by its path and write a buffer under a new one (ADR 0087).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys. The host lists directories, loads and saves.

use std::path::PathBuf;

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::SaveRefused;
use oso_core::minibuffer;

/// How many cells of a file's name a question shows, so that its own words always do.
const NAME_CELLS: usize = 40;

/// The commands for files.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "find-file",
        doc: "Visit a file, its path read with completion by segments, from the buffer's directory.\n\nA file open already shows its buffer, and one that does not exist yet opens empty. A file of 2 GiB or more asks first.",
        args: ArgSpec::File {
            prompt: "Find file:",
            history: "file",
        },
        reach: Reach::Person,
        run: find_file,
        state: None,
    },
    Command {
        name: "open-large-file",
        doc: "Visit the file a question asked about, of 2 GiB or more, all of it held in memory.\n\nIt is what `y` runs; a file larger than when asked is asked about again.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: open_large_file,
        state: None,
    },
    Command {
        name: "write-file",
        doc: "Write the buffer to a file, its path read with completion, and visit that file.\n\nA file there already asks first. The old file stays as it was.",
        args: ArgSpec::File {
            prompt: "Write file:",
            history: "file",
        },
        reach: Reach::Person,
        run: write_file,
        state: None,
    },
    Command {
        name: "write-file-over",
        doc: "Write the buffer over the file a question asked about, and visit it.\n\nIt is what `y` runs; it writes only if that file is still the one asked about.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: write_file_over,
        state: None,
    },
];

fn find_file(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let path = PathBuf::from(ctx.answer().unwrap_or_default());
    visit(ctx, path)
}

/// Asks the host to visit `path`, through the loader's verdict, or asks first about a file a
/// look or a load found to be 2 GiB or more (ADR 0087).
pub(super) fn visit(ctx: &mut Ctx<'_>, path: PathBuf) -> Result<(), CommandError> {
    if let Some(bytes) = ctx.editor.large_file(&path) {
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let (cut, more) = minibuffer::cut_to(&name, NAME_CELLS);
        let size = oso_core::load::gib(bytes);
        let question = format!("{cut}{more} is {size}, all held in memory; open it?");
        return super::session::ask(ctx, question, "open-large-file", ["open", "keep"]);
    }
    ctx.editor.request_visit(path, oso_core::load::UNASKED);
    ctx.request_redraw();
    Ok(())
}

fn open_large_file(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let Some((path, bytes)) = ctx.editor.take_large() else {
        return Err(CommandError::new("No file was asked about"));
    };
    if !ctx.confirmed() {
        return Ok(());
    }
    ctx.editor
        .request_visit(path, bytes.max(oso_core::load::LARGE));
    ctx.request_redraw();
    Ok(())
}

fn write_file(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let path = PathBuf::from(ctx.answer().unwrap_or_default());
    write(ctx, path, false)
}

fn write_file_over(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let path = ctx
        .editor
        .write_over_asked()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| CommandError::new("No file was asked about"))?;
    write(ctx, path, ctx.confirmed())
}

/// Writes the selected buffer to `path`, asking before it writes over a file found there.
fn write(ctx: &mut Ctx<'_>, path: PathBuf, over: bool) -> Result<(), CommandError> {
    match ctx.editor.start_write(path.clone(), over) {
        Ok(edited) => {
            if edited {
                ctx.request_redraw();
            }
            Ok(())
        }
        Err(SaveRefused::Exists) => {
            let name = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let (cut, more) = minibuffer::cut_to(&name, NAME_CELLS);
            let question = format!("{cut}{more} exists; write over it?");
            super::session::ask(ctx, question, "write-file-over", ["write over", "keep"])
        }
        Err(SaveRefused::Unmodified) => {
            ctx.editor.message(SaveRefused::Unmodified.to_string());
            Ok(())
        }
        Err(refused) => Err(CommandError::new(refused.to_string())),
    }
}
