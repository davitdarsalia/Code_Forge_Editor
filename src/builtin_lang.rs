use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui_kit::component::highlighter::LanguageRegistry;
use gpui_kit::component::input::Rope;
use lsp_types::{Diagnostic, DiagnosticSeverity, LocationLink, Position, Range};

use crate::lsp::{self, Event};
use crate::{files, symbols};

const COVERED: &[&str] = &["kotlin", "groovy", "xml"];
const NO_SYNTAX_CHECK: &[&str] = &["text", "markdown", "markdown-inline", "diff", "gitignore", "make", "ini", "properties"];
const INDEXED: &[&str] = &[
    "kotlin", "groovy", "java", "swift", "objc", "c", "cpp", "rust", "go", "python", "javascript", "typescript", "tsx",
    "ruby", "dart", "csharp", "scala", "lua", "php", "elixir", "zig",
];
const FAMILIES: &[&[&str]] = &[
    &["kotlin", "java", "groovy"],
    &["c", "cpp", "objc"],
    &["javascript", "typescript", "tsx"],
];
const MAX_DIAGNOSTICS: usize = 50;
const MAX_INDEXED_FILES: usize = 20_000;
const MAX_INDEXED_BYTES: u64 = 512 * 1024;

pub fn covers(language: &str, server_failed: bool) -> bool {
    COVERED.contains(&language) || (server_failed && !NO_SYNTAX_CHECK.contains(&language))
}

pub fn indexes(language: &str) -> bool {
    INDEXED.contains(&language)
}

fn same_family(a: &str, b: &str) -> bool {
    a == b || FAMILIES.iter().any(|family| family.contains(&a) && family.contains(&b))
}

fn utf16_column(line: &str, byte: usize) -> u32 {
    let head = line.get(..byte.min(line.len())).unwrap_or(line);
    head.chars().map(|c| c.len_utf16() as u32).sum()
}

fn position(lines: &[&str], point: tree_sitter::Point) -> Position {
    let line = lines.get(point.row).copied().unwrap_or("");
    Position::new(point.row as u32, utf16_column(line, point.column))
}

pub fn syntax_diagnostics(language: &tree_sitter::Language, text: &str) -> Vec<Diagnostic> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else { return Vec::new() };
    let root = tree.root_node();
    if !root.has_error() {
        return Vec::new();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if out.len() >= MAX_DIAGNOSTICS {
            break;
        }
        let message = if node.is_missing() {
            Some(if node.is_named() { format!("Missing {}", node.kind()) } else { format!("Missing `{}`", node.kind()) })
        } else if node.is_error() {
            Some("Syntax error".to_owned())
        } else {
            None
        };
        if let Some(message) = message {
            let start = node.start_position();
            let mut end = node.end_position();
            if end.row != start.row {
                let width = lines.get(start.row).map_or(0, |l| l.trim_end_matches('\r').len());
                end = tree_sitter::Point { row: start.row, column: width };
            }
            out.push(Diagnostic {
                range: Range::new(position(&lines, start), position(&lines, end)),
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("syntax".to_owned()),
                message,
                ..Diagnostic::default()
            });
        } else if node.has_error() {
            let mut cursor = node.walk();
            let children: Vec<_> = node.children(&mut cursor).collect();
            stack.extend(children.into_iter().rev());
        }
    }
    out.sort_by_key(|d| (d.range.start.line, d.range.start.character));
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Loc {
    pub path: PathBuf,
    pub language: &'static str,
    pub line: u32,
    pub column: u32,
    pub len: u32,
}

fn locations(path: &Path, language: &'static str, text: &str) -> Vec<(String, Loc)> {
    let mut out = Vec::new();
    for symbol in symbols::extract(text) {
        let len = symbol.name.chars().count() as u32;
        let loc = Loc { path: path.to_path_buf(), language, line: symbol.line, column: symbol.column, len };
        if let Some((_, last)) = symbol.name.rsplit_once('.') {
            let last_len = last.chars().count() as u32;
            if last_len > 0 {
                let shifted = Loc { column: symbol.column + (len - last_len), len: last_len, ..loc.clone() };
                out.push((last.to_owned(), shifted));
            }
        }
        out.push((symbol.name, loc));
    }
    out
}

struct Waiter {
    name: String,
    from: PathBuf,
    language: String,
    reply: flume::Sender<Vec<Loc>>,
}

type FileLocs = (PathBuf, Vec<(String, Loc)>);

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Building { waiters: Vec<Waiter>, updates: Vec<FileLocs> },
    Ready,
}

#[derive(Default)]
struct Inner {
    generation: u64,
    state: State,
    names: HashMap<String, Vec<Loc>>,
}

#[derive(Default)]
pub struct Index {
    inner: Mutex<Inner>,
}

fn answer(names: &HashMap<String, Vec<Loc>>, waiter: &Waiter) -> Vec<Loc> {
    let mut found: Vec<Loc> = names
        .get(&waiter.name)
        .map(|locs| locs.iter().filter(|l| same_family(l.language, &waiter.language)).cloned().collect())
        .unwrap_or_default();
    found.sort_by(|a, b| (a.path != waiter.from, &a.path, a.line).cmp(&(b.path != waiter.from, &b.path, b.line)));
    found.dedup();
    found
}

fn scan(root: &Path) -> Vec<FileLocs> {
    let walker = ignore::WalkBuilder::new(root)
        .filter_entry(|entry| !files::IGNORED.contains(&entry.file_name().to_string_lossy().as_ref()))
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten().filter(|e| e.file_type().is_some_and(|t| t.is_file())) {
        if out.len() >= MAX_INDEXED_FILES {
            break;
        }
        let path = entry.path();
        let language = files::language_for(path);
        if !indexes(language) || entry.metadata().map_or(true, |m| m.len() > MAX_INDEXED_BYTES) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        out.push((path.to_path_buf(), locations(path, language, &text)));
    }
    out
}

impl Index {
    pub fn reset(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.generation += 1;
        inner.state = State::Idle;
        inner.names.clear();
    }

    pub fn lookup(self: &Arc<Self>, root: PathBuf, name: String, from: PathBuf, language: String) -> flume::Receiver<Vec<Loc>> {
        let (reply, receiver) = flume::bounded(1);
        let waiter = Waiter { name, from, language, reply };
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let generation = inner.generation;
        match &mut inner.state {
            State::Ready => {
                let _ = waiter.reply.send(answer(&inner.names, &waiter));
            }
            State::Building { waiters, .. } => waiters.push(waiter),
            State::Idle => {
                inner.state = State::Building { waiters: vec![waiter], updates: Vec::new() };
                let index = self.clone();
                std::thread::spawn(move || index.build(generation, &root));
            }
        }
        receiver
    }

    fn build(&self, generation: u64, root: &Path) {
        let scanned = scan(root);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.generation != generation {
            return;
        }
        let State::Building { waiters, updates } = std::mem::replace(&mut inner.state, State::Ready) else { return };
        let mut names: HashMap<String, Vec<Loc>> = HashMap::new();
        for (name, loc) in scanned.into_iter().flat_map(|(_, locs)| locs) {
            names.entry(name).or_default().push(loc);
        }
        inner.names = names;
        for (path, locs) in updates {
            replace_file(&mut inner.names, &path, locs);
        }
        for waiter in waiters {
            let _ = waiter.reply.send(answer(&inner.names, &waiter));
        }
    }

    pub fn update_file(&self, path: &Path, language: &'static str, text: &str) {
        if !indexes(language) {
            return;
        }
        let locs = locations(path, language, text);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match &mut inner.state {
            State::Idle => {}
            State::Building { updates, .. } => updates.push((path.to_path_buf(), locs)),
            State::Ready => replace_file(&mut inner.names, path, locs),
        }
    }
}

fn replace_file(names: &mut HashMap<String, Vec<Loc>>, path: &Path, locs: Vec<(String, Loc)>) {
    names.retain(|_, entries| {
        entries.retain(|l| l.path != path);
        !entries.is_empty()
    });
    for (name, loc) in locs {
        names.entry(name).or_default().push(loc);
    }
}

pub fn word_at(line: &str, column: usize) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut at = column.min(chars.len());
    if at == chars.len() || !is_word(chars[at]) {
        at = at.checked_sub(1).filter(|i| is_word(chars[*i]))?;
    }
    let mut begin = at;
    while begin > 0 && is_word(chars[begin - 1]) {
        begin -= 1;
    }
    let mut end = at;
    while end < chars.len() && is_word(chars[end]) {
        end += 1;
    }
    let word: String = chars[begin..end].iter().collect();
    (!word.starts_with(|c: char| c.is_ascii_digit())).then_some((word, begin))
}

pub fn link(origin: Range, loc: &Loc) -> Option<LocationLink> {
    let target = Range::new(Position::new(loc.line, loc.column), Position::new(loc.line, loc.column + loc.len));
    Some(LocationLink {
        origin_selection_range: Some(origin),
        target_uri: lsp::path_to_uri(&loc.path).parse().ok()?,
        target_range: target,
        target_selection_range: target,
    })
}

struct Job {
    path: PathBuf,
    language: &'static str,
    text: Rope,
}

pub struct Engine {
    pub index: Arc<Index>,
    jobs: flume::Sender<Job>,
    seen: Mutex<HashSet<PathBuf>>,
}

impl Engine {
    pub fn new(events: flume::Sender<Event>) -> Engine {
        let index = Arc::new(Index::default());
        let (jobs, queue) = flume::unbounded::<Job>();
        let worker_index = index.clone();
        std::thread::spawn(move || worker(queue, events, worker_index));
        Engine { index, jobs, seen: Mutex::new(HashSet::new()) }
    }

    pub fn reset(&self) {
        self.index.reset();
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub fn sync(&self, language: &'static str, path: &Path, stale: bool, text: Rope) {
        let first = self.seen.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf());
        if first || stale {
            let _ = self.jobs.send(Job { path: path.to_path_buf(), language, text });
        }
    }

    pub fn forget(&self, path: &Path) {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).remove(path);
    }
}

fn worker(queue: flume::Receiver<Job>, events: flume::Sender<Event>, index: Arc<Index>) {
    let mut grammars: HashMap<&'static str, Option<tree_sitter::Language>> = HashMap::new();
    while let Ok(first) = queue.recv() {
        let mut batch: HashMap<PathBuf, Job> = HashMap::new();
        batch.insert(first.path.clone(), first);
        while let Ok(job) = queue.try_recv() {
            batch.insert(job.path.clone(), job);
        }
        for job in batch.into_values() {
            let text = job.text.to_string();
            index.update_file(&job.path, job.language, &text);
            if NO_SYNTAX_CHECK.contains(&job.language) {
                continue;
            }
            let grammar = grammars
                .entry(job.language)
                .or_insert_with(|| LanguageRegistry::singleton().language(job.language).and_then(|c| c.language))
                .clone();
            let Some(grammar) = grammar else { continue };
            let diagnostics = syntax_diagnostics(&grammar, &text);
            if events.send(Event::Diagnostics { path: job.path, diagnostics }).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn grammar(name: &str) -> tree_sitter::Language {
        crate::languages::register();
        LanguageRegistry::singleton().language(name).and_then(|c| c.language).expect(name)
    }

    #[test]
    fn valid_code_has_no_syntax_diagnostics() {
        let kotlin = grammar("kotlin");
        assert!(syntax_diagnostics(&kotlin, "package a\n\nfun main() {\n    println(\"hi\")\n}\n").is_empty());
        assert!(syntax_diagnostics(&grammar("xml"), "<a><b/></a>\n").is_empty());
    }

    #[test]
    fn broken_code_reports_errors_with_positions() {
        let kotlin = grammar("kotlin");
        let found = syntax_diagnostics(&kotlin, "fun main() {\n    val x = (1 +\n}\n");
        assert!(!found.is_empty());
        assert!(found.iter().all(|d| d.severity == Some(DiagnosticSeverity::ERROR)));
        assert!(found.iter().any(|d| d.message == "Syntax error" || d.message.starts_with("Missing")), "{found:?}");
        let xml = syntax_diagnostics(&grammar("xml"), "<a><b></a>\n");
        assert!(!xml.is_empty(), "{xml:?}");
        let groovy = syntax_diagnostics(&grammar("groovy"), "def x = (1 + \n");
        assert!(!groovy.is_empty(), "{groovy:?}");
    }

    #[test]
    fn eligibility_follows_coverage_and_failures() {
        assert!(covers("kotlin", false) && covers("groovy", false) && covers("xml", false));
        assert!(!covers("go", false));
        assert!(covers("go", true));
        assert!(!covers("markdown", true));
    }

    #[test]
    fn words_are_found_around_the_cursor() {
        assert_eq!(word_at("val fooBar = 1", 6), Some(("fooBar".into(), 4)));
        assert_eq!(word_at("val fooBar = 1", 10), Some(("fooBar".into(), 4)));
        assert_eq!(word_at("a b", 1), Some(("a".into(), 0)));
        assert_eq!(word_at("  ", 1), None);
        assert_eq!(word_at("x = 12", 5), None);
    }

    #[test]
    fn index_builds_lazily_and_updates_files() {
        let dir = std::env::temp_dir().join(format!("forge-builtin-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/A.kt"), "class Greeter {\n}\nfun String.shout() {}\n").unwrap();
        std::fs::write(dir.join("src/B.kt"), "fun helper() {}\n").unwrap();
        std::fs::write(dir.join("src/C.swift"), "class Greeter {}\n").unwrap();
        let index = Arc::new(Index::default());
        let ask = |name: &str, from: &Path, language: &str| {
            let reply = index.lookup(dir.clone(), name.into(), from.to_path_buf(), language.into());
            reply.recv_timeout(Duration::from_secs(10)).unwrap()
        };
        let a = dir.join("src/A.kt");
        let hits = ask("Greeter", &a, "kotlin");
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].path.as_path(), hits[0].line, hits[0].column), (a.as_path(), 0, 6));
        assert_eq!(ask("shout", &a, "kotlin")[0].column, 11);
        assert_eq!(ask("helper", &a, "kotlin").len(), 1);
        index.update_file(&dir.join("src/B.kt"), "kotlin", "fun renamed() {}\n");
        assert!(ask("helper", &a, "kotlin").is_empty());
        assert_eq!(ask("renamed", &a, "kotlin").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_publishes_syntax_diagnostics_off_thread() {
        crate::languages::register();
        let (events, receiver) = flume::unbounded();
        let engine = Engine::new(events);
        let path = PathBuf::from("/tmp/forge-engine/A.kt");
        engine.sync("kotlin", &path, false, Rope::from("fun main( {\n"));
        match receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
            Event::Diagnostics { path: got, diagnostics } => {
                assert_eq!(got, path);
                assert!(!diagnostics.is_empty());
            }
            Event::Stopped { .. } => panic!("unexpected stop"),
        }
        engine.sync("kotlin", &path, false, Rope::from("fun main() {}\n"));
        assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
        engine.sync("kotlin", &path, true, Rope::from("fun main() {}\n"));
        match receiver.recv_timeout(Duration::from_secs(10)).unwrap() {
            Event::Diagnostics { diagnostics, .. } => assert!(diagnostics.is_empty()),
            Event::Stopped { .. } => panic!("unexpected stop"),
        }
    }
}
