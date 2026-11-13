//! Sending to a session (ADR 0121, Q17, step 5): `session-send` pastes the region, or a note,
//! into a session as the person's bracketed paste, not submitted; `session-mention` gives
//! another session's name as `@NAME`; `session-follow-message` shows who sent a session its
//! newest message.
//!
//! Must not depend on: winit, the GPU, `bindings` or `sessions`; a command reads the window's
//! selections and its buffer, and asks the host, which knows each session's directory and
//! state, puts the text together, and refuses a session that needs input.
//!
//! Sending and mentioning are the person's typing into a terminal, `Person`: the agent channel
//! never writes to one (ADR 0120, Q15). Following a message selects the sender's terminal,
//! which then takes the person's keys, so it is `Person` too.

use oso_core::buffer::Buffer;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{SendExcerpt, SendPart, SendTo, SessionRequest};

use super::super::edit::insert_at_cursors;

/// The list `session-send` completes over: the running sessions, the session of this space
/// shown last first.
pub const SEND_LIST: &str = "session-send";

/// The list `session-mention` completes over: the live sessions' names, hosted and external.
pub const MENTION_LIST: &str = "session-mention";

/// The most a send pastes, 256 KiB (ADR 0121): a region is checked from its bounds, before
/// any text is copied, and the host checks the whole text it pastes.
pub const MAX_SEND_BYTES: usize = 256 << 10;

/// What a send of `bytes` past [`MAX_SEND_BYTES`] says.
pub fn too_large(bytes: usize) -> String {
    format!(
        "That is {} KiB; at most {} KiB goes to a session. C-u C-x a m sends the path and \
         lines only",
        bytes.div_ceil(1024),
        MAX_SEND_BYTES >> 10
    )
}

/// The sending commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "session-send",
        doc: "Paste the region into an agent session, read with completion, or with no region, a note read in the minibuffer.\n\nThe region goes as its path relative to the session's directory, its lines and its text in a fence; with `C-u`, as its path and lines only, `path:10-20`, which the agent reads itself (with no region, the line at point, and the note after it). A buffer that may hold secrets sends its path and lines only. The paste is not submitted: the session is shown, you read it in its input box and press Enter. A session that asks for permission or another answer is refused until you answer it. At most 256 KiB.",
        args: ArgSpec::ListedAndNoteUnlessRegion {
            list: SEND_LIST,
            prompt: "Send to session:",
            history: "session",
            note_prompt: "Note:",
            note_history: "session-note",
        },
        reach: Reach::Person,
        run: session_send,
        state: None,
    },
    Command {
        name: "session-mention",
        doc: "Give a live session's name, read with completion, as `@NAME`: inserted at each cursor, or pasted into the session whose terminal this is.\n\nAn agent addresses another session by that name with Claude Code's own `SendMessage`.",
        args: ArgSpec::Listed {
            list: MENTION_LIST,
            prompt: "Mention session:",
            history: "session",
        },
        reach: Reach::Person,
        run: session_mention,
        state: None,
    },
    Command {
        name: "session-follow-message",
        doc: "Show the session that sent the newest message to the session whose terminal is this buffer.\n\nA message is the line Claude Code shows as `Message from @NAME`; its sender is read from the screen, so it is a pointer to a session, not proof of who wrote it.",
        args: ArgSpec::None,
        // It selects the sender's terminal, in front of the person's keys (ADR 0121, Q2).
        reach: Reach::Person,
        run: session_follow_message,
        state: None,
    },
];

fn session_send(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let (session, note) = ctx
        .listed_and_note()
        .filter(|(session, _)| !session.is_empty())
        .ok_or_else(|| CommandError::new("No session"))?;
    let place_only = args.raw().is_some();
    let mut parts = match note {
        // No region showed: the line at point with `C-u`, then the note.
        Some(_) if place_only => lines_at_points(ctx),
        Some(_) => Vec::new(),
        None => regions(ctx, place_only)?,
    };
    if let Some(note) = note.filter(|note| !note.is_empty()) {
        parts.push(SendPart::Note(note.to_owned()));
    }
    if parts.is_empty() {
        return Err(CommandError::new("Nothing to send"));
    }
    ctx.editor.request_session(SessionRequest::Send {
        to: SendTo::Named(session.to_owned()),
        parts,
    });
    Ok(())
}

/// An excerpt for each region the selected window shows, in buffer order: with its text,
/// unless `place_only` or the buffer may hold secrets. Refused past [`MAX_SEND_BYTES`], before any
/// text is copied.
fn regions(ctx: &Ctx<'_>, place_only: bool) -> Result<Vec<SendPart>, CommandError> {
    let (window, buffer) = ctx.editor.current();
    let mut ranges: Vec<_> = window
        .selections
        .iter()
        .map(|selection| selection.range())
        .filter(|range| !range.is_empty())
        .collect();
    ranges.sort_by_key(|range| range.start);
    if ranges.is_empty() {
        return Err(CommandError::new("The region is empty"));
    }
    let with_text = !place_only && !buffer.is_secret();
    if with_text {
        let bytes: usize = ranges.iter().map(std::ops::Range::len).sum();
        if bytes > MAX_SEND_BYTES {
            return Err(CommandError::new(too_large(bytes)));
        }
    }
    Ok(ranges
        .into_iter()
        .map(|range| {
            let text = with_text.then(|| buffer.text().slice(range.clone()).to_string());
            SendPart::Excerpt(excerpt(buffer, range, text))
        })
        .collect())
}

/// The line each cursor is on, as a place only, in buffer order, each line once.
fn lines_at_points(ctx: &Ctx<'_>) -> Vec<SendPart> {
    let (window, buffer) = ctx.editor.current();
    let mut cursors: Vec<usize> = window
        .selections
        .iter()
        .map(|selection| selection.cursor)
        .collect();
    cursors.sort_unstable();
    let mut places: Vec<SendExcerpt> = cursors
        .into_iter()
        .map(|cursor| excerpt(buffer, cursor..cursor, None))
        .collect();
    places.dedup_by_key(|place| place.first);
    places.into_iter().map(SendPart::Excerpt).collect()
}

/// The excerpt of `buffer` that `range` covers: its lines from 1, a region that ends at the
/// start of a line leaving that line out.
fn excerpt(buffer: &Buffer, range: std::ops::Range<usize>, text: Option<String>) -> SendExcerpt {
    let rope = buffer.text();
    let start = rope.offset_to_point(range.start);
    let end = rope.offset_to_point(range.end);
    let last = if range.end > range.start && end.column == 0 {
        end.row.saturating_sub(1).max(start.row)
    } else {
        end.row
    };
    SendExcerpt {
        path: buffer.path().map(std::path::Path::to_path_buf),
        buffer: buffer.name().to_owned(),
        first: start.row + 1,
        last: last + 1,
        text,
    }
}

fn session_mention(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let name = super::answer(ctx, "No session")?;
    if !ctx.editor.current().1.is_terminal() {
        return insert_at_cursors(ctx, &format!("@{name}"), 1);
    }
    // In a terminal: pasted, as the person's typing, into the session it holds; the host
    // refuses a terminal that holds none.
    let id = ctx.editor.window().buffer();
    ctx.editor.request_session(SessionRequest::Send {
        to: SendTo::Buffer(id),
        parts: vec![SendPart::Note(format!("@{name} "))],
    });
    Ok(())
}

fn session_follow_message(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let buffer = ctx.editor.window().buffer();
    ctx.editor
        .request_session(SessionRequest::FollowMessage(buffer));
    Ok(())
}
