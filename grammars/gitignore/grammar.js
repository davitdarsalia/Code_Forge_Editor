// .gitignore and friends: one pattern or comment per line.
module.exports = grammar({
  name: 'gitignore',

  extras: _ => [],

  rules: {
    source_file: $ => repeat(choice($.comment, $.pattern, /\r?\n/)),

    comment: _ => token(prec(1, /#[^\n]*/)),

    pattern: $ => prec.right(seq(
      optional($.negation),
      repeat1(choice($.text, $.separator, $.wildcard, $.bracket, $.escape)),
    )),
    negation: _ => '!',
    text: _ => /[^\n\/*?\[\\]+/,
    separator: _ => '/',
    wildcard: _ => choice('*', '**', '?'),
    bracket: _ => /\[[^\]\n]*\]/,
    escape: _ => /\\./,
  },
});
