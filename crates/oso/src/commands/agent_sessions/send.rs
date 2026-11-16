//! Sending to a session (ADR 0121, Q17, step 5): `session-send` pastes the region, or a note,
//! into a session as the person's bracketed paste, not submitted; `session-mention` gives
//! another session's name as `@NAME`; `session-follow-message` shows who sent a session its
//! newest message; `session-yank-image` keeps the clipboard's image in a file and pastes an `@`
//! reference to it into the session (ADR 0142); `session-send-diagnostics` pastes a language
//! server's diagnostics of the buffer or the region, and `session-fix-error` the one at point
//! with its lines around and the ask to fix it (ADR 0138, step 11).
//!
//! Must not depend on: winit, the GPU, `bindings` or `sessions`; a command reads the window's
//! selections and its buffer, and asks the host, which knows each session's directory and
//! state, puts the text together, and refuses a session that needs input.
//!
//! Sending and mentioning are the person's typing into a terminal, `Person`: the agent channel
//! never writes to one (ADR 0120, Q15). Following a message selects the sender's terminal,
//! which then takes the person's keys, so it is `Person` too. Yanking an image reads the
//! clipboard and may start `sips`: `Person`.

use oso_core::buffer::Buffer;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::diagnostic::Severity;
use oso_core::editor::{SendDiagnostic, SendExcerpt, SendPart, SendTo, SessionRequest};

use super::super::edit::insert_at_cursors;

/// The list `session-send` completes over: the running sessions, the session of this space
/// shown last first.
pub const SEND_LIST: &str = "session-send";

/// The list `session-mention` completes over: the live sessions' names, hosted and external.
pub const MENTION_LIST: &str = "session-mention";

/// The most a send pastes, 256 KiB (ADR 0121): a region is checked from its bounds, before
/// any text is copied, and the host checks the whole text it pastes.
pub const MAX_SEND_BYTES: usize = 256 << 10;

/// What a send of `bytes` past [`MAX_SEND_BYTES`] says, with `send` the keys of `session-send`
/// (ADR 0143).
pub fn too_large(bytes: usize, send: &str) -> String {
    format!(
        "That is {} KiB; at most {} KiB goes to a session. C-u {send} sends the path and \
         lines only",
        bytes.div_ceil(1024),
        MAX_SEND_BYTES >> 10
    )
}

/// The name of the command that yanks the clipboard's image into a session, whose origin the
/// dispatcher notes (ADR 0142).
pub const YANK_IMAGE: &str = "session-yank-image";

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
        name: "session-send-diagnostics",
        doc: "Paste the language server's diagnostics of this buffer, or of the region, into an agent session, read with completion.\n\nEach goes as a line, `path:line: severity: message`, the path relative to the session's directory and the message's first line; hints are left out. The paste is not submitted: you read it in the session's input box and press Enter. At most 256 KiB.",
        args: ArgSpec::Listed {
            list: SEND_LIST,
            prompt: "Send diagnostics to session:",
            history: "session",
        },
        reach: Reach::Person,
        run: session_send_diagnostics,
        state: None,
    },
    Command {
        name: "session-fix-error",
        doc: "Ask an agent session, read with completion, to fix the diagnostic at the primary cursor.\n\nIt pastes the diagnostic, `path:line:column: severity`, the server's message in a fence under a line saying a language server wrote it, the 10 lines either side of it in a fence, and \"Fix this error.\". Of several there, the most severe goes; hints never do. The paste is not submitted: you read it in the session's input box and press Enter.",
        args: ArgSpec::Listed {
            list: SEND_LIST,
            prompt: "Fix the error with session:",
            history: "session",
        },
        reach: Reach::Person,
        run: session_fix_error,
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
    Command {
        name: YANK_IMAGE,
        doc: "Keep the image on the clipboard in a file and paste an `@` reference to it into the session whose terminal is this buffer.\n\nThe reference is the file's absolute path, which Claude Code reads when you press Enter: the image goes by path, never inline. A PNG, JPEG, GIF or WebP is kept as it came; a TIFF alone, as some macOS apps copy, is converted to PNG with `sips`. The files are private to you, under the cache directory's `oso/images`, and go when the session is archived, or after a week. As with `session-send`, a session that asks for permission or another answer is refused until you answer it. macOS only for now.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: session_yank_image,
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
            // A command cannot look up keys: it names itself, as where-is does a command bound
            // nowhere (ADR 0143).
            return Err(CommandError::new(too_large(bytes, "M-x session-send")));
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

/// The lines either side of a diagnostic `session-fix-error` sends.
const AROUND: usize = 10;

/// Sends the buffer's diagnostics, or the region's, hints left out (ADR 0138, step 11).
fn session_send_diagnostics(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let session = super::answer(ctx, "No session")?;
    let (window, buffer) = ctx.editor.current();
    let set = diagnosed(buffer)?;
    let mut regions: Vec<_> = window
        .selections
        .iter()
        .map(|selection| selection.range())
        .filter(|range| !range.is_empty())
        .collect();
    regions.sort_unstable_by_key(|range| range.start);
    // A diagnostic in a region, found by a binary search of the regions, which meet no other.
    let meets = |range: std::ops::Range<usize>| {
        let after = regions.partition_point(|region| region.end <= range.start);
        regions.is_empty()
            || regions
                .get(after)
                .is_some_and(|region| region.start <= range.start)
    };
    let mut diagnostics: Vec<SendDiagnostic> = Vec::new();
    let mut bytes = 0;
    for diagnostic in (set.iter())
        .filter(|diagnostic| diagnostic.severity() != Severity::Hint)
        .filter(|diagnostic| meets(diagnostic.range()))
    {
        // Its first line only, as the line sends, and no column, which it shows none of.
        let mut sent = send_diagnostic(buffer, &diagnostic, false);
        sent.message = oso_core::diagnostic::first_line(&sent.message).to_owned();
        bytes += sent.message.len() + sent.buffer.len() + 32;
        if bytes > MAX_SEND_BYTES {
            return Err(CommandError::new(too_large(
                bytes,
                "M-x session-send-diagnostics",
            )));
        }
        diagnostics.push(sent);
    }
    if diagnostics.is_empty() {
        return Err(CommandError::new(if regions.is_empty() {
            "No diagnostics to send"
        } else {
            "No diagnostics in the region"
        }));
    }
    ctx.editor.request_session(SessionRequest::Send {
        to: SendTo::Named(session),
        parts: vec![SendPart::Diagnostics(diagnostics)],
    });
    Ok(())
}

/// Sends the most severe diagnostic at the primary cursor, its lines around and "Fix this error." (ADR 0121,
/// ADR 0138, step 11).
fn session_fix_error(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let session = super::answer(ctx, "No session")?;
    let (window, buffer) = ctx.editor.current();
    let set = diagnosed(buffer)?;
    let cursor = window.selections.primary().cursor;
    let Some(diagnostic) = set
        .at(cursor)
        .filter(|diagnostic| diagnostic.severity() != Severity::Hint)
        .min_by_key(|diagnostic| diagnostic.severity())
    else {
        return Err(CommandError::new("No diagnostic at point"));
    };
    let text = buffer.text();
    let row = text.offset_to_point(diagnostic.range().start).row;
    let first = row.saturating_sub(AROUND);
    let last = (row + AROUND).min(text.line_count().saturating_sub(1));
    let range = text.line_start(first)..text.line_end(last);
    if range.len() > MAX_SEND_BYTES {
        return Err(CommandError::new(too_large(
            range.len(),
            "M-x session-fix-error",
        )));
    }
    let lines = text.slice(range.clone()).to_string();
    let around = excerpt(buffer, range, Some(lines));
    ctx.editor.request_session(SessionRequest::Send {
        to: SendTo::Named(session),
        parts: vec![SendPart::FixError {
            diagnostic: send_diagnostic(buffer, &diagnostic, true),
            around,
        }],
    });
    Ok(())
}

/// The set of diagnostics of `buffer`, or why there is none to send.
fn diagnosed(buffer: &Buffer) -> Result<&oso_core::diagnostic::Diagnostics, CommandError> {
    if buffer.is_secret() {
        return Err(CommandError::new(
            "No server for a buffer that may hold secrets",
        ));
    }
    buffer
        .diagnostics()
        .ok_or_else(|| CommandError::new("No language server reported on this buffer"))
}

/// `diagnostic` of `buffer` as a send gives it, its message cut to what a send keeps, and its
/// column in characters if `column`, else 0.
fn send_diagnostic(
    buffer: &Buffer,
    diagnostic: &oso_core::diagnostic::Diagnostic<'_>,
    column: bool,
) -> SendDiagnostic {
    let text = buffer.text();
    let start = diagnostic.range().start;
    let point = text.offset_to_point(start);
    let column = column.then(|| text.slice(text.line_start(point.row)..start).char_count() + 1);
    let message = diagnostic.message();
    let kept = &message[..message.floor_char_boundary(crate::channel::diagnostics::MESSAGE_BYTES)];
    SendDiagnostic {
        path: buffer.path().map(std::path::Path::to_path_buf),
        buffer: buffer.name().to_owned(),
        line: point.row + 1,
        column: column.unwrap_or(0),
        severity: diagnostic.severity(),
        message: kept.to_owned(),
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

fn session_yank_image(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = super::mode::session_buffer(ctx)?;
    ctx.editor
        .request_session(SessionRequest::YankImage(buffer));
    Ok(())
}
