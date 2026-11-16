//! The text of a `session-search` row's preview (ADR 0145): a line naming the whole directory
//! the conversation resumes in, then the messages around the row's, each starting with `› ` for
//! a prompt or `‹ ` for a reply; and the place a row names for a directory.
//!
//! Must not depend on: files, threads or the UI; plain functions over what is held.
//!
//! - Before the row's match, at most half the preview's cells of text are kept, counted in
//!   characters: the last lines of the messages before it, the lines of its message before its
//!   line, and of its line, the characters before the match. So a long line or message never
//!   pushes the match out of the excerpt.

use std::path::Path;

use oso_core::editor::ConversationPreviewRequest;

use super::held::Whole;
use super::{About, Message};
use crate::sessions::registry::shown_directory;

/// How many messages before the row's the preview shows at most.
const BEFORE: usize = 3;

/// How many messages after the row's the preview shows at most.
const AFTER: usize = 4;

/// What starts a message's first line: a prompt's mark, or a reply's.
const PROMPT: &str = "› ";
const REPLY: &str = "‹ ";

/// What starts its other lines, as wide as a mark.
const INDENT: &str = "  ";

/// What stands for the text left out before the match.
const CUT: &str = "…";

/// The preview's text for the row of `request` in `about`'s `messages`, and the line, from 0,
/// and column, in characters, its match is at in it.
pub(super) fn text(
    about: &About,
    messages: &[Message],
    request: &ConversationPreviewRequest,
) -> (String, usize, usize) {
    let mut out = Out::default();
    let directory = about
        .shown_directory()
        .map_or_else(|| "?".to_owned(), shown_directory);
    let gone = if about.gone { " (gone)" } else { "" };
    out.line(&format!("in {directory}{gone}"));
    let hit = request.message.min(messages.len().saturating_sub(1));
    let first = hit.saturating_sub(BEFORE);
    let last = (hit + AFTER + 1).min(messages.len());
    // The characters of text kept before the match, half the preview's cells.
    let mut room = (request.rows.max(4) / 2) * request.columns.max(8);
    let before = messages.get(first..hit).unwrap_or_default();
    // The messages before the row's, newest first, take what room is left, line by line.
    let mut kept: Vec<(&Message, usize)> = Vec::new();
    for message in before.iter().rev() {
        let lines: Vec<&str> = message.text.split('\n').collect();
        let skip = skip_for(&lines, &mut room);
        kept.push((message, skip));
        if room == 0 {
            break;
        }
    }
    for (message, skip) in kept.into_iter().rev() {
        out.gap();
        out.message(message, skip, None);
    }
    if let Some(message) = messages.get(hit) {
        let lines: Vec<&str> = message.text.split('\n').collect();
        let line = request.line.min(lines.len().saturating_sub(1));
        // The row's line first: the characters before its match, then the lines before it.
        let kept = request.column.min(room);
        room -= kept;
        let skip = skip_for(lines.get(..line).unwrap_or_default(), &mut room);
        out.gap();
        out.message(message, skip, Some((line, kept, request.column - kept)));
    }
    for message in messages.get(hit + 1..last).unwrap_or_default() {
        out.gap();
        out.message(message, 0, None);
    }
    (out.text, out.hit.0, out.hit.1)
}

/// How many of `lines`, from the first, are left out for the rest to fit in `room` characters,
/// which it takes from.
fn skip_for(lines: &[&str], room: &mut usize) -> usize {
    let mut skip = lines.len();
    for line in lines.iter().rev() {
        let chars = line.chars().count().max(1);
        if chars > *room {
            *room = 0;
            break;
        }
        *room -= chars;
        skip -= 1;
    }
    skip
}

/// The preview's text as it is made, and where its match is.
#[derive(Default)]
struct Out {
    text: String,
    /// The lines written, the one being written counted.
    lines: usize,
    hit: (usize, usize),
}

impl Out {
    fn line(&mut self, line: &str) {
        if !self.text.is_empty() {
            self.text.push('\n');
            self.lines += 1;
        }
        self.text.push_str(line);
    }

    fn gap(&mut self) {
        self.line("");
    }

    /// Writes `message` leaving out its first `skip` lines, and with `hit`, the line of the
    /// match, how many characters before the match are kept, and how many left out.
    fn message(&mut self, message: &Message, skip: usize, hit: Option<(usize, usize, usize)>) {
        let mark = if message.prompt { PROMPT } else { REPLY };
        let mut first = true;
        if skip > 0 {
            self.line(&format!("{mark}{CUT}"));
            first = false;
        }
        for (number, part) in message.text.split('\n').enumerate().skip(skip) {
            let start = if first { mark } else { INDENT };
            first = false;
            match hit {
                Some((line, kept, left_out)) if line == number => {
                    let marks = INDENT.chars().count();
                    let (shown, column) = if left_out > 0 {
                        let at = part
                            .char_indices()
                            .nth(left_out)
                            .map_or(part.len(), |(at, _)| at);
                        (format!("{start}{CUT}{}", &part[at..]), marks + 1 + kept)
                    } else {
                        (format!("{start}{part}"), marks + kept)
                    };
                    self.line(&shown);
                    self.hit = (self.lines, column);
                }
                _ => self.line(&format!("{start}{part}")),
            }
        }
        if let Some((line, _, _)) = hit
            && line < skip
        {
            self.hit = (self.lines, 0);
        }
    }
}

/// The read-only view's text of the conversation `about` holds, read whole as `whole` (ADR
/// 0145, step 2): a header naming its directory whole, its id and age, and what was left out,
/// then every prompt and reply, each starting with its mark, its other lines indented, a blank
/// line between; and the line its message `message` starts on. A message's lines start with a
/// mark only on its first, so the view's `n` and `p` find messages by their marks.
pub(super) fn view(
    about: &About,
    whole: &Whole,
    message: usize,
    now: std::time::SystemTime,
) -> (String, usize) {
    let mut out = Out::default();
    let directory = about
        .shown_directory()
        .map_or_else(|| "?".to_owned(), shown_directory);
    let gone = if about.gone { " (gone)" } else { "" };
    let resumes = if about.directory.is_some() {
        ""
    } else {
        " (cannot resume: oso cannot tell where it ran)"
    };
    out.line(&format!("in {directory}{gone}{resumes}"));
    let age = now
        .duration_since(about.changed)
        .map_or_else(|_| "now".to_owned(), crate::sessions::list::age_of);
    out.line(&format!(
        "session {} · {age} ago · {} messages",
        about.id.as_str(),
        whole.messages.len()
    ));
    if whole.cut {
        out.line("… more was said than the view keeps; the rest is left out");
    }
    out.line("n and p: next and previous message · r: go to the session · f: fork · g: read again");
    let mut at = 0;
    for (index, said) in whole.messages.iter().enumerate() {
        out.gap();
        if index == message {
            at = out.lines + 1;
        }
        out.message(said, 0, None);
    }
    (out.text, at)
}

/// The place a row names for a conversation that ran in `directory`: `repository/worktree` for
/// one under `<repository>/.claude/worktrees/`, else the directory's last name.
pub fn place(directory: &Path) -> String {
    let parts: Vec<&std::ffi::OsStr> = directory
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let worktree = parts
        .windows(4)
        .rev()
        .find(|window| window[1] == ".claude" && window[2] == "worktrees");
    match worktree {
        Some(window) => format!(
            "{}/{}",
            window[0].to_string_lossy(),
            window[3].to_string_lossy()
        ),
        None => parts.last().map_or_else(
            || "/".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        ),
    }
}
