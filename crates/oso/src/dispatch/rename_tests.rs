//! A rename's keys through the dispatcher (ADR 0138, step 14): `M-x lsp-rename` asks the host,
//! the host's prompt reads the new name with the old on `M-n`, `*lsp-rename*`'s keys apply or
//! drop it, `C-g` stops a rename that waits, and the agent runs none of it.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::{Mode, ServerMark};
use oso_core::editor::ServerRequest;

use super::lsp_tests::{followed, send};

fn typed(name: &str) -> String {
    let keys: Vec<String> = name.chars().map(String::from).collect();
    keys.join(" ")
}

#[test]
fn m_x_lsp_rename_asks_the_host_to_check_the_place() {
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    send(&mut d, &mut editor, "C-f C-f");
    send(
        &mut d,
        &mut editor,
        &format!("M-x {} RET", typed("lsp-rename")),
    );
    match editor.take_server_requests().as_slice() {
        [ServerRequest::Rename(at)] => assert_eq!((at.offset, at.agent), (2, false)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_hosts_prompt_reads_the_new_name_with_the_old_on_m_n() {
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    let prompt = d.read_new_name(&mut editor, "Rename ‘value’ to: ", "value");
    assert!(prompt.is_some());
    assert!(
        d.read_new_name(&mut editor, "Rename ‘value’ to: ", "value")
            .is_none(),
        "not over a prompt"
    );
    send(&mut d, &mut editor, "M-n");
    let id = editor.prompt().unwrap().buffer();
    assert_eq!(editor.buffer(id).unwrap().text().to_string(), "value");
    send(&mut d, &mut editor, "C-a C-k c o u n t RET");
    assert!(editor.prompt().is_none());
    assert_eq!(
        editor.take_server_requests(),
        [ServerRequest::RenameTo("count".into())]
    );
    // By name, it opens nothing.
    send(
        &mut d,
        &mut editor,
        &format!("M-x {} RET", typed("lsp-rename-to")),
    );
    assert!(editor.take_server_requests().is_empty());
}

#[test]
fn the_previews_keys_apply_or_drop_the_rename() {
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    let preview = editor.add_internal_buffer("*lsp-rename*", Mode::RenamePreview);
    editor.set_internal_text(preview, "rust-analyzer renames ‘value’ to ‘count’\n");
    editor.show(preview);
    send(&mut d, &mut editor, "y");
    assert_eq!(editor.take_server_requests(), [ServerRequest::ApplyRename]);
    for key in ["n", "q", "C-g"] {
        send(&mut d, &mut editor, key);
        assert_eq!(
            editor.take_server_requests(),
            [ServerRequest::CancelRename],
            "{key}"
        );
    }
}

#[test]
fn c_g_stops_a_rename_only_while_one_waits() {
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    send(&mut d, &mut editor, "C-g");
    assert!(editor.take_server_requests().is_empty(), "none waits");
    d.set_rename_waits(true);
    send(&mut d, &mut editor, "C-g");
    assert_eq!(editor.take_server_requests(), [ServerRequest::CancelRename]);
}

#[test]
fn the_agent_renames_nothing() {
    use super::args::Given;
    use oso_core::command::Reach;
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    for name in [
        "lsp-rename",
        "lsp-rename-to",
        "lsp-rename-apply",
        "lsp-rename-cancel",
    ] {
        for reach in [Reach::Look, Reach::Change] {
            let ran = d.run_for_agent(&mut editor, name, Given::Nothing, reach);
            assert!(
                ran.is_err() || ran.is_ok_and(|(_, done)| done.is_err()),
                "{name}"
            );
        }
    }
    assert!(editor.take_server_requests().is_empty());
}
