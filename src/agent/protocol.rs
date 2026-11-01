use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: u32 = 1;

pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const SESSION_NEW: &str = "session/new";
    pub const SESSION_PROMPT: &str = "session/prompt";
    pub const SESSION_CANCEL: &str = "session/cancel";
    pub const SESSION_SET_MODE: &str = "session/set_mode";
    pub const SESSION_SET_CONFIG: &str = "session/set_config_option";
    pub const SESSION_UPDATE: &str = "session/update";
    pub const REQUEST_PERMISSION: &str = "session/request_permission";
    pub const FS_READ: &str = "fs/read_text_file";
    pub const FS_WRITE: &str = "fs/write_text_file";
    pub const TERMINAL_CREATE: &str = "terminal/create";
    pub const TERMINAL_OUTPUT: &str = "terminal/output";
    pub const TERMINAL_WAIT: &str = "terminal/wait_for_exit";
    pub const TERMINAL_KILL: &str = "terminal/kill";
    pub const TERMINAL_RELEASE: &str = "terminal/release";
}

pub mod code {
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const AUTH_REQUIRED: i64 = -32000;
    pub const RESOURCE_NOT_FOUND: i64 = -32002;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        RpcError { code, message: super::policy::redact(&message.into()), data: None }
    }

    pub fn describe(&self) -> String {
        let detail = match &self.data {
            Some(Value::String(text)) => format!(": {text}"),
            Some(Value::Object(map)) => map
                .get("message")
                .or_else(|| map.get("details"))
                .and_then(Value::as_str)
                .map(|text| format!(": {text}"))
                .unwrap_or_default(),
            _ => String::new(),
        };
        let message = if self.message.is_empty() { format!("error {}", self.code) } else { self.message.clone() };
        super::policy::redact(&format!("{message}{detail}"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
    Response { id: u64, result: Result<Value, RpcError> },
}

pub fn parse_line(line: &str) -> Option<Incoming> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let object = value.as_object()?;
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    match (object.get("method").and_then(Value::as_str), object.get("id")) {
        (Some(method), Some(id)) if !id.is_null() => {
            Some(Incoming::Request { id: id.clone(), method: method.to_string(), params })
        }
        (Some(method), _) => Some(Incoming::Notification { method: method.to_string(), params }),
        (None, Some(id)) => {
            let id = id.as_u64()?;
            let result = match object.get("error") {
                Some(error) => Err(serde_json::from_value(error.clone())
                    .unwrap_or_else(|_| RpcError::new(code::INTERNAL, error.to_string()))),
                None => Ok(object.get("result").cloned().unwrap_or(Value::Null)),
            };
            Some(Incoming::Response { id, result })
        }
        (None, None) => None,
    }
}

pub fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

pub fn notification(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

pub fn response(id: &Value, result: Result<Value, RpcError>) -> String {
    match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}).to_string(),
    }
}

pub fn initialize_params(version: &str) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": {"readTextFile": true, "writeTextFile": true},
            "terminal": true,
        },
        "clientInfo": {"name": "forge", "title": "Forge", "version": version},
    })
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Implementation {
    pub name: String,
    pub title: Option<String>,
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptCapabilities {
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentCapabilities {
    pub load_session: bool,
    pub prompt_capabilities: PromptCapabilities,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitializeResponse {
    pub protocol_version: u32,
    pub agent_capabilities: AgentCapabilities,
    pub auth_methods: Vec<AuthMethod>,
    pub agent_info: Option<Implementation>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionMode {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionModes {
    pub current_mode_id: String,
    pub available_modes: Vec<SessionMode>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub current: String,
    pub choices: Vec<ConfigChoice>,
}

fn parse_choices(options: &Value) -> Vec<ConfigChoice> {
    let mut out = Vec::new();
    for item in options.as_array().into_iter().flatten() {
        if let Some(group) = item.get("options") {
            out.extend(parse_choices(group));
            continue;
        }
        let value = item.get("value").and_then(Value::as_str);
        let name = item.get("name").and_then(Value::as_str);
        if let Some(value) = value {
            out.push(ConfigChoice { value: value.into(), name: name.unwrap_or(value).into() });
        }
    }
    out
}

pub fn parse_config_options(value: &Value) -> Vec<ConfigOption> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|option| option.get("type").and_then(Value::as_str).is_none_or(|t| t == "select"))
        .filter_map(|option| {
            let id = option.get("id")?.as_str()?.to_string();
            let current = option.get("currentValue")?.as_str()?.to_string();
            let choices = parse_choices(option.get("options")?);
            if choices.is_empty() {
                return None;
            }
            Some(ConfigOption {
                name: option.get("name").and_then(Value::as_str).unwrap_or(&id).to_string(),
                category: option.get("category").and_then(Value::as_str).map(str::to_string),
                id,
                current,
                choices,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewSessionResponse {
    pub session_id: String,
    pub modes: Option<SessionModes>,
    pub config: Vec<ConfigOption>,
}

pub fn parse_new_session(value: &Value) -> Option<NewSessionResponse> {
    Some(NewSessionResponse {
        session_id: value.get("sessionId")?.as_str()?.to_string(),
        modes: value.get("modes").and_then(|m| serde_json::from_value(m.clone()).ok()),
        config: value.get("configOptions").map(parse_config_options).unwrap_or_default(),
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ResourceLink {
        uri: String,
        name: String,
    },
    Resource {
        resource: EmbeddedResource,
    },
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedResource {
    pub uri: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

impl ContentBlock {
    pub fn text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } => Some(text),
            ContentBlock::Resource { resource } => resource.text.as_deref(),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    Content {
        content: ContentBlock,
    },
    #[serde(rename_all = "camelCase")]
    Diff {
        path: PathBuf,
        #[serde(default)]
        old_text: Option<String>,
        new_text: String,
    },
    #[serde(rename_all = "camelCase")]
    Terminal {
        terminal_id: String,
    },
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Location {
    pub path: PathBuf,
    #[serde(default)]
    pub line: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCallFields {
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub content: Option<Vec<ToolContent>>,
    pub locations: Option<Vec<Location>>,
    pub raw_input: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallUpdate {
    pub tool_call_id: String,
    #[serde(flatten)]
    pub fields: ToolCallFields,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolStatus,
    pub content: Vec<ToolContent>,
    pub locations: Vec<Location>,
    pub raw_input: Option<Value>,
}

impl ToolCall {
    pub fn from_update(update: ToolCallUpdate) -> ToolCall {
        let mut call = ToolCall { id: update.tool_call_id, ..ToolCall::default() };
        call.apply(update.fields);
        call
    }

    pub fn apply(&mut self, fields: ToolCallFields) {
        if let Some(title) = fields.title {
            self.title = title;
        }
        if let Some(kind) = fields.kind {
            self.kind = kind;
        }
        if let Some(status) = fields.status {
            self.status = status;
        }
        if let Some(content) = fields.content {
            self.content = content;
        }
        if let Some(locations) = fields.locations {
            self.locations = locations;
        }
        if fields.raw_input.is_some() {
            self.raw_input = fields.raw_input;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    #[serde(default)]
    pub status: PlanStatus,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CommandInput {
    #[serde(default)]
    pub hint: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AvailableCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input: Option<CommandInput>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionUpdate {
    UserChunk(ContentBlock),
    AgentChunk(ContentBlock),
    ThoughtChunk(ContentBlock),
    ToolCall(ToolCallUpdate),
    ToolCallUpdate(ToolCallUpdate),
    Plan(Vec<PlanEntry>),
    Commands(Vec<AvailableCommand>),
    Mode(String),
    Config(Vec<ConfigOption>),
    Other,
}

pub fn parse_update(params: &Value) -> Option<SessionUpdate> {
    let update = params.get("update")?;
    let kind = update.get("sessionUpdate")?.as_str()?;
    let content = || update.get("content").and_then(|c| serde_json::from_value::<ContentBlock>(c.clone()).ok());
    let tool = || serde_json::from_value::<ToolCallUpdate>(update.clone()).ok();
    Some(match kind {
        "user_message_chunk" => SessionUpdate::UserChunk(content()?),
        "agent_message_chunk" => SessionUpdate::AgentChunk(content()?),
        "agent_thought_chunk" => SessionUpdate::ThoughtChunk(content()?),
        "tool_call" => SessionUpdate::ToolCall(tool()?),
        "tool_call_update" => SessionUpdate::ToolCallUpdate(tool()?),
        "plan" => SessionUpdate::Plan(serde_json::from_value(update.get("entries")?.clone()).ok()?),
        "available_commands_update" => {
            SessionUpdate::Commands(serde_json::from_value(update.get("availableCommands")?.clone()).ok()?)
        }
        "current_mode_update" => SessionUpdate::Mode(update.get("currentModeId")?.as_str()?.to_string()),
        "config_option_update" => SessionUpdate::Config(parse_config_options(update.get("configOptions")?)),
        _ => SessionUpdate::Other,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: PermissionKind,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub tool_call: ToolCallUpdate,
    pub options: Vec<PermissionOption>,
}

pub fn permission_outcome(choice: Option<&str>) -> Value {
    match choice {
        Some(option) => json!({"outcome": {"outcome": "selected", "optionId": option}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ReadRequest {
    pub path: PathBuf,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WriteRequest {
    pub path: PathBuf,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTerminal {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub output_byte_limit: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRef {
    pub terminal_id: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExitStatus {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

impl ExitStatus {
    pub fn to_json(&self) -> Value {
        json!({"exitCode": self.code, "signal": self.signal})
    }
}

pub fn stop_reason(result: &Value) -> String {
    result.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests_notifications_and_responses() {
        let request = parse_line(r#"{"jsonrpc":"2.0","id":7,"method":"fs/read_text_file","params":{"path":"/a"}}"#);
        assert!(matches!(request, Some(Incoming::Request { ref method, .. }) if method == "fs/read_text_file"));
        let note = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#);
        assert!(matches!(note, Some(Incoming::Notification { .. })));
        let ok = parse_line(r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#);
        assert_eq!(ok, Some(Incoming::Response { id: 3, result: Ok(json!({"stopReason":"end_turn"})) }));
        let err = parse_line(r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32000,"message":"Authentication required"}}"#);
        let Some(Incoming::Response { result: Err(error), .. }) = err else { panic!() };
        assert_eq!(error.code, code::AUTH_REQUIRED);
        assert!(parse_line("not json").is_none());
    }

    #[test]
    fn session_updates_decode() {
        let chunk = json!({"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}});
        assert_eq!(parse_update(&chunk), Some(SessionUpdate::AgentChunk(ContentBlock::Text { text: "hi".into() })));
        let tool = json!({"sessionId":"s","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"Edit a.rs","kind":"edit","status":"pending",
            "content":[{"type":"diff","path":"/p/a.rs","oldText":"a","newText":"b"},{"type":"terminal","terminalId":"term-1"}],
            "locations":[{"path":"/p/a.rs","line":3}]}});
        let Some(SessionUpdate::ToolCall(update)) = parse_update(&tool) else { panic!() };
        let call = ToolCall::from_update(update);
        assert_eq!((call.kind, call.status, call.title.as_str()), (ToolKind::Edit, ToolStatus::Pending, "Edit a.rs"));
        assert_eq!(call.locations[0].line, Some(3));
        assert!(matches!(&call.content[0], ToolContent::Diff { old_text: Some(old), .. } if old == "a"));
        assert!(matches!(&call.content[1], ToolContent::Terminal { terminal_id } if terminal_id == "term-1"));
        let plan = json!({"update":{"sessionUpdate":"plan","entries":[{"content":"x","priority":"high","status":"in_progress"}]}});
        assert_eq!(parse_update(&plan), Some(SessionUpdate::Plan(vec![PlanEntry { content: "x".into(), status: PlanStatus::InProgress }])));
        let unknown = json!({"update":{"sessionUpdate":"usage_update","used":1,"size":2}});
        assert_eq!(parse_update(&unknown), Some(SessionUpdate::Other));
    }

    #[test]
    fn tool_updates_merge_fields() {
        let mut call = ToolCall { id: "t".into(), title: "Run".into(), ..ToolCall::default() };
        let update: ToolCallUpdate = serde_json::from_value(json!({"toolCallId":"t","status":"completed"})).unwrap();
        call.apply(update.fields);
        assert_eq!((call.title.as_str(), call.status), ("Run", ToolStatus::Completed));
    }

    #[test]
    fn new_session_reads_modes_and_select_options() {
        let response = json!({"sessionId":"abc","modes":{"currentModeId":"default","availableModes":[{"id":"default","name":"Default"},{"id":"plan","name":"Plan"}]},
            "configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"opus",
                "options":[{"value":"opus","name":"Opus"},{"group":"g","name":"More","options":[{"value":"haiku","name":"Haiku"}]}]},
                {"id":"flag","name":"Flag","type":"boolean","currentValue":true}]});
        let parsed = parse_new_session(&response).unwrap();
        assert_eq!(parsed.session_id, "abc");
        assert_eq!(parsed.modes.unwrap().available_modes.len(), 2);
        assert_eq!(parsed.config.len(), 1);
        assert_eq!(parsed.config[0].choices.iter().map(|c| c.value.as_str()).collect::<Vec<_>>(), vec!["opus", "haiku"]);
        assert_eq!(parsed.config[0].category.as_deref(), Some("model"));
    }

    #[test]
    fn outgoing_messages_are_json_rpc() {
        let line = request(1, method::INITIALIZE, initialize_params("0.1.0"));
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["params"]["clientCapabilities"]["fs"]["writeTextFile"], json!(true));
        assert_eq!(value["params"]["protocolVersion"], json!(1));
        let reply: Value = serde_json::from_str(&response(&json!(9), Err(RpcError::new(code::INVALID_PARAMS, "bad")))).unwrap();
        assert_eq!(reply["error"]["code"], json!(-32602));
        assert_eq!(permission_outcome(Some("once"))["outcome"]["optionId"], json!("once"));
        assert_eq!(permission_outcome(None)["outcome"]["outcome"], json!("cancelled"));
        let block = serde_json::to_value(ContentBlock::ResourceLink { uri: "file:///a".into(), name: "a".into() }).unwrap();
        assert_eq!(block, json!({"type":"resource_link","uri":"file:///a","name":"a"}));
    }
}
