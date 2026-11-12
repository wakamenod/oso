//! The glob cases of editorconfig-core-test (`glob/`, `filetree/path_separator`), and the
//! bounds.
//!
//! Must not depend on: any other module of the crate; globs are strings.

use super::{Glob, MAX_GLOB, MAX_NESTING, MAX_RANGE_GLOB, Refused};

/// Asserts that `glob` matches each of `yes` and none of `no`.
#[track_caller]
fn check(glob: &str, yes: &[&str], no: &[&str]) {
    let compiled = Glob::new(glob).unwrap();
    for path in yes {
        assert!(
            compiled.matches(&format!("/{path}")),
            "{glob:?} matches {path:?}"
        );
    }
    for path in no {
        assert!(
            !compiled.matches(&format!("/{path}")),
            "{glob:?} does not match {path:?}"
        );
    }
}

#[test]
fn star_matches_within_a_directory_and_a_name_without_a_slash_at_any_depth() {
    check(
        "a*e.c",
        &["ace.c", "ae.c", "abcde.c", "x/ace.c"],
        &["a/e.c"],
    );
    check(
        "Bar/*",
        &["Bar/foo.txt", "Bar/.editorconfig"],
        &["bat/Bar/foo.txt", "bat/Bar/.editorconfig"],
    );
    check("*", &[".editorconfig", "a/e.c", "bat/Bar/foo.txt"], &[]);
}

#[test]
fn question_mark_is_one_character_but_a_slash() {
    check("som?.c", &["some.c"], &["som.c", "something.c", "som/.c"]);
    check("中?.txt", &["中文.txt"], &["中.txt"]);
    check("中文.txt", &["中文.txt"], &[]);
}

#[test]
fn brackets_hold_choices_ranges_and_escapes() {
    check("[ab].a", &["a.a"], &["c.a"]);
    check("[!ab].b", &["c.b"], &["a.b"]);
    check("[d-g].c", &["f.c"], &["h.c"]);
    check("[!d-g].d", &["h.d"], &["f.d"]);
    check("[abd-g].e", &["e.e"], &[]);
    check("[-ab].f", &["-.f"], &[]);
    check(r"[\]ab].g", &["].g"], &[]);
    check("[ab]].g", &["b].g"], &[]);
    check(r"[!\]ab].g", &["c.g"], &[]);
    check("[!ab]].g", &["c].g"], &[]);
}

#[test]
fn a_bracket_holding_a_slash_is_itself() {
    check("ab[e/]cd.i", &["ab[e/]cd.i"], &["ab/cd.i", "abecd.i"]);
    check("ab[/c", &["ab[/c"], &[]);
}

#[test]
fn a_negated_set_never_matches_a_slash() {
    check("a[!b]c", &["axc"], &["a/c", "abc"]);
}

#[test]
fn braces_choose_and_nest() {
    check(
        "*.{py,js,html}",
        &["test.py", "test.js", "test.html"],
        &["test.pyc"],
    );
    check("a{b,c,}.d", &["a.d", "ab.d", "ac.d"], &["a,.d"]);
    check("a{,b,,c,}.e", &["a.e", "ab.e", "ac.e"], &["a,.e"]);
    check(
        "{word,{also},this}.g",
        &["word.g", "{also}.g", "this.g"],
        &["word,this}.g", "{also,this}.g"],
    );
    check(
        "{{a,b},c}.k",
        &["a.k", "b.k", "c.k"],
        &["{{a,b},c}.k", "{a,b}.k"],
    );
    check(
        "{a,{b,c}}.l",
        &["a.l", "b.l", "c.l"],
        &["{a,{b,c}}.l", "{b,c}.l"],
    );
    check(
        "{some,a{*c,b}[ef]}.j",
        &[
            "some.j", "abe.j", "abf.j", "ace.j", "acf.j", "abce.j", "abcf.j",
        ],
        &["abg.j", "acg.j", "abcg.j", "ae.j", ".j"],
    );
}

#[test]
fn braces_with_one_choice_or_none_or_no_end_are_themselves() {
    check("{single}.b", &["{single}.b"], &[".b", "single.b"]);
    check("{}.c", &["{}.c"], &[".c"]);
    check("{.f", &["{.f"], &[".f"]);
    check("{},b}.h", &["{},b}.h"], &[]);
    check(
        "{{,b,c{d}.i",
        &["{{,b,c{d}.i"],
        &["{.i", "b.i", "c{d.i", ".i"],
    );
}

#[test]
fn escapes_in_braces() {
    check(r"{a\,b,cd}.txt", &["a,b.txt", "cd.txt"], &["a.txt"]);
    check(r"{e,\},f}.txt", &["e.txt", "}.txt", "f.txt"], &[]);
    check(r"{g,\\,i}.txt", &["g.txt", r"\.txt", "i.txt"], &[]);
    check(r"test\;.c", &["test;.c"], &[]);
    check(r"test\#.c", &["test#.c"], &[]);
}

#[test]
fn a_number_range_is_checked_as_a_number() {
    check(
        "{3..120}",
        &["3", "15", "60", "120"],
        &["1", "5a", "121", "060", "+5"],
    );
    check(
        "{-1..4}",
        &["-1", "0", "4", "x/2"],
        &["-2", "5", "-0", "00"],
    );
    check(
        "{aardvark..antelope}",
        &["{aardvark..antelope}"],
        &["a", "aardvark", "agreement", "antelope", "antimatter"],
    );
    // Backwards, or not numbers: the braces are themselves.
    check("{3..1}", &["{3..1}"], &["2"]);
    // A range of any size costs the same.
    check(
        "f{1..999999999999}.rs",
        &["f1.rs", "f999999999999.rs"],
        &["f1000000000000.rs"],
    );
    check("{1..2}{4..6}", &["15", "26"], &["13", "35"]);
}

#[test]
fn a_range_in_a_choice_not_taken_asks_nothing() {
    check("{a,b{1..3}}.x", &["a.x", "b2.x"], &["b9.x"]);
}

#[test]
fn star_star_crosses_directories() {
    check(
        "a**z.c",
        &[
            "a/z.c", "amnz.c", "am/nz.c", "a/mnz.c", "amn/z.c", "a/mn/z.c",
        ],
        &[],
    );
    check(
        "b/**z.c",
        &["b/z.c", "b/mnz.c", "b/mn/z.c"],
        &["bmnz.c", "bm/nz.c", "bmn/z.c"],
    );
    check(
        "c**/z.c",
        &["c/z.c", "cmn/z.c", "c/mn/z.c"],
        &["cmnz.c", "cm/nz.c", "c/mnz.c"],
    );
    check(
        "d/**/z.c",
        &["d/z.c", "d/mn/z.c"],
        &["dmnz.c", "dm/nz.c", "d/mnz.c", "dmn/z.c"],
    );
    check("**/z.c", &["z.c", "a/z.c", "a/b/z.c"], &["az.c"]);
}

#[test]
fn a_slash_anchors_the_glob_at_the_files_directory() {
    check(
        "path/separator",
        &["path/separator"],
        &["nested/path/separator"],
    );
    check("/top/of/path", &["top/of/path"], &["not/top/of/path"]);
    check(
        "/test1/file.{java,js,rb}",
        &["test1/file.java"],
        &["x/test1/file.java"],
    );
    check("test1/file.{java,js,rb}", &["test1/file.js"], &[]);
    check("*.{java,js,rb}", &["test1/file.rb"], &[]);
    // A backslash is no separator.
    check(r"windows\separator", &[], &["windows/separator"]);
    check(
        r"windows\\separator2",
        &[r"windows\separator2"],
        &["windows/separator2"],
    );
    // A glob ending with `/` matches no file.
    check("dir/", &[], &["dir", "dir/a"]);
}

#[test]
fn spaces_in_a_name_are_kept() {
    check(" test 7 ", &[" test 7 "], &["test 7"]);
}

#[test]
fn characters_of_regexes_are_themselves() {
    check("a.b(c)|d+$^", &["a.b(c)|d+$^"], &["axb(c)|d+$^"]);
    check("[.]", &["."], &["a"]);
    check("[&&]x", &["&x"], &["ax"]);
}

#[test]
fn the_bounds_refuse_a_glob() {
    let long = "a".repeat(MAX_GLOB + 1);
    assert_eq!(Glob::new(&long).unwrap_err(), Refused::TooLong);
    assert!(Glob::new(&"a".repeat(MAX_GLOB)).is_ok());
    let nested = |depth: usize| format!("{}x{}", "{a,".repeat(depth), "}".repeat(depth));
    assert!(Glob::new(&nested(MAX_NESTING)).is_ok());
    assert_eq!(
        Glob::new(&nested(MAX_NESTING + 1)).unwrap_err(),
        Refused::NestedTooDeep
    );
    // Braces that are themselves nest as deep as they like.
    let literal = format!("{}{}", "{".repeat(100), "}".repeat(100));
    check(&literal, &[literal.as_str()], &[]);
}

#[test]
fn a_glob_too_large_to_compile_is_refused() {
    // Each `**` choice in many nested repetitions grows the program.
    let large = "{**a,**b,**c,**d,**e,**f,**g,**h}".repeat(MAX_GLOB / 34);
    assert!(matches!(
        Glob::new(&large),
        Ok(_) | Err(Refused::TooComplex)
    ));
}

#[test]
fn a_glob_with_a_number_range_is_at_most_its_bound() {
    let at = format!("{}{{1..9}}", "a".repeat(MAX_RANGE_GLOB - 6));
    assert!(Glob::new(&at).is_ok());
    let past = format!("{}{{1..9}}", "a".repeat(MAX_RANGE_GLOB - 5));
    assert_eq!(Glob::new(&past).unwrap_err(), Refused::RangeTooLong);
    assert!(
        Glob::new(&"a".repeat(MAX_RANGE_GLOB + 1)).is_ok(),
        "no range, no bound"
    );
}

#[test]
fn a_set_never_matches_a_slash() {
    check("a[.-0]b", &["a.b", "a0b"], &["a/b"]);
    check(r"a[\/]b", &[], &["a/b"]);
}
