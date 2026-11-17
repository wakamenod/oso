//! The list of a review's keys (ADR 0122): `?` in a review buffer shows each key with its
//! command's name and summary, in an internal buffer made once.
//!
//! Must not depend on: the host or its threads; the list is made from the key table and the
//! commands' docstrings.

use oso_core::buffer::Mode;
use oso_core::command::{Args, CommandError, Ctx};

use super::{COMMANDS, COMMENT_COMMANDS, DECISION_COMMANDS, KEYS, SEND_COMMANDS};

/// What the buffer `review-keys` shows is named.
const KEYS_BUFFER: &str = "*Review keys*";

pub(super) fn review_keys(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let text = keys_text();
    let shown = ctx
        .editor
        .buffer_named(KEYS_BUFFER)
        .filter(|id| ctx.editor.buffer(*id).is_some_and(|b| b.is_internal()));
    let id = match shown {
        Some(id) => id,
        None => ctx
            .editor
            .add_internal_buffer(KEYS_BUFFER, Mode::Fundamental),
    };
    ctx.editor.set_internal_text(id, &text);
    let window = ctx.editor.window().id();
    ctx.editor.show_in(window, id);
    ctx.request_redraw();
    Ok(())
}

/// What `*Review keys*` says: each key, and its command's name and summary.
fn keys_text() -> String {
    let mut text = String::from("Keys in a review buffer\n\n");
    for (keys, name) in KEYS {
        let summary = COMMANDS
            .iter()
            .chain(COMMENT_COMMANDS)
            .chain(DECISION_COMMANDS)
            .chain(SEND_COMMANDS)
            .find(|command| command.name == *name)
            .and_then(|command| command.doc.lines().next())
            .unwrap_or_default();
        text.push_str(&format!("{keys:<8}{name:<24}{summary}\n"));
    }
    text.push_str("\nC-x v r opens the review menu; C-x k closes the review.\n");
    text
}
