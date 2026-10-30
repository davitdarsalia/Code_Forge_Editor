(literal_suffix) @operator

(template_parameter_list
  [
    "<"
    ">"
  ] @punctuation.bracket)

(template_argument_list
  [
    "<"
    ">"
  ] @punctuation.bracket)

"::" @punctuation.delimiter

"<=>" @operator

[
  "new"
  "delete"
  "xor"
  "bitand"
  "bitor"
  "compl"
  "not"
  "xor_eq"
  "and_eq"
  "or_eq"
  "not_eq"
  "and"
  "or"
] @keyword.operator

[
  "public"
  "private"
  "protected"
  "final"
  "virtual"
] @keyword.modifier

[
  "co_await"
  "co_yield"
  "co_return"
] @keyword.coroutine

[
  "class"
  "namespace"
  "template"
  "typename"
  "concept"
] @keyword.type

[
  "decltype"
  "explicit"
  "friend"
  "override"
  "using"
  "requires"
  "constexpr"
] @keyword

[
  "try"
  "catch"
  "noexcept"
  "throw"
] @keyword.exception

(raw_string_literal) @string

(false) @boolean

(true) @boolean

(null
  "nullptr" @boolean)

(this) @variable.special

((field_initializer
  (field_identifier) @constructor
  (argument_list))
  (#match? @constructor "^A-Z"))

((call_expression
  function: (field_expression
    field: (field_identifier) @constructor))
  (#match? @constructor "^A-Z"))

((call_expression
  function: (qualified_identifier
    name: (identifier) @constructor))
  (#match? @constructor "^A-Z"))

((call_expression
  function: (identifier) @constructor)
  (#match? @constructor "^A-Z"))

((function_declarator
  (qualified_identifier
    (identifier) @constructor))
  (#match? @constructor "^A-Z"))

(call_expression
  (field_expression
    (field_identifier) @function))

(function_declarator
  (template_method
    (field_identifier) @function))

(call_expression
  (qualified_identifier
    (qualified_identifier
      (qualified_identifier
        (template_function
          (identifier) @function)))))

(call_expression
  (qualified_identifier
    (qualified_identifier
      (template_function
        (identifier) @function))))

(call_expression
  (qualified_identifier
    (template_function
      (identifier) @function)))

(call_expression
  (template_function
    (identifier) @function))

(call_expression
  (qualified_identifier
    (qualified_identifier
      (qualified_identifier
        (identifier) @function))))

(call_expression
  (qualified_identifier
    (qualified_identifier
      (identifier) @function)))

(call_expression
  (qualified_identifier
    (identifier) @function))

"static_assert" @function

"operator" @function

(operator_name) @function

(function_declarator
  (template_function
    (identifier) @function))

(function_declarator
  (qualified_identifier
    (qualified_identifier
      (qualified_identifier
        (identifier) @function))))

(function_declarator
  (qualified_identifier
    (qualified_identifier
      (identifier) @function)))

(function_declarator
  (qualified_identifier
    (identifier) @function))

(destructor_name
  (identifier) @function)

(using_declaration
  .
  "using"
  .
  "namespace"
  .
  [
    (qualified_identifier)
    (identifier)
  ] @type)

(case_statement
  value: (qualified_identifier
    (identifier) @constant))

((namespace_identifier) @type
  (#match? @type "^[A-Z]"))

(namespace_identifier) @type

(auto) @enum

(alias_declaration
  name: (type_identifier) @type)

(concept_definition
  name: (identifier) @type)

(function_declarator
  declarator: (field_identifier) @function)

(field_initializer
  (field_identifier) @property)

(field_declaration
  (field_identifier) @property)

(optional_parameter_declaration
  declarator: (_) @variable)

(variadic_parameter_declaration
  declarator: (variadic_declarator
    (_) @variable))

(parameter_declaration
  declarator: (reference_declarator) @variable)

((identifier) @property
  (#match? @property "^m_.*$"))

[
  "__attribute__"
  "__declspec"
  "__based"
  "__cdecl"
  "__clrcall"
  "__stdcall"
  "__fastcall"
  "__thiscall"
  "__vectorcall"
  (ms_pointer_modifier)
  (attribute_declaration)
] @attribute

(preproc_params
  (identifier) @variable)

(parameter_declaration
  declarator: (pointer_declarator) @variable)

(parameter_declaration
  declarator: (array_declarator) @variable)

(parameter_declaration
  declarator: (identifier) @variable)

((comment) @comment.doc
  (#match? @comment.doc "^/[*][*][^*].*[*]/$"))

(comment) @comment 

(preproc_function_def
  name: (identifier) @preproc)

(function_declarator
  declarator: (parenthesized_declarator
    (pointer_declarator
      declarator: (field_identifier) @function)))

(function_declarator
  declarator: (identifier) @function)

(call_expression
  function: (field_expression
    field: (field_identifier) @function))

(call_expression
  function: (identifier) @function)

(preproc_defined
  (identifier) @preproc)

(preproc_elifdef
  name: (identifier) @preproc)

(preproc_ifdef
  name: (identifier) @preproc)

(preproc_call
  directive: (preproc_directive) @_u
  argument: (_) @preproc
  (#eq? @_u "#undef"))

(preproc_def
  name: (_) @preproc)

((call_expression
  function: (identifier) @function)
  (#match? @function "^__builtin_"))

(attribute_specifier
  (argument_list
    (call_expression
      function: (identifier) @variable.special)))

(attribute_specifier
  (argument_list
    (identifier) @variable.special))

(preproc_def
  (preproc_arg) @boolean
  
  (#any-of? @boolean
    "stderr" "stdin" "stdout"
    "__FILE__" "__LINE__" "__DATE__" "__TIME__"
    "__STDC__" "__STDC_VERSION__" "__STDC_HOSTED__"
    "__cplusplus" "__OBJC__" "__ASSEMBLER__"
    "__BASE_FILE__" "__FILE_NAME__" "__INCLUDE_LEVEL__"
    "__TIMESTAMP__" "__clang__" "__clang_major__"
    "__clang_minor__" "__clang_patchlevel__"
    "__clang_version__" "__clang_literal_encoding__"
    "__clang_wide_literal_encoding__"
    "__FUNCTION__" "__func__" "__PRETTY_FUNCTION__"
    "__VA_ARGS__" "__VA_OPT__"))

((identifier) @boolean
  
  (#any-of? @boolean
    "stderr" "stdin" "stdout"
    "__FILE__" "__LINE__" "__DATE__" "__TIME__"
    "__STDC__" "__STDC_VERSION__" "__STDC_HOSTED__"
    "__cplusplus" "__OBJC__" "__ASSEMBLER__"
    "__BASE_FILE__" "__FILE_NAME__" "__INCLUDE_LEVEL__"
    "__TIMESTAMP__" "__clang__" "__clang_major__"
    "__clang_minor__" "__clang_patchlevel__"
    "__clang_version__" "__clang_literal_encoding__"
    "__clang_wide_literal_encoding__"
    "__FUNCTION__" "__func__" "__PRETTY_FUNCTION__"
    "__VA_ARGS__" "__VA_OPT__"))

(case_statement
  value: (identifier) @constant)

(enumerator
  name: (identifier) @constant)

(preproc_def
  (preproc_arg) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]+$"))

((identifier) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]+$"))

(sized_type_specifier
  _ @enum
  type: _?)

(primitive_type) @enum

(type_definition
  declarator: (type_identifier) @type)

(linkage_specification
  "extern" @keyword.modifier)

[
  (type_qualifier)
  (gnu_asm_qualifier)
  "__extension__"
] @keyword.modifier

(storage_class_specifier) @keyword.modifier

[
  (type_identifier)
  (type_descriptor)
] @type

(declaration
  type: (type_identifier) @_type
  declarator: (identifier) @label
  (#eq? @_type "__label__"))

(statement_identifier) @label

(field_designator) @property

(preproc_defined) @preproc

(char_literal) @number

(number_literal) @number

(null) @boolean

(escape_sequence) @string.escape

(system_lib_string) @string

(string_literal) @string

(conditional_expression
  [
    "?"
    ":"
  ] @keyword.conditional.ternary)

[
  (true)
  (false)
] @boolean

(comma_expression
  "," @operator)

[
  "="
  "-"
  "*"
  "/"
  "+"
  "%"
  "~"
  "|"
  "&"
  "^"
  "<<"
  ">>"
  "->"
  "<"
  "<="
  ">="
  ">"
  "=="
  "!="
  "!"
  "&&"
  "||"
  "-="
  "+="
  "*="
  "/="
  "%="
  "|="
  "&="
  "^="
  ">>="
  "<<="
  "--"
  "++"
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

"..." @punctuation.special

[
  ";"
  ":"
  ","
  "."
  "::"
] @punctuation.delimiter

"#include" @keyword.import

"#define" @preproc

[
  "#if"
  "#ifdef"
  "#ifndef"
  "#else"
  "#elif"
  "#endif"
  "#elifdef"
  "#elifndef"
  (preproc_directive)
] @preproc

[
  "if"
  "else"
  "case"
  "switch"
] @keyword.conditional

[
  "while"
  "for"
  "do"
  "continue"
  "break"
] @keyword.repeat

"return" @keyword.return

(alignof_expression
  .
  _ @keyword.operator)

[
  "sizeof"
  "offsetof"
] @keyword.operator

[
  "enum"
  "struct"
  "union"
  "typedef"
] @keyword.type

[
  "default"
  "goto"
  "asm"
  "__asm__"
] @keyword

(preproc_def
  (preproc_arg) @variable)

((identifier) @variable
  )
