use gpui_kit::Action;

use crate::{
    Build, ClearRecent, CleanBuild, CloneRepository, CloseTab, CloseOtherTabs, CloseTabsToRight, CloseTabsToLeft, CloseSavedTabs, CloseAllTabs, CopyPath, CopyRelativePath, FindInProject, FocusCommit, FocusTerminal,
    FormatFile, GitFetch, GoBack, GoForward, JumpToDefinition, GitPull, GitPush, InstallBuild, Minimize, NewFile, NewTerminal, NewWindow, NextTab,
    OpenFolder, OpenSettings, PreviousTab, QuickOpenFile, RenameSymbol, Quit, RevealInNavigator, SaveAll, SaveFile,
    ShowChangesNavigator, ShowFilesNavigator, ShowSearchNavigator, StopBuild, ToggleInspector, ToggleNavigator,
    ToggleTerminal, Zoom,
};

pub struct Command {
    pub name: &'static str,
    pub keys: &'static str,
    pub action: fn() -> Box<dyn Action>,
}

pub const COMMANDS: &[Command] = &[
    Command { name: "New Window", keys: "⇧⌘N", action: || Box::new(NewWindow) },
    Command { name: "New File…", keys: "⌘N", action: || Box::new(NewFile) },
    Command { name: "Open…", keys: "⌘O", action: || Box::new(OpenFolder) },
    Command { name: "Open Quickly…", keys: "⌘P", action: || Box::new(QuickOpenFile) },
    Command { name: "Clone Git Repository…", keys: "⇧⌘C", action: || Box::new(CloneRepository) },
    Command { name: "Save", keys: "⌘S", action: || Box::new(SaveFile) },
    Command { name: "Save All", keys: "⌥⌘S", action: || Box::new(SaveAll) },
    Command { name: "Close Tab", keys: "⌘W", action: || Box::new(CloseTab) },
    Command { name: "Close Other Tabs", keys: "⌥⌘T", action: || Box::new(CloseOtherTabs) },
    Command { name: "Close Tabs to the Right", keys: "⌘K ⌘→", action: || Box::new(CloseTabsToRight) },
    Command { name: "Close Tabs to the Left", keys: "⌘K ⌘←", action: || Box::new(CloseTabsToLeft) },
    Command { name: "Close Saved Tabs", keys: "⌘K U", action: || Box::new(CloseSavedTabs) },
    Command { name: "Close All Tabs", keys: "⌘K ⌘W", action: || Box::new(CloseAllTabs) },
    Command { name: "Copy Path", keys: "⌥⌘C", action: || Box::new(CopyPath) },
    Command { name: "Copy Relative Path", keys: "⌥⇧⌘C", action: || Box::new(CopyRelativePath) },
    Command { name: "Clear Recent Projects", keys: "", action: || Box::new(ClearRecent) },
    Command { name: "Format File", keys: "⌘I", action: || Box::new(FormatFile) },
    Command { name: "Show Files Navigator", keys: "⌘1", action: || Box::new(ShowFilesNavigator) },
    Command { name: "Show Search Navigator", keys: "⌘2", action: || Box::new(ShowSearchNavigator) },
    Command { name: "Show Changes Navigator", keys: "⌘3", action: || Box::new(ShowChangesNavigator) },
    Command { name: "Toggle Navigator", keys: "⌘B", action: || Box::new(ToggleNavigator) },
    Command { name: "Toggle Inspector", keys: "⌘R", action: || Box::new(ToggleInspector) },
    Command { name: "Toggle Terminal", keys: "⌘J", action: || Box::new(ToggleTerminal) },
    Command { name: "Focus Terminal", keys: "⌃`", action: || Box::new(FocusTerminal) },
    Command { name: "New Terminal", keys: "⌃⇧`", action: || Box::new(NewTerminal) },
    Command { name: "Find in Project…", keys: "⇧⌘F", action: || Box::new(FindInProject) },
    Command { name: "Reveal in Navigator", keys: "⇧⌘J", action: || Box::new(RevealInNavigator) },
    Command { name: "Jump to Definition", keys: "⌃⌘J", action: || Box::new(JumpToDefinition) },
    Command { name: "Rename Symbol…", keys: "F2", action: || Box::new(RenameSymbol) },
    Command { name: "Go Back", keys: "⌃⌘←", action: || Box::new(GoBack) },
    Command { name: "Go Forward", keys: "⌃⌘→", action: || Box::new(GoForward) },
    Command { name: "Show Next Tab", keys: "⇧⌘]", action: || Box::new(NextTab) },
    Command { name: "Show Previous Tab", keys: "⇧⌘[", action: || Box::new(PreviousTab) },
    Command { name: "Commit…", keys: "⌘G", action: || Box::new(FocusCommit) },
    Command { name: "Pull", keys: "⌥⌘X", action: || Box::new(GitPull) },
    Command { name: "Push", keys: "⌥⌘P", action: || Box::new(GitPush) },
    Command { name: "Fetch", keys: "⌥⌘F", action: || Box::new(GitFetch) },
    Command { name: "Build", keys: "⇧⌘B", action: || Box::new(Build) },
    Command { name: "Install (Gradle)", keys: "⇧⌘I", action: || Box::new(InstallBuild) },
    Command { name: "Clean Build Folder", keys: "⇧⌘K", action: || Box::new(CleanBuild) },
    Command { name: "Stop", keys: "⌘.", action: || Box::new(StopBuild) },
    Command { name: "Minimize", keys: "⌘M", action: || Box::new(Minimize) },
    Command { name: "Zoom", keys: "", action: || Box::new(Zoom) },
    Command { name: "Settings…", keys: "⌘,", action: || Box::new(OpenSettings) },
    Command { name: "Quit Forge", keys: "⌘Q", action: || Box::new(Quit) },
];

pub fn names() -> impl Iterator<Item = &'static str> {
    COMMANDS.iter().map(|c| c.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Modifiers;

    fn keystroke_hint(modifiers: &Modifiers, key: &str) -> String {
        let mut out = String::new();
        for (on, glyph) in [
            (modifiers.control, "⌃"),
            (modifiers.alt, "⌥"),
            (modifiers.shift, "⇧"),
            (modifiers.platform, "⌘"),
        ] {
            if on {
                out.push_str(glyph);
            }
        }
        match key {
            "enter" => out.push('↩'),
            "tab" => out.push('⇥'),
            "escape" => out.push('⎋'),
            "left" => out.push('←'),
            "right" => out.push('→'),
            key => out.push_str(&key.to_uppercase()),
        }
        out
    }

    fn first_global_binding_hint(action: &dyn Action) -> Option<String> {
        crate::key_bindings()
            .into_iter()
            .find(|b| b.predicate().is_none() && b.action().partial_eq(action))
            .map(|b| {
                b.keystrokes()
                    .iter()
                    .map(|k| keystroke_hint(k.modifiers(), k.key()))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
    }

    #[test]
    fn hints_match_registered_bindings() {
        for command in COMMANDS {
            let action = (command.action)();
            let expected = first_global_binding_hint(action.as_ref()).unwrap_or_default();
            assert_eq!(command.keys, expected, "shortcut hint for {:?}", command.name);
        }
    }

    #[test]
    fn every_bound_action_is_in_the_palette() {
        for binding in crate::key_bindings() {
            let listed = COMMANDS.iter().any(|c| binding.action().partial_eq((c.action)().as_ref()));
            assert!(listed, "{} has a shortcut but no palette entry", binding.action().name());
        }
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = names().collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
    }
}
