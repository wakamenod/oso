//! The method a native session's bridge asks the person's permission with (ADR 0165, step 2),
//! parsed into plain data. Only the sessions' socket answers it, and only for a native
//! session's bridge; the agent channel refuses it.
//!
//! Must not depend on: sockets, threads, editor state or the sessions module.
//!
//! - `permission`: `{"tool_name": "Bash", "tool_use_id": "toolu_…", "input": {…}}`, the call
//!   the session's `claude` asks about through its `--permission-prompt-tool`, with its input
//!   when that is at most [`MAX_INPUT`] bytes as JSON. What the person is asked about is read
//!   from the session's own stream, by the call's id; the input must say the same, and is what
//!   `?` shows whole, as it is what runs.
//! - The reply is `{"behavior": "allow"}` or `{"behavior": "deny", "message": "…"}`; the
//!   bridge gives the CLI the call's own input back with an `allow`. An `AskUserQuestion`'s
//!   `allow` carries `"answers": {QUESTION: ANSWER}` too, which the bridge puts in that input
//!   (step 2e-2).

use serde_json::{Map, Value};

use super::short;

/// The longest tool name taken, in bytes: an MCP tool's is `mcp__SERVER__TOOL`.
pub const MAX_TOOL_NAME: usize = 256;

/// The longest call id taken, in bytes: the API's are about 30.
pub const MAX_CALL_ID: usize = 128;

/// The longest message a denial gives the CLI, in bytes.
pub const MAX_MESSAGE: usize = 4096;

/// The longest input sent, as JSON, in bytes: as much as a native session's rows read of a
/// call's input.
pub const MAX_INPUT: usize = 64 << 10;

/// A call a session's `claude` asks permission for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub tool_name: String,
    pub tool_use_id: String,
    /// Its input, if it was at most [`MAX_INPUT`] bytes as JSON.
    pub input: Option<Value>,
}

/// The parameters of `permission`. A name or an id holds only printable ASCII, so neither can
/// carry what a terminal or a label would read as anything else.
pub fn parse(params: &Map<String, Value>) -> Result<Call, String> {
    let names = ["tool_name", "tool_use_id", "input"];
    if let Some(other) = params.keys().find(|key| !names.contains(&key.as_str())) {
        return Err(format!("permission takes no parameter {:?}", short(other)));
    }
    let text = |key: &str, max: usize| match params.get(key) {
        Some(Value::String(text))
            if !text.is_empty() && text.len() <= max && text.bytes().all(printable) =>
        {
            Ok(text.clone())
        }
        _ => Err(format!(
            "permission needs \"{key}\": printable ASCII of at most {max} bytes"
        )),
    };
    let input = match params.get("input") {
        None | Some(Value::Null) => None,
        Some(input @ Value::Object(_)) if within(input, MAX_INPUT) => Some(input.clone()),
        Some(_) => {
            return Err(format!(
                "permission's \"input\" is an object of at most {MAX_INPUT} bytes as JSON"
            ));
        }
    };
    Ok(Call {
        tool_name: text("tool_name", MAX_TOOL_NAME)?,
        tool_use_id: text("tool_use_id", MAX_CALL_ID)?,
        input,
    })
}

/// Whether `value` is at most `max` bytes as JSON, measured by a writer that stops there.
pub fn within(value: &Value, max: usize) -> bool {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = (self.0.checked_sub(bytes.len())).ok_or(std::io::ErrorKind::FileTooLarge)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(max), value).is_ok()
}

/// Whether `byte` is printable ASCII, a space excluded.
fn printable(byte: u8) -> bool {
    byte.is_ascii_graphic()
}

/// The reply that lets the call run.
pub fn allow() -> Value {
    serde_json::json!({ "behavior": "allow" })
}

/// The reply that lets an `AskUserQuestion` run with the person's `answers`, each a question
/// and its answer, which the bridge gives the CLI in the call's `updatedInput` (step 2e-2).
pub fn allow_answering(answers: &[(String, String)]) -> Value {
    let answers: Map<String, Value> = (answers.iter())
        .map(|(question, answer)| (question.clone(), Value::String(answer.clone())))
        .collect();
    serde_json::json!({ "behavior": "allow", "answers": answers })
}

/// The answers an `allow` carries for an `AskUserQuestion`, if it carries any: each a string.
pub fn answers(reply: &Value) -> Option<Map<String, Value>> {
    let answers = reply.get("answers")?.as_object()?;
    answers
        .values()
        .all(Value::is_string)
        .then(|| answers.clone())
}

/// The reply that refuses the call, telling the CLI `message`, cut to [`MAX_MESSAGE`] bytes.
pub fn deny(message: &str) -> Value {
    let end = message.floor_char_boundary(MAX_MESSAGE);
    serde_json::json!({ "behavior": "deny", "message": &message[..end] })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn params(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn a_call_names_its_tool_and_id() {
        let call = parse(&params(
            json!({"tool_name": "Bash", "tool_use_id": "toolu_01"}),
        ));
        assert_eq!(
            call,
            Ok(Call {
                tool_name: "Bash".into(),
                tool_use_id: "toolu_01".into(),
                input: None,
            })
        );
    }

    #[test]
    fn an_input_is_kept_up_to_its_bound() {
        let input = json!({"command": "ls"});
        let call = parse(&params(
            json!({"tool_name": "Bash", "tool_use_id": "t", "input": input}),
        ));
        assert_eq!(call.unwrap().input, Some(input));
        assert!(
            within(&json!({"c": "x".repeat(10)}), 18) && !within(&json!({"c": "x".repeat(10)}), 17)
        );
    }

    #[test]
    fn anything_else_is_refused() {
        let bad = [
            json!({"tool_name": "Bash"}),
            json!({"tool_name": "Bash", "tool_use_id": "toolu_01", "input": []}),
            json!({"tool_name": "Bash", "tool_use_id": "toolu_01", "input": {"c": "x".repeat(MAX_INPUT)}}),
            json!({"tool_name": "Bash", "tool_use_id": "toolu_01", "other": {}}),
            json!({"tool_name": "Ba sh", "tool_use_id": "toolu_01"}),
            json!({"tool_name": "Bash\u{1b}[2J", "tool_use_id": "toolu_01"}),
            json!({"tool_name": "Bash", "tool_use_id": ""}),
            json!({"tool_name": "Bash", "tool_use_id": 7}),
            json!({"tool_name": "Bash", "tool_use_id": "x".repeat(MAX_CALL_ID + 1)}),
            json!({"tool_name": "é", "tool_use_id": "toolu_01"}),
        ];
        for params_ in bad {
            assert!(parse(&params(params_.clone())).is_err(), "{params_}");
        }
    }

    #[test]
    fn a_denial_is_cut_at_a_character() {
        let long = "é".repeat(MAX_MESSAGE);
        let message = deny(&long)["message"].as_str().unwrap().to_owned();
        assert!(message.len() <= MAX_MESSAGE);
        assert!(long.starts_with(&message));
        assert_eq!(allow(), json!({"behavior": "allow"}));
    }
}
