; oso's highlight query for Markdown's blocks (ADR 0111). The text inside them is the inline
; grammar's, which queries/markdown_inline highlights.
;
; Written from the block highlights query of tree-sitter-md 0.5.4
; (https://github.com/tree-sitter-grammars/tree-sitter-markdown), MIT, by MDeiml, itself
; from nvim-treesitter. Cut to oso's highlight classes: each capture
; names one, and what has none stays plain. Code blocks stay plain until a later step injects
; their languages.
;
; Predicates: none.

(atx_heading
  (inline) @heading)

(setext_heading
  (paragraph) @heading)

[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
] @markup

(link_title) @string

(link_destination) @link

(link_label) @link

[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
  (thematic_break)
] @markup

[
  (block_continuation)
  (block_quote_marker)
] @markup

(backslash_escape) @string
