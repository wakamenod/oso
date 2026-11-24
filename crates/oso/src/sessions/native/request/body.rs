//! What a call's block shows under its title (ADR 0182): what runs, why, and every other field, all
//! of it, a change as a diff.
//!
//! Must not depend on: the app or the UI; plain functions over what `wire` read. The comparison of a
//! change alone reads the clock, for the time its caller gives.
//!
//! - Columns 0 and 1 are oso's: a line of Claude's has oso's mark there (`$ `, `+ `, `- `) or
//!   U+0020 twice, and a line that has neither is oso's words, from column 0. A mark of Claude's
//!   that would join what is before it goes on `◌`.
//! - A line starts in [`Face::RequestBody`], or in a changed line's face with its mark, and oso's
//!   marks and words alone are in the one. Its other text is Claude's, in no face, a changed line's, or the quiet one.
//! - Where a part could be read on into the next, as a command of several lines or a change, oso's
//!   words stand between them.
//! - A field's name and value that could be read in two ways are JSON. Nothing else is taken out,
//!   put in or turned into another, but for that base, and a space after a
//!   carriage return that ends a line, spaces or not, so that the display shows it as the escape
//!   it shows any other as. White space at a line's end, which shows as nothing, is said. What else it does not print, it escapes, with or without highlighting.
//! - At most [`MAX_LINES`] lines; how many are left out is counted, and said.

use std::borrow::Cow;
use std::time::Instant;

use oso_core::display_map::text_clusters;

use crate::review::diff::hunks;
use crate::sessions::native::render::{Face, Rendered};
use crate::sessions::native::wire::request::{Part, Shown};
use crate::sessions::native::wire::tools::MAX_INPUT;

/// The most lines a block shows under its title, but for the one that says how many it does not.
pub(super) const MAX_LINES: usize = 2000;
/// What stands before a line of Claude's that has no mark of oso's.
const INDENT: &str = "  ";
/// What stands before a line of a field's value of several lines.
const DEEPER: &str = "    ";
/// The most characters of a value beside its field's name.
const MAX_BESIDE: usize = 40;
/// What a mark of Claude's goes on where it would join what is before it.
pub(super) const BASE: &str = "\u{25cc}";
/// What says that the line before it ends in white space.
pub(super) const SPACE: &str = "\\ white space at its end";
/// What says that the line before it has no line break, as the other text of its change has.
pub(super) const NO_BREAK: &str = "\\ no line break at its end";

/// What of a call its block does not show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hidden {
    Nothing,
    /// This many lines.
    Lines(usize),
    /// Its whole input.
    All,
}

/// What a call's changes come to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    /// Lines added to a file, and taken out of it.
    Changed { added: usize, removed: usize },
    /// The lines of a file written whole.
    Written(usize),
}

/// `1 line`, `2 lines`.
pub(super) fn lines(count: usize) -> String {
    format!("{count} line{}", if count == 1 { "" } else { "s" })
}

/// The body of a call whose input is not shown.
pub(super) fn not_shown(name: Option<&str>) -> (Rendered, Option<Count>, Hidden) {
    let mut body = Body::default();
    if let Some(name) = name {
        body.line((INDENT, Face::RequestBody), name, None);
    }
    let kib = MAX_INPUT >> 10;
    body.say(&format!(
        "Its input is over {kib} KiB, or cannot be read: not shown at all"
    ));
    (body.out, None, Hidden::All)
}

/// The tool's `name`, where its block says it, and `parts` as text and faces, what their changes
/// come to, and what is left out.
pub(super) fn body(
    name: Option<&str>,
    parts: &[Part],
    deadline: Instant,
) -> (Rendered, Option<Count>, Hidden) {
    let mut body = Body::default();
    if let Some(name) = name {
        body.line((INDENT, Face::RequestBody), name, None);
    }
    if parts.is_empty() {
        body.say("It has no input");
    }
    for part in parts {
        match part {
            Part::Command(command) => body.text("$ ", command, None),
            Part::File(path) if path.starts_with('"') || path.contains(char::is_control) => {
                body.text(INDENT, &quoted(path), None);
            }
            Part::File(path) => body.text(INDENT, path, None),
            Part::Nth(at, of) => body.say(&format!("Change {at} of {of}")),
            Part::Change { old, new } => {
                body.head(body.fields, "The change");
                body.change(old, new, deadline);
            }
            Part::Written(text) => body.written(text),
            Part::Why(why) => {
                body.head(body.several || reads_as_field(why), "Why");
                body.text(INDENT, why, Some(Face::Dim));
            }
            Part::Field(name, value) => {
                body.head(body.several, "Its other fields");
                body.field(name, value);
            }
            Part::Call => body.say("The call's other fields"),
        }
        body.fields = matches!(part, Part::Field(..));
        // A field's lines say which are its own.
        body.several = match part {
            Part::Command(text) | Part::Why(text) => text.contains('\n'),
            Part::Change { .. } | Part::Written(_) => true,
            _ => false,
        };
    }
    let hidden = match body.left_out {
        0 => Hidden::Nothing,
        left => {
            body.put_said(&format!("… {} more not shown", lines(left)));
            Hidden::Lines(left)
        }
    };
    (body.out, body.count, hidden)
}

/// A body as it is put together.
#[derive(Default)]
struct Body {
    out: Rendered,
    lines: usize,
    left_out: usize,
    count: Option<Count>,
    /// The part put last is a field, or a text that may be of several lines.
    fields: bool,
    several: bool,
}

impl Body {
    /// Oso's `words` over a part, if the one before it could be read on into it.
    fn head(&mut self, apart: bool, words: &str) {
        if apart {
            self.say(words);
        }
    }

    /// Whether there is room for one more line, which is counted if there is none.
    fn room(&mut self) -> bool {
        if self.lines == MAX_LINES {
            self.left_out += 1;
            return false;
        }
        self.lines += 1;
        true
    }

    /// A line of oso's words.
    fn say(&mut self, words: &str) {
        if self.room() {
            self.put_said(words);
        }
    }

    fn put_said(&mut self, words: &str) {
        self.put(words, Some(Face::RequestBody));
        self.put("\n", Some(Face::RequestBody));
    }

    /// Claude's `text`, its first line after `lead`, oso's mark, and the others in line with it.
    fn text(&mut self, lead: &str, text: &str, face: Option<Face>) {
        for (at, line) in text.split('\n').enumerate() {
            let lead = if at == 0 { lead } else { INDENT };
            self.line((lead, Face::RequestBody), line, face);
            self.space(line);
        }
    }

    /// Says that `line` ends in white space, if it does.
    fn space(&mut self, line: &str) {
        let blank = |c: char| match c {
            // Letters and a pattern that draw nothing.
            '\t' | '\u{115f}' | '\u{1160}' | '\u{2800}' => true,
            // What the display escapes.
            '\u{2028}' | '\u{2029}' => false,
            c => c.is_whitespace() && !c.is_control(),
        };
        if line.ends_with(blank) {
            self.say(SPACE);
        }
    }

    /// A field: its value after its name where it is one line, else under it, further in.
    fn field(&mut self, name: &str, value: &Shown) {
        let (Shown::Text(text) | Shown::Json(text)) = value;
        if self.lines == MAX_LINES {
            let more = text.matches('\n').count();
            self.left_out += if more > 0 { more + 2 } else { 1 };
            return;
        }
        let name = named(name);
        let lead = (INDENT, Face::RequestBody);
        // Further in, so that where it goes on is not read as a field.
        if text.contains('\n') || text.chars().nth(MAX_BESIDE).is_some() {
            self.line(lead, &name, None);
            for line in text.split('\n') {
                self.line((DEEPER, Face::RequestBody), line, None);
                self.space(line);
            }
            return;
        }
        let base = if joins(text.trim_start_matches(BASE)) {
            BASE
        } else {
            ""
        };
        self.line(lead, &format!("{name} {base}{text}"), None);
        self.space(text);
    }

    /// A text written whole, each line of it added.
    fn written(&mut self, text: &str) {
        if text.is_empty() {
            self.say("Nothing is written");
        }
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let count = self.side(&lines, Side::Added, !ends(text));
        self.count = Some(Count::Written(count));
    }

    /// The change of `old` into `new`, compared until `deadline`.
    fn change<'a>(&mut self, old: &'a str, new: &'a str, deadline: Instant) {
        if old.is_empty() && new.is_empty() {
            self.say("Nothing is changed");
        }
        let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
        let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
        // A last line with no break is another line, and says so, unless both texts lack it, as
        // pieces of lines do.
        let same = ends(old) == ends(new) || old.is_empty() || new.is_empty();
        let kept = |line: &'_ &str| -> usize { line.len() - usize::from(same && ends(line)) };
        let bytes = |lines: &[&'a str]| -> Vec<&'a [u8]> {
            lines
                .iter()
                .map(|line| &line.as_bytes()[..kept(line)])
                .collect()
        };
        let (old_bytes, new_bytes) = (bytes(&old_lines), bytes(&new_lines));
        let bare = |text: &str, other: &str| !ends(text) && (ends(other) || other.is_empty());
        let (old_bare, new_bare) = (bare(old, new), bare(new, old));
        let (mut added, mut removed) = (0, 0);
        match hunks(&old_bytes, &new_bytes, deadline) {
            Some(hunks) => {
                let mut kept = 0;
                for hunk in hunks {
                    let from = index(hunk.old_start);
                    let to = index(hunk.new_start);
                    self.side(&old_lines[kept..from], Side::Kept, false);
                    let gone = from..from + hunk.removed.len();
                    kept = gone.end;
                    let last = old_bare && kept == old_lines.len();
                    removed += self.side(&old_lines[gone], Side::Removed, last);
                    let come = to..to + hunk.added.len();
                    let last = new_bare && come.end == new_lines.len();
                    added += self.side(&new_lines[come], Side::Added, last);
                }
                self.side(&old_lines[kept..], Side::Kept, false);
            }
            None => {
                removed = self.side(&old_lines, Side::Removed, old_bare);
                added = self.side(&new_lines, Side::Added, new_bare);
            }
        }
        let (before, out) = match self.count {
            Some(Count::Changed { added, removed }) => (added, removed),
            _ => (0, 0),
        };
        self.count = Some(Count::Changed {
            added: before + added,
            removed: out + removed,
        });
    }

    /// `lines` of one side of a change, the last of them with no line break if `bare`, which is
    /// said; how many they are.
    fn side(&mut self, lines: &[&str], side: Side, bare: bool) -> usize {
        let (lead, face) = match side {
            Side::Kept => ((INDENT, Face::RequestBody), Face::Dim),
            Side::Removed => (("- ", Face::Removed), Face::Removed),
            Side::Added => (("+ ", Face::Added), Face::Added),
        };
        for line in lines {
            let line = line.strip_suffix('\n').unwrap_or(line);
            self.line(lead, line, Some(face));
            if !matches!(side, Side::Kept) {
                self.space(line);
            }
        }
        if bare && !lines.is_empty() {
            self.say(NO_BREAK);
        }
        lines.len()
    }

    /// A line of Claude's, `text`, which holds no line break, after oso's `lead`, its two columns
    /// or more.
    fn line(&mut self, (lead, mark): (&str, Face), text: &str, face: Option<Face>) {
        debug_assert!(!text.contains('\n'));
        if !self.room() {
            return;
        }
        if text.is_empty() {
            // No line of it ends in a space, and its tint is on its break.
            self.put(lead.trim_end_matches(' '), Some(mark));
            self.put("\n", Some(mark));
            return;
        }
        self.put(lead, Some(mark));
        // One more than there are of either, so that what is put in is told from what is there.
        if joins(text.trim_start_matches(BASE)) {
            self.put(BASE, face);
        }
        self.put(text, face);
        if text.trim_end_matches(' ').ends_with('\r') {
            self.put(" ", face);
        }
        // Oso's, so that a body ends in a face, as the runs of a part of the tail do.
        self.put("\n", Some(mark));
    }

    /// `text` in `face`, one run with what it follows where that is in it too.
    fn put(&mut self, text: &str, face: Option<Face>) {
        let start = self.out.text.len();
        if let (Some(face), Some((last, same))) = (face, self.out.faces.last_mut())
            && *same == face
            && last.end == start
        {
            self.out.text.push_str(text);
            last.end = self.out.text.len();
            return;
        }
        self.out.put(text, face);
    }
}

/// A field's name as it is on its line: as JSON unless it is plain, so that nothing of it can be
/// read as its value, or as another field's.
fn named(name: &str) -> Cow<'_, str> {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
    if !name.is_empty() && name.chars().all(plain) {
        return Cow::Borrowed(name);
    }
    Cow::Owned(quoted(name))
}

/// Whether `why` could be read as a field: a word, and a value.
fn reads_as_field(why: &str) -> bool {
    let value = why.split_once(' ').map(|(_, value)| value);
    why.starts_with('"')
        || value.is_some_and(|value| serde_json::from_str::<serde_json::Value>(value).is_ok())
}

/// `text` as JSON.
fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// Where a line of a change stands.
#[derive(Clone, Copy)]
enum Side {
    Kept,
    Removed,
    Added,
}

/// Whether `text` ends with a line break.
fn ends(text: &str) -> bool {
    text.ends_with('\n')
}

/// A hunk's line number as an index.
fn index(number: u32) -> usize {
    usize::try_from(number)
        .unwrap_or(usize::MAX)
        .saturating_sub(1)
}

/// Whether `text`'s first character would join a space before it, as the display has it.
fn joins(text: &str) -> bool {
    let Some(first) = text.chars().next().filter(|c| !c.is_ascii()) else {
        return false;
    };
    let joined = format!(" {first}");
    text_clusters(&joined, 0)
        .next()
        .is_some_and(|cluster| cluster.text != " ")
}
