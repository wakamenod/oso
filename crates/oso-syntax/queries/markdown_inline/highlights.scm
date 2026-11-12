; oso's highlight query for Markdown's inline text (ADR 0111).
;
; Written from the inline highlights query of tree-sitter-md 0.5.4
; (https://github.com/tree-sitter-grammars/tree-sitter-markdown), MIT, by MDeiml, itself
; from nvim-treesitter. Cut to oso's highlight classes: each capture
; names one, and what has none (a link's brackets) stays plain.
;
; Predicates: none.

[
  (code_span)
  (link_title)
] @string

(emphasis) @markup

(strong_emphasis) @markup

[
  (link_destination)
  (uri_autolink)
] @link

[
  (link_label)
  (link_text)
  (image_description)
] @link

[
  (backslash_escape)
  (hard_line_break)
] @string
