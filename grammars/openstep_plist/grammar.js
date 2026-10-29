// Old-style (OpenStep / NeXTSTEP) property lists: Xcode .pbxproj files and
// Apple .strings files. A .strings file is a dictionary body without braces,
// so the top level accepts bare `key = value;` pairs as well as one value.
module.exports = grammar({
  name: 'openstep_plist',

  extras: $ => [/\s/, $.comment],

  rules: {
    source_file: $ => repeat(choice($.pair, $._value)),

    _value: $ => choice($.dictionary, $.array, $.string, $.bare, $.data),

    dictionary: $ => seq('{', repeat($.pair), '}'),

    pair: $ => seq(
      field('key', choice($.string, $.bare)),
      '=',
      field('value', $._value),
      ';',
    ),

    array: $ => seq(
      '(',
      optional(seq($._value, repeat(seq(',', $._value)), optional(','))),
      ')',
    ),

    string: $ => seq('"', repeat(choice($.string_content, $.escape_sequence)), '"'),
    string_content: _ => token.immediate(prec(1, /[^"\\]+/)),
    escape_sequence: _ => token.immediate(/\\(.|\n|[0-7]{1,3}|U[0-9a-fA-F]{4})/),

    // Unquoted strings: identifiers, numbers, hex object ids, paths.
    bare: _ => /[A-Za-z0-9_$+\/:.\-]+/,

    data: _ => /<[0-9a-fA-F\s]*>/,

    comment: _ => token(choice(
      seq('//', /[^\n]*/),
      seq('/*', /[^*]*\*+([^/*][^*]*\*+)*/, '/'),
    )),
  },
});
