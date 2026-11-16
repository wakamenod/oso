//! Tests of a graph's text: its rows as a text grid with their faces, the table of which
//! commit each line is, names and subjects escaped, and the note once the graph is cut.
//!
//! Must not depend on: git or the file system.

use oso_core::syntax::Highlight;

use super::super::lanes::Lanes;
use super::super::{Commit, Kind, Label, MAX_COMMITS};
use super::{Laid, MAX_LABELS, MORE_NOTE, Text};

fn commit(n: u8, parents: &[u8], labels: Vec<Label>, subject: &[u8]) -> Commit {
    let id = |n: u8| format!("{n:x}").repeat(40);
    Commit {
        short: id(n)[..7].to_owned(),
        id: id(n),
        parents: parents.iter().map(|p| id(*p)).collect(),
        labels,
        author: b"Ann".to_vec(),
        time: 0,
        subject: subject.to_vec(),
    }
}

fn text(commits: Vec<Commit>) -> Text {
    let mut lanes = Lanes::default();
    let mut text = Text::default();
    for commit in commits {
        let (cells, _) = lanes.row(&commit.id, &commit.parents);
        text.push(&cells, &commit, "2026-11-16");
    }
    text
}

/// Each line, and under it a mark for each character's face.
fn grid(laid: &Laid) -> String {
    let text = laid.text.text().to_string();
    let mut out = String::new();
    let mut at = 0;
    for line in text.split('\n') {
        let marks: String = line
            .char_indices()
            .map(|(offset, _)| match laid.highlights.at(at + offset) {
                None => ' ',
                Some(Highlight::GitSectionHeading) => 'H',
                Some(Highlight::GitHash) => 'h',
                Some(Highlight::GitBranchLocal) => 'b',
                Some(Highlight::GitBranchRemote) => 'r',
                Some(Highlight::GitTag) => 't',
                Some(Highlight::GitDetail) => 'c',
                Some(Highlight::GitLane0) => '0',
                Some(Highlight::GitLane1) => '1',
                Some(_) => '?',
            })
            .collect();
        out.push_str(line);
        out.push('\n');
        if !marks.trim().is_empty() {
            out.push_str(marks.trim_end());
            out.push('\n');
        }
        at += line.len() + 1;
    }
    out
}

#[test]
fn grid_tests_graph_rows_with_lanes_labels_and_faces() {
    let label = |kind, name: &str| Label {
        kind,
        name: name.into(),
    };
    let laid = text(vec![
        commit(
            1,
            &[2, 3],
            vec![label(Kind::Head, "HEAD"), label(Kind::Local, "main")],
            b"Merge side",
        ),
        commit(
            3,
            &[4],
            vec![label(Kind::Remote, "origin/side")],
            "s\u{202e}ide".as_bytes(),
        ),
        commit(2, &[4], vec![label(Kind::Tag, "v1")], b"main's"),
        commit(4, &[], Vec::new(), b"base"),
    ])
    .laid("Commits of main", false);
    insta::assert_snapshot!("git_graph_rows", grid(&laid));
    assert_eq!(laid.built.commit_at(0), None);
    assert_eq!(laid.built.commit_at(1), Some("1".repeat(40).as_str()));
    assert_eq!(laid.built.commit_at(4), Some("4".repeat(40).as_str()));
    assert_eq!(laid.built.commit_at(5), None);
}

#[test]
fn a_graph_cut_at_its_most_ends_with_a_note_and_pages_no_more() {
    let one = commit(1, &[], Vec::new(), b"x");
    let mut many = Text::default();
    for _ in 0..MAX_COMMITS {
        many.push(&[], &one, "2026-11-16");
    }
    let laid = many.laid("Commits", true);
    let shown = laid.text.text().to_string();
    assert!(shown.ends_with(&format!("{MORE_NOTE}\n")));
    // At its most, no page more is asked, whatever git has.
    assert!(!laid.built.more);
    let laid = text(vec![one]).laid("Commits", true);
    assert!(!laid.text.text().to_string().contains(MORE_NOTE));
    assert!(laid.built.more);
}

#[test]
fn a_row_s_labels_are_counted_past_their_most_and_names_escaped() {
    let labels: Vec<Label> = (0..MAX_LABELS + 3)
        .map(|n| Label {
            kind: Kind::Tag,
            name: format!("t{n}"),
        })
        .collect();
    let mut crafted = commit(1, &[], labels, b"a\nb\\c");
    crafted.author = b"x\ry".to_vec();
    let shown = text(vec![crafted])
        .laid("Commits", false)
        .text
        .text()
        .to_string();
    let row = shown.lines().nth(1).unwrap();
    assert!(row.contains(" +3 "), "{row}");
    assert!(!row.contains(&format!("t{}", MAX_LABELS)), "{row}");
    assert_eq!(shown.lines().count(), 2, "{shown:?}");
    assert!(!row.contains('\r'), "{row:?}");
}

#[test]
fn a_line_offers_its_branches_and_marks_order_oldest_first() {
    use super::super::Label;
    let labels = vec![
        Label {
            kind: Kind::Head,
            name: "HEAD".into(),
        },
        Label {
            kind: Kind::Local,
            name: "main".into(),
        },
        Label {
            kind: Kind::Remote,
            name: "origin/main".into(),
        },
        Label {
            kind: Kind::Tag,
            name: "v1".into(),
        },
        Label {
            kind: Kind::Local,
            name: "-x".into(),
        },
    ];
    let laid = text(vec![
        commit(1, &[2], labels, b"newest"),
        commit(2, &[3], Vec::new(), b"middle"),
        commit(3, &[], Vec::new(), b"oldest"),
    ])
    .laid("Commits", false);
    let built = &laid.built;
    let branches = built.line(1).unwrap().branches();
    assert_eq!(
        branches,
        [("main".to_owned(), false), ("origin/main".to_owned(), true)]
    );
    let id = |n: u8| format!("{n:x}").repeat(40);
    let marks = vec![id(1), id(3), id(2)];
    let ordered: Vec<String> = built
        .oldest_first(&marks)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ordered, [id(3), id(2), id(1)]);
    assert_eq!(
        built.range(&[id(1), id(3)]).as_deref(),
        Some(format!("{}..{}", id(3), id(1)).as_str())
    );
    assert_eq!(built.range(&[id(1)]), None);
    assert_eq!(built.range(&[id(1), "9".repeat(40)]), None);
}
