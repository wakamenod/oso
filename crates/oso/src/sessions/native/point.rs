//! What a native session's keys find at point (ADR 0165, ADR 0130's design): the call of an
//! open row of several that a line is in, the row `n`/`p` and `]`/`[` go to, and the whole
//! result `RET` shows in `*result: NAME*`.
//!
//! Must not depend on: the editor or the UI; the host says which row point is in, with
//! `Placed`.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::Path;

use super::placed::Placed;
use super::render::{one_line, target};
use super::rows::{Kind, Row, Tool};
use super::wire::MAX_TEXT;

/// What a move goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wanted {
    /// A message of the person's.
    Prompt,
    /// A row of tool calls.
    Tools,
}

impl Wanted {
    fn is(self, row: &Row) -> bool {
        match self {
            Self::Prompt => matches!(row.kind, Kind::Prompt(_)),
            Self::Tools => matches!(row.kind, Kind::Tools(_)),
        }
    }
}

/// Where point's line is: in row `row`, its first line or not; or past the last row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    In { row: usize, first_line: bool },
    End,
}

/// Where row `row`'s first line that shows something starts: past its gap.
pub fn first_line(rows: &[Row], placed: &Placed, row: usize) -> usize {
    placed.start(row) + usize::from(rows.get(row).is_some_and(|row| row.gap))
}

/// Where byte `offset` is, where a line starts. The blank line before a row is the row
/// before's, so that the row comes next.
pub fn point(rows: &[Row], placed: &Placed, offset: usize) -> Point {
    let Some(row) = placed.row_at(offset) else {
        return Point::End;
    };
    let first = first_line(rows, placed, row);
    match row.checked_sub(1).filter(|_| offset < first) {
        Some(row) => Point::In {
            row,
            first_line: false,
        },
        None => Point::In {
            row,
            first_line: offset == first,
        },
    }
}

/// The row `count` rows of `wanted` after point, before it if negative; `None` if there are
/// not so many. Backwards, a row of `wanted` that point is inside, past its first line, counts
/// first, as `p` goes to the start of the message point is in.
pub fn step(rows: &[Row], point: Point, count: isize, wanted: Wanted) -> Option<usize> {
    let left = count.unsigned_abs().max(1);
    if count >= 0 {
        let after = match point {
            Point::In { row, .. } => row + 1,
            Point::End => return None,
        };
        let mut found = (after..rows.len()).filter(|at| wanted.is(&rows[*at]));
        return found.nth(left - 1);
    }
    let before = match point {
        Point::In {
            row,
            first_line: true,
        } => row,
        Point::In {
            row,
            first_line: false,
        } => row + 1,
        Point::End => rows.len(),
    };
    let mut found = (0..before.min(rows.len()))
        .rev()
        .filter(|at| wanted.is(&rows[*at]));
    found.nth(left - 1)
}

/// The call of an open row of several whose lines hold byte `within` of the row, `calls` being
/// where [`Rendered::calls`](super::render::Rendered::calls) put each; `None` on the row's own line.
pub fn slot(calls: &[(usize, Range<usize>)], within: usize) -> Option<usize> {
    calls
        .iter()
        .find(|(_, range)| range.contains(&within))
        .map(|(slot, _)| *slot)
}

/// The call `RET` shows of `row`: the one at `slot`, or the row's only one.
pub fn call(row: &Row, slot: Option<usize>) -> Option<&Tool> {
    let Kind::Tools(tools) = &row.kind else {
        return None;
    };
    match (tools.as_slice(), slot) {
        ([tool], _) => Some(tool),
        (_, Some(slot)) => tools.get(slot),
        _ => None,
    }
}

/// What `*result: NAME*` shows of `tool`: its line, then its whole result or change, as it
/// came. `None` while it has none.
pub fn whole(tool: &Tool, directory: &Path) -> Option<String> {
    let result = tool.result.as_ref()?;
    let mut out = one_line(&tool.name);
    let target = target(tool, directory);
    if !target.is_empty() {
        out.push(' ');
        out.push_str(&target);
    }
    if result.error {
        out.push_str(" · error");
    }
    out.push_str("\n\n");
    for hunk in &result.patch {
        let _ = writeln!(
            out,
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
        );
        for line in &hunk.lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    if result.patch.is_empty() {
        out.push_str(&result.text);
        if !result.text.ends_with('\n') {
            out.push('\n');
        }
    }
    if result.cut {
        let _ = writeln!(out, "… cut at {} MiB as it came", MAX_TEXT >> 20);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::placed::Placed;
    use super::super::rows::{Kind, Row, Tool, ToolState};
    use super::super::wire::{Hunk, ToolInput, ToolResult, TurnResult};
    use super::{Point, Wanted, call, first_line, point, slot, step, whole};
    use crate::sessions::native::render::render;

    fn row(kind: Kind) -> Row {
        Row {
            kind,
            depth: 0,
            folded: false,
            gap: false,
        }
    }

    fn tool(name: &str, text: &str) -> Tool {
        Tool {
            id: name.to_owned(),
            name: name.to_owned(),
            input: ToolInput::default(),
            streamed: 0,
            state: ToolState::Done,
            approved: None,
            result: Some(ToolResult {
                tool_use_id: name.to_owned(),
                error: false,
                text: text.to_owned(),
                cut: false,
                lines: text.lines().count(),
                patch: Vec::new(),
                changed: (0, 0),
            }),
            progress: None,
            folded: true,
            apart: false,
            waiting: false,
        }
    }

    /// A prompt, a reply, a call, a prompt, two calls in a row, a result.
    fn rows() -> Vec<Row> {
        vec![
            row(Kind::Prompt("one".to_owned())),
            row(Kind::Note("a".to_owned())),
            row(Kind::Tools(vec![tool("Bash", "x")])),
            row(Kind::Prompt("two".to_owned())),
            row(Kind::Tools(vec![tool("Read", "a"), tool("Read", "b")])),
            row(Kind::Result(TurnResult::default())),
        ]
    }

    #[test]
    fn n_and_p_go_from_prompt_to_prompt() {
        let rows = rows();
        let at = |row, first_line| Point::In { row, first_line };
        assert_eq!(step(&rows, at(0, true), 1, Wanted::Prompt), Some(3));
        assert_eq!(step(&rows, at(1, false), 1, Wanted::Prompt), Some(3));
        assert_eq!(step(&rows, at(0, true), 2, Wanted::Prompt), None);
        assert_eq!(step(&rows, at(3, true), -1, Wanted::Prompt), Some(0));
        // Inside a prompt, past its start, `p` goes to its start first.
        assert_eq!(step(&rows, at(3, false), -1, Wanted::Prompt), Some(3));
        assert_eq!(step(&rows, Point::End, -2, Wanted::Prompt), Some(0));
        assert_eq!(step(&rows, Point::End, 1, Wanted::Prompt), None);
    }

    #[test]
    fn a_blank_line_before_a_row_is_the_row_before_s() {
        let mut rows = rows();
        for at in [1, 3] {
            rows[at].gap = true;
        }
        let mut placed = Placed::default();
        let mut at = 0;
        for (row, shown) in rows.iter().enumerate() {
            let length = render(shown, Path::new("/w"), 80).text.len();
            placed.put(row, at..at, length);
            at += length;
        }
        let inside = |row| Point::In {
            row,
            first_line: false,
        };
        let on = |row| Point::In {
            row,
            first_line: true,
        };
        let blank = placed.start(3);
        assert_eq!(first_line(&rows, &placed, 3), blank + 1);
        assert_eq!(first_line(&rows, &placed, 2), placed.start(2));
        assert_eq!(point(&rows, &placed, blank), inside(2));
        assert_eq!(point(&rows, &placed, blank + 1), on(3));
        assert_eq!(point(&rows, &placed, 0), on(0));
        assert_eq!(point(&rows, &placed, at), Point::End);
        // From the blank line, the message under it is the next, and the one above the last.
        assert_eq!(step(&rows, inside(2), 1, Wanted::Prompt), Some(3));
        assert_eq!(step(&rows, inside(2), -1, Wanted::Prompt), Some(0));
        assert_eq!(step(&rows, inside(2), -1, Wanted::Tools), Some(2));
    }

    #[test]
    fn brackets_go_from_tool_row_to_tool_row() {
        let rows = rows();
        let at = |row| Point::In {
            row,
            first_line: true,
        };
        assert_eq!(step(&rows, at(0), 1, Wanted::Tools), Some(2));
        assert_eq!(step(&rows, at(0), 2, Wanted::Tools), Some(4));
        assert_eq!(step(&rows, at(5), -1, Wanted::Tools), Some(4));
        assert_eq!(step(&rows, at(2), -1, Wanted::Tools), None);
    }

    #[test]
    fn a_line_of_an_open_row_names_its_call() {
        let rows = rows();
        let rendered = render(&rows[4], Path::new("/w"), 80);
        let first = rendered.text.find('\n').unwrap();
        assert_eq!(slot(&rendered.calls, 0), None);
        assert_eq!(slot(&rendered.calls, first + 1), Some(0));
        assert_eq!(slot(&rendered.calls, rendered.text.len() - 1), Some(1));
        assert_eq!(call(&rows[4], None), None);
        let text = |tool: &Tool| tool.result.as_ref().map(|result| result.text.clone());
        assert_eq!(call(&rows[4], Some(1)).and_then(text), Some("b".to_owned()));
        assert_eq!(call(&rows[2], None).map(|t| t.name.as_str()), Some("Bash"));
        assert_eq!(call(&rows[0], None), None);
    }

    #[test]
    fn ret_shows_a_result_whole_or_its_change() {
        let mut long = tool("Bash", &"line\n".repeat(500));
        let shown = whole(&long, Path::new("/w")).unwrap();
        assert_eq!(shown.lines().count(), 502);
        assert!(shown.starts_with("Bash\n\nline\n"), "{shown}");
        long.result.as_mut().unwrap().patch = vec![Hunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: vec!["-a".to_owned(), "+b".to_owned()],
        }];
        let shown = whole(&long, Path::new("/w")).unwrap();
        assert_eq!(shown, "Bash\n\n@@ -1,1 +1,1 @@\n-a\n+b\n");
        long.name = "Read\n\n@@ -1 +1 @@".to_owned();
        let header = whole(&long, Path::new("/w")).unwrap();
        assert!(
            header.starts_with("Read  @@ -1 +1 @@\n\n@@ -1,1"),
            "{header}"
        );
        long.result = None;
        assert_eq!(whole(&long, Path::new("/w")), None);
    }
}
