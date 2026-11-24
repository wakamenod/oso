//! A reply's Markdown as drawn: which marks go and what stays, the faces left, and what Claude
//! writes passing for nothing.
//!
//! Must not depend on: rows, the syntax threads or the theme.

use oso_core::syntax::Highlight;

use super::signs::could_be_mark;
use super::{LineStart, Style, walk};
use crate::sessions::native::render::{Face, Icon};

mod streams;

/// A width no line of these tests needs.
const WIDE: usize = 98;

/// `text` as drawn, whole: each line's text, its breaks kept.
fn shown(text: &str) -> String {
    let mut out = String::new();
    walk(text, LineStart::default(), false, WIDE, |pieces, _| {
        out.extend(pieces.iter().map(|piece| piece.text));
        out.push('\n');
    });
    out
}

/// The pieces of `text` with a face, whole.
fn faces(text: &str) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    walk(text, LineStart::default(), false, WIDE, |pieces, _| {
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
            ("b", Style::StrongCode),
            (" ", Style::Heading),
            ("c", Style::Heading),
            ("d ", Style::Strong),
            ("e", Style::StrongCode),
            ("f", Style::Emphasis),
        ]
        .map(|(text, style)| (text.to_owned(), style))
    );
    assert_eq!(
        faces("*a `b` **c** d* ***e*** **f *g* [h](https://i.j)** `k`\n- *l*\n> **m**"),
        [
            ("a ", Style::Emphasis),
            ("b", Style::Code),
            (" ", Style::Emphasis),
            ("c", Style::Strong),
            (" d", Style::Emphasis),
            ("e", Style::Strong),
            ("f ", Style::Strong),
            ("g", Style::Strong),
            (" ", Style::Strong),
            ("h", Style::Link),
            ("k", Style::Code),
            ("• ", Style::Mark),
            ("l", Style::Emphasis),
            ("│ ", Style::Mark),
            ("m", Style::Strong),
        ]
        .map(|(text, style)| (text.to_owned(), style))
    );
    // A line goes under a link's words, and under no address that shows.
    assert_eq!(
        faces("[a b](https://c.d) [e.f](https://g.h) [https://i.j](https://i.j) **[k](l)**"),
        [
            ("a b", Style::Link),
            ("https://c.d", Style::Target),
            ("e.f", Style::Link),
            ("https://g.h", Style::Target),
            ("https://i.j", Style::Target),
            ("k", Style::Link),
            ("l", Style::Target),
            (")", Style::Strong),
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

/// Whether a reply's text can be in `highlight`: its Markdown's, or what a parser gives its
/// code; what Claude thought is a row's. With no wildcard, so that a new one does not compile
/// until it is placed.
#[expect(clippy::too_many_lines, reason = "one line a class")]
fn of_a_reply(highlight: Highlight) -> bool {
    match highlight {
        Highlight::Comment
        | Highlight::String
        | Highlight::Keyword
        | Highlight::Type
        | Highlight::Function
        | Highlight::Constant
        | Highlight::Property
        | Highlight::Macro
        | Highlight::Heading
        | Highlight::Link
        | Highlight::Markup
        | Highlight::SessionCode
        | Highlight::SessionBullet
        | Highlight::SessionTable
        | Highlight::SessionHeading
        | Highlight::SessionStrong
        | Highlight::SessionStrongCode
        | Highlight::SessionEmphasis
        | Highlight::SessionLink => true,
        Highlight::DiffAdded
        | Highlight::DiffRemoved
        | Highlight::DiffFileHeader
        | Highlight::DiffHunkHeader
        | Highlight::DiffRefineAdded
        | Highlight::DiffRefineRemoved
        | Highlight::CommentPerson
        | Highlight::CommentAgent
        | Highlight::Outdated
        | Highlight::Sent
        | Highlight::Decided
        | Highlight::SessionAlert
        | Highlight::SessionAlertLit
        | Highlight::CardTitle
        | Highlight::CardKey
        | Highlight::RequestBody
        | Highlight::SessionWorking
        | Highlight::SessionDone
        | Highlight::SessionQuiet
        | Highlight::SessionFront
        | Highlight::SessionSpace
        | Highlight::SessionRepository
        | Highlight::SessionPrompt
        | Highlight::SessionPromptMark
        | Highlight::SessionBrief
        | Highlight::SessionBriefMark
        | Highlight::SessionTool
        | Highlight::SessionRunning
        | Highlight::SessionThinking
        | Highlight::SessionIconRead
        | Highlight::SessionIconWrite
        | Highlight::SessionIconShell
        | Highlight::SessionIconSearch
        | Highlight::SessionIconAgent
        | Highlight::SessionIconWeb
        | Highlight::SessionIconTask
        | Highlight::SessionIconOther
        | Highlight::DiredHeader
        | Highlight::DiredDirectory
        | Highlight::DiredSymlink
        | Highlight::DiredBrokenSymlink
        | Highlight::DiredExecutable
        | Highlight::DiredSpecial
        | Highlight::DiredDetails
        | Highlight::DiredMark
        | Highlight::DiredMarked
        | Highlight::GitSectionHeading
        | Highlight::GitBranchLocal
        | Highlight::GitBranchRemote
        | Highlight::GitHash
        | Highlight::GitDetail
        | Highlight::GitCommitOverlong
        | Highlight::GitLane0
        | Highlight::GitLane1
        | Highlight::GitLane2
        | Highlight::GitLane3
        | Highlight::GitLane4
        | Highlight::GitLane5
        | Highlight::GitLane6
        | Highlight::GitLane7
        | Highlight::GitTag => false,
    }
}

/// The face after `face`, from [`Face::Prompt`] on, and whether `face` is one a reply's text
/// may be in. With no wildcard, so that a new one does not compile until it is placed.
fn next(face: Face) -> (Option<Face>, bool) {
    let (next, reply) = match face {
        Face::Prompt => (Face::PromptMark, false),
        Face::PromptMark => (Face::Said, false),
        Face::Said => (Face::BriefMark, false),
        Face::BriefMark => (Face::Brief, false),
        Face::Brief => (Face::Dim, false),
        Face::Dim => (Face::Thought, false),
        Face::Thought => (Face::Tool, false),
        Face::Tool => (Face::Running, false),
        Face::Running => (Face::Added, false),
        Face::Added => (Face::Removed, false),
        Face::Removed => (Face::Error, false),
        Face::Error => (Face::Waiting, false),
        Face::Waiting => (Face::CardTitle, false),
        Face::CardTitle => (Face::CardKey, false),
        Face::CardKey => (Face::RequestBody, false),
        Face::RequestBody => (Face::Icon(Icon::Read), false),
        Face::Icon(Icon::Read) => (Face::Icon(Icon::Write), false),
        Face::Icon(Icon::Write) => (Face::Icon(Icon::Shell), false),
        Face::Icon(Icon::Shell) => (Face::Icon(Icon::Search), false),
        Face::Icon(Icon::Search) => (Face::Icon(Icon::Agent), false),
        Face::Icon(Icon::Agent) => (Face::Icon(Icon::Web), false),
        Face::Icon(Icon::Web) => (Face::Icon(Icon::Task), false),
        Face::Icon(Icon::Task) => (Face::Icon(Icon::Other), false),
        Face::Icon(Icon::Other) => (Face::Target, false),
        // Of a card and a strip, which no reply is in: Claude's own words there.
        Face::Target => (Face::Reply(Style::Heading), true),
        Face::Reply(Style::Heading) => (Face::Reply(Style::Strong), true),
        Face::Reply(Style::Strong) => (Face::Reply(Style::Emphasis), true),
        Face::Reply(Style::Emphasis) => (Face::Reply(Style::Code), true),
        Face::Reply(Style::Code) => (Face::Reply(Style::StrongCode), true),
        Face::Reply(Style::StrongCode) => (Face::Reply(Style::Link), true),
        Face::Reply(Style::Link) => (Face::Reply(Style::Target), true),
        Face::Reply(Style::Target) => (Face::Reply(Style::Mark), true),
        Face::Reply(Style::Mark) => (Face::Reply(Style::Rule), true),
        Face::Reply(Style::Rule) => return (None, true),
    };
    (Some(next), reply)
}

/// No class a reply's text can have, its Markdown's or a fence's language's, is one of a row's,
/// a card's or a chip's, so none of it is drawn as oso's words are, nor has a band or a chip.
#[test]
fn no_text_of_a_reply_has_a_class_of_a_row_s_own() {
    let mut face = Some(Face::Prompt);
    let mut seen = 0;
    while let Some(at) = face {
        let (after, reply) = next(at);
        assert_eq!(of_a_reply(at.highlight()), reply, "{at:?}");
        face = after;
        seen += 1;
    }
    assert_eq!(seen, 34);
}

/// What is drawn in bold, in italic or over a line is a reply's, or what Claude thought: nothing
/// of oso's own has a style, so none of a reply's stands for a row.
#[test]
fn no_row_of_oso_s_has_a_style() {
    let styled = [
        Highlight::SessionHeading,
        Highlight::SessionStrong,
        Highlight::SessionStrongCode,
        Highlight::SessionEmphasis,
        Highlight::SessionLink,
        Highlight::SessionThinking,
    ];
    let mut face = Some(Face::Prompt);
    let mut seen = 0;
    while let Some(at) = face {
        let (after, reply) = next(at);
        if styled.contains(&at.highlight()) {
            assert!(reply || at == Face::Thought, "{at:?}");
            seen += 1;
        }
        face = after;
    }
    assert_eq!(seen, styled.len());
}
