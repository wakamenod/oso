//! What `RET` on a request's line shows: the call's whole input as JSON (ADR 0182, "The line").
//!
//! Must not depend on: the app or the UI; plain functions over a parsed value.

use serde_json::Value;

use crate::sessions::native::wire::tools::{MAX_INPUT, pretty};

/// The most bytes of it that are shown, indented.
const MAX_SHOWN: usize = 4 * MAX_INPUT;

/// The input of the call of `tool`, as oso's words say it, of session `name`, as indented JSON, at
/// most [`MAX_SHOWN`] bytes of it: what is left out is said.
pub fn input(name: &str, tool: &str, input: Option<&Value>) -> String {
    let Some(input) = input else {
        let kib = MAX_INPUT >> 10;
        return format!("{tool}, in session {name}: its input is over {kib} KiB, and not shown\n");
    };
    let (text, cut) = pretty(input, MAX_SHOWN);
    let more = if cut {
        "… the rest is not shown here\n"
    } else {
        ""
    };
    format!("{tool}, in session {name}, asks to run with:\n\n{text}\n{more}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{MAX_SHOWN, input};

    #[test]
    fn an_input_is_shown_whole_or_says_that_it_is_not() {
        let said = input("s", "Bash", Some(&json!({"command": "ls", "timeout": 5})));
        assert!(said.contains("\"timeout\": 5\n}\n"), "{said}");
        assert!(!said.contains("not shown"), "{said}");
        // Its lines' indents alone are more than is shown.
        let deep = (0..120).fold(json!(vec![1; 143]), |value, _| json!([value]));
        let said = input("s", "Bash", Some(&json!({"k": vec![deep; 20]})));
        assert!(said.len() > MAX_SHOWN && said.ends_with("the rest is not shown here\n"));
        assert!(input("s", "Bash", None).contains("over 64 KiB, and not shown"));
    }
}
