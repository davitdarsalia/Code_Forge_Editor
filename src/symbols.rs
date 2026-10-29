#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: &'static str,
    pub line: u32,
    pub column: u32,
}

const DECL_KEYWORDS: &[&str] = &[
    "fn", "func", "function", "def", "fun", "class", "struct", "enum", "trait", "interface",
    "protocol", "extension", "impl", "mod", "module", "object", "union", "namespace",
];

const VALUE_KEYWORDS: &[&str] = &["val", "let", "var", "const", "static", "type", "typealias"];

const MODIFIERS: &[&str] = &[
    "pub", "export", "default", "async", "private", "public", "protected", "internal", "open",
    "override", "final", "abstract", "sealed", "data", "inline", "unsafe", "extern", "fileprivate",
    "mutating", "nonmutating", "suspend", "operator", "infix", "lateinit", "declare", "readonly",
    "virtual", "companion", "inner", "enum", "indirect", "convenience", "required", "dynamic",
    "lazy", "weak", "tailrec", "external", "actual", "expect", "value", "annotation", "partial",
];

pub fn extract(text: &str) -> Vec<Symbol> {
    text.lines().enumerate().filter_map(|(line, s)| parse_line(s, line as u32)).collect()
}

fn parse_line(line: &str, line_no: u32) -> Option<Symbol> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') || trimmed.starts_with("--") {
        return None;
    }
    let mut rest = trimmed;
    let mut stripped_static = false;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix('@') {
            rest = skip_word(after);
            continue;
        }
        let word = leading_word(rest);
        if word.is_empty() {
            return None;
        }
        let after = &rest[word.len()..];
        if word == "pub" && after.starts_with('(') {
            rest = &after[after.find(')')? + 1..];
            continue;
        }
        if word == "extern" && after.trim_start().starts_with('"') {
            let quoted = after.trim_start();
            rest = &quoted[1 + quoted[1..].find('"')? + 1..];
            continue;
        }
        let next_is_keyword = || {
            let next = leading_word(after.trim_start());
            DECL_KEYWORDS.contains(&next) || VALUE_KEYWORDS.contains(&next) || MODIFIERS.contains(&next)
        };
        if (MODIFIERS.contains(&word) && next_is_keyword())
            || (matches!(word, "class" | "static" | "const") && next_is_keyword())
        {
            stripped_static |= word == "static";
            rest = after;
            continue;
        }
        if MODIFIERS.contains(&word) && word != "enum" && word != "value" {
            rest = after;
            continue;
        }
        let decl = DECL_KEYWORDS.iter().find(|k| **k == word);
        let value = VALUE_KEYWORDS.iter().find(|k| **k == word);
        let kind: &'static str = match (decl, value) {
            (Some(k), _) => k,
            (None, Some(k)) if indent == 0 && !stripped_static => k,
            _ => return None,
        };
        let separator_ok = after.starts_with(char::is_whitespace)
            || (kind == "impl" && after.starts_with('<'))
            || (kind == "function" && after.starts_with('*'));
        if !separator_ok {
            return None;
        }
        let start = trimmed.len() - after.len() + indent;
        return name_after(line, start, kind, line_no);
    }
}

fn name_after(line: &str, from: usize, kind: &'static str, line_no: u32) -> Option<Symbol> {
    let mut pos = from + (line[from..].len() - line[from..].trim_start().len());
    let rest = &line[pos..];
    if kind == "impl" {
        let header = rest.split(['{', ';']).next()?.split(" where ").next()?.trim();
        let header = strip_generics_prefix(header);
        if header.is_empty() {
            return None;
        }
        return Some(Symbol { name: format!("impl {header}"), kind, line: line_no, column: char_col(line, pos) });
    }
    if kind == "func" && rest.starts_with('(') {
        let close = rest.find(')')?;
        let after = &rest[close + 1..];
        pos += close + 1 + (after.len() - after.trim_start().len());
    }
    let rest = &line[pos..];
    if let Some(after) = rest.strip_prefix('*') {
        pos += 1 + (after.len() - after.trim_start().len());
    }
    let rest = &line[pos..];
    let name: &str = &rest[..rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$' || c == '.'))
        .unwrap_or(rest.len())];
    let name = name.trim_end_matches('.');
    if !name.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '$') {
        return None;
    }
    Some(Symbol { name: name.to_string(), kind, line: line_no, column: char_col(line, pos) })
}

fn strip_generics_prefix(s: &str) -> &str {
    if !s.starts_with('<') {
        return s;
    }
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return s[i + 1..].trim();
                }
            }
            _ => {}
        }
    }
    s
}

fn leading_word(s: &str) -> &str {
    &s[..s.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len())]
}

fn skip_word(s: &str) -> &str {
    let word = s.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).unwrap_or(s.len());
    let mut rest = &s[word..];
    if rest.starts_with('(') {
        rest = rest.find(')').map_or("", |i| &rest[i + 1..]);
    }
    rest
}

fn char_col(line: &str, byte: usize) -> u32 {
    line[..byte].chars().count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<(String, &'static str)> {
        extract(text).into_iter().map(|s| (s.name, s.kind)).collect()
    }

    fn n(name: &str, kind: &'static str) -> (String, &'static str) {
        (name.to_string(), kind)
    }

    #[test]
    fn rust() {
        let src = "use std::io;\n\npub(crate) struct Foo<T> {\n    x: u32,\n}\n\nimpl<T: Clone> Display for Foo<T> where T: X {\n    pub async fn render(&self) {}\n    fn helper() { let x = 1; }\n}\npub enum Mode { A }\nconst MAX: usize = 3;\npub static NAME: &str = \"\";\nextern \"C\" fn ffi() {}\n// fn commented() {}\n";
        assert_eq!(
            names(src),
            vec![
                n("Foo", "struct"),
                n("impl Display for Foo<T>", "impl"),
                n("render", "fn"),
                n("helper", "fn"),
                n("Mode", "enum"),
                n("MAX", "const"),
                n("NAME", "static"),
                n("ffi", "fn"),
            ]
        );
    }

    #[test]
    fn swift_and_kotlin() {
        let src = "@MainActor\nfinal class Session: Base {\n    @objc private func connect(to tv: TV) {}\n    class func make() -> Self {}\n    let local = 1\n}\nprotocol Remote {}\nenum class Kind { A }\ndata class Point(val x: Int)\nfun String.shout() = uppercase()\nval top = 3\n";
        assert_eq!(
            names(src),
            vec![
                n("Session", "class"),
                n("connect", "func"),
                n("make", "func"),
                n("Remote", "protocol"),
                n("Kind", "class"),
                n("Point", "class"),
                n("String.shout", "fun"),
                n("top", "val"),
            ]
        );
    }

    #[test]
    fn go_python_js() {
        let src = "func (s *Server) Serve(addr string) error {\n}\ntype Config struct {}\ndef main():\n    def inner(): pass\nclass Foo(Base):\n# def nope(): pass\nexport default function* gen() {}\nexport const handler = () => {}\n  const notTop = 1\ninterface Props {}\n";
        assert_eq!(
            names(src),
            vec![
                n("Serve", "func"),
                n("Config", "type"),
                n("main", "def"),
                n("inner", "def"),
                n("Foo", "class"),
                n("gen", "function"),
                n("handler", "const"),
                n("Props", "interface"),
            ]
        );
    }

    #[test]
    fn positions_are_char_based() {
        let syms = extract("// é\n    fn naïve() {}\n");
        assert_eq!(syms, vec![Symbol { name: "naïve".into(), kind: "fn", line: 1, column: 7 }]);
    }

    #[test]
    fn ignores_non_declarations() {
        assert!(extract("let x = fnord;\n    return class_name;\nclass=\"btn\"\nfn(\n").iter().all(|s| s.name == "x"));
    }
}
