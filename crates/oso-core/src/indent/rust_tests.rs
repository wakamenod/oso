//! Tests of Rust's indentation rules (ADR 0115): each case the ADR names, written as rustfmt
//! writes it, reindents to itself; and the new line `RET` would start.
//!
//! Must not depend on: `buffer` or `editor`; they run on ropes alone.

use super::tests::{misindented, new_lines, reindented};
use super::{Answer, Settings, rust};

/// Asserts that every non-blank line of `text` the rules answer a column for is indented
/// as written.
#[track_caller]
fn fixed(text: &str) {
    assert_eq!(misindented(rust, text), [], "{text}");
}

#[test]
fn blocks_and_closers_go_by_their_openers_line() {
    fixed("fn main() {\n    let x = 1;\n    if x {\n        y();\n    }\n}\n");
    fixed(
        "fn f() {\n    if a {\n        b\n    } else if c {\n        d\n    } else {\n        e\n    }\n}\n",
    );
    fixed("pub fn f(\n    a: u32,\n    b: u32,\n) -> Result<u32, Error> {\n    a\n}\n");
    fixed("fn f() {\n    foo(\n        a,\n        [\n            1,\n        ],\n    );\n}\n");
}

#[test]
fn a_statement_that_goes_on_is_one_level_in_once() {
    fixed(
        "fn f() {\n    let x =\n        compute(1);\n    let y = a\n        + b\n        + c;\n}\n",
    );
    // A chain stays one level in, however long.
    fixed(
        "fn f() {\n    let v = items\n        .iter()\n        .map(f)\n        .filter(g)\n        .collect();\n}\n",
    );
    // A chain after a bracket the statement closed stays where the closer is.
    fixed("fn f() {\n    let x = foo(\n        a,\n    )\n    .bar()\n    .baz();\n}\n");
    fixed("fn f() {\n    spawn(async move {\n        work();\n    })\n    .await;\n}\n");
    fixed(
        "fn f() {\n    let v = items\n        .iter()\n        .map(|x| {\n            x + 1\n        })\n        .collect();\n}\n",
    );
}

#[test]
fn match_arms_take_their_body_one_level_in() {
    fixed(concat!(
        "fn f() {\n    match x {\n        A => 1,\n        B => {\n            2\n        }\n",
        "        Some(x) => x\n            .foo()\n            .bar(),\n        C =>\n            long_call(),\n    }\n}\n",
    ));
}

#[test]
fn where_clauses_sit_at_the_item_and_bounds_one_level_in() {
    fixed("impl<T> Foo for T\nwhere\n    T: Bar,\n    U: Baz,\n{\n    fn f() {}\n}\n");
    fixed(concat!(
        "impl X {\n    fn f<T>(t: T) -> u32\n    where\n        T: Into<u32>,\n        T: Copy,\n",
        "    {\n        t.into()\n    }\n}\n",
    ));
}

#[test]
fn attributes_end_their_line() {
    fixed(
        "#[derive(Debug)]\nstruct A;\n\n#[cfg_attr(\n    test,\n    derive(Clone)\n)]\nstruct B;\n",
    );
    fixed(
        "fn f() {\n    #[expect(clippy::x, reason = \"y\")]\n    let a = 1;\n    #![allow(x)]\n    b();\n}\n",
    );
}

#[test]
fn strings_keep_and_raw_strings_close_on_their_hashes() {
    let text = "fn f() {\n    let s = r##\"a \"# b\nfn not_code() {\n\"##;\n    let t = \"x\n  y\";\n    z();\n}\n";
    let answers = reindented(rust, text);
    assert_eq!(
        answers,
        [
            Some(0),
            Some(4),
            None,
            None,
            Some(4),
            None,
            Some(4),
            Some(0),
            Some(0)
        ]
    );
    fixed(
        "fn f() {\n    let a = br##\"}\"#\"##;\n    let b = c\"{\";\n    let c = b\"\\\"{\";\n    d();\n}\n",
    );
}

#[test]
fn chars_are_told_from_lifetimes() {
    fixed(
        "fn f<'a>(x: &'a str) -> char {\n    let c = '{';\n    let d = '\\'';\n    let e = b'(';\n    'outer: loop {\n        break 'outer;\n    }\n    '\\u{7b}'\n}\n",
    );
    fixed("struct S<'a> {\n    a: &'a [u8],\n    b: &'static str,\n}\n");
}

#[test]
fn block_comments_nest_and_their_lines_follow_the_opener() {
    let text = "fn f() {\n    /* a /* b */\n     * c\n  d */\n    x();\n}\n";
    assert_eq!(
        reindented(rust, text),
        [
            Some(0),
            Some(4),
            Some(5),
            Some(5),
            Some(4),
            Some(0),
            Some(0)
        ]
    );
    // A line comment holds no brackets.
    fixed("fn f() {\n    // {\n    x();\n}\n");
}

#[test]
fn a_new_line_follows_the_line_it_leaves() {
    let cases: [(&str, &[usize], &[usize]); 5] = [
        ("fn f() {", &[8], &[4]),
        ("fn f() {}", &[8], &[0]),
        ("fn f() {\n    let x =", &[20], &[8]),
        ("    foo(a, b)", &[8, 11], &[8, 8]),
        ("fn f() {\n    match x {", &[22], &[8]),
    ];
    for (text, offsets, expected) in cases {
        let columns: Vec<_> = new_lines(rust, text, offsets)
            .into_iter()
            .map(|answer| match answer {
                Answer::Column(column) => column,
                Answer::Keep => usize::MAX,
            })
            .collect();
        assert_eq!(columns, expected, "{text:?} at {offsets:?}");
    }
}

#[test]
fn a_line_split_by_an_earlier_new_line_is_the_openers_line() {
    // `RET` after both openers: the second opener's line is the new one, at 4.
    let answers = new_lines(rust, "foo(bar(x))", &[4, 8]);
    assert_eq!(answers, [Answer::Column(4), Answer::Column(8)]);
}

#[test]
fn indent_size_is_the_settings() {
    let settings = Settings::new(2, super::Style::Spaces, 8).unwrap();
    let rope = crate::rope::Rope::from("fn f() {\nx\n}\n");
    let lines = [super::Line::Row(1), super::Line::Row(2)];
    let answers = rust(&rope, settings, &lines, &mut super::Budget::new());
    assert_eq!(answers, [Answer::Column(2), Answer::Column(0)]);
}

#[test]
fn operands_sit_together_and_a_chain_under_one_is_a_level_further() {
    fixed(concat!(
        "fn f() {\n    if let Some(q) = asking\n        && self\n            .editor\n",
        "            .prompt()\n            .is_some()\n        && done\n    {\n        quit();\n    }\n}\n",
    ));
    // An operand that starts after the opener on its line: the rest sit one level past it.
    fixed(concat!(
        "fn f() {\n    let p = from == last\n        && ((gap && row == last)\n",
        "            || (below && row == last + 1));\n}\n",
    ));
    // After a call that closes over lines, the next operand sits with the one it closes.
    fixed(concat!(
        "fn f() {\n    word(a)\n        && charged(left, |budget| {\n            may(budget)\n        })\n",
        "        && charged(right, |budget| {\n            may(budget)\n        })\n}\n",
    ));
}

#[test]
fn a_guard_and_its_operands_go_one_level_past_the_pattern() {
    fixed(concat!(
        "fn f() {\n    match x {\n        Err(e)\n            if e.is_not_found()\n",
        "                && new\n                && last =>\n        {\n            make();\n        }\n",
        "        _ => {}\n    }\n}\n",
    ));
    fixed(
        "fn f() {\n    match x {\n        A\n        | B\n        | C => 1,\n        D => 2,\n    }\n}\n",
    );
}

#[test]
fn a_brace_closed_on_its_own_line_ends_no_statement_unless_empty() {
    fixed(
        "fn f() {\n    let s = unsafe { make(1) }\n        .expect(\"made\");\n    let k = A { b: 1 }\n        && c;\n}\n",
    );
    fixed(
        "fn f() {\n    match x {\n        A => {}\n        B => {}\n    }\n}\nimpl X for Y {}\nstruct Z;\n",
    );
}

#[test]
fn a_chain_after_a_block_or_a_let_else_sits_with_its_closer() {
    fixed(concat!(
        "fn f() {\n    let a = match b {\n        C => 1,\n    }\n    .max(2)\n    .min(3);\n",
        "    let Ok(id) =\n        self.editor\n            .ask(label)\n    else {\n        return;\n    };\n}\n",
    ));
    fixed(
        "fn f() -> R {\n    Ok(entries\n        .into_iter()\n        .filter(g)\n        .collect())\n}\n",
    );
}

#[test]
fn a_value_after_a_line_that_ends_with_an_equals_sign_keeps_the_operands_place() {
    fixed(concat!(
        "fn f() {\n    if depth == 0\n        && let Some(x) =\n            (root.get(1), root.get(2))\n",
        "        && x.is_ok()\n    {\n        g();\n    }\n}\n",
    ));
}
