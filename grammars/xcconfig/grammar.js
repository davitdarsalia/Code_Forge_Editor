// Xcode build configuration files (.xcconfig). Line based: `//` starts a
// comment anywhere (hence the `https:/$()/host` idiom), values run to the end
// of the line, settings may carry `[sdk=*][config=Release]` conditions.
module.exports = grammar({
  name: 'xcconfig',

  extras: $ => [/[ \t\r]/, $.comment],

  rules: {
    // One statement per line; the last line may lack its newline.
    source_file: $ => seq(
      repeat(seq(optional($._statement), $._newline)),
      optional($._statement),
    ),
    _statement: $ => choice($.include, $.assignment),
    _newline: _ => /\r?\n/,

    include: $ => seq(field('directive', $.directive), field('path', $.string)),
    directive: _ => /#include\??/,

    assignment: $ => seq(
      field('name', $.setting),
      repeat($.condition),
      '=',
      optional(field('value', $.value)),
    ),

    setting: _ => /[A-Za-z_][A-Za-z0-9_]*/,

    condition: $ => seq(
      '[',
      field('name', $.condition_name),
      '=',
      field('value', $.condition_value),
      ']',
    ),
    condition_name: _ => /[A-Za-z_][A-Za-z0-9_]*/,
    condition_value: _ => /[^\]\n]+/,

    value: $ => repeat1(choice($.variable, $.string, $.text)),
    variable: _ => token(prec(1, /\$(\([^)\n]*\)|\{[^}\n]*\}|[A-Za-z_][A-Za-z0-9_]*)/)),
    string: _ => /"[^"\n]*"/,
    // Value text; a lone `/` is its own token so `//` always lexes as a comment.
    text: _ => token(choice(/[^\s\/$"][^\n\/$"]*/, '/', '$')),

    comment: _ => token(seq('//', /[^\n]*/)),
  },
});
