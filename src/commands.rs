//! Registry of app commands for the ⌘P `>` mode. Each entry builds the action
//! that the menus and shortcuts already dispatch, so the palette adds no
//! behaviour of its own.

use gpui_kit::Action;

use crate::{
    ClearRecent, CloneRepository, CloseTab, FindInProject, FocusCommit, NewFile, OpenFolder,
    Quit, SaveAll, SaveFile, ToggleInspector, ToggleNavigator, TogglePreview, ToggleTerminal,
};

pub struct Command {
    pub name: &'static str,
    /// Shortcut hint shown on the right; keep in sync with `register_actions`.
    pub keys: &'static str,
    pub action: fn() -> Box<dyn Action>,
}

pub const COMMANDS: &[Command] = &[
    Command { name: "Open…", keys: "⌘O", action: || Box::new(OpenFolder) },
    Command { name: "New File…", keys: "⌘N", action: || Box::new(NewFile) },
    Command { name: "Clone Git Repository…", keys: "⇧⌘C", action: || Box::new(CloneRepository) },
    Command { name: "Save", keys: "⌘S", action: || Box::new(SaveFile) },
    Command { name: "Save All", keys: "⌥⌘S", action: || Box::new(SaveAll) },
    Command { name: "Close Tab", keys: "⌘W", action: || Box::new(CloseTab) },
    Command { name: "Find in Project…", keys: "⇧⌘F", action: || Box::new(FindInProject) },
    Command { name: "Commit Changes…", keys: "⌥⌘C", action: || Box::new(FocusCommit) },
    Command { name: "Toggle Navigator", keys: "⌘0", action: || Box::new(ToggleNavigator) },
    Command { name: "Toggle Inspector", keys: "⌥⌘0", action: || Box::new(ToggleInspector) },
    Command { name: "Toggle Terminal", keys: "⇧⌘Y", action: || Box::new(ToggleTerminal) },
    Command { name: "Toggle Canvas", keys: "⌥⌘↩", action: || Box::new(TogglePreview) },
    Command { name: "Clear Recent Projects", keys: "", action: || Box::new(ClearRecent) },
    Command { name: "Quit Forge", keys: "⌘Q", action: || Box::new(Quit) },
];

pub fn names() -> impl Iterator<Item = &'static str> {
    COMMANDS.iter().map(|c| c.name)
}
