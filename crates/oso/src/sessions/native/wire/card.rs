//! What a permission card shows of a call's input (ADR 0165, step 2e-2): an `AskUserQuestion`'s
//! questions and an `ExitPlanMode`'s plan, read from the input the request carries.
//!
//! Must not depend on: `rows`, `render` or the UI; plain functions over a parsed value.
//!
//! - All of it is Claude's text, untrusted: at most [`MAX_QUESTIONS`] questions of at most
//!   [`MAX_OPTIONS`] options, each label and header cut to [`MAX_NAME`] bytes of its first line,
//!   each description to [`MAX_DESCRIBED`] bytes with its lines, and a plan to [`MAX_PLAN`] bytes.
//!   What is cut says so.
//! - A question is its answer's key, which the CLI looks up whole: one over [`MAX_ASKED`] bytes
//!   cannot be asked, nor the call it is in (ADR 0180).

use serde_json::Value;

use super::{MAX_NAME, capped};

/// The most questions one `AskUserQuestion` asks: the tool takes 1 to 4.
pub const MAX_QUESTIONS: usize = 4;
/// The most options a question offers: the tool takes 2 to 4, and the card numbers 1 to 9.
pub const MAX_OPTIONS: usize = 9;
/// The most of a question's text, in bytes.
pub const MAX_ASKED: usize = 8 << 10;
/// The most of an option's description kept, in bytes.
pub const MAX_DESCRIBED: usize = 2 << 10;
/// The most of a plan kept, in bytes.
pub const MAX_PLAN: usize = 64 << 10;

/// One question an `AskUserQuestion` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// The question, whose text is also its answer's key.
    pub question: String,
    /// Its short label, if any.
    pub header: String,
    /// Each option's label and description.
    pub options: Vec<(String, String)>,
    /// It takes several options.
    pub multi: bool,
}

/// The questions of an `AskUserQuestion`'s `input`, if it asks any as the tool does.
pub fn questions(input: &Value) -> Option<Vec<Asked>> {
    let listed = input.get("questions")?.as_array()?;
    let text_of = |question: &Value| {
        let text = question.get("question").and_then(Value::as_str);
        text.unwrap_or("").len()
    };
    if (listed.iter().take(MAX_QUESTIONS)).any(|question| text_of(question) > MAX_ASKED) {
        return None;
    }
    let asked: Vec<Asked> = listed
        .iter()
        .take(MAX_QUESTIONS)
        .filter_map(|question| {
            let text = question.get("question")?.as_str()?;
            let field = |value: &Value, key: &str| {
                let text = value.get(key).and_then(Value::as_str).unwrap_or("").trim();
                let first = text.lines().next().unwrap_or("").trim();
                let (kept, cut) = capped(first, MAX_NAME);
                (kept.to_owned(), cut || first.len() < text.len())
            };
            let options = (question.get("options")?.as_array()?.iter())
                .take(MAX_OPTIONS)
                .filter_map(|option| {
                    let (label, cut) = field(option, "label");
                    (!label.is_empty()).then(|| (label, described(option, cut)))
                })
                .collect::<Vec<_>>();
            Some(Asked {
                question: text.to_owned(),
                header: match field(question, "header") {
                    (header, false) => header,
                    (header, true) => format!("{header} … (cut)"),
                },
                options,
                multi: question.get("multiSelect").and_then(Value::as_bool) == Some(true),
            })
        })
        .filter(|asked| !asked.question.is_empty() && !asked.options.is_empty())
        .collect();
    (!asked.is_empty()).then_some(asked)
}

/// What `option` says of itself, its lines kept, cut to [`MAX_DESCRIBED`] bytes, after that its
/// label is cut if it is, as the label is the answer.
fn described(option: &Value, label_cut: bool) -> String {
    let text = option.get("description").and_then(Value::as_str);
    let (kept, cut) = capped(text.unwrap_or("").trim(), MAX_DESCRIBED);
    let cut = if cut { " … (cut)" } else { "" };
    let label = if label_cut { "(its label is cut) " } else { "" };
    format!("{label}{kept}{cut}")
}

/// The plan an `ExitPlanMode`'s `input` gives, cut to [`MAX_PLAN`], and whether it was cut.
pub fn plan(input: &Value) -> Option<(String, bool)> {
    let plan = input.get("plan")?.as_str()?;
    let (kept, cut) = capped(plan, MAX_PLAN);
    Some((kept.to_owned(), cut))
}
