//! `describe-indentation` (ADR 0115): the buffer's indentation setting, and where each of its
//! properties came from, in the echo area.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state.
//!
//! An EditorConfig file is named by its path in the buffer's project, and one outside it, or
//! with no project, is not named: the message shows no path outside the project.

use std::path::Path;

use oso_core::buffer::Buffer;
use oso_core::command::{Args, CommandError, Ctx};
use oso_core::editor::Editor;
use oso_core::editorconfig::Applied;
use oso_core::indent::{Origin, Style};

/// Says the buffer's indentation setting and where each property came from.
pub(super) fn describe_indentation(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let said = described(ctx.editor, ctx.editor.current().1);
    ctx.editor.message(said);
    Ok(())
}

/// What `describe-indentation` says of `buffer`.
fn described(editor: &Editor, buffer: &Buffer) -> String {
    let settings = buffer.indent_settings();
    let origins = buffer.indent_origins();
    let applied = buffer.editorconfig();
    let root = buffer
        .project()
        .and_then(|id| editor.project(id))
        .map(|project| project.root().to_path_buf());
    let from = |origin: Origin, source: Option<usize>| {
        origin_said(origin, applied, source, root.as_deref())
    };
    let properties = applied.map(|applied| &applied.properties);
    let style = match settings.style() {
        Style::Spaces => "spaces",
        Style::Tabs => "tabs",
    };
    format!(
        "{} mode: indent size {} ({}), {style} ({}), tab width {} ({})",
        buffer.mode().name(),
        settings.indent_size(),
        from(
            origins.indent_size,
            properties
                .and_then(|p| p.indent_size())
                .map(|(_, source)| source)
        ),
        from(
            origins.style,
            properties
                .and_then(|p| p.indent_style())
                .map(|(_, source)| source)
        ),
        settings.tab_width().get(),
        from(
            origins.tab_width,
            properties
                .and_then(|p| p.tab_width())
                .map(|(_, source)| source)
        ),
    )
}

/// Where a property came from, in words: for EditorConfig, its file `source` of `applied`,
/// by its path in the project at `root`.
fn origin_said(
    origin: Origin,
    applied: Option<&Applied>,
    source: Option<usize>,
    root: Option<&Path>,
) -> String {
    match origin {
        Origin::EditorConfig => {
            let file = applied
                .zip(source)
                .and_then(|(applied, source)| applied.files.get(source));
            match file
                .zip(root)
                .and_then(|(file, root)| file.strip_prefix(root).ok())
            {
                Some(inside) => format!("EditorConfig {}", inside.display()),
                None => "EditorConfig outside the project".to_owned(),
            }
        }
        Origin::Configuration => "the configuration".to_owned(),
        Origin::Default => "the default".to_owned(),
        Origin::Visited => "as visited; another applies at the next visit".to_owned(),
    }
}
