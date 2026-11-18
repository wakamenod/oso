; oso's highlight query for CSS (ADR 0157).
;
; Written from the highlights query of tree-sitter-css 0.25.0
; (https://github.com/tree-sitter/tree-sitter-css), MIT, Copyright (c) 2018 Max Brunsfeld.
; Cut to oso's highlight classes: each capture names one, and what has none (plain values such
; as `red` or `auto`, operators, punctuation) stays plain. A tag selector is a function, as the
; HTML tag it names is; classes, ids, pseudo-classes and pseudo-elements are types. Where two
; patterns capture the same node, the first one wins; an inner node's capture paints over an
; outer one's.
;
; Predicates: only #eq? and #match? (a unit test checks).

; Selectors: a pseudo-element's name before any tag's, as of two patterns on one node the first
; wins.

(pseudo_element_selector
  (tag_name) @type)

(tag_name) @function

[
  (class_name)
  (id_name)
  (attribute_name)
] @type

(attribute_selector
  (plain_value) @string)

; Declarations: custom properties and their uses are properties too.

[
  (property_name)
  (feature_name)
] @property

((plain_value) @property
  (#match? @property "^--"))

; `url(...)`'s address is a string, and its name a function as any other's.

((call_expression
  (function_name) @function
  (arguments
    (plain_value) @string))
  (#eq? @function "url"))

(function_name) @function

; Keywords

[
  "@media"
  "@import"
  "@charset"
  "@namespace"
  "@supports"
  "@keyframes"
  "@scope"
  "and"
  "or"
  "not"
  "only"
  (at_keyword)
  (to)
  (from)
  (important)
] @keyword

; Literals

(string_value) @string

[
  (color_value)
  (integer_value)
  (float_value)
] @constant

; Nothing inside a comment may be captured: C-e takes a comment's run for the whole comment
; (ADR 0111, step 6), and a test checks.
[
  (comment)
  (js_comment)
] @comment
