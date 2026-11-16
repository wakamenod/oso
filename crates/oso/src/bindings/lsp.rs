//! The `lsp` minor mode's key bindings (ADR 0138, Keys in buffers a server follows), in a keymap
//! above the major mode's in each buffer a language server follows, as Eglot's minor mode map:
//! `C-M-n` and `C-M-p` move to the next and previous diagnostic, as the author's Flycheck keys
//! do. Later steps add the rest of the layer's keys.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in a buffer a server follows only.
pub const LSP: &[(&str, &str)] = &[
    ("C-M-n", "lsp-next-diagnostic"),
    ("C-M-p", "lsp-previous-diagnostic"),
];
