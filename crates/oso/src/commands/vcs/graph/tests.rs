//! Tests of the graph's commands: what each asks of the host from a status, a graph and a
//! file of the project, the keys of a graph, and refusals where they mean nothing.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::Args;
use oso_core::editor::{Editor, GitRequest, GraphAct, GraphScope};

use super::{GRAPH_REFS_LIST, KEYS, listed_takes_input};
use crate::commands::tests::{editor, run};

fn in_graph() -> (
    Editor,
    oso_core::buffer::BufferId,
    oso_core::project::ProjectId,
) {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let id = editor.add_git_graph(project, "*graph: p*");
    editor.show(id);
    (editor, id, project)
}

#[test]
fn the_log_menu_s_heads_ask_for_their_scope_in_the_project() {
    let (mut editor, _, project) = in_graph();
    for name in ["git-graph", "git-graph-all", "git-graph-other"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    let graph = |scope| GitRequest::Graph { project, scope };
    assert_eq!(
        editor.take_git_requests(),
        [
            graph(GraphScope::Current),
            graph(GraphScope::All),
            graph(GraphScope::Pick),
        ]
    );
}

#[test]
fn a_graph_s_keys_ask_for_the_commit_at_point() {
    let (mut editor, buffer, _) = in_graph();
    for name in [
        "git-graph-review",
        "git-graph-copy-id",
        "git-graph-refresh",
        "git-graph-toggle-all",
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    let act = |act| GitRequest::GraphAct {
        buffer,
        act,
        line: 0,
    };
    assert_eq!(
        editor.take_git_requests(),
        [
            act(GraphAct::Review),
            act(GraphAct::CopyId),
            act(GraphAct::Refresh),
            GitRequest::GraphToggle { buffer },
        ]
    );
}

#[test]
fn outside_a_graph_its_keys_say_so() {
    let mut editor = editor("text\n");
    for name in ["git-graph-review", "git-graph-toggle-all"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_err(), "{name}");
    }
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn the_graph_s_keys_are_magit_s_and_its_prompt_takes_what_is_typed() {
    for (key, name) in [("RET", "git-graph-review"), ("w", "git-graph-copy-id")] {
        assert!(KEYS.contains(&(key, name)), "{key}");
    }
    assert!(listed_takes_input(GRAPH_REFS_LIST));
}

#[test]
fn the_ref_prompt_s_answer_asks_for_that_ref_and_an_empty_one_is_refused() {
    let (mut editor, _, project) = in_graph();
    let mut answer = |text: &str| {
        let registry = crate::commands::registry();
        let id = registry.lookup("git-graph-of").unwrap();
        let invocation = oso_core::command::Invocation {
            answer: Some(text),
            ..oso_core::command::Invocation::default()
        };
        let mut ctx = oso_core::command::Ctx::invoked(&mut editor, invocation);
        registry.execute(id, &mut ctx, Args::NONE).is_ok()
    };
    assert!(answer(" origin/main "));
    assert!(!answer("  "));
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::Graph {
            project,
            scope: GraphScope::Named("origin/main".into()),
        }]
    );
}

#[test]
fn the_keys_of_4b_ask_for_their_acts_and_c_u_t_annotates() {
    let (mut editor, buffer, _) = in_graph();
    for name in [
        "git-graph-mark",
        "git-graph-checkout",
        "git-graph-branch",
        "git-graph-tag",
        "git-graph-cherry-pick",
        "git-graph-revert",
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    let (result, _) = run(&mut editor, "git-graph-tag", Args::numeric(4));
    assert!(result.is_ok());
    let act = |act| GitRequest::GraphAct {
        buffer,
        act,
        line: 0,
    };
    assert_eq!(
        editor.take_git_requests(),
        [
            act(GraphAct::Mark),
            act(GraphAct::Checkout),
            act(GraphAct::Branch),
            act(GraphAct::Tag { annotated: false }),
            act(GraphAct::CherryPick),
            act(GraphAct::Revert),
            act(GraphAct::Tag { annotated: true }),
        ]
    );
    let (result, _) = run(&mut editor, "git-graph-confirmed", Args::NONE);
    assert!(result.is_err());
}

#[test]
fn a_graph_prompt_s_answer_goes_to_the_host_as_read() {
    let (mut editor, buffer, _) = in_graph();
    let registry = crate::commands::registry();
    let id = registry.lookup("git-graph-tag-name").unwrap();
    let invocation = oso_core::command::Invocation {
        answer: Some("v1"),
        ..oso_core::command::Invocation::default()
    };
    let mut ctx = oso_core::command::Ctx::invoked(&mut editor, invocation);
    assert!(registry.execute(id, &mut ctx, Args::NONE).is_ok());
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::GraphAnswer {
            buffer,
            text: "v1".into(),
        }]
    );
}
