//! What `oso dev` tells the editor of a failed build (ADR 0090): one row per error for
//! `*Messages*`, and a summary for the echo area, from cargo's JSON diagnostics.
//!
//! Must not depend on: processes, sockets or the terminal; these are pure functions of
//! cargo's output lines.
//!
//! Only the level, the message, the code and the primary span are read, never the
//! rendered text, so no escape sequence of cargo's reaches the editor. The editor escapes
//! control characters in any message too.

use serde_json::Value;

/// The most errors sent as rows of their own.
const MAX_ROWS: usize = 20;

/// What one line of cargo's output says of the build, if it is a compiler message.
#[derive(Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// The text cargo would have shown, in color, for the terminal.
    pub rendered: Option<String>,
    /// The error as one row, if it is one.
    pub error: Option<String>,
}

/// What a line of cargo's JSON output says, if it is a compiler message.
pub fn diagnostic(message: &Value) -> Option<Diagnostic> {
    if message["reason"] != "compiler-message" {
        return None;
    }
    let message = &message["message"];
    let rendered = message["rendered"].as_str().map(str::to_owned);
    let text = message["message"].as_str().unwrap_or("");
    let error = message["level"]
        .as_str()
        .is_some_and(|level| level.starts_with("error"))
        // The count at the end says nothing the rows do not.
        && !text.starts_with("aborting due to");
    Some(Diagnostic {
        rendered,
        error: error.then(|| row(message, text)),
    })
}

/// `file:line:column: message [code]`, from the primary span if there is one.
fn row(message: &Value, text: &str) -> String {
    let spans = message["spans"].as_array();
    let primary = spans.and_then(|spans| spans.iter().find(|span| span["is_primary"] == true));
    let place = primary.and_then(|span| {
        Some(format!(
            "{}:{}:{}: ",
            span["file_name"].as_str()?,
            span["line_start"].as_u64()?,
            span["column_start"].as_u64()?
        ))
    });
    let code = message["code"]["code"]
        .as_str()
        .map(|code| format!(" [{code}]"));
    format!(
        "{}{text}{}",
        place.unwrap_or_default(),
        code.unwrap_or_default()
    )
}

/// What the editor is told of a failed build with `errors`: the rows for `*Messages*`,
/// then the summary, which the echo area keeps as it comes last.
pub fn messages(errors: &[String]) -> Vec<String> {
    let mut messages: Vec<String> = errors
        .iter()
        .take(MAX_ROWS)
        .map(|error| format!("oso dev: error: {error}"))
        .collect();
    if errors.len() > MAX_ROWS {
        messages.push(format!("oso dev: … and {} more", errors.len() - MAX_ROWS));
    }
    messages.push(match errors {
        [] => "oso dev: build failed; the terminal says why".to_owned(),
        [only] => format!("oso dev: build failed: 1 error; {only}"),
        [first, ..] => format!("oso dev: build failed: {} errors; {first}", errors.len()),
    });
    messages
}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, MAX_ROWS, diagnostic, messages};
    use serde_json::{Value, json};

    fn compiler_message(level: &str, text: &str, spans: Value) -> Value {
        json!({
            "reason": "compiler-message",
            "message": {
                "message": text,
                "level": level,
                "code": { "code": "E0308" },
                "spans": spans,
                "rendered": "\u{1b}[31merror\u{1b}[0m: mismatched types",
            },
        })
    }

    #[test]
    fn an_error_is_a_row_at_its_primary_span_and_keeps_its_rendering_for_the_terminal() {
        let spans = json!([
            { "file_name": "other.rs", "line_start": 1, "column_start": 1, "is_primary": false },
            { "file_name": "crates/oso/src/x.rs", "line_start": 12, "column_start": 5, "is_primary": true },
        ]);
        let found = diagnostic(&compiler_message("error", "mismatched types", spans)).unwrap();
        assert_eq!(
            found.error.as_deref(),
            Some("crates/oso/src/x.rs:12:5: mismatched types [E0308]")
        );
        assert!(
            found.rendered.unwrap().contains('\u{1b}'),
            "colors stay for the terminal"
        );
    }

    #[test]
    fn warnings_and_the_closing_count_are_no_rows_and_other_lines_no_message() {
        let warning = diagnostic(&compiler_message("warning", "unused", json!([]))).unwrap();
        assert_eq!(warning.error, None);
        assert!(warning.rendered.is_some(), "the terminal still shows it");
        let count = compiler_message("error", "aborting due to 2 previous errors", json!([]));
        assert_eq!(diagnostic(&count).unwrap().error, None);
        let spanless = diagnostic(&compiler_message("error", "linking failed", json!([]))).unwrap();
        assert_eq!(spanless.error.as_deref(), Some("linking failed [E0308]"));
        assert_eq!(
            diagnostic(&json!({ "reason": "build-finished" })),
            None::<Diagnostic>
        );
    }

    #[test]
    fn the_summary_comes_last_and_the_rows_stop_at_their_bound() {
        assert_eq!(
            messages(&["a.rs:1:1: no".to_owned()]),
            [
                "oso dev: error: a.rs:1:1: no",
                "oso dev: build failed: 1 error; a.rs:1:1: no"
            ]
        );
        let many: Vec<String> = (0..MAX_ROWS + 3).map(|n| format!("e{n}")).collect();
        let told = messages(&many);
        assert_eq!(told.len(), MAX_ROWS + 2);
        assert_eq!(told[MAX_ROWS], "oso dev: … and 3 more");
        assert_eq!(
            told[MAX_ROWS + 1],
            format!("oso dev: build failed: {} errors; e0", MAX_ROWS + 3)
        );
        assert_eq!(
            messages(&[]),
            ["oso dev: build failed; the terminal says why"]
        );
    }
}
