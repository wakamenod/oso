//! Searching past agent conversations (ADR 0145): `session-search` goes to the session of the
//! row its prompt gave, `session-search-scope` moves that prompt to its next scope, and
//! `C-RET` and `M-RET` there read the row's conversation or fork it (step 2). A read
//! conversation, `*conversation: NAME*`, has keys of its own: its messages, its session, a
//! fork and a new read.
//!
//! Must not depend on: winit, the GPU or `bindings`; the dispatcher opens the prompt, the host's
//! history thread searches, and the host shows or adopts the session.
//!
//! They are `Person`: their rows and views are every project's conversations, which no agent
//! gets, and going to a stopped session, or forking one, starts a process. Moving between a
//! view's messages is `Look`: the view is withheld from agents anyway.

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::SessionRequest;
use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;

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
        name: "session-search-read",
        doc: "In `session-search`'s prompt, read the selected row's conversation in `*conversation: NAME*`, read-only, at its message.

The whole transcript is read again, prompts and replies only, up to 8 MiB of them. The buffer is never saved and agents never read it. In it, `n` and `p` go to the next and previous message, `r` goes to the session as `RET` in the search does, `f` forks it, `g` reads it again and `q` quits.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: session_search_read,
        state: None,
    },
    Command {
        name: "session-search-fork",
        doc: "In `session-search`'s prompt, fork the selected row's conversation: resume a copy of it under a new id, in a new hosted session.

It starts in the directory the conversation's transcript is found from, as `session-fork` forks a hosted one; one whose directory is gone, or that oso cannot tell, is not forked. A session that runs elsewhere may be forked: the copy goes on apart from it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: session_search_fork,
        state: None,
    },
    Command {
        name: "conversation-next-message",
        doc: "In `*conversation: NAME*`, go to the next prompt or reply.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: conversation_next_message,
        state: None,
    },
    Command {
        name: "conversation-previous-message",
        doc: "In `*conversation: NAME*`, go to the previous prompt or reply.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: conversation_previous_message,
        state: None,
    },
    Command {
        name: "conversation-go",
        doc: "In `*conversation: NAME*`, go to its session: show it if oso hosts it, else adopt it, as `RET` in `session-search` does.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: conversation_go,
        state: None,
    },
    Command {
        name: "conversation-fork",
        doc: "In `*conversation: NAME*`, fork its conversation, as `M-RET` in `session-search` does.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: conversation_fork,
        state: None,
    },
    Command {
        name: "conversation-revert",
        doc: "In `*conversation: NAME*`, read its transcript again, at the message point is in.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: conversation_revert,
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
        .request_session(SessionRequest::GoToConversation {
            session: hit.session.into(),
            message: hit.message,
        });
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

/// What starts the view's line that names its session.
const SESSION_LINE: &str = "session ";

/// The most lines a view's header takes, where its session is named.
const HEADER_LINES: usize = 5;

/// The first `bytes` of the line `row` of `text`, or fewer, cut at a character.
fn line_head(text: &Rope, row: usize, bytes: usize) -> String {
    let start = text.line_start(row);
    let end = text.clip_offset((start + bytes).min(text.line_end(row)), Bias::Left);
    text.chunks_in_range(start..end).collect()
}

/// The offsets of the first lines of messages in `range` of `text`, each handed to `found`
/// until it says to stop: a line break, then a prompt's or a reply's mark (`› `, `‹ `), which
/// starts a message's first line only, as its other lines are indented. The text's chunks are read as bytes, once,
/// with nothing allocated, so a key costs at most one pass over what lies between point and
/// the message it goes to (ADR 0145, step 2).
fn message_starts(
    text: &Rope,
    range: std::ops::Range<usize>,
    mut found: impl FnMut(usize) -> bool,
) {
    // The bytes of the pattern matched so far: a line break, then a mark's three bytes and a
    // space; the mark's last byte is one of two.
    let mut matched = 0;
    let mut at = range.start;
    for chunk in text.chunks_in_range(range) {
        for &byte in chunk.as_bytes() {
            matched = match (matched, byte) {
                (_, b'\n') => 1,
                (1, 0xE2) => 2,
                (2, 0x80) => 3,
                (3, 0xB9 | 0xBA) => 4,
                (4, b' ') => {
                    // The line starts at the mark, four bytes back.
                    if !found(at - 3) {
                        return;
                    }
                    0
                }
                _ => 0,
            };
            at += 1;
        }
    }
}

/// Reads the selected row's conversation into its view, closing the prompt.
fn session_search_read(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let Some(hit) = ctx.editor.close_conversation_search() else {
        return Err(CommandError::new("No conversation is selected"));
    };
    ctx.editor
        .request_session(SessionRequest::ReadConversation {
            session: hit.session.into(),
            message: hit.message,
        });
    ctx.request_redraw();
    Ok(())
}

/// Forks the selected row's conversation, closing the prompt.
fn session_search_fork(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let Some(hit) = ctx.editor.close_conversation_search() else {
        return Err(CommandError::new("No conversation is selected"));
    };
    ctx.editor
        .request_session(SessionRequest::ForkConversation {
            session: hit.session.into(),
        });
    ctx.request_redraw();
    Ok(())
}

/// The current buffer's view, if it is one: its session's id, from its header's line that
/// names it, which oso wrote, and the index of the message point is in, counted by marks.
fn view(ctx: &Ctx<'_>) -> Result<(String, usize), CommandError> {
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::Conversation {
        return Err(CommandError::new("This buffer is no conversation"));
    }
    let text = buffer.text();
    let point = window.selections.primary().cursor;
    let row = text.offset_to_point(point).row;
    // The header's few lines name the session, before any message.
    let session = (0..text.line_count().min(HEADER_LINES))
        .find_map(|number| {
            let line = line_head(text, number, SESSION_LINE.len() + 40);
            let id = line.strip_prefix(SESSION_LINE)?;
            id.split(' ').next().map(str::to_owned)
        })
        .ok_or_else(|| CommandError::new("This conversation names no session"))?;
    // The messages whose first line starts at or before point's line.
    let line_end = text.line_end(row);
    let mut starts = 0usize;
    message_starts(text, 0..line_end, |_| {
        starts += 1;
        true
    });
    Ok((session, starts.saturating_sub(1)))
}

/// Moves point `count` messages on, back if negative, to a message's first line.
fn move_by_messages(ctx: &mut Ctx<'_>, count: isize) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::Conversation {
        return Err(CommandError::new("This buffer is no conversation"));
    }
    let text = buffer.text();
    let point = window.selections.primary().cursor;
    let row = text.offset_to_point(point).row;
    let wanted = count.unsigned_abs().max(1);
    let target = if count >= 0 {
        // From the end of point's line on, the `wanted`th.
        let mut left = wanted;
        let mut target = None;
        message_starts(text, text.line_end(row)..text.len(), |start| {
            left -= 1;
            if left == 0 {
                target = Some(start);
            }
            left > 0
        });
        target
    } else {
        // Up to point's line, the `wanted`th from the last; a ring of them is kept.
        let mut ring = std::collections::VecDeque::with_capacity(wanted);
        let before = text.line_start(row).saturating_sub(1);
        message_starts(text, 0..before, |start| {
            if ring.len() == wanted {
                ring.pop_front();
            }
            ring.push_back(start);
            true
        });
        (ring.len() == wanted)
            .then(|| ring.front().copied())
            .flatten()
    };
    let Some(offset) = target else {
        return Err(CommandError::new(if count >= 0 {
            "No later message"
        } else {
            "No earlier message"
        }));
    };
    let selections = &mut ctx.editor.current_mut().0.selections;
    selections.keep_primary();
    selections.move_primary(offset);
    ctx.request_redraw();
    Ok(())
}

fn conversation_next_message(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by_messages(ctx, args.count())
}

fn conversation_previous_message(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by_messages(ctx, -args.count())
}

fn conversation_go(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (session, message) = view(ctx)?;
    ctx.editor
        .request_session(SessionRequest::GoToConversation { session, message });
    Ok(())
}

fn conversation_fork(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (session, _) = view(ctx)?;
    ctx.editor
        .request_session(SessionRequest::ForkConversation { session });
    Ok(())
}

fn conversation_revert(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (session, message) = view(ctx)?;
    ctx.editor
        .request_session(SessionRequest::ReadConversation { session, message });
    Ok(())
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Mode;
    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::{ConversationRow, ConversationScope, Editor, SessionRequest};

    const SESSION: &str = "6c1b5d1e-2f3a-4b5c-8d9e-0f1a2b3c4d5e";

    fn run(editor: &mut Editor, name: &str, args: Args) -> Result<(), String> {
        let registry = crate::commands::registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::invoked(editor, Invocation::default());
        registry
            .execute(id, &mut ctx, args)
            .map_err(|error| error.to_string())
    }

    /// An editor showing a view: its header, then three messages, the second of two lines.
    fn viewing() -> Editor {
        let mut editor = Editor::default();
        let id = editor.add_internal_buffer("*conversation: x*", Mode::Conversation);
        let text = format!(
            "in /w/oso\nsession {SESSION} · 2m ago · 3 messages\nn and p: …\n\n› one\n\n‹ two\n  › not a mark\n\n› three"
        );
        editor.set_internal_text(id, &text);
        editor.show(id);
        editor
    }

    fn row(editor: &Editor) -> usize {
        let (window, buffer) = editor.current();
        buffer
            .text()
            .offset_to_point(window.selections.primary().cursor)
            .row
    }

    #[test]
    fn n_and_p_go_from_message_to_message_by_their_marks() {
        let mut editor = viewing();
        run(&mut editor, "conversation-next-message", Args::NONE).unwrap();
        assert_eq!(row(&editor), 4);
        run(&mut editor, "conversation-next-message", Args::numeric(2)).unwrap();
        assert_eq!(row(&editor), 9, "an indented line is no message");
        let none = run(&mut editor, "conversation-next-message", Args::NONE);
        assert!(none.unwrap_err().ends_with("No later message"));
        run(&mut editor, "conversation-previous-message", Args::NONE).unwrap();
        assert_eq!(row(&editor), 6);
    }

    #[test]
    fn the_views_keys_name_its_session_and_the_message_point_is_in() {
        let mut editor = viewing();
        run(&mut editor, "conversation-next-message", Args::numeric(2)).unwrap();
        run(&mut editor, "conversation-revert", Args::NONE).unwrap();
        run(&mut editor, "conversation-fork", Args::NONE).unwrap();
        run(&mut editor, "conversation-go", Args::NONE).unwrap();
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::ReadConversation {
                    session: SESSION.into(),
                    message: 1,
                },
                SessionRequest::ForkConversation {
                    session: SESSION.into(),
                },
                SessionRequest::GoToConversation {
                    session: SESSION.into(),
                    message: 1,
                },
            ]
        );
        let mut elsewhere = Editor::default();
        let refused = run(&mut elsewhere, "conversation-go", Args::NONE);
        assert!(
            refused
                .unwrap_err()
                .ends_with("This buffer is no conversation")
        );
    }

    #[test]
    fn c_ret_and_m_ret_close_the_search_and_ask_for_the_selected_row() {
        for (name, forks) in [
            ("session-search-read", false),
            ("session-search-fork", true),
        ] {
            let mut editor = Editor::default();
            editor
                .open_conversation_search(vec![ConversationScope::all()], 0, "session-search")
                .unwrap();
            let refused = run(&mut editor, name, Args::NONE);
            assert!(
                refused
                    .unwrap_err()
                    .ends_with("No conversation is selected")
            );
            let prompt = editor.prompt().unwrap().buffer();
            editor
                .apply_edits(
                    prompt,
                    &[oso_core::buffer::Replacement {
                        range: 0..0,
                        text: "needle",
                    }],
                )
                .unwrap();
            editor.match_completion();
            let request = editor.take_conversation_search_request().unwrap();
            let row = ConversationRow {
                session: SESSION.into(),
                label: "◌ x · y · 1m ›".into(),
                message: 4,
                line: 0,
                column: 0,
                text: "needle".into(),
            };
            editor.conversation_rows_found(request.generation, vec![row], None);
            run(&mut editor, name, Args::NONE).unwrap();
            assert!(editor.prompt().is_none(), "the prompt closes");
            let asked = if forks {
                SessionRequest::ForkConversation {
                    session: SESSION.into(),
                }
            } else {
                SessionRequest::ReadConversation {
                    session: SESSION.into(),
                    message: 4,
                }
            };
            assert_eq!(editor.take_session_requests(), [asked]);
        }
    }
}
