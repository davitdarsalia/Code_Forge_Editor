; Old-style plists: .pbxproj, .strings. Specific patterns first: on an equal
; range the highlighter keeps the first capture.
(comment) @comment
(pair key: (string) @property)
(pair key: (bare) @property)
((bare) @boolean (#match? @boolean "^(YES|NO|true|false)$"))
((bare) @number (#match? @number "^-?[0-9]+(\\.[0-9]+)?$"))
((bare) @type (#match? @type "^(PBX|XC)[A-Za-z]+$"))
((bare) @constant (#match? @constant "^[0-9A-F]{24}$"))
(string) @string
(escape_sequence) @string.escape
(bare) @string.special
(data) @constant
["=" ";" ","] @punctuation.delimiter
["{" "}" "(" ")"] @punctuation.bracket
