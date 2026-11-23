//! The learning log as `dictation-learning-log` shows it (ADR 0175, step 4): its newest line
//! first, each with when, what became of a pair and the pair.
//!
//! Must not depend on: `app` or the editor; this makes text of the log's lines, and the host
//! says how a time is shown.
//!
//! - A line of the log that is not one oso writes is left out, and a word in it shows only
//!   characters shown as text, as the file can be changed outside oso.

use std::fmt::Write;

use serde_json::Value;

use super::MAX_LOG_LINES;
use crate::dictation::fix;

/// The log `log` as its buffer shows it, `when` giving the time of a line's seconds since the
/// epoch.
pub fn shown(log: &str, when: impl Fn(u64) -> String) -> String {
    let mut text = String::from("What dictation learned, newest first.\n\n");
    let start = text.len();
    for line in log.lines().rev().take(MAX_LOG_LINES) {
        if let Some((at, said)) = line_of(line) {
            let _ = writeln!(text, "{}  {said}", when(at));
        }
    }
    if text.len() == start {
        text.push_str("Nothing yet.\n");
    }
    text
}

/// The time and the words of one line of the log, if it is one oso writes.
fn line_of(line: &str) -> Option<(u64, String)> {
    let value: Value = serde_json::from_str(line).ok()?;
    let at = value.get("at")?.as_u64()?;
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    let pair = || {
        let (wrong, right) = (text("wrong")?, text("right")?);
        Some(format!("{} → {}", plain(wrong), plain(right)))
    };
    let kind = || match text("as")? {
        "term" => Some("term"),
        "correction" => Some("correction"),
        _ => None,
    };
    let why = || text("why").map(plain).unwrap_or_default();
    let said = match text("event")? {
        "observed" => {
            let what = match (text("source")?, text("outcome")?) {
                ("fix", "stood") => "fix kept",
                ("fix", "undone") => "fix undone",
                ("edit" | "plain", "stood") => "you rewrote",
                _ => return None,
            };
            let plain = if text("source")? == "plain" {
                " (typed plain, never sent)"
            } else {
                ""
            };
            format!("{what}: {}{plain}", pair()?)
        }
        "history" => {
            let count = |key: &str| value.get(key).and_then(Value::as_u64);
            let (accepted, rejected) = (count("accepted")?, count("rejected")?);
            format!("sent with a fix: {accepted} kept and {rejected} undone")
        }
        "added" => format!("added to TypeWhisper as a {}: {}", kind()?, pair()?),
        "removed" => format!("removed from TypeWhisper: {} ({})", pair()?, why()),
        "released" => format!("left to you in TypeWhisper: {} ({})", pair()?, why()),
        "skipped" => format!("not added to TypeWhisper: {} ({})", pair()?, why()),
        _ => return None,
    };
    Some((at, said))
}

/// `text` on one row, with what is not shown as text as U+FFFD.
fn plain(text: &str) -> String {
    text.chars()
        .map(|c| if fix::shown_as_text(c) { c } else { '\u{fffd}' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_line_is_said_newest_first() {
        let log = [
            r#"{"at":1,"event":"observed","source":"fix","outcome":"stood","wrong":"リアクト","right":"React"}"#,
            r#"{"at":2,"event":"observed","source":"plain","outcome":"stood","wrong":"a","right":"b"}"#,
            r#"{"at":3,"event":"history","accepted":2,"rejected":1}"#,
            r#"{"at":4,"event":"added","as":"term","wrong":"リアクト","right":"React","why":""}"#,
            r#"{"at":5,"event":"removed","as":"term","wrong":"x","right":"y","why":"not used"}"#,
            r#"{"at":6,"event":"skipped","as":"correction","wrong":"x","right":"y","why":"why"}"#,
            r#"{"at":7,"event":"released","as":"term","wrong":"x","right":"y","why":"why"}"#,
        ]
        .join("\n");
        assert_eq!(
            shown(&log, |at| at.to_string()),
            "What dictation learned, newest first.\n\n\
             7  left to you in TypeWhisper: x → y (why)\n\
             6  not added to TypeWhisper: x → y (why)\n\
             5  removed from TypeWhisper: x → y (not used)\n\
             4  added to TypeWhisper as a term: リアクト → React\n\
             3  sent with a fix: 2 kept and 1 undone\n\
             2  you rewrote: a → b (typed plain, never sent)\n\
             1  fix kept: リアクト → React\n"
        );
    }

    #[test]
    fn a_line_not_oso_s_is_left_out_and_a_word_stays_on_its_row() {
        let log = "nonsense\n{\"at\":1}\n{\"at\":2,\"event\":\"added\",\"as\":\"term\",\
                   \"wrong\":\"a\\nb\",\"right\":\"c\\u202ed\"}";
        assert_eq!(
            shown(log, |at| at.to_string()),
            "What dictation learned, newest first.\n\n\
             2  added to TypeWhisper as a term: a\u{fffd}b → c\u{fffd}d\n"
        );
        assert!(shown("", |_| String::new()).ends_with("Nothing yet.\n"));
    }
}
