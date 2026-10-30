(call_expression
  (selector_expression) @_function
  (#any-of? @_function
    "regexp.Match" "regexp.MatchReader" "regexp.MatchString" "regexp.Compile" "regexp.CompilePOSIX"
    "regexp.MustCompile" "regexp.MustCompilePOSIX")
  (argument_list
    .
    [
      (raw_string_literal
        (raw_string_literal_content) @string.regex)
      (interpreted_string_literal
        (interpreted_string_literal_content) @string.regex)
    ]))

(source_file
  (comment)+ @comment.doc
  .
  (var_declaration))

(source_file
  (comment)+ @comment.doc
  .
  (type_declaration))

(source_file
  (comment)+ @comment.doc
  .
  (function_declaration))

(source_file
  (comment)+ @comment.doc
  .
  (const_declaration))

(source_file
  .
  (comment)+ @comment.doc)

(comment) @comment 

(field_declaration
  name: (field_identifier) @property)

(keyed_element
  .
  (literal_element
    (identifier) @property))

[
  (nil)
  (iota)
] @boolean

[
  (true)
  (false)
] @boolean

(imaginary_literal) @number

(float_literal) @number.float

(int_literal) @number

(escape_sequence) @string.escape

(rune_literal) @string

(raw_string_literal) @string

(interpreted_string_literal) @string

"]" @punctuation.bracket

"[" @punctuation.bracket

"}" @punctuation.bracket

"{" @punctuation.bracket

")" @punctuation.bracket

"(" @punctuation.bracket

";" @punctuation.delimiter

":" @punctuation.delimiter

"," @punctuation.delimiter

"." @punctuation.delimiter

((identifier) @function
  (#any-of? @function
    "append" "cap" "clear" "close" "complex" "copy" "delete" "imag" "len" "make" "max" "min" "new"
    "panic" "print" "println" "real" "recover"))

((type_identifier) @enum
  (#any-of? @enum
    "any" "bool" "byte" "comparable" "complex128" "complex64" "error" "float32" "float64" "int"
    "int16" "int32" "int64" "int8" "rune" "string" "uint" "uint16" "uint32" "uint64" "uint8"
    "uintptr"))

[
  "chan"
  "map"
] @enum

[
  "else"
  "case"
  "switch"
  "if"
] @keyword.conditional

[
  "import"
  "package"
] @keyword.import

"for" @keyword.repeat

"go" @keyword.coroutine

"return" @keyword.return

"func" @keyword.function

[
  "type"
  "struct"
  "interface"
] @keyword.type

[
  "break"
  "const"
  "continue"
  "default"
  "defer"
  "goto"
  "range"
  "select"
  "var"
  "fallthrough"
] @keyword

[
  "--"
  "-"
  "-="
  ":="
  "!"
  "!="
  "..."
  "*"
  "*"
  "*="
  "/"
  "/="
  "&"
  "&&"
  "&="
  "&^"
  "&^="
  "%"
  "%="
  "^"
  "^="
  "+"
  "++"
  "+="
  "<-"
  "<"
  "<<"
  "<<="
  "<="
  "="
  "=="
  ">"
  ">="
  ">>"
  ">>="
  "|"
  "|="
  "||"
  "~"
] @operator

((call_expression
  (identifier) @constructor)
  (#match? @constructor "^[mM]ake.+$"))

((call_expression
  (identifier) @constructor)
  (#match? @constructor "^[nN]ew.+$"))

(method_elem
  name: (field_identifier) @function)

(method_declaration
  name: (field_identifier) @function)

(function_declaration
  name: (identifier) @function)

(call_expression
  function: (selector_expression
    field: (field_identifier) @function))

(call_expression
  function: (identifier) @function)

(const_spec
  name: (identifier) @constant)

(label_name) @label

(variadic_parameter_declaration
  (identifier) @variable)

(parameter_declaration
  (identifier) @variable)

(package_identifier) @type

(identifier) @variable

(field_identifier) @property

(type_spec
  name: (type_identifier) @type)

(type_identifier) @type
