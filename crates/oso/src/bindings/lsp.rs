//! The `lsp` minor mode's key bindings (ADR 0138, Keys in buffers a server follows), in a keymap
//! above the major mode's in each buffer a language server follows, as Eglot's minor mode map:
//! `C-M-n` and `C-M-p` move to the next and previous diagnostic, as the author's Flycheck keys
//! do, and `M-g d` lists the project's, as `consult-flymake` (step 6b); `M-.` goes to a
//! definition, `M-?` lists the references (step 8b), and `M-,` and `C-M-,` go back and forward,
//! as the author's xref keys there, so `M-,` is `previous-buffer` only in buffers no server
//! follows (step 8); `C-M-i` completes at point (step 9); `C-c C-d` and `C-h .` describe what is
//! at point in a float, which `C-v` and `M-v` scroll (step 10); and `C-c C-r` lists the code
//! actions of the region or point, as the author's `eglot-code-actions` (step 13). A rename has
//! no key, as `C-c` and a letter are the person's (step 14, Q2); `*lsp-rename*`'s are `rename`'s.
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run, in a buffer a server follows only.
pub const LSP: &[(&str, &str)] = &[
    ("C-M-n", "lsp-next-diagnostic"),
    ("C-M-p", "lsp-previous-diagnostic"),
    ("M-g d", "lsp-diagnostics"),
    ("M-.", "lsp-find-definition"),
    ("M-?", "lsp-find-references"),
    ("M-,", "lsp-go-back"),
    ("C-M-,", "lsp-go-forward"),
    ("C-M-i", "completion-at-point"),
    ("C-c C-d", "lsp-describe-at-point"),
    ("C-h .", "lsp-describe-at-point"),
    ("C-c C-r", "lsp-code-actions"),
];

/// The keys of a server's description at point while it shows, in place of the layer of
/// `C-x TAB`: any other key closes it and does what it always does (step 10).
pub const HOVER: &[(&str, &str)] = &[
    ("C-v", "lsp-describe-scroll-up"),
    ("M-v", "lsp-describe-scroll-down"),
    ("C-g", "lsp-describe-close"),
];
