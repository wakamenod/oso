//! A native session's request's line, as keys (ADR 0182): `y` allows the call once all of it was
//! read, `C-u y` asks to allow it unread, `n` and `C-c C-k` deny it, `N` with a note, `RET` shows
//! its input, `SPC`, `DEL`, `C-v` and `M-v` page the transcript and `M-<` and `M->` go to its block's
//! ends, and `C-c C-o` and `C-g` leave it for the transcript. `C-c C-j`, in the transcript and the
//! composer, goes to the request. Each asks the host, which holds the call and answers its bridge.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! Every key is `Person`: what a line answers lets a call of a session run, which no agent may do,
//! and what it shows is what the person read of it.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::NativeAction;

use super::native::at_point;

/// The commands of a native session's request's line.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "native-request-allow",
        doc: "Allow the call a native session's request asks about, once, if all of it was read.\n\nElse show what was not. With a prefix argument, allow it unread, after `y` to a question that says what was not read.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: allow,
        state: None,
    },
    Command {
        name: "native-request-deny",
        doc: "Deny the call a native session's request asks about.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: deny,
        state: None,
    },
    Command {
        name: "native-request-note",
        doc: "Deny the call a native session's request asks about with a note, read in the minibuffer, which the session's claude is told.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: note,
        state: None,
    },
    Command {
        name: "native-request-input",
        doc: "Show the whole input of the call a native session's request asks about, read-only, as JSON in `*result: NAME*`.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: input,
        state: None,
    },
    Command {
        name: "native-request-read-on",
        doc: "Show the next page of the transcript above a native session's request's line, times the count; at its end, what of the request was not read.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: read_on,
        state: None,
    },
    Command {
        name: "native-request-read-back",
        doc: "Show the page before of the transcript above a native session's request's line, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: read_back,
        state: None,
    },
    Command {
        name: "native-request-first",
        doc: "Show the start of a native session's request in its transcript.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: first,
        state: None,
    },
    Command {
        name: "native-request-last",
        doc: "Show the end of a native session's request in its transcript.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: last,
        state: None,
    },
    Command {
        name: "native-request-leave",
        doc: "Leave a native session's request's line for its transcript: the request waits on.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: leave,
        state: None,
    },
    Command {
        name: "native-session-request",
        doc: "Go to what waits for the person of the native session whose transcript or composer this is: its request's line, or its card.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: request,
        state: None,
    },
];

fn allow(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let action = match args.raw() {
        Some(_) => NativeAction::Unread,
        None => NativeAction::Allow,
    };
    at_point(ctx, action)
}

fn deny(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Deny)
}

fn note(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Note)
}

fn input(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Open)
}

fn read_on(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::ReadOn(pages(args.count())))
}

fn read_back(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    at_point(
        ctx,
        NativeAction::ReadOn(pages(args.count().saturating_neg())),
    )
}

/// `count` pages, the ends' values aside.
fn pages(count: isize) -> isize {
    count.clamp(isize::MIN + 1, isize::MAX - 1)
}

fn first(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::ReadOn(isize::MIN))
}

fn last(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::ReadOn(isize::MAX))
}

fn leave(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Transcript)
}

fn request(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Request)
}
