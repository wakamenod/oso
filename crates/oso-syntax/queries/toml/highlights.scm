; oso's highlight query for TOML (ADR 0111).
;
; Written from the highlights query of tree-sitter-toml-ng 0.7.0
; (https://github.com/tree-sitter-grammars/tree-sitter-toml), MIT, by Ika
; and Amaan Qureshi. Cut to oso's highlight
; classes: each capture names one, and what has none (punctuation, `=`) stays plain. Upstream
; captures a whole pair as a property; here only its key is, and a table's name is a type.
;
; Predicates: none.

; Keys

(table
  [
    (bare_key)
    (quoted_key)
    (dotted_key)
  ] @type)

(table_array_element
  [
    (bare_key)
    (quoted_key)
    (dotted_key)
  ] @type)

(pair
  [
    (bare_key)
    (quoted_key)
    (dotted_key)
  ] @property)

; Literals

; Nothing inside a comment may be captured: C-e takes a comment's run for the whole comment
; (ADR 0111, step 6), and a test checks.
(comment) @comment

(string) @string

[
  (offset_date_time)
  (local_date_time)
  (local_date)
  (local_time)
] @string

(boolean) @constant

[
  (integer)
  (float)
] @constant
