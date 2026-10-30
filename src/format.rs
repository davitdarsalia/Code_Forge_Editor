use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const FORMAT_TIMEOUT: Duration = Duration::from_secs(5);

const EXTRA_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    SwiftFormat,
    NickSwiftFormat,
    Ktfmt,
    Ktlint,
    Rustfmt,
    Prettier,
    Xmllint,
    Gofmt,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::SwiftFormat => "swift-format",
            Tool::NickSwiftFormat => "swiftformat",
            Tool::Ktfmt => "ktfmt",
            Tool::Ktlint => "ktlint",
            Tool::Rustfmt => "rustfmt",
            Tool::Prettier => "prettier",
            Tool::Xmllint => "xmllint",
            Tool::Gofmt => "gofmt",
        }
    }
}

struct Entry {
    language: &'static str,
    candidates: &'static [Tool],
    install: Option<&'static str>,
    reindent: bool,
}

const TABLE: &[Entry] = &[
    Entry { language: "swift", candidates: &[Tool::SwiftFormat, Tool::NickSwiftFormat], install: Some("install Xcode 16+ or `brew install swiftformat`"), reindent: true },
    Entry { language: "kotlin", candidates: &[Tool::Ktfmt, Tool::Ktlint], install: Some("`brew install ktlint`"), reindent: true },
    Entry { language: "groovy", candidates: &[], install: None, reindent: true },
    Entry { language: "rust", candidates: &[Tool::Rustfmt], install: Some("`rustup component add rustfmt`"), reindent: true },
    Entry { language: "javascript", candidates: &[Tool::Prettier], install: Some("`brew install prettier`"), reindent: true },
    Entry { language: "typescript", candidates: &[Tool::Prettier], install: Some("`brew install prettier`"), reindent: true },
    Entry { language: "tsx", candidates: &[Tool::Prettier], install: Some("`brew install prettier`"), reindent: true },
    Entry { language: "json", candidates: &[Tool::Prettier], install: Some("`brew install prettier`"), reindent: true },
    Entry { language: "css", candidates: &[Tool::Prettier], install: Some("`brew install prettier`"), reindent: true },
    Entry { language: "markdown", candidates: &[Tool::Prettier], install: Some("`brew install prettier`"), reindent: false },
    Entry { language: "html", candidates: &[Tool::Prettier], install: Some("`brew install prettier`"), reindent: false },
    Entry { language: "yaml", candidates: &[Tool::Prettier], install: Some("`brew install prettier`"), reindent: false },
    Entry { language: "xml", candidates: &[Tool::Xmllint], install: Some("`brew install libxml2`"), reindent: false },
    Entry { language: "go", candidates: &[Tool::Gofmt], install: Some("`brew install go`"), reindent: true },
    Entry { language: "java", candidates: &[], install: None, reindent: true },
    Entry { language: "c", candidates: &[], install: None, reindent: true },
    Entry { language: "cpp", candidates: &[], install: None, reindent: true },
    Entry { language: "objc", candidates: &[], install: None, reindent: true },
    Entry { language: "csharp", candidates: &[], install: None, reindent: true },
    Entry { language: "dart", candidates: &[], install: None, reindent: true },
    Entry { language: "scala", candidates: &[], install: None, reindent: true },
    Entry { language: "php", candidates: &[], install: None, reindent: true },
    Entry { language: "proto", candidates: &[], install: None, reindent: true },
    Entry { language: "zig", candidates: &[], install: None, reindent: true },
];

#[derive(Debug, PartialEq)]
pub enum Plan {
    External { tool: Tool, program: PathBuf },
    Reindent { hint: Option<String> },
    Unsupported { hint: Option<String> },
}

pub fn plan(
    language: &str,
    resolve: &dyn Fn(Tool) -> Option<PathBuf>,
    has_config: &dyn Fn(&str) -> bool,
) -> Plan {
    let Some(entry) = TABLE.iter().find(|e| e.language == language) else {
        return Plan::Unsupported { hint: None };
    };
    let mut order: Vec<Tool> = entry.candidates.to_vec();
    if language == "swift" && has_config(".swiftformat") && !has_config(".swift-format") {
        order.reverse();
    }
    if language == "kotlin" && has_config(".editorconfig") {
        order.reverse();
    }
    for tool in order {
        if let Some(program) = resolve(tool) {
            return Plan::External { tool, program };
        }
    }
    let hint = entry.install.map(|cmd| {
        let names: Vec<&str> = entry.candidates.iter().map(|t| t.name()).collect();
        format!("{} not found; {cmd}", names.join(" / "))
    });
    if entry.reindent { Plan::Reindent { hint } } else { Plan::Unsupported { hint } }
}

pub enum Outcome {
    Changed { text: String, notice: Option<String> },
    Unchanged { notice: Option<String> },
    Failed(String),
}

pub fn format(path: &Path, language: &str, text: &str) -> Outcome {
    let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    let plan = plan(language, &|tool| resolve(tool), &|name| find_up(&dir, name).is_some());
    let (result, notice) = match plan {
        Plan::External { tool, program } => (run_tool(tool, &program, path, text), None),
        Plan::Reindent { hint } => {
            let unit = indent_unit(path, text);
            match reindent(text, &unit, language) {
                Some(out) => (Ok(out), hint.map(|h| format!("Re-indented only ({h})"))),
                None => (Err("Could not re-indent: unbalanced brackets or unterminated string".into()), None),
            }
        }
        Plan::Unsupported { hint } => {
            let message = match hint {
                Some(h) => format!("No formatter for {language}: {h}"),
                None => format!("No formatter for {language} files"),
            };
            return Outcome::Failed(message);
        }
    };
    match result {
        Ok(out) if out == text => Outcome::Unchanged { notice },
        Ok(out) => Outcome::Changed { text: out, notice },
        Err(err) => Outcome::Failed(err),
    }
}

pub(crate) fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(EXTRA_BIN_DIRS.iter().map(PathBuf::from));
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join(".local/bin"));
    }
    dirs
}

pub(crate) fn which(name: &str) -> Option<PathBuf> {
    search_dirs().into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

fn xcode_swift_format() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let out = Command::new("/usr/bin/xcrun").args(["--find", "swift-format"]).output().ok()?;
            let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            (out.status.success() && path.is_file()).then_some(path)
        })
        .clone()
}

fn resolve(tool: Tool) -> Option<PathBuf> {
    match tool {
        Tool::SwiftFormat => which("swift-format").or_else(xcode_swift_format),
        other => which(other.name()),
    }
}

fn find_up(dir: &Path, name: &str) -> Option<PathBuf> {
    dir.ancestors().map(|d| d.join(name)).find(|p| p.exists())
}

fn run_tool(tool: Tool, program: &Path, path: &Path, text: &str) -> Result<String, String> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let file = path.to_string_lossy().into_owned();
    let mut args: Vec<String> = Vec::new();
    match tool {
        Tool::SwiftFormat => {
            args.extend(["format".into(), "--assume-filename".into(), file]);
            if let Some(config) = find_up(dir, ".swift-format") {
                args.extend(["--configuration".into(), config.to_string_lossy().into_owned()]);
            }
            args.push("-".into());
        }
        Tool::NickSwiftFormat => args.extend(["stdin".into(), "--stdinpath".into(), file, "--quiet".into()]),
        Tool::Ktfmt => {
            args.extend(["--kotlinlang-style".into(), "-".into()]);
        }
        Tool::Ktlint => args.extend(["--format".into(), "--stdin".into(), "--stdin-path".into(), file, "--log-level=none".into()]),
        Tool::Rustfmt => {
            args.extend(["--emit".into(), "stdout".into(), "--quiet".into()]);
            args.extend(["--edition".into(), rust_edition(dir)]);
        }
        Tool::Prettier => args.extend(["--stdin-filepath".into(), file]),
        Tool::Xmllint => args.extend(["--format".into(), "-".into()]),
        Tool::Gofmt => {}
    }
    let output = run_with_timeout(program, &args, dir, text, FORMAT_TIMEOUT)?;
    if tool == Tool::Xmllint && !text.trim_start().starts_with("<?xml") {
        if let Some(rest) = output.strip_prefix("<?xml version=\"1.0\"?>\n") {
            return Ok(rest.to_string());
        }
    }
    Ok(output)
}

fn rust_edition(dir: &Path) -> String {
    find_up(dir, "Cargo.toml")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|toml| {
            toml.lines().find_map(|line| {
                let rest = line.trim().strip_prefix("edition")?.trim_start().strip_prefix('=')?;
                Some(rest.trim().trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "2021".into())
}

fn run_with_timeout(
    program: &Path,
    args: &[String],
    cwd: &Path,
    input: &str,
    timeout: Duration,
) -> Result<String, String> {
    let name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let path_env = std::env::join_paths(search_dirs()).unwrap_or_default();
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("PATH", path_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Cannot run {name}: {err}"))?;

    let input_blank = input.trim().is_empty();
    let mut stdin = child.stdin.take();
    let input = input.to_owned();
    let writer = std::thread::spawn(move || {
        if let Some(stdin) = stdin.as_mut() {
            if let Err(err) = stdin.write_all(input.as_bytes()) {
                eprintln!("forge: formatter stdin: {err}");
            }
        }
    });
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(out) = stdout.as_mut() {
            let _ = out.read_to_end(&mut buf);
        }
        buf
    });
    let mut stderr = child.stderr.take();
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(err) = stderr.as_mut() {
            let _ = err.read_to_end(&mut buf);
        }
        buf
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                if let Err(err) = child.kill() {
                    eprintln!("forge: cannot kill {name}: {err}");
                }
                let _ = child.wait();
                return Err(format!("{name} timed out after {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(err) => return Err(format!("{name} failed: {err}")),
        }
    };
    let _ = writer.join();
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("no details");
        return Err(format!("{name} failed: {first}"));
    }
    let out = String::from_utf8(stdout).map_err(|_| format!("{name} returned invalid UTF-8"))?;
    if out.trim().is_empty() && !input_blank {
        return Err(format!("{name} returned nothing"));
    }
    Ok(out)
}

fn indent_unit(path: &Path, text: &str) -> String {
    if let Some(unit) = editorconfig_indent(path) {
        return unit;
    }
    let (mut tabs, mut spaces) = (0, 0);
    for line in text.lines() {
        if line.starts_with('\t') {
            tabs += 1;
        } else if line.starts_with(' ') {
            spaces += 1;
        }
    }
    if tabs > spaces { "\t".into() } else { "    ".into() }
}

fn editorconfig_indent(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (mut style, mut size): (Option<String>, Option<usize>) = (None, None);
    for dir in path.parent()?.ancestors() {
        let Ok(text) = std::fs::read_to_string(dir.join(".editorconfig")) else { continue };
        let (s, z, root) = parse_editorconfig(&text, &name);
        style = style.or(s);
        size = size.or(z);
        if root {
            break;
        }
    }
    match style.as_deref() {
        Some("tab") => Some("\t".into()),
        Some("space") | None if size.is_some() => Some(" ".repeat(size.unwrap_or(4).clamp(1, 8))),
        Some("space") => Some("    ".into()),
        _ => None,
    }
}

fn parse_editorconfig(text: &str, file_name: &str) -> (Option<String>, Option<usize>, bool) {
    let (mut style, mut size, mut root) = (None, None, false);
    let mut matches = false;
    let mut in_preamble = true;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_preamble = false;
            matches = glob_matches(section, file_name);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim().to_lowercase(), value.trim().to_lowercase());
        if in_preamble {
            root |= key == "root" && value == "true";
        } else if matches {
            match key.as_str() {
                "indent_style" => style = Some(value),
                "indent_size" => size = value.parse().ok().or(size),
                _ => {}
            }
        }
    }
    (style, size, root)
}

fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern = pattern.rsplit('/').next().unwrap_or(pattern);
    if pattern == "*" {
        return true;
    }
    if let Some(rest) = pattern.strip_prefix("*.") {
        if let Some(list) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
            return list.split(',').any(|ext| name.ends_with(&format!(".{}", ext.trim())));
        }
        return name.ends_with(&format!(".{rest}"));
    }
    if let Some(list) = pattern.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
        return list.split(',').any(|n| n.trim() == name);
    }
    pattern == name
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Code,
    BlockComment(u32),
    TripleString(char),
    Backtick,
}

pub fn reindent(text: &str, unit: &str, language: &str) -> Option<String> {
    let crlf = text.contains("\r\n");
    let normalized;
    let text = if crlf {
        normalized = text.replace("\r\n", "\n");
        normalized.as_str()
    } else {
        text
    };
    let quote_strings = matches!(language, "groovy" | "javascript" | "typescript" | "tsx" | "json" | "css" | "php" | "dart" | "python");
    let triple = matches!(language, "groovy" | "kotlin" | "swift" | "scala" | "dart" | "java");
    let backtick = matches!(language, "javascript" | "typescript" | "tsx");

    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut state = State::Code;
    let mut out = String::with_capacity(text.len() + text.len() / 8);

    for (ix, line) in text.split('\n').enumerate() {
        if ix > 0 {
            out.push('\n');
        }
        let level = |stack: &[(char, usize)], drop: usize| -> usize {
            stack[..stack.len().saturating_sub(drop)].iter().map(|(_, c)| c).sum()
        };
        let trimmed = line.trim();
        match state {
            State::TripleString(_) | State::Backtick => out.push_str(line),
            State::BlockComment(_) => {
                if trimmed.starts_with('*') {
                    out.push_str(&unit.repeat(level(&stack, 0)));
                    out.push(' ');
                    out.push_str(trimmed);
                } else {
                    out.push_str(line.trim_end());
                }
            }
            State::Code if trimmed.is_empty() => {}
            State::Code => {
                let closers = trimmed.chars().take_while(|c| matches!(c, '}' | ')' | ']')).count();
                let mut depth = level(&stack, closers);
                if is_continuation(trimmed) {
                    depth += 1;
                }
                out.push_str(&unit.repeat(depth));
                out.push_str(trimmed);
            }
        }

        let mut min_len = stack.len();
        state = scan_line(line, state, &mut stack, &mut min_len, quote_strings, triple, backtick)?;
        for (i, entry) in stack.iter_mut().enumerate().skip(min_len) {
            entry.1 = usize::from(i == min_len);
        }
    }
    if !stack.is_empty() || matches!(state, State::BlockComment(_) | State::TripleString(_) | State::Backtick) {
        return None;
    }
    Some(if crlf { out.replace('\n', "\r\n") } else { out })
}

fn is_continuation(trimmed: &str) -> bool {
    (trimmed.starts_with('.') && !trimmed.starts_with("..")) || ["?.", "?:", "&&", "||"].iter().any(|p| trimmed.starts_with(p))
}

fn scan_line(
    line: &str,
    mut state: State,
    stack: &mut Vec<(char, usize)>,
    min_len: &mut usize,
    quote_strings: bool,
    triple: bool,
    backtick: bool,
) -> Option<State> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match state {
            State::BlockComment(depth) => {
                if c == '*' && next == Some('/') {
                    state = if depth <= 1 { State::Code } else { State::BlockComment(depth - 1) };
                    i += 2;
                    continue;
                }
                if c == '/' && next == Some('*') {
                    state = State::BlockComment(depth + 1);
                    i += 2;
                    continue;
                }
            }
            State::TripleString(q) => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == q && next == Some(q) && chars.get(i + 2) == Some(&q) {
                    state = State::Code;
                    i += 3;
                    continue;
                }
            }
            State::Backtick => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == '`' {
                    state = State::Code;
                }
            }
            State::Code => match c {
                '/' if next == Some('/') => break,
                '#' if i == 0 || chars[..i].iter().all(|c| c.is_whitespace()) => {
                    break;
                }
                '/' if next == Some('*') => {
                    state = State::BlockComment(1);
                    i += 2;
                    continue;
                }
                '"' | '\'' if triple && next == Some(c) && chars.get(i + 2) == Some(&c) => {
                    state = State::TripleString(c);
                    i += 3;
                    continue;
                }
                '`' if backtick => state = State::Backtick,
                '"' => i = skip_string(&chars, i, '"')?,
                '\'' => {
                    if quote_strings {
                        i = skip_string(&chars, i, '\'')?;
                    } else if let Some(end) = char_literal_end(&chars, i) {
                        i = end;
                    }
                }
                '{' | '(' | '[' => stack.push((c, 1)),
                '}' | ')' | ']' => {
                    let open = match c {
                        '}' => '{',
                        ')' => '(',
                        _ => '[',
                    };
                    let (top, _) = stack.pop()?;
                    if top != open {
                        return None;
                    }
                    *min_len = (*min_len).min(stack.len());
                }
                _ => {}
            },
        }
        i += 1;
    }
    Some(state)
}

fn skip_string(chars: &[char], start: usize, quote: char) -> Option<usize> {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            c if c == quote => return Some(i),
            _ => i += 1,
        }
    }
    Some(chars.len())
}

fn char_literal_end(chars: &[char], start: usize) -> Option<usize> {
    match chars.get(start + 1)? {
        '\\' => (start + 3..chars.len().min(start + 12)).find(|&j| chars[j] == '\''),
        _ => (chars.get(start + 2) == Some(&'\'')).then_some(start + 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_system_tools_and_search_dirs_cover_brew_and_cargo() {
        assert!(which("sh").is_some_and(|p| p.ends_with("sh") && p.is_file()));
        assert!(which("forge-definitely-not-a-tool").is_none());
        let dirs = search_dirs();
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(dirs.contains(&PathBuf::from("/usr/bin")));
        if let Some(home) = dirs::home_dir() {
            assert!(dirs.contains(&home.join(".cargo/bin")));
        }
    }

    #[test]
    fn reindents_groovy_gradle() {
        let input = "android {\ncompileSdk 34\n  defaultConfig {\n        minSdk 24\n  }\n}\n";
        let want = "android {\n    compileSdk 34\n    defaultConfig {\n        minSdk 24\n    }\n}\n";
        assert_eq!(reindent(input, "    ", "groovy").unwrap(), want);
    }

    #[test]
    fn leaves_strings_and_comments_alone() {
        let input = "deps {\nimplementation 'a:b:1.0' // {\n/* not { a block\n   still */\nx = \"}\"\n}\n";
        let want = "deps {\n    implementation 'a:b:1.0' // {\n    /* not { a block\n   still */\n    x = \"}\"\n}\n";
        assert_eq!(reindent(input, "    ", "groovy").unwrap(), want);
    }

    #[test]
    fn kotlin_multiline_string_untouched() {
        let input = "fun f() {\nval s = \"\"\"\n  { raw\n\"\"\"\nreturn\n}";
        let want = "fun f() {\n    val s = \"\"\"\n  { raw\n\"\"\"\n    return\n}";
        assert_eq!(reindent(input, "    ", "kotlin").unwrap(), want);
    }

    #[test]
    fn kotlin_chains_and_lambdas() {
        let input = "fun main() {\nlistOf(1).map {\nit * 2\n}\n.forEach({ x ->\nprintln(x)\n})\nif (a) {\n} else {\nb()\n}\n}";
        let want = "fun main() {\n    listOf(1).map {\n        it * 2\n    }\n        .forEach({ x ->\n        println(x)\n    })\n    if (a) {\n    } else {\n        b()\n    }\n}";
        assert_eq!(reindent(input, "    ", "kotlin").unwrap(), want);
    }

    #[test]
    fn javadoc_star_lines_follow_indent() {
        let input = "class A {\n/**\n* Doc\n*/\nfun x() {}\n}";
        let want = "class A {\n    /**\n     * Doc\n     */\n    fun x() {}\n}";
        assert_eq!(reindent(input, "    ", "kotlin").unwrap(), want);
    }

    #[test]
    fn refuses_unbalanced_or_mismatched() {
        assert!(reindent("a {\n", "    ", "groovy").is_none());
        assert!(reindent("a }\n", "    ", "groovy").is_none());
        assert!(reindent("a ( }\n", "    ", "groovy").is_none());
        assert!(reindent("val s = \"\"\"\nopen", "    ", "kotlin").is_none());
    }

    #[test]
    fn char_literals_and_lifetimes() {
        let input = "fn f<'a>(x: &'a str) {\nlet c = '{';\n}";
        let want = "fn f<'a>(x: &'a str) {\n    let c = '{';\n}";
        assert_eq!(reindent(input, "    ", "rust").unwrap(), want);
    }

    #[test]
    fn keeps_crlf_and_tabs() {
        let input = "a {\r\nb\r\n}\r\n";
        assert_eq!(reindent(input, "\t", "groovy").unwrap(), "a {\r\n\tb\r\n}\r\n");
    }

    #[test]
    fn strips_whitespace_only_lines() {
        assert_eq!(reindent("a {\n   \n}", "    ", "groovy").unwrap(), "a {\n\n}");
    }

    fn only(tools: &'static [Tool]) -> impl Fn(Tool) -> Option<PathBuf> {
        move |t| tools.contains(&t).then(|| PathBuf::from(format!("/bin/{}", t.name())))
    }

    #[test]
    fn swift_prefers_apple_swift_format() {
        let plan = plan("swift", &only(&[Tool::SwiftFormat, Tool::NickSwiftFormat]), &|_| false);
        assert!(matches!(plan, Plan::External { tool: Tool::SwiftFormat, .. }));
    }

    #[test]
    fn swift_follows_swiftformat_config() {
        let resolve = only(&[Tool::SwiftFormat, Tool::NickSwiftFormat]);
        let plan = plan("swift", &resolve, &|name| name == ".swiftformat");
        assert!(matches!(plan, Plan::External { tool: Tool::NickSwiftFormat, .. }));
        let plan = super::plan("swift", &only(&[Tool::SwiftFormat]), &|name| name == ".swiftformat");
        assert!(matches!(plan, Plan::External { tool: Tool::SwiftFormat, .. }));
    }

    #[test]
    fn kotlin_selection_and_fallback() {
        let both = only(&[Tool::Ktfmt, Tool::Ktlint]);
        assert!(matches!(plan("kotlin", &both, &|_| false), Plan::External { tool: Tool::Ktfmt, .. }));
        assert!(matches!(plan("kotlin", &both, &|n| n == ".editorconfig"), Plan::External { tool: Tool::Ktlint, .. }));
        match plan("kotlin", &only(&[]), &|_| false) {
            Plan::Reindent { hint: Some(hint) } => assert!(hint.contains("brew install ktlint")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn groovy_and_unknown() {
        assert_eq!(plan("groovy", &only(&[]), &|_| false), Plan::Reindent { hint: None });
        assert_eq!(plan("python", &only(&[]), &|_| false), Plan::Unsupported { hint: None });
        assert!(matches!(plan("yaml", &only(&[]), &|_| false), Plan::Unsupported { hint: Some(_) }));
    }

    #[test]
    fn editorconfig_sections() {
        let text = "root = true\n[*]\nindent_size = 2\n[*.{kt,kts}]\nindent_size = 4\nindent_style = space\n";
        assert_eq!(parse_editorconfig(text, "a.kt"), (Some("space".into()), Some(4), true));
        assert_eq!(parse_editorconfig(text, "a.gradle"), (None, Some(2), true));
        assert!(glob_matches("**/*.gradle", "build.gradle"));
        assert!(!glob_matches("*.kt", "a.kts"));
    }
}
