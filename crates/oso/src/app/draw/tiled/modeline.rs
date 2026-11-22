//! A tiled window's modeline, as the spans it is drawn in (ADR 0112): its warnings, the agent's
//! and the sessions' marks, the buffer's name and project, the language server's state and its
//! counts in their severities' faces (ADR 0138), and the rest; and each tiled window's status,
//! which its modeline and the echo area say.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::editor::{Editor, ServerFace, Status};
use oso_core::window::WindowId;
use oso_render::{Face, Span};

/// A modeline's spans: warnings first, so that nothing can push them off the row, after
/// `Agent` while the agent channel is on, dim, and bright while an agent is connected
/// (ADR 0085), which only the selected window's says, as it does the space in front's name
/// after the project (ADR 0159), then the sessions' marks, `⚠ N` and
/// `● N` in their states' faces, as `*Spaces*` draws them (ADR 0121, ADR 0131), and the
/// server's counts in their severities' faces (ADR 0138). A name and the rest are shaped apart,
/// so that neither changes how the other reads. A window that is not selected has its modeline
/// in [`Face::ModelineInactive`] (ADR 0112).
pub(super) fn modeline_spans(status: &Status) -> [Span<'_>; MODELINE_SPANS] {
    let span = |text, face| Span { text, face };
    let agent = if status.agent_connected {
        Face::Warning
    } else {
        Face::Prompt
    };
    let face = modeline_face(status);
    let (waiting, done) = status
        .sessions
        .split_at(status.sessions.find('●').unwrap_or(status.sessions.len()));
    let [errors, warnings, information] = status
        .count_parts()
        .map(|(text, severity)| (text, super::severity_face(severity).unwrap_or(face)));
    [
        span(status.agent, agent),
        span(waiting, Face::SessionAlert),
        span(done, Face::SessionDone),
        span(&status.warnings[..], Face::Warning),
        span(&status.name[..], face),
        span(&status.project[..], face),
        // Dim, as `Agent` is: what the trust record says, never a name (ADR 0114).
        span(status.restricted, Face::Prompt),
        // Dim too: what the space in front is called, in the selected window's (ADR 0159).
        span(&status.space[..], Face::Prompt),
        // The language server's state (ADR 0138), then its counts in their severities' faces.
        span(
            &status.lsp[..],
            match status.lsp_face {
                ServerFace::Dim => Face::Prompt,
                ServerFace::Plain => face,
                ServerFace::Warning => Face::Warning,
            },
        ),
        span(errors.0, errors.1),
        span(warnings.0, warnings.1),
        span(information.0, information.1),
        // Dim too: what an agent's open or save left for the person (ADR 0118).
        span(&status.waits[..], Face::Prompt),
        span(&status.state[..], face),
    ]
}

/// How many spans a modeline is drawn in.
pub(super) const MODELINE_SPANS: usize = 14;

/// The face of a modeline and its text: the selected window's, or that of one not selected
/// (ADR 0112).
pub(super) fn modeline_face(status: &Status) -> Face {
    if status.selected {
        Face::Modeline
    } else {
        Face::ModelineInactive
    }
}

/// Every tiled window's status, in the tree's order, into `out`, reusing its strings.
pub(in crate::app) fn statuses_into(editor: &Editor, out: &mut Vec<(WindowId, Status)>) {
    let windows = editor.frame().windows();
    out.resize_with(windows.len(), || (windows[0].0, Status::default()));
    for ((id, status), (window, _)) in out.iter_mut().zip(windows) {
        *id = *window;
        editor.status_into(*window, status);
    }
}

/// The status of the selected tiled window, or the one beneath the prompts, among
/// `statuses`, which says the echo area too.
pub(in crate::app) fn tiled_status<'a>(
    editor: &Editor,
    statuses: &'a [(WindowId, Status)],
) -> Option<&'a Status> {
    let tiled = editor.tiled().0.id();
    statuses
        .iter()
        .find_map(|(id, status)| (*id == tiled).then_some(status))
}
