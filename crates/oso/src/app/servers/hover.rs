//! A server's description of what is at point (ADR 0138, Hover, step 10), on the UI thread:
//! the answer to `C-c C-d`'s ask goes to its float, or the echo area says why there is none.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - An answer shows only while nothing moved since the ask, as a definition goes only then
//!   (`places`); a later one is dropped and says nothing. Nothing shows while it waits.
//! - What is said names the server first, so that none passes as oso, and never reaches the
//!   log: a server's text may quote what it read.

use oso_core::buffer::BufferId;
use oso_core::editor::AskAt;
use oso_lsp::server::{Answered, AskKind};

use super::super::App;
use super::places::Asker;

impl App {
    /// Takes the answer to the description asked of server `index`, by `taken`, who asked and
    /// about which buffer, if it is still that ask's.
    pub(super) fn lsp_described(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        outcome: Result<Answered, String>,
    ) {
        let Some((asked, asker)) = taken.filter(|(asked, asker)| self.still_there(*asked, asker))
        else {
            return;
        };
        let name = self.servers.running[index].entry.name.clone();
        let said = match outcome {
            Ok(Answered::Hover(Some(hover))) => {
                let at = AskAt {
                    buffer: asked,
                    revision: asker.revision,
                    offset: asker.offset,
                    agent: false,
                };
                if self.editor.show_hover(at, &hover.text, hover.markdown) {
                    self.request_redraw();
                    return;
                }
                format!("{name}: nothing to describe here")
            }
            Ok(_) => format!("{name}: nothing to describe here"),
            Err(why) if why == overdue() => format!("{name} did not answer"),
            Err(why) => format!("{name}: description failed: {why}"),
        };
        self.editor.message_unlogged(said);
        self.request_redraw();
    }
}

/// What the server's thread says of an ask whose wait ended, as it says it.
fn overdue() -> String {
    format!("no answer within {} s", AskKind::Hover.wait().as_secs())
}
