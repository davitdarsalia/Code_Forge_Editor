use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{
    DefinitionProvider as _, Editor, EditorState, Escape, Input, InputEvent, InputState, Position, RopeExt as _,
    ShowDocumentHandler,
};
use gpui_kit::base::{Spring, spring};
use gpui_kit::component::Icon;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, MouseButton, MouseMoveEvent, Context, Entity, ExternalPaths, FocusHandle,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, PromptLevel,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
    ScrollStrategy, UniformListScrollHandle, canvas, div, prelude::FluentBuilder as _, px, uniform_list,
};

use crate::file_icons;
use crate::build::BuildAction;
use crate::build_panel::{self, BuildEvent, BuildPanel};
use crate::files::{self, Node, Row};
use crate::git::{self, FileStatus, RepoState, TreeMarks};
use crate::preview::{self, Preview};
use crate::quick_open::{QuickOpen, QuickOpenEvent};
use crate::recent;
use crate::index_cache;
use crate::lsp;
use crate::diagnostics;
use crate::issues_panel;
use crate::search::{FileIndex, IndexProgress};
use crate::settings::Settings;
use crate::search_panel::{SearchEvent, SearchPanel};
use crate::terminal::Terminal;
use gpui_kit::Focusable as _;
use crate::theme::{Palette, ca};

const GIT_REFRESH_DELAY: Duration = Duration::from_millis(600);
const ACTIVATION_RESYNC_MIN: Duration = Duration::from_secs(2);
const INDEX_REWALK_MIN: Duration = Duration::from_secs(30);
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const TOOLBAR_HEIGHT: f32 = 38.;
const STATUS_HEIGHT: f32 = 22.;
const INDEX_TICK: Duration = Duration::from_millis(80);
const INDEX_BAR_DELAY: Duration = Duration::from_millis(120);
const MAX_LOG_LINES: usize = 200;
const MAX_HISTORY: usize = 100;
const LSP_SYNC_DELAY: Duration = Duration::from_millis(250);

struct Indexing {
    progress: Arc<IndexProgress>,
    started: Instant,
    expected: Option<usize>,
    _ticker: Task<()>,
}

struct LogLine {
    at: Duration,
    text: SharedString,
}

const ROW_HEIGHT: f32 = 22.;
const HANDLE_WIDTH: f32 = 6.;
const PANEL_SPRING: Spring = Spring::new(Duration::from_millis(420)).with_damping(0.82);

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Left,
    Right,
    Bottom,
    Preview,
}

struct Document {
    id: usize,
    path: PathBuf,
    editor: Entity<EditorState>,
    saved_hash: u64,
    dirty: bool,
    mtime: Option<SystemTime>,
    conflict: bool,
    save_task: Option<Task<()>>,
    save_seq: u64,
    applied_seq: u64,
    edit_gen: u64,
    saving: bool,
    resave: bool,
    gate: Arc<files::WriteGate>,
    definitions: Option<Rc<lsp::Definitions>>,
    _subscription: Subscription,
}

#[derive(Clone, PartialEq)]
struct NavPoint {
    path: PathBuf,
    position: Position,
}

#[derive(Clone, Copy, PartialEq)]
enum Navigator {
    Files,
    Search,
    Changes,
}

#[derive(Clone, Copy, PartialEq)]
enum BottomTab {
    Terminal,
    Build,
    Issues,
}

#[derive(Clone)]
enum Notice {
    Info(SharedString),
    Error(SharedString),
}

pub struct Workspace {
    root: Option<PathBuf>,
    recent: Vec<PathBuf>,
    tree: Option<Node>,
    rows: Rc<Vec<Row>>,
    docs: Vec<Document>,
    active: Option<usize>,
    next_doc_id: usize,
    navigator: Navigator,
    show_left: bool,
    show_right: bool,
    show_terminal: bool,
    terminal: Option<Entity<Terminal>>,
    bottom_tab: BottomTab,
    build: Entity<BuildPanel>,
    preview: Preview,
    show_preview: bool,
    preview_task: Option<Task<()>>,
    repo: Option<RepoState>,
    git_marks: Rc<TreeMarks>,
    last_sync: Option<Instant>,
    last_walk: Option<Instant>,
    git_busy: bool,
    git_task: Option<Task<()>>,
    selected_change: Option<(bool, PathBuf)>,
    commit_input: Entity<InputState>,
    clone_input: Entity<InputState>,
    clone_open: bool,
    clone_busy: bool,
    notice: Option<Notice>,
    notice_task: Option<Task<()>>,
    format_task: Option<Task<()>>,
    reload_task: Option<Task<()>>,
    tree_task: Option<Task<()>>,
    index: Option<Arc<FileIndex>>,
    index_task: Option<Task<()>>,
    cache_task: Option<Task<()>>,
    indexing: Option<Indexing>,
    cache_in_sync: bool,
    index_log: Vec<LogLine>,
    index_log_open: bool,
    log_epoch: Instant,
    search: Entity<SearchPanel>,
    quick_open: Entity<QuickOpen>,
    quick_open_visible: bool,
    left_width: f32,
    right_width: f32,
    terminal_height: f32,
    preview_width: f32,
    drag: Option<Drag>,
    focus: FocusHandle,
    tree_scroll: UniformListScrollHandle,
    soft_wrap: bool,
    lsp: Rc<lsp::Manager>,
    definition_task: Option<Task<()>>,
    issues: diagnostics::Store,
    lsp_sync: std::collections::HashMap<usize, Task<()>>,
    _lsp_events: Task<()>,
    back: Vec<NavPoint>,
    forward: Vec<NavPoint>,
    _subscriptions: Vec<Subscription>,
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[derive(Debug, PartialEq)]
enum ExternalChange {
    None,
    Touched(Option<SystemTime>),
    Changed,
}

fn external_change(path: &Path, known_mtime: Option<SystemTime>, saved_hash: u64) -> ExternalChange {
    let current = mtime(path);
    if current.is_none() || current == known_mtime {
        return ExternalChange::None;
    }
    match std::fs::read_to_string(path) {
        Ok(text) if hash_text(&text) == saved_hash => ExternalChange::Touched(current),
        Ok(_) => ExternalChange::Changed,
        Err(_) => ExternalChange::None,
    }
}

struct LoadedText {
    text: String,
    saved_hash: u64,
    language: &'static str,
    mtime: Option<SystemTime>,
}

fn load_text(path: &Path) -> Result<LoadedText, String> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > MAX_FILE_BYTES {
        return Err(format!("{} is too large to open", path.display()));
    }
    let before = mtime(path);
    let bytes = std::fs::read(path).map_err(|err| format!("Cannot open {}: {err}", path.display()))?;
    if files::looks_binary(&bytes) {
        return Err(format!("{} looks like a binary file", path.display()));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{} is not UTF-8 text; not opened to avoid corrupting it", path.display()))?;
    let saved_hash = hash_text(&text);
    let language = files::detect_language(path, &text);
    Ok(LoadedText { text, saved_hash, language, mtime: before })
}

#[derive(Debug, PartialEq)]
enum SaveOutcome {
    Unchanged,
    Conflict,
    Superseded,
    Written { hash: u64, mtime: Option<SystemTime> },
    Failed { message: String, touched: Option<Option<SystemTime>> },
}

fn run_save(
    text: &str,
    path: &Path,
    known_mtime: Option<SystemTime>,
    saved_hash: u64,
    check_external: bool,
    seq: u64,
    gate: &files::WriteGate,
) -> SaveOutcome {
    let hash = hash_text(text);
    if hash == saved_hash {
        return SaveOutcome::Unchanged;
    }
    let mut touched = None;
    if check_external {
        match external_change(path, known_mtime, saved_hash) {
            ExternalChange::None => {}
            ExternalChange::Touched(mtime) => touched = Some(mtime),
            ExternalChange::Changed => return SaveOutcome::Conflict,
        }
    }
    match gate.write(seq, path, text.as_bytes()) {
        Ok(true) => SaveOutcome::Written { hash, mtime: mtime(path) },
        Ok(false) => SaveOutcome::Superseded,
        Err(err) => SaveOutcome::Failed { message: format!("Save failed for {}: {err}", path.display()), touched },
    }
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::startup::mark("Workspace::new start");
        let commit_input = cx.new(|cx| InputState::new(window, cx).placeholder("Commit message"));
        let clone_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://github.com/user/repo.git")
        });
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe_in(&commit_input, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event {
                this.commit(window, cx);
            }
        }));
        subscriptions.push(cx.subscribe_in(&clone_input, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.start_clone(window, cx);
            }
        }));
        let search = cx.new(|cx| SearchPanel::new(window, cx));
        subscriptions.push(cx.subscribe_in(&search, window, |this, _, event, window, cx| match event {
            SearchEvent::Open { path, line, column } => this.open_at(path, *line, *column, window, cx),
        }));
        let build = cx.new(|cx| BuildPanel::new(window, cx));
        subscriptions.push(cx.subscribe_in(&build, window, |this, _, event, window, cx| match event {
            BuildEvent::Open { path, line, column } => this.open_at(path, *line, *column, window, cx),
            BuildEvent::Started => {
                this.show_terminal = true;
                this.bottom_tab = BottomTab::Build;
                cx.notify();
            }
        }));
        subscriptions.push(cx.observe(&build, |_, _, cx| cx.notify()));
        let quick_open = cx.new(|cx| QuickOpen::new(window, cx));
        subscriptions.push(cx.subscribe_in(&quick_open, window, |this, _, event, window, cx| {
            this.quick_open_visible = false;
            match event {
                QuickOpenEvent::Open(path) => this.open_file(&path.clone(), window, cx),
                QuickOpenEvent::OpenAt { path, line, column } => this.open_at(&path.clone(), *line, *column, window, cx),
                QuickOpenEvent::Run(action) => {
                    this.focus_active_editor(window, cx);
                    window.dispatch_action(action.boxed_clone(), cx);
                }
                QuickOpenEvent::Dismiss => this.focus_active_editor(window, cx),
            }
            cx.notify();
        }));
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            if crate::startup::enabled() {
                crate::startup::mark(&format!("window active={}", window.is_window_active()));
            }
            if window.is_window_active() {
                this.reload_changed_documents(window, cx);
                if this.last_sync.is_some_and(|at| at.elapsed() < ACTIVATION_RESYNC_MIN) {
                    return;
                }
                this.last_sync = Some(Instant::now());
                this.refresh_tree(cx);
                this.refresh_git(cx);
                if this.index_is_stale() {
                    this.reindex(cx);
                }
            } else {
                let ids: Vec<usize> = this.docs.iter().filter(|d| d.dirty).map(|d| d.id).collect();
                for id in ids {
                    this.save_doc_async(id, cx);
                }
            }
        }));
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            if let Some(workspace) = weak.upgrade() {
                workspace.update(cx, |ws, cx| ws.save_all(true, cx));
            }
            true
        });
        subscriptions.push(cx.on_app_quit(|this, cx| {
            this.save_all(true, cx);
            async {}
        }));

        let lsp = Rc::new(lsp::Manager::default());
        let events = lsp.events();
        let lsp_events = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                if this.update(cx, |this, cx| this.on_lsp_event(event, cx)).is_err() {
                    break;
                }
            }
        });

        crate::startup::mark("Workspace::new children built");
        Workspace {
            root: None,
            recent: recent::load(),
            tree: None,
            rows: Rc::new(Vec::new()),
            docs: Vec::new(),
            active: None,
            next_doc_id: 0,
            navigator: Navigator::Files,
            show_left: true,
            show_right: Settings::get(cx).show_inspector,
            show_terminal: true,
            terminal: None,
            bottom_tab: BottomTab::Terminal,
            build,
            preview: Preview::new(),
            show_preview: true,
            preview_task: None,
            repo: None,
            git_marks: Rc::default(),
            last_sync: None,
            last_walk: None,
            git_busy: false,
            git_task: None,
            selected_change: None,
            commit_input,
            clone_input,
            clone_open: false,
            clone_busy: false,
            notice: None,
            notice_task: None,
            format_task: None,
            reload_task: None,
            tree_task: None,
            index: None,
            index_task: None,
            cache_task: None,
            indexing: None,
            cache_in_sync: false,
            index_log: Vec::new(),
            index_log_open: false,
            log_epoch: Instant::now(),
            search,
            quick_open,
            quick_open_visible: false,
            left_width: 250.,
            right_width: 260.,
            terminal_height: 220.,
            preview_width: 420.,
            drag: None,
            focus: cx.focus_handle(),
            tree_scroll: UniformListScrollHandle::new(),
            soft_wrap: Settings::get(cx).soft_wrap,
            lsp,
            definition_task: None,
            issues: diagnostics::Store::default(),
            lsp_sync: std::collections::HashMap::new(),
            _lsp_events: lsp_events,
            back: Vec::new(),
            forward: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn open_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        for path in paths {
            let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            if path.is_dir() {
                self.open_project(path, window, cx);
            } else if path.is_file() {
                let inside = self.root.as_ref().is_some_and(|root| path.starts_with(root));
                if !inside {
                    let project = git_root(&path).or_else(|| path.parent().map(Path::to_path_buf));
                    if let Some(project) = project {
                        self.open_project(project, window, cx);
                    }
                }
                self.open_file(&path, window, cx);
            }
        }
    }

    pub fn open_project(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.as_ref() == Some(&root) {
            return;
        }
        if !self.save_all(false, cx) {
            self.notify_error("Some files could not be saved; save (⌘S) or close them before switching projects", cx);
            return;
        }
        self.docs.clear();
        self.active = None;
        self.back.clear();
        self.forward.clear();
        self.lsp.set_project(Some(root.clone()));
        self.issues.clear();
        self.lsp_sync.clear();
        self.repo = None;
        self.git_marks = Rc::default();
        self.terminal = None;
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        window.set_window_title(&name);
        crate::startup::mark("open_project: start");
        self.tree = Some(Node::root(&root));
        self.rebuild_rows();
        crate::startup::mark("open_project: tree root loaded");
        self.root = Some(root.clone());
        self.recent = recent::push(&root);
        cx.on_next_frame(window, |_, _, cx| crate::set_menus(cx));
        crate::startup::mark("open_project: recent + menus");
        cx.on_next_frame(window, |this, window, cx| {
            if this.show_terminal && this.root.is_some() {
                this.ensure_terminal(window, cx);
                crate::startup::mark("terminal spawned (after first frame)");
                cx.notify();
            }
        });
        self.last_sync = Some(Instant::now());
        self.refresh_git(cx);
        self.build.update(cx, |build, cx| build.set_root(root.clone(), cx));
        self.index = None;
        self.cache_in_sync = false;
        self.index_log.clear();
        self.log_epoch = Instant::now();
        self.search.update(cx, |search, cx| search.set_index(None, cx));
        self.reindex(cx);
        crate::startup::mark("open_project: done (git/build/index queued)");
        cx.notify();
    }

    fn index_is_stale(&self) -> bool {
        self.indexing.is_none() && self.last_walk.is_none_or(|at| at.elapsed() >= INDEX_REWALK_MIN)
    }

    fn reindex(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        self.last_walk = Some(Instant::now());
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

        if self.index.is_none() {
            let cache_root = root.clone();
            let project = root.clone();
            let load = cx.background_spawn(async move {
                let started = Instant::now();
                (index_cache::load(&cache_root), started.elapsed())
            });
            self.cache_task = Some(cx.spawn(async move |this, cx| {
                let (cached, took) = load.await;
                let _ = this.update(cx, |this, cx| {
                    if this.root.as_ref() != Some(&project) {
                        return;
                    }
                    match cached {
                        Some(index) if this.index.is_none() => {
                            this.log(format!("Loaded {} files from cache in {} ms", index.files.len(), took.as_millis()));
                            if let Some(indexing) = this.indexing.as_mut() {
                                indexing.expected = Some(index.files.len());
                            }
                            this.cache_in_sync = true;
                            this.apply_index(Arc::new(index), cx);
                        }
                        Some(_) => {}
                        None => this.log("No cache yet, first full scan of this project"),
                    }
                    cx.notify();
                });
            }));
        }

        let progress = Arc::new(IndexProgress::default());
        self.log(format!("Scanning {name}…"));
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(INDEX_TICK).await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    this.indexing.is_some()
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        });
        self.indexing = Some(Indexing {
            progress: progress.clone(),
            started: Instant::now(),
            expected: self.index.as_ref().map(|i| i.files.len()),
            _ticker: ticker,
        });
        let walk_root = root.clone();
        let task = cx.background_spawn(async move {
            let started = Instant::now();
            let index = crate::search::build_index_with_progress(&walk_root, &progress);
            crate::startup::mark("bg: index walk done");
            (index, started.elapsed())
        });
        self.index_task = Some(cx.spawn(async move |this, cx| {
            let (index, took) = task.await;
            let index = Arc::new(index);
            let _ = this.update(cx, |this, cx| {
                if this.root.as_ref() != Some(&index.root) {
                    return;
                }
                this.indexing = None;
                let (added, removed) = match this.index.as_ref() {
                    Some(old) => index_cache::diff(&old.files, &index.files),
                    None => (index.files.len(), 0),
                };
                this.log(format!(
                    "Indexed {} files in {} ms (+{added} −{removed})",
                    index.files.len(),
                    took.as_millis()
                ));
                if added + removed > 0 || !this.cache_in_sync {
                    this.cache_in_sync = true;
                    let to_save = index.clone();
                    let save = cx.background_spawn(async move { index_cache::save(&to_save) });
                    cx.spawn(async move |this, cx| {
                        let result = save.await;
                        let _ = this.update(cx, |this, cx| {
                            match result {
                                Ok(path) => this.log(format!("Cache saved → {}", shorten_home(&path))),
                                Err(err) => {
                                    this.cache_in_sync = false;
                                    this.log(format!("Cache save failed: {err}"));
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                } else {
                    this.log("Cache already up to date");
                }
                this.apply_index(index, cx);
            });
        }));
    }

    fn apply_index(&mut self, index: Arc<FileIndex>, cx: &mut Context<Self>) {
        if self.index.is_none() && crate::startup::enabled() {
            crate::startup::mark(&format!("index ready ({} files)", index.files.len()));
        }
        self.search.update(cx, |search, cx| search.set_index(Some(index.clone()), cx));
        self.index = Some(index);
        cx.notify();
    }

    fn log(&mut self, message: impl Into<String>) {
        self.index_log.push(LogLine { at: self.log_epoch.elapsed(), text: message.into().into() });
        if self.index_log.len() > MAX_LOG_LINES {
            self.index_log.remove(0);
        }
    }

    fn toggle_index_log(&mut self, cx: &mut Context<Self>) {
        self.index_log_open = !self.index_log_open;
        cx.notify();
    }

    pub fn show_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.quick_open_visible = true;
        if self.index_is_stale() {
            self.reindex(cx);
        }
        let index = self.index.clone();
        let active = self.active_doc().map(|doc| (doc.path.clone(), doc.editor.clone()));
        self.quick_open.update(cx, |q, cx| q.show(index, active, window, cx));
        cx.notify();
    }

    pub fn show_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.show_left = true;
        self.navigator = Navigator::Search;
        self.search.update(cx, |search, cx| search.focus(window, cx));
        cx.notify();
    }

    fn open_at(&mut self, path: &Path, line: u32, column: u32, window: &mut Window, cx: &mut Context<Self>) {
        let owned = path.to_path_buf();
        self.open_file_then(path, window, cx, move |this, window, cx| {
            let Some(doc) = this.active.and_then(|ix| this.docs.get(ix)) else { return };
            if doc.path != owned {
                return;
            }
            doc.editor.update(cx, |state, cx| {
                state.set_cursor_position(gpui_kit::component::input::Position { line, character: column }, window, cx)
            });
        });
    }

    fn focus_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active.and_then(|ix| self.docs.get(ix)) {
            Some(doc) => doc.editor.update(cx, |state, cx| state.focus(window, cx)),
            None => window.focus(&self.focus, cx),
        }
    }

    pub fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file_then(path, window, cx, |_, _, _| {});
    }

    fn open_file_then(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if let Some(ix) = self.docs.iter().position(|d| d.path == path) {
            self.activate(ix, window, cx);
            then(self, window, cx);
            return;
        }
        let path = path.to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let read_path = path.clone();
            let loaded = cx.background_spawn(async move { load_text(&read_path) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let loaded = match loaded {
                    Ok(loaded) => loaded,
                    Err(message) => {
                        this.notify_error(message, cx);
                        return;
                    }
                };
                this.finish_open(&path, loaded, window, cx);
                then(this, window, cx);
            });
        })
        .detach();
    }

    fn finish_open(&mut self, path: &Path, loaded: LoadedText, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.docs.iter().position(|d| d.path == path) {
            self.activate(ix, window, cx);
            return;
        }
        let LoadedText { text, saved_hash, language, mtime: loaded_mtime } = loaded;
        let definitions = lsp::definitions_for(Rc::downgrade(&self.lsp), language, path);
        let show_document = self.show_document_handler(cx);
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                .folding(true)
                .soft_wrap(self.soft_wrap)
                .default_value(text);
            if let Some(definitions) = definitions.clone() {
                let lsp = state.lsp_mut();
                lsp.definition_provider = Some(definitions);
                lsp.show_document = Some(show_document);
            }
            state
        });
        let id = self.next_doc_id;
        self.next_doc_id += 1;
        let subscription = cx.subscribe_in(&editor, window, move |this, _, event, window, cx| match event {
            InputEvent::Change => {
                if let Some(definitions) = this.docs.iter().find(|d| d.id == id).and_then(|d| d.definitions.as_ref()) {
                    definitions.stale.set(true);
                }
                this.schedule_lsp_sync(id, cx);
                this.schedule_save(id, cx);
                this.schedule_preview(id, window, cx);
            }
            InputEvent::Blur => {
                if this.docs.iter().any(|d| d.id == id && d.dirty) {
                    this.save_doc_async(id, cx);
                }
            }
            _ => {}
        });
        self.issues.attach(path, &editor, cx);
        self.docs.push(Document {
            id,
            path: path.to_path_buf(),
            editor,
            saved_hash,
            dirty: false,
            mtime: loaded_mtime,
            conflict: false,
            save_task: None,
            save_seq: 0,
            applied_seq: 0,
            edit_gen: 0,
            saving: false,
            resave: false,
            gate: Arc::new(files::WriteGate::default()),
            definitions,
            _subscription: subscription,
        });
        if let Some(tree) = self.tree.as_mut() {
            tree.reveal(path);
            self.rebuild_rows();
        }
        let ix = self.docs.len() - 1;
        self.activate(ix, window, cx);
    }

    fn show_document_handler(&self, cx: &mut Context<Self>) -> ShowDocumentHandler {
        let workspace = cx.weak_entity();
        Rc::new(move |params, window, cx| {
            let Some(path) = lsp::uri_to_path(params.uri.as_str()) else { return false };
            let start = params.selection.map(|range| range.start).unwrap_or_default();
            let workspace = workspace.clone();
            window.defer(cx, move |window, cx| {
                let _ = workspace.update(cx, |ws, cx| ws.jump_to(&path, start, window, cx));
            });
            true
        })
    }

    fn nav_point(&self, cx: &App) -> Option<NavPoint> {
        let doc = self.active_doc()?;
        let state = doc.editor.read(cx);
        Some(NavPoint { path: doc.path.clone(), position: state.text().offset_to_position(state.cursor()) })
    }

    fn push_history(stack: &mut Vec<NavPoint>, point: NavPoint) {
        if stack.last() != Some(&point) {
            stack.push(point);
            if stack.len() > MAX_HISTORY {
                stack.remove(0);
            }
        }
    }

    fn jump_to(&mut self, path: &Path, lsp_start: Position, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.nav_point(cx);
        self.open_navigated(path, window, cx, move |this, window, cx| {
            let Some(doc) = this.active_doc() else { return };
            let position = {
                let text = doc.editor.read(cx).text();
                let line = if (lsp_start.line as usize) < text.lines_len() {
                    text.slice_line(lsp_start.line as usize).to_string()
                } else {
                    String::new()
                };
                Position { line: lsp_start.line, character: lsp::char_column(&line, lsp_start.character) }
            };
            doc.editor.update(cx, |state, cx| state.set_cursor_position(position, window, cx));
            if let Some(from) = from {
                Self::push_history(&mut this.back, from);
                this.forward.clear();
            }
        });
    }

    fn open_navigated(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let owned = path.to_path_buf();
        self.open_file_then(path, window, cx, move |this, window, cx| {
            let Some(doc) = this.active_doc().filter(|doc| doc.path == owned) else { return };
            if this.lsp.is_external(&owned) {
                doc.editor.update(cx, |state, cx| state.set_readonly(true, cx));
            }
            then(this, window, cx);
        });
    }

    pub fn go_to_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc() else { return };
        let Some(definitions) = doc.definitions.clone() else {
            self.notify_info(format!("Go to Definition works in {} files", lsp::supported_languages()), cx);
            return;
        };
        let (text, offset) = {
            let state = doc.editor.read(cx);
            (state.text().clone(), state.cursor())
        };
        let request = definitions.definitions(&text, offset, window, cx);
        let kind = definitions.kind;
        let indexing = format!("{} is still indexing the project; try again shortly", kind.map_or("the symbol index", |k| k.server_name()));
        self.definition_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.definition_task = None;
                match result {
                    Ok(links) => {
                        let target = links
                            .first()
                            .and_then(|link| Some((lsp::uri_to_path(link.target_uri.as_str())?, link.target_selection_range.start)));
                        match target {
                            Some((path, start)) => this.jump_to(&path, start, window, cx),
                            None if kind.is_some_and(|kind| this.lsp.indexing(kind)) => this.notify_info(indexing, cx),
                            None => this.notify_info("No definition found", cx),
                        }
                    }
                    Err(err) if matches!(err.downcast_ref(), Some(lsp::ServerError::Busy)) => this.notify_info(indexing, cx),
                    Err(err) => this.notify_error(err.to_string(), cx),
                }
            });
        }));
    }

    pub fn step_history(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let target = if back { self.back.pop() } else { self.forward.pop() };
        let Some(target) = target else { return };
        if let Some(here) = self.nav_point(cx) {
            Self::push_history(if back { &mut self.forward } else { &mut self.back }, here);
        }
        let position = target.position;
        self.open_navigated(&target.path, window, cx, move |this, window, cx| {
            if let Some(doc) = this.active_doc() {
                doc.editor.update(cx, |state, cx| state.set_cursor_position(position, window, cx));
            }
        });
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = Some(ix);
        let editor = self.docs[ix].editor.clone();
        editor.update(cx, |state, cx| state.focus(window, cx));
        self.sync_lsp(self.docs[ix].id, cx);
        self.refresh_preview(window, cx);
        cx.notify();
    }

    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        let id = self.docs[ix].id;
        if !self.save_doc(id, false, cx) {
            let name = self.docs[ix].path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("“{name}” was not saved"),
                Some("It could not be written or it changed on disk. Close it and lose your edits?"),
                &["Close Without Saving", "Cancel"],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                if answer.await == Ok(0) {
                    let result = this.update_in(cx, |this, window, cx| this.remove_doc(id, window, cx));
                    if let Err(err) = result {
                        eprintln!("forge: cannot close tab: {err}");
                    }
                }
            })
            .detach();
            return;
        }
        self.remove_doc(id, window, cx);
    }

    fn remove_doc(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.docs.iter().position(|d| d.id == id) else { return };
        let doc = self.docs.remove(ix);
        self.lsp.close(&doc.path);
        self.lsp_sync.remove(&id);
        self.issues.forget(&doc.path);
        self.active = if self.docs.is_empty() { None } else { Some(ix.min(self.docs.len() - 1)) };
        if let Some(ix) = self.active {
            self.activate(ix, window, cx);
        }
        cx.notify();
    }

    fn close_doc(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = Some(ix);
        self.close_active(window, cx);
    }

    fn schedule_save(&mut self, id: usize, cx: &mut Context<Self>) {
        let delay = Settings::get(cx).autosave_delay();
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return };
        let became_dirty = !doc.dirty;
        doc.dirty = true;
        doc.edit_gen += 1;
        doc.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| this.save_doc_async(id, cx));
        }));
        if became_dirty {
            cx.notify();
        }
    }

    fn save_doc_async(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(ix) = self.docs.iter().position(|d| d.id == id) else { return };
        let doc = &mut self.docs[ix];
        doc.save_task = None;
        if doc.saving {
            doc.resave = true;
            return;
        }
        doc.saving = true;
        doc.save_seq += 1;
        let seq = doc.save_seq;
        let edit_gen = doc.edit_gen;
        let rope = doc.editor.read(cx).text().clone();
        let (path, known_mtime, saved_hash, gate) = (doc.path.clone(), doc.mtime, doc.saved_hash, doc.gate.clone());
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let text = rope.to_string();
                    run_save(&text, &path, known_mtime, saved_hash, true, seq, &gate)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_save(id, seq, edit_gen, outcome, cx);
                let again = this.docs.iter_mut().find(|d| d.id == id).is_some_and(|doc| {
                    doc.saving = false;
                    std::mem::take(&mut doc.resave)
                });
                if again {
                    this.save_doc_async(id, cx);
                }
            });
        })
        .detach();
    }

    fn save_doc(&mut self, id: usize, force: bool, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.docs.iter().position(|d| d.id == id) else { return true };
        let text = self.docs[ix].editor.read(cx).value();
        let doc = &mut self.docs[ix];
        doc.save_task = None;
        doc.save_seq += 1;
        let seq = doc.save_seq;
        let edit_gen = doc.edit_gen;
        let check = !force && !doc.saving;
        let outcome = run_save(&text, &doc.path, doc.mtime, doc.saved_hash, check, seq, &doc.gate);
        self.apply_save(id, seq, edit_gen, outcome, cx)
    }

    fn apply_save(&mut self, id: usize, seq: u64, edit_gen: u64, outcome: SaveOutcome, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.docs.iter().position(|d| d.id == id) else { return true };
        let doc = &mut self.docs[ix];
        if seq < doc.applied_seq {
            return true;
        }
        doc.applied_seq = seq;
        let current = doc.edit_gen == edit_gen;
        let written = matches!(outcome, SaveOutcome::Written { .. }).then(|| doc.path.clone());
        let saved = match outcome {
            SaveOutcome::Unchanged => {
                if doc.dirty && current {
                    doc.dirty = false;
                }
                true
            }
            SaveOutcome::Superseded => true,
            SaveOutcome::Conflict => {
                let first = !doc.conflict;
                doc.conflict = true;
                if first {
                    let message = format!(
                        "{} changed on disk; your edits are not saved. ⌘S overwrites it.",
                        doc.path.display()
                    );
                    self.notify_error(message, cx);
                }
                false
            }
            SaveOutcome::Written { hash, mtime } => {
                doc.saved_hash = hash;
                if current {
                    doc.dirty = false;
                }
                doc.conflict = false;
                doc.mtime = mtime;
                self.schedule_git_refresh(cx);
                true
            }
            SaveOutcome::Failed { message, touched } => {
                if let Some(touched) = touched {
                    doc.mtime = touched;
                }
                self.notify_error(message, cx);
                false
            }
        };
        if let Some(path) = written {
            self.sync_lsp(id, cx);
            self.lsp.saved(&path);
        }
        cx.notify();
        saved
    }

    pub fn save_all(&mut self, force: bool, cx: &mut Context<Self>) -> bool {
        let ids: Vec<usize> = self.docs.iter().map(|d| d.id).collect();
        let mut all = true;
        for id in ids {
            all &= self.save_doc(id, force, cx);
        }
        all
    }

    fn reload_changed_documents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let probes: Vec<(usize, PathBuf, Option<SystemTime>, u64)> = self
            .docs
            .iter()
            .filter(|doc| !doc.dirty)
            .map(|doc| (doc.id, doc.path.clone(), doc.mtime, doc.saved_hash))
            .collect();
        if probes.is_empty() {
            return;
        }
        self.reload_task = Some(cx.spawn_in(window, async move |this, cx| {
            let changes = cx
                .background_spawn(async move {
                    let mut changes = Vec::new();
                    for (id, path, known, saved_hash) in probes {
                        let current = mtime(&path);
                        if current.is_none() || current == known {
                            continue;
                        }
                        let Ok(text) = std::fs::read_to_string(&path) else { continue };
                        let hash = hash_text(&text);
                        changes.push((id, current, hash, (hash != saved_hash).then_some(text)));
                    }
                    changes
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.reload_task = None;
                let mut reloaded = Vec::new();
                for (id, current, hash, text) in changes {
                    let Some(doc) = this.docs.iter_mut().find(|d| d.id == id) else { continue };
                    if doc.dirty || doc.saving {
                        continue;
                    }
                    doc.mtime = current;
                    let Some(text) = text else { continue };
                    doc.saved_hash = hash;
                    doc.editor.update(cx, |state, cx| state.set_value(text, window, cx));
                    if let Some(definitions) = doc.definitions.as_ref() {
                        definitions.stale.set(true);
                        reloaded.push(id);
                    }
                }
                for id in reloaded {
                    this.schedule_lsp_sync(id, cx);
                }
                this.refresh_preview(window, cx);
                cx.notify();
            });
        }));
    }

    pub fn format_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc() else { return };
        let (id, path) = (doc.id, doc.path.clone());
        let text = doc.editor.read(cx).value().to_string();
        let language = files::language_for(&path);
        let before = doc.edit_gen;
        self.format_task = Some(cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { crate::format::format(&path, language, &text) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| this.apply_format(id, before, outcome, window, cx));
        }));
    }

    fn apply_format(
        &mut self,
        id: usize,
        before: u64,
        outcome: crate::format::Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::format::Outcome;
        self.format_task = None;
        let Some(doc) = self.docs.iter().find(|d| d.id == id) else { return };
        let editor = doc.editor.clone();
        let changed_meanwhile = doc.edit_gen != before;
        let (text, notice) = match outcome {
            Outcome::Changed { text, notice } => (text, notice),
            Outcome::Unchanged { notice } => {
                if let Some(notice) = notice {
                    self.notify_info(notice, cx);
                }
                return;
            }
            Outcome::Failed(message) => {
                self.notify_error(message, cx);
                return;
            }
        };
        if changed_meanwhile {
            self.notify_info("File changed while formatting; press ⌘I again", cx);
            return;
        }
        editor.update(cx, |state, cx| {
            let cursor = state.cursor_position();
            state.replace_all(text.clone(), window, cx);
            let last_line = text.split('\n').count().saturating_sub(1) as u32;
            let line = cursor.line.min(last_line);
            state.set_cursor_position(Position { line, character: cursor.character }, window, cx);
        });
        self.schedule_save(id, cx);
        if let Some(notice) = notice {
            self.notify_info(notice, cx);
        }
    }

    fn active_doc(&self) -> Option<&Document> {
        self.active.and_then(|ix| self.docs.get(ix))
    }

    fn preview_open(&self) -> bool {
        self.show_preview && self.active_doc().is_some_and(|doc| preview::supports(&doc.path))
    }

    fn schedule_preview(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.preview_open() || self.active_doc().map(|d| d.id) != Some(id) {
            return;
        }
        self.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(preview::UPDATE_DELAY).await;
            let _ = this.update_in(cx, |this, window, cx| this.refresh_preview(window, cx));
        }));
    }

    fn refresh_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.preview_open() {
            return;
        }
        let Some(doc) = self.active_doc() else { return };
        let source = doc.editor.read(cx).value();
        let path = doc.path.clone();
        self.preview.update(&path, &source, window);
        cx.notify();
    }

    pub fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_preview = !self.show_preview;
        self.refresh_preview(window, cx);
        cx.notify();
    }

    fn rebuild_rows(&mut self) {
        self.rows = Rc::new(self.tree.as_ref().map(Node::rows).unwrap_or_default());
    }

    fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        let Some(dirs) = self.tree.as_ref().map(Node::loaded_dirs) else { return };
        self.tree_task = Some(cx.spawn(async move |this, cx| {
            let mut listings = cx
                .background_spawn(async move {
                    dirs.into_iter().map(|dir| { let list = files::list_dir(&dir); (dir, list) }).collect::<std::collections::HashMap<_, _>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.tree_task = None;
                if let Some(tree) = this.tree.as_mut() {
                    tree.apply_listings(&mut listings);
                    this.rebuild_rows();
                    cx.notify();
                }
            });
        }));
    }

    fn click_row(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        if row.is_dir {
            if let Some(tree) = self.tree.as_mut() {
                tree.toggle(&row.path);
                self.rebuild_rows();
                cx.notify();
            }
        } else {
            self.open_file(&row.path.clone(), window, cx);
        }
    }

    fn schedule_git_refresh(&mut self, cx: &mut Context<Self>) {
        if self.git_busy {
            return;
        }
        self.git_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GIT_REFRESH_DELAY).await;
            let _ = this.update(cx, |this, cx| this.refresh_git(cx));
        }));
    }

    pub fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let task = cx.background_spawn(async move {
            let state = if git::is_repo(&root) { git::read_state(&root).ok() } else { None };
            let marks = state.as_ref().map(RepoState::tree_marks).unwrap_or_default();
            crate::startup::mark("bg: git state read");
            (state, marks)
        });
        self.git_task = Some(cx.spawn(async move |this, cx| {
            let (state, marks) = task.await;
            let _ = this.update(cx, |this, cx| {
                static GIT_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                crate::startup::mark_once(&GIT_READY, "git ready");
                this.git_marks = Rc::new(marks);
                this.repo = state;
                cx.notify();
            });
        }));
    }

    fn run_git(
        &mut self,
        label: &'static str,
        op: impl FnOnce(&Path) -> anyhow::Result<String> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else { return };
        if self.git_busy {
            return;
        }
        self.save_all(false, cx);
        self.git_busy = true;
        self.notice = Some(Notice::Info(format!("{label}…").into()));
        cx.notify();
        let task = cx.background_spawn(async move { op(&root) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.git_busy = false;
                match result {
                    Ok(_) => this.notify_info(format!("{label} finished"), cx),
                    Err(err) => this.notify_error(format!("{label} failed: {err}"), cx),
                }
                this.reload_changed_documents(window, cx);
                this.refresh_tree(cx);
                this.refresh_git(cx);
            });
        })
        .detach();
    }

    pub fn run_remote(
        &mut self,
        label: &'static str,
        op: fn(&Path) -> anyhow::Result<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.repo.is_none() {
            if self.root.is_some() {
                self.notify_error("Not a git repository", cx);
            }
            return;
        }
        self.run_git(label, op, window, cx);
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.commit_input.read(cx).value().trim().to_string();
        if message.is_empty() {
            self.notify_error("Write a commit message first", cx);
            return;
        }
        self.commit_input.update(cx, |input, cx| input.set_value("", window, cx));
        self.run_git("Commit", move |root| git::commit(root, &message, true), window, cx);
    }

    fn confirm_discard(&mut self, change: git::Change, window: &mut Window, cx: &mut Context<Self>) {
        let name = change.path.display().to_string();
        let detail = if change.status == FileStatus::Untracked {
            "The file is untracked and will be deleted. This cannot be undone."
        } else {
            "Your changes to this file will be lost. This cannot be undone."
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Discard changes to “{name}”?"),
            Some(detail),
            &["Discard Changes", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let result = this.update_in(cx, |this, window, cx| {
                this.run_git("Discard", move |root| git::discard(root, &change).map(|_| String::new()), window, cx)
            });
            if let Err(err) = result {
                eprintln!("forge: discard dropped: {err}");
            }
        })
        .detach();
    }

    fn run_index_op(
        &mut self,
        op: impl FnOnce(&Path) -> anyhow::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else { return };
        if self.git_busy {
            return;
        }
        self.save_all(false, cx);
        let task = cx.background_spawn(async move { op(&root) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(err) = result {
                    this.notify_error(format!("Git failed: {err}"), cx);
                }
                this.refresh_git(cx);
            });
        })
        .detach();
    }

    pub fn show_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clone_open = true;
        self.clone_input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn close_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.clone_busy {
            return;
        }
        self.clone_open = false;
        self.focus_active_editor(window, cx);
        cx.notify();
    }

    fn start_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.clone_input.read(cx).value().trim().to_string();
        if url.is_empty() || self.clone_busy {
            return;
        }
        if git::repo_name(&url).is_none() {
            self.notify_error("That does not look like a repository URL", cx);
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(parent) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                this.clone_busy = true;
                this.notice = Some(Notice::Info(format!("Cloning {url}…").into()));
                cx.notify();
            });
            let result = cx.background_spawn(async move { git::clone(&url, &parent) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.clone_busy = false;
                match result {
                    Ok(path) => {
                        this.clone_open = false;
                        this.clone_input.update(cx, |input, cx| input.set_value("", window, cx));
                        this.notify_info("Clone finished", cx);
                        this.open_project(path, window, cx);
                    }
                    Err(err) => this.notify_error(format!("Clone failed: {err}"), cx),
                }
            });
        })
        .detach();
    }

    pub fn toggle_left(&mut self, cx: &mut Context<Self>) {
        self.show_left = !self.show_left;
        cx.notify();
    }

    pub fn toggle_right(&mut self, cx: &mut Context<Self>) {
        self.show_right = !self.show_right;
        cx.notify();
    }

    pub fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_terminal = !self.show_terminal;
        if self.show_terminal {
            self.ensure_terminal(window, cx);
            if self.bottom_tab == BottomTab::Terminal {
                if let Some(terminal) = self.terminal.as_ref() {
                    let focus = terminal.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                }
            }
        } else {
            self.focus_active_editor(window, cx);
        }
        cx.notify();
    }

    fn ensure_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal.is_some() {
            return;
        }
        let cwd = self.root.clone().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("/"));
        self.terminal = Some(cx.new(|cx| Terminal::new(&cwd, window, cx)));
    }

    pub fn restart_terminal(&mut self, cx: &mut Context<Self>) {
        let cwd = self.root.clone().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("/"));
        if let Some(terminal) = self.terminal.as_ref() {
            terminal.update(cx, |terminal, cx| terminal.restart(&cwd, cx));
        }
    }

    pub fn run_build(&mut self, action: BuildAction, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.save_all(false, cx);
        self.build.update(cx, |build, cx| build.start(action, cx));
    }

    pub fn stop_build(&mut self, cx: &mut Context<Self>) {
        self.build.update(cx, |build, cx| build.stop(cx));
    }

    pub fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_terminal = true;
        self.bottom_tab = BottomTab::Terminal;
        self.ensure_terminal(window, cx);
        if let Some(terminal) = self.terminal.as_ref() {
            let focus = terminal.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let existed = self.terminal.is_some();
        self.focus_terminal(window, cx);
        if existed {
            self.restart_terminal(cx);
        }
    }

    pub fn show_navigator(&mut self, tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        let nav = match tab {
            0 => Navigator::Files,
            1 => Navigator::Search,
            _ => Navigator::Changes,
        };
        if nav == Navigator::Search {
            self.show_search(window, cx);
            return;
        }
        self.show_left = true;
        self.navigator = nav;
        cx.notify();
    }

    pub fn reveal_active(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.active.and_then(|ix| self.docs.get(ix)).map(|d| d.path.clone()) else { return };
        let Some(tree) = self.tree.as_mut() else { return };
        tree.reveal(&path);
        self.rebuild_rows();
        self.show_left = true;
        self.navigator = Navigator::Files;
        if let Some(ix) = self.rows.iter().position(|row| row.path == path) {
            self.tree_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        cx.notify();
    }

    pub fn focus_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.show_left = true;
        self.navigator = Navigator::Changes;
        self.commit_input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.docs.len();
        let Some(ix) = self.active.filter(|_| count > 1) else { return };
        let next = if forward { (ix + 1) % count } else { (ix + count - 1) % count };
        self.activate(next, window, cx);
    }

    fn sync_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let soft_wrap = Settings::get(cx).soft_wrap;
        if soft_wrap == self.soft_wrap {
            return;
        }
        self.soft_wrap = soft_wrap;
        for doc in &self.docs {
            doc.editor.update(cx, |state, cx| state.set_soft_wrap(soft_wrap, window, cx));
        }
    }

    fn schedule_lsp_sync(&mut self, id: usize, cx: &mut Context<Self>) {
        if !self.docs.iter().any(|d| d.id == id && d.definitions.is_some()) {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LSP_SYNC_DELAY).await;
            let _ = this.update(cx, |this, cx| this.sync_lsp(id, cx));
        });
        self.lsp_sync.insert(id, task);
    }

    fn sync_lsp(&mut self, id: usize, cx: &mut Context<Self>) {
        self.lsp_sync.remove(&id);
        let Some(doc) = self.docs.iter().find(|d| d.id == id) else { return };
        let Some(definitions) = doc.definitions.as_ref() else { return };
        if self.lsp.is_external(&doc.path) {
            return;
        }
        let (kind, path, stale) = (definitions.kind, doc.path.clone(), definitions.stale.replace(false));
        let text = doc.editor.read(cx).text().clone();
        self.lsp.sync(kind, definitions.language, &path, stale, &text, cx);
    }

    fn on_lsp_event(&mut self, event: lsp::Event, cx: &mut Context<Self>) {
        match event {
            lsp::Event::Diagnostics { path, diagnostics } => {
                if self.root.is_none() || self.lsp.is_external(&path) {
                    return;
                }
                let editor = self.docs.iter().find(|d| d.path == path).map(|d| d.editor.clone());
                self.issues.publish(path, diagnostics, editor.as_ref(), cx);
                cx.notify();
            }
            lsp::Event::Stopped { kind, reason, spawn_failed } => {
                if let Some(message) = self.lsp.stopped(kind, reason, spawn_failed) {
                    self.notify_error(message, cx);
                }
                if self.lsp.resync_kind(kind) {
                    let ids: Vec<usize> = self
                        .docs
                        .iter()
                        .filter(|d| d.definitions.as_ref().is_some_and(|def| def.kind == Some(kind)))
                        .map(|d| d.id)
                        .collect();
                    for id in ids {
                        self.sync_lsp(id, cx);
                    }
                }
            }
        }
    }

    pub fn show_issues(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_terminal && self.bottom_tab == BottomTab::Issues {
            self.toggle_terminal(window, cx);
            return;
        }
        self.show_terminal = true;
        self.bottom_tab = BottomTab::Issues;
        cx.notify();
    }

    fn issues_open_handler(&self, cx: &mut Context<Self>) -> issues_panel::OpenHandler {
        let workspace = cx.weak_entity();
        Rc::new(move |path, line, column, window, cx| {
            let path = path.to_path_buf();
            let _ = workspace.update(cx, |ws, cx| ws.open_at(&path, line, column, window, cx));
        })
    }

    fn notify_info(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.set_notice(Notice::Info(message.into()), cx);
    }

    fn notify_error(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        eprintln!("forge: {message}");
        self.set_notice(Notice::Error(message), cx);
    }

    fn set_notice(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.notice = Some(notice);
        self.notice_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            let _ = this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

fn git_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().skip(1).find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    crate::icons::ui_icon(name, size, color)
}

fn tool_button(
    id: &'static str,
    name: IconName,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|s| s.bg(ca(0xffffff, 0.08)))
        .active(|s| s.bg(ca(0xffffff, 0.14)))
        .child(icon(name, 16., if active { Palette::accent() } else { Palette::muted() }))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
}

fn text_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    primary: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id: SharedString = id.into();
    div()
        .id(gpui_kit::ElementId::Name(id))
        .px(px(12.))
        .h(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(primary, |el| {
            el.bg(Palette::accent())
                .text_color(gpui_kit::white())
                .hover(|s| s.bg(Palette::accent_hover()))
        })
        .when(!primary, |el| {
            el.bg(ca(0xffffff, 0.08))
                .text_color(Palette::text())
                .hover(|s| s.bg(ca(0xffffff, 0.13)))
        })
        .child(label.into())
        .on_click(on_click)
}

fn section_header(title: impl Into<SharedString>) -> impl IntoElement {
    div()
        .px(px(12.))
        .pt(px(12.))
        .pb(px(6.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Palette::muted())
        .child(title.into())
}

fn change_section_header(
    id: &'static str,
    title: String,
    action: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px(px(12.))
        .pt(px(12.))
        .pb(px(6.))
        .flex()
        .items_center()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Palette::muted())
        .child(div().flex_1().child(title))
        .child(
            div()
                .id(id)
                .px(px(6.))
                .rounded(px(4.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(|s| s.bg(ca(0xffffff, 0.08)).text_color(Palette::text()))
                .child(action)
                .on_click(on_click),
        )
}

fn status_color(status: FileStatus) -> Hsla {
    match status {
        FileStatus::Modified | FileStatus::Renamed => Palette::git_modified(),
        FileStatus::Added => Palette::git_added(),
        FileStatus::Deleted | FileStatus::Conflicted => Palette::git_deleted(),
        FileStatus::Untracked => Palette::git_untracked(),
    }
}

impl Workspace {
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self
            .root
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Forge".into());
        let branch = self.repo.as_ref().and_then(|r| r.branch.clone());
        let build_status = self.build.read(cx).status();
        let status: SharedString = match (&self.notice, &build_status) {
            (Some(Notice::Info(m)) | Some(Notice::Error(m)), _) => m.clone(),
            (None, Some((m, _))) => m.clone(),
            (None, None) => match self.active.and_then(|ix| self.docs.get(ix)) {
                Some(doc) if doc.dirty => "Editing…".into(),
                Some(_) => "Saved".into(),
                None => "Ready".into(),
            },
        };
        let is_error = match &self.notice {
            Some(notice) => matches!(notice, Notice::Error(_)),
            None => build_status.as_ref().is_some_and(|(_, failed)| *failed),
        };

        div()
            .id("toolbar")
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .bg(Palette::toolbar_bg())
            .border_b_1()
            .border_color(Palette::border())
            .pl(px(78.))
            .pr(px(8.))
            .gap(px(4.))
            .on_mouse_down(gpui_kit::MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .child(tool_button("toggle-left", IconName::PanelLeft, self.show_left, cx.listener(
                |this, _, _, cx| this.toggle_left(cx),
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .ml(px(8.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(Palette::text())
                            .child(project),
                    )
                    .when_some(branch, |el, branch| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .text_size(px(11.))
                                .text_color(Palette::muted())
                                .child(icon(IconName::GitBranch, 10., Palette::muted()))
                                .child(branch),
                        )
                    }),
            )
            .child(div().flex_1())
            .when(self.root.is_some() && self.build.read(cx).has_projects(), |el| {
                el.child(build_panel::toolbar_segment(&self.build, cx))
            })
            .child(
                div()
                    .h(px(24.))
                    .flex_shrink(1.)
                    .min_w(px(160.))
                    .max_w(px(520.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .bg(ca(0x000000, 0.25))
                    .text_size(px(12.))
                    .text_color(if is_error { Palette::git_deleted() } else { Palette::muted() })
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(status),
            )
            .child(div().flex_1())
            .when(self.active_doc().is_some_and(|doc| preview::supports(&doc.path)), |el| {
                el.child(tool_button("toggle-preview", IconName::Smartphone, self.show_preview, cx.listener(
                    |this, _, window, cx| this.toggle_preview(window, cx),
                )))
            })
            .child(tool_button("toggle-terminal", IconName::PanelBottom, self.show_terminal, cx.listener(
                |this, _, window, cx| this.toggle_terminal(window, cx),
            )))
            .child(tool_button("toggle-right", IconName::PanelRight, self.show_right, cx.listener(
                |this, _, _, cx| this.toggle_right(cx),
            )))
    }

    fn render_navigator(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let change_count = self.git_marks.change_count;
        let tab = |id: &'static str, name: IconName, nav: Navigator, cx: &mut Context<Self>| -> AnyElement {
            let active = self.navigator == nav;
            tool_button(id, name, active, cx.listener(move |this, _, _, cx| {
                this.navigator = nav;
                cx.notify();
            }))
            .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(Palette::sidebar_bg())
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(Palette::border())
                    .child(tab("nav-files", IconName::Folder, Navigator::Files, cx))
                    .child(tab("nav-search", IconName::Search, Navigator::Search, cx))
                    .child(
                        div()
                            .relative()
                            .child(tab("nav-changes", IconName::GitCompare, Navigator::Changes, cx))
                            .when(change_count > 0, |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .top(px(0.))
                                        .right(px(-4.))
                                        .px(px(4.))
                                        .rounded_full()
                                        .bg(Palette::accent())
                                        .text_size(px(9.))
                                        .text_color(gpui_kit::white())
                                        .child(change_count.to_string()),
                                )
                            }),
                    ),
            )
            .child(match self.navigator {
                Navigator::Files => self.render_files(cx).into_any_element(),
                Navigator::Search => div().flex_1().min_h(px(0.)).child(self.search.clone()).into_any_element(),
                Navigator::Changes => self.render_changes(cx).into_any_element(),
            })
    }

    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.root.is_none() {
            return div()
                .p(px(16.))
                .text_size(px(12.))
                .text_color(Palette::muted())
                .child("No folder open")
                .into_any_element();
        }
        let rows = self.rows.clone();
        let active_path = self.active.and_then(|ix| self.docs.get(ix)).map(|d| d.path.clone());
        let marks = self.git_marks.clone();
        let scroll = self.tree_scroll.clone();
        uniform_list("file-tree", rows.len(), cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
            range
                .map(|ix| {
                    let row = rows[ix].clone();
                    let selected = active_path.as_ref() == Some(&row.path);
                    let status = if row.is_dir {
                        marks.dirs.contains(&row.path).then_some(FileStatus::Modified)
                    } else {
                        marks.files.get(&row.path).copied()
                    };
                    this.render_row(ix, row, selected, status, cx)
                })
                .collect::<Vec<_>>()
        }))
        .track_scroll(&scroll)
        .flex_1()
        .py(px(4.))
        .into_any_element()
    }

    fn render_row(
        &self,
        ix: usize,
        row: Row,
        selected: bool,
        status: Option<FileStatus>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let indent = 8. + row.depth as f32 * 14.;
        let chevron = if row.expanded { IconName::ChevronDown } else { IconName::ChevronRight };
        let kind_icon = if row.is_dir {
            file_icons::folder_icon_img(&row.path, row.expanded)
        } else {
            file_icons::file_icon_img(&row.path)
        };
        let name_color = match status {
            Some(status) if !row.is_dir => status_color(status),
            _ => Palette::text(),
        };
        let row_for_click = row.clone();
        let inner = div()
            .size_full()
            .pl(px(indent))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(5.))
            .rounded(px(5.))
            .text_size(px(12.5))
            .when(selected, |el| el.bg(Palette::row_selected()))
            .when(!selected, |el| el.hover(|s| s.bg(Palette::row_hover())))
            .child(
                div().w(px(12.)).flex_none().when(row.is_dir, |el| el.child(icon(chevron, 11., Palette::faint()))),
            )
            .child(kind_icon)
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(name_color)
                    .child(row.name.clone()),
            )
            .when_some(status.filter(|_| !row.is_dir), |el, status| {
                el.child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(status_color(status))
                        .child(status.letter()),
                )
            })
            .when(row.is_dir && status.is_some(), |el| {
                el.child(div().size(px(5.)).rounded_full().bg(Palette::git_modified()))
            });
        div()
            .id(("row", ix))
            .w_full()
            .h(px(crate::icons::row_height(ROW_HEIGHT, crate::file_icons::ICON_SIZE)))
            .px(px(6.))
            .cursor_pointer()
            .child(inner)
            .on_click(cx.listener(move |this, _, window, cx| this.click_row(&row_for_click, window, cx)))
            .into_any_element()
    }

    fn render_changes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(repo) = self.repo.as_ref() else {
            return div()
                .p(px(16.))
                .text_size(px(12.))
                .text_color(Palette::muted())
                .child(if self.root.is_some() { "Not a git repository" } else { "No folder open" })
                .into_any_element();
        };
        let workdir = repo.workdir.clone();
        let commit_label = if repo.staged.is_empty() { "Commit All" } else { "Commit" };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .child(
                div()
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(Palette::border())
                    .child(Input::new(&self.commit_input))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(text_button("commit", commit_label, true, cx.listener(
                                |this, _, window, cx| this.commit(window, cx),
                            )))
                            .child(div().flex_1())
                            .child(tool_button("pull", IconName::ArrowDownToLine, false, cx.listener(
                                |this, _, window, cx| this.run_git("Pull", git::pull, window, cx),
                            )))
                            .child(tool_button("push", IconName::ArrowUpFromLine, false, cx.listener(
                                |this, _, window, cx| this.run_git("Push", git::push, window, cx),
                            )))
                            .child(tool_button("fetch", IconName::RefreshCw, false, cx.listener(
                                |this, _, window, cx| this.run_git("Fetch", git::fetch, window, cx),
                            ))),
                    )
                    .when(repo.ahead > 0 || repo.behind > 0, |el| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(Palette::muted())
                                .child(format!("↑{} ↓{}", repo.ahead, repo.behind)),
                        )
                    }),
            )
            .child(
                div()
                    .id("changes")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(!repo.staged.is_empty(), |el| {
                        el.child(change_section_header(
                            "staged-header",
                            format!("STAGED ({})", repo.staged.len()),
                            "Unstage All",
                            cx.listener(|this, _, _, cx| this.run_index_op(git::unstage_all, cx)),
                        ))
                        .children(
                            repo.staged
                                .iter()
                                .enumerate()
                                .map(|(ix, change)| self.render_change_row(ix, change, &workdir, cx)),
                        )
                    })
                    .child(change_section_header(
                        "changes-header",
                        format!("CHANGES ({})", repo.unstaged.len()),
                        "Stage All",
                        cx.listener(|this, _, _, cx| this.run_index_op(git::stage_all, cx)),
                    ))
                    .children(
                        repo.unstaged
                            .iter()
                            .enumerate()
                            .map(|(ix, change)| self.render_change_row(ix, change, &workdir, cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_change_row(&self, ix: usize, change: &git::Change, workdir: &Path, cx: &mut Context<Self>) -> AnyElement {
        let staged = change.staged;
        let selected = self
            .selected_change
            .as_ref()
            .is_some_and(|(s, path)| *s == staged && *path == change.path);
        let name = change.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = change.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
        let absolute = workdir.join(&change.path);
        let rel = change.path.clone();
        let group: SharedString = if staged { "staged-row".into() } else { "change-row".into() };
        let row_id = if staged { ("staged", ix) } else { ("change", ix) };

        let action_button = |id: (&'static str, usize), icon_name: IconName| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(18.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(ca(0xffffff, 0.12)))
                .when(!selected, |el| el.invisible().group_hover(group.clone(), |s| s.visible()))
                .child(icon(icon_name, 12., Palette::muted()))
        };

        let mut row = div()
            .id(row_id)
            .group(group.clone())
            .h(px(crate::icons::row_height(ROW_HEIGHT, crate::file_icons::ICON_SIZE)))
            .mx(px(6.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(5.))
            .text_size(px(12.5))
            .when(selected, |el| el.bg(Palette::row_selected()))
            .when(!selected, |el| el.hover(|s| s.bg(Palette::row_hover())))
            .child(
                div()
                    .w(px(12.))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(status_color(change.status))
                    .child(change.status.letter()),
            )
            .child(file_icons::file_icon_img(&change.path))
            .child(div().text_color(Palette::text()).whitespace_nowrap().child(name))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(11.))
                    .text_color(Palette::faint())
                    .child(dir),
            );

        if staged {
            let rel = rel.clone();
            row = row.child(action_button(("unstage", ix), IconName::Minus).on_click(cx.listener(
                move |this, _, _, cx| {
                    cx.stop_propagation();
                    let path = rel.clone();
                    this.run_index_op(move |root| git::unstage_file(root, &path), cx);
                },
            )));
        } else {
            let change_for_discard = change.clone();
            let rel = rel.clone();
            row = row
                .child(action_button(("discard", ix), IconName::Undo2).on_click(cx.listener(
                    move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.confirm_discard(change_for_discard.clone(), window, cx);
                    },
                )))
                .child(action_button(("stage", ix), IconName::Plus).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        let path = rel.clone();
                        this.run_index_op(move |root| git::stage_file(root, &path), cx);
                    },
                )));
        }

        row.on_click(cx.listener(move |this, _, window, cx| {
            this.selected_change = Some((staged, rel.clone()));
            cx.notify();
            if absolute.is_file() {
                this.open_file(&absolute, window, cx);
            }
        }))
        .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tabs")
            .h(px(30.))
            .flex_none()
            .flex()
            .overflow_x_scroll()
            .bg(Palette::tab_inactive())
            .border_b_1()
            .border_color(Palette::border())
            .children(self.docs.iter().enumerate().map(|(ix, doc)| {
                let active = self.active == Some(ix);
                let name = doc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                div()
                    .id(("tab", ix))
                    .group("tab")
                    .h_full()
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .border_r_1()
                    .border_color(Palette::border())
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(active, |el| el.bg(Palette::tab_active()).text_color(Palette::text()))
                    .when(!active, |el| el.text_color(Palette::muted()).hover(|s| s.bg(Palette::row_hover())))
                    .child(file_icons::file_icon_img(&doc.path))
                    .child(name)
                    .child(
                        div()
                            .id(("tab-close", ix))
                            .size(px(14.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .hover(|s| s.bg(ca(0xffffff, 0.12)))
                            .map(|el| {
                                if doc.dirty {
                                    el.child(div().size(px(6.)).rounded_full().bg(Palette::muted()))
                                } else {
                                    el.invisible()
                                        .group_hover("tab", |s| s.visible())
                                        .when(active, |el| el.visible())
                                        .child(icon(IconName::Close, 10., Palette::muted()))
                                }
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_doc(ix, window, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.activate(ix, window, cx)))
            }))
    }

    fn render_editor_area(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.active.and_then(|ix| self.docs.get(ix)) {
            Some(doc) => div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .relative()
                        .child(
                            Editor::new(&doc.editor)
                                .bordered(false)
                                .appearance(false)
                                .text_size(px(Settings::get(cx).editor_font_size))
                                .h_full(),
                        )
                        .when_some(self.issues.marks(&doc.path), |el, marks| {
                            let size = px((Settings::get(cx).editor_font_size * 0.9).round());
                            el.child(diagnostics::overlay(doc.editor.clone(), marks, size))
                        }),
                )
                .when(self.preview_open(), |el| {
                    el.child(drag_handle("handle-preview", Drag::Preview, cx))
                        .child(self.render_preview(cx))
                })
                .into_any_element(),
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(Palette::faint())
                .text_size(px(15.))
                .child("No Editor")
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(Palette::editor_bg())
            .when(!self.docs.is_empty(), |el| el.child(self.render_tabs(cx)))
            .child(content)
    }

    fn render_preview(&self, cx: &App) -> impl IntoElement {
        let live = !self.quick_open_visible
            && !self.clone_open
            && !self.build.read(cx).picker_open()
            && self.drag != Some(Drag::Preview);
        div()
            .w(px(self.preview_width))
            .flex_none()
            .h_full()
            .bg(Palette::panel_bg())
            .when(live, |el| el.child(canvas(self.preview.place(), |_, _, _, _| {}).size_full()))
    }

    fn render_terminal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(Palette::panel_bg())
            .child(
                div()
                    .h(px(26.))
                    .flex_none()
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .border_t_1()
                    .border_b_1()
                    .border_color(Palette::border())
                    .bg(Palette::sidebar_bg())
                    .child(self.bottom_tab_button("bottom-terminal", IconName::SquareTerminal, "Terminal", BottomTab::Terminal, None, cx))
                    .child(self.bottom_tab_button("bottom-build", IconName::Hammer, "Build", BottomTab::Build, None, cx))
                    .child({
                        let (errors, warnings) = self.issues.counts();
                        let badge = (errors + warnings > 0).then(|| issues_panel::counts_badge(errors, warnings));
                        self.bottom_tab_button("bottom-issues", IconName::TriangleAlert, "Issues", BottomTab::Issues, badge, cx)
                    })
                    .child(div().flex_1())
                    .when(self.bottom_tab == BottomTab::Terminal, |el| {
                        el.child(tool_button("term-restart", IconName::RotateCw, false, cx.listener(
                            |this, _, _, cx| this.restart_terminal(cx),
                        )))
                    })
                    .child(tool_button("term-hide", IconName::Close, false, cx.listener(
                        |this, _, window, cx| this.toggle_terminal(window, cx),
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .map(|el| match self.bottom_tab {
                        BottomTab::Terminal => el.when_some(self.terminal.clone(), |el, terminal| el.child(terminal)),
                        BottomTab::Build => el.child(self.build.clone()),
                        BottomTab::Issues => el.child(issues_panel::render(
                            &self.issues,
                            self.root.as_deref(),
                            self.issues_open_handler(cx),
                        )),
                    }),
            )
    }

    fn bottom_tab_button(
        &self,
        id: &'static str,
        name: IconName,
        label: &'static str,
        tab: BottomTab,
        badge: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.bottom_tab == tab;
        let color = if active { Palette::text() } else { Palette::muted() };
        div()
            .id(id)
            .h(px(20.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(5.))
            .rounded(px(5.))
            .cursor_pointer()
            .when(active, |el| el.bg(ca(0xffffff, 0.08)))
            .hover(|s| s.bg(ca(0xffffff, 0.06)))
            .child(icon(name, 13., color))
            .child(div().text_size(px(11.5)).text_color(color).child(label))
            .when_some(badge, |el, badge| el.child(div().ml(px(2.)).child(badge)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.bottom_tab = tab;
                if tab == BottomTab::Terminal {
                    this.ensure_terminal(window, cx);
                }
                cx.notify();
            }))
    }

    fn render_inspector(&self, cx: &App) -> impl IntoElement {
        let doc = self.active.and_then(|ix| self.docs.get(ix));
        let field = |label: &'static str, value: String| {
            div()
                .px(px(12.))
                .py(px(3.))
                .flex()
                .flex_col()
                .child(div().text_size(px(10.5)).text_color(Palette::faint()).child(label))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(Palette::text())
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(value),
                )
        };
        let mut panel = div()
            .id("inspector")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(Palette::sidebar_bg());

        panel = panel.child(section_header("FILE"));
        panel = match doc {
            Some(doc) => {
                let state = doc.editor.read(cx);
                let text = state.text();
                let lines = gpui_kit::component::RopeExt::lines_len(text);
                let bytes = text.len();
                let cursor = state.cursor_position();
                let relative = self
                    .root
                    .as_ref()
                    .and_then(|root| doc.path.strip_prefix(root).ok())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| doc.path.display().to_string());
                panel
                    .child(field("Name", doc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
                    .child(field("Path", relative))
                    .child(field("Type", files::language_for(&doc.path).to_string()))
                    .child(field("Size", format_bytes(bytes)))
                    .child(field("Lines", lines.to_string()))
                    .child(field("Cursor", format!("Ln {}, Col {}", cursor.line + 1, cursor.character + 1)))
                    .child(field("Autosave", if doc.dirty { "Pending…".into() } else { "Saved".into() }))
            }
            None => panel.child(
                div().px(px(12.)).text_size(px(12.)).text_color(Palette::faint()).child("No Selection"),
            ),
        };

        if let Some(repo) = self.repo.as_ref() {
            panel = panel.child(section_header("HISTORY")).children(repo.log.iter().map(|commit| {
                div()
                    .mx(px(6.))
                    .px(px(6.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .hover(|s| s.bg(Palette::row_hover()))
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(Palette::text())
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(commit.subject.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .text_size(px(10.5))
                            .text_color(Palette::faint())
                            .child(div().font_family(crate::theme::MONO_FONT).child(commit.hash.clone()))
                            .child(commit.author.clone())
                            .child(commit.when.clone()),
                    )
            }));
        }
        panel
    }

    fn render_index_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.indexing.as_ref().filter(|i| i.started.elapsed() >= INDEX_BAR_DELAY);
        let content = match running {
            Some(indexing) => {
                let found = indexing.progress.files.load(std::sync::atomic::Ordering::Relaxed);
                const BAR: f32 = 120.;
                let (left, width) = match indexing.expected {
                    Some(expected) if expected > 0 => (0., BAR * (found as f32 / expected as f32).min(0.97)),
                    _ => {
                        let phase = (indexing.started.elapsed().as_millis() % 1200) as f32 / 1200.;
                        (BAR * 0.7 * phase, BAR * 0.3)
                    }
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child("Indexing")
                    .child(
                        div()
                            .relative()
                            .w(px(BAR))
                            .h(px(4.))
                            .rounded_full()
                            .bg(ca(0xffffff, 0.1))
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left(px(left))
                                    .h_full()
                                    .w(px(width.max(4.)))
                                    .rounded_full()
                                    .bg(Palette::accent()),
                            ),
                    )
                    .child(format!("{found} files"))
                    .child(
                        div()
                            .max_w(px(260.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(Palette::faint())
                            .child(indexing.progress.current_dir()),
                    )
                    .into_any_element()
            }
            None => match self.index.as_ref() {
                Some(index) => format!("{} files indexed", index.files.len()).into_any_element(),
                None => "Indexing…".into_any_element(),
            },
        };
        div()
            .id("index-status")
            .px(px(6.))
            .h_full()
            .flex()
            .items_center()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(Palette::row_hover()))
            .child(content)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_index_log(cx)))
    }

    fn render_index_log(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("index-log")
            .absolute()
            .bottom(px(STATUS_HEIGHT + 6.))
            .left(px(8.))
            .w(px(520.))
            .max_h(px(260.))
            .flex()
            .flex_col()
            .rounded(px(8.))
            .bg(Palette::toolbar_bg())
            .border_1()
            .border_color(Palette::hairline())
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(10.))
                    .h(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(Palette::border())
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(Palette::muted())
                    .child("INDEXING LOG")
                    .child(div().flex_1())
                    .child(tool_button("index-log-close", IconName::Close, false, cx.listener(|this, _, _, cx| {
                        this.toggle_index_log(cx)
                    }))),
            )
            .child(
                div()
                    .id("index-log-lines")
                    .flex_1()
                    .overflow_y_scroll()
                    .py(px(4.))
                    .font_family(crate::theme::MONO_FONT)
                    .text_size(px(11.))
                    .children(self.index_log.iter().rev().map(|line| {
                        div()
                            .px(px(10.))
                            .py(px(2.))
                            .flex()
                            .gap(px(10.))
                            .child(
                                div()
                                    .w(px(56.))
                                    .flex_none()
                                    .text_color(Palette::faint())
                                    .child(format!("+{:.3}s", line.at.as_secs_f32())),
                            )
                            .child(div().text_color(Palette::text()).child(line.text.clone()))
                    })),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let doc = self.active.and_then(|ix| self.docs.get(ix));
        let cursor = doc.map(|doc| doc.editor.read(cx).cursor_position());
        let index_status = self.render_index_status(cx);
        div()
            .h(px(STATUS_HEIGHT))
            .flex_none()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(14.))
            .bg(Palette::toolbar_bg())
            .border_t_1()
            .border_color(Palette::border())
            .text_size(px(11.))
            .text_color(Palette::muted())
            .when_some(self.repo.as_ref().and_then(|r| r.branch.clone()), |el, branch| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(icon(IconName::GitBranch, 11., Palette::muted()))
                        .child(branch),
                )
            })
            .child(index_status)
            .when(!self.issues.is_empty(), |el| {
                let (errors, warnings) = self.issues.counts();
                el.child(
                    div()
                        .id("status-issues")
                        .px(px(4.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(ca(0xffffff, 0.06)))
                        .child(issues_panel::counts_badge(errors, warnings))
                        .on_click(cx.listener(|this, _, window, cx| this.show_issues(window, cx))),
                )
            })
            .child(div().flex_1())
            .when_some(cursor, |el, cursor| {
                el.child(format!("Ln {}, Col {}", cursor.line + 1, cursor.character + 1))
            })
            .when_some(doc, |el, doc| el.child(files::language_for(&doc.path)))
            .child("UTF-8")
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .bg(Palette::editor_bg())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .size(px(84.))
                            .rounded(px(20.))
                            .bg(gpui_kit::linear_gradient(
                                160.,
                                gpui_kit::linear_color_stop(crate::theme::c(0x3d8bff), 0.),
                                gpui_kit::linear_color_stop(crate::theme::c(0x5b3fd9), 1.),
                            ))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(IconName::FileCode, 40., gpui_kit::white())),
                    )
                    .child(
                        div()
                            .mt(px(14.))
                            .text_size(px(30.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(Palette::text())
                            .child("Forge"),
                    )
                    .child(div().text_size(px(13.)).text_color(Palette::muted()).child("Drop a folder or file anywhere to open it"))
                    .child(
                        div()
                            .mt(px(26.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .w(px(260.))
                            .child(welcome_action("w-open", IconName::FolderOpen, "Open Folder…", "⌘O", cx.listener(
                                |_, _, window, cx| window.dispatch_action(Box::new(crate::OpenFolder), cx),
                            )))
                            .child(welcome_action("w-clone", IconName::FolderGit2, "Clone Git Repository…", "⇧⌘C", cx.listener(
                                |this, _, window, cx| this.show_clone(window, cx),
                            ))),
                    ),
            )
            .child(
                div()
                    .id("recent")
                    .w(px(300.))
                    .h_full()
                    .flex_none()
                    .bg(Palette::sidebar_bg())
                    .border_l_1()
                    .border_color(Palette::border())
                    .overflow_y_scroll()
                    .py(px(10.))
                    .child(section_header("RECENT"))
                    .when(self.recent.is_empty(), |el| {
                        el.child(div().px(px(12.)).text_size(px(12.)).text_color(Palette::faint()).child("No recent projects"))
                    })
                    .children(self.recent.iter().enumerate().map(|(ix, path)| {
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let parent = path
                            .parent()
                            .map(|p| shorten_home(p))
                            .unwrap_or_default();
                        let path = path.clone();
                        div()
                            .id(("recent", ix))
                            .mx(px(8.))
                            .px(px(8.))
                            .py(px(6.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .cursor_pointer()
                            .hover(|s| s.bg(Palette::row_hover()))
                            .child(icon(IconName::Folder, 22., Palette::folder()))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .child(div().text_size(px(13.)).text_color(Palette::text()).child(name))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(Palette::faint())
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(parent),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_project(path.clone(), window, cx)
                            }))
                    })),
            )
    }

    fn render_clone_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("clone-overlay")
            .absolute()
            .inset_0()
            .bg(ca(0x000000, 0.45))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(120.))
            .on_click(cx.listener(|this, _, window, cx| this.close_clone(window, cx)))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                this.close_clone(window, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .id("clone-dialog")
                    .w(px(480.))
                    .p(px(18.))
                    .rounded(px(12.))
                    .bg(Palette::toolbar_bg())
                    .border_1()
                    .border_color(Palette::hairline())
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(icon(IconName::FolderGit2, 18., Palette::accent()))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(Palette::text())
                                    .child("Clone Git Repository"),
                            ),
                    )
                    .child(Input::new(&self.clone_input))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(Palette::faint())
                            .child("You will pick the parent folder next. Uses your git credentials."),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(text_button("clone-cancel", "Cancel", false, cx.listener(|this, _, window, cx| {
                                this.close_clone(window, cx)
                            })))
                            .child(text_button(
                                "clone-go",
                                if self.clone_busy { "Cloning…" } else { "Clone…" },
                                true,
                                cx.listener(|this, _, window, cx| this.start_clone(window, cx)),
                            )),
                    ),
            )
    }
}

fn welcome_action(
    id: &'static str,
    name: IconName,
    label: &'static str,
    shortcut: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(12.))
        .py(px(9.))
        .rounded(px(8.))
        .bg(ca(0xffffff, 0.05))
        .hover(|s| s.bg(ca(0xffffff, 0.1)))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(icon(name, 16., Palette::accent()))
        .child(div().flex_1().text_size(px(13.)).text_color(Palette::text()).child(label))
        .child(div().text_size(px(11.)).text_color(Palette::faint()).child(shortcut))
        .on_click(on_click)
}

fn format_bytes(bytes: usize) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.),
        b => format!("{:.1} MB", b as f64 / (1024. * 1024.)),
    }
}

fn shorten_home(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn drag_handle(id: &'static str, drag: Drag, cx: &mut Context<Workspace>) -> impl IntoElement {
    let vertical = drag == Drag::Bottom;
    div()
        .id(id)
        .flex_none()
        .relative()
        .bg(Palette::border())
        .map(|el| if vertical { el.h(px(1.)).w_full() } else { el.w(px(1.)).h_full() })
        .child(
            div()
                .id((id, 1usize))
                .absolute()
                .map(|el| {
                    if vertical {
                        el.left_0().right_0().top(px(-HANDLE_WIDTH / 2.)).h(px(HANDLE_WIDTH)).cursor_row_resize()
                    } else {
                        el.top_0().bottom_0().left(px(-HANDLE_WIDTH / 2.)).w(px(HANDLE_WIDTH)).cursor_col_resize()
                    }
                })
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.drag = Some(drag);
                    cx.stop_propagation();
                    cx.notify();
                })),
        )
}

impl Workspace {
    fn on_drag_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else { return };
        if !event.dragging() {
            self.drag = None;
            cx.notify();
            return;
        }
        let viewport = window.viewport_size();
        let (x, y) = (f32::from(event.position.x), f32::from(event.position.y));
        match drag {
            Drag::Left => self.left_width = x.clamp(160., 600.),
            Drag::Right => self.right_width = (f32::from(viewport.width) - x).clamp(180., 600.),
            Drag::Preview => {
                let right = if self.show_right { self.right_width + 1. } else { 0. };
                self.preview_width = (f32::from(viewport.width) - right - x).clamp(240., 1200.);
            }
            Drag::Bottom => {
                let bottom = f32::from(viewport.height) - STATUS_HEIGHT;
                self.terminal_height = (bottom - y).clamp(80., f32::from(viewport.height) - 200.);
            }
        }
        cx.notify();
    }

    fn panel_size(&self, id: &'static str, open: bool, size: f32, drag: Drag, window: &mut Window, cx: &mut App) -> f32 {
        let target = if open { size } else { 0. };
        let policy = if self.drag == Some(drag) { Spring::new(Duration::ZERO) } else { PANEL_SPRING };
        spring(id, target, policy, window, cx).max(0.)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        static FIRST_RENDER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if crate::startup::enabled() && !FIRST_RENDER.load(std::sync::atomic::Ordering::Relaxed) {
            crate::startup::mark_once(&FIRST_RENDER, "first render");
            window.on_next_frame(|_, _| crate::startup::mark("first frame drawn"));
        }
        self.sync_settings(window, cx);
        let preview_live = self.preview_open()
            && !self.quick_open_visible
            && !self.clone_open
            && !self.build.read(cx).picker_open()
            && self.drag != Some(Drag::Preview);
        if !preview_live {
            self.preview.hide();
        }
        let body = if self.root.is_none() && self.docs.is_empty() {
            self.render_welcome(cx).into_any_element()
        } else {
            let left = self.panel_size("left-panel", self.show_left, self.left_width, Drag::Left, window, cx);
            let right = self.panel_size("right-panel", self.show_right, self.right_width, Drag::Right, window, cx);
            let bottom = self.panel_size("bottom-panel", self.show_terminal, self.terminal_height, Drag::Bottom, window, cx);
            let slide = |size: f32, full: f32, horizontal: bool, content: AnyElement| {
                div()
                    .flex_none()
                    .overflow_hidden()
                    .map(|el| if horizontal { el.w(px(size)).h_full() } else { el.h(px(size)).w_full() })
                    .when(size > 0.5, |el| {
                        el.child(
                            div()
                                .map(|el| if horizontal { el.w(px(full)).h_full() } else { el.h(px(full)).w_full() })
                                .child(content),
                        )
                    })
            };
            let center = div()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h(px(0.)).child(self.render_editor_area(cx)))
                .when(bottom > 0.5, |el| el.child(drag_handle("handle-bottom", Drag::Bottom, cx)))
                .child(slide(bottom, self.terminal_height, false, self.render_terminal(cx).into_any_element()));
            div()
                .size_full()
                .flex()
                .child(slide(left, self.left_width, true, self.render_navigator(cx).into_any_element()))
                .when(left > 0.5, |el| el.child(drag_handle("handle-left", Drag::Left, cx)))
                .child(center)
                .when(right > 0.5, |el| el.child(drag_handle("handle-right", Drag::Right, cx)))
                .child(slide(right, self.right_width, true, self.render_inspector(cx).into_any_element()))
                .into_any_element()
        };

        div()
            .id("workspace")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(Palette::editor_bg())
            .text_color(Palette::text())
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .when(self.drag.is_some(), |el| {
                el.on_mouse_move(cx.listener(Self::on_drag_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.drag = None;
                        cx.notify();
                    }))
                    .map(|el| if self.drag == Some(Drag::Bottom) { el.cursor_row_resize() } else { el.cursor_col_resize() })
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.open_paths(paths.paths(), window, cx);
            }))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(Palette::drop_target()))
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h(px(0.)).child(body))
            .when(self.root.is_some(), |el| el.child(self.render_status_bar(cx)))
            .when(self.root.is_some() && self.index_log_open, |el| el.child(self.render_index_log(cx)))
            .when(self.build.read(cx).picker_open(), |el| {
                let build = self.build.clone();
                el.child(
                    div()
                        .id("build-picker-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            build.update(cx, |build, cx| build.close_picker(cx))
                        }),
                )
            })
            .when(self.clone_open, |el| el.child(self.render_clone_dialog(cx)))
            .when(self.quick_open_visible, |el| {
                el.child(
                    div()
                        .id("quick-open-overlay")
                        .absolute()
                        .inset_0()
                        .flex()
                        .justify_center()
                        .pt(px(70.))
                        .bg(ca(0x000000, 0.25))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.quick_open_visible = false;
                            this.focus_active_editor(window, cx);
                            cx.notify();
                        }))
                        .child(self.quick_open.clone()),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_change_detects_rewrites_but_not_touches() {
        let dir = std::env::temp_dir().join(format!("forge-ws-ext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "one").unwrap();
        let loaded = mtime(&path);
        let saved_hash = hash_text("one");
        assert_eq!(external_change(&path, loaded, saved_hash), ExternalChange::None);

        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        assert!(matches!(external_change(&path, Some(old), saved_hash), ExternalChange::Touched(Some(_))));

        std::fs::write(&path, "two").unwrap();
        assert_eq!(external_change(&path, Some(old), saved_hash), ExternalChange::Changed);

        std::fs::remove_file(&path).unwrap();
        assert_eq!(external_change(&path, loaded, saved_hash), ExternalChange::None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_save_orders_unchanged_conflict_and_superseded() {
        let dir = std::env::temp_dir().join(format!("forge-ws-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.txt");
        std::fs::write(&path, "base").unwrap();
        let gate = files::WriteGate::default();
        let known = mtime(&path);
        let base = hash_text("base");

        assert_eq!(run_save("base", &path, known, base, true, 1, &gate), SaveOutcome::Unchanged);

        let outcome = run_save("edit2", &path, known, base, true, 2, &gate);
        assert!(matches!(outcome, SaveOutcome::Written { hash, .. } if hash == hash_text("edit2")));
        assert_eq!(run_save("edit1", &path, known, base, false, 1, &gate), SaveOutcome::Superseded);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "edit2");

        let old = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
        assert_eq!(run_save("edit3", &path, old, base, true, 3, &gate), SaveOutcome::Conflict);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "edit2");
        assert!(matches!(run_save("edit3", &path, old, base, false, 3, &gate), SaveOutcome::Written { .. }));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "edit3");

        let missing = dir.join("nope/x.txt");
        assert!(matches!(run_save("x", &missing, None, base, false, 4, &gate), SaveOutcome::Failed { .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_text_rejects_binary_and_non_utf8() {
        let dir = std::env::temp_dir().join(format!("forge-ws-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("a.rs");
        std::fs::write(&ok, "fn main() {}\n").unwrap();
        let loaded = load_text(&ok).unwrap();
        assert_eq!(loaded.saved_hash, hash_text(&loaded.text));
        assert_eq!(loaded.language, "rust");
        let bin = dir.join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2, 0, 255]).unwrap();
        assert!(load_text(&bin).is_err());
        assert!(load_text(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
