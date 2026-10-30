[
  "abstract"
  "assert"
  "break"
  "case"
  "catch"
  "class"
  "continue"
  "default"
  "do"
  "else"
  "enum"
  "exports"
  "extends"
  "final"
  "finally"
  "for"
  "if"
  "implements"
  "import"
  "instanceof"
  "interface"
  "module"
  "native"
  "new"
  "non-sealed"
  "open"
  "opens"
  "package"
  "permits"
  "private"
  "protected"
  "provides"
  "public"
  "requires"
  "record"
  "return"
  "sealed"
  "static"
  "strictfp"
  "switch"
  "synchronized"
  "throw"
  "throws"
  "to"
  "transient"
  "transitive"
  "try"
  "uses"
  "volatile"
  "when"
  "while"
  "with"
  "yield"
] @keyword

[
  (line_comment)
  (block_comment)
] @comment

[
  (true)
  (false)
  (null_literal)
] @boolean

(escape_sequence) @string.escape

[
  (character_literal)
  (string_literal)
] @string

[
  (hex_integer_literal)
  (decimal_integer_literal)
  (octal_integer_literal)
  (decimal_floating_point_literal)
  (hex_floating_point_literal)
] @number

(this) @variable.special

((identifier) @constant
 (#match? @constant "^_*[A-Z][A-Z\\d_]+$"))

[
  (boolean_type)
  (integral_type)
  (floating_point_type)
  (floating_point_type)
  (void_type)
] @enum

(constructor_declaration
  name: (identifier) @type)

((method_reference
  . (identifier) @type)
 (#match? @type "^[A-Z]"))

((method_invocation
  object: (identifier) @type)
 (#match? @type "^[A-Z]"))

((scoped_identifier
  scope: (identifier) @type)
 (#match? @type "^[A-Z]"))

((field_access
  object: (identifier) @type)
 (#match? @type "^[A-Z]"))

(enum_declaration
  name: (identifier) @type)

(class_declaration
  name: (identifier) @type)

(interface_declaration
  name: (identifier) @type)

(type_identifier) @type

"@" @operator

(marker_annotation
  name: (identifier) @attribute)

(annotation
  name: (identifier) @attribute)

(super) @function

(method_invocation
  name: (identifier) @function)

(method_declaration
  name: (identifier) @function)

(identifier) @variable
