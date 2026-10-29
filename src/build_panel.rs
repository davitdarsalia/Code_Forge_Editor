//! Build UI: the toolbar segment (build / stop / target picker), the "Build"
//! tab of the bottom panel (streamed log + issues list) and the run state.
//! Process and parsing logic lives in `build.rs`.

use std::collections::VecDeque;
use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{
    MouseButton,
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    UniformListScrollHandle, Window, anchored, deferred, div, prelude::FluentBuilder as _, px, uniform_list,
};

use crate::build::{
    self, BuildAction, BuildProject, Destination, Issue, IssueSet, ModuleKind, ProjectKind, RunEvent, RunHandle,
    Severity,
};
use crate::file_icons;
use crate::theme::{MONO_FONT, Palette, ca};

/// Oldest log lines are dropped past this; issues are kept regardless.
const MAX_LOG_LINES: usize = 20_000;
/// Longer lines (xcodebuild prints multi-KB compiler invocations) are cut to
/// this many bytes in the log view; issues are parsed from the full line first.
const MAX_LINE_BYTES: usize = 2_048;
const LOG_ROW_HEIGHT: f32 = 17.;
const ISSUE_ROW_HEIGHT: f32 = 24.;

pub enum BuildEvent {
    Open { path: PathBuf, line: u32, column: u32 },
    /// A run started: the workspace reveals the Build tab.
    Started,
}

#[derive(Clone, PartialEq)]
enum RunState {
    Idle,
    Running { action: BuildAction, name: String },
    Done { action: BuildAction, success: bool },
    Stopped,
    Message(SharedString),
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Log,
    Issues,
}

pub struct BuildPanel {
    root: Option<PathBuf>,
    projects: Vec<BuildProject>,
    detecting: bool,
    /// Selected project index and unit (scheme or module).
    selected: Option<(usize, Option<String>)>,
    simulators: Vec<Destination>,
    destination: Destination,
    picker_open: bool,
    log: VecDeque<SharedString>,
    issues: Vec<Issue>,
    issue_set: IssueSet,
    errors: usize,
    warnings: usize,
    state: RunState,
    stopping: bool,
    run: Option<RunHandle>,
    view: View,
    scroll: UniformListScrollHandle,
    run_task: Option<Task<()>>,
    detect_task: Option<Task<()>>,
    scheme_tasks: Vec<Task<()>>,
    sim_task: Option<Task<()>>,
}

impl EventEmitter<BuildEvent> for BuildPanel {}

/// Closing the window (or quitting, which drops every window) must not leave
/// xcodebuild/Gradle running: the build sits in its own process group, so no
/// terminal hangup reaches it.
impl Drop for BuildPanel {
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            run.stop();
        }
    }
}

impl BuildPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        BuildPanel {
            root: None,
            projects: Vec::new(),
            detecting: false,
            selected: None,
            simulators: Vec::new(),
            destination: Destination::generic_simulator(),
            picker_open: false,
            log: VecDeque::new(),
            issues: Vec::new(),
            issue_set: IssueSet::default(),
            errors: 0,
            warnings: 0,
            state: RunState::Idle,
            stopping: false,
            run: None,
            view: View::Log,
            scroll: UniformListScrollHandle::new(),
            run_task: None,
            detect_task: None,
            scheme_tasks: Vec::new(),
            sim_task: None,
        }
    }

    // ---- Detection -------------------------------------------------------

    /// Called on project open: detects build roots in the background, then
    /// lists Xcode schemes (slow) per project.
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.abandon_run(cx);
        self.root = Some(root.clone());
        self.projects.clear();
        self.selected = None;
        self.picker_open = false;
        self.scheme_tasks.clear();
        self.state = RunState::Idle;
        self.detecting = true;
        let scan_root = root.clone();
        let task = cx.background_spawn(async move { build::detect(&scan_root) });
        self.detect_task = Some(cx.spawn(async move |this, cx| {
            let projects = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.root.as_ref() != Some(&root) {
                    return;
                }
                this.detecting = false;
                crate::startup::mark("build detection ready");
                this.projects = projects;
                this.select_default();
                this.load_schemes(cx);
                if this.projects.iter().any(BuildProject::is_xcode) {
                    this.refresh_simulators(cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn load_schemes(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        for (ix, project) in self.projects.iter().enumerate() {
            let ProjectKind::Xcode { container, .. } = &project.kind else { continue };
            let (dir, container) = (project.dir.clone(), container.clone());
            let task = cx.background_spawn(async move { crate::scheme_cache::list_schemes(&dir, &container) });
            let root = root.clone();
            self.scheme_tasks.push(cx.spawn(async move |this, cx| {
                let result = task.await;
                crate::startup::mark("xcodebuild -list ready");
                let _ = this.update(cx, |this, cx| {
                    if this.root.as_ref() != Some(&root) {
                        return;
                    }
                    let Some(project) = this.projects.get_mut(ix) else { return };
                    match result {
                        Ok(list) => {
                            if let ProjectKind::Xcode { schemes, .. } = &mut project.kind {
                                *schemes = list;
                            }
                        }
                        Err(err) => eprintln!("forge: cannot list schemes of {}: {err}", project.dir.display()),
                    }
                    // Upgrade a "whole project" selection to the preferred scheme.
                    if this.selected.as_ref().is_some_and(|(p, unit)| *p == ix && unit.is_none()) {
                        this.selected = Some((ix, preferred_unit(&this.projects[ix])));
                    }
                    cx.notify();
                });
            }));
        }
    }

    fn refresh_simulators(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move { build::booted_simulators() });
        self.sim_task = Some(cx.spawn(async move |this, cx| {
            let sims = task.await;
            let _ = this.update(cx, |this, cx| {
                // Keep the user's pick if that simulator is still booted.
                if !sims.contains(&this.destination) && this.destination.spec.contains(",id=") {
                    this.destination = Destination::generic_simulator();
                }
                this.simulators = sims;
                cx.notify();
            });
        }));
    }

    fn select_default(&mut self) {
        // Prefer an app: an Xcode project, else a Gradle build with an app module.
        let ix = self
            .projects
            .iter()
            .position(|p| matches!(p.kind, ProjectKind::Xcode { container: build::XcodeContainer::Workspace(_) | build::XcodeContainer::Project(_), .. }))
            .or_else(|| {
                self.projects.iter().position(|p| match &p.kind {
                    ProjectKind::Gradle { modules } => modules.iter().any(|m| m.kind == ModuleKind::AndroidApp),
                    _ => false,
                })
            })
            .or(if self.projects.is_empty() { None } else { Some(0) });
        self.selected = ix.map(|ix| (ix, preferred_unit(&self.projects[ix])));
    }

    fn select(&mut self, project: usize, unit: Option<String>, cx: &mut Context<Self>) {
        self.selected = Some((project, unit));
        self.picker_open = false;
        cx.notify();
    }

    fn select_destination(&mut self, destination: Destination, cx: &mut Context<Self>) {
        self.destination = destination;
        self.picker_open = false;
        cx.notify();
    }

    pub fn picker_open(&self) -> bool {
        self.picker_open
    }

    pub fn close_picker(&mut self, cx: &mut Context<Self>) {
        self.picker_open = false;
        cx.notify();
    }

    fn toggle_picker(&mut self, cx: &mut Context<Self>) {
        self.picker_open = !self.picker_open;
        if self.picker_open && self.projects.iter().any(BuildProject::is_xcode) {
            self.refresh_simulators(cx);
        }
        cx.notify();
    }

    fn selected_project(&self) -> Option<(&BuildProject, Option<&str>)> {
        let (ix, unit) = self.selected.as_ref()?;
        Some((self.projects.get(*ix)?, unit.as_deref()))
    }

    /// Picker label, e.g. "DMote ▸ iPhone 16" or "D-Mote ▸ app".
    fn target_label(&self) -> SharedString {
        match self.selected_project() {
            Some((project, unit)) if project.is_xcode() => {
                format!("{} ▸ {}", unit.unwrap_or(&project.name), self.destination.label).into()
            }
            Some((project, Some(unit))) => format!("{} ▸ {unit}", project.name).into(),
            Some((project, None)) => project.name.clone().into(),
            None if self.detecting => "Detecting…".into(),
            None => "No build target".into(),
        }
    }

    // ---- Running ---------------------------------------------------------

    /// Whether an Xcode or Gradle project was found (the toolbar segment is
    /// hidden otherwise).
    pub fn has_projects(&self) -> bool {
        !self.projects.is_empty()
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, RunState::Running { .. })
    }

    /// Starts `action` on the selected target, stopping any run in flight.
    pub fn start(&mut self, action: BuildAction, cx: &mut Context<Self>) {
        self.picker_open = false;
        let Some((project, unit)) = self.selected_project() else {
            self.state = RunState::Message(if self.detecting { "Still detecting projects…" } else { "No buildable project found" }.into());
            cx.notify();
            return;
        };
        let Some(command) = build::command_for(project, unit, &self.destination, action) else {
            self.state = RunState::Message(format!("{} is not available for this target", action.verb()).into());
            cx.notify();
            return;
        };
        let name = unit.map(str::to_string).unwrap_or_else(|| project.name.clone());
        self.abandon_run(cx);
        self.log.clear();
        self.issues.clear();
        self.issue_set = IssueSet::default();
        self.errors = 0;
        self.warnings = 0;
        self.stopping = false;
        self.view = View::Log;
        self.push_line(format!("$ {}", command.display()).into());

        let (tx, rx) = flume::unbounded();
        match build::spawn(&command, tx) {
            Ok(handle) => self.run = Some(handle),
            Err(err) => {
                eprintln!("forge: cannot start {}: {err}", command.display());
                self.push_line(format!("forge: cannot start build: {err}").into());
                self.state = RunState::Done { action, success: false };
                cx.emit(BuildEvent::Started);
                cx.notify();
                return;
            }
        }
        self.state = RunState::Running { action, name };
        cx.emit(BuildEvent::Started);
        self.run_task = Some(cx.spawn(async move |this, cx| {
            // Each wake drains everything queued: one UI update per burst.
            while let Ok(first) = rx.recv_async().await {
                let mut batch = vec![first];
                batch.extend(rx.drain());
                let alive = this.update(cx, |this, cx| {
                    for event in batch {
                        this.handle_event(event, action);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
        }));
        cx.notify();
    }

    fn handle_event(&mut self, event: RunEvent, action: BuildAction) {
        let follow = self.scroll.is_scrolled_to_end().unwrap_or(true);
        match event {
            RunEvent::Line(text, issue) => {
                if let Some(issue) = issue {
                    if self.issue_set.insert(&issue) {
                        match issue.severity {
                            Severity::Error => self.errors += 1,
                            Severity::Warning => self.warnings += 1,
                        }
                        self.issues.push(issue);
                    }
                }
                self.push_line(text.into());
            }
            RunEvent::Exited(code) => {
                self.run = None;
                self.state = if self.stopping {
                    RunState::Stopped
                } else {
                    RunState::Done { action, success: code == Some(0) }
                };
                let tail = match code {
                    Some(code) => format!("forge: process exited with code {code}"),
                    None => "forge: process terminated".into(),
                };
                self.push_line(tail.into());
                if !matches!(self.state, RunState::Done { success: true, .. }) && self.errors > 0 {
                    self.view = View::Issues;
                }
            }
            RunEvent::Failed(message) => {
                eprintln!("forge: build {message}");
                self.run = None;
                self.state = RunState::Done { action, success: false };
                self.push_line(format!("forge: {message}").into());
            }
        }
        if follow {
            self.scroll.scroll_to_bottom();
        }
    }

    fn push_line(&mut self, line: SharedString) {
        let line = truncate_line(line);
        if self.log.len() >= MAX_LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    /// Stops the running process group; its exit arrives as a normal event
    /// and turns the state into "stopped".
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_ref() else { return };
        if !self.stopping {
            self.stopping = true;
            run.stop();
            cx.notify();
        }
    }

    /// Stops the current run and ignores whatever it still prints.
    fn abandon_run(&mut self, cx: &mut Context<Self>) {
        self.stop(cx);
        self.run = None;
        self.run_task = None;
        if self.is_running() {
            self.state = RunState::Stopped;
        }
    }

    /// Text for the toolbar activity pill; `None` when nothing to say.
    pub fn status(&self) -> Option<(SharedString, bool)> {
        let counts = |errors: usize, warnings: usize| -> String {
            let mut parts = Vec::new();
            if errors > 0 {
                parts.push(format!("{errors} error{}", if errors == 1 { "" } else { "s" }));
            }
            if warnings > 0 {
                parts.push(format!("{warnings} warning{}", if warnings == 1 { "" } else { "s" }));
            }
            if parts.is_empty() { String::new() } else { format!(" — {}", parts.join(", ")) }
        };
        match &self.state {
            RunState::Idle => None,
            RunState::Running { action, name } => Some((format!("{} {name}…", action.progressive()).into(), false)),
            RunState::Done { action, success: true } => {
                Some((format!("{} Succeeded{}", action.verb(), counts(0, self.warnings)).into(), false))
            }
            RunState::Done { action, success: false } => {
                Some((format!("{} Failed{}", action.verb(), counts(self.errors, self.warnings)).into(), true))
            }
            RunState::Stopped => Some(("Build Stopped".into(), false)),
            RunState::Message(message) => Some((message.clone(), true)),
        }
    }

    fn open_issue(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(issue) = self.issues.get(ix) else { return };
        let Some(path) = issue.path.clone() else { return };
        cx.emit(BuildEvent::Open {
            path,
            line: issue.line.saturating_sub(1),
            column: issue.column.saturating_sub(1),
        });
    }
}

fn preferred_unit(project: &BuildProject) -> Option<String> {
    match &project.kind {
        ProjectKind::Xcode { schemes, .. } => schemes
            .iter()
            .find(|s| **s == project.name)
            .or_else(|| schemes.first())
            .cloned(),
        ProjectKind::Gradle { modules } => modules
            .iter()
            .find(|m| m.kind == ModuleKind::AndroidApp)
            .or_else(|| modules.first())
            .map(|m| m.path.clone()),
    }
}

// ---- Rendering --------------------------------------------------------------

fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    crate::icons::ui_icon(name, size, color)
}

fn segment_button(
    id: &'static str,
    name: IconName,
    enabled: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded(px(6.))
        .when(enabled, |el| {
            el.cursor_pointer()
                .hover(|s| s.bg(ca(0xffffff, 0.08)))
                .active(|s| s.bg(ca(0xffffff, 0.14)))
                .on_click(move |_, window, cx| on_click(window, cx))
        })
        .child(icon(name, 15., if enabled { Palette::text() } else { Palette::faint() }))
        .into_any_element()
}

/// The Xcode-like toolbar segment: ▶ build, ■ stop, target picker.
pub fn toolbar_segment(build: &Entity<BuildPanel>, cx: &App) -> AnyElement {
    let panel = build.read(cx);
    let running = panel.is_running();
    let has_target = panel.selected.is_some();
    let label = panel.target_label();
    let open = panel.picker_open;
    let b3 = build.clone();
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .mr(px(8.))
        // Keep clicks off the toolbar, whose double-click zooms the window.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // Same path as ⇧⌘B / ⌘. so the toolbar also saves open files first.
        .child(segment_button("build-run", IconName::Play, has_target, |window, cx| {
            window.dispatch_action(Box::new(crate::Build), cx)
        }))
        .child(segment_button("build-stop", IconName::Square, running, |window, cx| {
            window.dispatch_action(Box::new(crate::StopBuild), cx)
        }))
        .child(
            div()
                .relative()
                .child(
                    div()
                        .id("build-target")
                        .h(px(24.))
                        .max_w(px(280.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(open, |el| el.bg(ca(0xffffff, 0.1)))
                        .hover(|s| s.bg(ca(0xffffff, 0.08)))
                        .text_size(px(12.))
                        .text_color(Palette::text())
                        .child(
                            div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label),
                        )
                        .child(icon(IconName::ChevronsUpDown, 11., Palette::muted()))
                        .on_click(move |_, _, cx| b3.update(cx, |b, cx| b.toggle_picker(cx))),
                )
                .when(open, |el| {
                    el.child(div().absolute().top(px(28.)).left_0().child(deferred(
                        anchored().snap_to_window_with_margin(px(8.)).child(
                            div()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(render_picker(build, cx)),
                        ),
                    )))
                }),
        )
        .into_any_element()
}

fn picker_row(id: SharedString, label: SharedString, checked: bool, indent: bool, on_click: impl Fn(&mut App) + 'static) -> AnyElement {
    div()
        .id(gpui_kit::ElementId::Name(id))
        .h(px(24.))
        .px(px(8.))
        .when(indent, |el| el.pl(px(14.)))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(|s| s.bg(Palette::accent()))
        .text_size(px(12.5))
        .child(div().w(px(12.)).flex_none().when(checked, |el| el.child(icon(IconName::Check, 11., Palette::text()))))
        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label))
        .on_click(move |_, _, cx| on_click(cx))
        .into_any_element()
}

fn picker_header(label: impl Into<SharedString>) -> AnyElement {
    div()
        .px(px(8.))
        .pt(px(6.))
        .pb(px(2.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Palette::muted())
        .child(label.into())
        .into_any_element()
}

fn render_picker(build: &Entity<BuildPanel>, cx: &App) -> AnyElement {
    let panel = build.read(cx);
    let mut rows: Vec<AnyElement> = Vec::new();
    if panel.projects.is_empty() {
        rows.push(picker_header(if panel.detecting { "Detecting projects…" } else { "No Xcode or Gradle project found" }));
    }
    for (pix, project) in panel.projects.iter().enumerate() {
        rows.push(picker_header(project.label()));
        for unit in project.units() {
            let checked = panel.selected.as_ref() == Some(&(pix, unit.clone()));
            let label: SharedString = unit.clone().unwrap_or_else(|| project.name.clone()).into();
            let build = build.clone();
            let id = format!("build-unit-{pix}-{label}").into();
            rows.push(picker_row(id, label, checked, true, move |cx| {
                let unit = unit.clone();
                build.update(cx, |b, cx| b.select(pix, unit, cx))
            }));
        }
    }
    if panel.selected_project().is_some_and(|(p, _)| p.is_xcode()) {
        rows.push(div().h(px(1.)).my(px(4.)).bg(Palette::hairline()).into_any_element());
        rows.push(picker_header("Destination"));
        let mut destinations = vec![Destination::generic_simulator()];
        destinations.extend(panel.simulators.iter().cloned());
        destinations.push(Destination::mac());
        for (ix, destination) in destinations.into_iter().enumerate() {
            let checked = panel.destination == destination;
            let build = build.clone();
            let label: SharedString = destination.label.clone().into();
            rows.push(picker_row(format!("build-dest-{ix}").into(), label, checked, true, move |cx| {
                let destination = destination.clone();
                build.update(cx, |b, cx| b.select_destination(destination, cx))
            }));
        }
    }
    div()
        .id("build-picker")
        .occlude()
        .min_w(px(240.))
        .max_w(px(420.))
        .max_h(px(480.))
        .overflow_y_scroll()
        .p(px(4.))
        .flex()
        .flex_col()
        .rounded(px(8.))
        .border_1()
        .border_color(Palette::hairline())
        .bg(Palette::toolbar_bg())
        .shadow_lg()
        .text_color(Palette::text())
        .children(rows)
        .into_any_element()
}

impl BuildPanel {
    fn render_log(&self, cx: &mut Context<Self>) -> AnyElement {
        uniform_list("build-log", self.log.len(), cx.processor(|this, range: std::ops::Range<usize>, _window, _cx| {
            range
                .filter_map(|ix| {
                    let line = this.log.get(ix)?.clone();
                    let color = line_color(&line);
                    Some(
                        div()
                            .h(px(LOG_ROW_HEIGHT))
                            .px(px(10.))
                            .whitespace_nowrap()
                            .text_color(color)
                            .child(line)
                            .into_any_element(),
                    )
                })
                .collect::<Vec<_>>()
        }))
        .track_scroll(&self.scroll)
        .flex_1()
        .font_family(MONO_FONT)
        .text_size(px(11.5))
        .into_any_element()
    }

    fn render_issues(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.issues.is_empty() {
            return div()
                .p(px(12.))
                .text_size(px(12.))
                .text_color(Palette::muted())
                .child("No issues")
                .into_any_element();
        }
        uniform_list("build-issues", self.issues.len(), cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
            range
                .filter_map(|ix| {
                    let issue = this.issues.get(ix)?;
                    let (name, color) = match issue.severity {
                        Severity::Error => (IconName::CircleX, Palette::git_deleted()),
                        Severity::Warning => (IconName::TriangleAlert, Palette::git_modified()),
                    };
                    let location = issue.path.as_ref().map(|p| {
                        let file = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        format!("{file}:{}", issue.line)
                    });
                    let clickable = issue.path.is_some();
                    Some(
                        div()
                            .id(("build-issue", ix))
                            .h(px(crate::icons::row_height(ISSUE_ROW_HEIGHT, crate::file_icons::ICON_SIZE)))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .when(clickable, |el| {
                                el.cursor_pointer()
                                    .hover(|s| s.bg(Palette::row_hover()))
                                    .on_click(cx.listener(move |this, _, _, cx| this.open_issue(ix, cx)))
                            })
                            .child(icon(name, 13., color))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(Palette::text())
                                    .child(issue.message.clone()),
                            )
                            .when_some(issue.path.as_ref().zip(location), |el, (path, location)| {
                                el.child(file_icons::file_icon_img(path))
                                    .child(div().flex_none().text_color(Palette::muted()).child(location))
                            })
                            .into_any_element(),
                    )
                })
                .collect::<Vec<_>>()
        }))
        .flex_1()
        .into_any_element()
    }
}

fn line_color(line: &str) -> Hsla {
    if line.contains(": error: ") || line.starts_with("e: ") || line.contains("BUILD FAILED") || line.starts_with("FAILURE") {
        Palette::git_deleted()
    } else if line.contains(": warning: ") || line.starts_with("w: ") {
        Palette::git_modified()
    } else if line.contains("BUILD SUCCEEDED") || line.starts_with("BUILD SUCCESSFUL") || line.contains("CLEAN SUCCEEDED") {
        Palette::git_added()
    } else if line.starts_with("$ ") || line.starts_with("forge: ") {
        Palette::accent()
    } else {
        Palette::muted()
    }
}

impl Render for BuildPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str, label: String, view: View, cx: &mut Context<Self>| {
            let active = self.view == view;
            div()
                .id(id)
                .px(px(8.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(if active { Palette::text() } else { Palette::muted() })
                .when(active, |el| el.bg(ca(0xffffff, 0.1)))
                .hover(|s| s.bg(ca(0xffffff, 0.06)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.view = view;
                    cx.notify();
                }))
        };
        let issue_label = match (self.errors, self.warnings) {
            (0, 0) => "Issues".to_string(),
            (e, w) => format!("Issues  {e} ✕  {w} ⚠"),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(26.))
                    .flex_none()
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .border_b_1()
                    .border_color(Palette::border())
                    .child(tab("build-view-log", "Log".into(), View::Log, cx))
                    .child(tab("build-view-issues", issue_label, View::Issues, cx)),
            )
            .child(match self.view {
                View::Log => self.render_log(cx),
                View::Issues => self.render_issues(cx),
            })
    }
}

/// Cuts `line` to `MAX_LINE_BYTES` (on a char boundary) and marks the cut.
fn truncate_line(line: SharedString) -> SharedString {
    if line.len() <= MAX_LINE_BYTES {
        return line;
    }
    let mut end = MAX_LINE_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} bytes)", &line[..end], line.len()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_log_lines_are_cut_on_a_char_boundary() {
        let short: SharedString = "ok".into();
        assert_eq!(truncate_line(short.clone()), short);
        let long: SharedString = "é".repeat(MAX_LINE_BYTES).into();
        let cut = truncate_line(long);
        assert!(cut.len() < MAX_LINE_BYTES + 32);
        assert!(cut.ends_with(&format!("… ({} bytes)", MAX_LINE_BYTES * 2)));
    }
}
