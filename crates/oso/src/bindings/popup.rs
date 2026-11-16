//! The completion popup's keys (ADR 0138, Completion in the buffer, step 9), a layer above the
//! buffer's while it is open, as `corfu-map`: `C-n` and `C-p` and the arrows move, `TAB`
//! inserts, `C-g` closes. `RET` inserts only once the selection was moved, so an automatic
//! popup never takes a newline: until then it is not bound here, and goes on to the buffer.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// The popup's keys.
pub const POPUP: &[(&str, &str)] = &[
    ("C-n", "completion-popup-next"),
    ("<down>", "completion-popup-next"),
    ("C-p", "completion-popup-previous"),
    ("<up>", "completion-popup-previous"),
    ("TAB", "completion-popup-insert"),
    ("C-g", "completion-popup-quit"),
];

/// The popup's keys once its selection was moved: `RET` inserts too.
pub const POPUP_MOVED: &[(&str, &str)] = &[("RET", "completion-popup-insert")];

/// The commands typed into the buffer that keep the popup open, as they edit the word it
/// narrows by.
pub const TYPING: &[&str] = &["self-insert-command", "delete-backward-char"];
