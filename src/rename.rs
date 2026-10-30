use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState, Position};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use lsp_types::{DocumentChangeOperation, DocumentChanges, OneOf, TextEdit, WorkspaceEdit};

use crate::lsp;
use crate::theme::{Palette, ca};
use crate::ui::{icon, text_button};

pub struct FileEdits {
    pub path: PathBuf,
    pub edits: Vec<TextEdit>,
}

#[derive(Default)]
pub struct EditPlan {
    pub files: Vec<FileEdits>,
    pub skipped_operations: usize,
}

pub fn plan(edit: WorkspaceEdit) -> EditPlan {
    let mut plan = EditPlan::default();
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    let mut add = |plan: &mut EditPlan, uri: &str, edits: Vec<TextEdit>| {
        let Some(path) = lsp::uri_to_path(uri) else {
            plan.skipped_operations += 1;
            return;
        };
        let slot = *index.entry(path.clone()).or_insert_with(|| {
            plan.files.push(FileEdits { path, edits: Vec::new() });
            plan.files.len() - 1
        });
        plan.files[slot].edits.extend(edits);
    };
    let text_edits = |edits: Vec<OneOf<TextEdit, lsp_types::AnnotatedTextEdit>>| -> Vec<TextEdit> {
        edits
            .into_iter()
            .map(|e| match e {
                OneOf::Left(edit) => edit,
                OneOf::Right(annotated) => annotated.text_edit,
            })
            .collect()
    };
    match edit.document_changes {
        Some(DocumentChanges::Edits(documents)) => {
            for document in documents {
                add(&mut plan, document.text_document.uri.as_str(), text_edits(document.edits));
            }
        }
        Some(DocumentChanges::Operations(operations)) => {
            for operation in operations {
                match operation {
                    DocumentChangeOperation::Edit(document) => {
                        add(&mut plan, document.text_document.uri.as_str(), text_edits(document.edits));
                    }
                    DocumentChangeOperation::Op(_) => plan.skipped_operations += 1,
                }
            }
        }
        None => {
            for (uri, edits) in edit.changes.unwrap_or_default() {
                add(&mut plan, uri.as_str(), edits);
            }
        }
    }
    plan.files.retain(|f| !f.edits.is_empty());
    plan
}

fn byte_offset(text: &str, line_starts: &[usize], position: lsp_types::Position) -> usize {
    let Some(&start) = line_starts.get(position.line as usize) else { return text.len() };
    let end = line_starts.get(position.line as usize + 1).copied().unwrap_or(text.len());
    let line = text[start..end].trim_end_matches('\n').trim_end_matches('\r');
    let chars = lsp::char_column(line, position.character) as usize;
    start + line.char_indices().nth(chars).map_or(line.len(), |(i, _)| i)
}

pub struct Applied {
    pub text: String,
    pub cursor: Position,
}

pub fn apply(text: &str, edits: &[TextEdit], cursor_byte: usize) -> Result<Applied, String> {
    let mut line_starts = vec![0];
    line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let mut resolved: Vec<(usize, usize, usize, &str)> = edits
        .iter()
        .enumerate()
        .map(|(n, e)| {
            let start = byte_offset(text, &line_starts, e.range.start);
            let end = byte_offset(text, &line_starts, e.range.end).max(start);
            (start, end, n, e.new_text.as_str())
        })
        .collect();
    resolved.sort_by_key(|&(start, end, n, _)| (start, end, n));
    for pair in resolved.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err("The language server returned overlapping edits".into());
        }
    }
    let mut cursor = cursor_byte.min(text.len());
    let mut out = text.to_string();
    for &(start, end, _, replacement) in resolved.iter().rev() {
        out.replace_range(start..end, replacement);
        if start < cursor && cursor < end {
            cursor = start;
        } else if end <= cursor && !(start == end && start == cursor) {
            cursor = cursor + replacement.len() - (end - start);
        }
    }
    let cursor = cursor.min(out.len());
    let before = &out[..cursor];
    let line = before.matches('\n').count() as u32;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) as u32;
    Ok(Applied { text: out, cursor: Position::new(line, column) })
}

pub enum RenameEvent {
    Dismissed,
    Submit(String),
}

pub struct RenameDialog {
    input: Entity<InputState>,
    open: bool,
    original: String,
    _subscription: Subscription,
}

impl EventEmitter<RenameEvent> for RenameDialog {}

impl RenameDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("New name"));
        let subscription = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(cx);
            }
        });
        RenameDialog { input, open: false, original: String::new(), _subscription: subscription }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn show(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.original = name.to_string();
        self.input.update(cx, |input, cx| {
            input.set_value(name.to_string(), window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        cx.emit(RenameEvent::Dismissed);
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let name = self.input.read(cx).value().trim().to_string();
        if name.is_empty() || name == self.original {
            self.close(cx);
            return;
        }
        self.open = false;
        cx.emit(RenameEvent::Submit(name));
        cx.notify();
    }
}

impl Render for RenameDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = format!("Rename “{}”", self.original).into();
        div()
            .id("rename-overlay")
            .absolute()
            .inset_0()
            .bg(ca(0x000000, 0.35))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(120.))
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                this.close(cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .id("rename-dialog")
                    .w(px(420.))
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
                            .child(icon(IconName::Replace, 18., Palette::accent()))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(Palette::text())
                                    .child(title),
                            ),
                    )
                    .child(Input::new(&self.input))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(Palette::faint())
                            .child("Renames every reference across the project."),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(text_button("rename-cancel", "Cancel", false, cx.listener(|this, _, _, cx| this.close(cx))))
                            .child(text_button("rename-go", "Rename", true, cx.listener(|this, _, _, cx| this.submit(cx)))),
                    ),
            )
    }
}

pub fn within(root: Option<&Path>, path: &Path) -> bool {
    root.is_some_and(|root| path.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Range;
    use serde_json::json;

    fn edit(sl: u32, sc: u32, el: u32, ec: u32, text: &str) -> TextEdit {
        TextEdit { range: Range::new(lsp_types::Position::new(sl, sc), lsp_types::Position::new(el, ec)), new_text: text.into() }
    }

    fn workspace_edit(value: serde_json::Value) -> WorkspaceEdit {
        serde_json::from_value(value).unwrap()
    }

    fn range(l: u32, a: u32, b: u32) -> serde_json::Value {
        json!({ "start": { "line": l, "character": a }, "end": { "line": l, "character": b } })
    }

    #[test]
    fn plan_reads_changes_map() {
        let plan = plan(workspace_edit(json!({ "changes": {
            "file:///p/a.rs": [{ "range": range(0, 3, 6), "newText": "bar" }, { "range": range(2, 0, 3), "newText": "bar" }],
            "file:///p/b.rs": [{ "range": range(1, 0, 3), "newText": "bar" }],
        } })));
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.files.iter().map(|f| f.edits.len()).sum::<usize>(), 3);
        let a = plan.files.iter().find(|f| f.path == Path::new("/p/a.rs")).unwrap();
        assert_eq!(a.edits.len(), 2);
    }

    #[test]
    fn plan_reads_document_changes_and_skips_resource_ops() {
        let plan = plan(workspace_edit(json!({ "documentChanges": [
            { "textDocument": { "uri": "file:///p/a.swift", "version": 3 }, "edits": [{ "range": range(0, 0, 3), "newText": "Bar" }] },
            { "kind": "rename", "oldUri": "file:///p/Foo.swift", "newUri": "file:///p/Bar.swift" },
            { "textDocument": { "uri": "file:///p/a.swift", "version": null }, "edits": [{ "range": range(4, 0, 3), "newText": "Bar" }] },
            { "textDocument": { "uri": "file:///p/My%20File.swift", "version": null }, "edits": [
                { "range": range(1, 0, 3), "newText": "Bar", "annotationId": "x" }] },
        ] })));
        assert_eq!(plan.skipped_operations, 1);
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.files[0].path, Path::new("/p/a.swift"));
        assert_eq!(plan.files[0].edits.len(), 2);
        assert_eq!(plan.files[1].path, Path::new("/p/My File.swift"));
    }

    #[test]
    fn plan_prefers_document_changes_over_changes() {
        let plan = plan(workspace_edit(json!({
            "changes": { "file:///p/x.rs": [{ "range": range(0, 0, 1), "newText": "z" }] },
            "documentChanges": [{ "textDocument": { "uri": "file:///p/a.rs", "version": null },
                "edits": [{ "range": range(0, 0, 1), "newText": "y" }] }],
        })));
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, Path::new("/p/a.rs"));
    }

    #[test]
    fn edits_apply_in_reverse_order_regardless_of_input_order() {
        let text = "foo(foo)\nlet x = foo\n";
        let edits = [edit(1, 8, 1, 11, "barbaz"), edit(0, 0, 0, 3, "barbaz"), edit(0, 4, 0, 7, "barbaz")];
        let applied = apply(text, &edits, 0).unwrap();
        assert_eq!(applied.text, "barbaz(barbaz)\nlet x = barbaz\n");
    }

    #[test]
    fn utf16_columns_map_to_chars() {
        let text = "let 😀 = foo\n";
        let applied = apply(text, &[edit(0, 9, 0, 12, "bar")], 0).unwrap();
        assert_eq!(applied.text, "let 😀 = bar\n");
        let accents = "é = foo";
        assert_eq!(apply(accents, &[edit(0, 4, 0, 7, "x")], 0).unwrap().text, "é = x");
    }

    #[test]
    fn edits_past_line_end_clamp() {
        let applied = apply("ab\ncd", &[edit(0, 1, 0, 99, "X"), edit(5, 0, 5, 0, "!")], 0).unwrap();
        assert_eq!(applied.text, "aX\ncd!");
    }

    #[test]
    fn crlf_lines_keep_their_terminators() {
        let applied = apply("foo\r\nfoo\r\n", &[edit(0, 0, 0, 3, "bar"), edit(1, 0, 1, 3, "bar")], 0).unwrap();
        assert_eq!(applied.text, "bar\r\nbar\r\n");
    }

    #[test]
    fn overlapping_edits_are_rejected() {
        assert!(apply("abcdef", &[edit(0, 0, 0, 4, "x"), edit(0, 2, 0, 5, "y")], 0).is_err());
    }

    #[test]
    fn cursor_follows_edits() {
        let text = "foo foo\nfoo";
        let edits = [edit(0, 0, 0, 3, "barbaz"), edit(0, 4, 0, 7, "barbaz"), edit(1, 0, 1, 3, "barbaz")];
        let applied = apply(text, &edits, 5).unwrap();
        assert_eq!(applied.text, "barbaz barbaz\nbarbaz");
        assert_eq!(applied.cursor, Position::new(0, 7));
        let inside = apply(text, &edits, 9).unwrap();
        assert_eq!(inside.cursor, Position::new(1, 0));
    }

    #[test]
    fn project_membership() {
        assert!(within(Some(Path::new("/p")), Path::new("/p/a.rs")));
        assert!(!within(Some(Path::new("/p")), Path::new("/q/a.rs")));
        assert!(!within(None, Path::new("/p/a.rs")));
    }
}
