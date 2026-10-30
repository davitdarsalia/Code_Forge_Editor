(type_arguments
  [
    "<"
    ">"
  ] @punctuation.bracket)

[
  "+"
  "-"
  "*"
  "/"
  "%"
  "="
  "+="
  "-="
  "*="
  "/="
  "<"
  ">"
  "<<"
  ">>"
  "<="
  ">="
  "++"
  "--"
  "^"
  "&"
  "&&"
  "|"
  "||"
  "~"
  "%="
  "!="
  "!=="
  "=="
  "==="
  "?"
  "??"
  "->"
  "..<"
  "..."
  (bang)
] @operator

(custom_operator) @operator

(regex_literal) @string.regex

(wildcard_pattern) @variable.special

(nil_literal) @boolean

(boolean_literal) @boolean

(real_literal) @number.float

[
  (integer_literal)
  (hex_literal)
  (oct_literal)
  (bin_literal)
] @number

(lambda_literal
  "in" @keyword.operator)

[
  "\""
  "\"\"\""
] @string

(raw_str_interpolation
  [
    (raw_str_interpolation_start)
    ")"
  ] @punctuation.special)

(multi_line_string_literal
  [
    "\\("
    ")"
  ] @punctuation.special)

(line_string_literal
  [
    "\\("
    ")"
  ] @punctuation.special)

(raw_str_end_part) @string

(raw_str_part) @string

(multi_line_str_text) @string

(str_escaped_char) @string.escape

(line_str_text) @string

((multiline_comment) @comment.doc
  (#match? @comment.doc "^/[*][*][^*].*[*]/$"))

((comment) @comment.doc
  (#match? @comment.doc "^///$"))

((comment) @comment.doc
  (#match? @comment.doc "^///[^/]"))

[
  (comment)
  (multiline_comment)
] @comment 

(statement_label) @label

[
  (try_operator)
  "do"
  (throw_keyword)
  (catch_keyword)
] @keyword.exception

(ternary_expression
  [
    "?"
    ":"
  ] @keyword.conditional.ternary)

"return" @keyword.return

(switch_entry
  (default_keyword) @keyword)

(switch_entry
  "fallthrough" @keyword)

(switch_entry
  "case" @keyword)

(switch_statement
  "switch" @keyword.conditional)

(if_statement
  "if" @keyword.conditional)

(guard_statement
  "guard" @keyword.conditional)

[
  "while"
  "repeat"
  "continue"
  "break"
] @keyword.repeat

(for_statement
  "in" @keyword.repeat)

(for_statement
  "for" @keyword.repeat)

(special_literal) @preproc

[
  (diagnostic)
  (availability_condition)
  (playground_literal)
  (key_path_string_expression)
  (selector_expression)
  (external_macro_definition)
] @preproc

(directive) @preproc

((navigation_expression
  (simple_identifier) @type) 
  (#match? @type "^[A-Z]"))

(call_expression
  (prefix_expression
    (simple_identifier) @function))

(call_expression
  
  (navigation_expression
    (navigation_suffix
      (simple_identifier) @function)))

(call_expression
  (simple_identifier) @function)

(modifiers
  (attribute
    "@" @attribute
    (user_type
      (type_identifier) @attribute)))

(enum_entry
  "case" @keyword)

(import_declaration
  "import" @keyword.import)

(value_argument
  name: (value_argument_label
    (simple_identifier) @property))

(navigation_expression
  (navigation_suffix
    (simple_identifier) @property))

(protocol_property_declaration
  (pattern
    (simple_identifier) @property))

(class_body
  (property_declaration
    (pattern
      (simple_identifier) @property)))

(shebang_line) @preproc

(unsafe_expression
  "unsafe" @keyword)

[
  "async"
  "await"
] @keyword.coroutine

[
  "enum"
  "struct"
  "class"
  "typealias"
] @keyword.type

[
  "protocol"
  "extension"
  "indirect"
  "nonisolated"
  "override"
  "convenience"
  "required"
  "some"
  "any"
  "weak"
  "unowned"
  "didSet"
  "willSet"
  "subscript"
  "let"
  "var"
  (throws)
  (where_keyword)
  (getter_specifier)
  (setter_specifier)
  (modify_specifier)
  (else)
  (as_operator)
] @keyword

(equality_constraint
  (identifier
    (simple_identifier) @variable))

(inheritance_constraint
  (identifier
    (simple_identifier) @variable))

(type_parameter
  (type_identifier) @variable)

(parameter
  name: (simple_identifier) @variable)

(parameter
  external_name: (simple_identifier) @variable)

(init_declaration
  "init" @constructor)

(protocol_function_declaration
  name: (simple_identifier) @function)

(function_declaration
  (simple_identifier) @function)

(simple_identifier) @variable

[
  (visibility_modifier)
  (member_modifier)
  (function_modifier)
  (property_modifier)
  (parameter_modifier)
  (inheritance_modifier)
  (mutation_modifier)
] @keyword.modifier

[
  "func"
  "deinit"
] @keyword.function

[
  (self_expression)
  (super_expression)
] @variable.special

(type_identifier) @type

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  "."
  ";"
  ":"
  ","
] @punctuation.delimiter
