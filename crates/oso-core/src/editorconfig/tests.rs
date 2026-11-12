//! The parser's and the properties' cases of editorconfig-core-test (`parser/`,
//! `properties/`, `filetree/`), the bounds, and a property test on random input.
//!
//! Must not depend on: any other module of the crate; files are text here.

use proptest::prelude::*;

use super::{
    Budget, File, IndentSize, IndentStyle, MAX_FILE, MAX_GLOB, MAX_SECTIONS, Problem, Properties,
    VISIT_GLOBS, VISIT_SECTIONS,
};

/// `text` parsed with a visit's whole budget.
fn parse(text: &str) -> File {
    File::parse(text, &mut Budget::default())
}

/// The properties `files`, farthest first, give the file at the paths from each of their
/// directories.
fn applied(files: &[(&str, &str)]) -> Properties {
    let mut properties = Properties::default();
    for (source, (text, path)) in files.iter().enumerate() {
        properties.apply(&parse(text), source, path);
    }
    properties
}

/// `(indent_style, indent_size, tab_width)` of `text` for `path`, without their sources.
fn three(text: &str, path: &str) -> (Option<IndentStyle>, Option<IndentSize>, Option<u8>) {
    let properties = applied(&[(text, path)]);
    (
        properties.indent_style().map(|(style, _)| style),
        properties.indent_size().map(|(size, _)| size),
        properties.tab_width().map(|(width, _)| width),
    )
}

#[test]
fn later_sections_win_and_repeat_sections_add_up() {
    let text = "[*.a]\nindent_size=2\n[*.a]\ntab_width=3\n\
                [*.b]\nindent_size = 4\ntab_width = 5\n[b.b]\ntab_width = 6\n[*.b]\nindent_size = 7\n";
    assert_eq!(
        three(text, "a.a"),
        (None, Some(IndentSize::Columns(2)), Some(3))
    );
    assert_eq!(
        three(text, "b.b"),
        (None, Some(IndentSize::Columns(7)), Some(6))
    );
}

#[test]
fn whitespace_comments_and_case() {
    let text = "  root = TrUe  \n\
                ; a comment\n\
                # another\n\
                \t[ *.c ]  \n\
                \x20  INDENT_STYLE   =   Space  \n\
                not a pair\n\
                Indent_Size=TAB\n\
                tab_width = 4 ; not a comment\n";
    let file = parse(text);
    assert!(file.is_root());
    // The section's name keeps its inner spaces: ` *.c ` matches ` x.c `.
    assert_eq!(
        three(text, " x.c "),
        (Some(IndentStyle::Space), Some(IndentSize::Tab), None),
        "a value with a `;` in it is not a width"
    );
    assert_eq!(three(text, "x.c"), (None, None, None));
}

#[test]
fn a_comment_after_a_section_header_is_passed_over() {
    let text = "[*.rs] ; rust\nindent_size = 2\n[*.md]   # docs\nindent_size = 3\n";
    assert_eq!(three(text, "a.rs").1, Some(IndentSize::Columns(2)));
    assert_eq!(three(text, "a.md").1, Some(IndentSize::Columns(3)));
    // With no space before it, a `;` is part of the name, as `[test\;.c]` is.
    assert_eq!(three("[a.rs];x\nindent_size = 2\n", "a.rs").1, None);
}

#[test]
fn root_counts_only_in_the_preamble() {
    assert!(!parse("[*]\nroot = true\n").is_root());
    assert!(!parse("root = false\n").is_root());
    assert!(parse("\u{feff}root=true\r\n[*]\r\nindent_size=2\r\n").is_root());
    assert_eq!(
        three("\u{feff}root=true\r\n[*]\r\nindent_size=2\r\n", "a"),
        (None, Some(IndentSize::Columns(2)), Some(2)),
        "a BOM and CRLF lines are read"
    );
}

#[test]
fn pairs_before_any_section_set_nothing() {
    assert_eq!(three("indent_size = 2\n[*]\n", "a.c"), (None, None, None));
}

#[test]
fn empty_files_and_files_of_comments_set_nothing() {
    for text in ["", "\n\n", "# a\n; b\n", "\n# a\n\n"] {
        let file = parse(text);
        assert!(!file.is_root());
        assert!(file.problems().is_empty());
        assert_eq!(three(text, "test4"), (None, None, None));
    }
}

#[test]
fn tab_width_defaults_to_a_numeric_indent_size() {
    let text = "root = true\n[test.c]\nindent_style = space\nindent_size = 4\n\
                [test2.c]\nindent_style = tab\nindent_size = tab\n";
    assert_eq!(
        three(text, "test.c"),
        (
            Some(IndentStyle::Space),
            Some(IndentSize::Columns(4)),
            Some(4)
        )
    );
    assert_eq!(
        three(text, "test2.c"),
        (Some(IndentStyle::Tab), Some(IndentSize::Tab), None)
    );
}

#[test]
fn indent_style_tab_makes_indent_size_tab() {
    let text = "root = true\n[test.c]\nindent_style = tab\n[test2.c]\nindent_style = space\n\
                [test3.c]\nindent_style = tab\ntab_width = 2\n";
    assert_eq!(
        three(text, "test.c"),
        (Some(IndentStyle::Tab), Some(IndentSize::Tab), None)
    );
    assert_eq!(
        three(text, "test2.c"),
        (Some(IndentStyle::Space), None, None)
    );
    assert_eq!(
        three(text, "test3.c"),
        (Some(IndentStyle::Tab), Some(IndentSize::Tab), Some(2)),
        "the tab is two columns, so a level is too"
    );
}

#[test]
fn widths_are_taken_from_1_to_32() {
    for (value, taken) in [
        ("1", Some(1)),
        ("32", Some(32)),
        ("0", None),
        ("33", None),
        ("+4", None),
        ("4.0", None),
        ("", None),
        ("999999999999", None),
    ] {
        let text = format!("[*]\ntab_width = {value}\n");
        assert_eq!(three(&text, "a").2, taken, "tab_width = {value}");
    }
}

#[test]
fn an_invalid_value_hides_a_farther_one() {
    let far = ("[*]\nindent_size = 2\nindent_style = tab\n", "d/a.rs");
    let near = ("[*]\nindent_size = 99\nindent_style = sideways\n", "a.rs");
    let properties = applied(&[far, near]);
    assert_eq!(properties.indent_size(), None);
    assert_eq!(properties.indent_style(), None);
    assert_eq!(
        properties.tab_width(),
        None,
        "derived from the invalid size"
    );
}

#[test]
fn nearer_files_win_and_each_value_names_its_file() {
    let parent = (
        "root = true\n[test.b]\nindent_size = 2\n[test.c]\ntab_width = 3\n[*.d]\nindent_style = tab\n",
        "child/test.b",
    );
    let child = (
        "[test.b]\nindent_style = space\n[*]\ntab_width = 5\n",
        "test.b",
    );
    let properties = applied(&[parent, child]);
    assert_eq!(properties.indent_size(), Some((IndentSize::Columns(2), 0)));
    assert_eq!(properties.indent_style(), Some((IndentStyle::Space, 1)));
    assert_eq!(properties.tab_width(), Some((5, 1)));
}

#[test]
fn unset_takes_a_value_away_and_what_it_gave() {
    let far = (
        "[*]\nindent_size = 2\nindent_style = tab\ntab_width = 4\n",
        "unset/a.txt",
    );
    let near = ("[*]\nindent_size = unset\ntab_width = UNSET\n", "a.txt");
    let properties = applied(&[far, near]);
    assert_eq!(
        properties.indent_size(),
        Some((IndentSize::Tab, 0)),
        "indent_style = tab still makes it tab"
    );
    assert_eq!(properties.tab_width(), None);
    let near = ("[*]\nindent_style = unset\nindent_size = unset\n", "a.txt");
    let properties = applied(&[far, near]);
    assert_eq!(properties.indent_size(), None);
    assert_eq!(properties.tab_width(), Some((4, 0)));
}

#[test]
fn a_root_file_is_only_marked_as_one() {
    // The search stopping is the host's; a root file is only marked.
    let child = parse("root = true\n");
    assert!(child.is_root());
    assert_eq!(
        applied(&[("root = true\n", "test.a")]),
        Properties::default()
    );
}

#[test]
fn other_properties_are_passed_over() {
    let text = "[*]\ncharset = utf-8\nend_of_line = lf\ntrim_trailing_whitespace = true\n\
                insert_final_newline = true\nmax_line_length = 80\nindent_size = 3\n";
    assert_eq!(
        three(text, "a"),
        (None, Some(IndentSize::Columns(3)), Some(3))
    );
}

#[test]
fn a_file_too_large_is_not_read() {
    let mut text = "[*]\nindent_size = 2\n".to_owned();
    text.push_str(&"#".repeat(MAX_FILE + 1 - text.len()));
    let file = parse(&text);
    assert_eq!(file.problems(), [Problem::TooLarge]);
    assert_eq!(three(&text, "a"), (None, None, None));
    text.pop();
    assert!(parse(&text).problems().is_empty());
    assert_eq!(three(&text, "a").1, Some(IndentSize::Columns(2)));
}

#[test]
fn sections_past_the_limit_are_skipped() {
    let mut text = String::new();
    for _ in 0..MAX_SECTIONS {
        text.push_str("[x]\n");
    }
    text.push_str("[*]\nindent_size = 2\n[y]\nindent_size = 3\n");
    let file = parse(&text);
    assert_eq!(
        file.problems(),
        [Problem::TooManySections {
            line: MAX_SECTIONS + 1
        }]
    );
    assert_eq!(three(&text, "a").1, None);
}

#[test]
fn a_section_with_a_bad_glob_is_skipped_and_said() {
    let long = "a".repeat(MAX_GLOB + 1);
    let deep = format!("{}x{}", "{a,".repeat(9), "}".repeat(9));
    let text = format!(
        "[*]\nindent_size = 2\n[{long}]\nindent_size = 3\n[{deep}]\nindent_size = 4\n\
         [{long}]\ncharset = utf-8\n"
    );
    let file = parse(&text);
    assert_eq!(
        file.problems(),
        [
            Problem::GlobTooLong { line: 3 },
            Problem::NestedTooDeep { line: 5 }
        ],
        "a section that sets nothing kept is never compiled, so never refused"
    );
    assert_eq!(three(&text, "a").1, Some(IndentSize::Columns(2)));
    assert_eq!(
        Problem::GlobTooLong { line: 3 }.to_string(),
        "line 3: a section name longer than 4 KiB is skipped"
    );
}

/// `count` sections of `glob`, each setting `indent_size` to its number from 1, cycling
/// through 1 to 32.
fn sections(glob: &str, count: usize) -> String {
    (0..count)
        .map(|n| format!("[{glob}]\nindent_size = {}\n", n % 32 + 1))
        .collect()
}

#[test]
fn a_visit_compiles_at_most_its_sections() {
    let mut text = sections("*", VISIT_SECTIONS);
    text.push_str("[*]\nindent_size = 3\n[*]\ncharset = utf-8\n[*]\nindent_size = 5\n");
    let file = parse(&text);
    let past = 2 * VISIT_SECTIONS + 1;
    assert_eq!(file.problems(), [Problem::OverBudget { line: past }]);
    let last = u8::try_from((VISIT_SECTIONS - 1) % 32 + 1).unwrap();
    assert_eq!(three(&text, "a").1, Some(IndentSize::Columns(last)));
    assert_eq!(
        Problem::OverBudget { line: 3 }.to_string(),
        "line 3: a visit compiles at most 128 sections and 2 KiB of globs; \
         this section and the rest are skipped"
    );
}

#[test]
fn a_visit_compiles_at_most_its_bytes_of_globs() {
    let glob = "a".repeat(VISIT_GLOBS / 2);
    let fits = format!("{}[*]\nindent_size = 7\n", sections(&glob, 2));
    let mut budget = Budget::default();
    assert!(File::parse(&fits, &mut budget).problems() == [Problem::OverBudget { line: 5 }]);
    let exact = format!(
        "{}{}",
        sections(&"a".repeat(VISIT_GLOBS - 1), 1),
        sections("*", 1)
    );
    assert!(parse(&exact).problems().is_empty(), "2 KiB in all fits");
}

#[test]
fn the_files_of_a_walk_share_the_budget_and_say_it_once() {
    let mut budget = Budget::default();
    let near = File::parse(&sections("*", VISIT_SECTIONS - 1), &mut budget);
    assert!(near.problems().is_empty());
    let far = File::parse(&sections("*.rs", 3), &mut budget);
    assert_eq!(far.problems(), [Problem::OverBudget { line: 3 }]);
    let farther = File::parse(&sections("*", 1), &mut budget);
    assert!(farther.problems().is_empty(), "said once a visit");
    let mut properties = Properties::default();
    properties.apply(&farther, 2, "a.rs");
    properties.apply(&far, 1, "a.rs");
    assert_eq!(properties.indent_size(), Some((IndentSize::Columns(1), 1)));
}

#[test]
fn a_glob_too_long_spends_nothing() {
    let long = "a".repeat(MAX_GLOB + 1);
    let text = format!("{}{}", sections(&long, 1), sections("*", VISIT_SECTIONS));
    assert_eq!(parse(&text).problems(), [Problem::GlobTooLong { line: 1 }]);
}

#[test]
fn a_walk_applies_farthest_first_with_each_files_globs_from_its_directory() {
    use std::path::{Path, PathBuf};

    let near = parse("[*.rs]\nindent_size = 2\n");
    let far = parse("root = true\n[src/*.rs]\nindent_style = tab\nindent_size = 8\n");
    let files = vec![
        (PathBuf::from("/p/src/.editorconfig"), near),
        (PathBuf::from("/p/.editorconfig"), far),
    ];
    let applied = super::Applied::from_walk(Path::new("/p/src/a.rs"), files);
    let properties = applied.properties();
    assert_eq!(properties.indent_size(), Some((IndentSize::Columns(2), 0)));
    assert_eq!(properties.indent_style(), Some((IndentStyle::Tab, 1)));
    assert_eq!(applied.file(1), Some(Path::new("/p/.editorconfig")));
    assert_eq!(applied.file(2), None);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Any text within the bounds parses, and any path matches or not, without a panic.
    #[test]
    fn any_file_and_path_parse_and_match(
        sections in prop::collection::vec(
            ("[*?{}\\[\\]!,.a-c/\\\\0-9-]{0,40}", "[a-z_ =0-9]{0,20}"),
            0..20,
        ),
        path in "[a-c./{}0-9-]{0,20}",
    ) {
        let mut text = String::from("root = true\n");
        for (glob, pair) in &sections {
            text.push_str(&format!("[{glob}]\n{pair}\nindent_size = 2\n"));
        }
        let file = parse(&text);
        let mut properties = Properties::default();
        properties.apply(&file, 0, &path);
        let _ = (properties.indent_size(), properties.tab_width(), properties.indent_style());
    }

    /// A glob of any characters, up to the bound, compiles or is refused.
    #[test]
    fn any_glob_compiles_or_is_refused(glob in "\\PC{0,64}", path in "\\PC{0,32}") {
        let text = format!("[{glob}]\nindent_size = 2\n");
        let file = parse(&text);
        let mut properties = Properties::default();
        properties.apply(&file, 0, &path);
    }

    /// A name with no character a glob reads matches itself alone, as `regex`'s own
    /// characters are taken as themselves.
    #[test]
    fn a_plain_name_matches_only_itself(
        name in "[^*?\\[\\]{}\\\\/\\n\\r]{1,24}",
        other in "[^*?\\[\\]{}\\\\/\\n\\r]{1,24}",
    ) {
        let name = name.trim();
        prop_assume!(!name.is_empty() && !name.starts_with(['#', ';']));
        let text = format!("[{name}]\nindent_size = 2\n");
        let matches = |path: &str| {
            let mut properties = Properties::default();
            properties.apply(&parse(&text), 0, path);
            properties.indent_size().is_some()
        };
        prop_assert!(matches(name));
        let below = format!("d/{name}");
        prop_assert!(matches(&below));
        prop_assert_eq!(matches(&other), other == name);
    }
}
