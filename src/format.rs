use std::borrow::Cow;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::tools::{search_dirs, which, xcrun_find};

const FORMAT_TIMEOUT: Duration = Duration::from_secs(5);

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
    if let Some(kind) = builtin_for(path) {
        return format_with_builtin(kind, path, text, &dir);
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Yaml,
    Json,
    Toml,
    Css,
    Scss,
    Less,
    Html,
    Markdown,
}

impl Builtin {
    pub fn label(self) -> &'static str {
        match self {
            Builtin::Yaml => "YAML",
            Builtin::Json => "JSON",
            Builtin::Toml => "TOML",
            Builtin::Css => "CSS",
            Builtin::Scss => "SCSS",
            Builtin::Less => "Less",
            Builtin::Html => "HTML",
            Builtin::Markdown => "Markdown",
        }
    }
}

pub fn builtin_for(path: &Path) -> Option<Builtin> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "yml" | "yaml" => Some(Builtin::Yaml),
        "json" | "jsonc" => Some(Builtin::Json),
        "toml" => Some(Builtin::Toml),
        "css" => Some(Builtin::Css),
        "scss" => Some(Builtin::Scss),
        "less" => Some(Builtin::Less),
        "html" | "htm" => Some(Builtin::Html),
        "md" | "markdown" => Some(Builtin::Markdown),
        _ if name == "cargo.lock" => Some(Builtin::Toml),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub use_tabs: bool,
    pub indent: usize,
    pub max_line: Option<usize>,
}

impl Default for Style {
    fn default() -> Self {
        Style { use_tabs: false, indent: 2, max_line: None }
    }
}

fn style_for(path: &Path) -> Style {
    let config = editorconfig_for(path);
    let mut style = Style::default();
    style.use_tabs = config.style.as_deref() == Some("tab");
    if let Some(size) = config.size {
        style.indent = size.clamp(1, 8);
    }
    style.max_line = config.max_line.filter(|n| *n >= 20);
    style
}

const PRETTIER_PACKAGE_KEY: &str = "prettier";

fn has_prettier_config(dir: &Path) -> bool {
    for ancestor in dir.ancestors() {
        if let Ok(entries) = std::fs::read_dir(ancestor) {
            let found = entries.filter_map(Result::ok).any(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name == ".prettierrc" || name.starts_with(".prettierrc.") || name.starts_with("prettier.config.")
            });
            if found {
                return true;
            }
        }
        if let Ok(bytes) = std::fs::read(ancestor.join("package.json")) {
            let declared = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .is_some_and(|value| value.get(PRETTIER_PACKAGE_KEY).is_some());
            if declared {
                return true;
            }
        }
        if ancestor.join(".git").exists() {
            break;
        }
    }
    false
}

pub fn prefers_prettier(installed: bool, configured: bool) -> bool {
    installed && configured
}

fn normalize_newlines(text: &str) -> Cow<'_, str> {
    if text.contains('\r') { Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n")) } else { Cow::Borrowed(text) }
}

fn finish(mut out: String) -> String {
    if out.trim().is_empty() {
        return String::new();
    }
    let kept = out.trim_end_matches(['\n', ' ', '\t']).len();
    out.truncate(kept);
    out.push('\n');
    out
}

fn first_line(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.chars().all(|c| matches!(c, '^' | '-' | '|' | '~' | '=')))
        .unwrap_or("syntax error")
        .to_string()
}

pub fn format_builtin(kind: Builtin, text: &str, style: &Style) -> Result<String, String> {
    let text = normalize_newlines(text);
    let text = text.as_ref();
    let result = match kind {
        Builtin::Yaml => format_yaml(text, style),
        Builtin::Json => format_json(text, style),
        Builtin::Toml => format_toml(text, style),
        Builtin::Css => format_css(text, malva::Syntax::Css, style),
        Builtin::Scss => format_css(text, malva::Syntax::Scss, style),
        Builtin::Less => format_css(text, malva::Syntax::Less, style),
        Builtin::Html => format_html(text, style),
        Builtin::Markdown => format_markdown(text, style),
    };
    result.map(finish).map_err(|message| format!("{} not formatted: {}", kind.label(), first_line(&message)))
}

fn format_yaml(text: &str, style: &Style) -> Result<String, String> {
    let mut options = pretty_yaml::config::FormatOptions::default();
    options.layout.indent_width = style.indent;
    if let Some(width) = style.max_line {
        options.layout.print_width = width;
    }
    pretty_yaml::format_text(text, &options).map_err(|err| err.to_string())
}

fn format_json(text: &str, style: &Style) -> Result<String, String> {
    use dprint_plugin_json::configuration::{ConfigurationBuilder, EofNewLineKind};
    let mut builder = ConfigurationBuilder::new();
    builder
        .use_tabs(style.use_tabs)
        .indent_width(style.indent.min(u8::MAX as usize) as u8)
        .eof_new_line(EofNewLineKind::Always)
        .package_json_apply_conventions(false);
    if let Some(width) = style.max_line {
        builder.line_width(width as u32);
    }
    let config = builder.build();
    let path = Path::new("file.json");
    match dprint_plugin_json::format_text(path, text, &config) {
        Ok(Some(out)) => Ok(out),
        Ok(None) => Ok(text.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

fn format_toml(text: &str, style: &Style) -> Result<String, String> {
    let parse = taplo::parser::parse(text);
    if let Some(err) = parse.errors.first() {
        let offset = usize::from(err.range.start()).min(text.len());
        let line = text.as_bytes()[..offset].iter().filter(|b| **b == b'\n').count() + 1;
        return Err(format!("{} (line {line})", err.message));
    }
    let mut options = taplo::formatter::Options::default();
    options.indent_string = if style.use_tabs { "\t".into() } else { " ".repeat(style.indent) };
    options.trailing_newline = true;
    options.crlf = false;
    if let Some(width) = style.max_line {
        options.column_width = width;
    }
    Ok(taplo::formatter::format_syntax(parse.into_syntax(), options))
}

fn css_options(style: &Style) -> malva::config::FormatOptions {
    let mut options = malva::config::FormatOptions::default();
    options.layout.use_tabs = style.use_tabs;
    options.layout.indent_width = style.indent;
    if let Some(width) = style.max_line {
        options.layout.print_width = width;
    }
    options
}

fn format_css(text: &str, syntax: malva::Syntax, style: &Style) -> Result<String, String> {
    malva::format_text(text, syntax, &css_options(style)).map_err(|err| err.to_string())
}

fn format_html(text: &str, style: &Style) -> Result<String, String> {
    let mut options = markup_fmt::config::FormatOptions::default();
    options.layout.use_tabs = style.use_tabs;
    options.layout.indent_width = style.indent;
    if let Some(width) = style.max_line {
        options.layout.print_width = width;
    }
    let css = css_options(style);
    markup_fmt::format_text(text, markup_fmt::Language::Html, &options, |code, hints| {
        let syntax = match hints.ext {
            "css" => malva::Syntax::Css,
            "scss" => malva::Syntax::Scss,
            "less" => malva::Syntax::Less,
            _ => return Ok(Cow::Borrowed(code)),
        };
        let mut css = css.clone();
        css.layout.print_width = hints.print_width;
        malva::format_text(code, syntax, &css)
            .map(Cow::Owned)
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    })
    .map_err(|err| err.to_string())
}

fn format_markdown(text: &str, style: &Style) -> Result<String, String> {
    use dprint_plugin_markdown::configuration::ConfigurationBuilder;
    let mut builder = ConfigurationBuilder::new();
    if let Some(width) = style.max_line {
        builder.line_width(width as u32);
    }
    let config = builder.build();
    match dprint_plugin_markdown::format_text(text, &config, |_, _, _| Ok(None)) {
        Ok(Some(out)) => Ok(out),
        Ok(None) => Ok(text.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

fn format_with_builtin(kind: Builtin, path: &Path, text: &str, dir: &Path) -> Outcome {
    let prettier = resolve(Tool::Prettier);
    let configured = has_prettier_config(dir);
    let result = match prettier {
        Some(program) if prefers_prettier(true, configured) && kind != Builtin::Toml => {
            run_tool(Tool::Prettier, &program, path, text)
        }
        _ => format_builtin(kind, text, &style_for(path)),
    };
    match result {
        Ok(out) if out == text => Outcome::Unchanged { notice: None },
        Ok(out) => Outcome::Changed { text: out, notice: None },
        Err(err) => Outcome::Failed(err),
    }
}

pub fn format_on_save(path: &Path, text: &str) -> Option<Outcome> {
    builtin_for(path)?;
    Some(format(path, crate::files::language_for(path), text))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

pub fn minimal_edit(old: &str, new: &str) -> Option<Edit> {
    if old == new {
        return None;
    }
    let mut start = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let room = old.len().min(new.len()) - start;
    let mut tail = old.bytes().rev().zip(new.bytes().rev()).take_while(|(a, b)| a == b).count().min(room);
    while !old.is_char_boundary(old.len() - tail) || !new.is_char_boundary(new.len() - tail) {
        tail -= 1;
    }
    Some(Edit { start, end: old.len() - tail, text: new[start..new.len() - tail].to_string() })
}

impl Edit {
    pub fn map_offset(&self, offset: usize) -> usize {
        if offset <= self.start {
            offset
        } else if offset >= self.end {
            offset - (self.end - self.start) + self.text.len()
        } else {
            let mut inside = (offset - self.start).min(self.text.len());
            while !self.text.is_char_boundary(inside) {
                inside -= 1;
            }
            self.start + inside
        }
    }
}

pub fn position_at(text: &str, offset: usize) -> (u32, u32) {
    let head = &text[..offset.min(text.len())];
    let line = head.bytes().filter(|b| *b == b'\n').count();
    let line_start = head.rfind('\n').map_or(0, |ix| ix + 1);
    (line as u32, head[line_start..].encode_utf16().count() as u32)
}

fn xcode_swift_format() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND.get_or_init(|| xcrun_find("swift-format")).clone()
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

#[derive(Clone, Debug, Default, PartialEq)]
struct EditorConfig {
    style: Option<String>,
    size: Option<usize>,
    max_line: Option<usize>,
}

fn editorconfig_for(path: &Path) -> EditorConfig {
    let mut found = EditorConfig::default();
    let (Some(name), Some(parent)) = (path.file_name(), path.parent()) else { return found };
    let name = name.to_string_lossy().into_owned();
    for dir in parent.ancestors() {
        let Ok(text) = std::fs::read_to_string(dir.join(".editorconfig")) else { continue };
        let (section, root) = parse_editorconfig(&text, &name);
        found.style = found.style.or(section.style);
        found.size = found.size.or(section.size);
        found.max_line = found.max_line.or(section.max_line);
        if root {
            break;
        }
    }
    found
}

fn editorconfig_indent(path: &Path) -> Option<String> {
    let EditorConfig { style, size, .. } = editorconfig_for(path);
    match style.as_deref() {
        Some("tab") => Some("\t".into()),
        Some("space") | None if size.is_some() => Some(" ".repeat(size.unwrap_or(4).clamp(1, 8))),
        Some("space") => Some("    ".into()),
        _ => None,
    }
}

fn parse_editorconfig(text: &str, file_name: &str) -> (EditorConfig, bool) {
    let (mut found, mut root) = (EditorConfig::default(), false);
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
                "indent_style" => found.style = Some(value),
                "indent_size" => found.size = value.parse().ok().or(found.size),
                "max_line_length" => found.max_line = value.parse().ok().or(found.max_line),
                _ => {}
            }
        }
    }
    (found, root)
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
        let kt = EditorConfig { style: Some("space".into()), size: Some(4), max_line: None };
        assert_eq!(parse_editorconfig(text, "a.kt"), (kt, true));
        let gradle = EditorConfig { style: None, size: Some(2), max_line: None };
        assert_eq!(parse_editorconfig(text, "a.gradle"), (gradle, true));
        assert!(glob_matches("**/*.gradle", "build.gradle"));
        assert!(!glob_matches("*.kt", "a.kts"));
    }

    fn fmt(kind: Builtin, text: &str) -> String {
        format_builtin(kind, text, &Style::default()).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-format-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn yaml_reindents_and_ends_with_newline() {
        assert_eq!(fmt(Builtin::Yaml, "a:\n      b: 1\n      c:\n          - x\n          - y"), "a:\n  b: 1\n  c:\n    - x\n    - y\n");
    }

    #[test]
    fn yaml_converts_crlf_to_lf() {
        assert_eq!(fmt(Builtin::Yaml, "a: 1\r\nb: 2\r\n"), "a: 1\nb: 2\n");
    }

    #[test]
    fn yaml_error_names_the_language() {
        let err = format_builtin(Builtin::Yaml, "a: [1, 2\nb: : :\n", &Style::default()).unwrap_err();
        assert!(err.starts_with("YAML not formatted: "), "{err}");
        assert!(!err.contains('\n'));
    }

    #[test]
    fn json_formats_with_two_spaces() {
        assert_eq!(fmt(Builtin::Json, "{\n        \"a\":1,\n        \"b\":[\n1,2,\n3]}"), "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2,\n    3\n  ]\n}\n");
    }

    #[test]
    fn json_keeps_comments() {
        assert_eq!(fmt(Builtin::Json, "{\n// note\n\"a\":1}"), "{\n  // note\n  \"a\": 1\n}\n");
    }

    #[test]
    fn json_error() {
        let err = format_builtin(Builtin::Json, "{\"a\": }", &Style::default()).unwrap_err();
        assert!(err.starts_with("JSON not formatted: "), "{err}");
    }

    #[test]
    fn toml_formats_and_ends_with_newline() {
        assert_eq!(fmt(Builtin::Toml, "[package]\nname   =   \"x\"\nversion=\"1\""), "[package]\nname = \"x\"\nversion = \"1\"\n");
    }

    #[test]
    fn toml_error_reports_line() {
        let err = format_builtin(Builtin::Toml, "a = 1\nb = = 2\n", &Style::default()).unwrap_err();
        assert!(err.starts_with("TOML not formatted: "), "{err}");
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn css_scss_less_format() {
        assert_eq!(fmt(Builtin::Css, "a{color:red;margin:0 auto}"), "a {\n  color: red;\n  margin: 0 auto;\n}\n");
        assert_eq!(fmt(Builtin::Scss, ".a{.b{color:red}}"), ".a {\n  .b {\n    color: red;\n  }\n}\n");
        assert_eq!(fmt(Builtin::Less, "@c:red;.a{color:@c}"), "@c: red;\n.a {\n  color: @c;\n}\n");
    }

    #[test]
    fn css_error() {
        let err = format_builtin(Builtin::Css, "a{color:red", &Style::default()).unwrap_err();
        assert!(err.starts_with("CSS not formatted: "), "{err}");
    }

    #[test]
    fn html_formats_with_two_spaces() {
        let out = fmt(Builtin::Html, "<div><p>hi</p><ul><li>a</li><li>b</li></ul></div>");
        assert_eq!(out, "<div>\n  <p>hi</p>\n  <ul>\n    <li>a</li>\n    <li>b</li>\n  </ul>\n</div>\n");
    }

    #[test]
    fn html_formats_embedded_css() {
        let out = fmt(Builtin::Html, "<style>a{color:red}</style>");
        assert!(out.contains("  color: red;"), "{out}");
    }

    #[test]
    fn html_error() {
        let err = format_builtin(Builtin::Html, "<div><span></div>", &Style::default()).unwrap_err();
        assert!(err.starts_with("HTML not formatted: "), "{err}");
    }

    #[test]
    fn markdown_formats() {
        let out = fmt(Builtin::Markdown, "# Title\n\n\n\n* a\n* b\n\nText   here\n");
        assert_eq!(out, "# Title\n\n- a\n- b\n\nText here\n");
    }

    #[test]
    fn blank_input_stays_blank() {
        for kind in [Builtin::Yaml, Builtin::Json, Builtin::Toml, Builtin::Css, Builtin::Markdown] {
            assert_eq!(format_builtin(kind, "", &Style::default()).map(|s| s.is_empty()), Ok(true), "{kind:?}");
        }
    }

    #[test]
    fn style_changes_indent() {
        let four = Style { use_tabs: false, indent: 4, max_line: None };
        assert_eq!(format_builtin(Builtin::Yaml, "a:\n b: 1", &four).unwrap(), "a:\n    b: 1\n");
        let tabs = Style { use_tabs: true, indent: 2, max_line: None };
        assert_eq!(format_builtin(Builtin::Json, "{\n\"a\":{\n\"b\":1}}", &tabs).unwrap(), "{\n\t\"a\": {\n\t\t\"b\": 1\n\t}\n}\n");
    }

    #[test]
    fn builtin_selection_by_extension() {
        assert_eq!(builtin_for(Path::new("/p/a.yml")), Some(Builtin::Yaml));
        assert_eq!(builtin_for(Path::new("/p/a.YAML")), Some(Builtin::Yaml));
        assert_eq!(builtin_for(Path::new("/p/a.jsonc")), Some(Builtin::Json));
        assert_eq!(builtin_for(Path::new("/p/Cargo.lock")), Some(Builtin::Toml));
        assert_eq!(builtin_for(Path::new("/p/a.scss")), Some(Builtin::Scss));
        assert_eq!(builtin_for(Path::new("/p/a.md")), Some(Builtin::Markdown));
        assert_eq!(builtin_for(Path::new("/p/a.mdx")), None);
        assert_eq!(builtin_for(Path::new("/p/a.xcstrings")), None);
        assert_eq!(builtin_for(Path::new("/p/a.swift")), None);
        assert_eq!(builtin_for(Path::new("/p/Makefile")), None);
    }

    #[test]
    fn format_on_save_only_for_builtin_languages() {
        assert!(format_on_save(Path::new("/nonexistent/a.swift"), "x").is_none());
        assert!(format_on_save(Path::new("/nonexistent/a.xml"), "<a/>").is_none());
        assert!(matches!(format_on_save(Path::new("/nonexistent/a.toml"), "a=1"), Some(Outcome::Changed { .. })));
        assert!(matches!(format_on_save(Path::new("/nonexistent/a.toml"), "a = 1\n"), Some(Outcome::Unchanged { .. })));
        assert!(matches!(format_on_save(Path::new("/nonexistent/a.toml"), "a = = 1\n"), Some(Outcome::Failed(_))));
    }

    #[test]
    fn editorconfig_drives_style() {
        let dir = scratch("ec");
        std::fs::write(
            dir.join(".editorconfig"),
            "root = true\n[*]\nindent_style = space\nindent_size = 4\nmax_line_length = 100\n[*.json]\nindent_style = tab\n",
        )
        .unwrap();
        let yaml = style_for(&dir.join("a.yml"));
        assert_eq!(yaml, Style { use_tabs: false, indent: 4, max_line: Some(100) });
        let json = style_for(&dir.join("a.json"));
        assert!(json.use_tabs);
        assert_eq!(format_builtin(Builtin::Yaml, "a:\n b: 1\n", &yaml).unwrap(), "a:\n    b: 1\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn editorconfig_absent_gives_defaults() {
        let dir = scratch("ec-none");
        std::fs::write(dir.join(".editorconfig"), "root = true\n[*.kt]\nindent_size = 8\n").unwrap();
        assert_eq!(style_for(&dir.join("a.yml")), Style::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn editorconfig_nearest_file_wins() {
        let dir = scratch("ec-nested");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join(".editorconfig"), "root = true\n[*]\nindent_size = 4\nmax_line_length = 90\n").unwrap();
        std::fs::write(dir.join("sub/.editorconfig"), "[*]\nindent_size = 3\n").unwrap();
        assert_eq!(style_for(&dir.join("sub/a.yml")), Style { use_tabs: false, indent: 3, max_line: Some(90) });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prettier_decision_needs_install_and_config() {
        assert!(prefers_prettier(true, true));
        assert!(!prefers_prettier(true, false));
        assert!(!prefers_prettier(false, true));
        assert!(!prefers_prettier(false, false));
    }

    #[test]
    fn detects_prettier_config_files() {
        for name in [".prettierrc", ".prettierrc.json", ".prettierrc.yaml", "prettier.config.js", "prettier.config.mjs"] {
            let dir = scratch("pc");
            std::fs::write(dir.join(name), "{}").unwrap();
            std::fs::create_dir_all(dir.join("src/deep")).unwrap();
            assert!(has_prettier_config(&dir.join("src/deep")), "{name}");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn detects_prettier_key_in_package_json() {
        let dir = scratch("pc-pkg");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("package.json"), "{\"name\": \"x\", \"devDependencies\": {\"prettier\": \"3\"}}").unwrap();
        assert!(!has_prettier_config(&dir));
        std::fs::write(dir.join("package.json"), "{\"name\": \"x\", \"prettier\": {\"semi\": false}}").unwrap();
        assert!(has_prettier_config(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prettier_search_stops_at_repository_root() {
        let dir = scratch("pc-root");
        std::fs::write(dir.join(".prettierrc"), "{}").unwrap();
        std::fs::create_dir_all(dir.join("repo/.git")).unwrap();
        assert!(!has_prettier_config(&dir.join("repo")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn minimal_edit_covers_only_the_difference() {
        let edit = minimal_edit("a:\n    b: 1\nc: 2\n", "a:\n  b: 1\nc: 2\n").unwrap();
        assert_eq!(edit, Edit { start: 5, end: 7, text: String::new() });
        assert_eq!(minimal_edit("same", "same"), None);
        assert_eq!(minimal_edit("", "x\n"), Some(Edit { start: 0, end: 0, text: "x\n".into() }));
        assert_eq!(minimal_edit("abc", "abXc"), Some(Edit { start: 2, end: 2, text: "X".into() }));
        assert_eq!(minimal_edit("aaa", "aa"), Some(Edit { start: 2, end: 3, text: String::new() }));
    }

    #[test]
    fn minimal_edit_respects_char_boundaries() {
        let edit = minimal_edit("é", "è").unwrap();
        assert_eq!(edit, Edit { start: 0, end: 2, text: "è".into() });
        let old = "x\u{1F600}y";
        let new = "x\u{1F601}y";
        let edit = minimal_edit(old, new).unwrap();
        let mut applied = old.to_string();
        applied.replace_range(edit.start..edit.end, &edit.text);
        assert_eq!(applied, new);
    }

    #[test]
    fn edit_maps_offsets_around_the_change() {
        let edit = Edit { start: 3, end: 7, text: "  ".into() };
        assert_eq!(edit.map_offset(2), 2);
        assert_eq!(edit.map_offset(3), 3);
        assert_eq!(edit.map_offset(7), 5);
        assert_eq!(edit.map_offset(10), 8);
        assert_eq!(edit.map_offset(5), 5);
    }

    #[test]
    fn positions_count_utf16_units() {
        assert_eq!(position_at("ab\ncd", 4), (1, 1));
        assert_eq!(position_at("a\u{1F600}b", 5), (0, 3));
        assert_eq!(position_at("x\n", 2), (1, 0));
    }
}
