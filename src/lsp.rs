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

use crate::format;

const IDLE_SHUTDOWN: Duration = Duration::from_secs(10 * 60);
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
const STDERR_TAIL: usize = 4;
const CONTENT_MODIFIED: i64 = -32801;
const MAX_CRASHES: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Swift,
    Rust,
}

struct Spec {
    kind: Kind,
    languages: &'static [&'static str],
    server_name: &'static str,
    language_id: &'static str,
    markers: &'static [&'static str],
    prefers_topmost: bool,
    binary: &'static str,
    xcrun: bool,
    missing: &'static str,
    init_options: fn() -> Value,
}

const SPECS: &[Spec] = &[
    Spec {
        kind: Kind::Swift,
        languages: &["swift"],
        server_name: "sourcekit-lsp",
        language_id: "swift",
        markers: &["Package.swift"],
        prefers_topmost: false,
        binary: "sourcekit-lsp",
        xcrun: true,
        missing: "sourcekit-lsp not found: install Xcode or a Swift toolchain",
        init_options: || Value::Null,
    },
    Spec {
        kind: Kind::Rust,
        languages: &["rust"],
        server_name: "rust-analyzer",
        language_id: "rust",
        markers: &["Cargo.toml"],
        prefers_topmost: true,
        binary: "rust-analyzer",
        xcrun: false,
        missing: "rust-analyzer not found: run `rustup component add rust-analyzer`",
        init_options: || json!({ "checkOnSave": true, "cargo": { "targetDir": true } }),
    },
];

impl Kind {
    fn spec(self) -> &'static Spec {
        SPECS.iter().find(|spec| spec.kind == self).unwrap_or(&SPECS[0])
    }

    pub fn for_language(language: &str) -> Option<Kind> {
        SPECS.iter().find(|spec| spec.languages.contains(&language)).map(|spec| spec.kind)
    }

    pub fn server_name(self) -> &'static str {
        self.spec().server_name
    }

    fn language_id(self) -> &'static str {
        self.spec().language_id
    }

    fn has_marker(self, dir: &Path) -> bool {
        self.spec().markers.iter().any(|marker| dir.join(marker).is_file())
    }

    fn command(self) -> Result<PathBuf, String> {
        let spec = self.spec();
        format::which(spec.binary)
            .or_else(|| if spec.xcrun { xcrun_find(spec.binary) } else { None })
            .ok_or_else(|| spec.missing.to_owned())
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
type Outgoing = Box<dyn FnOnce() -> Value + Send>;

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
    fn new(kind: Kind, events: flume::Sender<Event>) -> Shared {
        Shared {
            kind,
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
        .current_dir(root)
        .env("PATH", path_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Cannot run {}: {err}", kind.server_name()))
}

fn initialize_request(kind: Kind, root: &Path) -> Value {
    json!({
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
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "versionSupport": true,
                        "codeDescriptionSupport": true,
                        "tagSupport": { "valueSet": [1, 2] },
                    },
                },
                "window": { "workDoneProgress": true },
            },
            "initializationOptions": (kind.spec().init_options)(),
        },
    })
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
        Ok(Ok(_)) => {}
        Ok(Err(_)) | Err(_) => {
            shared.finish();
            return;
        }
    }
    let initialized = json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} });
    if shared.write(&initialized).is_err() {
        return;
    }
    while let Ok(build) = queue.recv() {
        if shared.write(&build()).is_err() {
            break;
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
    next_id: AtomicI64,
    open: RefCell<HashMap<String, i32>>,
    last_used: Cell<Instant>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Server {
    fn start(kind: Kind, root: &Path, events: flume::Sender<Event>) -> Server {
        let shared = Arc::new(Shared::new(kind, events));
        let child = Arc::new(Mutex::new(None));
        let (outgoing, queue) = flume::unbounded::<Outgoing>();
        let (boot_shared, boot_root, boot_slot) = (shared.clone(), root.to_path_buf(), child.clone());
        std::thread::spawn(move || boot(boot_shared, boot_root, boot_slot, queue));
        Server {
            kind,
            shared,
            outgoing,
            next_id: AtomicI64::new(1),
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
            .send(Box::new(move || json!({ "jsonrpc": "2.0", "method": method, "params": params() })));
    }

    fn request(&self, method: &str, params: Value) -> flume::Receiver<Reply> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, receiver) = flume::bounded(1);
        if !self.alive() {
            let _ = reply.send(Err(ServerError::Failed(self.shared.exit_reason())));
            return receiver;
        }
        self.shared.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, reply);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let _ = self.outgoing.send(Box::new(move || message));
        receiver
    }

    fn sync(&self, uri: &str, stale: bool, text: &Rope) {
        let mut open = self.open.borrow_mut();
        let text = text.clone();
        let uri_owned = uri.to_owned();
        match open.get_mut(uri) {
            None => {
                open.insert(uri.to_owned(), 1);
                let language_id = self.kind.language_id();
                self.notify_with("textDocument/didOpen", move || {
                    json!({ "textDocument": {
                        "uri": uri_owned, "languageId": language_id, "version": 1, "text": text.to_string(),
                    }})
                });
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
            }
            Some(_) => {}
        }
    }

    fn saved(&self, uri: &str) {
        if self.open.borrow().contains_key(uri) {
            let uri = uri.to_owned();
            self.notify_with("textDocument/didSave", move || json!({ "textDocument": { "uri": uri } }));
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
                    let items = message.pointer("/params/items").and_then(Value::as_array).map_or(0, Vec::len);
                    Value::Array(vec![Value::Null; items])
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

    pub fn sync(self: &Rc<Self>, kind: Kind, path: &Path, stale: bool, text: &Rope, cx: &mut App) {
        if let Ok(server) = self.server(kind, path, cx) {
            server.sync(&path_to_uri(path), stale, text);
        }
    }

    pub fn saved(&self, path: &Path) {
        let uri = path_to_uri(path);
        for server in self.servers.borrow().values() {
            server.saved(&uri);
        }
    }

    pub fn close(&self, path: &Path) {
        let uri = path_to_uri(path);
        for server in self.servers.borrow().values() {
            server.close(&uri);
        }
    }

    pub fn stopped(&self, kind: Kind, reason: String, spawn_failed: bool) -> Option<String> {
        if spawn_failed {
            self.failed.borrow_mut().insert(kind, reason.clone());
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

pub struct Definitions {
    pub manager: Weak<Manager>,
    pub kind: Kind,
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
        let server = match manager.server(self.kind, &self.path, cx) {
            Ok(server) => server,
            Err(reason) => return Task::ready(Err(anyhow!(reason))),
        };
        let uri = path_to_uri(&self.path);
        server.sync(&uri, self.stale.replace(false), text);
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
        assert_eq!(Kind::for_language("swift"), Some(Kind::Swift));
        assert_eq!(Kind::for_language("rust"), Some(Kind::Rust));
        assert_eq!(Kind::for_language("markdown"), None);
        for spec in SPECS {
            assert_eq!(spec.kind.spec().server_name, spec.server_name);
        }
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
        let shared = Shared::new(Kind::Rust, events);
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
        let shared = Shared::new(Kind::Swift, events);
        shared.fail("missing".into());
        shared.finish();
        assert!(matches!(receiver.try_recv(), Ok(Event::Stopped { spawn_failed: true, .. })));
        assert!(receiver.try_recv().is_err());

        let (events, receiver) = flume::unbounded();
        let shared = Shared::new(Kind::Swift, events);
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
        server.sync(&uri, false, &Rope::from(text));
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
        server.sync(&uri, false, &Rope::from("let x = 1\n"));
        server.sync(&uri, true, &Rope::from("let x: Int = \"hello\"\n"));
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
        server.sync(&uri, false, &Rope::from(text));
        server.saved(&uri);
        wait_for_errors(&receiver, &file);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
