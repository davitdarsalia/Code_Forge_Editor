extern crate gpui_kit as gpui;

mod file_icons;
mod commands;
mod build;
mod build_panel;
mod files;
mod format;
mod git;
mod languages;
mod preview;
mod quick_open;
mod recent;
mod scheme_cache;
mod search;
mod search_panel;
mod symbols;
mod settings;
mod startup;
mod terminal;
mod theme;
mod workspace;

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
use workspace::Workspace;

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
        TogglePreview,
        FocusCommit,
        ClearRecent,
        NewFile,
        QuickOpenFile,
        FindInProject,
        FormatFile,
        CommitChanges,
        Build,
        CleanBuild,
        StopBuild,
        InstallBuild,
    ]
);

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = forge, no_json)]
pub struct OpenRecent {
    pub path: PathBuf,
}

/// Every open project window, so app-level actions (menus, shortcuts) can
/// reach the workspace of whichever window is in front.
#[derive(Default)]
struct AppState {
    windows: Vec<(AnyWindowHandle, WeakEntity<Workspace>)>,
    settings_window: Option<AnyWindowHandle>,
}

impl Global for AppState {}

/// Paths handed to the app by Finder ("Open With", dock drops).
fn open_requests() -> &'static (flume::Sender<Vec<PathBuf>>, flume::Receiver<Vec<PathBuf>>) {
    static CHANNEL: OnceLock<(flume::Sender<Vec<PathBuf>>, flume::Receiver<Vec<PathBuf>>)> = OnceLock::new();
    CHANNEL.get_or_init(flume::unbounded)
}

/// Bundles the default component icons plus a few extra Lucide icons.
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

/// The workspace window actions should go to: the key window if it is a
/// project window, else the frontmost project window, else the newest one.
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

/// Runs `f` on the front workspace. Deferred because action handlers run
/// while the dispatching window is leased out of the app: updating that same
/// window synchronously fails ("window not found"), which is what made menu
/// items such as Open Recent silently do nothing.
fn with_workspace(cx: &mut App, f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static) {
    cx.defer(move |cx| {
        if let Some(target) = target_workspace(cx) {
            update_workspace(cx, target, f);
        }
    });
}

/// Like `with_workspace`, but opens a fresh window when none is left (macOS
/// apps keep running after their last window closes).
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
        // Zoomed to fill the screen like Xcode, not a separate fullscreen Space.
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

/// Opens a new project window on the welcome screen and registers it.
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

fn open_settings(cx: &mut App) {
    let existing = cx.try_global::<AppState>().and_then(|s| s.settings_window);
    if let Some(window) = existing {
        if window.update(cx, |_, window, _| window.activate_window()).is_ok() {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(440.), px(400.)), cx);
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

/// ⌘W in the Settings window closes it instead of a tab elsewhere.
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
            MenuItem::action("Re-Indent / Format", FormatFile),
        ]),
        Menu::new("Find").items([
            MenuItem::action("Find in Project…", FindInProject),
            MenuItem::action("Open Quickly…", QuickOpenFile),
        ]),
        Menu::new("View").items([
            MenuItem::submenu(Menu::new("Navigators").items([
                MenuItem::action("Show Files Navigator", ShowFilesNavigator),
                MenuItem::action("Show Search Navigator", ShowSearchNavigator),
                MenuItem::action("Show Changes Navigator", ShowChangesNavigator),
            ])),
            MenuItem::action("Toggle Navigator", ToggleNavigator),
            MenuItem::action("Toggle Inspector", ToggleInspector),
            MenuItem::action("Toggle Canvas", TogglePreview),
            MenuItem::separator(),
            MenuItem::action("Toggle Terminal", ToggleTerminal),
            MenuItem::action("Focus Terminal", FocusTerminal),
            MenuItem::action("New Terminal", NewTerminal),
        ]),
        Menu::new("Product").items([
            MenuItem::action("Build", Build),
            MenuItem::action("Install (Gradle)", InstallBuild),
            MenuItem::action("Clean Build Folder", CleanBuild),
            MenuItem::separator(),
            MenuItem::action("Stop", StopBuild),
        ]),
        Menu::new("Navigate").items([
            MenuItem::action("Reveal in Navigator", RevealInNavigator),
            MenuItem::separator(),
            MenuItem::action("Show Next Tab", NextTab),
            MenuItem::action("Show Previous Tab", PreviousTab),
        ]),
        Menu::new("Source Control").items([MenuItem::action("Commit…", FocusCommit)]),
    ]);
}

fn register_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| {
        // Deferred so the window that dispatched ⌘Q is not leased while saving.
        cx.defer(|cx| {
            let windows = cx.try_global::<AppState>().map(|s| s.windows.clone()).unwrap_or_default();
            for target in windows.into_iter().filter(|(_, ws)| ws.upgrade().is_some()) {
                update_workspace(cx, target, |ws, _, cx| ws.save_all(cx));
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
    cx.on_action(|_: &SaveFile, cx| with_workspace(cx, |ws, _, cx| ws.save_all(cx)));
    cx.on_action(|_: &SaveAll, cx| with_workspace(cx, |ws, _, cx| ws.save_all(cx)));
    cx.on_action(|_: &CloseTab, cx| {
        if !close_settings_if_active(cx) {
            with_workspace(cx, |ws, window, cx| ws.close_active(window, cx))
        }
    });
    cx.on_action(|_: &NextTab, cx| with_workspace(cx, |ws, window, cx| ws.cycle_tab(true, window, cx)));
    cx.on_action(|_: &PreviousTab, cx| with_workspace(cx, |ws, window, cx| ws.cycle_tab(false, window, cx)));
    cx.on_action(|_: &ToggleNavigator, cx| with_workspace(cx, |ws, _, cx| ws.toggle_left(cx)));
    cx.on_action(|_: &ShowFilesNavigator, cx| with_workspace(cx, |ws, window, cx| ws.show_navigator(0, window, cx)));
    cx.on_action(|_: &ShowSearchNavigator, cx| with_workspace(cx, |ws, window, cx| ws.show_navigator(1, window, cx)));
    cx.on_action(|_: &ShowChangesNavigator, cx| with_workspace(cx, |ws, window, cx| ws.show_navigator(2, window, cx)));
    cx.on_action(|_: &RevealInNavigator, cx| with_workspace(cx, |ws, _, cx| ws.reveal_active(cx)));
    cx.on_action(|_: &ToggleInspector, cx| with_workspace(cx, |ws, _, cx| ws.toggle_right(cx)));
    cx.on_action(|_: &ToggleTerminal, cx| {
        with_workspace(cx, |ws, window, cx| ws.toggle_terminal(window, cx))
    });
    cx.on_action(|_: &FocusTerminal, cx| with_workspace(cx, |ws, window, cx| ws.focus_terminal(window, cx)));
    cx.on_action(|_: &NewTerminal, cx| with_workspace(cx, |ws, window, cx| ws.new_terminal(window, cx)));
    cx.on_action(|_: &TogglePreview, cx| {
        with_workspace(cx, |ws, window, cx| ws.toggle_preview(window, cx))
    });
    cx.on_action(|_: &FocusCommit, cx| with_workspace(cx, |ws, window, cx| ws.focus_commit(window, cx)));

    cx.on_action(|_: &QuickOpenFile, cx| {
        with_workspace(cx, |ws, window, cx| ws.show_quick_open(window, cx))
    });
    cx.on_action(|_: &FindInProject, cx| {
        with_workspace(cx, |ws, window, cx| ws.show_search(window, cx))
    });
    cx.on_action(|_: &CommitChanges, cx| {
        with_workspace(cx, |ws, window, cx| ws.show_commit(window, cx))
    });

    cx.on_action(|_: &FormatFile, cx| {
        with_workspace(cx, |ws, window, cx| ws.format_active(window, cx))
    });

    use build::BuildAction;
    cx.on_action(|_: &Build, cx| with_workspace(cx, |ws, _, cx| ws.run_build(BuildAction::Build, cx)));
    cx.on_action(|_: &CleanBuild, cx| with_workspace(cx, |ws, _, cx| ws.run_build(BuildAction::Clean, cx)));
    cx.on_action(|_: &InstallBuild, cx| with_workspace(cx, |ws, _, cx| ws.run_build(BuildAction::Install, cx)));
    cx.on_action(|_: &StopBuild, cx| with_workspace(cx, |ws, _, cx| ws.stop_build(cx)));

    // Keybindings dispatch before key-down listeners, so these also fire while
    // the terminal is focused. The menu shows the FIRST binding of an action,
    // so the canonical one is listed first and fallbacks after.
    cx.bind_keys([
        // Global (no context): nothing in the editor's "Input" context binds
        // cmd-i, so this reaches the app handler from inside the editor too.
        KeyBinding::new("cmd-i", FormatFile, None),
        KeyBinding::new("cmd-b", Build, None),
        KeyBinding::new("cmd-shift-k", CleanBuild, None),
        KeyBinding::new("cmd-.", StopBuild, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-shift-n", NewWindow, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-shift-o", QuickOpenFile, None),
        KeyBinding::new("cmd-p", QuickOpenFile, None),
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
        KeyBinding::new("cmd-0", ToggleNavigator, None),
        KeyBinding::new("cmd-1", ShowFilesNavigator, None),
        KeyBinding::new("cmd-2", ShowSearchNavigator, None),
        KeyBinding::new("cmd-3", ShowChangesNavigator, None),
        KeyBinding::new("cmd-shift-j", RevealInNavigator, None),
        KeyBinding::new("cmd-alt-0", ToggleInspector, None),
        KeyBinding::new("cmd-shift-y", ToggleTerminal, None),
        KeyBinding::new("ctrl-`", FocusTerminal, None),
        KeyBinding::new("ctrl-shift-`", NewTerminal, None),
        KeyBinding::new("ctrl-~", NewTerminal, None),
        KeyBinding::new("cmd-alt-enter", TogglePreview, None),
        KeyBinding::new("cmd-alt-c", FocusCommit, None),
        // The text input binds ⌘⇧F to Replace in its "Input" context, which
        // outranks context-free bindings; re-bind it there (added later, so it
        // wins) so Find in Project works from the editor too.
        KeyBinding::new("cmd-shift-f", FindInProject, Some("Input")),
    ]);
}

fn main() {
    startup::init();
    let launch_paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    let app = gpui_kit::application().with_assets(Assets);
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
        cx.set_global(settings::load());
        languages::register();
        startup::mark("settings + languages");
        theme::apply(cx);
        startup::mark("theme applied");
        register_actions(cx);
        startup::mark("actions + keybindings");
        cx.activate(true);

        let Some(target) = open_workspace_window(cx) else { return };
        startup::mark("window opened");
        // Building the native menu bar costs ~20 ms the first time; nothing
        // shows it before the window's first frame, so do it right after.
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

        // Finder requests that arrive while the app is running.
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
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
