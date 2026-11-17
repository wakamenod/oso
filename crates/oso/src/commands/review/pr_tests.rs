//! The review menu's `p`: a pull request named by its number alone, a `#` before it allowed.
//!
//! Must not depend on: `bindings`, `dispatch`, `app` or winit; this runs the command through
//! the registry.

use oso_core::command::Args;
use oso_core::editor::{ReviewRequest, ScopeKind};

use super::tests::in_project;

#[test]
fn a_pull_request_is_reviewed_by_its_number_alone() {
    if cfg!(not(unix)) {
        return;
    }
    let (mut editor, project) = in_project(true);
    let mut answer = |text: &str| {
        let registry = crate::commands::registry();
        let id = registry.lookup("review-pull-request").unwrap();
        let invocation = oso_core::command::Invocation {
            answer: Some(text),
            ..oso_core::command::Invocation::default()
        };
        let mut ctx = oso_core::command::Ctx::invoked(&mut editor, invocation);
        registry.execute(id, &mut ctx, Args::NONE).is_ok()
    };
    assert!(answer(" 12 "));
    assert!(answer("#7"));
    for bad in ["", "  ", "-1", "+1", "1a", "#", "main", "1 2"] {
        assert!(!answer(bad), "{bad:?}");
    }
    let open = |number: &str| ReviewRequest::Open {
        project,
        scope: ScopeKind::PullRequest,
        revision: Some(number.into()),
    };
    assert_eq!(editor.take_review_requests(), [open("12"), open("7")]);
}
