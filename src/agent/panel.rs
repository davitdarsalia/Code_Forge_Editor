use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, linear_color_stop, linear_gradient,
    prelude::FluentBuilder as _, px,
};

use super::client::{self, Command, Event, Handle, Reply, Shared};
use super::diff;
use super::protocol::{
    AvailableCommand, ConfigOption, ContentBlock, EmbeddedResource, PermissionKind, PermissionOption, PlanEntry,
    PlanStatus, SessionModes, SessionUpdate, ToolCall, ToolContent, ToolKind, ToolStatus,
};
use super::registry::{self, AgentSpec};
use super::terminals::Terminals;
use crate::markdown_view::{self, MarkdownView};
use crate::settings::Settings;
use crate::theme::{Palette, ca, c};
use crate::ui::icon;

const FLUSH_DELAY: Duration = Duration::from_millis(60);
const MAX_TEXT_PREVIEW: usize = 4000;
const TERMINAL_TAIL_LINES: usize = 40;
const MAX_COMMAND_SUGGESTIONS: usize = 8;
const INPUT_MIN_ROWS: usize = 2;
const INPUT_MAX_ROWS: usize = 10;

pub type Slot<T> = Rc<RefCell<Option<T>>>;

pub enum PanelEvent {
    Read { path: PathBuf, line: Option<u32>, limit: Option<u32>, reply: Slot<Reply> },
    Write { path: PathBuf, content: String, reply: Slot<Reply> },
    Open { path: PathBuf, line: Option<u32> },
    Editing(PathBuf),
    AttachCurrent,
    FocusEditor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub path: PathBuf,
    pub selection: Option<(String, u32, u32)>,
}

impl Attachment {
    fn label(&self) -> String {
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match &self.selection {
            Some((_, start, end)) if start == end => format!("{name}:{start}"),
            Some((_, start, end)) => format!("{name}:{start}-{end}"),
            None => name,
        }
    }
}

pub fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::from("file://");
    for ch in text.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            '%' => out.push_str("%25"),
            other => out.push(other),
        }
    }
    out
}

pub fn prompt_blocks(text: &str, attachments: &[Attachment], embedded: bool) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    if !text.trim().is_empty() {
        blocks.push(ContentBlock::Text { text: text.to_string() });
    }
    for attachment in attachments {
        let uri = file_uri(&attachment.path);
        match &attachment.selection {
            Some((selected, start, end)) if embedded => blocks.push(ContentBlock::Resource {
                resource: EmbeddedResource {
                    uri: format!("{uri}#L{start}-{end}"),
                    text: Some(selected.clone()),
                    mime_type: Some("text/plain".into()),
                },
            }),
            Some((selected, start, end)) => blocks.push(ContentBlock::Text {
                text: format!("{}:{start}-{end}\n```\n{selected}\n```", attachment.path.display()),
            }),
            None => blocks.push(ContentBlock::ResourceLink {
                uri,
                name: attachment.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            }),
        }
    }
    blocks
}

pub fn matching_commands<'a>(input: &str, commands: &'a [AvailableCommand]) -> Vec<&'a AvailableCommand> {
    let Some(query) = input.strip_prefix('/') else { return Vec::new() };
    if query.contains(char::is_whitespace) {
        return Vec::new();
    }
    commands.iter().filter(|c| c.name.starts_with(query)).take(MAX_COMMAND_SUGGESTIONS).collect()
}

#[derive(Clone, PartialEq)]
enum Status {
    Idle,
    Starting,
    Ready,
    Busy,
    Failed(SharedString),
    Exited(SharedString),
}

struct ToolEntry {
    call: ToolCall,
    diffs: Vec<(PathBuf, Arc<diff::Summary>)>,
    diff_task: Option<Task<()>>,
    open: bool,
}

struct PermissionEntry {
    title: String,
    options: Vec<PermissionOption>,
    reply: Option<Reply>,
    chosen: Option<String>,
}

enum Entry {
    User { text: SharedString, attachments: Vec<SharedString> },
    Agent { text: String, view: Entity<MarkdownView>, dirty: bool },
    Thought { text: String, open: bool },
    Tool(ToolEntry),
    Permission(PermissionEntry),
    Notice { text: SharedString, error: bool },
}

struct Connection {
    handle: Handle,
    _task: Task<()>,
    _pump: Task<()>,
}

pub struct AgentPanel {
    root: Option<PathBuf>,
    agents: Vec<AgentSpec>,
    detected: bool,
    _detect: Option<Task<()>>,
    connection: Option<Connection>,
    retired: Vec<Connection>,
    status: Status,
    agent_title: Option<SharedString>,
    embedded_context: bool,
    entries: Vec<Entry>,
    tools: HashMap<String, usize>,
    plan: Vec<PlanEntry>,
    commands: Vec<AvailableCommand>,
    modes: Option<SessionModes>,
    config: Vec<ConfigOption>,
    input: Entity<TextareaState>,
    attachments: Vec<Attachment>,
    full_access: Arc<AtomicBool>,
    terminals: Arc<Terminals>,
    scroll: ScrollHandle,
    flush: Option<Task<()>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for AgentPanel {}

impl AgentPanel {
    pub fn new(root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask the agent…  (Enter to send, ⇧Enter for a new line)")
                .submit_on_enter(true)
                .auto_grow(INPUT_MIN_ROWS, INPUT_MAX_ROWS)
        });
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            }),
            cx.on_app_quit(|this, _| {
                this.shutdown();
                async {}
            }),
        ];
        let full_access = Arc::new(AtomicBool::new(Settings::get(cx).agent_full_access));
        let mut panel = AgentPanel {
            root,
            agents: Vec::new(),
            detected: false,
            _detect: None,
            connection: None,
            retired: Vec::new(),
            status: Status::Idle,
            agent_title: None,
            embedded_context: false,
            entries: Vec::new(),
            tools: HashMap::new(),
            plan: Vec::new(),
            commands: Vec::new(),
            modes: None,
            config: Vec::new(),
            input,
            attachments: Vec::new(),
            full_access,
            terminals: Arc::new(Terminals::default()),
            scroll: ScrollHandle::new(),
            flush: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        panel.detect(cx);
        panel
    }

    fn detect(&mut self, cx: &mut Context<Self>) {
        let custom = Settings::get(cx).agents.clone();
        let task = cx.background_spawn(async move { registry::detect(&custom, &crate::tools::which) });
        self._detect = Some(cx.spawn(async move |this, cx| {
            let agents = task.await;
            let _ = this.update(cx, |this, cx| {
                this.agents = agents;
                this.detected = true;
                cx.notify();
            });
        }));
    }

    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.root = root;
        self.stop_connection();
        self.reset_thread();
        self.status = Status::Idle;
        cx.notify();
    }

    pub fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| state.focus(window, cx));
    }

    pub fn add_attachment(&mut self, attachment: Attachment, cx: &mut Context<Self>) {
        if !self.attachments.contains(&attachment) {
            self.attachments.retain(|a| !(a.path == attachment.path && a.selection.is_none() && attachment.selection.is_none()));
            self.attachments.push(attachment);
            cx.notify();
        }
    }

    pub fn shutdown(&mut self) {
        self.stop_connection();
        self.retired.clear();
        self.terminals.kill_all();
    }

    fn selected_spec(&self, cx: &App) -> Option<AgentSpec> {
        registry::select(&self.agents, &Settings::get(cx).agent).cloned()
    }

    fn stop_connection(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.retired.push(connection);
        }
        for entry in &mut self.entries {
            if let Entry::Permission(permission) = entry {
                permission.reply.take();
            }
        }
    }

    fn reset_thread(&mut self) {
        self.entries.clear();
        self.tools.clear();
        self.plan.clear();
        self.flush = None;
    }

    fn ensure_connection(&mut self, cx: &mut Context<Self>) -> Option<Handle> {
        if let Some(connection) = self.connection.as_ref() {
            return Some(connection.handle.clone());
        }
        self.retired.clear();
        let Some(root) = self.root.clone() else {
            self.push_notice("Open a folder first; the agent works inside the project.", true, cx);
            return None;
        };
        let Some(spec) = self.selected_spec(cx) else {
            self.push_notice("No agent found. Install Node.js (for npx) or one of the agent CLIs.", true, cx);
            return None;
        };
        self.full_access.store(Settings::get(cx).agent_full_access, Ordering::Relaxed);
        self.terminals = Arc::new(Terminals::default());
        let shared = Shared { root, full_access: self.full_access.clone(), terminals: self.terminals.clone() };
        let (handle, inputs) = client::channel();
        let (events_tx, events) = flume::unbounded::<Event>();
        let task = cx.background_spawn(client::run(spec.clone(), shared, handle.clone(), inputs, events_tx));
        let pump = cx.spawn(async move |this, cx| {
            while let Ok(first) = events.recv_async().await {
                let mut batch = vec![first];
                batch.extend(events.drain());
                if this.update(cx, |this, cx| this.on_events(batch, cx)).is_err() {
                    break;
                }
            }
        });
        self.agent_title = Some(spec.name.clone().into());
        self.status = Status::Starting;
        self.connection = Some(Connection { handle: handle.clone(), _task: task, _pump: pump });
        cx.notify();
        Some(handle)
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        self.stop_connection();
        self.status = Status::Idle;
        self.modes = None;
        self.config.clear();
        self.commands.clear();
        self.ensure_connection(cx);
    }

    fn new_thread(&mut self, cx: &mut Context<Self>) {
        self.reset_thread();
        self.attachments.clear();
        match self.connection.as_ref() {
            Some(connection) if matches!(self.status, Status::Ready | Status::Busy) => {
                if self.status == Status::Busy {
                    connection.handle.send(Command::Cancel);
                }
                connection.handle.send(Command::NewSession);
                self.status = Status::Starting;
            }
            _ => {
                self.stop_connection();
                self.status = Status::Idle;
            }
        }
        cx.notify();
    }

    fn select_agent(&mut self, id: String, cx: &mut Context<Self>) {
        if Settings::get(cx).agent == id {
            return;
        }
        crate::settings::update(cx, |s| s.agent = id.clone());
        self.stop_connection();
        self.status = Status::Idle;
        self.modes = None;
        self.config.clear();
        self.commands.clear();
        if let Some(spec) = self.agents.iter().find(|a| a.id == id) {
            self.agent_title = Some(spec.name.clone().into());
            if !self.entries.is_empty() {
                self.push_notice(format!("Switched to {}. The next message starts a new session.", spec.name), false, cx);
            }
        }
        cx.notify();
    }

    fn toggle_full_access(&mut self, cx: &mut Context<Self>) {
        let on = !Settings::get(cx).agent_full_access;
        crate::settings::update(cx, |s| s.agent_full_access = on);
        self.full_access.store(on, Ordering::Relaxed);
        if on {
            self.answer_pending_automatically();
        }
        cx.notify();
    }

    fn answer_pending_automatically(&mut self) {
        for entry in &mut self.entries {
            if let Entry::Permission(permission) = entry {
                if permission.reply.is_some() {
                    if let Some(option) = super::policy::auto_pick(&permission.options) {
                        if let Some(reply) = permission.reply.take() {
                            reply.permission(Some(&option));
                        }
                        permission.chosen = Some(option);
                    }
                }
            }
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        if self.status == Status::Busy {
            return;
        }
        let Some(handle) = self.ensure_connection(cx) else { return };
        let blocks = prompt_blocks(text.trim_end(), &self.attachments, self.embedded_context);
        let labels = self.attachments.iter().map(|a| SharedString::from(a.label())).collect();
        self.entries.push(Entry::User { text: text.trim_end().to_string().into(), attachments: labels });
        self.attachments.clear();
        handle.send(Command::Prompt(blocks));
        if self.status == Status::Ready {
            self.status = Status::Busy;
        }
        self.input.update(cx, |state, cx| state.set_value("", window, cx));
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(connection) = self.connection.as_ref() {
            connection.handle.send(Command::Cancel);
        }
        for entry in &mut self.entries {
            if let Entry::Permission(permission) = entry {
                if let Some(reply) = permission.reply.take() {
                    reply.permission(None);
                    permission.chosen = Some(String::new());
                }
            }
        }
        cx.notify();
    }

    fn push_notice(&mut self, text: impl Into<SharedString>, error: bool, cx: &mut Context<Self>) {
        self.entries.push(Entry::Notice { text: text.into(), error });
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn on_events(&mut self, events: Vec<Event>, cx: &mut Context<Self>) {
        let mut notify = false;
        for event in events {
            notify |= self.on_event(event, cx);
        }
        if notify {
            cx.notify();
        }
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) -> bool {
        match event {
            Event::Ready(ready) => {
                self.agent_title = Some(ready.agent.into());
                self.embedded_context = ready.embedded_context;
                self.modes = ready.modes;
                self.config = ready.config;
                let waiting = self.entries.iter().rev().take_while(|e| !matches!(e, Entry::Agent { .. })).any(|e| matches!(e, Entry::User { .. }));
                self.status = if waiting { Status::Busy } else { Status::Ready };
            }
            Event::Update(update) => return self.on_update(update, cx),
            Event::TurnEnded(result) => {
                self.flush_now(cx);
                self.status = Status::Ready;
                match result {
                    Ok(reason) if reason == "cancelled" => self.push_notice("Stopped.", false, cx),
                    Ok(reason) if reason == "refusal" => self.push_notice("The agent declined this request.", true, cx),
                    Ok(reason) if reason == "max_tokens" || reason == "max_turn_requests" => {
                        self.push_notice("The agent hit its turn limit.", true, cx)
                    }
                    Ok(_) => {}
                    Err(message) => self.push_notice(message, true, cx),
                }
            }
            Event::Config(config) => self.config = config,
            Event::ModeChanged(mode) => {
                if let Some(modes) = self.modes.as_mut() {
                    modes.current_mode_id = mode;
                }
            }
            Event::Permission { request, reply } => {
                let title = request.tool_call.fields.title.clone().unwrap_or_else(|| "The agent wants to run a tool".into());
                self.upsert_tool(request.tool_call, cx);
                if self.full_access.load(Ordering::Relaxed) {
                    if let Some(option) = super::policy::auto_pick(&request.options) {
                        reply.permission(Some(&option));
                        return true;
                    }
                }
                self.entries.push(Entry::Permission(PermissionEntry {
                    title,
                    options: request.options,
                    reply: Some(reply),
                    chosen: None,
                }));
                self.scroll.scroll_to_bottom();
            }
            Event::AutoApproved(title) => {
                self.entries.push(Entry::Notice { text: format!("Auto-approved: {title}").into(), error: false });
            }
            Event::Read { path, line, limit, reply } => {
                cx.emit(PanelEvent::Read { path, line, limit, reply: Rc::new(RefCell::new(Some(reply))) });
                return false;
            }
            Event::Write { path, content, reply } => {
                cx.emit(PanelEvent::Write { path, content, reply: Rc::new(RefCell::new(Some(reply))) });
                return false;
            }
            Event::TerminalChanged => {}
            Event::Failed(message) => {
                if matches!(self.status, Status::Starting) {
                    self.status = Status::Failed(message.clone().into());
                    self.stop_connection();
                }
                self.push_notice(message, true, cx);
            }
            Event::Exited(reason) => {
                self.flush_now(cx);
                let failed = matches!(self.status, Status::Failed(_));
                self.stop_connection();
                if !failed {
                    self.status = Status::Exited(reason.clone().into());
                    self.push_notice(reason, true, cx);
                }
            }
        }
        true
    }

    fn on_update(&mut self, update: SessionUpdate, cx: &mut Context<Self>) -> bool {
        match update {
            SessionUpdate::AgentChunk(block) => {
                if let Some(text) = block.text() {
                    self.append_agent(text, cx);
                }
                return false;
            }
            SessionUpdate::ThoughtChunk(block) => {
                if let Some(text) = block.text() {
                    match self.entries.last_mut() {
                        Some(Entry::Thought { text: existing, .. }) => existing.push_str(text),
                        _ => self.entries.push(Entry::Thought { text: text.to_string(), open: false }),
                    }
                    self.schedule_flush(cx);
                }
                return false;
            }
            SessionUpdate::UserChunk(_) | SessionUpdate::Other => return false,
            SessionUpdate::ToolCall(update) | SessionUpdate::ToolCallUpdate(update) => self.upsert_tool(update, cx),
            SessionUpdate::Plan(entries) => self.plan = entries,
            SessionUpdate::Commands(commands) => self.commands = commands,
            SessionUpdate::Mode(mode) => {
                if let Some(modes) = self.modes.as_mut() {
                    modes.current_mode_id = mode;
                }
            }
            SessionUpdate::Config(config) => self.config = config,
        }
        true
    }

    fn append_agent(&mut self, text: &str, cx: &mut Context<Self>) {
        match self.entries.last_mut() {
            Some(Entry::Agent { text: existing, dirty, .. }) => {
                existing.push_str(text);
                *dirty = true;
            }
            _ => {
                let base: Arc<Path> = self.root.clone().unwrap_or_default().into();
                let view = cx.new(|_| MarkdownView::flow(base));
                self.entries.push(Entry::Agent { text: text.to_string(), view, dirty: true });
            }
        }
        self.schedule_flush(cx);
    }

    fn schedule_flush(&mut self, cx: &mut Context<Self>) {
        if self.flush.is_some() {
            return;
        }
        self.flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLUSH_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                this.flush = None;
                this.flush_now(cx);
            });
        }));
    }

    fn flush_now(&mut self, cx: &mut Context<Self>) {
        self.flush = None;
        let dark = markdown_view::is_dark(cx);
        for entry in &mut self.entries {
            if let Entry::Agent { text, view, dirty } = entry {
                if !*dirty {
                    continue;
                }
                *dirty = false;
                let source = text.clone();
                let view = view.clone();
                let parsed = cx.background_spawn(async move { markdown_view::parse(&source, dark) });
                cx.spawn(async move |_, cx| {
                    let doc = parsed.await;
                    let _ = view.update(cx, |view, cx| view.set_document(doc, cx));
                })
                .detach();
            }
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn upsert_tool(&mut self, update: super::protocol::ToolCallUpdate, cx: &mut Context<Self>) {
        let id = update.tool_call_id.clone();
        let ix = match self.tools.get(&id).copied() {
            Some(ix) => {
                if let Some(Entry::Tool(tool)) = self.entries.get_mut(ix) {
                    tool.call.apply(update.fields);
                }
                ix
            }
            None => {
                let call = ToolCall::from_update(update);
                self.entries.push(Entry::Tool(ToolEntry { call, diffs: Vec::new(), diff_task: None, open: true }));
                let ix = self.entries.len() - 1;
                self.tools.insert(id, ix);
                self.scroll.scroll_to_bottom();
                ix
            }
        };
        let Some(Entry::Tool(tool)) = self.entries.get_mut(ix) else { return };
        if tool.call.kind == ToolKind::Edit && matches!(tool.call.status, ToolStatus::Pending | ToolStatus::InProgress) {
            for location in &tool.call.locations {
                cx.emit(PanelEvent::Editing(location.path.clone()));
            }
        }
        let wanted: Vec<(PathBuf, String, String)> = tool
            .call
            .content
            .iter()
            .filter_map(|content| match content {
                ToolContent::Diff { path, old_text, new_text } => {
                    Some((path.clone(), old_text.clone().unwrap_or_default(), new_text.clone()))
                }
                _ => None,
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        let task = cx.background_spawn(async move {
            wanted.into_iter().map(|(path, old, new)| (path, Arc::new(diff::summarize(&old, &new)))).collect::<Vec<_>>()
        });
        tool.diff_task = Some(cx.spawn(async move |this, cx| {
            let diffs = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Entry::Tool(tool)) = this.entries.get_mut(ix) {
                    tool.diffs = diffs;
                    tool.diff_task = None;
                }
                cx.notify();
            });
        }));
    }

    fn choose_permission(&mut self, ix: usize, option: String, cx: &mut Context<Self>) {
        if let Some(Entry::Permission(permission)) = self.entries.get_mut(ix) {
            if let Some(reply) = permission.reply.take() {
                reply.permission(Some(&option));
                permission.chosen = Some(option);
            }
        }
        cx.notify();
    }

    fn relative(&self, path: &Path) -> String {
        self.root
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| path.display().to_string())
    }
}

fn status_color(status: &Status) -> Hsla {
    match status {
        Status::Idle => Palette::faint(),
        Status::Starting => Palette::git_modified(),
        Status::Ready => Palette::git_added(),
        Status::Busy => Palette::accent(),
        Status::Failed(_) | Status::Exited(_) => Palette::git_deleted(),
    }
}

fn tool_icon(kind: ToolKind) -> IconName {
    match kind {
        ToolKind::Read => IconName::FileText,
        ToolKind::Edit => IconName::Pencil,
        ToolKind::Delete => IconName::Close,
        ToolKind::Move => IconName::ArrowRight,
        ToolKind::Search => IconName::Search,
        ToolKind::Execute => IconName::SquareTerminal,
        ToolKind::Think => IconName::Brain,
        ToolKind::Fetch => IconName::Globe,
        ToolKind::SwitchMode | ToolKind::Other => IconName::Sparkles,
    }
}

fn tool_status(status: ToolStatus) -> (&'static str, Hsla) {
    match status {
        ToolStatus::Pending => ("Pending", Palette::muted()),
        ToolStatus::InProgress => ("Running", Palette::accent()),
        ToolStatus::Completed => ("Done", Palette::git_added()),
        ToolStatus::Failed => ("Failed", Palette::git_deleted()),
        ToolStatus::Unknown => ("", Palette::muted()),
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

pub fn sparkle_gradient() -> gpui_kit::Background {
    linear_gradient(135., linear_color_stop(c(0x4f8cff), 0.), linear_color_stop(c(0xec4899), 1.))
}

fn mono() -> &'static str {
    crate::theme::mono_font()
}

impl AgentPanel {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let selected = Settings::get(cx).agent.clone();
        let current = registry::select(&self.agents, &selected);
        let label: SharedString = match current {
            Some(spec) => spec.name.clone().into(),
            None if !self.detected => "Looking for agents…".into(),
            None => "No agents found".into(),
        };
        let agents = self.agents.clone();
        let current_id = current.map(|a| a.id.clone());
        let full = Settings::get(cx).agent_full_access;
        let picker = Button::new("agent-picker")
            .ghost()
            .xsmall()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for agent in &agents {
                    let id = agent.id.clone();
                    let entity = entity.clone();
                    let name = if agent.via_npx { format!("{}  (npx)", agent.name) } else { agent.name.clone() };
                    menu = menu.item(PopupMenuItem::new(name).checked(current_id.as_deref() == Some(agent.id.as_str())).on_click(
                        move |_, _, cx| entity.update(cx, |this, cx| this.select_agent(id.clone(), cx)),
                    ));
                }
                menu.separator().label("Add custom agents under \"agents\" in settings.json")
            });
        div()
            .h(px(34.))
            .flex_none()
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(4.))
            .border_b_1()
            .border_color(Palette::border())
            .child(icon(IconName::Sparkles, 14., Palette::accent()))
            .child(picker)
            .child(div().size(px(7.)).rounded_full().bg(status_color(&self.status)))
            .child(div().flex_1())
            .child(header_button("agent-full-access", if full { IconName::ShieldCheck } else { IconName::Shield }, full,
                if full { "Full access: tool calls are approved automatically" } else { "Ask before tool calls (click for full access)" },
                cx.listener(|this, _, _, cx| this.toggle_full_access(cx))))
            .when(matches!(self.status, Status::Exited(_) | Status::Failed(_)), |el| {
                el.child(header_button("agent-restart", IconName::RotateCw, false, "Restart agent", cx.listener(|this, _, _, cx| this.restart(cx))))
            })
            .child(header_button("agent-new-thread", IconName::SquarePen, false, "New thread", cx.listener(|this, _, _, cx| this.new_thread(cx))))
    }

    fn render_entry(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        match entry {
            Entry::User { text, attachments } => div()
                .mx(px(10.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(8.))
                .bg(ca(0xffffff, 0.06))
                .border_1()
                .border_color(Palette::hairline())
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(px(13.)).text_color(Palette::text()).child(text.clone()))
                .when(!attachments.is_empty(), |el| el.child(chips(attachments.iter().cloned(), None, cx)))
                .into_any_element(),
            Entry::Agent { view, .. } => div().px(px(12.)).child(view.clone()).into_any_element(),
            Entry::Thought { text, open } => {
                let open = *open;
                div()
                    .mx(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .id(("agent-thought", ix))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(Palette::muted())
                            .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 11., Palette::muted()))
                            .child(icon(IconName::Brain, 11., Palette::muted()))
                            .child("Thinking")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Entry::Thought { open, .. }) = this.entries.get_mut(ix) {
                                    *open = !*open;
                                }
                                cx.notify();
                            })),
                    )
                    .when(open, |el| {
                        el.child(
                            div()
                                .pl(px(16.))
                                .text_size(px(12.))
                                .italic()
                                .text_color(Palette::muted())
                                .child(clip(text, MAX_TEXT_PREVIEW)),
                        )
                    })
                    .into_any_element()
            }
            Entry::Tool(tool) => self.render_tool(ix, tool, cx),
            Entry::Permission(permission) => self.render_permission(ix, permission, cx),
            Entry::Notice { text, error } => div()
                .mx(px(10.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .text_size(px(12.))
                .when(*error, |el| el.bg(ca(0xff6b6b, 0.1)).text_color(c(0xff9b9b)))
                .when(!*error, |el| el.text_color(Palette::muted()))
                .child(text.clone())
                .into_any_element(),
        }
    }

    fn render_tool(&self, ix: usize, tool: &ToolEntry, cx: &mut Context<Self>) -> AnyElement {
        let call = &tool.call;
        let (status, color) = tool_status(call.status);
        let open = tool.open;
        let mut body = div().flex().flex_col().gap(px(4.));
        if !call.locations.is_empty() {
            body = body.child(div().flex().flex_wrap().gap(px(4.)).children(call.locations.iter().enumerate().map(|(n, location)| {
                let label = match location.line {
                    Some(line) => format!("{}:{line}", self.relative(&location.path)),
                    None => self.relative(&location.path),
                };
                let (path, line) = (location.path.clone(), location.line);
                div()
                    .id(("tool-location", ix * 64 + n))
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(ca(0x0a84ff, 0.12))
                    .text_size(px(11.))
                    .font_family(mono())
                    .text_color(c(0x8ab4ff))
                    .cursor_pointer()
                    .hover(|s| s.bg(ca(0x0a84ff, 0.22)))
                    .child(label)
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(PanelEvent::Open { path: path.clone(), line })))
            })));
        }
        if open {
            for content in &call.content {
                match content {
                    ToolContent::Content { content } => {
                        if let Some(text) = content.text().filter(|t| !t.trim().is_empty()) {
                            body = body.child(code_box(clip(text, MAX_TEXT_PREVIEW)));
                        }
                    }
                    ToolContent::Diff { path, .. } => {
                        if let Some((_, summary)) = tool.diffs.iter().find(|(p, _)| p == path) {
                            body = body.child(render_diff(&self.relative(path), summary));
                        }
                    }
                    ToolContent::Terminal { terminal_id } => {
                        if let Some(proc) = self.terminals.shown(terminal_id) {
                            let (output, truncated) = proc.snapshot();
                            let exit = proc.exit_status();
                            let state = match exit {
                                None => "running…".to_string(),
                                Some(status) => match (status.code, status.signal) {
                                    (Some(code), _) => format!("exit {code}"),
                                    (None, Some(signal)) => signal,
                                    _ => "finished".into(),
                                },
                            };
                            let text = tail_lines(&super::policy::redact(&output), TERMINAL_TAIL_LINES);
                            body = body.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(6.))
                                            .text_size(px(10.5))
                                            .text_color(Palette::faint())
                                            .child(div().font_family(mono()).child(clip(&proc.label, 120)))
                                            .child(state)
                                            .when(truncated, |el| el.child("(truncated)")),
                                    )
                                    .when(!text.is_empty(), |el| el.child(code_box(text))),
                            );
                        }
                    }
                    ToolContent::Unsupported => {}
                }
            }
        }
        div()
            .mx(px(10.))
            .rounded(px(7.))
            .border_1()
            .border_color(Palette::hairline())
            .bg(ca(0x000000, 0.18))
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("tool-header", ix))
                    .px(px(8.))
                    .py(px(5.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .child(icon(tool_icon(call.kind), 12., Palette::muted()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.))
                            .text_color(Palette::text())
                            .child(if call.title.is_empty() { "Tool call".to_string() } else { call.title.clone() }),
                    )
                    .child(div().text_size(px(10.5)).text_color(color).child(status))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(Entry::Tool(tool)) = this.entries.get_mut(ix) {
                            tool.open = !tool.open;
                        }
                        cx.notify();
                    })),
            )
            .child(div().px(px(8.)).pb(px(6.)).child(body))
            .into_any_element()
    }

    fn render_permission(&self, ix: usize, permission: &PermissionEntry, cx: &mut Context<Self>) -> AnyElement {
        let pending = permission.reply.is_some();
        let outcome = permission.chosen.as_ref().map(|chosen| {
            permission
                .options
                .iter()
                .find(|o| &o.option_id == chosen)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| "Cancelled".into())
        });
        div()
            .mx(px(10.))
            .p(px(8.))
            .rounded(px(7.))
            .border_1()
            .border_color(if pending { ca(0xd9a54a, 0.6) } else { Palette::hairline() })
            .bg(if pending { ca(0xd9a54a, 0.08) } else { ca(0x000000, 0.12) })
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(icon(IconName::Shield, 12., Palette::git_modified()))
                    .child(div().text_size(px(12.)).text_color(Palette::text()).child(permission.title.clone())),
            )
            .when(pending, |el| {
                el.child(div().flex().flex_wrap().gap(px(6.)).children(permission.options.iter().enumerate().map(|(n, option)| {
                    let allow = matches!(option.kind, PermissionKind::AllowOnce | PermissionKind::AllowAlways);
                    let id = option.option_id.clone();
                    div()
                        .id(("permission", ix * 16 + n))
                        .px(px(10.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .text_size(px(11.5))
                        .cursor_pointer()
                        .when(allow, |el| el.bg(Palette::accent()).text_color(gpui_kit::white()).hover(|s| s.bg(Palette::accent_hover())))
                        .when(!allow, |el| el.bg(ca(0xffffff, 0.08)).text_color(Palette::text()).hover(|s| s.bg(ca(0xffffff, 0.14))))
                        .child(option.name.clone())
                        .on_click(cx.listener(move |this, _, _, cx| this.choose_permission(ix, id.clone(), cx)))
                })))
            })
            .when_some(outcome, |el, outcome| el.child(div().text_size(px(11.)).text_color(Palette::muted()).child(outcome)))
            .into_any_element()
    }

    fn render_plan(&self) -> impl IntoElement {
        div()
            .mx(px(10.))
            .mb(px(6.))
            .p(px(8.))
            .rounded(px(7.))
            .bg(ca(0xffffff, 0.04))
            .border_1()
            .border_color(Palette::hairline())
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(Palette::muted())
                    .child(icon(IconName::ListTodo, 11., Palette::muted()))
                    .child("PLAN"),
            )
            .children(self.plan.iter().map(|entry| {
                let (name, color) = match entry.status {
                    PlanStatus::Completed => (IconName::CircleCheck, Palette::git_added()),
                    PlanStatus::InProgress => (IconName::CircleDot, Palette::accent()),
                    _ => (IconName::Circle, Palette::faint()),
                };
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .child(div().pt(px(2.)).child(icon(name, 11., color)))
                    .child(
                        div()
                            .flex_1()
                            .text_color(if entry.status == PlanStatus::Completed { Palette::faint() } else { Palette::text() })
                            .when(entry.status == PlanStatus::Completed, |el| el.line_through())
                            .child(entry.content.clone()),
                    )
            }))
    }

    fn render_suggestions(&self, matches: Vec<AvailableCommand>, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .mx(px(10.))
            .mb(px(4.))
            .py(px(4.))
            .rounded(px(7.))
            .bg(c(0x2b2b30))
            .border_1()
            .border_color(Palette::hairline())
            .flex()
            .flex_col()
            .children(matches.into_iter().enumerate().map(|(n, command)| {
                let name = command.name.clone();
                div()
                    .id(("agent-command", n))
                    .px(px(8.))
                    .py(px(3.))
                    .flex()
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(Palette::row_hover()))
                    .child(div().text_size(px(12.)).font_family(mono()).text_color(Palette::text()).child(format!("/{}", command.name)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.5))
                            .text_color(Palette::muted())
                            .child(command.description.clone()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let text = format!("/{name} ");
                        this.input.update(cx, |state, cx| {
                            state.set_value(text, window, cx);
                            state.focus(window, cx);
                        });
                    }))
            }))
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.status == Status::Busy;
        let entity = cx.entity();
        let mut controls = div().flex().items_center().gap(px(2.)).flex_wrap();
        if let Some(modes) = self.modes.as_ref().filter(|m| m.available_modes.len() > 1) {
            let current = modes
                .available_modes
                .iter()
                .find(|m| m.id == modes.current_mode_id)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| modes.current_mode_id.clone());
            let modes = modes.clone();
            let entity = entity.clone();
            controls = controls.child(Button::new("agent-mode").ghost().xsmall().label(current).dropdown_caret(true).dropdown_menu(
                move |mut menu, _, _| {
                    for mode in &modes.available_modes {
                        let (id, entity) = (mode.id.clone(), entity.clone());
                        menu = menu.item(PopupMenuItem::new(mode.name.clone()).checked(mode.id == modes.current_mode_id).on_click(
                            move |_, _, cx| {
                                entity.update(cx, |this, cx| {
                                    if let Some(connection) = this.connection.as_ref() {
                                        connection.handle.send(Command::SetMode(id.clone()));
                                    }
                                    cx.notify();
                                })
                            },
                        ));
                    }
                    menu
                },
            ));
        }
        let skip_mode = self.modes.is_some();
        for (n, option) in self.config.iter().enumerate() {
            if skip_mode && option.category.as_deref() == Some("mode") {
                continue;
            }
            let current = option.choices.iter().find(|c| c.value == option.current).map(|c| c.name.clone()).unwrap_or_else(|| option.current.clone());
            let (option, entity) = (option.clone(), entity.clone());
            controls = controls.child(
                Button::new(("agent-config", n)).ghost().xsmall().label(current).tooltip(option.name.clone()).dropdown_caret(true).dropdown_menu(
                    move |mut menu, _, _| {
                        for choice in &option.choices {
                            let (config, value, entity) = (option.id.clone(), choice.value.clone(), entity.clone());
                            menu = menu.item(PopupMenuItem::new(choice.name.clone()).checked(choice.value == option.current).on_click(
                                move |_, _, cx| {
                                    entity.update(cx, |this, cx| {
                                        if let Some(connection) = this.connection.as_ref() {
                                            connection.handle.send(Command::SetConfig(config.clone(), value.clone()));
                                        }
                                        cx.notify();
                                    })
                                },
                            ));
                        }
                        menu
                    },
                ),
            );
        }
        let send_icon = if busy { IconName::Square } else { IconName::ArrowUp };
        div()
            .flex_none()
            .mx(px(10.))
            .mb(px(10.))
            .rounded(px(9.))
            .border_1()
            .border_color(Palette::hairline())
            .bg(c(0x1b1b1f))
            .flex()
            .flex_col()
            .when(!self.attachments.is_empty(), |el| {
                el.child(div().px(px(8.)).pt(px(6.)).child(chips(self.attachments.iter().map(|a| SharedString::from(a.label())), Some(cx.entity()), cx)))
            })
            .child(div().px(px(4.)).pt(px(2.)).child(Textarea::new(&self.input).appearance(false).bordered(false)))
            .child(
                div()
                    .px(px(4.))
                    .pb(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(header_button("agent-attach", IconName::AtSign, false, "Add current file or selection", cx.listener(|_, _, _, cx| {
                        cx.emit(PanelEvent::AttachCurrent);
                    })))
                    .child(controls)
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("agent-send")
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .bg(if busy { ca(0xffffff, 0.14) } else { Palette::accent() })
                            .hover(|s| s.opacity(0.85))
                            .child(icon(send_icon, 12., gpui_kit::white()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.status == Status::Busy {
                                    this.stop(cx);
                                } else {
                                    this.send(window, cx);
                                }
                            })),
                    ),
            )
    }

    fn render_empty(&self) -> impl IntoElement {
        let hint = match &self.status {
            Status::Failed(message) => message.clone(),
            _ if self.root.is_none() => "Open a folder to start working with an agent.".into(),
            _ if self.detected && self.agents.is_empty() => {
                "No ACP agents found. Install Node.js to run Claude Code through npx, or install gemini, opencode or goose.".into()
            }
            _ => "Ask the agent to explain, change or run anything in this project. Press ⌘L to focus this box.".into(),
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .px(px(24.))
            .child(div().size(px(34.)).rounded_full().bg(sparkle_gradient()).flex().items_center().justify_center().child(icon(IconName::Sparkles, 16., gpui_kit::white())))
            .child(div().text_size(px(12.)).text_color(Palette::muted()).text_center().child(hint))
    }
}

fn header_button(
    id: &'static str,
    name: IconName,
    active: bool,
    tooltip: &'static str,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(icon(name, 13., if active { Palette::accent() } else { Palette::muted() }))
        .tooltip(tooltip)
        .on_click(on_click)
}

fn code_box(text: String) -> impl IntoElement {
    div()
        .px(px(6.))
        .py(px(4.))
        .rounded(px(4.))
        .bg(ca(0x000000, 0.25))
        .text_size(px(11.))
        .font_family(mono())
        .text_color(Palette::text())
        .child(text)
}

fn render_diff(path: &str, summary: &diff::Summary) -> impl IntoElement {
    div()
        .rounded(px(4.))
        .overflow_hidden()
        .border_1()
        .border_color(Palette::hairline())
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(6.))
                .py(px(2.))
                .flex()
                .gap(px(8.))
                .bg(ca(0xffffff, 0.04))
                .text_size(px(10.5))
                .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().font_family(mono()).text_color(Palette::muted()).child(path.to_string()))
                .child(div().text_color(Palette::git_added()).child(format!("+{}", summary.added)))
                .child(div().text_color(Palette::git_deleted()).child(format!("-{}", summary.removed))),
        )
        .children(summary.rows.iter().map(|row| match row {
            diff::Row::Gap(lines) => div()
                .px(px(6.))
                .text_size(px(10.))
                .text_color(Palette::faint())
                .bg(ca(0xffffff, 0.02))
                .child(format!("⋯ {lines} unchanged"))
                .into_any_element(),
            diff::Row::Line { kind, number, text } => {
                let (bg, fg, sign) = match kind {
                    diff::Kind::Added => (ca(0x5fc27a, 0.16), c(0xa6e3b5), "+"),
                    diff::Kind::Removed => (ca(0xff6b6b, 0.16), c(0xffb3b3), "-"),
                    diff::Kind::Same => (gpui_kit::transparent_black(), Palette::muted(), " "),
                };
                div()
                    .flex()
                    .bg(bg)
                    .font_family(mono())
                    .text_size(px(11.))
                    .child(div().w(px(34.)).flex_none().text_color(Palette::faint()).text_right().pr(px(4.)).child(number.map(|n| n.to_string()).unwrap_or_default()))
                    .child(div().w(px(10.)).flex_none().text_color(fg).child(sign))
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_color(fg).child(clip(text, 400)))
                    .into_any_element()
            }
        }))
        .when(summary.truncated, |el| el.child(div().px(px(6.)).text_size(px(10.)).text_color(Palette::faint()).child("diff truncated")))
}

fn chips(labels: impl Iterator<Item = SharedString>, removable: Option<Entity<AgentPanel>>, _cx: &mut Context<AgentPanel>) -> impl IntoElement {
    div().flex().flex_wrap().gap(px(4.)).children(labels.enumerate().map(move |(n, label)| {
        let removable = removable.clone();
        div()
            .id(("attachment", n))
            .px(px(6.))
            .h(px(18.))
            .flex()
            .items_center()
            .gap(px(4.))
            .rounded(px(4.))
            .bg(ca(0x0a84ff, 0.14))
            .text_size(px(11.))
            .text_color(c(0x8ab4ff))
            .child(icon(IconName::FileText, 10., c(0x8ab4ff)))
            .child(label)
            .when_some(removable, |el, entity| {
                el.cursor_pointer().child(icon(IconName::Close, 9., c(0x8ab4ff))).on_click(move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        if n < this.attachments.len() {
                            this.attachments.remove(n);
                        }
                        cx.notify();
                    })
                })
            })
    }))
}

impl Render for AgentPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let access = Settings::get(cx).agent_full_access;
        if self.full_access.load(Ordering::Relaxed) != access {
            self.full_access.store(access, Ordering::Relaxed);
        }
        let input = self.input.read(cx).value().to_string();
        let suggestions: Vec<AvailableCommand> = matching_commands(&input, &self.commands).into_iter().cloned().collect();
        let entries: Vec<AnyElement> = {
            let mut out = Vec::with_capacity(self.entries.len());
            let entries = std::mem::take(&mut self.entries);
            for (ix, entry) in entries.iter().enumerate() {
                out.push(self.render_entry(ix, entry, cx));
            }
            self.entries = entries;
            out
        };
        let empty = self.entries.is_empty();
        div()
            .id("agent-panel")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(Palette::sidebar_bg())
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(PanelEvent::FocusEditor)))
            .child(self.render_header(cx))
            .map(|el| {
                if empty {
                    el.child(self.render_empty())
                } else {
                    el.child(
                        div()
                            .id("agent-transcript")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .py(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .children(entries)
                            .when(self.status == Status::Busy, |el| {
                                el.child(
                                    div()
                                        .mx(px(12.))
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_size(px(11.5))
                                        .text_color(Palette::muted())
                                        .child(div().size(px(6.)).rounded_full().bg(sparkle_gradient()))
                                        .child("Working…"),
                                )
                            })
                            .when(self.status == Status::Starting, |el| {
                                el.child(div().mx(px(12.)).text_size(px(11.5)).text_color(Palette::muted()).child("Starting agent…"))
                            }),
                    )
                }
            })
            .when(!self.plan.is_empty(), |el| el.child(self.render_plan()))
            .when(!suggestions.is_empty(), |el| el.child(self.render_suggestions(suggestions, cx)))
            .child(self.render_composer(cx))
    }
}

impl gpui_kit::Focusable for AgentPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_blocks_link_files_and_embed_selections() {
        let file = Attachment { path: PathBuf::from("/p/My File.rs"), selection: None };
        let selection = Attachment { path: PathBuf::from("/p/a.rs"), selection: Some(("let x = 1;".into(), 3, 4)) };
        let blocks = prompt_blocks("fix it", &[file.clone(), selection.clone()], true);
        assert_eq!(blocks[0], ContentBlock::Text { text: "fix it".into() });
        assert_eq!(blocks[1], ContentBlock::ResourceLink { uri: "file:///p/My%20File.rs".into(), name: "My File.rs".into() });
        let ContentBlock::Resource { resource } = &blocks[2] else { panic!() };
        assert_eq!(resource.uri, "file:///p/a.rs#L3-4");
        assert_eq!(resource.text.as_deref(), Some("let x = 1;"));
        let plain = prompt_blocks("  ", &[selection], false);
        assert_eq!(plain.len(), 1);
        assert!(matches!(&plain[0], ContentBlock::Text { text } if text.contains("let x = 1;") && text.starts_with("/p/a.rs:3-4")));
        assert_eq!(file.label(), "My File.rs");
    }

    #[test]
    fn slash_suggestions_filter_by_prefix() {
        let commands = vec![
            AvailableCommand { name: "review".into(), description: String::new(), input: None },
            AvailableCommand { name: "init".into(), description: String::new(), input: None },
        ];
        assert_eq!(matching_commands("/re", &commands).len(), 1);
        assert_eq!(matching_commands("/", &commands).len(), 2);
        assert!(matching_commands("/review now", &commands).is_empty());
        assert!(matching_commands("review", &commands).is_empty());
    }
}
