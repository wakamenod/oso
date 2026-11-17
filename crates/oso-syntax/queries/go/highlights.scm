; oso's highlight query for Go (ADR 0147).
;
; Written from the highlights query of tree-sitter-go 0.25.0
; (https://github.com/tree-sitter/tree-sitter-go), MIT, Copyright (c) 2014 Max Brunsfeld.
; Cut to oso's highlight classes: each capture names one, and what has none (variables,
; package names, punctuation, operators) stays plain. Where two patterns capture the same node,
; the first one wins; an inner node's capture paints over an outer one's.
;
; Predicates: only #eq?, #match? and #any-of? (a unit test checks).

; Function calls, builtins too, before fields: of two patterns on one node the first wins.

(call_expression
  function: (identifier) @function)
(call_expression
  function: (selector_expression
    field: (field_identifier) @function))
(call_expression
  function: (index_expression
    operand: (identifier) @function))
(call_expression
  function: (index_expression
    operand: (selector_expression
      field: (field_identifier) @function)))

; Function definitions

(function_declaration
  name: (identifier) @function)
(method_declaration
  name: (field_identifier) @function)
(method_elem
  name: (field_identifier) @function)

; Types

(type_identifier) @type

; Constants declared with const.

(const_spec
  name: (identifier) @constant)

; Fields, and the keys of a struct literal.

(field_identifier) @property
(keyed_element
  .
  (literal_element
    (identifier) @property))

; Comments. Nothing inside a comment may be captured: C-e takes a comment's run for the whole
; comment (ADR 0111, step 6), and a test checks.

(comment) @comment

; Keywords

(label_name) @keyword

[
  "break"
  "case"
  "chan"
  "const"
  "continue"
  "default"
  "defer"
  "else"
  "fallthrough"
  "for"
  "func"
  "go"
  "goto"
  "if"
  "import"
  "interface"
  "map"
  "package"
  "range"
  "return"
  "select"
  "struct"
  "switch"
  "type"
  "var"
] @keyword

; Literals

[
  (interpreted_string_literal)
  (raw_string_literal)
  (rune_literal)
] @string

[
  (int_literal)
  (float_literal)
  (imaginary_literal)
  (true)
  (false)
  (nil)
  (iota)
] @constant
