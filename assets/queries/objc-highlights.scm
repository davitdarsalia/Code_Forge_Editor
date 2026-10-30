[
  "<"
  ">"
] @punctuation.bracket

"@" @punctuation.special

(version_number) @string.special

(platform) @string.special

"^" @operator

"..." @variable.special

(parameter_declaration
  declarator: (function_declarator
    declarator: (parenthesized_declarator
      (block_pointer_declarator
        declarator: (identifier) @variable))))

(method_parameter
  declarator: (identifier) @variable)

(method_parameter
  ":" @function
  (identifier) @variable)

(property_implementation
  "@synthesize"
  (identifier) @property)

[
  "__asm"
  "__asm__"
] @preproc

(property_attribute
  (identifier) @constant
  "="?)

[
  "BOOL"
  "IMP"
  "SEL"
  "Class"
  "id"
] @enum

(protocol_reference_list
  (identifier) @type)

(protocol_forward_declaration
  (identifier) @type)

(class_implementation
  "@implementation"
  .
  (identifier) @type
  superclass: _? @type
  category: _? @type)

(class_interface
  "@interface"
  .
  (identifier) @type
  superclass: _? @type
  category: _? @type)

(class_declaration
  (identifier) @type)

((call_expression
  function: (identifier) @preproc)
  (#eq? @preproc "testassert"))

[
  "__real"
  "__imag"
] @preproc

(type_qualifier
  [
    "_Complex"
    "_Nonnull"
    "_Nullable"
    "_Nullable_result"
    "_Null_unspecified"
    "__autoreleasing"
    "__block"
    "__bridge"
    "__bridge_retained"
    "__bridge_transfer"
    "__complex"
    "__kindof"
    "__nonnull"
    "__nullable"
    "__ptrauth_objc_class_ro"
    "__ptrauth_objc_isa_pointer"
    "__ptrauth_objc_super_pointer"
    "__strong"
    "__thread"
    "__unsafe_unretained"
    "__unused"
    "__weak"
  ]) @preproc

(availability_attribute_specifier
  [
    "CF_FORMAT_FUNCTION"
    "NS_AVAILABLE"
    "__IOS_AVAILABLE"
    "NS_AVAILABLE_IOS"
    "API_AVAILABLE"
    "API_UNAVAILABLE"
    "API_DEPRECATED"
    "NS_ENUM_AVAILABLE_IOS"
    "NS_DEPRECATED_IOS"
    "NS_ENUM_DEPRECATED_IOS"
    "NS_FORMAT_FUNCTION"
    "DEPRECATED_MSG_ATTRIBUTE"
    "__deprecated_msg"
    "__deprecated_enum_msg"
    "NS_SWIFT_NAME"
    "NS_SWIFT_UNAVAILABLE"
    "NS_EXTENSION_UNAVAILABLE_IOS"
    "NS_CLASS_AVAILABLE_IOS"
    "NS_CLASS_DEPRECATED_IOS"
    "__OSX_AVAILABLE_STARTING"
    "NS_ROOT_CLASS"
    "NS_UNAVAILABLE"
    "NS_REQUIRES_NIL_TERMINATION"
    "CF_RETURNS_RETAINED"
    "CF_RETURNS_NOT_RETAINED"
    "DEPRECATED_ATTRIBUTE"
    "UI_APPEARANCE_SELECTOR"
    "UNAVAILABLE_ATTRIBUTE"
  ]) @attribute

((message_expression
  method: (identifier) @constructor)
  (#eq? @constructor "init"))

(message_expression
  method: (identifier) @function)

(method_identifier
  (identifier)? @function
  ":" @function
  (identifier)? @function)

(method_declaration
  (identifier) @function)

(method_definition
  (identifier) @function)

[
  "objc_bridge_related"
  "@available"
  "__builtin_available"
  "va_arg"
  "asm"
] @function

((identifier) @variable.special
  (#any-of? @variable.special "self" "super"))

[
  "@try"
  "__try"
  "@catch"
  "__catch"
  "@finally"
  "__finally"
  "@throw"
] @keyword.exception

[
  "@synchronized"
  "oneway"
] @keyword.coroutine

[
  "__typeof__"
  "__typeof"
  "typeof"
  "in"
] @keyword.operator

(method_declaration
  [
    "+"
    "-"
  ] @keyword.function)

(method_definition
  [
    "+"
    "-"
  ] @keyword.function)

(class_declaration
  "@" @keyword.type
  "class" @keyword.type)

[
  "@protocol"
  "@interface"
  "@implementation"
  "@compatibility_alias"
  "@property"
  "@selector"
  "@defs"
  "availability"
  "@end"
] @keyword

[
  "@autoreleasepool"
  "@synthesize"
  "@dynamic"
  "volatile"
  (protocol_qualifier)
] @keyword.modifier

[
  "@optional"
  "@required"
  "__covariant"
  "__contravariant"
  (visibility_specification)
] @keyword.modifier

((preproc_include
  _ @keyword.import
  path: (_))
  (#any-of? @keyword.import "#include" "#import"))

(module_import
  "@import" @keyword.import
  path: (identifier) @type)

(preproc_undef
  name: (_) @constant) @preproc

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
