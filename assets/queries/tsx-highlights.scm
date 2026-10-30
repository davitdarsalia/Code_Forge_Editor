(property_signature
  name: (property_identifier) @function
  type: (type_annotation
    [
      (union_type
        (parenthesized_type
          (function_type)))
      (function_type)
    ]))

(abstract_method_signature
  name: (property_identifier) @function)

(method_signature
  name: (_) @function)

(ambient_declaration
  (function_signature
    name: (identifier) @function))

(ambient_declaration
  "global" @type)

(arrow_function
  parameter: (identifier) @variable)

(required_parameter
  (array_pattern
    (identifier) @variable))

(required_parameter
  (object_pattern
    (pair_pattern
      value: (identifier) @variable)))

(required_parameter
  (object_pattern
    (object_assignment_pattern
      (shorthand_property_identifier_pattern) @variable)))

(required_parameter
  (object_pattern
    (shorthand_property_identifier_pattern) @variable))

(required_parameter
  (rest_pattern
    (identifier) @variable))

(optional_parameter
  pattern: (identifier) @variable)

(required_parameter
  pattern: (identifier) @variable)

(conditional_type
  [
    "?"
    ":"
  ] @keyword.conditional.ternary)

(template_type
  [
    "${"
    "}"
  ] @punctuation.special)

(flow_maybe_type
  "?" @punctuation.special)

(public_field_definition
  [
    "?"
    "!"
  ] @punctuation.special)

(optional_type
  "?" @punctuation.special)

(optional_parameter
  "?" @punctuation.special)

(property_signature
  "?" @punctuation.special)

(method_definition
  "?" @punctuation.special)

(method_signature
  "?" @punctuation.special)

(abstract_method_signature
  "?" @punctuation.special)

"?." @punctuation.delimiter

(opting_type_annotation
  "?:" @punctuation.delimiter)

(adding_type_annotation
  "+?:" @punctuation.delimiter)

(omitting_type_annotation
  "-?:" @punctuation.delimiter)

(index_signature
  ":" @punctuation.delimiter)

(type_predicate_annotation
  ":" @punctuation.delimiter)

(type_annotation
  ":" @punctuation.delimiter)

(intersection_type
  "&" @punctuation.delimiter)

(union_type
  "|" @punctuation.delimiter)

(object_type
  [
    "{|"
    "|}"
  ] @punctuation.bracket)

(type_parameters
  [
    "<"
    ">"
  ] @punctuation.bracket)

(type_arguments
  [
    "<"
    ">"
  ] @punctuation.bracket)

(non_null_expression
  "!" @operator)

(template_literal_type) @string

(import_statement
  "type"
  (import_clause
    (named_imports
      (import_specifier
        name: (identifier) @type))))

(predefined_type) @enum

(type_identifier) @type

[
  "abstract"
  "private"
  "protected"
  "public"
  "readonly"
] @keyword.modifier

(mapped_type_clause
  "as" @keyword.operator)

(as_expression
  "as" @keyword.operator)

[
  "keyof"
  "satisfies"
] @keyword.operator

[
  "namespace"
  "interface"
  "enum"
] @keyword.type

[
  "declare"
  "implements"
  "type"
  "override"
  "module"
  "asserts"
  "infer"
  "is"
  "using"
] @keyword

(import_require_clause
  source: (string) @link_uri)

"require" @keyword.import

((jsx_attribute) @_jsx_attribute
  )

((jsx_element) @_jsx_element
  )

((jsx_attribute
  (property_identifier) @_attr
  (string
    (string_fragment) @link_uri))
  (#any-of? @_attr "href" "src"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @link_text)
  (#eq? @_tag "a"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @text.literal)
  (#any-of? @_tag "code" "kbd"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @emphasis)
  (#eq? @_tag "u"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @emphasis)
  (#any-of? @_tag "s" "del"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @emphasis)
  (#any-of? @_tag "em" "i"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @emphasis.strong)
  (#any-of? @_tag "strong" "b"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @title)
  (#eq? @_tag "h6"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @title)
  (#eq? @_tag "h5"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @title)
  (#eq? @_tag "h4"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @title)
  (#eq? @_tag "h3"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @title)
  (#eq? @_tag "h2"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @title)
  (#eq? @_tag "h1"))

((jsx_element
  (jsx_opening_element
    name: (identifier) @_tag)
  (jsx_text) @title)
  (#eq? @_tag "title"))

(html_character_reference) @variable.special

(html_character_reference) @tag

(jsx_self_closing_element
  (member_expression
    (identifier) @tag
    (property_identifier) @tag))

(jsx_self_closing_element
  ((identifier) @tag
    (#match? @tag "^[A-Z]")))

(jsx_closing_element
  (member_expression
    (identifier) @tag
    (property_identifier) @tag))

(jsx_closing_element
  ((identifier) @tag
    (#match? @tag "^[A-Z]")))

(jsx_opening_element
  (member_expression
    (identifier) @tag
    (property_identifier) @tag))

(jsx_opening_element
  ((identifier) @tag
    (#match? @tag "^[A-Z]")))

(jsx_self_closing_element
  name: (identifier) @tag)

(jsx_closing_element
  name: (identifier) @tag)

(jsx_opening_element
  name: (identifier) @tag)

(jsx_attribute
  (property_identifier) @attribute)

(jsx_self_closing_element
  [
    "<"
    "/>"
  ] @punctuation)

(jsx_element
  close_tag: (jsx_closing_element
    [
      "</"
      ">"
    ] @punctuation))

(jsx_element
  open_tag: (jsx_opening_element
    [
      "<"
      ">"
    ] @punctuation))

(switch_default
  "default" @keyword.conditional)

(export_statement
  "default" @keyword)

[
  "throw"
  "try"
  "catch"
  "finally"
] @keyword.exception

[
  "new"
  "delete"
  "in"
  "instanceof"
  "typeof"
] @keyword.operator

"function" @keyword.function

[
  "return"
  "yield"
] @keyword.return

[
  "async"
  "await"
] @keyword.coroutine

"class" @keyword.type

[
  "break"
  "const"
  "debugger"
  "extends"
  "get"
  "let"
  "set"
  "static"
  "target"
  "var"
  "with"
] @keyword

[
  "for"
  "of"
  "do"
  "while"
  "continue"
] @keyword.repeat

[
  "import"
  "from"
  "as"
  "export"
] @keyword.import

[
  "if"
  "else"
  "switch"
  "case"
] @keyword.conditional

(export_statement
  "*" @variable.special)

(namespace_export
  "*" @variable.special
  (identifier) @type)

(namespace_import
  "*" @variable.special
  (identifier) @type)

(template_substitution
  [
    "${"
    "}"
  ] @punctuation.special) 

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

(unary_expression
  [
    "delete"
    "void"
  ] @keyword.operator)

(unary_expression
  [
    "!"
    "~"
    "-"
    "+"
  ] @operator)

(ternary_expression
  [
    "?"
    ":"
  ] @keyword.conditional.ternary)

(binary_expression
  "/" @operator)

[
  "--"
  "-"
  "-="
  "&&"
  "+"
  "++"
  "+="
  "&="
  "/="
  "**="
  "<<="
  "<"
  "<="
  "<<"
  "="
  "=="
  "==="
  "!="
  "!=="
  "=>"
  ">"
  ">="
  ">>"
  "||"
  "%"
  "%="
  "*"
  "**"
  ">>>"
  "&"
  "|"
  "^"
  "??"
  "*="
  ">>="
  ">>>="
  "^="
  "|="
  "&&="
  "||="
  "??="
  "..."
] @operator

[
  ";"
  "."
  ","
  ":"
] @punctuation.delimiter

((identifier) @number
  (#any-of? @number "NaN" "Infinity"))

(number) @number

(regex
  "/" @punctuation.bracket)

(regex_flags) @variable.special

(regex_pattern) @string.regex

(escape_sequence) @string.escape

(template_string) @string

(string) @string

((string_fragment) @preproc
  (#eq? @preproc "use strict"))

(hash_bang_line) @preproc

((comment) @comment.doc
  (#match? @comment.doc "^/[*][*][^*].*[*]/$"))

[
  (comment)
  (html_comment)
] @comment 

[
  (null)
  (undefined)
] @boolean

[
  (true)
  (false)
] @boolean

((identifier) @variable.special
  (#eq? @variable.special "self"))

[
  (this)
  (super)
] @variable.special

(decorator
  "@" @attribute
  (call_expression
    (member_expression
      (property_identifier) @attribute)))

(decorator
  "@" @attribute
  (member_expression
    (property_identifier) @attribute))

(decorator
  "@" @attribute
  (call_expression
    (identifier) @attribute))

(decorator
  "@" @attribute
  (identifier) @attribute)

(new_expression
  constructor: (identifier) @constructor)

((identifier) @function
  (#any-of? @function
    "eval" "isFinite" "isNaN" "parseFloat" "parseInt" "decodeURI" "decodeURIComponent" "encodeURI"
    "encodeURIComponent" "require"))

((identifier) @type
  (#eq? @type "Intl"))

(call_expression
  function: (await_expression
    (member_expression
      property: [
        (property_identifier)
        (private_property_identifier)
      ] @function)))

(call_expression
  function: (await_expression
    (identifier) @function))

(call_expression
  function: (member_expression
    property: [
      (property_identifier)
      (private_property_identifier)
    ] @function))

(call_expression
  function: (identifier) @function)

(assignment_expression
  left: (identifier) @function
  right: (function_expression))

(assignment_expression
  left: (identifier) @function
  right: (arrow_function))

(variable_declarator
  name: (identifier) @function
  value: (function_expression))

(variable_declarator
  name: (identifier) @function
  value: (arrow_function))

(assignment_expression
  left: (member_expression
    property: (property_identifier) @function)
  right: (function_expression))

(assignment_expression
  left: (member_expression
    property: (property_identifier) @function)
  right: (arrow_function))

(pair
  key: (property_identifier) @function
  value: (arrow_function))

(pair
  key: (property_identifier) @function
  value: (function_expression))

(method_definition
  name: (property_identifier) @constructor
  (#eq? @constructor "constructor"))

(method_definition
  name: [
    (property_identifier)
    (private_property_identifier)
  ] @function)

(generator_function_declaration
  name: (identifier) @function)

(generator_function
  name: (identifier) @function)

(function_declaration
  name: (identifier) @function)

(function_expression
  name: (identifier) @function)

(statement_identifier) @label

((identifier) @enum
  (#any-of? @enum
    "Object" "Function" "Boolean" "Symbol" "Number" "Math" "Date" "String" "RegExp" "Map" "Set"
    "WeakMap" "WeakSet" "Promise" "Array" "Int8Array" "Uint8Array" "Uint8ClampedArray" "Int16Array"
    "Uint16Array" "Int32Array" "Uint32Array" "Float32Array" "Float64Array" "ArrayBuffer" "DataView"
    "Error" "EvalError" "InternalError" "RangeError" "ReferenceError" "SyntaxError" "TypeError"
    "URIError"))

((identifier) @variable.special
  (#any-of? @variable.special "arguments" "module" "console" "window" "document"))

((shorthand_property_identifier) @constant
  (#match? @constant "^_*[A-Z][A-Z\\d_]*$"))

((identifier) @constant
  (#match? @constant "^_*[A-Z][A-Z\\d_]*$"))

((identifier) @type
  (#match? @type "^[A-Z]"))

(object_pattern
  (object_assignment_pattern
    (shorthand_property_identifier_pattern) @variable))

(object_pattern
  (shorthand_property_identifier_pattern) @variable)

(private_property_identifier) @property

(shorthand_property_identifier) @property

(property_identifier) @property

(identifier) @variable
