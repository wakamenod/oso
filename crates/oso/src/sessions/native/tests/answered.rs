//! A call a space's orchestrator answered (ADR 0186): its row says who.
//!
//! Must not depend on: the editor or the UI; rows folded from lines of the stream.

use std::path::PathBuf;

use super::super::rows::{RowChange, Rows};
use super::{read, shown};

/// A call the space's orchestrator allowed says who on its row (ADR 0186), as it runs and once
/// its result came, but not once it failed; one it denied says who too.
#[test]
fn a_call_the_orchestrator_answered_says_who_on_its_row() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    let call = r#"{"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"e","name":"Bash","input":{"command":"ls"}}]}}"#;
    read(&mut rows, call);
    assert_eq!(
        rows.allowed("e", "allowed by fix-login"),
        Some(RowChange::Changed(0))
    );
    assert!(
        shown(&rows).contains("· allowed by fix-login"),
        "{}",
        shown(&rows)
    );
    let done = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"e","content":"a.md"}]}}"#;
    read(&mut rows, done);
    assert!(
        shown(&rows).contains("· allowed by fix-login"),
        "{}",
        shown(&rows)
    );
    assert_eq!(rows.allowed("none", "allowed by x"), None);
    let other = r#"{"type":"assistant","message":{"id":"n","content":[{"type":"tool_use","id":"q","name":"Bash","input":{"command":"make"}}]}}"#;
    read(&mut rows, other);
    rows.allowed("q", "allowed by fix-login");
    let failed = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"q","is_error":true,"content":"no"}]}}"#;
    read(&mut rows, failed);
    assert!(shown(&rows).contains("make · error"), "{}", shown(&rows));
    let third = r#"{"type":"assistant","message":{"id":"o","content":[{"type":"tool_use","id":"d","name":"Bash","input":{"command":"rm x"}}]}}"#;
    read(&mut rows, third);
    rows.denied("d", "by fix-login");
    assert!(
        shown(&rows).contains("rm x · denied: by fix-login"),
        "{}",
        shown(&rows)
    );
}
