use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use gpui_kit::component::input::{DefinitionProvider, Rope, RopeExt as _};
use gpui_kit::{App, Task, Window};
use lsp_types::{GotoDefinitionResponse, LocationLink, Position, PublishDiagnosticsParams, Range};
use serde_json::{Value, json};

use crate::{builtin_lang, format};

const IDLE_SHUTDOWN: Duration = Duration::from_secs(10 * 60);
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
const STDERR_TAIL: usize = 4;
const CONTENT_MODIFIED: i64 = -32801;
const MAX_CRASHES: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Swift,
    Rust,
    Clangd,
    Go,
    TypeScript,
    Json,
    Css,
    Html,
    Yaml,
    Toml,
    Markdown,
    Shell,
    Python,
    Kotlin,
    Xml,
}

struct Spec {
    kind: Kind,
    label: &'static str,
    languages: &'static [&'static str],
    server_name: &'static str,
    language_id: fn(&str, &Path) -> &'static str,
    markers: &'static [&'static str],
    prefers_topmost: bool,
    binary: &'static str,
    args: &'static [&'static str],
    xcrun: bool,
    missing: &'static str,
    init_options: fn(&Path) -> Value,
    settings: fn() -> Value,
    pull_diagnostics: bool,
    optional: bool,
    skip: fn(&Path) -> bool,
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn clangd_language_id(language: &str, path: &Path) -> &'static str {
    match (language, extension(path).as_str()) {
        ("c", _) => "c",
        ("objc", "mm") => "objective-cpp",
        ("objc", _) => "objective-c",
        _ => "cpp",
    }
}

fn typescript_language_id(_: &str, path: &Path) -> &'static str {
    match extension(path).as_str() {
        "jsx" => "javascriptreact",
        "tsx" => "typescriptreact",
        "ts" | "mts" | "cts" => "typescript",
        _ => "javascript",
    }
}

fn json_language_id(_: &str, path: &Path) -> &'static str {
    let name = file_name(path);
    if name.starts_with("tsconfig") || name.starts_with("jsconfig") || matches!(extension(path).as_str(), "jsonc" | "json5") {
        "jsonc"
    } else {
        "json"
    }
}

fn css_language_id(_: &str, path: &Path) -> &'static str {
    match extension(path).as_str() {
        "scss" => "scss",
        "less" => "less",
        _ => "css",
    }
}

fn typescript_init_options(root: &Path) -> Value {
    let tsserver = |dir: &Path| Some(dir.join("typescript/lib/tsserver.js")).filter(|p| p.is_file());
    let local = root.ancestors().find_map(|dir| tsserver(&dir.join("node_modules")));
    let found = local.or_else(|| {
        let binary = format::which("typescript-language-server")?;
        let real = binary.canonicalize().unwrap_or(binary);
        real.ancestors().find_map(|dir| tsserver(dir).or_else(|| tsserver(&dir.join("node_modules"))))
    });
    match found {
        Some(path) => json!({ "hostInfo": "forge", "tsserver": { "path": path } }),
        None => json!({ "hostInfo": "forge" }),
    }
}

fn never_skip(_: &Path) -> bool {
    false
}

fn skip_non_shell(path: &Path) -> bool {
    matches!(file_name(path).as_str(), "dockerfile" | ".env") || extension(path) == "fish"
}

fn skip_non_xml(path: &Path) -> bool {
    extension(path) != "xml"
}

fn json_settings() -> Value {
    let schema = |matches: &[&str], name: &str| {
        json!({ "fileMatch": matches, "url": format!("https://json.schemastore.org/{name}.json") })
    };
    json!({
        "json": {
            "validate": { "enable": true },
            "format": { "enable": true },
            "schemas": [
                schema(&["package.json"], "package"),
                schema(&["tsconfig.json", "tsconfig.*.json"], "tsconfig"),
                schema(&["jsconfig.json"], "jsconfig"),
                schema(&[".prettierrc", ".prettierrc.json"], "prettierrc"),
                schema(&[".eslintrc", ".eslintrc.json"], "eslintrc"),
            ],
        },
        "http": { "proxy": "", "proxyStrictSSL": true },
    })
}

fn css_settings() -> Value {
    let section = || json!({ "validate": true, "lint": {} });
    json!({ "css": section(), "scss": section(), "less": section() })
}

fn html_settings() -> Value {
    json!({
        "html": {
            "validate": { "scripts": true, "styles": true },
            "format": { "enable": true },
            "suggest": {},
            "autoClosingTags": true,
        },
        "css": { "validate": true },
        "javascript": { "format": {} },
    })
}

fn yaml_settings() -> Value {
    json!({
        "yaml": {
            "validate": true,
            "completion": true,
            "hover": true,
            "format": { "enable": true },
            "schemaStore": { "enable": true, "url": "https://www.schemastore.org/api/json/catalog.json" },
            "schemas": {},
        },
        "http": { "proxy": "", "proxyStrictSSL": true },
        "files": { "associations": {} },
    })
}

fn python_settings() -> Value {
    json!({
        "python": {
            "analysis": {
                "autoSearchPaths": true,
                "useLibraryCodeForTypes": true,
                "diagnosticMode": "openFilesOnly",
                "typeCheckingMode": "standard",
            },
        },
        "pyright": { "disableOrganizeImports": false },
    })
}

fn empty_object() -> Value {
    json!({})
}

const BASE: Spec = Spec {
    kind: Kind::Swift,
    label: "",
    languages: &[],
    server_name: "",
    language_id: |_, _| "plaintext",
    markers: &[],
    prefers_topmost: false,
    binary: "",
    args: &[],
    xcrun: false,
    missing: "",
    init_options: |_| Value::Null,
    settings: empty_object,
    pull_diagnostics: false,
    optional: false,
    skip: never_skip,
};

const SPECS: &[Spec] = &[
    Spec {
        kind: Kind::Swift,
        label: "Swift",
        languages: &["swift"],
        server_name: "sourcekit-lsp",
        language_id: |_, _| "swift",
        markers: &["Package.swift"],
        binary: "sourcekit-lsp",
        xcrun: true,
        missing: "sourcekit-lsp not found: install Xcode or a Swift toolchain",
        ..BASE
    },
    Spec {
        kind: Kind::Rust,
        label: "Rust",
        languages: &["rust"],
        server_name: "rust-analyzer",
        language_id: |_, _| "rust",
        markers: &["Cargo.toml"],
        prefers_topmost: true,
        binary: "rust-analyzer",
        missing: "rust-analyzer not found: run `rustup component add rust-analyzer`",
        init_options: |_| json!({ "checkOnSave": true, "cargo": { "targetDir": true } }),
        ..BASE
    },
    Spec {
        kind: Kind::Clangd,
        label: "C/C++/Objective-C",
        languages: &["c", "cpp", "objc"],
        server_name: "clangd",
        language_id: clangd_language_id,
        markers: &["compile_commands.json", "compile_flags.txt", ".clangd", "CMakeLists.txt"],
        binary: "clangd",
        args: &["--log=error", "--header-insertion=never"],
        xcrun: true,
        missing: "clangd not found: install Xcode Command Line Tools (`xcode-select --install`) or `brew install llvm`",
        ..BASE
    },
    Spec {
        kind: Kind::Go,
        label: "Go",
        languages: &["go"],
        server_name: "gopls",
        language_id: |_, _| "go",
        markers: &["go.work", "go.mod"],
        binary: "gopls",
        missing: "gopls not found: run `brew install gopls`",
        ..BASE
    },
    Spec {
        kind: Kind::TypeScript,
        label: "JavaScript/TypeScript",
        languages: &["javascript", "typescript", "tsx"],
        server_name: "typescript-language-server",
        language_id: typescript_language_id,
        markers: &["package.json", "tsconfig.json", "jsconfig.json"],
        binary: "typescript-language-server",
        args: &["--stdio"],
        missing: "typescript-language-server not found: run `npm i -g typescript typescript-language-server`",
        init_options: typescript_init_options,
        ..BASE
    },
    Spec {
        kind: Kind::Json,
        label: "JSON",
        languages: &["json"],
        server_name: "vscode-json-language-server",
        language_id: json_language_id,
        binary: "vscode-json-language-server",
        args: &["--stdio"],
        missing: "vscode-json-language-server not found: run `npm i -g vscode-langservers-extracted`",
        init_options: |_| json!({ "provideFormatter": true }),
        settings: json_settings,
        pull_diagnostics: true,
        ..BASE
    },
    Spec {
        kind: Kind::Css,
        label: "CSS/SCSS/Less",
        languages: &["css"],
        server_name: "vscode-css-language-server",
        language_id: css_language_id,
        binary: "vscode-css-language-server",
        args: &["--stdio"],
        missing: "vscode-css-language-server not found: run `npm i -g vscode-langservers-extracted`",
        init_options: |_| json!({ "provideFormatter": true }),
        settings: css_settings,
        pull_diagnostics: true,
        ..BASE
    },
    Spec {
        kind: Kind::Html,
        label: "HTML",
        languages: &["html"],
        server_name: "vscode-html-language-server",
        language_id: |_, _| "html",
        binary: "vscode-html-language-server",
        args: &["--stdio"],
        missing: "vscode-html-language-server not found: run `npm i -g vscode-langservers-extracted`",
        init_options: |_| json!({ "provideFormatter": true, "embeddedLanguages": { "css": true, "javascript": true } }),
        settings: html_settings,
        pull_diagnostics: true,
        ..BASE
    },
    Spec {
        kind: Kind::Yaml,
        label: "YAML",
        languages: &["yaml"],
        server_name: "yaml-language-server",
        language_id: |_, _| "yaml",
        binary: "yaml-language-server",
        args: &["--stdio"],
        missing: "yaml-language-server not found: run `npm i -g yaml-language-server`",
        settings: yaml_settings,
        ..BASE
    },
    Spec {
        kind: Kind::Toml,
        label: "TOML",
        languages: &["toml"],
        server_name: "taplo",
        language_id: |_, _| "toml",
        binary: "taplo",
        args: &["lsp", "stdio"],
        missing: "taplo not found: run `brew install taplo`",
        ..BASE
    },
    Spec {
        kind: Kind::Markdown,
        label: "Markdown",
        languages: &["markdown"],
        markers: &[".marksman.toml", ".git"],
        server_name: "marksman",
        language_id: |_, _| "markdown",
        binary: "marksman",
        args: &["server"],
        missing: "marksman not found: run `brew install marksman`",
        ..BASE
    },
    Spec {
        kind: Kind::Shell,
        label: "Shell",
        languages: &["bash"],
        server_name: "bash-language-server",
        language_id: |_, _| "shellscript",
        binary: "bash-language-server",
        args: &["start"],
        missing: "bash-language-server not found: run `npm i -g bash-language-server`",
        skip: skip_non_shell,
        ..BASE
    },
    Spec {
        kind: Kind::Python,
        label: "Python",
        languages: &["python"],
        server_name: "pyright",
        language_id: |_, _| "python",
        markers: &["pyrightconfig.json", "pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "Pipfile"],
        binary: "pyright-langserver",
        args: &["--stdio"],
        missing: "pyright-langserver not found: run `npm i -g pyright` or `brew install pyright`",
        settings: python_settings,
        ..BASE
    },
    Spec {
        kind: Kind::Kotlin,
        label: "Kotlin",
        languages: &["kotlin"],
        server_name: "kotlin-language-server",
        language_id: |_, _| "kotlin",
        markers: &["settings.gradle.kts", "settings.gradle"],
        prefers_topmost: true,
        binary: "kotlin-language-server",
        missing: "kotlin-language-server not found",
        optional: true,
        ..BASE
    },
    Spec {
        kind: Kind::Xml,
        label: "XML",
        languages: &["xml"],
        server_name: "lemminx",
        language_id: |_, _| "xml",
        binary: "lemminx",
        missing: "lemminx not found",
        optional: true,
        skip: skip_non_xml,
        ..BASE
    },
];

impl Kind {
    fn spec(self) -> &'static Spec {
        SPECS.iter().find(|spec| spec.kind == self).unwrap_or(&SPECS[0])
    }

    pub fn for_file(language: &str, path: &Path) -> Option<Kind> {
        SPECS
            .iter()
            .find(|spec| spec.languages.contains(&language) && !(spec.skip)(path))
            .map(|spec| spec.kind)
    }

    pub fn optional(self) -> bool {
        self.spec().optional
    }

    pub fn server_name(self) -> &'static str {
        self.spec().server_name
    }

    fn language_id(self, language: &str, path: &Path) -> &'static str {
        (self.spec().language_id)(language, path)
    }

    fn has_marker(self, dir: &Path) -> bool {
        self.spec().markers.iter().any(|marker| dir.join(marker).exists())
    }

    fn command(self) -> Result<PathBuf, String> {
        let spec = self.spec();
        format::which(spec.binary)
            .or_else(|| if spec.xcrun { xcrun_find(spec.binary) } else { None })
            .ok_or_else(|| spec.missing.to_owned())
    }
}

pub fn supported_languages() -> String {
    let mut labels: Vec<&str> = SPECS.iter().map(|spec| spec.label).collect();
    labels.push("Groovy");
    match labels.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => labels.join(", "),
    }
}

fn xcrun_find(binary: &str) -> Option<PathBuf> {
    let out = Command::new("/usr/bin/xcrun").args(["--find", binary]).output().ok()?;
    let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    (out.status.success() && path.is_file()).then_some(path)
}

fn root_for(kind: Kind, project: &Path, path: &Path) -> Option<PathBuf> {
    if !path.starts_with(project) {
        return None;
    }
    let mut manifests = path
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(project))
        .filter(|dir| kind.has_marker(dir));
    let found = if kind.spec().prefers_topmost { manifests.last() } else { manifests.next() };
    Some(found.unwrap_or(project).to_path_buf())
}

pub fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = &rest[rest.find('/')?..];
    Some(PathBuf::from(crate::percent_decode(rest)))
}

fn utf16_column(line: &str, chars: u32) -> u32 {
    line.chars().take(chars as usize).map(|c| c.len_utf16() as u32).sum()
}

pub fn char_column(line: &str, utf16: u32) -> u32 {
    let mut units = 0;
    let mut chars = 0;
    for c in line.chars() {
        if units >= utf16 {
            break;
        }
        units += c.len_utf16() as u32;
        chars += 1;
    }
    chars
}

pub fn line_text(text: &Rope, line: u32) -> String {
    if (line as usize) < text.lines_len() { text.slice_line(line as usize).to_string() } else { String::new() }
}

fn to_lsp(text: &Rope, offset: usize) -> Position {
    let pos = text.offset_to_position(offset);
    Position::new(pos.line, utf16_column(&line_text(text, pos.line), pos.character))
}

pub fn from_lsp(text: &Rope, pos: Position) -> Position {
    Position::new(pos.line, char_column(&line_text(text, pos.line), pos.character))
}

pub enum Event {
    Diagnostics { path: PathBuf, diagnostics: Vec<lsp_types::Diagnostic> },
    Stopped { kind: Kind, reason: String, spawn_failed: bool },
}

type Reply = std::result::Result<Value, ServerError>;
type Outgoing = Box<dyn FnOnce(&Shared) -> Option<Value> + Send>;

#[derive(Debug, Clone)]
pub enum ServerError {
    Busy,
    Failed(String),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::Busy => f.write_str("the language server is still loading the project"),
            ServerError::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ServerError {}

struct Shared {
    kind: Kind,
    root: PathBuf,
    next_id: AtomicI64,
    pull: AtomicBool,
    pulls: Mutex<HashMap<i64, String>>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<i64, flume::Sender<Reply>>>,
    progress: Mutex<HashSet<String>>,
    stderr_tail: Mutex<Vec<String>>,
    failure: Mutex<Option<String>>,
    exited: AtomicBool,
    stopping: AtomicBool,
    events: flume::Sender<Event>,
}

impl Shared {
    fn new(kind: Kind, root: PathBuf, events: flume::Sender<Event>) -> Shared {
        Shared {
            kind,
            root,
            next_id: AtomicI64::new(1),
            pull: AtomicBool::new(false),
            pulls: Mutex::new(HashMap::new()),
            stdin: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            progress: Mutex::new(HashSet::new()),
            stderr_tail: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
            exited: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            events,
        }
    }

    fn write(&self, message: &Value) -> std::io::Result<()> {
        let body = message.to_string();
        let mut stdin = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        let Some(stdin) = stdin.as_mut() else {
            return Err(std::io::Error::other("language server is not running"));
        };
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len())?;
        stdin.flush()
    }

    fn exit_reason(&self) -> String {
        if let Some(failure) = self.failure.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return failure;
        }
        let name = self.kind.server_name();
        let tail = self.stderr_tail.lock().unwrap_or_else(|e| e.into_inner());
        match tail.last() {
            Some(line) => format!("{name} exited: {line}"),
            None => format!("{name} exited"),
        }
    }

    fn fail(&self, reason: String) {
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
        self.finish();
    }

    fn finish(&self) {
        if self.exited.swap(true, Ordering::SeqCst) {
            return;
        }
        let reason = self.exit_reason();
        for (_, reply) in self.pending.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            let _ = reply.send(Err(ServerError::Failed(reason.clone())));
        }
        if !self.stopping.load(Ordering::SeqCst) {
            let spawn_failed = self.failure.lock().unwrap_or_else(|e| e.into_inner()).is_some();
            let _ = self.events.send(Event::Stopped { kind: self.kind, reason, spawn_failed });
        }
    }
}

fn spawn_process(kind: Kind, root: &Path) -> Result<Child, String> {
    let program = kind.command()?;
    let path_env = std::env::join_paths(format::search_dirs()).unwrap_or_default();
    Command::new(&program)
        .args(kind.spec().args)
        .current_dir(root)
        .env("PATH", path_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Cannot run {}: {err}", kind.server_name()))
}

fn initialize_request(kind: Kind, root: &Path) -> Value {
    let mut request = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "rootUri": path_to_uri(root),
            "workspaceFolders": [{
                "uri": path_to_uri(root),
                "name": root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            }],
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                "textDocument": {
                    "synchronization": { "dynamicRegistration": false, "didSave": true },
                    "definition": { "dynamicRegistration": false, "linkSupport": true },
                    "diagnostic": { "dynamicRegistration": false, "relatedDocumentSupport": true },
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "versionSupport": true,
                        "codeDescriptionSupport": true,
                        "tagSupport": { "valueSet": [1, 2] },
                    },
                },
                "workspace": {
                    "configuration": true,
                    "workspaceFolders": true,
                    "didChangeConfiguration": { "dynamicRegistration": false },
                    "diagnostics": { "refreshSupport": false },
                },
                "window": { "workDoneProgress": true },
            },
            "initializationOptions": (kind.spec().init_options)(root),
        },
    });
    if !kind.spec().pull_diagnostics {
        if let Some(text_document) = request.pointer_mut("/params/capabilities/textDocument").and_then(Value::as_object_mut) {
            text_document.remove("diagnostic");
        }
    }
    request
}

fn boot(shared: Arc<Shared>, root: PathBuf, slot: Arc<Mutex<Option<Child>>>, queue: flume::Receiver<Outgoing>) {
    let kind = shared.kind;
    let mut child = match spawn_process(kind, &root) {
        Ok(child) => child,
        Err(reason) => return shared.fail(reason),
    };
    let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        let _ = child.wait();
        return shared.fail(format!("Cannot talk to {}", kind.server_name()));
    };
    {
        let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
        if shared.stopping.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        *slot = Some(child);
    }
    *shared.stdin.lock().unwrap_or_else(|e| e.into_inner()) = Some(stdin);

    let tail = shared.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
            let mut kept = tail.stderr_tail.lock().unwrap_or_else(|e| e.into_inner());
            kept.push(line);
            if kept.len() > STDERR_TAIL {
                kept.remove(0);
            }
        }
    });

    let (init_reply, init_done) = flume::bounded(1);
    shared.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(0, init_reply);
    let reader = shared.clone();
    std::thread::spawn(move || read_loop(&reader, stdout));

    if shared.write(&initialize_request(kind, &root)).is_err() {
        return;
    }
    match init_done.recv_timeout(INIT_TIMEOUT) {
        Ok(Ok(result)) => {
            let pull = kind.spec().pull_diagnostics && result.pointer("/capabilities/diagnosticProvider").is_some_and(|v| !v.is_null());
            shared.pull.store(pull, Ordering::SeqCst);
        }
        Ok(Err(_)) | Err(_) => {
            shared.finish();
            return;
        }
    }
    let initialized = json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} });
    if shared.write(&initialized).is_err() {
        return;
    }
    let settings = (kind.spec().settings)();
    if settings.as_object().is_some_and(|o| !o.is_empty()) {
        let changed = json!({ "jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": { "settings": settings } });
        if shared.write(&changed).is_err() {
            return;
        }
    }
    while let Ok(build) = queue.recv() {
        if let Some(message) = build(&shared) {
            if shared.write(&message).is_err() {
                break;
            }
        }
    }
}

fn read_loop(shared: &Shared, stdout: ChildStdout) {
    let mut stdout = BufReader::new(stdout);
    while let Some(message) = read_message(&mut stdout) {
        dispatch(shared, message);
    }
    shared.finish();
}

pub struct Server {
    kind: Kind,
    shared: Arc<Shared>,
    outgoing: flume::Sender<Outgoing>,
    open: RefCell<HashMap<String, i32>>,
    last_used: Cell<Instant>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Server {
    fn start(kind: Kind, root: &Path, events: flume::Sender<Event>) -> Server {
        let shared = Arc::new(Shared::new(kind, root.to_path_buf(), events));
        let child = Arc::new(Mutex::new(None));
        let (outgoing, queue) = flume::unbounded::<Outgoing>();
        let (boot_shared, boot_root, boot_slot) = (shared.clone(), root.to_path_buf(), child.clone());
        std::thread::spawn(move || boot(boot_shared, boot_root, boot_slot, queue));
        Server {
            kind,
            shared,
            outgoing,
            open: RefCell::new(HashMap::new()),
            last_used: Cell::new(Instant::now()),
            child,
        }
    }

    fn alive(&self) -> bool {
        !self.shared.exited.load(Ordering::SeqCst)
    }

    pub fn indexing(&self) -> bool {
        !self.shared.progress.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    fn notify_with(&self, method: &'static str, params: impl FnOnce() -> Value + Send + 'static) {
        let _ = self
            .outgoing
            .send(Box::new(move |_| Some(json!({ "jsonrpc": "2.0", "method": method, "params": params() }))));
    }

    fn pull(&self, uri: &str) {
        let uri = uri.to_owned();
        let _ = self.outgoing.send(Box::new(move |shared| {
            if !shared.pull.load(Ordering::SeqCst) {
                return None;
            }
            let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
            shared.pulls.lock().unwrap_or_else(|e| e.into_inner()).insert(id, uri.clone());
            Some(json!({
                "jsonrpc": "2.0", "id": id, "method": "textDocument/diagnostic",
                "params": { "textDocument": { "uri": uri } },
            }))
        }));
    }

    fn request(&self, method: &str, params: Value) -> flume::Receiver<Reply> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, receiver) = flume::bounded(1);
        if !self.alive() {
            let _ = reply.send(Err(ServerError::Failed(self.shared.exit_reason())));
            return receiver;
        }
        self.shared.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, reply);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let _ = self.outgoing.send(Box::new(move |_| Some(message)));
        receiver
    }

    fn sync(&self, uri: &str, language_id: &'static str, stale: bool, text: &Rope) {
        let mut open = self.open.borrow_mut();
        let text = text.clone();
        let uri_owned = uri.to_owned();
        match open.get_mut(uri) {
            None => {
                open.insert(uri.to_owned(), 1);
                self.notify_with("textDocument/didOpen", move || {
                    json!({ "textDocument": {
                        "uri": uri_owned, "languageId": language_id, "version": 1, "text": text.to_string(),
                    }})
                });
                self.pull(uri);
            }
            Some(version) if stale => {
                *version += 1;
                let version = *version;
                self.notify_with("textDocument/didChange", move || {
                    json!({
                        "textDocument": { "uri": uri_owned, "version": version },
                        "contentChanges": [{ "text": text.to_string() }],
                    })
                });
                self.pull(uri);
            }
            Some(_) => {}
        }
    }

    fn saved(&self, uri: &str) {
        if self.open.borrow().contains_key(uri) {
            let uri = uri.to_owned();
            let pull_uri = uri.clone();
            self.notify_with("textDocument/didSave", move || json!({ "textDocument": { "uri": uri } }));
            self.pull(&pull_uri);
        }
    }

    fn close(&self, uri: &str) {
        if self.open.borrow_mut().remove(uri).is_some() {
            let uri = uri.to_owned();
            self.notify_with("textDocument/didClose", move || json!({ "textDocument": { "uri": uri } }));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut child) = child {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn dispatch(shared: &Shared, mut message: Value) {
    let method = message.get("method").and_then(Value::as_str).map(str::to_owned);
    let id = message.get("id").cloned();
    match (method.as_deref(), id) {
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let pulled = shared.pulls.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            if let Some(uri) = pulled {
                if let Some(result) = message.get_mut("result") {
                    publish_pulled(shared, &uri, result.take());
                }
                return;
            }
            let Some(reply) = shared.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) else { return };
            let result = match message.get("error") {
                Some(error) if error.get("code").and_then(Value::as_i64) == Some(CONTENT_MODIFIED) => {
                    Err(ServerError::Busy)
                }
                Some(error) => Err(ServerError::Failed(format!(
                    "{}: {}",
                    shared.kind.server_name(),
                    error.get("message").and_then(Value::as_str).unwrap_or("request failed")
                ))),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = reply.send(result);
        }
        (Some(method), Some(id)) => {
            let result = match method {
                "workspace/configuration" => {
                    let items = message.pointer("/params/items").and_then(Value::as_array).cloned().unwrap_or_default();
                    configuration_reply(shared.kind, &items)
                }
                "workspace/workspaceFolders" => json!([{
                    "uri": path_to_uri(&shared.root),
                    "name": shared.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                }]),
                "workspace/applyEdit" => json!({ "applied": false }),
                "client/registerCapability" | "client/unregisterCapability" | "window/workDoneProgress/create" => {
                    Value::Null
                }
                _ => Value::Null,
            };
            let _ = shared.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
        (Some("textDocument/publishDiagnostics"), None) => {
            let params = message.get_mut("params").map(Value::take).unwrap_or(Value::Null);
            let Ok(params) = serde_json::from_value::<PublishDiagnosticsParams>(params) else { return };
            let Some(path) = uri_to_path(params.uri.as_str()) else { return };
            let _ = shared.events.send(Event::Diagnostics { path, diagnostics: params.diagnostics });
        }
        (Some("$/progress"), None) => {
            let token = match message.pointer("/params/token") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => return,
            };
            let mut progress = shared.progress.lock().unwrap_or_else(|e| e.into_inner());
            match message.pointer("/params/value/kind").and_then(Value::as_str) {
                Some("begin") => {
                    progress.insert(token);
                }
                Some("end") => {
                    progress.remove(&token);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn section_value(settings: &Value, section: &str) -> Value {
    let mut current = settings;
    for part in section.split('.') {
        match current.get(part) {
            Some(next) => current = next,
            None => return json!({}),
        }
    }
    current.clone()
}

fn configuration_reply(kind: Kind, items: &[Value]) -> Value {
    let settings = (kind.spec().settings)();
    Value::Array(
        items
            .iter()
            .map(|item| match item.get("section").and_then(Value::as_str) {
                Some(section) if !section.is_empty() => section_value(&settings, section),
                _ => settings.clone(),
            })
            .collect(),
    )
}

fn publish_pulled(shared: &Shared, uri: &str, report: Value) {
    let Some(path) = uri_to_path(uri) else { return };
    if report.get("kind").and_then(Value::as_str) == Some("full") {
        let items = report.get("items").cloned().unwrap_or(Value::Array(vec![]));
        if let Ok(diagnostics) = serde_json::from_value::<Vec<lsp_types::Diagnostic>>(items) {
            let _ = shared.events.send(Event::Diagnostics { path, diagnostics });
        }
    }
    if let Some(related) = report.get("relatedDocuments").and_then(Value::as_object) {
        for (related_uri, related_report) in related {
            if related_uri != uri {
                publish_pulled(shared, related_uri, related_report.clone());
            }
        }
    }
}

fn into_links(response: Option<GotoDefinitionResponse>) -> Vec<LocationLink> {
    let location = |l: lsp_types::Location| LocationLink {
        origin_selection_range: None,
        target_uri: l.uri,
        target_range: l.range,
        target_selection_range: l.range,
    };
    match response {
        None => vec![],
        Some(GotoDefinitionResponse::Scalar(l)) => vec![location(l)],
        Some(GotoDefinitionResponse::Array(ls)) => ls.into_iter().map(location).collect(),
        Some(GotoDefinitionResponse::Link(links)) => links,
    }
}

pub struct Manager {
    project: RefCell<Option<PathBuf>>,
    servers: RefCell<HashMap<(Kind, PathBuf), Rc<Server>>>,
    failed: RefCell<HashMap<Kind, String>>,
    crashes: RefCell<HashMap<Kind, u32>>,
    noticed: RefCell<HashSet<Kind>>,
    last_root: RefCell<HashMap<Kind, PathBuf>>,
    reaper: RefCell<Option<Task<()>>>,
    builtin: builtin_lang::Engine,
    events: flume::Sender<Event>,
    receiver: flume::Receiver<Event>,
}

impl Default for Manager {
    fn default() -> Self {
        let (events, receiver) = flume::unbounded();
        Manager {
            project: RefCell::default(),
            servers: RefCell::default(),
            failed: RefCell::default(),
            crashes: RefCell::default(),
            noticed: RefCell::default(),
            last_root: RefCell::default(),
            reaper: RefCell::default(),
            builtin: builtin_lang::Engine::new(events.clone()),
            events,
            receiver,
        }
    }
}

impl Manager {
    pub fn events(&self) -> flume::Receiver<Event> {
        self.receiver.clone()
    }

    pub fn set_project(&self, root: Option<PathBuf>) {
        *self.project.borrow_mut() = root;
        self.servers.borrow_mut().clear();
        self.failed.borrow_mut().clear();
        self.crashes.borrow_mut().clear();
        self.noticed.borrow_mut().clear();
        self.last_root.borrow_mut().clear();
        self.builtin.reset();
        self.receiver.drain().for_each(drop);
    }

    pub fn is_external(&self, path: &Path) -> bool {
        self.project.borrow().as_ref().is_some_and(|root| !path.starts_with(root))
    }

    fn server(self: &Rc<Self>, kind: Kind, path: &Path, cx: &mut App) -> Result<Rc<Server>, String> {
        if let Some(reason) = self.failed.borrow().get(&kind) {
            return Err(reason.clone());
        }
        let project = self.project.borrow().clone();
        let root = project
            .as_deref()
            .and_then(|project| root_for(kind, project, path))
            .or_else(|| self.last_root.borrow().get(&kind).cloned())
            .or_else(|| path.ancestors().skip(1).find(|d| kind.has_marker(d)).map(Path::to_path_buf))
            .or_else(|| if kind.spec().markers.is_empty() { path.parent().map(Path::to_path_buf) } else { None })
            .ok_or_else(|| format!("No {} project for this file", kind.server_name()))?;

        let key = (kind, root.clone());
        let existing = self.servers.borrow().get(&key).filter(|s| s.alive()).cloned();
        let server = match existing {
            Some(server) => server,
            None => {
                let server = Rc::new(Server::start(kind, &root, self.events.clone()));
                self.servers.borrow_mut().insert(key, server.clone());
                server
            }
        };
        server.last_used.set(Instant::now());
        self.last_root.borrow_mut().insert(kind, root);
        self.arm_reaper(cx);
        Ok(server)
    }

    fn arm_reaper(self: &Rc<Self>, cx: &mut App) {
        let manager = Rc::downgrade(self);
        *self.reaper.borrow_mut() = Some(cx.spawn(async move |cx| {
            cx.background_executor().timer(IDLE_SHUTDOWN).await;
            if let Some(manager) = manager.upgrade() {
                manager.servers.borrow_mut().retain(|_, s| s.alive() && s.last_used.get().elapsed() < IDLE_SHUTDOWN);
            }
        }));
    }

    fn server_kind(&self, kind: Option<Kind>) -> Option<Kind> {
        kind.filter(|kind| !self.failed.borrow().contains_key(kind))
    }

    fn uses_builtin(&self, kind: Option<Kind>, language: &str) -> bool {
        builtin_lang::covers(language, kind.is_some_and(|kind| self.server_kind(Some(kind)).is_none()))
    }

    pub fn sync(
        self: &Rc<Self>,
        kind: Option<Kind>,
        language: &'static str,
        path: &Path,
        stale: bool,
        text: &Rope,
        cx: &mut App,
    ) {
        if let Some(kind) = self.server_kind(kind) {
            if let Ok(server) = self.server(kind, path, cx) {
                server.sync(&path_to_uri(path), kind.language_id(language, path), stale, text);
            }
        } else if self.uses_builtin(kind, language) {
            self.builtin.sync(language, path, stale, text.clone());
        }
    }

    pub fn resync_kind(&self, kind: Kind) -> bool {
        self.failed.borrow().contains_key(&kind)
    }

    pub fn saved(&self, path: &Path) {
        let uri = path_to_uri(path);
        for server in self.servers.borrow().values() {
            server.saved(&uri);
        }
    }

    pub fn close(&self, path: &Path) {
        self.builtin.forget(path);
        let uri = path_to_uri(path);
        for server in self.servers.borrow().values() {
            server.close(&uri);
        }
    }

    pub fn stopped(&self, kind: Kind, reason: String, spawn_failed: bool) -> Option<String> {
        if spawn_failed {
            self.failed.borrow_mut().insert(kind, reason.clone());
            if kind.optional() {
                return None;
            }
        } else {
            let mut crashes = self.crashes.borrow_mut();
            let count = crashes.entry(kind).or_default();
            *count += 1;
            if *count >= MAX_CRASHES {
                self.failed.borrow_mut().insert(kind, format!("{} keeps crashing: {reason}", kind.server_name()));
            }
        }
        self.noticed.borrow_mut().insert(kind).then_some(reason)
    }

    pub fn indexing(&self, kind: Kind) -> bool {
        self.servers.borrow().values().any(|s| s.kind == kind && s.alive() && s.indexing())
    }
}

pub fn definitions_for(manager: Weak<Manager>, language: &'static str, path: &Path) -> Option<Rc<Definitions>> {
    let kind = Kind::for_file(language, path);
    if kind.is_none() && !builtin_lang::covers(language, false) && !builtin_lang::indexes(language) {
        return None;
    }
    Some(Rc::new(Definitions {
        manager,
        kind,
        language,
        path: path.to_path_buf(),
        stale: Rc::new(Cell::new(false)),
    }))
}

impl Manager {
    fn builtin_definitions(
        &self,
        request: &Definitions,
        text: &Rope,
        offset: usize,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        self.builtin.sync(request.language, &request.path, request.stale.replace(false), text.clone());
        let at = text.offset_to_position(offset);
        let line = line_text(text, at.line);
        let Some((word, begin)) = builtin_lang::word_at(&line, at.character as usize) else {
            return Task::ready(Ok(vec![]));
        };
        let end = begin + word.chars().count();
        let origin = Range::new(Position::new(at.line, begin as u32), Position::new(at.line, end as u32));
        let root = self
            .project
            .borrow()
            .clone()
            .or_else(|| request.path.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let reply = self.builtin.index.lookup(root, word, request.path.clone(), request.language.to_owned());
        cx.spawn(async move |_| {
            let found = reply.recv_async().await.map_err(|_| anyhow!("the symbol index was reset"))?;
            Ok(found.iter().filter_map(|loc| builtin_lang::link(origin, loc)).collect())
        })
    }
}

pub struct Definitions {
    pub manager: Weak<Manager>,
    pub kind: Option<Kind>,
    pub language: &'static str,
    pub path: PathBuf,
    pub stale: Rc<Cell<bool>>,
}

impl DefinitionProvider for Definitions {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let Some(manager) = self.manager.upgrade() else { return Task::ready(Ok(vec![])) };
        let Some(kind) = manager.server_kind(self.kind) else {
            return manager.builtin_definitions(self, text, offset, cx);
        };
        let server = match manager.server(kind, &self.path, cx) {
            Ok(server) => server,
            Err(reason) => return Task::ready(Err(anyhow!(reason))),
        };
        let uri = path_to_uri(&self.path);
        server.sync(&uri, kind.language_id(self.language, &self.path), self.stale.replace(false), text);
        let reply = server.request(
            "textDocument/definition",
            json!({ "textDocument": { "uri": uri }, "position": to_lsp(text, offset) }),
        );
        let text = text.clone();
        cx.spawn(async move |_| {
            let value = reply.recv_async().await.map_err(|_| anyhow!("language server stopped"))??;
            let response = serde_json::from_value::<Option<GotoDefinitionResponse>>(value)?;
            let mut links = into_links(response);
            for link in &mut links {
                if let Some(range) = link.origin_selection_range.as_mut() {
                    *range = Range::new(from_lsp(&text, range.start), from_lsp(&text, range.end));
                }
            }
            Ok(links)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_with_escapes() {
        let path = Path::new("/Users/me/My Project/é#1.swift");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///Users/me/My%20Project/%C3%A9%231.swift");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        assert_eq!(uri_to_path("file://localhost/tmp/a.rs").as_deref(), Some(Path::new("/tmp/a.rs")));
        assert_eq!(uri_to_path("https://example.com/a"), None);
    }

    #[test]
    fn columns_convert_between_chars_and_utf16() {
        let line = "let 😀 = x";
        assert_eq!(utf16_column(line, 5), 6);
        assert_eq!(char_column(line, 6), 5);
        assert_eq!(char_column(line, 100), line.chars().count() as u32);
        assert_eq!(utf16_column("abc", 2), 2);
    }

    #[test]
    fn roots_follow_manifests() {
        let dir = std::env::temp_dir().join(format!("forge-lsp-roots-{}", std::process::id()));
        let crate_dir = dir.join("crates/a/src");
        let pkg = dir.join("Pkg/Sources/X");
        std::fs::create_dir_all(&crate_dir).unwrap();
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.join("crates/a/Cargo.toml"), "").unwrap();
        std::fs::write(dir.join("Pkg/Package.swift"), "").unwrap();

        assert_eq!(root_for(Kind::Rust, &dir, &crate_dir.join("lib.rs")), Some(dir.clone()));
        assert_eq!(root_for(Kind::Swift, &dir, &pkg.join("x.swift")), Some(dir.join("Pkg")));
        assert_eq!(root_for(Kind::Swift, &dir, &dir.join("App/a.swift")), Some(dir.clone()));
        assert_eq!(root_for(Kind::Rust, &dir, Path::new("/elsewhere/lib.rs")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn languages_map_to_server_specs() {
        assert_eq!(Kind::for_file("swift", Path::new("x")), Some(Kind::Swift));
        assert_eq!(Kind::for_file("rust", Path::new("x")), Some(Kind::Rust));
        assert_eq!(Kind::for_file("markdown", Path::new("x")), Some(Kind::Markdown));
        assert_eq!(Kind::for_file("groovy", Path::new("x")), None);
        assert_eq!(Kind::for_file("java", Path::new("x")), None);
        for spec in SPECS {
            assert_eq!(spec.kind.spec().server_name, spec.server_name);
            assert!(!spec.binary.is_empty() && !spec.missing.is_empty() && !spec.label.is_empty());
            assert!(!spec.languages.is_empty());
        }
        let kinds: HashSet<Kind> = SPECS.iter().map(|s| s.kind).collect();
        assert_eq!(kinds.len(), SPECS.len());
        let languages: Vec<&str> = SPECS.iter().flat_map(|s| s.languages.iter().copied()).collect();
        assert_eq!(languages.iter().collect::<HashSet<_>>().len(), languages.len());
    }

    #[test]
    fn language_ids_follow_file_types() {
        let id = |kind: Kind, language: &str, name: &str| kind.language_id(language, Path::new(name));
        assert_eq!(id(Kind::TypeScript, "javascript", "/p/a.js"), "javascript");
        assert_eq!(id(Kind::TypeScript, "javascript", "/p/a.mjs"), "javascript");
        assert_eq!(id(Kind::TypeScript, "javascript", "/p/a.jsx"), "javascriptreact");
        assert_eq!(id(Kind::TypeScript, "typescript", "/p/a.ts"), "typescript");
        assert_eq!(id(Kind::TypeScript, "tsx", "/p/a.tsx"), "typescriptreact");
        assert_eq!(id(Kind::Json, "json", "/p/package.json"), "json");
        assert_eq!(id(Kind::Json, "json", "/p/tsconfig.json"), "jsonc");
        assert_eq!(id(Kind::Json, "json", "/p/a.jsonc"), "jsonc");
        assert_eq!(id(Kind::Css, "css", "/p/a.scss"), "scss");
        assert_eq!(id(Kind::Css, "css", "/p/a.less"), "less");
        assert_eq!(id(Kind::Css, "css", "/p/a.css"), "css");
        assert_eq!(id(Kind::Clangd, "c", "/p/a.c"), "c");
        assert_eq!(id(Kind::Clangd, "cpp", "/p/a.cc"), "cpp");
        assert_eq!(id(Kind::Clangd, "objc", "/p/a.m"), "objective-c");
        assert_eq!(id(Kind::Clangd, "objc", "/p/a.mm"), "objective-cpp");
        assert_eq!(id(Kind::Shell, "bash", "/p/a.sh"), "shellscript");
        assert_eq!(id(Kind::Kotlin, "kotlin", "/p/build.gradle.kts"), "kotlin");
        assert_eq!(id(Kind::Markdown, "markdown", "/p/README.md"), "markdown");
    }

    #[test]
    fn files_pick_servers_and_skip_lookalikes() {
        let kind = |language: &str, name: &str| Kind::for_file(language, Path::new(name));
        assert_eq!(kind("bash", "/p/run.sh"), Some(Kind::Shell));
        assert_eq!(kind("bash", "/p/Dockerfile"), None);
        assert_eq!(kind("bash", "/p/.env"), None);
        assert_eq!(kind("xml", "/p/AndroidManifest.xml"), Some(Kind::Xml));
        assert_eq!(kind("xml", "/p/Info.plist"), None);
        assert_eq!(kind("kotlin", "/p/build.gradle.kts"), Some(Kind::Kotlin));
        assert_eq!(kind("groovy", "/p/build.gradle"), None);
        assert_eq!(kind("tsx", "/p/a.tsx"), Some(Kind::TypeScript));
    }

    #[test]
    fn support_summary_lists_every_label() {
        let summary = supported_languages();
        assert!(summary.starts_with("Swift, Rust, "));
        assert!(summary.contains("Kotlin") && summary.contains("JavaScript/TypeScript"));
        assert!(summary.ends_with(" and Groovy"));
    }

    #[test]
    fn roots_for_new_languages() {
        let dir = std::env::temp_dir().join(format!("forge-lsp-roots2-{}", std::process::id()));
        let go = dir.join("svc/internal");
        let kt = dir.join("android/app/src");
        let web = dir.join("web/src");
        for d in [&go, &kt, &web] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(dir.join("svc/go.mod"), "").unwrap();
        std::fs::write(dir.join("android/settings.gradle.kts"), "").unwrap();
        std::fs::write(dir.join("android/app/settings.gradle"), "").unwrap();
        std::fs::write(dir.join("web/package.json"), "").unwrap();
        std::fs::write(dir.join("compile_commands.json"), "").unwrap();
        assert_eq!(root_for(Kind::Go, &dir, &go.join("a.go")), Some(dir.join("svc")));
        assert_eq!(root_for(Kind::Kotlin, &dir, &kt.join("A.kt")), Some(dir.join("android")));
        assert_eq!(root_for(Kind::TypeScript, &dir, &web.join("a.ts")), Some(dir.join("web")));
        assert_eq!(root_for(Kind::Clangd, &dir, &web.join("a.c")), Some(dir.clone()));
        assert_eq!(root_for(Kind::Yaml, &dir, &web.join("a.yaml")), Some(dir.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configuration_requests_get_section_defaults() {
        let items = [json!({ "section": "python.analysis" }), json!({ "section": "yaml" }), json!({ "section": "nope" }), json!({})];
        let python = configuration_reply(Kind::Python, &items);
        assert_eq!(python[0]["diagnosticMode"], "openFilesOnly");
        assert_eq!(python[2], json!({}));
        let yaml = configuration_reply(Kind::Yaml, &items);
        assert_eq!(yaml[1]["validate"], true);
        assert!(yaml[3]["yaml"].is_object());
        let json_cfg = configuration_reply(Kind::Json, &[json!({ "section": "json" })]);
        assert!(json_cfg[0]["schemas"].as_array().is_some_and(|s| !s.is_empty()));
    }

    #[test]
    fn pull_capability_only_advertised_when_wanted() {
        let on = initialize_request(Kind::Json, Path::new("/p"));
        assert!(on.pointer("/params/capabilities/textDocument/diagnostic").is_some());
        let off = initialize_request(Kind::Rust, Path::new("/p"));
        assert!(off.pointer("/params/capabilities/textDocument/diagnostic").is_none());
    }

    #[test]
    fn pulled_reports_become_events() {
        let (events, receiver) = flume::unbounded();
        let shared = Shared::new(Kind::Json, PathBuf::new(), events);
        shared.pulls.lock().unwrap().insert(7, "file:///tmp/a.json".into());
        dispatch(
            &shared,
            json!({ "jsonrpc": "2.0", "id": 7, "result": { "kind": "full", "items": [{
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
                "message": "bad",
            }], "relatedDocuments": { "file:///tmp/b.json": { "kind": "full", "items": [] } } } }),
        );
        let mut seen = vec![];
        while let Ok(Event::Diagnostics { path, diagnostics }) = receiver.try_recv() {
            seen.push((path, diagnostics.len()));
        }
        assert_eq!(seen, vec![(PathBuf::from("/tmp/a.json"), 1), (PathBuf::from("/tmp/b.json"), 0)]);
        dispatch(&shared, json!({ "jsonrpc": "2.0", "id": 8, "result": { "kind": "unchanged", "resultId": "x" } }));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn reads_framed_messages() {
        let body = r#"{"jsonrpc":"2.0","id":3,"result":null}"#;
        let raw = format!("Content-Length: {}\r\nContent-Type: x\r\n\r\n{body}", body.len());
        let message = read_message(&mut raw.as_bytes()).unwrap();
        assert_eq!(message["id"], 3);
        assert!(read_message(&mut "".as_bytes()).is_none());
    }

    #[test]
    fn definition_responses_become_links() {
        let array = json!([{ "uri": "file:///a.swift", "range": {
            "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 4 } } }]);
        let links = into_links(serde_json::from_value(array).unwrap());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_selection_range.start, Position::new(1, 2));
        assert!(into_links(serde_json::from_value(Value::Null).unwrap()).is_empty());
    }

    #[test]
    fn published_diagnostics_become_events() {
        let (events, receiver) = flume::unbounded();
        let shared = Shared::new(Kind::Rust, PathBuf::new(), events);
        dispatch(
            &shared,
            json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": { "uri": "file:///tmp/a%20b.rs", "version": 4, "diagnostics": [{
                    "range": { "start": { "line": 2, "character": 1 }, "end": { "line": 2, "character": 5 } },
                    "severity": 1, "message": "mismatched types",
                }]},
            }),
        );
        match receiver.try_recv().unwrap() {
            Event::Diagnostics { path, diagnostics } => {
                assert_eq!(path, Path::new("/tmp/a b.rs"));
                assert_eq!(diagnostics[0].message, "mismatched types");
            }
            Event::Stopped { .. } => panic!("expected diagnostics"),
        }
    }

    #[test]
    fn failures_are_reported_once_and_not_after_stop() {
        let (events, receiver) = flume::unbounded();
        let shared = Shared::new(Kind::Swift, PathBuf::new(), events);
        shared.fail("missing".into());
        shared.finish();
        assert!(matches!(receiver.try_recv(), Ok(Event::Stopped { spawn_failed: true, .. })));
        assert!(receiver.try_recv().is_err());

        let (events, receiver) = flume::unbounded();
        let shared = Shared::new(Kind::Swift, PathBuf::new(), events);
        shared.stopping.store(true, Ordering::SeqCst);
        shared.finish();
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn manager_notices_each_kind_once() {
        let manager = Manager::default();
        assert!(manager.stopped(Kind::Rust, "crash".into(), false).is_some());
        assert!(manager.stopped(Kind::Rust, "crash".into(), false).is_none());
        assert!(!manager.failed.borrow().contains_key(&Kind::Rust));
        manager.stopped(Kind::Rust, "crash".into(), false);
        assert!(manager.failed.borrow().contains_key(&Kind::Rust));
        assert!(manager.stopped(Kind::Swift, "missing".into(), true).is_some());
        assert!(manager.failed.borrow().contains_key(&Kind::Swift));
    }

    fn wait_for_errors(receiver: &flume::Receiver<Event>, path: &Path) -> Vec<lsp_types::Diagnostic> {
        let deadline = Instant::now() + Duration::from_secs(180);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match receiver.recv_timeout(left) {
                Ok(Event::Diagnostics { path: got, diagnostics }) if got == path => {
                    if diagnostics.iter().any(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)) {
                        return diagnostics;
                    }
                }
                Ok(Event::Stopped { reason, .. }) => panic!("{reason}"),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        panic!("no error diagnostics for {}", path.display());
    }

    #[test]
    #[ignore]
    fn sourcekit_finds_foundation_interface() {
        let dir = std::env::temp_dir().join(format!("forge-lsp-swift-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.swift");
        let text = "import Foundation\n";
        std::fs::write(&file, text).unwrap();
        let (events, _receiver) = flume::unbounded();
        let server = Server::start(Kind::Swift, &dir, events);
        let uri = path_to_uri(&file);
        server.sync(&uri, "swift", false, &Rope::from(text));
        let reply = server.request(
            "textDocument/definition",
            json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 9 } }),
        );
        let value = reply.recv_timeout(Duration::from_secs(60)).unwrap().unwrap();
        let links = into_links(serde_json::from_value(value).unwrap());
        assert!(links[0].target_uri.as_str().ends_with("Foundation.swiftinterface"), "{links:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore]
    fn sourcekit_publishes_diagnostics_after_change() {
        let dir = std::env::temp_dir().join(format!("forge-lsp-swift-diag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.swift");
        std::fs::write(&file, "let x = 1\n").unwrap();
        let (events, receiver) = flume::unbounded();
        let server = Server::start(Kind::Swift, &dir, events);
        let uri = path_to_uri(&file);
        server.sync(&uri, "swift", false, &Rope::from("let x = 1\n"));
        server.sync(&uri, "swift", true, &Rope::from("let x: Int = \"hello\"\n"));
        let diagnostics = wait_for_errors(&receiver, &file);
        assert_eq!(diagnostics[0].range.start.line, 0);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore]
    fn rust_analyzer_publishes_diagnostics() {
        let dir = std::env::temp_dir().join(format!("forge-lsp-rust-diag-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
            .unwrap();
        let file = dir.join("src/main.rs");
        let text = "fn main() { let x: u32 = \"no\"; }\n";
        std::fs::write(&file, text).unwrap();
        let (events, receiver) = flume::unbounded();
        let server = Server::start(Kind::Rust, &dir, events);
        let uri = path_to_uri(&file);
        server.sync(&uri, "rust", false, &Rope::from(text));
        server.saved(&uri);
        wait_for_errors(&receiver, &file);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn smoke(kind: Kind, files: &[(&str, &str)], open: &str, secs: u64) {
        smoke_with(kind, files, open, secs, &|_| {});
    }

    fn smoke_with(kind: Kind, files: &[(&str, &str)], open: &str, secs: u64, prepare: &dyn Fn(&Path)) {
        if kind.command().is_err() {
            eprintln!("SKIP {}: not installed", kind.server_name());
            return;
        }
        let dir = std::env::temp_dir().join(format!("forge-lsp-smoke-{:?}-{}", kind, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let dir = dir.canonicalize().unwrap();
        prepare(&dir);
        let file = dir.join(open);
        let text = std::fs::read_to_string(&file).unwrap();
        let (events, receiver) = flume::unbounded();
        let server = Server::start(kind, &dir, events);
        let uri = path_to_uri(&file);
        let language = crate::files::language_for(&file);
        server.sync(&uri, kind.language_id(language, &file), false, &Rope::from(text.as_str()));
        server.saved(&uri);
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut found = None;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match receiver.recv_timeout(left) {
                Ok(Event::Diagnostics { path, diagnostics }) if path == file && !diagnostics.is_empty() => {
                    found = Some(diagnostics);
                    break;
                }
                Ok(Event::Stopped { reason, .. }) => panic!("{reason}"),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
        let diagnostics = found.unwrap_or_else(|| panic!("{} produced no diagnostics", kind.server_name()));
        eprintln!("{}: {}", kind.server_name(), diagnostics[0].message);
    }

    fn smoke_symbols(kind: Kind, files: &[(&str, &str)], open: &str) {
        if kind.command().is_err() {
            eprintln!("SKIP {}: not installed", kind.server_name());
            return;
        }
        let dir = std::env::temp_dir().join(format!("forge-lsp-symbols-{:?}-{}", kind, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let dir = dir.canonicalize().unwrap();
        let file = dir.join(open);
        let text = std::fs::read_to_string(&file).unwrap();
        let (events, _receiver) = flume::unbounded();
        let server = Server::start(kind, &dir, events);
        let uri = path_to_uri(&file);
        let language = crate::files::language_for(&file);
        server.sync(&uri, kind.language_id(language, &file), false, &Rope::from(text.as_str()));
        let reply = server.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
        let mut value = reply.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
        for _ in 0..10 {
            if value.as_array().is_some_and(|a| !a.is_empty()) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
            let retry = server.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
            value = retry.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
        }
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(value.as_array().is_some_and(|a| !a.is_empty()), "{} returned no symbols: {value}", kind.server_name());
        eprintln!("{}: {} symbols", kind.server_name(), value.as_array().map_or(0, Vec::len));
    }

    #[test]
    #[ignore]
    fn smoke_clangd() {
        smoke(Kind::Clangd, &[("compile_flags.txt", "-xc\n"), ("main.c", "int main(void) { int x = \"a\"; return y; }\n")], "main.c", 60);
    }

    #[test]
    #[ignore]
    fn smoke_gopls() {
        smoke(Kind::Go, &[("go.mod", "module probe\n\ngo 1.21\n"), ("main.go", "package main\n\nfunc main() {\n\tvar x int = \"a\"\n}\n")], "main.go", 90);
    }

    #[test]
    #[ignore]
    fn smoke_typescript() {
        let cache = std::env::temp_dir().join("forge-lsp-ts5");
        if !cache.join("node_modules/typescript/lib/tsserver.js").is_file() {
            std::fs::create_dir_all(&cache).unwrap();
            let status = Command::new("npm")
                .args(["install", "--silent", "--no-audit", "--no-fund", "--prefix"])
                .arg(&cache)
                .arg("typescript@5")
                .env("PATH", std::env::join_paths(format::search_dirs()).unwrap_or_default())
                .status()
                .unwrap();
            assert!(status.success());
        }
        let link = |dir: &Path| {
            std::os::unix::fs::symlink(cache.join("node_modules"), dir.join("node_modules")).unwrap();
        };
        smoke_with(
            Kind::TypeScript,
            &[("package.json", "{}"), ("tsconfig.json", "{}"), ("a.ts", "const x: number = \"s\";\n")],
            "a.ts",
            90,
            &link,
        );
    }

    #[test]
    #[ignore]
    fn smoke_json() {
        smoke(Kind::Json, &[("a.json", "{ \"a\": }\n")], "a.json", 30);
    }

    #[test]
    #[ignore]
    fn smoke_css() {
        smoke(Kind::Css, &[("a.css", "a { color: ; colr: red }\n")], "a.css", 30);
    }

    #[test]
    #[ignore]
    fn smoke_html() {
        smoke_symbols(Kind::Html, &[("a.html", "<html><body><div id=\"x\"></div></body></html>\n")], "a.html");
    }

    #[test]
    #[ignore]
    fn smoke_yaml() {
        smoke(Kind::Yaml, &[("a.yaml", "a: [1, 2\nb: : :\n")], "a.yaml", 30);
    }

    #[test]
    #[ignore]
    fn smoke_toml() {
        smoke(Kind::Toml, &[("a.toml", "a = = 1\n")], "a.toml", 30);
    }

    #[test]
    #[ignore]
    fn smoke_markdown() {
        smoke(Kind::Markdown, &[(".marksman.toml", ""), ("a.md", "# A\n\n[x](missing.md)\n[y](#nope)\n")], "a.md", 30);
    }

    #[test]
    #[ignore]
    fn smoke_shell() {
        smoke_symbols(Kind::Shell, &[("a.sh", "#!/bin/bash\nfoo() {\n  echo hi\n}\nfoo\n")], "a.sh");
    }

    #[test]
    #[ignore]
    fn smoke_python() {
        smoke(Kind::Python, &[("pyproject.toml", ""), ("a.py", "x: int = \"s\"\nprint(undefined_name)\n")], "a.py", 60);
    }

    #[test]
    #[ignore]
    fn smoke_xml() {
        smoke(Kind::Xml, &[("a.xml", "<a><b></a>\n")], "a.xml", 30);
    }

    #[test]
    #[ignore]
    fn smoke_kotlin_language_server() {
        smoke(
            Kind::Kotlin,
            &[("settings.gradle.kts", "rootProject.name = \"probe\"\n"), ("A.kt", "val x: Int = \"s\"\n")],
            "A.kt",
            180,
        );
    }

    #[test]
    fn optional_servers_fail_silently_and_fall_back() {
        let manager = Manager::default();
        assert!(Kind::Kotlin.optional() && Kind::Xml.optional() && !Kind::Go.optional());
        assert!(manager.stopped(Kind::Kotlin, "missing".into(), true).is_none());
        assert!(manager.stopped(Kind::Go, "missing".into(), true).is_some());
        assert!(manager.uses_builtin(Some(Kind::Kotlin), "kotlin"));
        assert!(manager.uses_builtin(Some(Kind::Go), "go"));
        assert!(manager.uses_builtin(None, "groovy"));
        assert!(!manager.uses_builtin(None, "go"));
        assert!(!manager.uses_builtin(Some(Kind::Rust), "rust"));
    }
}
