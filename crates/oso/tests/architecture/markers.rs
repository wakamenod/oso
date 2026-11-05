//! Conflict markers left behind by a merge (ADR 0038, ADR 0039). A compiler
//! finds them in Rust and a parser in TOML, but nothing reads Markdown, so this
//! check looks at every file in the repository.
//!
//! Must not depend on: the file system or the other checks; it sees only paths
//! and text.

/// How long a marker is, and the characters it repeats. Markers are built from
/// these at run time, so this file holds none for the check to trip on.
///
/// The separator, seven `=`, is left out on purpose. It underlines a Markdown
/// heading of any length, so alone it proves nothing. Between the other markers
/// it would catch nothing new, because they already fail the file. The cost is a
/// conflict resolved by deleting only the outer markers: its separator stays
/// unnoticed where nothing parses the file.
const LENGTH: usize = 7;
const MARKS: [char; 3] = ['<', '|', '>'];

/// The end of a path from each place the walk must reach, most of them hidden
/// or not Rust. If the walk loses one, the check fails instead of passing on
/// fewer files.
const MUST_SEE: [&str; 7] = [
    ".cargo/mutants.toml",
    ".claude/settings.json",
    ".github/workflows/ci.yml",
    ".snap",
    "Justfile",
    "docs/dev/adr/README.md",
    "guardrails/layers.txt",
];

/// The marker that `line` starts with: a mark repeated [`LENGTH`] times, then a
/// space and git's label, or nothing.
fn marker(line: &str) -> Option<&str> {
    let (head, rest) = line.split_at_checked(LENGTH)?;
    let first = head.chars().next()?;
    let is_marker = MARKS.contains(&first)
        && head.chars().all(|c| c == first)
        && (rest.is_empty() || rest.starts_with(' '));
    is_marker.then_some(head)
}

/// What is wrong with `files`, given as paths and text: every marker, by file
/// and line, without the rest of its line, and every place the walk missed.
pub fn problems(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for ending in MUST_SEE {
        if !files.iter().any(|(at, _)| at.ends_with(ending)) {
            problems.push(format!("the walk did not see {ending}"));
        }
    }
    for (at, text) in files {
        for (index, line) in text.lines().enumerate() {
            if let Some(marker) = marker(line) {
                let line = index + 1;
                problems.push(format!(
                    "{at}:{line}: conflict marker `{marker}`; finish resolving the merge"
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::{LENGTH, MUST_SEE, problems};

    fn marker(mark: char) -> String {
        mark.to_string().repeat(LENGTH)
    }

    /// Every place the walk must reach, with `extra` after it.
    fn files(extra: &[(&str, &str)]) -> Vec<(String, String)> {
        MUST_SEE
            .iter()
            .map(|at| ((*at).to_owned(), String::new()))
            .chain(
                extra
                    .iter()
                    .map(|(at, t)| ((*at).to_owned(), (*t).to_owned())),
            )
            .collect()
    }

    fn found(at: &str, line: usize, mark: char) -> String {
        let marker = marker(mark);
        format!("{at}:{line}: conflict marker `{marker}`; finish resolving the merge")
    }

    #[test]
    fn every_marker_is_named_by_file_and_line_without_its_label() {
        let (ours, base, theirs) = (marker('<'), marker('|'), marker('>'));
        let separator = "=".repeat(LENGTH);
        let text = format!(
            "intro\n{ours} HEAD\na\n{base} merged common ancestors\nb\n{separator}\nc\n\
             {theirs} origin/main\n"
        );
        assert_eq!(
            problems(&files(&[("docs/a.md", &text), ("b.snap", &text)])),
            [
                found("docs/a.md", 2, '<'),
                found("docs/a.md", 4, '|'),
                found("docs/a.md", 8, '>'),
                found("b.snap", 2, '<'),
                found("b.snap", 4, '|'),
                found("b.snap", 8, '>'),
            ]
        );
    }

    #[test]
    fn a_marker_may_end_its_line_or_a_windows_line() {
        let text = format!("{}\n{} HEAD\r\na\r\n", marker('>'), marker('<'));
        assert_eq!(
            problems(&files(&[("a.md", &text)])),
            [found("a.md", 1, '>'), found("a.md", 2, '<')]
        );
    }

    #[test]
    fn a_lone_separator_is_a_heading_underline_and_passes() {
        let text = format!("Title\n{}\n\nText.\n", "=".repeat(LENGTH));
        assert_eq!(problems(&files(&[("a.md", &text)])), Vec::<String>::new());
    }

    #[test]
    fn lines_that_only_look_like_markers_pass() {
        let ours = marker('<');
        let lines = [
            // Too short or too long, as a quote or a table can be.
            format!("{} a", "<".repeat(LENGTH - 1)),
            format!("{} a", ">".repeat(LENGTH + 1)),
            // Not at the start of the line, or with no space before a label.
            format!(" {ours} HEAD"),
            format!("see {ours} HEAD"),
            format!("{ours}HEAD"),
            // Mixed marks, and characters wider than a byte across the length.
            "<<<>>>> a".to_owned(),
            "日本語のテキスト".to_owned(),
        ];
        for line in lines {
            let text = format!("{line}\n");
            assert_eq!(
                problems(&files(&[("a.md", &text)])),
                Vec::<String>::new(),
                "{line}"
            );
        }
    }

    #[test]
    fn a_walk_that_misses_a_place_fails() {
        for (index, ending) in MUST_SEE.iter().enumerate() {
            let mut seen = files(&[]);
            seen.remove(index);
            assert_eq!(problems(&seen), [format!("the walk did not see {ending}")]);
        }
    }
}
