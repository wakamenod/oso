//! A reply's Markdown as drawn: which marks go and what stays, the faces left, that a reply
//! drawn as it streams is the one drawn whole, and what Claude writes passing for nothing.
//!
//! Must not depend on: rows, the syntax threads or the theme.

use proptest::prelude::*;

use super::signs::could_be_mark;
use super::{LineStart, MAX_MARKED, MAX_QUOTES, MAX_RULES, Style, walk};
use crate::sessions::native::render::Face;

/// `text` as drawn, whole: each line's text, its breaks kept.
fn shown(text: &str) -> String {
    let mut out = String::new();
    walk(text, LineStart::default(), false, |pieces| {
        out.extend(pieces.iter().map(|piece| piece.text));
        out.push('\n');
    });
    out
}

/// The pieces of `text` with a face, whole.
fn faces(text: &str) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    walk(text, LineStart::default(), false, |pieces| {
        for piece in pieces {
            out.extend(piece.style.map(|style| (piece.text.to_owned(), style)));
        }
    });
    out
}

/// Each of `cases` is drawn as written.
fn stays(cases: &[&str]) {
    for case in cases {
        assert_eq!(shown(case), format!("{case}\n"));
    }
}

#[test]
fn marks_are_hidden_and_leave_a_face() {
    assert_eq!(
        shown("## 原因\n\n`redirect()` は値を**検証せずに**そのまま*入れて*いました。"),
        "原因\n\nredirect() は値を検証せずにそのまま入れていました。\n"
    );
    assert_eq!(
        faces("# A `b` **c**\n**d `e`** *f*"),
        [
            ("A ", Style::Heading),
            ("b", Style::Code),
            (" ", Style::Heading),
            ("c", Style::Heading),
            ("d ", Style::Strong),
            ("e", Style::Code),
            ("f", Style::Strong),
        ]
        .map(|(text, style)| (text.to_owned(), style))
    );
    assert_eq!(shown("***both*** and `` a`b ``"), "both and a`b\n");
    // A Japanese bracket inside the marks, a letter outside.
    assert_eq!(shown("**「安全」**です"), "「安全」です\n");
}

#[test]
fn bullets_quotes_and_rules_are_drawn() {
    assert_eq!(
        shown("- a\n  * b\n  + **c**\n1. d\n2) e\n- [x] f"),
        "• a\n  • b\n  + c\n1. d\n2) e\n• [x] f\n"
    );
    assert_eq!(shown("> a **b**\n>\n>> - c\n"), "│ a b\n│\n│ │ • c\n");
    let rule = "─".repeat(32);
    assert_eq!(
        shown("a\n\n---\n\n* * *\n"),
        format!("a\n\n{rule}\n\n{rule}\n")
    );
    // Below text, it is a heading's underline.
    stays(&["a\n---", "a\n==="]);
}

#[test]
fn fences_go_and_code_is_kept_as_it_is() {
    assert_eq!(
        shown("a:\n```rust\nlet *x = **y;\n\n    - `z`\n```\nb"),
        "a:\n\nlet *x = **y;\n\n    - `z`\n\nb\n"
    );
    // An empty line is there already.
    assert_eq!(shown("a\n\n~~~\nb\n~~~\n\nc"), "a\n\nb\n\nc\n");
    assert_eq!(shown("```\na\n```\n```\nb\n```"), "a\n\nb\n");
    assert_eq!(shown("````md\n```\n*a*\n```\n````"), "```\n*a*\n```\n");
    // In a list, at its indent, and one that never ends.
    assert_eq!(
        shown("- a\n\n      ```sh\n      *b*\n      ```"),
        "• a\n\n      *b*\n"
    );
    assert_eq!(shown("```\n*a*"), "*a*\n");
    assert_eq!(faces("```\na\n```"), [("a".to_owned(), Style::Code)]);
    // Indented code; in a list, more of the item.
    stays(&["    *a*\n\n    - **b**"]);
    assert_eq!(shown("- a\n\n    **b**"), "• a\n\n    b\n");
}

#[test]
fn what_could_be_something_else_stays() {
    stays(&[
        "2 * 3 * 4, 2*3*4, a*b",
        "*.rs and *.toml, src/**/mod.rs, src/*",
        "f(*args, **kwargs)",
        "\"Action\": \"*\", \"Resource\": \"*\" and \"**\", \"**\"",
        "Allow Bash(*) and Edit(*)",
        "SELECT *, count(*) FROM t",
        "rm -rf *_old* *~ *.bak*",
        "https://a.b?x=*y* and https://a.b/**c** https://a.b/é*x*",
        "https://a.b/(v)/(*x*)",
        "[a](https://b.c/(*d*)) ![x](https://a.b?*c*)",
        "+ a",
        "\u{2028}",
        "e`\u{301}` a",
        "字*\u{180b}a*",
        "👩\u{200d}*👧*",
        "snake_case_name __init__ _a_ ~~not~~",
        r"\*a\* \`b\` \[c](d)",
        "an `open tick and **open",
        "a **** b",
        "#1 and #",
        "- ",
        "-a +b 1.2",
        "![image](https://a.b/c.png) [a][b] [c]",
        "[a]: https://b.c",
        "<details><summary>**a**</summary>",
        "<!-- **a** -->",
        "| `a` | **b** |\n|---|---|\n| *c* | d |",
        "> ```\n> **a** `b`\n> ```",
        "- ```\n  **a**\n  ```",
        "[]() [](https://a.b) [ ](https://a.b)",
    ]);
    // A fence whose info string says more than a language.
    stays(&[
        "``` run this: rm\n*a*\n```",
        "```averyveryverylongword\n*a*\n```",
    ]);
}

#[test]
fn a_link_hides_its_target_only_behind_plain_words() {
    let hidden = |text: &str, drawn: &str| assert_eq!(shown(text), format!("{drawn}\n"));
    hidden(
        "背景は [OWASP の説明](https://owasp.org/) にあります。",
        "背景は OWASP の説明 にあります。",
    );
    hidden("[**the** `docs`](https://a.b/c).", "the docs.");
    hidden(
        "1. 仕様は「[仕様書](https://a.b/c)」を参照。次に進みます。",
        "1. 仕様は「仕様書」を参照。次に進みます。",
    );
    hidden(
        "- **[Docs](https://a.b/c)**: all of it",
        "• Docs: all of it",
    );
    hidden("[https://a.b/](https://a.b/)", "https://a.b/");
    for (text, drawn) in [
        // The text could be an address, or be read as one.
        ("[owasp.org](https://a.b/)", "owasp.org (https://a.b/)"),
        (
            "[https://a.b/](https://c.d/)",
            "https://a.b/ (https://c.d/)",
        ),
        ("[owasp。org](https://a.b/)", "owasp。org (https://a.b/)"),
        ("[owasp․org](https://a.b/)", "owasp․org (https://a.b/)"),
        ("[owaspꓸorg](https://a.b/)", "owaspꓸorg (https://a.b/)"),
        ("[a\u{200b}b](https://a.b/)", "[a\u{200b}b](https://a.b/)"),
        ("[a\u{202e}b](https://a.b/)", "[a\u{202e}b](https://a.b/)"),
        (
            "[https://a.b/c](https://a.b/*c*)",
            "https://a.b/c (https://a.b/*c*)",
        ),
        (
            "[https://a.b?*c*](https://a.b?*c*)",
            "https://a.b?*c* (https://a.b?*c*)",
        ),
        // The target is no web address: a path, or words.
        ("[`a.rs`](src/a.rs#L1)", "a.rs (src/a.rs#L1)"),
        ("[Done](I-deleted-it)", "Done (I-deleted-it)"),
        ("[Done](javascript:x)", "Done (javascript:x)"),
    ] {
        hidden(text, drawn);
    }
    // A title, escapes and spaces: as written.
    stays(&[
        "[a](https://b.c \"title\")",
        "[a](https://b.c/(d))",
        "[a](<https://b.c>)",
        "[a](https://b.c",
    ]);
}

/// Links whose target shows for what is around them, and what is drawn.
const AROUND: &[(&str, &str)] = &[
    (
        "[OWASP](https://a.b/) (https://owasp.org/)",
        "OWASP (https://a.b/) (https://owasp.org/)",
    ),
    (
        "[OWASP](https://a.b/)❨owasp.org❩",
        "OWASP (https://a.b/)❨owasp.org❩",
    ),
    (
        "[OWASP](https://a.b/) owasp.org",
        "OWASP (https://a.b/) owasp.org",
    ),
    (
        "[OWASP](https://a.b/) https://owasp.org/",
        "OWASP (https://a.b/) https://owasp.org/",
    ),
    (
        "[OWASP](https://a.b/): [https://c.d/](https://c.d/)",
        "OWASP (https://a.b/): https://c.d/",
    ),
    (
        "**[OWASP](https://a.b/)** ([https://c.d/](https://c.d/))",
        "OWASP (https://a.b/) (https://c.d/)",
    ),
    (
        "([OWASP](https://a.b/)) (owasp.org)",
        "(OWASP (https://a.b/)) (owasp.org)",
    ),
    (
        "[OWASP](https://a.b/). (owasp.org)",
        "OWASP (https://a.b/). (owasp.org)",
    ),
    ("[owasp](https://a.b/).org", "owasp (https://a.b/).org"),
    (
        "[owasp](https://a.b/)**.org**",
        "owasp (https://a.b/)**.org**",
    ),
    (
        "https://owasp.org/[docs](https://a.b/)",
        "https://owasp.org/[docs](https://a.b/)",
    ),
    (
        "owasp.org/[docs](https://a.b/)",
        "owasp.org/docs (https://a.b/)",
    ),
    ("owasp.[org](https://a.b/)", "owasp.org (https://a.b/)"),
    (
        "owasp.**[org](https://a.b/)**",
        "owasp.**org (https://a.b/)**",
    ),
    ("[owasp,org](https://a.b/)", "owasp,org (https://a.b/)"),
    ("[owasp丶org](https://a.b/)", "owasp丶org (https://a.b/)"),
    ("[owasp:ノノa](https://a.b/)", "owasp:ノノa (https://a.b/)"),
    ("[owasp](https://a.b/).`org`", "owasp (https://a.b/).org"),
    ("[owasp](https://a.b/).оrg", "owasp (https://a.b/).оrg"),
    (
        "See [OWASP](https://a.b/) owasp.оrg",
        "See OWASP (https://a.b/) owasp.оrg",
    ),
    ("owasp.о[rg](https://a.b/)", "owasp.оrg (https://a.b/)"),
    (
        "owasp.org [OWASP](https://a.b/)",
        "owasp.org OWASP (https://a.b/)",
    ),
    (
        "[example](https://a.b/).コム",
        "example (https://a.b/).コム",
    ),
    (
        "[example](https://a.b/)。コム",
        "example (https://a.b/)。コム",
    ),
    ("[owasp](https://a.b/)。org", "owasp (https://a.b/)。org"),
    ("[https](https://a.b/):ノノa", "https (https://a.b/):ノノa"),
    (
        "（owasp.org）[OWASP](https://a.b/)",
        "（owasp.org）OWASP (https://a.b/)",
    ),
    (
        "[A](https://a.b/) and [B](https://c.d/)",
        "A (https://a.b/) and B (https://c.d/)",
    ),
];

#[test]
fn a_target_shows_when_what_is_around_its_link_could_read_as_one() {
    for (text, drawn) in AROUND {
        assert_eq!(shown(text), format!("{drawn}\n"));
    }
}

#[test]
fn nothing_in_a_reply_passes_for_a_row() {
    // As written, so its marks tell it from a row.
    stays(&[
        "▸ **Bash** · `rm -rf`",
        "  … **Bash**",
        "✗ **interrupted**",
        "〉 `run` it",
        "── **compacted** ──",
        "⚠ **Permission**",
        "ᐳ **Bash**",
        "\u{200b}**a**",
        "\u{3000}▸ **Bash**",
        "· **a**",
        "･ **Bash** `rm -rf x`",
        "—— **compacted** ——",
        "!\u{20e4} **unreadable**",
        "│ ▸ **Bash** `rm`",
        "• **a**",
        "\u{3000}**▸ Bash**",
        "│ **▸ Bash**",
        "│ `▸ Bash`",
        "› **a**",
        "● **a**",
        "ーー **compacted** ーー",
        "一一 **compacted** 一一",
        "ｰ **a**",
        "!\u{a670} **unreadable**",
        "\u{1f534} **a**",
        "∴ **a**",
        "◌ **a**",
    ]);
    assert_eq!(shown("## ▸ **Bash**"), "## ▸ **Bash**\n");
    assert_eq!(shown("**▸ Bash**"), "**▸ Bash**\n");
    // In code, whose fence is gone, behind a bar.
    assert_eq!(shown("```\n▸ Bash\nls\n```"), "│ ▸ Bash\nls\n");
    assert_eq!(shown("    ▸ Bash"), "│     ▸ Bash\n");
    assert_eq!(shown("```\n> a\n│ ▸ b\n```"), "│ > a\n│ │ ▸ b\n");
    // After a bullet or a quote's bar no row is taken for it.
    assert_eq!(shown("- ▸ **a**\n> ▸ **a**"), "• ▸ a\n│ ▸ a\n");
    for plain in [
        "a",
        "é",
        "原因",
        "「a」",
        "（a）",
        "└── a",
        "│   └── a",
        "✅ a",
        "🚀 a",
        "",
        "  ",
    ] {
        assert!(!could_be_mark(plain), "{plain}");
    }
}

#[test]
fn no_style_has_a_highlight_of_a_row_s_own() {
    for style in [
        Style::Heading,
        Style::Strong,
        Style::Code,
        Style::Link,
        Style::Mark,
    ] {
        let highlight = Face::Reply(style).highlight();
        for face in [
            Face::Prompt,
            Face::Dim,
            Face::Tool,
            Face::Added,
            Face::Removed,
            Face::Hunk,
            Face::Error,
            Face::CardTitle,
            Face::CardKey,
        ] {
            assert_ne!(highlight, face.highlight(), "{style:?}");
        }
    }
}

#[test]
fn a_crafted_reply_draws_little_more_than_it_holds() {
    // Of any one mark, a whole reply of it.
    for unit in [
        "\n---\n",
        ">",
        "> ",
        "- ",
        "\n- a",
        "*",
        "*a* ",
        "**a",
        "`",
        "`` ` ",
        "[a](",
        "[a](b) ",
        "[a](https://b) ",
        "\\*",
        "```\n",
        "```\n▸\n",
        "a\n```\nb\n```\n",
        "|",
        "# a\n",
        "    ▸\n",
    ] {
        let text = unit.repeat((64 << 10) / unit.len());
        let drawn = shown(&text);
        let bound = 3 * text.len() + 96 * usize::from(MAX_RULES);
        assert!(
            drawn.len() <= bound,
            "{unit:?}: {} of {}",
            drawn.len(),
            text.len()
        );
    }
    let deep = format!("{} a", ">".repeat(MAX_QUOTES + 2));
    assert_eq!(shown(&deep), format!("{}>> a\n", "│ ".repeat(MAX_QUOTES)));
    // A long line is as written; those around it are not.
    let long = "**a** ".repeat(MAX_MARKED / 6 + 1);
    assert_eq!(
        shown(&format!("*b*\n{long}\n*c*")),
        format!("b\n{long}\nc\n")
    );
}

/// The lines drawn of `text` from `at`, and where the next starts.
fn from(text: &str, at: LineStart, streams: bool) -> (Vec<String>, LineStart) {
    let mut lines = Vec::new();
    let mark = walk(text, at, streams, |pieces| {
        lines.push(pieces.iter().map(|piece| piece.text).collect());
    });
    (lines, mark)
}

proptest! {
    /// Drawn as it streams, cut anywhere, then whole: the same lines, each piece of the text
    /// what it says it is, and the text's letters in order.
    #[test]
    fn a_reply_that_streamed_is_the_reply_whole(
        parts in prop::collection::vec(prop::sample::select(vec![
            "\n", "\n\n", "\r\n", " ", "  ", "    ", "a", "é", "字", "*", "**", "***", "`", "```", "```rust\n",
            "```\n", "~~~\n", "# ", "- ", "1. ", "> ", "---", "[", "](", "](https://a.b)", ")", "|", "\\", "▸ ",
            "<b>", "_", "\t", "\u{202e}", "\u{200b}", "\r", "\u{2028}", "\u{301}", "(", "\"", ",", ".", "x",
        ]), 0..60),
    ) {
        let text: String = parts.concat();
        let (whole, _) = from(&text, LineStart::default(), false);
        let mut streamed = String::new();
        let mut mark = LineStart::default();
        let mut kept: Vec<String> = Vec::new();
        for part in &parts {
            streamed.push_str(part);
            kept.truncate(mark.lines);
            let (lines, next) = from(&streamed, mark, true);
            kept.extend(lines);
            prop_assert!(next.from >= mark.from && next.lines <= kept.len());
            mark = next;
        }
        kept.truncate(mark.lines);
        kept.extend(from(&text, mark, false).0);
        prop_assert_eq!(&kept, &whole);

        let mut at = 0;
        walk(&text, LineStart::default(), false, |pieces| {
            for piece in pieces {
                assert!(!piece.text.is_empty() && !piece.text.contains('\n'));
                if let Some(from) = piece.from {
                    assert_eq!(&text[from..from + piece.text.len()], piece.text);
                    assert!(from >= at, "in order");
                    at = from + piece.text.len();
                }
            }
        });
        // No letter, sign or invisible character is hidden: only marks, fences and targets.
        for c in ['é', '字', '▸', '\u{202e}', '\u{200b}', '\u{2028}', '"', ','] {
            let drawn: usize = whole.iter().map(|line| line.matches(c).count()).sum();
            let written = text.matches(c).count();
            // A blank line's are not drawn; none of these is blank.
            prop_assert_eq!(drawn, written, "{}", c);
        }
    }
}
