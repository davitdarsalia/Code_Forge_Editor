extern crate gpui_kit as gpui;

mod file_icons;
mod commands;
mod build;
mod build_panel;
mod builtin_lang;
mod clone_dialog;
mod files;
mod image_view;
mod preview;
mod preview_pane;
mod diagnostics;
mod document;
mod format;
mod git;
mod icons;
mod index_cache;
mod issues_panel;
mod languages;
mod markdown_model;
mod markdown_view;
mod net;
mod lsp;
mod quick_open;
mod recent;
mod scheme_cache;
mod search;
mod search_panel;
mod symbols;
mod rename;
mod settings;
mod startup;
mod terminal;
mod theme;
mod tools;
mod ui;
mod workspace;
mod xcode_lsp;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::OnceLock;

use gpui_kit::component::TitleBar;
use gpui_kit::{
    Action, AnyWindowHandle, App, AppContext as _, AssetSource, Bounds, Context, Global, KeyBinding,
    Menu, MenuItem, PathPromptOptions, SharedString, SystemMenuType, WeakEntity, Window,
    WindowBounds, WindowOptions, actions, px, size,
};

use settings::Settings;
use workspace::{CloseKind, Workspace};

actions!(
    forge,
    [
        Quit,
        NewWindow,
        OpenSettings,
        OpenFolder,
        CloneRepository,
        SaveFile,
        SaveAll,
        CloseTab,
        CloseOtherTabs,
        CloseTabsToRight,
        CloseTabsToLeft,
        CloseSavedTabs,
        CloseAllTabs,
        CopyPath,
        CopyRelativePath,
        NextTab,
        PreviousTab,
        ToggleNavigator,
        ShowFilesNavigator,
        ShowSearchNavigator,
        ShowChangesNavigator,
        RevealInNavigator,
        ToggleInspector,
        ToggleTerminal,
        FocusTerminal,
        NewTerminal,
        FocusCommit,
        ClearRecent,
        NewFile,
        QuickOpenFile,
        FindInProject,
        JumpToDefinition,
        RenameSymbol,
        GoBack,
        GoForward,
        FormatFile,
        GitPull,
        GitPush,
        GitFetch,
        Build,
        CleanBuild,
        StopBuild,
        InstallBuild,
        Minimize,
        Zoom,
        TogglePreview,
        ImageZoomIn,
        ImageZoomOut,
        ImageZoomFit,
    ]
);

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = forge, no_json)]
pub struct OpenRecent {
    pub path: PathBuf,
}

#[derive(Default)]
struct AppState {
    windows: Vec<(AnyWindowHandle, WeakEntity<Workspace>)>,
    settings_window: Option<AnyWindowHandle>,
}

impl Global for AppState {}

fn open_requests() -> &'static (flume::Sender<Vec<PathBuf>>, flume::Receiver<Vec<PathBuf>>) {
    static CHANNEL: OnceLock<(flume::Sender<Vec<PathBuf>>, flume::Receiver<Vec<PathBuf>>)> = OnceLock::new();
    CHANNEL.get_or_init(flume::unbounded)
}

struct Assets;

const EXTRA_ICONS: &[(&str, &[u8])] = &[
    ("icons/git-branch.svg", include_bytes!("../assets/icons/git-branch.svg")),
    ("icons/git-commit-horizontal.svg", include_bytes!("../assets/icons/git-commit-horizontal.svg")),
    ("icons/git-compare.svg", include_bytes!("../assets/icons/git-compare.svg")),
    ("icons/arrow-down-to-line.svg", include_bytes!("../assets/icons/arrow-down-to-line.svg")),
    ("icons/arrow-up-from-line.svg", include_bytes!("../assets/icons/arrow-up-from-line.svg")),
    ("icons/file-code.svg", include_bytes!("../assets/icons/file-code.svg")),
    ("icons/folder-git-2.svg", include_bytes!("../assets/icons/folder-git-2.svg")),
    ("icons/undo-2.svg", include_bytes!("../assets/icons/undo-2.svg")),
    ("icons/circle-dot.svg", include_bytes!("../assets/icons/circle-dot.svg")),
    ("icons/hammer.svg", include_bytes!("../assets/icons/hammer.svg")),
    ("icons/lightbulb.svg", include_bytes!("../assets/icons/lightbulb.svg")),
    ("icons/message-square-warning.svg", include_bytes!("../assets/icons/message-square-warning.svg")),
    ("icons/octagon-alert.svg", include_bytes!("../assets/icons/octagon-alert.svg")),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = EXTRA_ICONS.iter().chain(file_icons::ASSETS).find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut list = gpui_kit::assets::Assets.list(path)?;
        list.extend(EXTRA_ICONS.iter().chain(file_icons::ASSETS).filter(|(p, _)| p.starts_with(path)).map(|(p, _)| (*p).into()));
        Ok(list)
    }
}

fn target_workspace(cx: &App) -> Option<(AnyWindowHandle, WeakEntity<Workspace>)> {
    let state = cx.try_global::<AppState>()?;
    let live = |handle: AnyWindowHandle| {
        state.windows.iter().find(|(w, ws)| *w == handle && ws.upgrade().is_some()).cloned()
    };
    cx.active_window()
        .and_then(live)
        .or_else(|| cx.window_stack()?.into_iter().find_map(live))
        .or_else(|| state.windows.iter().rev().find(|(_, ws)| ws.upgrade().is_some()).cloned())
}

fn update_workspace(
    cx: &mut App,
    (window, workspace): (AnyWindowHandle, WeakEntity<Workspace>),
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>),
) {
    let result = window.update(cx, |_, window, cx| {
        if let Some(workspace) = workspace.upgrade() {
            workspace.update(cx, |ws, cx| f(ws, window, cx));
        }
    });
    if let Err(err) = result {
        eprintln!("forge: window is gone: {err}");
    }
}

fn with_workspace(cx: &mut App, f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static) {
    cx.defer(move |cx| {
        if let Some(target) = target_workspace(cx) {
            update_workspace(cx, target, f);
        }
    });
}

fn with_workspace_or_new(
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) {
    cx.defer(move |cx| {
        if let Some(target) = target_workspace(cx).or_else(|| open_workspace_window(cx)) {
            update_workspace(cx, target, f);
        }
    });
}

fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1360.), px(860.)), cx);
    let window_bounds = if Settings::get(cx).launch_full_screen {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    };
    WindowOptions {
        window_bounds: Some(window_bounds),
        window_min_size: Some(size(px(720.), px(420.))),
        ..TitleBar::window_options()
    }
}

fn open_workspace_window(cx: &mut App) -> Option<(AnyWindowHandle, WeakEntity<Workspace>)> {
    let options = window_options(cx);
    match gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Workspace::new(window, cx))) {
        Ok((window, workspace)) => {
            let entry = (window, workspace.downgrade());
            let state = cx.default_global::<AppState>();
            state.windows.retain(|(_, ws)| ws.upgrade().is_some());
            state.windows.push(entry.clone());
            Some(entry)
        }
        Err(err) => {
            eprintln!("forge: failed to open window: {err}");
            None
        }
    }
}

fn with_active_window(cx: &mut App, f: impl FnOnce(&mut Window) + 'static) {
    cx.defer(move |cx| {
        let Some(handle) = cx.active_window() else { return };
        if let Err(err) = handle.update(cx, |_, window, _| f(window)) {
            eprintln!("forge: window is gone: {err}");
        }
    });
}

fn open_settings(cx: &mut App) {
    let existing = cx.try_global::<AppState>().and_then(|s| s.settings_window);
    if let Some(window) = existing {
        if window.update(cx, |_, window, _| window.activate_window()).is_ok() {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(460.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        is_resizable: false,
        is_minimizable: false,
        ..TitleBar::window_options()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title("Settings");
        cx.new(|cx| settings::SettingsView::new(window, cx))
    }) {
        Ok((window, _)) => cx.default_global::<AppState>().settings_window = Some(window),
        Err(err) => eprintln!("forge: failed to open settings: {err}"),
    }
}

fn close_settings_if_active(cx: &mut App) -> bool {
    let settings = cx.try_global::<AppState>().and_then(|s| s.settings_window);
    let Some(window) = settings.filter(|w| cx.active_window() == Some(*w)) else { return false };
    cx.defer(move |cx| {
        if let Err(err) = window.update(cx, |_, window, _| window.remove_window()) {
            eprintln!("forge: cannot close settings: {err}");
        }
    });
    true
}

pub fn set_menus(cx: &mut App) {
    let home = dirs::home_dir();
    let mut recent_items: Vec<MenuItem> = recent::load()
        .into_iter()
        .map(|path| {
            let label = match home.as_ref().and_then(|h| path.strip_prefix(h).ok()) {
                Some(rest) => format!("~/{}", rest.display()),
                None => path.display().to_string(),
            };
            MenuItem::action(label, OpenRecent { path })
        })
        .collect();
    if !recent_items.is_empty() {
        recent_items.push(MenuItem::separator());
    }
    recent_items.push(MenuItem::action("Clear Menu", ClearRecent));

    use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
    use gpui_kit::OsAction;
    cx.set_menus([
        Menu::new("Forge").items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Forge", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Window", NewWindow),
            MenuItem::action("New File…", NewFile),
            MenuItem::separator(),
            MenuItem::action("Open…", OpenFolder),
            MenuItem::submenu(Menu::new("Open Recent").items(recent_items)),
            MenuItem::action("Open Quickly…", QuickOpenFile),
            MenuItem::action("Clone Git Repository…", CloneRepository),
            MenuItem::separator(),
            MenuItem::action("Save", SaveFile),
            MenuItem::action("Save All", SaveAll),
            MenuItem::action("Close Tab", CloseTab),
            MenuItem::action("Close Other Tabs", CloseOtherTabs),
            MenuItem::action("Close Tabs to the Right", CloseTabsToRight),
            MenuItem::action("Close Tabs to the Left", CloseTabsToLeft),
            MenuItem::action("Close Saved Tabs", CloseSavedTabs),
            MenuItem::action("Close All Tabs", CloseAllTabs),
            MenuItem::separator(),
            MenuItem::action("Copy Path", CopyPath),
            MenuItem::action("Copy Relative Path", CopyRelativePath),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Format File", FormatFile),
            MenuItem::action("Rename Symbol…", RenameSymbol),
        ]),
        Menu::new("View").items([
            MenuItem::submenu(Menu::new("Navigators").items([
                MenuItem::action("Show Files Navigator", ShowFilesNavigator),
                MenuItem::action("Show Search Navigator", ShowSearchNavigator),
                MenuItem::action("Show Changes Navigator", ShowChangesNavigator),
            ])),
            MenuItem::action("Toggle Navigator", ToggleNavigator),
            MenuItem::action("Toggle Inspector", ToggleInspector),
            MenuItem::action("Show Preview", TogglePreview),
            MenuItem::separator(),
            MenuItem::action("Zoom In", ImageZoomIn),
            MenuItem::action("Zoom Out", ImageZoomOut),
            MenuItem::action("Zoom to Fit", ImageZoomFit),
            MenuItem::separator(),
            MenuItem::action("Toggle Terminal", ToggleTerminal),
            MenuItem::action("Focus Terminal", FocusTerminal),
            MenuItem::action("New Terminal", NewTerminal),
        ]),
        Menu::new("Find").items([MenuItem::action("Find in Project…", FindInProject)]),
        Menu::new("Navigate").items([
            MenuItem::action("Reveal in Navigator", RevealInNavigator),
            MenuItem::separator(),
            MenuItem::action("Jump to Definition", JumpToDefinition),
            MenuItem::action("Go Back", GoBack),
            MenuItem::action("Go Forward", GoForward),
            MenuItem::separator(),
            MenuItem::action("Show Next Tab", NextTab),
            MenuItem::action("Show Previous Tab", PreviousTab),
        ]),
        Menu::new("Source Control").items([
            MenuItem::action("Commit…", FocusCommit),
            MenuItem::separator(),
            MenuItem::action("Pull", GitPull),
            MenuItem::action("Push", GitPush),
            MenuItem::action("Fetch", GitFetch),
        ]),
        Menu::new("Product").items([
            MenuItem::action("Build", Build),
            MenuItem::action("Install (Gradle)", InstallBuild),
            MenuItem::action("Clean Build Folder", CleanBuild),
            MenuItem::separator(),
            MenuItem::action("Stop", StopBuild),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
    ]);
}

fn register_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| {
        cx.defer(|cx| {
            let windows = cx.try_global::<AppState>().map(|s| s.windows.clone()).unwrap_or_default();
            for target in windows.into_iter().filter(|(_, ws)| ws.upgrade().is_some()) {
                update_workspace(cx, target, |ws, _, cx| {
                    ws.save_all(true, cx);
                });
            }
            cx.quit();
        });
    });
    cx.on_action(|_: &NewWindow, cx| {
        cx.defer(|cx| {
            open_workspace_window(cx);
        })
    });
    cx.on_action(|_: &OpenSettings, cx| cx.defer(open_settings));
    cx.on_action(|_: &OpenFolder, cx| {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = cx.update(|cx| {
                    with_workspace_or_new(cx, move |ws, window, cx| ws.open_paths(&paths, window, cx))
                });
            }
        })
        .detach();
    });
    cx.on_action(|_: &NewFile, cx| {
        let dir = recent::load().into_iter().next().or_else(dirs::home_dir).unwrap_or_default();
        let picked = cx.prompt_for_new_path(&dir, Some("untitled.txt"));
        cx.spawn(async move |cx| {
            if let Ok(Ok(Some(path))) = picked.await {
                if let Err(err) = std::fs::write(&path, b"") {
                    eprintln!("forge: cannot create {}: {err}", path.display());
                    return;
                }
                let _ = cx.update(|cx| {
                    with_workspace_or_new(cx, move |ws, window, cx| ws.open_paths(&[path], window, cx))
                });
            }
        })
        .detach();
    });
    cx.on_action(|action: &OpenRecent, cx| {
        let path = action.path.clone();
        with_workspace_or_new(cx, move |ws, window, cx| ws.open_paths(&[path], window, cx));
    });
    cx.on_action(|_: &ClearRecent, cx| {
        recent::clear();
        set_menus(cx);
    });
    cx.on_action(|_: &CloneRepository, cx| {
        with_workspace_or_new(cx, |ws, window, cx| ws.show_clone(window, cx))
    });
    cx.on_action(|_: &SaveFile, cx| with_workspace(cx, |ws, _, cx| {
        ws.save_all(true, cx);
    }));
    cx.on_action(|_: &SaveAll, cx| with_workspace(cx, |ws, _, cx| {
        ws.save_all(true, cx);
    }));
    cx.on_action(|_: &CloseTab, cx| {
        if !close_settings_if_active(cx) {
            with_workspace(cx, |ws, window, cx| ws.close_active(window, cx))
        }
    });
    cx.on_action(|_: &CloseOtherTabs, cx| with_workspace(cx, |ws, window, cx| ws.close_tabs(CloseKind::Others, None, window, cx)));
    cx.on_action(|_: &CloseTabsToRight, cx| with_workspace(cx, |ws, window, cx| ws.close_tabs(CloseKind::Right, None, window, cx)));
    cx.on_action(|_: &CloseTabsToLeft, cx| with_workspace(cx, |ws, window, cx| ws.close_tabs(CloseKind::Left, None, window, cx)));
    cx.on_action(|_: &CloseSavedTabs, cx| with_workspace(cx, |ws, window, cx| ws.close_tabs(CloseKind::Saved, None, window, cx)));
    cx.on_action(|_: &CloseAllTabs, cx| with_workspace(cx, |ws, window, cx| ws.close_tabs(CloseKind::All, None, window, cx)));
    cx.on_action(|_: &CopyPath, cx| with_workspace(cx, |ws, _, cx| ws.copy_tab_path(false, None, cx)));
    cx.on_action(|_: &CopyRelativePath, cx| with_workspace(cx, |ws, _, cx| ws.copy_tab_path(true, None, cx)));
    cx.on_action(|_: &NextTab, cx| with_workspace(cx, |ws, window, cx| ws.cycle_tab(true, window, cx)));
    cx.on_action(|_: &PreviousTab, cx| with_workspace(cx, |ws, window, cx| ws.cycle_tab(false, window, cx)));
    cx.on_action(|_: &ToggleNavigator, cx| with_workspace(cx, |ws, _, cx| ws.toggle_left(cx)));
    cx.on_action(|_: &ShowFilesNavigator, cx| with_workspace(cx, |ws, window, cx| ws.show_navigator(0, window, cx)));
    cx.on_action(|_: &ShowSearchNavigator, cx| with_workspace(cx, |ws, window, cx| ws.show_navigator(1, window, cx)));
    cx.on_action(|_: &ShowChangesNavigator, cx| with_workspace(cx, |ws, window, cx| ws.show_navigator(2, window, cx)));
    cx.on_action(|_: &RevealInNavigator, cx| with_workspace(cx, |ws, _, cx| ws.reveal_active(cx)));
    cx.on_action(|_: &ToggleInspector, cx| with_workspace(cx, |ws, _, cx| ws.toggle_right(cx)));
    cx.on_action(|_: &TogglePreview, cx| with_workspace(cx, |ws, _, cx| ws.toggle_preview(cx)));
    cx.on_action(|_: &ImageZoomIn, cx| with_workspace(cx, |ws, _, cx| ws.zoom_image(workspace::ImageZoom::In, cx)));
    cx.on_action(|_: &ImageZoomOut, cx| with_workspace(cx, |ws, _, cx| ws.zoom_image(workspace::ImageZoom::Out, cx)));
    cx.on_action(|_: &ImageZoomFit, cx| with_workspace(cx, |ws, _, cx| ws.zoom_image(workspace::ImageZoom::Fit, cx)));
    cx.on_action(|_: &ToggleTerminal, cx| {
        with_workspace(cx, |ws, window, cx| ws.toggle_terminal(window, cx))
    });
    cx.on_action(|_: &FocusTerminal, cx| with_workspace(cx, |ws, window, cx| ws.focus_terminal(window, cx)));
    cx.on_action(|_: &NewTerminal, cx| with_workspace(cx, |ws, window, cx| ws.new_terminal(window, cx)));
    cx.on_action(|_: &FocusCommit, cx| with_workspace(cx, |ws, window, cx| ws.focus_commit(window, cx)));

    cx.on_action(|_: &QuickOpenFile, cx| {
        with_workspace(cx, |ws, window, cx| ws.show_quick_open(window, cx))
    });
    cx.on_action(|_: &FindInProject, cx| {
        with_workspace(cx, |ws, window, cx| ws.show_search(window, cx))
    });
    cx.on_action(|_: &JumpToDefinition, cx| with_workspace(cx, |ws, window, cx| ws.go_to_definition(window, cx)));
    cx.on_action(|_: &RenameSymbol, cx| with_workspace(cx, |ws, window, cx| ws.rename_symbol(window, cx)));
    cx.on_action(|_: &GoBack, cx| with_workspace(cx, |ws, window, cx| ws.step_history(true, window, cx)));
    cx.on_action(|_: &GoForward, cx| with_workspace(cx, |ws, window, cx| ws.step_history(false, window, cx)));
    cx.on_action(|_: &GitPull, cx| with_workspace(cx, |ws, window, cx| ws.run_remote("Pull", git::pull, window, cx)));
    cx.on_action(|_: &GitPush, cx| with_workspace(cx, |ws, window, cx| ws.run_remote("Push", git::push, window, cx)));
    cx.on_action(|_: &GitFetch, cx| with_workspace(cx, |ws, window, cx| ws.run_remote("Fetch", git::fetch, window, cx)));
    cx.on_action(|_: &Minimize, cx| with_active_window(cx, |window| window.minimize_window()));
    cx.on_action(|_: &Zoom, cx| with_active_window(cx, |window| window.zoom_window()));

    cx.on_action(|_: &FormatFile, cx| {
        with_workspace(cx, |ws, window, cx| ws.format_active(window, cx))
    });

    use build::BuildAction;
    cx.on_action(|_: &Build, cx| with_workspace(cx, |ws, _, cx| ws.run_build(BuildAction::Build, cx)));
    cx.on_action(|_: &CleanBuild, cx| with_workspace(cx, |ws, _, cx| ws.run_build(BuildAction::Clean, cx)));
    cx.on_action(|_: &InstallBuild, cx| with_workspace(cx, |ws, _, cx| ws.run_build(BuildAction::Install, cx)));
    cx.on_action(|_: &StopBuild, cx| with_workspace(cx, |ws, _, cx| ws.stop_build(cx)));

    cx.bind_keys(key_bindings());
}

fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-i", FormatFile, None),
        KeyBinding::new("cmd-shift-b", Build, None),
        KeyBinding::new("cmd-shift-k", CleanBuild, None),
        KeyBinding::new("cmd-shift-i", InstallBuild, None),
        KeyBinding::new("cmd-.", StopBuild, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-shift-n", NewWindow, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-p", QuickOpenFile, None),
        KeyBinding::new("cmd-shift-o", QuickOpenFile, None),
        KeyBinding::new("cmd-shift-f", FindInProject, None),
        KeyBinding::new("cmd-n", NewFile, None),
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("cmd-shift-c", CloneRepository, None),
        KeyBinding::new("cmd-s", SaveFile, None),
        KeyBinding::new("cmd-alt-s", SaveAll, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-}", NextTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("cmd-shift-[", PreviousTab, None),
        KeyBinding::new("cmd-{", PreviousTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        KeyBinding::new("cmd-b", ToggleNavigator, None),
        KeyBinding::new("cmd-0", ToggleNavigator, None),
        KeyBinding::new("cmd-1", ShowFilesNavigator, None),
        KeyBinding::new("cmd-2", ShowSearchNavigator, None),
        KeyBinding::new("cmd-3", ShowChangesNavigator, None),
        KeyBinding::new("cmd-shift-j", RevealInNavigator, None),
        KeyBinding::new("ctrl-cmd-j", JumpToDefinition, None),
        KeyBinding::new("f2", RenameSymbol, None),
        KeyBinding::new("ctrl-cmd-left", GoBack, None),
        KeyBinding::new("ctrl-cmd-right", GoForward, None),
        KeyBinding::new("cmd-r", ToggleInspector, None),
        KeyBinding::new("cmd-alt-0", ToggleInspector, None),
        KeyBinding::new("cmd-j", ToggleTerminal, None),
        KeyBinding::new("cmd-shift-y", ToggleTerminal, None),
        KeyBinding::new("ctrl-`", FocusTerminal, None),
        KeyBinding::new("ctrl-shift-`", NewTerminal, None),
        KeyBinding::new("ctrl-~", NewTerminal, None),
        KeyBinding::new("cmd-g", FocusCommit, None),
        KeyBinding::new("cmd-alt-c", CopyPath, None),
        KeyBinding::new("cmd-alt-shift-c", CopyRelativePath, None),
        KeyBinding::new("cmd-alt-t", CloseOtherTabs, None),
        KeyBinding::new("cmd-k cmd-right", CloseTabsToRight, None),
        KeyBinding::new("cmd-k cmd-left", CloseTabsToLeft, None),
        KeyBinding::new("cmd-k u", CloseSavedTabs, None),
        KeyBinding::new("cmd-k cmd-w", CloseAllTabs, None),
        KeyBinding::new("cmd-alt-x", GitPull, None),
        KeyBinding::new("cmd-alt-p", GitPush, None),
        KeyBinding::new("cmd-alt-f", GitFetch, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-alt-enter", TogglePreview, None),
        KeyBinding::new("cmd-=", ImageZoomIn, None),
        KeyBinding::new("cmd-+", ImageZoomIn, None),
        KeyBinding::new("cmd-shift-=", ImageZoomIn, None),
        KeyBinding::new("cmd--", ImageZoomOut, None),
        KeyBinding::new("cmd-0", ImageZoomFit, Some("ImageView")),
        KeyBinding::new("cmd-shift-f", FindInProject, Some("Input")),
        KeyBinding::new("cmd-.", StopBuild, Some("Input")),
    ]
}

fn main() {
    startup::init();
    let launch_paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    let app = gpui_kit::application().with_assets(Assets).with_http_client(std::sync::Arc::new(net::CurlClient));
    app.on_open_urls(|urls| {
        let paths: Vec<PathBuf> = urls
            .iter()
            .filter_map(|url| url.strip_prefix("file://"))
            .map(|p| PathBuf::from(percent_decode(p)))
            .collect();
        if !paths.is_empty() {
            let _ = open_requests().0.send(paths);
        }
    });

    app.run(move |cx: &mut App| {
        startup::mark("app.run callback");
        gpui_kit::init(cx);
        startup::mark("gpui_kit::init");
        let loaded = settings::load();
        loaded.apply_statics();
        cx.set_global(loaded);
        languages::register();
        startup::mark("settings + languages");
        theme::apply(cx);
        startup::mark("theme applied");
        register_actions(cx);
        startup::mark("actions + keybindings");
        cx.activate(true);

        let Some(target) = open_workspace_window(cx) else { return };
        startup::mark("window opened");
        update_workspace(cx, target.clone(), |_, window, _| {
            window.on_next_frame(|_, cx| {
                set_menus(cx);
                startup::mark("menus built (after first frame)");
            });
        });

        let mut initial = launch_paths.clone();
        initial.extend(open_requests().1.drain().flatten());
        if initial.is_empty() && Settings::get(cx).open_last_project {
            initial.extend(recent::load().into_iter().next());
        }
        update_workspace(cx, target, move |ws, window, cx| ws.open_paths(&initial, window, cx));
        startup::mark("open_paths returned");

        cx.spawn(async move |cx| {
            while let Ok(paths) = open_requests().1.recv_async().await {
                let _ = cx.update(|cx| {
                    with_workspace_or_new(cx, move |ws, window, cx| ws.open_paths(&paths, window, cx));
                    cx.activate(true);
                });
            }
        })
        .detach();
    });
}

fn percent_decode(input: &str) -> String {
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_escapes_and_non_ascii() {
        assert_eq!(percent_decode("/Users/me/My%20Project"), "/Users/me/My Project");
        assert_eq!(percent_decode("/a/%C3%A9t%C3%A9"), "/a/été");
        assert_eq!(percent_decode("/a/%é%"), "/a/%é%");
        assert_eq!(percent_decode("/a/100%"), "/a/100%");
        assert_eq!(percent_decode("/a/%zz"), "/a/%zz");
    }
}
