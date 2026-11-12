//! Markdown's own key bindings (ADR 0115), in a keymap above the global one in a Markdown buffer:
//! `M-RET` starts a new list item.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in a Markdown buffer only.
pub const MARKDOWN: &[(&str, &str)] = &[("M-RET", "markdown-insert-list-item")];
