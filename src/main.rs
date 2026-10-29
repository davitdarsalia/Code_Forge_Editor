extern crate gpui_kit as gpui;

mod file_icons;
mod commands;
mod files;
mod format;
mod git;
mod languages;
mod preview;
mod quick_open;
mod recent;
mod search;
mod search_panel;
mod symbols;
mod terminal;
mod theme;
mod workspace;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::OnceLock;

use gpui_kit::component::TitleBar;
use gpui_kit::{
    Action, App, AppContext as _, AssetSource, Bounds, Global, KeyBinding, Menu, MenuItem,
    PathPromptOptions, SharedString, SystemMenuType, WeakEntity, WindowBounds, WindowOptions,
    actions, px, size,
};

use workspace::Workspace;

actions!(
    forge,
    [
        Quit,
        OpenFolder,
        CloneRepository,
        SaveFile,
        SaveAll,
        CloseTab,
        ToggleNavigator,
        ToggleInspector,
        ToggleTerminal,
        TogglePreview,
        ClearRecent,
        NewFile,
        QuickOpenFile,
        FindInProject,
        FormatFile,
        CommitChanges,
    ]
);

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = forge, no_json)]
pub struct OpenRecent {
    pub path: PathBuf,
}

/// Lets app-level actions (menus, shortcuts) reach the single workspace.
struct AppState {
    workspace: WeakEntity<Workspace>,
    window: gpui_kit::AnyWindowHandle,
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

fn with_workspace(
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut gpui_kit::Window, &mut gpui_kit::Context<Workspace>),
) {
    let Some(state) = cx.try_global::<AppState>() else { return };
    let (workspace, window) = (state.workspace.clone(), state.window);
    let _ = window.update(cx, |_, window, cx| {
        if let Some(workspace) = workspace.upgrade() {
            workspace.update(cx, |ws, cx| f(ws, window, cx));
        }
    });
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
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Forge", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New File…", NewFile),
            MenuItem::action("Open…", OpenFolder),
            MenuItem::submenu(Menu::new("Open Recent").items(recent_items)),
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
        Menu::new("View").items([
            MenuItem::action("Toggle Navigator", ToggleNavigator),
            MenuItem::action("Toggle Inspector", ToggleInspector),
            MenuItem::action("Toggle Terminal", ToggleTerminal),
            MenuItem::action("Toggle Canvas", TogglePreview),
            MenuItem::separator(),
            MenuItem::action("Open Quickly…", QuickOpenFile),
            MenuItem::action("Find in Project…", FindInProject),
        ]),
    ]);
}

fn register_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| {
        with_workspace(cx, |ws, _, cx| ws.save_all(cx));
        cx.quit();
    });
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
                    with_workspace(cx, |ws, window, cx| ws.open_paths(&paths, window, cx))
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
                    with_workspace(cx, |ws, window, cx| ws.open_paths(&[path], window, cx))
                });
            }
        })
        .detach();
    });
    cx.on_action(|action: &OpenRecent, cx| {
        let path = action.path.clone();
        with_workspace(cx, |ws, window, cx| ws.open_paths(&[path], window, cx));
    });
    cx.on_action(|_: &ClearRecent, cx| {
        recent::clear();
        set_menus(cx);
    });
    cx.on_action(|_: &CloneRepository, cx| {
        with_workspace(cx, |ws, window, cx| ws.show_clone(window, cx))
    });
    cx.on_action(|_: &SaveFile, cx| with_workspace(cx, |ws, _, cx| ws.save_all(cx)));
    cx.on_action(|_: &SaveAll, cx| with_workspace(cx, |ws, _, cx| ws.save_all(cx)));
    cx.on_action(|_: &CloseTab, cx| {
        with_workspace(cx, |ws, window, cx| ws.close_active(window, cx))
    });
    cx.on_action(|_: &ToggleNavigator, cx| with_workspace(cx, |ws, _, cx| ws.toggle_left(cx)));
    cx.on_action(|_: &ToggleInspector, cx| with_workspace(cx, |ws, _, cx| ws.toggle_right(cx)));
    cx.on_action(|_: &ToggleTerminal, cx| {
        with_workspace(cx, |ws, window, cx| ws.toggle_terminal(window, cx))
    });
    cx.on_action(|_: &TogglePreview, cx| {
        with_workspace(cx, |ws, window, cx| ws.toggle_preview(window, cx))
    });

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

    cx.bind_keys([
        // Global (no context): nothing in the editor's "Input" context binds
        // cmd-i, so this reaches the app handler from inside the editor too.
        KeyBinding::new("cmd-i", FormatFile, None),
        KeyBinding::new("cmd-p", QuickOpenFile, None),
        KeyBinding::new("cmd-shift-o", QuickOpenFile, None),
        KeyBinding::new("cmd-shift-f", FindInProject, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-n", NewFile, None),
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("cmd-shift-c", CloneRepository, None),
        KeyBinding::new("cmd-s", SaveFile, None),
        KeyBinding::new("cmd-alt-s", SaveAll, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-0", ToggleNavigator, None),
        KeyBinding::new("cmd-alt-0", ToggleInspector, None),
        KeyBinding::new("cmd-shift-y", ToggleTerminal, None),
        KeyBinding::new("ctrl-`", ToggleTerminal, None),
        KeyBinding::new("cmd-alt-enter", TogglePreview, None),
    ]);
}

fn main() {
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
        gpui_kit::init(cx);
        languages::register();
        theme::apply(cx);
        register_actions(cx);
        set_menus(cx);
        cx.activate(true);

        let bounds = Bounds::centered(None, size(px(1360.), px(860.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(720.), px(420.))),
            ..TitleBar::window_options()
        };
        let (window, workspace) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .expect("failed to open window");
        cx.set_global(AppState { workspace: workspace.downgrade(), window });

        let mut initial = launch_paths.clone();
        initial.extend(open_requests().1.drain().flatten());
        if initial.is_empty() {
            initial.extend(recent::load().into_iter().next());
        }
        with_workspace(cx, |ws, window, cx| ws.open_paths(&initial, window, cx));

        // Finder requests that arrive while the app is running.
        cx.spawn(async move |cx| {
            while let Ok(paths) = open_requests().1.recv_async().await {
                let _ = cx.update(|cx| {
                    with_workspace(cx, |ws, window, cx| ws.open_paths(&paths, window, cx));
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
