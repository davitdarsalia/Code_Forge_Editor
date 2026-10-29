// ProGuard / R8 rule files (proguard-rules.pro, consumer-rules.pro).
// Purely lexical: a flat stream of options, keywords, class specs and
// punctuation, which is all highlighting needs and never fails to parse.
module.exports = grammar({
  name: 'proguard',

  extras: $ => [/\s/, $.comment],
  word: $ => $.identifier,

  rules: {
    source_file: $ => repeat($._item),

    _item: $ => choice(
      $.option, $.keyword, $.member_wildcard, $.annotation, $.identifier,
      $.number, $.string, $.punctuation,
    ),

    option: _ => /-[A-Za-z][A-Za-z0-9]*/,

    keyword: _ => choice(
      'class', 'interface', 'enum', 'extends', 'implements', 'public', 'private',
      'protected', 'static', 'final', 'abstract', 'volatile', 'transient',
      'synchronized', 'native', 'strictfp', 'bridge', 'varargs', 'synthetic',
      'includedescriptorclasses', 'includecode', 'allowshrinking',
      'allowoptimization', 'allowobfuscation', 'return', 'void', 'boolean', 'byte',
      'char', 'short', 'int', 'long', 'float', 'double',
    ),

    member_wildcard: _ => choice('<init>', '<clinit>', '<fields>', '<methods>'),

    annotation: _ => /@[A-Za-z_$*][A-Za-z0-9_$.*]*/,

    // Class names, wildcards (`*`, `**`, `***`), file paths.
    identifier: _ => /[A-Za-z_$*\/][A-Za-z0-9_$.*\/\-]*/,

    number: _ => /[0-9]+/,
    string: _ => choice(/"[^"\n]*"/, /'[^'\n]*'/),
    punctuation: _ => choice('{', '}', ';', '(', ')', ',', '[', ']', '!', '...', ':', '%', '<', '>', '='),

    comment: _ => token(seq('#', /[^\n]*/)),
  },
});
