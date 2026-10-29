; ProGuard / R8 rules.
(comment) @comment
(option) @keyword
(keyword) @keyword
(member_wildcard) @variable.special
(annotation) @attribute
((identifier) @operator (#match? @operator "^\\*+$"))
(identifier) @type
(number) @number
(string) @string
(punctuation) @punctuation
