//! Searching past agent conversations (ADR 0145): `session-search` goes to the session of the
//! row its prompt gave, and `session-search-scope` moves that prompt to its next scope.
//!
//! Must not depend on: winit, the GPU or `bindings`; the dispatcher opens the prompt, the host's
//! history thread searches, and the host shows or adopts the session.
//!
//! Both are `Person`: their rows are every project's conversations, which no agent gets, and
//! going to a stopped session resumes a process.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::SessionRequest;

/// The history of `session-search`'s inputs.
pub const SEARCH_HISTORY: &str = "session-search";

/// The commands of the search of conversations.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "session-search",
        doc: "Search what was said in Claude Code's past conversations, and go to the session of the row picked.

Every conversation on this machine is searched, of every project; `C-c C-s` in the prompt moves to the space in front's, then the current project's, then back; `C-u` starts in the next. A row is one prompt (`›`) or reply (`‹`) that holds every word of the input, in any order, with its session's mark (`●` running here, `○` stopped here, `⊘` running in another program, `◌` not oso's), name, place and age; the preview shows the conversation around it. Tool inputs and results, thinking and attachments are never searched. `RET` shows a session oso hosts; another is adopted as `session-adopt` adopts it, resumed in the directory it ran in. A conversation whose directory is gone is not resumed.",
        args: ArgSpec::ConversationSearch {
            history: SEARCH_HISTORY,
        },
        reach: Reach::Person,
        run: session_search,
        state: None,
    },
    Command {
        name: "session-search-scope",
        doc: "In `session-search`'s prompt, search the next scope: every conversation, the space in front's, then the current project's.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: session_search_scope,
        state: None,
    },
];

/// Goes to the session of the row the prompt gave.
fn session_search(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let Some(hit) = ctx.editor.take_conversation_hit() else {
        return Err(CommandError::new("No conversation was picked"));
    };
    ctx.editor
        .request_session(SessionRequest::Found(hit.session.into()));
    ctx.request_redraw();
    Ok(())
}

/// Moves the search's prompt to its next scope.
fn session_search_scope(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.editor.next_conversation_scope() {
        return Err(CommandError::new("Not searching conversations"));
    }
    ctx.request_redraw();
    Ok(())
}
