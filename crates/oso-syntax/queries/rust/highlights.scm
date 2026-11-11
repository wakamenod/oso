; oso's highlight query for Rust (ADR 0111).
;
; Written from the highlights query of tree-sitter-rust 0.24.2
; (https://github.com/tree-sitter/tree-sitter-rust), MIT, Copyright (c) 2017 Maxim Sokolov.
; Cut to oso's highlight classes: each capture names one, and what has none (variables,
; punctuation, operators) stays plain. Where two patterns capture the same node, the first one
; wins; an inner node's capture paints over an outer one's.
;
; Predicates: only #eq?, #match? and #any-of? (a unit test checks).

; Identifiers

(type_identifier) @type
(primitive_type) @type

; Identifier conventions

; All-caps names are constants.
((identifier) @constant
 (#match? @constant "^[A-Z][A-Z\\d_]+$"))

; Other capitalised names are enum constructors.
((identifier) @type
 (#match? @type "^[A-Z]"))

; Capitalised names in paths are types.
((scoped_identifier
  path: (identifier) @type)
 (#match? @type "^[A-Z]"))
((scoped_identifier
  path: (scoped_identifier
    name: (identifier) @type))
 (#match? @type "^[A-Z]"))
((scoped_type_identifier
  path: (identifier) @type)
 (#match? @type "^[A-Z]"))
((scoped_type_identifier
  path: (scoped_identifier
    name: (identifier) @type))
 (#match? @type "^[A-Z]"))

; Qualified names in struct patterns are enum constructors, or struct names.
(struct_pattern
  type: (scoped_type_identifier
    name: (type_identifier) @type))

; Function calls

(call_expression
  function: (identifier) @function)
(call_expression
  function: (field_expression
    field: (field_identifier) @function))
(call_expression
  function: (scoped_identifier
    "::"
    name: (identifier) @function))

(generic_function
  function: (identifier) @function)
(generic_function
  function: (scoped_identifier
    name: (identifier) @function))
(generic_function
  function: (field_expression
    field: (field_identifier) @function))

(macro_invocation
  macro: (identifier) @macro
  "!" @macro)

; Function definitions

(function_item (identifier) @function)
(function_signature_item (identifier) @function)

; Fields, after methods: of two patterns on one node the first wins.

(field_identifier) @property

; Comments

(line_comment) @comment
(block_comment) @comment

; Keywords

(lifetime) @keyword

[
  "as"
  "async"
  "await"
  "break"
  "const"
  "continue"
  "default"
  "dyn"
  "else"
  "enum"
  "extern"
  "fn"
  "for"
  "gen"
  "if"
  "impl"
  "in"
  "let"
  "loop"
  "macro_rules!"
  "match"
  "mod"
  "move"
  "pub"
  "raw"
  "ref"
  "return"
  "static"
  "struct"
  "trait"
  "type"
  "union"
  "unsafe"
  "use"
  "where"
  "while"
  "yield"
] @keyword
(crate) @keyword
(mutable_specifier) @keyword
(use_list (self) @keyword)
(scoped_use_list (self) @keyword)
(scoped_identifier (self) @keyword)
(super) @keyword

; Literals

(char_literal) @string
(string_literal) @string
(raw_string_literal) @string
(escape_sequence) @string

(boolean_literal) @constant
(integer_literal) @constant
(float_literal) @constant

; Attributes

(attribute_item) @macro
(inner_attribute_item) @macro
