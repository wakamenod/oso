//! The lists of a review's keys (ADR 0122): `?` in a review buffer and in its files pane shows
//! each key with its command's name and summary, in an internal buffer made once.
//!
//! Must not depend on: the host or its threads; the lists are made from the key tables and the
//! commands' docstrings.

use oso_core::buffer::Mode;
use oso_core::command::{Args, CommandError, Ctx};

use super::{
    COMMANDS, COMMENT_COMMANDS, DECISION_COMMANDS, FILES_COMMANDS, KEYS, SEND_COMMANDS,
    VISIT_COMMANDS,
};

/// What the buffer `review-keys` shows is named.
const KEYS_BUFFER: &str = "*Review keys*";

pub(super) fn review_keys(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let text = keys_text(
        "Keys in a review buffer",
        KEYS,
        "C-x v r opens the review menu; C-x k closes the review.",
    );
    show_keys(ctx, KEYS_BUFFER, &text)
}

/// Shows `text` in the internal buffer `name`, made once, in the selected window.
pub(super) fn show_keys(ctx: &mut Ctx<'_>, name: &str, text: &str) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let shown = ctx
        .editor
        .buffer_named(name)
        .filter(|id| ctx.editor.buffer(*id).is_some_and(|b| b.is_internal()));
    let id = match shown {
        Some(id) => id,
        None => ctx.editor.add_internal_buffer(name, Mode::Fundamental),
    };
    ctx.editor.set_internal_text(id, text);
    let window = ctx.editor.window().id();
    ctx.editor.show_in(window, id);
    ctx.request_redraw();
    Ok(())
}

/// What a list of a review's keys says: `title`, each of `keys` with its command's name and
/// summary, and `footer`.
pub(super) fn keys_text(title: &str, keys: &[(&str, &str)], footer: &str) -> String {
    let mut text = format!("{title}\n\n");
    for (keys, name) in keys {
        let summary = COMMANDS
            .iter()
            .chain(COMMENT_COMMANDS)
            .chain(DECISION_COMMANDS)
            .chain(SEND_COMMANDS)
            .chain(FILES_COMMANDS)
            .chain(VISIT_COMMANDS)
            .find(|command| command.name == *name)
            .and_then(|command| command.doc.lines().next())
            .unwrap_or_default();
        text.push_str(&format!("{keys:<8}{name:<24}{summary}\n"));
    }
    text.push_str(&format!("\n{footer}\n"));
    text
}
