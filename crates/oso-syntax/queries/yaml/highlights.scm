; oso's highlight query for YAML (ADR 0157).
;
; Written from the highlights query of tree-sitter-yaml 0.7.2
; (https://github.com/tree-sitter-grammars/tree-sitter-yaml), MIT, by Ika and Amaan Qureshi.
; Cut to oso's highlight classes: each capture names one, and what has none (punctuation, `-`,
; `:`) stays plain. A mapping's key is a property; a scalar is a string unless it is a number,
; a boolean, null or a date; a tag is a type, and an anchor or alias a macro. Where two
; patterns capture the same node, the first one wins.
;
; Predicates: none.

; Keys, before scalars: of two patterns on one node the first wins.

(block_mapping_pair
  key: (flow_node
    [
      (double_quote_scalar)
      (single_quote_scalar)
      (plain_scalar)
    ] @property))

(flow_pair
  key: (flow_node
    [
      (double_quote_scalar)
      (single_quote_scalar)
      (plain_scalar)
    ] @property))

; A plain key's scalar, so that `1:` or `true:` is a key too, not a number or a boolean.

(block_mapping_pair
  key: (flow_node
    (plain_scalar
      (_) @property)))

(flow_pair
  key: (flow_node
    (plain_scalar
      (_) @property)))

; Scalars

[
  (boolean_scalar)
  (null_scalar)
  (integer_scalar)
  (float_scalar)
  (timestamp_scalar)
] @constant

[
  (double_quote_scalar)
  (single_quote_scalar)
  (block_scalar)
  (string_scalar)
] @string

; Tags, anchors and aliases

(tag) @type

[
  (anchor)
  (alias)
] @macro

; Directives and document markers

[
  (yaml_directive)
  (tag_directive)
  (reserved_directive)
  "---"
  "..."
] @keyword

; Nothing inside a comment may be captured: C-e takes a comment's run for the whole comment
; (ADR 0111, step 6), and a test checks.
(comment) @comment
