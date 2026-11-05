//! Conflict markers left behind by a merge (ADR 0038, ADR 0039). A compiler
//! finds them in Rust and a parser in TOML, but nothing reads Markdown, so this
//! check looks at every file in the repository.
//!
//! Must not depend on: the file system or the other checks; it sees only paths
//! and text.

/// How long a marker is, and the characters it repeats. Markers are built from
/// these at run time, so this file holds none for the check to trip on.
const LENGTH: usize = 7;
const MARKS: [char; 3] = ['<', '|', '>'];

/// What the separator repeats. A conflict resolved by deleting only the outer
/// markers leaves the separator alone, and in a table of ADRs, where keeping both
/// sides is the natural resolution, nothing else would notice. It also underlines
/// a Markdown heading, so a line of exactly this length fails there too: the docs
/// write headings with `#`.
const SEPARATOR: char = '=';

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
/// space and git's label, or nothing. The separator has no label, so it must be
/// the whole line.
fn marker(line: &str) -> Option<&str> {
    let (head, rest) = line.split_at_checked(LENGTH)?;
    let first = head.chars().next()?;
    let ends_well = if first == SEPARATOR {
        rest.is_empty()
    } else {
        MARKS.contains(&first) && (rest.is_empty() || rest.starts_with(' '))
    };
    (ends_well && head.chars().all(|c| c == first)).then_some(head)
}

/// The report of `marker` at `line` of `at`. Only the separator can be something
/// else, a heading underline, so its report says what to do then.
fn problem(at: &str, line: usize, marker: &str) -> String {
    let heading = if marker.starts_with(SEPARATOR) {
        ", or write the heading with `#`"
    } else {
        ""
    };
    format!("{at}:{line}: conflict marker `{marker}`; finish resolving the merge{heading}")
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
                problems.push(problem(at, index + 1, marker));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::{LENGTH, MUST_SEE, problem, problems};

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
        problem(at, line, &marker(mark))
    }

    #[test]
    fn every_marker_is_named_by_file_and_line_without_its_label() {
        let (ours, base, theirs) = (marker('<'), marker('|'), marker('>'));
        let separator = marker('=');
        let text = format!(
            "intro\n{ours} HEAD\na\n{base} merged common ancestors\nb\n{separator}\nc\n\
             {theirs} origin/main\n"
        );
        assert_eq!(
            problems(&files(&[("docs/a.md", &text), ("b.snap", &text)])),
            [
                found("docs/a.md", 2, '<'),
                found("docs/a.md", 4, '|'),
                found("docs/a.md", 6, '='),
                found("docs/a.md", 8, '>'),
                found("b.snap", 2, '<'),
                found("b.snap", 4, '|'),
                found("b.snap", 6, '='),
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
    fn a_lone_separator_fails_and_says_what_to_do_about_a_heading() {
        let separator = marker('=');
        // Both sides of a conflict kept, on any line ending or at the end of a file.
        for text in [
            format!("| a |\n{separator}\n| b |\n"),
            format!("| a |\r\n{separator}\r\n| b |\r\n"),
            format!("| a |\n{separator}"),
        ] {
            assert_eq!(
                problems(&files(&[("a.md", &text)])),
                [format!(
                    "a.md:2: conflict marker `{separator}`; finish resolving the merge, or write \
                     the heading with `#`"
                )]
            );
        }
    }

    #[test]
    fn the_other_markers_do_not_mention_a_heading() {
        assert!(!found("a.md", 1, '<').contains("heading"));
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
            // The separator at another length, not alone on its line, or indented.
            "=".repeat(LENGTH - 1),
            "=".repeat(LENGTH + 1),
            "===".to_owned(),
            format!("{} ", marker('=')),
            format!("{} label", marker('=')),
            format!(" {}", marker('=')),
            format!("text {}", marker('=')),
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
