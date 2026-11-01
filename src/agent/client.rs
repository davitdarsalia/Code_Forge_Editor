use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::policy;
use super::protocol::{
    self, ConfigOption, ContentBlock, CreateTerminal, Incoming, InitializeResponse, PermissionRequest, ReadRequest,
    RpcError, SessionModes, SessionUpdate, TerminalRef, WriteRequest, code, method,
};
use super::registry::AgentSpec;
use super::terminals::Terminals;

const STDERR_TAIL: usize = 4096;
const STDERR_LINES: usize = 4;

pub enum Command {
    Prompt(Vec<ContentBlock>),
    Cancel,
    SetMode(String),
    SetConfig(String, String),
    NewSession,
}

pub struct Ready {
    pub agent: String,
    pub modes: Option<SessionModes>,
    pub config: Vec<ConfigOption>,
    pub embedded_context: bool,
}

pub enum Event {
    Ready(Ready),
    Update(SessionUpdate),
    TurnEnded(Result<String, String>),
    Config(Vec<ConfigOption>),
    ModeChanged(String),
    Permission { request: PermissionRequest, reply: Reply },
    AutoApproved(String),
    Read { path: PathBuf, line: Option<u32>, limit: Option<u32>, reply: Reply },
    Write { path: PathBuf, content: String, reply: Reply },
    TerminalChanged,
    Failed(String),
    Exited(String),
}

pub enum Input {
    Line(String),
    Closed,
    Command(Command),
    Respond(Value, Result<Value, RpcError>),
}

pub struct Reply {
    id: Value,
    tx: Option<flume::Sender<Input>>,
    fallback: Result<Value, RpcError>,
}

impl Reply {
    fn new(id: Value, tx: flume::Sender<Input>, fallback: Result<Value, RpcError>) -> Self {
        Reply { id, tx: Some(tx), fallback }
    }

    pub fn send(mut self, result: Result<Value, RpcError>) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Input::Respond(self.id.clone(), result));
        }
    }

    pub fn read(self, result: Result<String, String>) {
        self.send(result.map(|content| json!({ "content": content })).map_err(|m| RpcError::new(code::RESOURCE_NOT_FOUND, m)));
    }

    pub fn written(self, result: Result<(), String>) {
        self.send(result.map(|()| json!({})).map_err(|m| RpcError::new(code::INTERNAL, m)));
    }

    pub fn permission(self, option: Option<&str>) {
        self.send(Ok(protocol::permission_outcome(option)));
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let fallback = std::mem::replace(&mut self.fallback, Ok(Value::Null));
            let _ = tx.send(Input::Respond(self.id.clone(), fallback));
        }
    }
}

#[derive(Clone)]
pub struct Handle(flume::Sender<Input>);

impl Handle {
    pub fn send(&self, command: Command) {
        let _ = self.0.send(Input::Command(command));
    }
}

#[derive(Clone)]
pub struct Shared {
    pub root: PathBuf,
    pub full_access: Arc<AtomicBool>,
    pub terminals: Arc<Terminals>,
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.try_wait();
    }
}

enum Pending {
    Initialize,
    NewSession,
    Prompt,
    SetMode(String),
    SetConfig,
}

pub fn channel() -> (Handle, flume::Receiver<Input>) {
    let (tx, rx) = flume::unbounded();
    (Handle(tx), rx)
}

pub fn spawn_process(spec: &AgentSpec, cwd: &Path) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt as _;
    Process::new(&spec.program)
        .args(&spec.args)
        .env("PATH", super::registry::search_path())
        .envs(&spec.env)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
}

fn stderr_tail(tail: &Mutex<String>) -> String {
    let text = tail.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(STDERR_LINES);
    policy::redact(&lines[start..].join("\n"))
}

fn auth_hint(methods: &[protocol::AuthMethod]) -> String {
    let names: Vec<&str> = methods.iter().map(|m| m.name.as_str()).collect();
    let how = "Sign in with the agent's own CLI in the terminal, then restart the agent.";
    if names.is_empty() { format!("The agent needs you to sign in. {how}") } else { format!("The agent needs you to sign in ({}). {how}", names.join(", ")) }
}

struct Connection {
    shared: Shared,
    events: flume::Sender<Event>,
    tx: flume::Sender<Input>,
    out: flume::Sender<String>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    session: Option<String>,
    queued: Vec<Command>,
    agent: String,
    init: InitializeResponse,
}

impl Connection {
    fn call(&mut self, method: &str, params: Value, pending: Pending) {
        self.next_id += 1;
        self.pending.insert(self.next_id, pending);
        let _ = self.out.send(protocol::request(self.next_id, method, params));
    }

    fn respond(&self, id: &Value, result: Result<Value, RpcError>) {
        let _ = self.out.send(protocol::response(id, result));
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn new_session(&mut self) {
        let params = json!({ "cwd": self.shared.root, "mcpServers": [] });
        self.call(method::SESSION_NEW, params, Pending::NewSession);
    }

    fn command(&mut self, command: Command) {
        let Some(session) = self.session.clone() else {
            self.queued.push(command);
            return;
        };
        match command {
            Command::Prompt(blocks) => {
                self.call(method::SESSION_PROMPT, json!({ "sessionId": session, "prompt": blocks }), Pending::Prompt)
            }
            Command::Cancel => {
                let _ = self.out.send(protocol::notification(method::SESSION_CANCEL, json!({ "sessionId": session })));
            }
            Command::SetMode(mode) => self.call(
                method::SESSION_SET_MODE,
                json!({ "sessionId": session, "modeId": mode }),
                Pending::SetMode(mode),
            ),
            Command::SetConfig(config, value) => self.call(
                method::SESSION_SET_CONFIG,
                json!({ "sessionId": session, "configId": config, "value": value }),
                Pending::SetConfig,
            ),
            Command::NewSession => {
                self.session = None;
                self.new_session();
            }
        }
    }

    fn on_response(&mut self, id: u64, result: Result<Value, RpcError>) -> bool {
        let Some(pending) = self.pending.remove(&id) else { return true };
        match (pending, result) {
            (Pending::Initialize, Ok(value)) => {
                self.init = serde_json::from_value(value).unwrap_or_default();
                if let Some(info) = self.init.agent_info.as_ref() {
                    self.agent = info.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| info.name.clone());
                }
                self.new_session();
            }
            (Pending::Initialize, Err(err)) => {
                self.emit(Event::Failed(format!("Agent failed to start: {}", err.describe())));
                return false;
            }
            (Pending::NewSession, Ok(value)) => match protocol::parse_new_session(&value) {
                Some(session) => {
                    self.session = Some(session.session_id);
                    self.emit(Event::Ready(Ready {
                        agent: self.agent.clone(),
                        modes: session.modes,
                        config: session.config,
                        embedded_context: self.init.agent_capabilities.prompt_capabilities.embedded_context,
                    }));
                    for command in std::mem::take(&mut self.queued) {
                        self.command(command);
                    }
                }
                None => self.emit(Event::Failed("The agent returned an invalid session".into())),
            },
            (Pending::NewSession, Err(err)) if err.code == code::AUTH_REQUIRED => {
                self.emit(Event::Failed(auth_hint(&self.init.auth_methods)));
            }
            (Pending::NewSession, Err(err)) => self.emit(Event::Failed(err.describe())),
            (Pending::Prompt, result) => {
                self.emit(Event::TurnEnded(result.map(|v| protocol::stop_reason(&v)).map_err(|e| e.describe())))
            }
            (Pending::SetMode(mode), Ok(_)) => self.emit(Event::ModeChanged(mode)),
            (Pending::SetConfig, Ok(value)) => {
                if let Some(options) = value.get("configOptions") {
                    self.emit(Event::Config(protocol::parse_config_options(options)));
                }
            }
            (Pending::SetMode(_) | Pending::SetConfig, Err(err)) => self.emit(Event::Failed(err.describe())),
        }
        true
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf, RpcError> {
        policy::resolve_path(&self.shared.root, path, self.shared.full_access.load(Ordering::Relaxed))
            .map_err(|m| RpcError::new(code::INVALID_PARAMS, m))
    }

    fn on_request(&mut self, id: Value, method: &str, params: Value) {
        let invalid = |e: serde_json::Error| RpcError::new(code::INVALID_PARAMS, e.to_string());
        let cancelled = Ok(protocol::permission_outcome(None));
        let gone = Err(RpcError::new(code::INTERNAL, "the editor closed the request"));
        match method {
            method::REQUEST_PERMISSION => {
                let request: PermissionRequest = match serde_json::from_value(params) {
                    Ok(request) => request,
                    Err(e) => return self.respond(&id, Err(invalid(e))),
                };
                if self.shared.full_access.load(Ordering::Relaxed) {
                    if let Some(option) = policy::auto_pick(&request.options) {
                        let title = request.tool_call.fields.title.clone().unwrap_or_else(|| "Tool call".into());
                        self.emit(Event::AutoApproved(title));
                        return self.respond(&id, Ok(protocol::permission_outcome(Some(&option))));
                    }
                }
                let reply = Reply::new(id, self.tx.clone(), cancelled);
                self.emit(Event::Permission { request, reply });
            }
            method::FS_READ => {
                let request: ReadRequest = match serde_json::from_value(params) {
                    Ok(request) => request,
                    Err(e) => return self.respond(&id, Err(invalid(e))),
                };
                match self.resolve(&request.path) {
                    Ok(path) => {
                        let reply = Reply::new(id, self.tx.clone(), gone);
                        self.emit(Event::Read { path, line: request.line, limit: request.limit, reply });
                    }
                    Err(err) => self.respond(&id, Err(err)),
                }
            }
            method::FS_WRITE => {
                let request: WriteRequest = match serde_json::from_value(params) {
                    Ok(request) => request,
                    Err(e) => return self.respond(&id, Err(invalid(e))),
                };
                match self.resolve(&request.path) {
                    Ok(path) => {
                        let reply = Reply::new(id, self.tx.clone(), gone);
                        self.emit(Event::Write { path, content: request.content, reply });
                    }
                    Err(err) => self.respond(&id, Err(err)),
                }
            }
            method::TERMINAL_CREATE => {
                let request: CreateTerminal = match serde_json::from_value(params) {
                    Ok(request) => request,
                    Err(e) => return self.respond(&id, Err(invalid(e))),
                };
                let cwd = match request.cwd.as_deref() {
                    Some(cwd) => match self.resolve(cwd) {
                        Ok(cwd) => cwd,
                        Err(err) => return self.respond(&id, Err(err)),
                    },
                    None => self.shared.root.clone(),
                };
                let notify = self.events.clone();
                let on_change: super::terminals::OnChange = Arc::new(move |_: &str| {
                    let _ = notify.send(Event::TerminalChanged);
                });
                let result = self
                    .shared
                    .terminals
                    .create(&request, &cwd, on_change)
                    .map(|terminal| json!({ "terminalId": terminal }))
                    .map_err(|e| RpcError::new(code::INTERNAL, format!("cannot start {}: {e}", request.command)));
                self.respond(&id, result);
            }
            method::TERMINAL_OUTPUT | method::TERMINAL_WAIT | method::TERMINAL_KILL | method::TERMINAL_RELEASE => {
                let terminal: TerminalRef = match serde_json::from_value(params) {
                    Ok(terminal) => terminal,
                    Err(e) => return self.respond(&id, Err(invalid(e))),
                };
                let unknown = || Err(RpcError::new(code::INVALID_PARAMS, "unknown terminal"));
                let terminals = self.shared.terminals.clone();
                match method {
                    method::TERMINAL_OUTPUT => match terminals.get(&terminal.terminal_id) {
                        Some(proc) => {
                            let (output, truncated) = proc.snapshot();
                            let exit = proc.exit_status().map(|s| s.to_json());
                            self.respond(&id, Ok(json!({ "output": output, "truncated": truncated, "exitStatus": exit })));
                        }
                        None => self.respond(&id, unknown()),
                    },
                    method::TERMINAL_WAIT => match terminals.get(&terminal.terminal_id) {
                        Some(proc) => {
                            let tx = self.tx.clone();
                            std::thread::spawn(move || {
                                let status = futures::executor::block_on(proc.wait());
                                let _ = tx.send(Input::Respond(id, Ok(status.to_json())));
                            });
                        }
                        None => self.respond(&id, unknown()),
                    },
                    method::TERMINAL_KILL => {
                        let result = if terminals.kill(&terminal.terminal_id) { Ok(json!({})) } else { unknown() };
                        self.respond(&id, result);
                    }
                    _ => {
                        terminals.release(&terminal.terminal_id);
                        self.respond(&id, Ok(json!({})));
                    }
                }
            }
            _ => self.respond(&id, Err(RpcError::new(code::METHOD_NOT_FOUND, format!("{method} is not supported")))),
        }
    }
}

pub async fn run(
    spec: AgentSpec,
    shared: Shared,
    handle: Handle,
    inputs: flume::Receiver<Input>,
    events: flume::Sender<Event>,
) {
    let mut child = match spawn_process(&spec, &shared.root) {
        Ok(child) => child,
        Err(err) => {
            let _ = events.send(Event::Exited(format!("Cannot start {}: {err}", spec.command_line())));
            return;
        }
    };
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let guard = ChildGuard(child);
    let tx = handle.0.clone();
    let tail = Arc::new(Mutex::new(String::new()));
    if let Some(stdout) = stdout {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) if line.trim().is_empty() => continue,
                    Ok(line) => {
                        if tx.send(Input::Line(line)).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(Input::Closed);
        });
    }
    if let Some(mut stderr) = stderr {
        let tail = tail.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 2048];
            while let Ok(n) = stderr.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let mut text = tail.lock().unwrap_or_else(|p| p.into_inner());
                text.push_str(&String::from_utf8_lossy(&buffer[..n]));
                if text.len() > STDERR_TAIL {
                    let mut cut = text.len() - STDERR_TAIL;
                    while !text.is_char_boundary(cut) {
                        cut += 1;
                    }
                    text.drain(..cut);
                }
            }
        });
    }
    let (out, lines) = flume::unbounded::<String>();
    if let Some(mut stdin) = stdin {
        std::thread::spawn(move || {
            while let Ok(line) = lines.recv() {
                if stdin.write_all(line.as_bytes()).and_then(|()| stdin.write_all(b"\n")).and_then(|()| stdin.flush()).is_err() {
                    break;
                }
            }
        });
    }
    let mut conn = Connection {
        shared,
        events: events.clone(),
        tx,
        out,
        next_id: 0,
        pending: HashMap::new(),
        session: None,
        queued: Vec::new(),
        agent: spec.name.clone(),
        init: InitializeResponse::default(),
    };
    conn.call(method::INITIALIZE, protocol::initialize_params(env!("CARGO_PKG_VERSION")), Pending::Initialize);
    drop(handle);
    let mut ended_by_error = false;
    while let Ok(input) = inputs.recv_async().await {
        match input {
            Input::Line(line) => match protocol::parse_line(&line) {
                Some(Incoming::Response { id, result }) => {
                    if !conn.on_response(id, result) {
                        ended_by_error = true;
                        break;
                    }
                }
                Some(Incoming::Request { id, method, params }) => conn.on_request(id, &method, params),
                Some(Incoming::Notification { method, params }) => {
                    if method == method::SESSION_UPDATE {
                        if let Some(update) = protocol::parse_update(&params) {
                            conn.emit(Event::Update(update));
                        }
                    }
                }
                None => {}
            },
            Input::Command(command) => conn.command(command),
            Input::Respond(id, result) => conn.respond(&id, result),
            Input::Closed => break,
        }
    }
    conn.shared.terminals.kill_all();
    let mut guard = guard;
    let status = guard.0.try_wait().ok().flatten();
    drop(guard);
    let detail = stderr_tail(&tail);
    let mut reason = match status {
        Some(status) if status.success() => "Agent stopped".to_string(),
        Some(status) => format!("Agent exited ({status})"),
        None if ended_by_error => "Agent stopped".to_string(),
        None => "Agent closed the connection".to_string(),
    };
    if !detail.is_empty() {
        reason.push_str(":\n");
        reason.push_str(&detail);
    }
    let _ = events.send(Event::Exited(reason));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_replies_send_their_fallback() {
        let (handle, rx) = channel();
        let reply = Reply::new(json!(5), handle.0.clone(), Ok(protocol::permission_outcome(None)));
        drop(reply);
        let Ok(Input::Respond(id, Ok(value))) = rx.try_recv() else { panic!("no fallback") };
        assert_eq!(id, json!(5));
        assert_eq!(value["outcome"]["outcome"], json!("cancelled"));
        let reply = Reply::new(json!(6), handle.0.clone(), Ok(Value::Null));
        reply.read(Ok("text".into()));
        let Ok(Input::Respond(_, Ok(value))) = rx.try_recv() else { panic!() };
        assert_eq!(value["content"], json!("text"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn auth_hint_lists_methods() {
        let methods = vec![protocol::AuthMethod { id: "a".into(), name: "Claude login".into(), description: None }];
        assert!(auth_hint(&methods).contains("Claude login"));
        assert!(auth_hint(&[]).starts_with("The agent needs you to sign in."));
    }

    #[test]
    #[ignore]
    fn claude_adapter_initializes() {
        let Some(spec) = super::super::registry::detect(&[], &crate::tools::which)
            .into_iter()
            .find(|a| a.id == super::super::registry::DEFAULT_AGENT)
        else {
            eprintln!("skipped: Claude ACP adapter is not available (no npx)");
            return;
        };
        let mut child = spawn_process(&spec, &std::env::temp_dir()).expect("spawn adapter");
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let guard = ChildGuard(child);
        let line = protocol::request(1, method::INITIALIZE, protocol::initialize_params("test"));
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut response = None;
        for _ in 0..200 {
            let mut buffer = String::new();
            if reader.read_line(&mut buffer).unwrap_or(0) == 0 {
                break;
            }
            if let Some(Incoming::Response { id: 1, result }) = protocol::parse_line(&buffer) {
                response = Some(result);
                break;
            }
        }
        drop(guard);
        let result = response.expect("no initialize response").expect("initialize failed");
        let init: InitializeResponse = serde_json::from_value(result).unwrap();
        assert_eq!(init.protocol_version, protocol::PROTOCOL_VERSION);
        eprintln!("agent: {:?}", init.agent_info.map(|i| (i.name, i.version)));
    }
}
