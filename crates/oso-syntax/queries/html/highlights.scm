; oso's highlight query for HTML (ADR 0157).
;
; Written from the highlights query of tree-sitter-html 0.23.2
; (https://github.com/tree-sitter/tree-sitter-html), MIT, Copyright (c) 2014 Max Brunsfeld.
; Cut to oso's highlight classes: each capture names one, and what has none (text, the angle
; brackets, an end tag that closes nothing) stays plain. A tag's name is a function, as JSX's
; is (ADR 0147), and the doctype a keyword. The code in `<script>` and `<style>` is not this
; query's: another grammar parses it, or it stays plain.
;
; Predicates: none.

(tag_name) @function

(attribute_name) @property

[
  (attribute_value)
  (quoted_attribute_value)
] @string

(entity) @constant

(doctype) @keyword

; Nothing inside a comment may be captured: C-e takes a comment's run for the whole comment
; (ADR 0111, step 6), and a test checks.
(comment) @comment
