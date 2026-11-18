; oso's highlight query for JSON (ADR 0157).
;
; Written from the highlights query of tree-sitter-json 0.24.8
; (https://github.com/tree-sitter/tree-sitter-json), MIT, Copyright (c) 2014 Max Brunsfeld.
; Cut to oso's highlight classes: each capture names one, and what has none (punctuation) stays
; plain. An object's key is a property, its quotes too. Where two patterns capture the same
; node, the first one wins.
;
; Predicates: none.

(pair
  key: (string) @property)

(string) @string

[
  (number)
  (true)
  (false)
  (null)
] @constant

; Nothing inside a comment may be captured: C-e takes a comment's run for the whole comment
; (ADR 0111, step 6), and a test checks.
(comment) @comment
