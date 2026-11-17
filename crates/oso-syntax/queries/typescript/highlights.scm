; oso's highlight query for what TypeScript and TSX add to JavaScript (ADR 0147), before the
; ecma part.
;
; Written from the highlights query of tree-sitter-typescript 0.23.2
; (https://github.com/tree-sitter/tree-sitter-typescript), MIT, Copyright (c) 2017 Max
; Brunsfeld. Cut to oso's highlight classes; parameters stay plain.
;
; Predicates: only #eq?, #match? and #any-of? (a unit test checks).

; Types

(type_identifier) @type
(predefined_type) @type

; Signatures, which have no body.

(function_signature
  name: (identifier) @function)
(method_signature
  name: [(property_identifier) (private_property_identifier)] @function)
(abstract_method_signature
  name: [(property_identifier) (private_property_identifier)] @function)

; An enum's members are constants.

(enum_body
  name: (property_identifier) @constant)
(enum_assignment
  name: (property_identifier) @constant)

; Keywords

(this_type) @keyword

[
  "abstract"
  "accessor"
  "asserts"
  "declare"
  "enum"
  "global"
  "implements"
  "infer"
  "interface"
  "is"
  "keyof"
  "module"
  "namespace"
  "override"
  "private"
  "protected"
  "public"
  "readonly"
  "satisfies"
  "type"
] @keyword
