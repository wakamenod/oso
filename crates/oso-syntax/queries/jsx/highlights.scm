; oso's highlight query for JSX, in JavaScript and TSX (ADR 0147), before the ecma part.
;
; Written from the JSX highlights query of tree-sitter-javascript 0.25.0
; (https://github.com/tree-sitter/tree-sitter-javascript), MIT, Copyright (c) 2014 Max
; Brunsfeld. A tag's name is a function, as Emacs's tsx-ts-mode draws it with a call's face, an
; intrinsic one (`div`) and a component (`Button`, `Menu.Item`) alike; an attribute's name is a
; property, which the ecma part gives it; a tag's text and its brackets stay plain. Of two
; captures with the same span the first pattern wins, so these come before the ecma part's
; naming conventions. A name of three parts or more (`<A.B.C>`) and a namespaced one
; (`<svg:rect>`) are not matched here, and take the ecma part's classes.
;
; Predicates: only #eq?, #match? and #any-of? (a unit test checks).

(jsx_opening_element name: (identifier) @function)
(jsx_closing_element name: (identifier) @function)
(jsx_self_closing_element name: (identifier) @function)

(jsx_opening_element
  name: (member_expression
    object: (identifier) @function
    property: (property_identifier) @function))
(jsx_closing_element
  name: (member_expression
    object: (identifier) @function
    property: (property_identifier) @function))
(jsx_self_closing_element
  name: (member_expression
    object: (identifier) @function
    property: (property_identifier) @function))
