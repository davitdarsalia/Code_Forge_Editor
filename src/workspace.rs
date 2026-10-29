//! The single window: toolbar, navigator (left), editor tabs (center),
//! inspector (right), terminal (bottom) and status bar.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::base::{Spring, spring};
use gpui_kit::component::Icon;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, MouseButton, MouseMoveEvent, Context, Entity, ExternalPaths, FocusHandle,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
    ScrollStrategy, UniformListScrollHandle, canvas, div, prelude::FluentBuilder as _, px, uniform_list,
};

use crate::files::{self, Node, Row};
use crate::git::{self, FileStatus, RepoState};
use crate::preview::{self, Preview};
use crate::quick_open::{QuickOpen, QuickOpenEvent};
use crate::recent;
use crate::search::FileIndex;
use crate::settings::Settings;
use crate::search_panel::{SearchEvent, SearchPanel};
use crate::terminal::Terminal;
use gpui_kit::Focusable as _;
use crate::theme::{Palette, ca};

const GIT_REFRESH_DELAY: Duration = Duration::from_millis(600);
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const TOOLBAR_HEIGHT: f32 = 38.;
const STATUS_HEIGHT: f32 = 22.;
const ROW_HEIGHT: f32 = 22.;
const HANDLE_WIDTH: f32 = 6.;
/// Feel of the sidebar slide: ~0.4 s period, slightly under-damped.
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
    save_task: Option<Task<()>>,
    _subscription: Subscription,
}

#[derive(Clone, Copy, PartialEq)]
enum Navigator {
    Files,
    Search,
    Changes,
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
    preview: Preview,
    show_preview: bool,
    preview_task: Option<Task<()>>,
    repo: Option<RepoState>,
    git_status: HashMap<PathBuf, FileStatus>,
    git_busy: bool,
    git_task: Option<Task<()>>,
    commit_input: Entity<InputState>,
    clone_input: Entity<InputState>,
    clone_open: bool,
    clone_busy: bool,
    notice: Option<Notice>,
    notice_task: Option<Task<()>>,
    index: Option<Arc<FileIndex>>,
    index_task: Option<Task<()>>,
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
    /// Soft-wrap value last pushed to the open editors (see `sync_settings`).
    soft_wrap: bool,
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

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        let quick_open = cx.new(|cx| QuickOpen::new(window, cx));
        subscriptions.push(cx.subscribe_in(&quick_open, window, |this, _, event, window, cx| {
            this.quick_open_visible = false;
            match event {
                QuickOpenEvent::Open(path) => this.open_file(&path.clone(), window, cx),
                QuickOpenEvent::Dismiss => this.focus_active_editor(window, cx),
            }
            cx.notify();
        }));
        // Pick up edits made outside the editor (git checkout, other tools).
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.reload_changed_documents(window, cx);
                this.refresh_tree(cx);
                this.refresh_git(cx);
                this.reindex(cx);
            } else {
                this.save_all(cx);
            }
        }));

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
            preview: Preview::new(),
            show_preview: true,
            preview_task: None,
            repo: None,
            git_status: HashMap::new(),
            git_busy: false,
            git_task: None,
            commit_input,
            clone_input,
            clone_open: false,
            clone_busy: false,
            notice: None,
            notice_task: None,
            index: None,
            index_task: None,
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
            _subscriptions: subscriptions,
        }
    }

    // ---- Opening ---------------------------------------------------------

    /// Opens whatever the user dropped, picked or passed on the command line.
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
        self.save_all(cx);
        self.docs.clear();
        self.active = None;
        self.repo = None;
        self.git_status.clear();
        self.terminal = None;
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        window.set_window_title(&name);
        self.tree = Some(Node::root(&root));
        self.rebuild_rows();
        self.root = Some(root.clone());
        self.recent = recent::push(&root);
        crate::set_menus(cx);
        if self.show_terminal {
            self.ensure_terminal(window, cx);
        }
        self.refresh_git(cx);
        self.index = None;
        self.search.update(cx, |search, cx| search.set_index(None, cx));
        self.reindex(cx);
        cx.notify();
    }

    // ---- Index, quick open, search --------------------------------------

    /// Re-walks the project on every core in the background.
    fn reindex(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let started = std::time::Instant::now();
        let task = cx.background_spawn(async move { crate::search::build_index(&root) });
        self.index_task = Some(cx.spawn(async move |this, cx| {
            let index = Arc::new(task.await);
            let _ = this.update(cx, |this, cx| {
                // A project switch while indexing makes this result stale.
                if this.root.as_ref() != Some(&index.root) {
                    return;
                }
                let first = this.index.is_none();
                if first {
                    let message = format!("Indexed {} files in {} ms", index.files.len(), started.elapsed().as_millis());
                    this.notify_info(message, cx);
                }
                this.search.update(cx, |search, cx| search.set_index(Some(index.clone()), cx));
                this.index = Some(index);
                cx.notify();
            });
        }));
    }

    pub fn show_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.quick_open_visible = true;
        let index = self.index.clone();
        self.quick_open.update(cx, |q, cx| q.show(index, window, cx));
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
        self.open_file(path, window, cx);
        let Some(doc) = self.active.and_then(|ix| self.docs.get(ix)) else { return };
        if doc.path != path {
            return;
        }
        doc.editor.update(cx, |state, cx| {
            state.set_cursor_position(gpui_kit::component::input::Position { line, character: column }, window, cx)
        });
    }

    fn focus_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.active.and_then(|ix| self.docs.get(ix)) {
            doc.editor.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    pub fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.docs.iter().position(|d| d.path == path) {
            self.activate(ix, window, cx);
            return;
        }
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_FILE_BYTES {
            self.notify_error(format!("{} is too large to open", path.display()), cx);
            return;
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.notify_error(format!("Cannot open {}: {err}", path.display()), cx);
                return;
            }
        };
        if files::looks_binary(&bytes) {
            self.notify_error(format!("{} looks like a binary file", path.display()), cx);
            return;
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let saved_hash = hash_text(&text);
        let language = files::language_for(path);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                .folding(true)
                .soft_wrap(self.soft_wrap)
                .default_value(text)
        });
        let id = self.next_doc_id;
        self.next_doc_id += 1;
        let subscription = cx.subscribe_in(&editor, window, move |this, _, event, window, cx| match event {
            InputEvent::Change => {
                this.schedule_save(id, cx);
                this.schedule_preview(id, window, cx);
            }
            InputEvent::Blur => this.save_doc(id, cx),
            _ => {}
        });
        self.docs.push(Document {
            id,
            path: path.to_path_buf(),
            editor,
            saved_hash,
            dirty: false,
            mtime: mtime(path),
            save_task: None,
            _subscription: subscription,
        });
        if let Some(tree) = self.tree.as_mut() {
            tree.reveal(path);
            self.rebuild_rows();
        }
        let ix = self.docs.len() - 1;
        self.activate(ix, window, cx);
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = Some(ix);
        let editor = self.docs[ix].editor.clone();
        editor.update(cx, |state, cx| state.focus(window, cx));
        self.refresh_preview(window, cx);
        cx.notify();
    }

    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        let id = self.docs[ix].id;
        self.save_doc(id, cx);
        self.docs.remove(ix);
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

    // ---- Autosave --------------------------------------------------------

    fn schedule_save(&mut self, id: usize, cx: &mut Context<Self>) {
        let delay = Settings::get(cx).autosave_delay();
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return };
        doc.dirty = true;
        // Replacing the task drops (cancels) the previous timer: a debounce.
        doc.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| this.save_doc(id, cx));
        }));
        cx.notify();
    }

    fn save_doc(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(ix) = self.docs.iter().position(|d| d.id == id) else { return };
        let text = self.docs[ix].editor.read(cx).value();
        let hash = hash_text(&text);
        let doc = &mut self.docs[ix];
        doc.save_task = None;
        if hash == doc.saved_hash {
            if doc.dirty {
                doc.dirty = false;
                cx.notify();
            }
            return;
        }
        match files::write_atomic(&doc.path, text.as_bytes()) {
            Ok(()) => {
                doc.saved_hash = hash;
                doc.dirty = false;
                doc.mtime = mtime(&doc.path);
                self.schedule_git_refresh(cx);
            }
            Err(err) => {
                let message = format!("Save failed for {}: {err}", doc.path.display());
                self.notify_error(message, cx);
            }
        }
        cx.notify();
    }

    pub fn save_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<usize> = self.docs.iter().map(|d| d.id).collect();
        for id in ids {
            self.save_doc(id, cx);
        }
    }

    fn reload_changed_documents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for doc in self.docs.iter_mut() {
            let current = mtime(&doc.path);
            if current.is_none() || current == doc.mtime || doc.dirty {
                continue;
            }
            doc.mtime = current;
            let Ok(text) = std::fs::read_to_string(&doc.path) else { continue };
            let hash = hash_text(&text);
            if hash == doc.saved_hash {
                continue;
            }
            doc.saved_hash = hash;
            doc.editor.update(cx, |state, cx| state.set_value(text, window, cx));
        }
        self.refresh_preview(window, cx);
        cx.notify();
    }

    // ---- Preview canvas --------------------------------------------------

    fn active_doc(&self) -> Option<&Document> {
        self.active.and_then(|ix| self.docs.get(ix))
    }

    /// Whether the active file gets a canvas next to the editor.
    fn preview_open(&self) -> bool {
        self.show_preview && self.active_doc().is_some_and(|doc| preview::supports(&doc.path))
    }

    fn schedule_preview(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.preview_open() || self.active_doc().map(|d| d.id) != Some(id) {
            return;
        }
        // Replacing the task drops (cancels) the previous timer: a debounce.
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

    // ---- File tree -------------------------------------------------------

    fn rebuild_rows(&mut self) {
        self.rows = Rc::new(self.tree.as_ref().map(Node::rows).unwrap_or_default());
    }

    fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        if let Some(tree) = self.tree.as_mut() {
            tree.refresh();
            self.rebuild_rows();
            cx.notify();
        }
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

    // ---- Git -------------------------------------------------------------

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
            if git::is_repo(&root) { git::read_state(&root).ok() } else { None }
        });
        self.git_task = Some(cx.spawn(async move |this, cx| {
            let state = task.await;
            let _ = this.update(cx, |this, cx| {
                if let (Some(root), Some(state)) = (this.root.as_ref(), state.as_ref()) {
                    this.git_status = state.status_map(root);
                } else {
                    this.git_status.clear();
                }
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
        self.save_all(cx);
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

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.commit_input.read(cx).value().trim().to_string();
        if message.is_empty() {
            self.notify_error("Write a commit message first", cx);
            return;
        }
        self.commit_input.update(cx, |input, cx| input.set_value("", window, cx));
        self.run_git("Commit", move |root| git::commit_all(root, &message), window, cx);
    }

    // ---- Clone -----------------------------------------------------------

    pub fn show_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clone_open = true;
        self.clone_input.update(cx, |input, cx| input.focus(window, cx));
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

    // ---- Panels ----------------------------------------------------------

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
            if let Some(terminal) = self.terminal.as_ref() {
                let focus = terminal.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
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

    // ---- Shortcut targets ------------------------------------------------

    /// Shows the terminal (starting it if needed) and moves focus into it.
    pub fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_terminal = true;
        self.ensure_terminal(window, cx);
        if let Some(terminal) = self.terminal.as_ref() {
            let focus = terminal.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// Restarts the shell in the project root, showing and focusing it.
    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let existed = self.terminal.is_some();
        self.focus_terminal(window, cx);
        if existed {
            self.restart_terminal(cx);
        }
    }

    /// Switches the navigator tab (⌘1/⌘2/⌘3), opening the navigator.
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

    /// Expands the tree down to the active file and scrolls it into view.
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

    /// Opens the Changes tab and puts the cursor in the commit message box.
    pub fn focus_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.show_left = true;
        self.navigator = Navigator::Changes;
        self.commit_input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Cycles editor tabs; `forward` = ⌘⇧], otherwise ⌘⇧[.
    pub fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.docs.len();
        let Some(ix) = self.active.filter(|_| count > 1) else { return };
        let next = if forward { (ix + 1) % count } else { (ix + count - 1) % count };
        self.activate(next, window, cx);
    }

    /// Pushes settings that live in editor state (not read at render time).
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

    // ---- Notices ---------------------------------------------------------

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

// ---- Rendering --------------------------------------------------------------

fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(name).size(px(size)).text_color(color)
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
                .hover(|s| s.bg(crate::theme::c(0x2a95ff)))
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
        let status: SharedString = match &self.notice {
            Some(Notice::Info(m)) | Some(Notice::Error(m)) => m.clone(),
            None => match self.active.and_then(|ix| self.docs.get(ix)) {
                Some(doc) if doc.dirty => "Editing…".into(),
                Some(_) => "Saved".into(),
                None => "Ready".into(),
            },
        };
        let is_error = matches!(self.notice, Some(Notice::Error(_)));

        div()
            .id("toolbar")
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .bg(Palette::toolbar_bg())
            .border_b_1()
            .border_color(Palette::border())
            // Room for the traffic lights.
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
            .child(
                // Xcode-style activity pill.
                div()
                    .h(px(24.))
                    .min_w(px(260.))
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
        let change_count = self.repo.as_ref().map_or(0, |r| r.changes.len());
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
        let statuses = self.git_status.clone();
        let scroll = self.tree_scroll.clone();
        uniform_list("file-tree", rows.len(), cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
            range
                .map(|ix| {
                    let row = rows[ix].clone();
                    let selected = active_path.as_ref() == Some(&row.path);
                    let status = if row.is_dir {
                        statuses
                            .iter()
                            .any(|(p, _)| p.starts_with(&row.path))
                            .then_some(FileStatus::Modified)
                    } else {
                        statuses.get(&row.path).copied()
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
        let (kind_icon, kind_color) = if row.is_dir {
            (IconName::Folder, Palette::folder())
        } else {
            (IconName::FileCode, Palette::muted())
        };
        let name_color = match status {
            Some(status) if !row.is_dir => status_color(status),
            _ => Palette::text(),
        };
        let row_for_click = row.clone();
        // The whole width of the sidebar is the hit target, not just the label.
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
            .child(icon(kind_icon, 14., kind_color))
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
            .h(px(ROW_HEIGHT))
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
        let root = self.root.clone().unwrap_or_default();
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
                            .child(text_button("commit", "Commit All", true, cx.listener(
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
            .child(section_header(format!("CHANGES ({})", repo.changes.len())))
            .child(
                div()
                    .id("changes")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(repo.changes.iter().enumerate().map(|(ix, change)| {
                        let path = root.join(&change.path);
                        let name = change
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let dir = change.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
                        let change_for_discard = change.clone();
                        div()
                            .id(("change", ix))
                            .group("change")
                            .h(px(ROW_HEIGHT))
                            .mx(px(6.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded(px(5.))
                            .text_size(px(12.5))
                            .hover(|s| s.bg(Palette::row_hover()))
                            .child(
                                div()
                                    .w(px(12.))
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(status_color(change.status))
                                    .child(change.status.letter()),
                            )
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
                            )
                            .child(
                                div()
                                    .id(("discard", ix))
                                    .invisible()
                                    .group_hover("change", |s| s.visible())
                                    .child(icon(IconName::Undo2, 12., Palette::muted()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        let change = change_for_discard.clone();
                                        this.run_git(
                                            "Discard",
                                            move |root| git::discard(root, &change).map(|_| String::new()),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if path.is_file() {
                                    this.open_file(&path, window, cx);
                                }
                            }))
                    })),
            )
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
                    .child(icon(IconName::FileCode, 12., if active { Palette::accent() } else { Palette::faint() }))
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
                        .child(
                            Editor::new(&doc.editor)
                                .bordered(false)
                                .appearance(false)
                                .text_size(px(Settings::get(cx).editor_font_size))
                                .h_full(),
                        ),
                )
                .when(self.preview_open(), |el| {
                    el.child(drag_handle("handle-preview", Drag::Preview, cx))
                        .child(self.render_preview())
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

    /// Pane the native web view is laid over. The web view draws above all
    /// GPUI content, so it is hidden while overlays are open or the pane edge
    /// is dragged (it would swallow the mouse).
    fn render_preview(&self) -> impl IntoElement {
        let live = !self.quick_open_visible && !self.clone_open && self.drag != Some(Drag::Preview);
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
                    .child(icon(IconName::SquareTerminal, 13., Palette::muted()))
                    .child(div().text_size(px(11.5)).text_color(Palette::muted()).child("Terminal"))
                    .child(div().flex_1())
                    .child(tool_button("term-restart", IconName::RotateCw, false, cx.listener(
                        |this, _, _, cx| this.restart_terminal(cx),
                    )))
                    .child(tool_button("term-hide", IconName::Close, false, cx.listener(
                        |this, _, window, cx| this.toggle_terminal(window, cx),
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .when_some(self.terminal.clone(), |el, terminal| el.child(terminal)),
            )
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

    fn render_status_bar(&self, cx: &App) -> impl IntoElement {
        let doc = self.active.and_then(|ix| self.docs.get(ix));
        let cursor = doc.map(|doc| doc.editor.read(cx).cursor_position());
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
            .on_click(cx.listener(|this, _, _, cx| {
                if !this.clone_busy {
                    this.clone_open = false;
                    cx.notify();
                }
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
                            .child(text_button("clone-cancel", "Cancel", false, cx.listener(|this, _, _, cx| {
                                this.clone_open = false;
                                cx.notify();
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

/// Thin divider with a wider invisible grab area for resizing a panel.
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

    /// Animated size of a collapsible panel. While the user drags its edge the
    /// spring is bypassed so the panel tracks the pointer exactly.
    fn panel_size(&self, id: &'static str, open: bool, size: f32, drag: Drag, window: &mut Window, cx: &mut App) -> f32 {
        let target = if open { size } else { 0. };
        let policy = if self.drag == Some(drag) { Spring::new(Duration::ZERO) } else { PANEL_SPRING };
        spring(id, target, policy, window, cx).max(0.)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_settings(window, cx);
        let preview_live = self.preview_open()
            && !self.quick_open_visible
            && !self.clone_open
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
            // Content keeps its full size and is clipped by the animating
            // container, so it slides instead of re-flowing every frame.
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
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(ca(0x0a84ff, 0.08)))
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h(px(0.)).child(body))
            .when(self.root.is_some(), |el| el.child(self.render_status_bar(cx)))
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
