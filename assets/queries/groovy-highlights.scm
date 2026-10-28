; Minimal Groovy / Gradle highlights (the grammar crate ships none).
[(line_comment) (block_comment) (shebang)] @comment
[(string_literal) (string_fragment) (multiline_string_fragment) (character_literal)] @string
(escape_sequence) @string.escape
(string_interpolation) @embedded
[(decimal_integer_literal) (hex_integer_literal) (octal_integer_literal) (binary_integer_literal)
 (decimal_floating_point_literal) (hex_floating_point_literal)] @number
[(true) (false)] @boolean
(null_literal) @constant
[(this) (super)] @variable.special
(type_identifier) @type
[(integral_type) (floating_point_type) (boolean_type) (void_type)] @type
(annotation name: (identifier) @attribute)
(marker_annotation name: (identifier) @attribute)
(method_declaration name: (identifier) @function)
(method_invocation name: (identifier) @function)
(juxt_function_call) @function
(function_definition) @function
(map_item) @property
[
 "abstract" "as" "assert" "break" "case" "catch" "class" "continue" "def" "default" "do" "else"
 "enum" "extends" "final" "finally" "for" "if" "implements" "import" "in" "instanceof" "interface"
 "new" "package" "private" "protected" "public" "return" "static" "switch" "synchronized" "throw"
 "throws" "try" "while"
] @keyword
