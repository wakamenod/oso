//! Requests' fuzz target (ADR 0182, "Order"): calls made at random, of the tools a block knows and
//! others, their fields of the wrong types and their texts made of pieces that look like oso's
//! marks, rows and escapes. Whatever they are, only oso stands in a block's columns 0 and 1 and in
//! its faces, wrapped too; a change's lines give back both its texts; and a title and a line keep
//! to their rows.
//!
//! Must not depend on: `app`, threads or a process; these tests drive the pure part only.
//!
//! It runs in `just ci` with proptest's default cases. For a long run:
//! `PROPTEST_CASES=200000 cargo test -p oso --release sessions::native::request::fuzz_tests`.

use oso_core::display_map::text_clusters;
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::minibuffer::cells_of;
use oso_core::window::Screen;
use proptest::prelude::*;
use serde_json::{Map, json};

use super::body::{BASE, NO_BREAK, SPACE};
use super::line::{Says, line};
use super::reading::Reading;
use super::tests::{block, later};
use super::*;
use crate::sessions::native::render::Face;
use crate::sessions::native::wire::tools::MAX_INPUT;

/// A text of pieces that are read, or drawn, as something else somewhere.
fn text() -> impl Strategy<Value = String> {
    const PIECES: [&str; 43] = [
        "a",
        "word ",
        " ",
        "  ",
        "\n",
        "\n\n",
        "\n",
        "\r",
        "\r\n",
        "\t",
        "$ ",
        "+ ",
        "- ",
        "+",
        "-",
        "⚠ ",
        "⚠ Bash asks to run\n",
        " y  allow",
        "〉 ",
        "▸ $ Bash",
        "│ ",
        "1 of 2",
        "Change 1 of 2\n",
        "… 5 lines more not shown\n",
        NO_BREAK,
        "It has no input\n",
        "timeout 600000\n",
        "Its other fields\n",
        SPACE,
        "\u{301}",
        "\u{20e3}",
        "\u{1f3fb}",
        "\u{200d}",
        "\u{fe0f}",
        "\u{202e}",
        "\u{2028}",
        "\u{85}",
        "\u{1b}[31m",
        "\u{3000}",
        BASE,
        "あ",
        "한",
        "👩\u{200d}👩",
    ];
    prop_oneof![
        8 => prop::collection::vec(prop::sample::select(&PIECES[..]), 0..14)
            .prop_map(|pieces| pieces.concat()),
        1 => ".{0,60}",
        1 => (prop::sample::select(&PIECES[..]), 1usize..3000).prop_map(|(piece, times)| piece.repeat(times)),
    ]
}

fn value() -> impl Strategy<Value = Value> {
    prop_oneof![
        8 => text().prop_map(Value::String),
        1 => Just(json!(null)),
        1 => Just(json!(true)),
        1 => Just(json!(600_000)),
        1 => text().prop_map(|text| json!([text, {"k": text}])),
        1 => prop::collection::vec((text(), text()), 0..3).prop_map(|edits| {
            let edits = edits.into_iter();
            edits.map(|(old, new)| json!({"old_string": old, "new_string": new, "x": 1})).collect()
        }),
    ]
}

/// A tool and the input of a call of it.
pub(crate) fn call() -> impl Strategy<Value = (String, Value)> {
    let tool = prop_oneof![
        prop::sample::select(&["Bash", "Edit", "MultiEdit", "Write", "Read", "mcp__x__y"][..])
            .prop_map(str::to_owned),
        text(),
    ];
    const NAMES: [&str; 9] = [
        "command",
        "description",
        "timeout",
        "file_path",
        "old_string",
        "new_string",
        "replace_all",
        "content",
        "edits",
    ];
    let name = prop_oneof![
        8 => prop::sample::select(&NAMES[..]).prop_map(str::to_owned),
        1 => text(),
    ];
    (tool, prop::collection::vec((name, value()), 0..8)).prop_map(|(tool, fields)| {
        (
            tool,
            Value::Object(fields.into_iter().collect::<Map<_, _>>()),
        )
    })
}

/// Whether `words` are ones oso says in a block.
fn osos(words: &str) -> bool {
    let counted = || {
        let count =
            (words.strip_prefix("… ")).and_then(|rest| rest.strip_suffix(" lines more not shown"));
        count.is_some_and(|count| count.parse::<usize>().is_ok())
    };
    let nth = |rest: &str| {
        let numbers = rest.split_once(" of ");
        numbers.is_some_and(|(at, of)| at.parse::<usize>().is_ok() && of.parse::<usize>().is_ok())
    };
    const SAID: [&str; 8] = [
        NO_BREAK,
        SPACE,
        "It has no input",
        "Its input is over 64 KiB, or cannot be read: not shown at all",
        "The change",
        "Why",
        "Its other fields",
        "The call's other fields",
    ];
    SAID.contains(&words)
        || words == "Nothing is written"
        || words == "Nothing is changed"
        || words.strip_prefix("Change ").is_some_and(nth)
        || counted()
        || words == "… 1 line more not shown"
}

/// The face of the byte `at` of `body`.
fn face_at(body: &Rendered, at: usize) -> Option<Face> {
    let found = body.faces.partition_point(|(range, _)| range.end <= at);
    (body.faces.get(found))
        .filter(|(range, _)| range.contains(&at))
        .map(|(_, face)| *face)
}

/// Says what of `body` is not as it must be: each line starts with oso's mark, in oso's face or a
/// changed line's, or is oso's words, and Claude's text is in neither place.
pub(crate) fn only_oso_stands_first(body: &Rendered) -> Result<(), String> {
    let mut at = 0;
    for line in body.text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let text = line.strip_suffix('\n').ok_or("no line break at the end")?;
        let first = face_at(body, start).ok_or("a line that starts in no face")?;
        // Where the line's first run ends, if within the line.
        let run = (body.faces.iter())
            .find(|(range, _)| range.contains(&start))
            .map_or(start, |(range, _)| range.end.min(start + text.len()))
            - start;
        let wrong = |what: &str| Err(format!("{what}: {line:?}"));
        let lead = match first {
            Face::Added if text.starts_with("+ ") || text == "+" => "+ ",
            Face::Removed if text.starts_with("- ") || text == "- " || text == "-" => "- ",
            Face::RequestBody if osos(text) => {
                if run != text.len() {
                    return wrong("oso's words in part");
                }
                continue;
            }
            Face::RequestBody => {
                let own = &text[..run];
                let lead = ["  ", "    ", "$ ", "", "$"]
                    .into_iter()
                    .find(|lead| *lead == own);
                match lead {
                    Some(lead) if run > 1 || text == own => lead,
                    _ => return wrong("no mark of oso's"),
                }
            }
            _ => return wrong("not a face a line starts in"),
        };
        if face_at(body, start + text.len()) != Some(first) {
            return wrong("its break is in another face");
        }
        // What it shows past oso's mark is in none of oso's faces, and no cluster of it joins the
        // mark's columns.
        let own = lead.len().min(text.len());
        for (offset, _) in text[own..].char_indices() {
            let claudes = match face_at(body, start + own + offset) {
                None | Some(Face::Dim) => first == Face::RequestBody,
                Some(face) => face == first && first != Face::RequestBody,
            };
            if !claudes {
                return wrong("a face Claude's text is not in");
            }
        }
        let clusters = text_clusters(text, 0).take(2);
        for (cluster, own) in clusters.zip(text.chars()) {
            if cluster.cells != 1 || !cluster.text.chars().eq([own]) || cluster.column > 1 {
                return wrong("a cluster that takes more than a column of oso's");
            }
        }
    }
    Ok(())
}

/// The lines of a text of a change, as [`sides`] gives them, the last of them without its break
/// where a body says so: where the `other` text has one, or is empty.
fn pieces(text: &str, other: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(|line| format!("{line}\n")).collect();
    // What is after the last break is a line only if it holds something.
    match lines.last_mut() {
        Some(last) if last == "\n" => drop(lines.pop()),
        Some(last) if other.is_empty() || other.ends_with('\n') => drop(last.pop()),
        _ => {}
    }
    lines
}

/// The lines of the old and the new text of the change `body` shows, as read from it.
fn sides(body: &Rendered) -> [Vec<String>; 2] {
    let [mut old, mut new] = [Vec::new(), Vec::new()];
    let mut at = 0;
    let mut last = Face::Dim;
    for line in body.text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let line = line.trim_end_matches('\n');
        let osos = face_at(body, start + 2) == Some(Face::RequestBody);
        if start == 0 || osos && [SPACE, "Nothing is changed"].contains(&line) {
            continue;
        }
        if osos && line == NO_BREAK {
            let side = if last == Face::Removed {
                &mut old
            } else {
                &mut new
            };
            side.last_mut().map(String::pop);
            continue;
        }
        let line = line.get(2..).unwrap_or("");
        // What oso put in: a base before a mark, and a space after a carriage return.
        let line = (line.strip_prefix(BASE))
            .filter(|rest| joins(rest.trim_start_matches(BASE)))
            .unwrap_or(line);
        let line = format!("{}\n", unspaced(line));
        last = face_at(body, start).unwrap_or(Face::Dim);
        match last {
            Face::Removed => old.push(line),
            Face::Added => new.push(line),
            _ => {
                old.push(line.clone());
                new.push(line);
            }
        }
    }
    [old, new]
}

/// Whether the first character of `text` joins a space before it.
fn joins(text: &str) -> bool {
    let spaced = format!(" {text}");
    (text_clusters(&spaced, 0).next()).is_some_and(|cluster| cluster.text != " ")
}

/// A field's name: one a tool has, or any.
fn name() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(&["a", "b", "timeout", "c.d-e_f"][..]).prop_map(str::to_owned),
        text(),
    ]
}

/// The fields `body` shows, after its tool's name.
fn read_back(body: &Rendered) -> Option<Map<String, Value>> {
    let mut fields = Map::new();
    let lines: Vec<&str> = body
        .text
        .strip_suffix('\n')?
        .split('\n')
        .filter(|line| *line != SPACE)
        .collect();
    let mut lines = lines.into_iter().skip(1).peekable();
    while let Some(line) = lines.next() {
        let line = line.strip_prefix("  ")?;
        let mut names = serde_json::Deserializer::from_str(line).into_iter::<String>();
        let (name, rest) = match line.starts_with('"') {
            true => (names.next()?.ok()?, &line[names.byte_offset()..]),
            false => {
                let end = line.find(' ').unwrap_or(line.len());
                (line[..end].to_owned(), &line[end..])
            }
        };
        let value = if let Some(rest) = rest.strip_prefix(' ') {
            let rest = (rest.strip_prefix(BASE))
                .filter(|rest| joins(rest.trim_start_matches(BASE)))
                .unwrap_or(rest);
            unspaced(rest).to_owned()
        } else {
            let mut value = Vec::new();
            let more = |line: &&str| line.is_empty() || line.starts_with("    ");
            while let Some(line) = lines.next_if(more) {
                let line = line.get(4..).unwrap_or("");
                let line = (line.strip_prefix(BASE))
                    .filter(|rest| joins(rest.trim_start_matches(BASE)))
                    .unwrap_or(line);
                value.push(unspaced(line));
            }
            value.join("\n")
        };
        let value = serde_json::from_str(&value).unwrap_or(Value::String(value));
        fields.insert(name, value);
    }
    Some(fields)
}

/// `line` without the space oso put after a carriage return at its end.
fn unspaced(line: &str) -> &str {
    line.strip_suffix(' ')
        .filter(|line| line.trim_end_matches(' ').ends_with('\r'))
        .unwrap_or(line)
}

proptest! {
    #[test]
    fn only_oso_stands_in_its_columns_and_its_faces(
        (tool, input) in call(),
        late in any::<bool>(),
        shown in any::<bool>(),
        place in (1usize..4, 1usize..4),
        width in 0usize..140,
    ) {
        let deadline = if late { Instant::now() } else { later() };
        let block = Block::call(&tool, shown.then_some(&input), deadline);
        let body = block.body();
        prop_assert_eq!(only_oso_stands_first(body), Ok(()), "{:?}", body.text);
        let mut at = 0;
        for (range, _) in &body.faces {
            prop_assert!(at <= range.start && range.start < range.end);
            prop_assert!(body.text.is_char_boundary(range.start));
            prop_assert!(body.text.is_char_boundary(range.end));
            at = range.end;
        }
        // Its runs cover it, as a part of the tail's do.
        prop_assert_eq!(body.runs().iter().map(|run| run.0).sum::<usize>(), body.text.len());
        let count = body.text.lines().count();
        prop_assert!(count <= super::body::MAX_LINES + 1);
        prop_assert!(body.text.len() < 4 * MAX_INPUT);
        prop_assert_eq!(block.hidden() == Hidden::All, body.text.ends_with("cannot be read: not shown at all
"));
        prop_assert!(shown || block.hidden() == Hidden::All);
        let title = block.title(place, width);
        prop_assert_eq!(title.text.matches('\n').count(), 1);
        prop_assert!(cells_of(title.text.trim_end_matches('\n')) <= width.max(1));
        prop_assert!(title.text.chars().all(|c| c.is_ascii() || "⚠·…".contains(c)));
        prop_assert!(title.faces.iter().all(|(_, face)| *face == Face::CardTitle));
        prop_assert_eq!(title.runs().iter().map(|run| run.0).sum::<usize>(), title.text.len());
        let says = Says { here: late, answers: shown };
        let line = line(&block, &Reading::new(&block), says, width);
        prop_assert_eq!(line.shown.text.split('\n').count(), line.rows);
        prop_assert!(line.shown.text.chars().all(|c| c.is_ascii() || "·…".contains(c)));
        prop_assert!(line.shown.text.split('\n').all(|row| cells_of(row) <= width.max(1)));
    }

    /// Wrapped in a session's window, no row of a body starts in oso's two columns but a line's
    /// first.
    #[test]
    fn a_wrapped_row_never_starts_in_osos_columns((tool, input) in call(), columns in 15usize..100) {
        let block = block(&tool, &input);
        let mut editor = Editor::default();
        editor.set_frame_size(FrameSize { rows: 400, columns });
        let id = editor.add_native_session_buffer("*claude: s*");
        editor.show(id);
        editor.set_wraps(id, true);
        let title = block.title((1, 2), columns);
        for part in [&title, block.body()] {
            let end = editor.buffer(id).unwrap().text().len();
            editor.replace_native_rows(id, end..end, &part.text, &part.runs());
        }
        prop_assert_eq!(editor.buffer(id).unwrap().text().len(), title.text.len() + block.body().text.len());
        let mut screen = Screen::default();
        editor.screen_of(editor.window().id(), &mut screen);
        for row in screen.rows().iter().filter(|row| !row.starts_line()) {
            let indent = screen.drawn(row).start - screen.origin(row);
            prop_assert!(indent >= 2, "{} in {:?}", indent, block.body().text);
        }
    }

    /// A call's fields can be read back from its block.
    #[test]
    fn fields_are_exact(fields in prop::collection::vec((name(), value()), 0..6)) {
        let input: Map<String, Value> = fields.into_iter().collect();
        let block = block("X", &Value::Object(input.clone()));
        if block.hidden() == Hidden::Nothing && !input.is_empty() {
            prop_assert_eq!(read_back(block.body()), Some(input), "{:?}", block.body().text);
        }
    }

    /// A change's lines give both its texts back, whatever they hold and however they were
    /// compared.
    #[test]
    fn a_change_is_exact(old in text(), new in text(), late in any::<bool>()) {
        let deadline = if late { Instant::now() } else { later() };
        let input = json!({"file_path": "a", "old_string": old, "new_string": new});
        let block = Block::call("Edit", Some(&input), deadline);
        if block.hidden() == Hidden::Nothing {
            prop_assert_eq!(sides(block.body()), [pieces(&old, &new), pieces(&new, &old)]);
        }
    }
}
