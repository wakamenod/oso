//! What a hosted session's state is read from (ADR 0121): the bottom of its live screen, its
//! title and its OSC 9;4 progress, cut to fixed sizes, and the regions rules look at.
//!
//! Must not depend on: `app` or `dispatch`; this is plain data over text, which
//! the terminal's host API (ADR 0120) fills.
//!
//! Everything here is untrusted: a model wrote it, steered by whatever it read. So each row and
//! the title are cut to a fixed size, and control characters become spaces, which bounds what
//! a rule ever looks at to about 41 KiB.

use std::fmt;

/// The most rows kept: the last non-empty rows of the live screen (ADR 0121).
pub const MAX_ROWS: usize = 40;

/// The most bytes kept of a row, cut on a character's boundary.
pub const MAX_ROW_BYTES: usize = 1024;

/// The most bytes kept of the title, cut on a character's boundary.
pub const MAX_TITLE_BYTES: usize = 1024;

/// The fewest `─` a row needs to be a rule, the line the TUI draws above and below its prompt.
const MIN_RULE_CHARS: usize = 3;

/// The longest session name a rule's label holds.
const MAX_LABEL_CHARS: usize = 128;

/// The terminal's OSC 9;4 progress: its state (0 cleared, 1 a value, 2 error, 3 under way, 4
/// paused) and the value, if any, from 0 to 100.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    state: u8,
    value: Option<u8>,
}

impl Progress {
    /// The progress a terminal reported, `None` for a state past 4, with the value cut to 100:
    /// the bytes came from the session.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the terminal's host API reports progress (ADR 0121 step 3)"
        )
    )]
    pub fn new(state: u8, value: Option<u8>) -> Option<Self> {
        (state <= 4).then(|| Self {
            state,
            value: value.map(|value| value.min(100)),
        })
    }
}

impl fmt::Display for Progress {
    /// As the `progress` region shows it: `state`, or `state;value`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.value {
            Some(value) => write!(f, "{};{value}", self.state),
            None => write!(f, "{}", self.state),
        }
    }
}

/// What one look at a session's terminal found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    /// The last non-empty rows, oldest first, each cut and cleaned, with no trailing spaces.
    rows: Vec<String>,
    title: String,
    progress: Option<Progress>,
    /// Echo is off: the rows are withheld (ADR 0120), and the state keeps its last value.
    /// See [`Evidence::echo_off`] for what counts.
    echo_off: bool,
}

impl Evidence {
    /// The evidence of `rows`, the screen's from the top, with `title` and `progress`. Only the
    /// last [`MAX_ROWS`] non-empty rows are kept.
    pub fn new<'r>(
        rows: impl DoubleEndedIterator<Item = &'r str>,
        title: &str,
        progress: Option<Progress>,
    ) -> Self {
        let mut kept: Vec<String> = rows
            .rev()
            .map(|row| clean(row, MAX_ROW_BYTES))
            .filter(|row| !row.is_empty())
            .take(MAX_ROWS)
            .collect();
        kept.reverse();
        Self {
            rows: kept,
            title: clean(title, MAX_TITLE_BYTES),
            progress,
            echo_off: false,
        }
    }

    /// The evidence of a saved screen's text, with no title or progress, for
    /// `session-explain-file`.
    pub fn from_text(text: &str) -> Self {
        Self::new(text.lines(), "", None)
    }

    /// The evidence while echo is off: no rows, as the terminal withholds them.
    ///
    /// The engine's own hold signal, not what makes a terminal secret, which is ADR 0120's to
    /// settle. Claude Code's TUI runs in raw mode, with ECHO and ICANON both clear, so a signal
    /// of plain "ECHO clear" would hold every session's state for good: it needs a narrower
    /// one, such as a password prompt's shape, ECHO clear while ICANON is set (ADR 0121, step 2
    /// as built).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the terminal's host API feeds it (ADR 0121 step 3)"
        )
    )]
    pub fn echo_off(title: &str, progress: Option<Progress>) -> Self {
        Self {
            echo_off: true,
            ..Self::new(std::iter::empty(), title, progress)
        }
    }

    /// Whether echo is off.
    pub fn is_echo_off(&self) -> bool {
        self.echo_off
    }

    /// The rows kept, oldest first.
    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    /// The text of `region`, its rows joined by `\n`.
    pub fn region(&self, region: Region) -> String {
        match region {
            Region::Title => self.title.clone(),
            Region::Progress => self.progress.map(|p| p.to_string()).unwrap_or_default(),
            Region::Bottom(n) => {
                let from = self.rows.len().saturating_sub(usize::from(n));
                self.rows[from..].join("\n")
            }
            Region::PromptBox => match self.last_two_rules() {
                (Some(top), Some(bottom)) => self.rows[top + 1..bottom].join("\n"),
                _ => String::new(),
            },
            // With no rule there is no prompt box, so nothing is below it: a model's output
            // must not pass for a dialog's footer.
            Region::AfterLastRule => match self.last_two_rules().1 {
                Some(last) => self.rows[last + 1..].join("\n"),
                None => String::new(),
            },
            Region::AbovePromptBox => match self.last_two_rules() {
                (Some(top), Some(_)) if top > 0 => self.rows[top - 1].clone(),
                _ => String::new(),
            },
        }
    }

    /// Where the last two rules are among the rows: the one before the last, and the last.
    fn last_two_rules(&self) -> (Option<usize>, Option<usize>) {
        let mut rules = self
            .rows
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, row)| is_rule(row))
            .map(|(at, _)| at);
        let last = rules.next();
        (rules.next(), last)
    }
}

/// Whether `row` is a rule: at least [`MIN_RULE_CHARS`] of `─`, then nothing, or a label and
/// one to three `─`: the TUI writes a session's `--name` at the right end of the prompt box's
/// top rule, `──── api-fix ─`. A dialog's dashed `╌` lines and a box's corners are not.
fn is_rule(row: &str) -> bool {
    let row = row.trim();
    let lead = row.chars().take_while(|&c| c == '─').count();
    if lead < MIN_RULE_CHARS {
        return false;
    }
    let rest = &row[lead * '─'.len_utf8()..];
    rest.is_empty() || is_label(rest)
}

/// Whether `rest`, what follows a rule's run of `─`, is ` NAME ─`: a name of at most
/// [`MAX_LABEL_CHARS`] characters with no `─` in it, and one to three `─` after it.
fn is_label(rest: &str) -> bool {
    let Some(rest) = rest.strip_prefix(' ') else {
        return false;
    };
    let tail = rest.chars().rev().take_while(|&c| c == '─').count();
    let label = rest[..rest.len() - tail * '─'.len_utf8()].strip_suffix(' ');
    (1..=3).contains(&tail)
        && label.is_some_and(|label| {
            !label.trim().is_empty()
                && !label.contains('─')
                && label.chars().nth(MAX_LABEL_CHARS).is_none()
        })
}

/// `text` cut at `max` bytes on a character's boundary, with control characters as spaces and
/// no trailing spaces.
fn clean(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cleaned: String = text[..end]
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned.trim_end().to_owned()
}

/// Where in the evidence a rule looks (ADR 0121).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Region {
    /// The title.
    Title,
    /// The OSC 9;4 progress, as `state` or `state;value`; empty if none was set.
    Progress,
    /// The last N non-empty rows, N from 1 to [`MAX_ROWS`].
    Bottom(u8),
    /// The rows between the last two rules: the TUI's prompt box, or the body of a dialog that
    /// takes its place.
    PromptBox,
    /// The rows after the last rule: below the prompt box, or a dialog with its footer.
    AfterLastRule,
    /// The last row above the prompt box: the TUI's status line while it works.
    AbovePromptBox,
}

impl Region {
    /// The region a rules file names, if it is one.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "title" => Self::Title,
            "progress" => Self::Progress,
            "prompt_box" => Self::PromptBox,
            "after_last_rule" => Self::AfterLastRule,
            "above_prompt_box" => Self::AbovePromptBox,
            _ => {
                let n = name.strip_prefix("bottom(")?.strip_suffix(')')?;
                let n: u8 = n.parse().ok()?;
                if n == 0 || usize::from(n) > MAX_ROWS {
                    return None;
                }
                Self::Bottom(n)
            }
        })
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Title => f.write_str("title"),
            Self::Progress => f.write_str("progress"),
            Self::Bottom(n) => write!(f, "bottom({n})"),
            Self::PromptBox => f.write_str("prompt_box"),
            Self::AfterLastRule => f.write_str("after_last_rule"),
            Self::AbovePromptBox => f.write_str("above_prompt_box"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Evidence, MAX_ROW_BYTES, MAX_ROWS, Progress, Region, is_rule};

    #[test]
    fn a_rule_may_end_with_the_sessions_name() {
        for row in [
            "───",
            "  ──────  ",
            "────── api-fix ─",
            "──── my session ───",
        ] {
            assert!(is_rule(row), "{row:?}");
        }
        let long = format!("──── {} ─", "n".repeat(129));
        for row in [
            "──",
            "──── ─",
            "────api ─",
            "──── api",
            "──── api ────",
            "──── a─b ─",
            "╌╌╌╌╌╌",
            "╭────╮",
            "──── ok ─ more",
            &long,
        ] {
            assert!(!is_rule(row), "{row:?}");
        }
    }

    const IDLE: &str = "\
 Claude Code
  some output

────────────
❯ Try something
────────────
  ? for shortcuts
";

    #[test]
    fn regions_find_the_prompt_box_by_its_two_rules() {
        let evidence = Evidence::from_text(IDLE);
        assert_eq!(evidence.region(Region::PromptBox), "❯ Try something");
        assert_eq!(evidence.region(Region::AfterLastRule), "  ? for shortcuts");
        assert_eq!(evidence.region(Region::AbovePromptBox), "  some output");
        assert_eq!(
            evidence.region(Region::Bottom(2)),
            "────────────\n  ? for shortcuts"
        );
    }

    #[test]
    fn without_rules_every_region_of_the_prompt_box_is_empty() {
        let evidence = Evidence::from_text("a\n╌╌╌╌╌\nb");
        assert_eq!(evidence.region(Region::PromptBox), "");
        assert_eq!(evidence.region(Region::AbovePromptBox), "");
        assert_eq!(evidence.region(Region::AfterLastRule), "");
        assert_eq!(evidence.region(Region::Bottom(3)), "a\n╌╌╌╌╌\nb");
    }

    #[test]
    fn progress_past_its_states_is_none_and_its_value_is_cut_to_100() {
        assert_eq!(Progress::new(5, None), None);
        assert_eq!(Progress::new(255, Some(1)), None);
        assert_eq!(Progress::new(4, Some(250)).unwrap().to_string(), "4;100");
        assert_eq!(Progress::new(0, None).unwrap().to_string(), "0");
    }

    #[test]
    fn only_the_last_non_empty_rows_are_kept_cut_and_cleaned() {
        let mut text = String::new();
        for n in 0..100 {
            text.push_str(&format!("row {n}\n\n"));
        }
        text.push_str(&"é".repeat(MAX_ROW_BYTES));
        text.push_str("\nbell\u{7}here   \n");
        let evidence = Evidence::from_text(&text);
        assert_eq!(evidence.rows().len(), MAX_ROWS);
        assert_eq!(evidence.rows()[0], "row 62");
        let long = &evidence.rows()[MAX_ROWS - 2];
        assert_eq!(long.len(), MAX_ROW_BYTES, "cut on a boundary of two bytes");
        assert_eq!(evidence.rows()[MAX_ROWS - 1], "bell here");
    }

    #[test]
    fn title_and_progress_are_their_own_regions() {
        let progress = Progress::new(1, Some(40));
        let evidence = Evidence::new(["x"].into_iter(), "✳ Claude\u{1b}", progress);
        assert_eq!(evidence.region(Region::Title), "✳ Claude");
        assert_eq!(evidence.region(Region::Progress), "1;40");
        let off = Evidence::echo_off("t", None);
        assert!(off.is_echo_off() && off.rows().is_empty());
        assert_eq!(off.region(Region::Progress), "");
    }

    #[test]
    fn region_names_round_trip_and_bottom_is_bounded() {
        for name in [
            "title",
            "progress",
            "bottom(1)",
            "bottom(40)",
            "prompt_box",
            "after_last_rule",
            "above_prompt_box",
        ] {
            assert_eq!(Region::parse(name).unwrap().to_string(), name);
        }
        for bad in [
            "bottom(0)",
            "bottom(41)",
            "bottom()",
            "bottom(x)",
            "whole_recent",
        ] {
            assert_eq!(Region::parse(bad), None, "{bad}");
        }
    }
}
