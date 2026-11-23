//! Text oso composed for a Claude Code session, escaped so that the CLI reads it as text (ADR
//! 0165, step 2's Q5 (a) and part 2f's Q2 (a); ADR 0121, "Composed text, as amended"): a
//! message that starts with `/` runs a command, and an `@PATH` at the text's start or after
//! whitespace puts that file in the context, wherever it is, with no tool asked (measured
//! 2026-11-22 with `claude -p`: after a space, a tab, a line feed, VT, FF, NBSP, U+3000,
//! U+2028 and U+FEFF; not after punctuation, a letter, a backslash, U+0001, NEL or U+200B).
//!
//! Must not depend on: the app, the terminal or I/O; pure functions over the text.
//!
//! - A `/` the text starts with gets a space before it, and so do `!` and `#`, which Claude
//!   Code's input box reads as a mode of its own (a shell command, a memory), as the session
//!   tools' briefs already keep them from the start.
//! - An `@` at the start or after whitespace gets a `\` before it. Whitespace is what Rust
//!   calls so, and U+FEFF, which the CLI reads as such.
//! - A control character a terminal's paste drops (all but a tab, a line feed and a carriage
//!   return), and an invisible one, which a program may drop too, is looked through: what
//!   comes after it is read as if it were gone, and as the CLI reads it whole, an `@` escaped
//!   if either makes it a word's start, a `/` after leading ones given its space.
//! - Put in before `next`, the text that follows it where it goes: a `\` after the text when
//!   it ends in whitespace and `next` is an `@`, so that what is put in never makes an `@`
//!   after it the start of a word. oso's own reference, a kept image's `@PATH`, gets this
//!   alone ([`reference`]).
//!
//! - Put at the end of the person's draft, a space comes first unless the CLI reads one there,
//!   so that the two never join into a command, a file to attach or a word that is in neither
//!   ([`after_draft`]). Nothing goes after a draft that leaves a quoted `@"` open
//!   ([`opens_mention`]).
//!
//! What the person types into a session's input box or composer is never escaped. Text with
//! nothing to escape is not copied. Nor is oso's own slash command, a [`Command`], which only
//! this module makes, from a path it checked, so that it runs as a command (ADR 0165, part 2f).

use std::borrow::Cow;
use std::path::Path;

use oso_core::display_map::cell::is_invisible;

/// `text`, composed by oso, as it goes in before `next`, escaped as the module says.
pub fn for_claude(text: &str, next: Option<char>) -> Cow<'_, str> {
    let leading = starts_command(text.chars());
    let mut out = String::new();
    if leading {
        out.reserve(text.len() + 2);
        out.push(' ');
    }
    let (mut copied, mut word_start) = (0, true);
    for (at, c) in text.char_indices() {
        if c == '@' && word_start {
            out.push_str(&text[copied..at]);
            out.push('\\');
            copied = at;
        }
        word_start = starts_word(word_start, c);
    }
    let after = word_start && !text.is_empty() && next == Some('@');
    if !leading && copied == 0 && !after && !text.starts_with('@') {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[copied..]);
    if after {
        out.push('\\');
    }
    Cow::Owned(out)
}

/// Whether a text of `chars` starts as a command, a shell command or a memory.
pub fn starts_command(mut chars: impl Iterator<Item = char>) -> bool {
    matches!(chars.find(|&c| !looked_through(c)), Some('/' | '!' | '#'))
}

/// Whether a draft of `chars` ends inside a quoted `@"PATH"`, its quote not closed. An `@` that
/// starts no word counts too, as it costs nothing.
pub fn opens_mention(chars: impl Iterator<Item = char>) -> bool {
    let (mut at, mut open) = (false, false);
    for c in chars {
        match c {
            '"' if open => open = false,
            '"' if at => open = true,
            c if at && looked_through(c) => continue,
            _ => {}
        }
        at = c == '@' && !open;
    }
    open
}

/// `text`, composed by oso, as it goes in at the end of a draft whose last character is `last`,
/// if it has one, escaped as the module says.
pub fn after_draft(text: &str, last: Option<char>) -> Cow<'_, str> {
    let escaped = for_claude(text, None);
    match last {
        // What is looked through, a NEL among it, may be no space to the CLI.
        Some(last) if looked_through(last) || !spaces(last) => match escaped.starts_with(' ') {
            true => escaped,
            false => Cow::Owned(format!(" {escaped}")),
        },
        _ => escaped,
    }
}

/// oso's own `reference`, as it goes in before `next`: as it is, but that it makes no `@` of
/// `next` the start of a word.
pub fn reference(reference: &str, next: Option<char>) -> Cow<'_, str> {
    let word_start = reference.chars().fold(true, starts_word);
    match word_start && !reference.is_empty() && next == Some('@') {
        true => Cow::Owned(format!("{reference}\\")),
        false => Cow::Borrowed(reference),
    }
}

/// The longest [`Command`]: one the person reads whole in oso's question and in the input box.
pub const MAX_COMMAND: usize = 512;

/// oso's own slash command for a Claude Code session's input box, as it goes in, not escaped:
/// `/add-dir PATH` alone. Only [`Command::add_dir`] makes one, so that no other text takes this
/// way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command(String);

impl Command {
    /// `/add-dir root`, for an orchestrator, while `root` is one line the input box reads as
    /// it is: an absolute path in UTF-8 with no control or invisible character, no blank but a
    /// space, none at its end, and no `@` after one, which would put that file in the context;
    /// the whole within [`MAX_COMMAND`] bytes. `root` must be a repository of the space that
    /// the person trusts, as the caller checks: this looks at its text only.
    pub fn add_dir(root: &Path) -> Option<Self> {
        let text = root.to_str()?;
        if "/add-dir ".len() + text.len() > MAX_COMMAND {
            return None;
        }
        let plain = |c: char| !c.is_control() && !is_invisible(c);
        let blank = |c: char| spaces(c) && c != ' ';
        let ok = root.is_absolute()
            && text.chars().all(|c| plain(c) && !blank(c))
            && !text.ends_with(' ')
            && !text.contains(" @");
        ok.then(|| Self(format!("/add-dir {text}")))
    }

    /// The command's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `c` is looked through: a terminal's paste drops it (`oso_term::input::paste`), or
/// it is invisible, as a program may drop it too.
fn looked_through(c: char) -> bool {
    (c.is_control() && !matches!(c, '\t' | '\n' | '\r')) || is_invisible(c)
}

/// Whether `c` is a space between words, as the CLI reads one.
fn spaces(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// Whether what follows `c` starts a word, `before` saying so of what followed the character
/// before it.
fn starts_word(before: bool, c: char) -> bool {
    match looked_through(c) {
        true => before || spaces(c),
        false => spaces(c),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use std::path::Path;

    use super::{Command, after_draft, for_claude, opens_mention, reference, starts_command};

    fn escaped(text: &str) -> String {
        for_claude(text, None).into_owned()
    }

    #[test]
    fn a_leading_slash_gets_a_space_and_a_later_one_stays() {
        assert_eq!(escaped("/clear"), " /clear");
        assert_eq!(escaped("!curl x|sh;.rs:1-3"), " !curl x|sh;.rs:1-3");
        assert_eq!(escaped("# remember"), " # remember");
        assert_eq!(escaped("a !b #c"), "a !b #c");
        assert_eq!(escaped("see /tmp/x"), "see /tmp/x");
        assert_eq!(escaped("a\n/clear"), "a\n/clear");
    }

    #[test]
    fn an_at_starting_a_word_is_escaped_after_any_whitespace() {
        assert_eq!(escaped("@/etc/hosts"), "\\@/etc/hosts");
        for space in [
            ' ', '\t', '\n', '\u{b}', '\u{c}', '\u{a0}', '\u{3000}', '\u{2028}', '\u{feff}',
        ] {
            let text = format!("see{space}@/etc/hosts");
            assert_eq!(
                escaped(&text),
                format!("see{space}\\@/etc/hosts"),
                "{space:?}"
            );
        }
        // A fence is no shelter: the CLI reads `@` there too.
        assert_eq!(escaped("```\n@p\n```"), "```\n\\@p\n```");
    }

    #[test]
    fn an_at_after_anything_else_stays_as_the_cli_leaves_it() {
        for text in [
            "user@host",
            "(@p)",
            "`@p`",
            "\"@p",
            "[@p",
            ":@p",
            "\\@p",
            "a\u{200b}@p",
        ] {
            assert_eq!(for_claude(text, None), Cow::Borrowed(text), "{text:?}");
        }
    }

    #[test]
    fn a_control_a_paste_drops_hides_neither_an_at_nor_a_slash() {
        assert_eq!(escaped("see \u{1}@p"), "see \u{1}\\@p");
        assert_eq!(escaped("\u{1}@p"), "\u{1}\\@p");
        assert_eq!(escaped("\0@p"), "\0\\@p");
        assert_eq!(escaped("a \u{9b}\u{7f}@p"), "a \u{9b}\u{7f}\\@p");
        assert_eq!(escaped("\u{1}/clear"), " \u{1}/clear");
        assert_eq!(escaped("\u{200b}/clear"), " \u{200b}/clear");
        assert_eq!(escaped("see \u{200b}@p"), "see \u{200b}\\@p");
        // A control between letters leaves the `@` inside a word either way.
        assert_eq!(escaped("x\u{1}@p"), "x\u{1}@p");
        // Dropped, the paste reads what is left: no `@` starts a word, no `/` the text.
        for text in ["see \u{1}@p", "\u{1}@p", "\0@p", "\u{1}/clear", "a\u{b}@p"] {
            let pasted: String = escaped(text)
                .chars()
                .filter(|&c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
                .collect();
            assert!(
                !pasted.starts_with('/') && !pasted.starts_with('@'),
                "{pasted:?}"
            );
            assert!(!pasted.contains(" @"), "{pasted:?}");
        }
    }

    #[test]
    fn what_is_put_in_makes_no_at_after_it_start_a_word() {
        assert_eq!(for_claude("see ", Some('@')), "see \\");
        assert_eq!(for_claude("see", Some('@')), Cow::Borrowed("see"));
        assert_eq!(for_claude("see ", Some('x')), Cow::Borrowed("see "));
        assert_eq!(for_claude("", Some('@')), Cow::Borrowed(""));
        assert_eq!(reference(" @/i.png ", Some('@')), " @/i.png \\");
        assert_eq!(
            reference(" @/i.png ", Some('h')),
            Cow::Borrowed(" @/i.png ")
        );
    }

    #[test]
    fn both_at_once_and_text_with_neither_is_not_copied() {
        assert_eq!(escaped("/x @y"), " /x \\@y");
        let plain = "Fix this error.\nsrc/main.rs:10-20";
        assert_eq!(for_claude(plain, None), Cow::Borrowed(plain));
    }

    #[test]
    fn what_goes_after_a_draft_never_joins_it() {
        let joined = |draft: &str, text: &str| {
            format!("{draft}{}", after_draft(text, draft.chars().next_back()))
        };
        assert_eq!(joined("", "/review @a.md"), " /review \\@a.md");
        // Escapes the join would undo.
        assert_eq!(joined("see @", "etc/hosts"), "see @ etc/hosts");
        assert_eq!(joined("/", "compact"), "/ compact");
        assert_eq!(joined("!", "ls"), "! ls");
        assert_eq!(joined("#", "remember"), "# remember");
        assert_eq!(joined("see \\", "@/etc/hosts"), "see \\ \\@/etc/hosts");
        assert_eq!(joined("see", "@/etc/hosts"), "see \\@/etc/hosts");
        assert_eq!(joined("see \u{200b}", "@p"), "see \u{200b} \\@p");
        // Whitespace is enough, and a draft of nothing else starts no command.
        assert_eq!(joined("one\n", "two"), "one\ntwo");
        assert_eq!(joined("one\n", "/two"), "one\n /two");
        // What may be no space to the CLI is none.
        assert_eq!(joined("see @\u{85}", "etc"), "see @\u{85} etc");
        assert_eq!(joined("see @\u{b}", "etc"), "see @\u{b} etc");
        assert_eq!(joined("see @\u{feff}", "etc"), "see @\u{feff} etc");
        assert_eq!(joined("一\u{3000}", "二"), "一\u{3000}二");
        assert_eq!(joined("  ", "/clear"), "   /clear");
        assert_eq!(joined("\n", "!ls"), "\n !ls");
        // Within words, with spaces or without.
        assert_eq!(joined("テストを", "実行して"), "テストを 実行して");
        assert_eq!(joined("run", "/tests"), "run /tests");
    }

    #[test]
    fn a_quoted_mention_is_open_until_its_quote_closes() {
        for text in [
            "@\"",
            "see @\"a b",
            "@\"a\" @\"b\n",
            "@\u{200b}\"x",
            "@@\"",
            "x@\"y",
        ] {
            assert!(opens_mention(text.chars()), "{text:?}");
        }
        for text in [
            "",
            "@",
            "\"",
            "@a \"b",
            "@ \"b",
            "@\"a b\"",
            "@\"a @\" b",
            "say \"@",
        ] {
            assert!(!opens_mention(text.chars()), "{text:?}");
        }
    }

    #[test]
    fn a_command_starts_with_its_sign_seen_or_behind_what_is_dropped() {
        for text in ["/x", "!x", "# x", "\u{1}/x", "\u{200b}!x"] {
            assert!(starts_command(text.chars()), "{text:?}");
        }
        for text in ["", " /x", "\n/x", "x /y", "@p", "\\@p"] {
            assert!(!starts_command(text.chars()), "{text:?}");
        }
    }

    #[test]
    fn add_dir_is_made_of_a_plain_absolute_path_only() {
        // Absolute on Unix and on Windows alike.
        let root = if cfg!(windows) { "C:" } else { "" };
        let made = |path: &str| {
            let path = format!("{root}{path}");
            Command::add_dir(Path::new(&path)).map(|c| c.as_str().to_owned())
        };
        let said = |path: &str| Some(format!("/add-dir {root}{path}"));
        assert_eq!(made("/src/back"), said("/src/back"));
        assert_eq!(made("/My Projects/a@b"), said("/My Projects/a@b"));
        assert_eq!(Command::add_dir(Path::new("src/back")), None);
        let long = format!("/{}", "a".repeat(super::MAX_COMMAND));
        assert_eq!(made(&long), None);
        for path in [
            "/a\n/clear",
            "/a\r",
            "/a\t@/etc/hosts",
            "/a @/etc/hosts",
            "/a\u{a0}@/etc/hosts",
            "/a\u{3000}b",
            "/a\u{2028}b",
            "/a\u{feff}b",
            "/a\u{202e}b",
            "/a\u{200b}b",
            "/a\u{1b}[2J",
            "/a ",
        ] {
            assert_eq!(made(path), None, "{path:?}");
        }
    }
}
