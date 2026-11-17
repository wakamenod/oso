; oso's highlight query for what JavaScript, TypeScript and TSX share (ADR 0147): JavaScript's
; query is the jsx part and this one, TypeScript's the typescript part and this one, TSX's all
; three, the more specific parts first. Every pattern here compiles in all three grammars.
;
; Written from the highlights query of tree-sitter-javascript 0.25.0
; (https://github.com/tree-sitter/tree-sitter-javascript), MIT, Copyright (c) 2014 Max
; Brunsfeld. Cut to oso's highlight classes: each capture names one, and what has none
; (variables, parameters, punctuation, operators, a template's `${` and `}`) stays plain. Of two
; captures with the same span the first pattern wins, a parent's and its child's too; otherwise
; an inner node's capture paints over an outer one's.
;
; Predicates: only #eq?, #match? and #any-of? (a unit test checks).

; Decorators, before calls: `@` and the decorator's name.

(decorator "@" @macro)
(decorator (identifier) @macro)
(decorator (member_expression property: (property_identifier) @macro))
(decorator (call_expression function: (identifier) @macro))
(decorator
  (call_expression function: (member_expression property: (property_identifier) @macro)))

; Function and method definitions

(function_expression
  name: (identifier) @function)
(function_declaration
  name: (identifier) @function)
(generator_function_declaration
  name: (identifier) @function)
(method_definition
  name: [(property_identifier) (private_property_identifier)] @function)

(pair
  key: (property_identifier) @function
  value: [(function_expression) (arrow_function)])

(assignment_expression
  left: (member_expression
    property: (property_identifier) @function)
  right: [(function_expression) (arrow_function)])

(variable_declarator
  name: (identifier) @function
  value: [(function_expression) (arrow_function)])

(assignment_expression
  left: (identifier) @function
  right: [(function_expression) (arrow_function)])

; Function and method calls

(call_expression
  function: (identifier) @function)

(call_expression
  function: (member_expression
    property: [(property_identifier) (private_property_identifier)] @function))

; Naming conventions: all-caps names are constants, other capitalised ones types.

([
  (identifier)
  (shorthand_property_identifier)
  (shorthand_property_identifier_pattern)
 ] @constant
 (#match? @constant "^[A-Z_][A-Z\\d_]+$"))

((identifier) @type
 (#match? @type "^[A-Z]"))

; Properties, after methods.

[
  (property_identifier)
  (private_property_identifier)
] @property

; Comments. Nothing inside a comment may be captured: C-e takes a comment's run for the whole
; comment (ADR 0111, step 6), and a test checks.

(comment) @comment
(hash_bang_line) @comment

; Literals. A template string's text is a string, and the code in its `${…}` is code.

(string) @string
(template_string "`" @string)
(template_string (string_fragment) @string)
(template_string (escape_sequence) @string)
(regex) @string

[
  (number)
  (true)
  (false)
  (null)
  (undefined)
] @constant

; Keywords

[
  (this)
  (super)
] @keyword

[
  "as"
  "async"
  "await"
  "break"
  "case"
  "catch"
  "class"
  "const"
  "continue"
  "debugger"
  "default"
  "delete"
  "do"
  "else"
  "export"
  "extends"
  "finally"
  "for"
  "from"
  "function"
  "get"
  "if"
  "import"
  "in"
  "instanceof"
  "let"
  "meta"
  "new"
  "of"
  "return"
  "set"
  "static"
  "switch"
  "target"
  "throw"
  "try"
  "typeof"
  "using"
  "var"
  "void"
  "while"
  "with"
  "yield"
] @keyword
