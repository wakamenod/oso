//! The keys of worktree sessions (ADR 0121): `C-x a w` waits for the branches, `C-c C-c` and
//! `C-c C-k` in `*Brief*` send and drop the brief, and in the read-only `*Worktrees*` the
//! letters `g` and `D` run their commands rather than type.
//!
//! Must not depend on: `app` or winit; these drive the default dispatcher.

use oso_core::buffer::{Buffer, Mode};
use oso_core::editor::{Editor, SessionRequest};

use super::Dispatcher;
use super::motion_tests::send;

#[test]
fn c_x_a_w_waits_for_the_branches() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut d, &mut editor, "C-x a w");
    assert!(editor.prompt().is_none());
    assert!(d.take_branches_request());
    d.branches_answered(&mut editor, Ok(vec!["main".into()]));
    assert_eq!(editor.prompt().unwrap().label(), "Worktree branch:");
    send(&mut d, &mut editor, "f i x RET");
    assert_eq!(
        editor.take_session_requests(),
        [SessionRequest::Worktree {
            branch: "fix".into(),
            base: None
        }]
    );
}

#[test]
fn a_brief_takes_typing_and_its_own_keys() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let brief = editor.add_buffer(Buffer::from_text("*Brief*", ""));
    assert!(editor.buffer_mut(brief).unwrap().set_mode(Mode::Brief));
    editor.show(brief);
    send(&mut d, &mut editor, "h i C-c C-c C-c C-k");
    assert_eq!(editor.current().1.text().to_string(), "hi");
    assert_eq!(
        editor.take_session_requests(),
        [
            SessionRequest::SendBrief(brief),
            SessionRequest::CancelBrief(brief)
        ]
    );
}

#[test]
fn the_worktrees_letters_run_commands() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let list = editor.add_internal_buffer("*Worktrees*", Mode::Worktrees);
    editor.set_internal_text(list, "worktrees\n");
    editor.show(list);
    send(&mut d, &mut editor, "g D");
    assert_eq!(
        editor.take_session_requests(),
        [
            SessionRequest::WorktreeList,
            SessionRequest::WorktreeRemove(None)
        ]
    );
    send(&mut d, &mut editor, "x");
    assert_eq!(editor.current().1.text().to_string(), "worktrees\n");
}

#[test]
fn m_x_offers_the_worktree_commands_outside_their_list() {
    let (d, editor) = (Dispatcher::new(), Editor::default());
    let items = d.command_candidates(&editor);
    let listed = |name: &str| items.iter().any(|item| item.text() == name);
    assert!(listed("worktree-list"));
    assert!(listed("worktree-remove"));
    assert!(listed("session-worktree"));
    // The list's own keys are offered in the list alone.
    assert!(!listed("worktree-list-refresh"));
    assert!(!listed("worktree-list-remove"));
}
