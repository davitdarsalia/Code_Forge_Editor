use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use gpui_kit::component::input::{DefinitionProvider, Rope, RopeExt as _};
use gpui_kit::{App, Task, Window};
use lsp_types::{GotoDefinitionResponse, LocationLink, Position, Range};
use serde_json::{Value, json};

use crate::format;

const IDLE_SHUTDOWN: Duration = Duration::from_secs(10 * 60);
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
const STDERR_TAIL: usize = 4;
const CONTENT_MODIFIED: i64 = -32801;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Swift,
    Rust,
}

impl Kind {
    pub fn for_language(language: &str) -> Option<Kind> {
        match language {
            "swift" => Some(Kind::Swift),
            "rust" => Some(Kind::Rust),
            _ => None,
        }
    }

    pub fn server_name(self) -> &'static str {
        match self {
            Kind::Swift => "sourcekit-lsp",
            Kind::Rust => "rust-analyzer",
        }
    }

    fn language_id(self) -> &'static str {
        match self {
            Kind::Swift => "swift",
            Kind::Rust => "rust",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Kind::Swift => "Package.swift",
            Kind::Rust => "Cargo.toml",
        }
    }

    fn prefers_topmost(self) -> bool {
        self == Kind::Rust
    }

    fn command(self) -> Result<PathBuf, String> {
        let found = match self {
            Kind::Swift => format::which("sourcekit-lsp").or_else(xcode_sourcekit_lsp),
            Kind::Rust => format::which("rust-analyzer"),
        };
        found.ok_or_else(|| match self {
            Kind::Swift => "sourcekit-lsp not found: install Xcode or a Swift toolchain".to_owned(),
            Kind::Rust => "rust-analyzer not found: run `rustup component add rust-analyzer`".to_owned(),
        })
    }

    fn initialization_options(self) -> Value {
        match self {
            Kind::Swift => Value::Null,
            Kind::Rust => json!({ "checkOnSave": false, "cargo": { "targetDir": true } }),
        }
    }
}

fn xcode_sourcekit_lsp() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let out = Command::new("/usr/bin/xcrun").args(["--find", "sourcekit-lsp"]).output().ok()?;
            let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            (out.status.success() && path.is_file()).then_some(path)
        })
        .clone()
}

fn root_for(kind: Kind, project: &Path, path: &Path) -> Option<PathBuf> {
    if !path.starts_with(project) {
        return None;
    }
    let mut manifests = path
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(project))
        .filter(|dir| dir.join(kind.marker()).is_file());
    let found = if kind.prefers_topmost() { manifests.last() } else { manifests.next() };
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

fn line_text(text: &Rope, line: u32) -> String {
    if (line as usize) < text.lines_len() { text.slice_line(line as usize).to_string() } else { String::new() }
}

fn to_lsp(text: &Rope, offset: usize) -> Position {
    let pos = text.offset_to_position(offset);
    Position::new(pos.line, utf16_column(&line_text(text, pos.line), pos.character))
}

fn from_lsp(text: &Rope, pos: Position) -> Position {
    Position::new(pos.line, char_column(&line_text(text, pos.line), pos.character))
}

type Reply = std::result::Result<Value, ServerError>;

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
    stdin: Mutex<ChildStdin>,
    pending: Mutex<HashMap<i64, flume::Sender<Reply>>>,
    progress: Mutex<HashSet<String>>,
    stderr_tail: Mutex<Vec<String>>,
    exited: AtomicBool,
    name: &'static str,
}

impl Shared {
    fn write(&self, message: &Value) -> std::io::Result<()> {
        let body = message.to_string();
        let mut stdin = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len())?;
        stdin.flush()
    }

    fn exit_reason(&self) -> String {
        let tail = self.stderr_tail.lock().unwrap_or_else(|e| e.into_inner());
        match tail.last() {
            Some(line) => format!("{} exited: {line}", self.name),
            None => format!("{} exited", self.name),
        }
    }

    fn fail_pending(&self) {
        self.exited.store(true, Ordering::SeqCst);
        let reason = self.exit_reason();
        for (_, reply) in self.pending.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            let _ = reply.send(Err(ServerError::Failed(reason.clone())));
        }
    }
}

pub struct Server {
    kind: Kind,
    shared: Arc<Shared>,
    outgoing: flume::Sender<Value>,
    next_id: AtomicI64,
    open: RefCell<HashMap<String, i32>>,
    last_used: Cell<Instant>,
    child: Child,
}

impl Server {
    fn start(kind: Kind, root: &Path) -> Result<Server, String> {
        let program = kind.command()?;
        let path_env = std::env::join_paths(format::search_dirs()).unwrap_or_default();
        let mut child = Command::new(&program)
            .current_dir(root)
            .env("PATH", path_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("Cannot run {}: {err}", kind.server_name()))?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err(format!("Cannot talk to {}", kind.server_name()));
        };

        let shared = Arc::new(Shared {
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            progress: Mutex::new(HashSet::new()),
            stderr_tail: Mutex::new(Vec::new()),
            exited: AtomicBool::new(false),
            name: kind.server_name(),
        });

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

        let reader = shared.clone();
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            while let Some(message) = read_message(&mut stdout) {
                dispatch(&reader, message);
            }
            reader.fail_pending();
        });

        let (outgoing, queue) = flume::unbounded::<Value>();
        let (init_reply, init_done) = flume::bounded(1);
        shared.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(0, init_reply);
        let init = json!({
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
                        "synchronization": { "dynamicRegistration": false },
                        "definition": { "dynamicRegistration": false, "linkSupport": true },
                    },
                    "window": { "workDoneProgress": true },
                },
                "initializationOptions": kind.initialization_options(),
            },
        });
        let writer = shared.clone();
        std::thread::spawn(move || {
            if writer.write(&init).is_err() {
                return;
            }
            match init_done.recv_timeout(INIT_TIMEOUT) {
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => {
                    writer.fail_pending();
                    return;
                }
            }
            let initialized = json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} });
            if writer.write(&initialized).is_err() {
                return;
            }
            while let Ok(message) = queue.recv() {
                if writer.write(&message).is_err() {
                    break;
                }
            }
        });

        Ok(Server {
            kind,
            shared,
            outgoing,
            next_id: AtomicI64::new(1),
            open: RefCell::new(HashMap::new()),
            last_used: Cell::new(Instant::now()),
            child,
        })
    }

    fn alive(&self) -> bool {
        !self.shared.exited.load(Ordering::SeqCst)
    }

    pub fn indexing(&self) -> bool {
        !self.shared.progress.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self.outgoing.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn request(&self, method: &str, params: Value) -> flume::Receiver<Reply> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, receiver) = flume::bounded(1);
        if !self.alive() {
            let _ = reply.send(Err(ServerError::Failed(self.shared.exit_reason())));
            return receiver;
        }
        self.shared.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, reply);
        let _ = self.outgoing.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        receiver
    }

    fn sync(&self, uri: &str, stale: bool, text: impl FnOnce() -> String) {
        let mut open = self.open.borrow_mut();
        match open.get_mut(uri) {
            None => {
                open.insert(uri.to_owned(), 1);
                self.notify(
                    "textDocument/didOpen",
                    json!({ "textDocument": {
                        "uri": uri, "languageId": self.kind.language_id(), "version": 1, "text": text(),
                    }}),
                );
            }
            Some(version) if stale => {
                *version += 1;
                self.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": *version },
                        "contentChanges": [{ "text": text() }],
                    }),
                );
            }
            Some(_) => {}
        }
    }

    fn close(&self, uri: &str) {
        if self.open.borrow_mut().remove(uri).is_some() {
            self.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } }));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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

fn dispatch(shared: &Shared, message: Value) {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").cloned();
    match (method, id) {
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let Some(reply) = shared.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) else { return };
            let result = match message.get("error") {
                Some(error) if error.get("code").and_then(Value::as_i64) == Some(CONTENT_MODIFIED) => {
                    Err(ServerError::Busy)
                }
                Some(error) => Err(ServerError::Failed(format!(
                    "{}: {}",
                    shared.name,
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

#[derive(Default)]
pub struct Manager {
    project: RefCell<Option<PathBuf>>,
    servers: RefCell<HashMap<(Kind, PathBuf), Rc<Server>>>,
    failed: RefCell<HashMap<Kind, String>>,
    last_root: RefCell<HashMap<Kind, PathBuf>>,
    reaper: RefCell<Option<Task<()>>>,
}

impl Manager {
    pub fn set_project(&self, root: Option<PathBuf>) {
        *self.project.borrow_mut() = root;
        self.servers.borrow_mut().clear();
        self.failed.borrow_mut().clear();
        self.last_root.borrow_mut().clear();
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
            .or_else(|| path.ancestors().skip(1).find(|d| d.join(kind.marker()).is_file()).map(Path::to_path_buf))
            .ok_or_else(|| format!("No {} project for this file", kind.server_name()))?;

        let key = (kind, root.clone());
        let existing = self.servers.borrow().get(&key).filter(|s| s.alive()).cloned();
        let server = match existing {
            Some(server) => server,
            None => match Server::start(kind, &root) {
                Ok(server) => {
                    let server = Rc::new(server);
                    self.servers.borrow_mut().insert(key, server.clone());
                    server
                }
                Err(reason) => {
                    self.failed.borrow_mut().insert(kind, reason.clone());
                    return Err(reason);
                }
            },
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

    pub fn close(&self, path: &Path) {
        let uri = path_to_uri(path);
        for server in self.servers.borrow().values() {
            server.close(&uri);
        }
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
        server.sync(&uri, self.stale.replace(false), || text.to_string());
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
    #[ignore]
    fn sourcekit_finds_foundation_interface() {
        let dir = std::env::temp_dir().join(format!("forge-lsp-swift-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.swift");
        let text = "import Foundation\n";
        std::fs::write(&file, text).unwrap();
        let server = Server::start(Kind::Swift, &dir).expect("sourcekit-lsp");
        let uri = path_to_uri(&file);
        server.sync(&uri, false, || text.to_owned());
        let reply = server.request(
            "textDocument/definition",
            json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 9 } }),
        );
        let value = reply.recv_timeout(Duration::from_secs(60)).unwrap().unwrap();
        let links = into_links(serde_json::from_value(value).unwrap());
        assert!(links[0].target_uri.as_str().ends_with("Foundation.swiftinterface"), "{links:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
