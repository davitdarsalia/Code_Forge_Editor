use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::base::input::{Diagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{
    EditorState, Position, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle, Rope, RopeExt as _,
};
use gpui_kit::{
    App, Bounds, ContentMask, Entity, Hsla, IntoElement, Pixels, SharedString, Styled as _, TextAlign, TextRun,
    Window, canvas, fill, point, px,
};

use crate::lsp;
use crate::theme::{c, ca};

const MAX_MARKS: usize = 400;
const MAX_LABEL_CHARS: usize = 240;
const LABEL_GAP: f32 = 28.;
const LABEL_PAD: f32 = 6.;
const SCROLLBAR_ROOM: f32 = 14.;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub severity: DiagnosticSeverity,
    pub start: Position,
    pub end: Position,
    pub message: SharedString,
    pub source: Option<SharedString>,
}

impl Item {
    pub fn headline(&self) -> SharedString {
        let first = self.message.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim();
        if first.chars().count() > MAX_LABEL_CHARS {
            let cut: String = first.chars().take(MAX_LABEL_CHARS).collect();
            format!("{cut}…").into()
        } else {
            SharedString::from(first.to_owned())
        }
    }
}

pub fn rank(severity: DiagnosticSeverity) -> u8 {
    match severity {
        DiagnosticSeverity::Error => 0,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Info => 2,
        DiagnosticSeverity::Hint => 3,
    }
}

pub fn foreground(severity: DiagnosticSeverity) -> Hsla {
    match severity {
        DiagnosticSeverity::Error => c(0xff6b6b),
        DiagnosticSeverity::Warning => c(0xe5b94e),
        DiagnosticSeverity::Info | DiagnosticSeverity::Hint => c(0x6aa9ff),
    }
}

fn label_text(severity: DiagnosticSeverity) -> Hsla {
    match severity {
        DiagnosticSeverity::Error => c(0xffb4ad),
        DiagnosticSeverity::Warning => c(0xf3d78f),
        DiagnosticSeverity::Info | DiagnosticSeverity::Hint => c(0xb5d3ff),
    }
}

fn label_background(severity: DiagnosticSeverity) -> Hsla {
    match severity {
        DiagnosticSeverity::Error => ca(0xff453a, 0.24),
        DiagnosticSeverity::Warning => ca(0xffcc00, 0.18),
        DiagnosticSeverity::Info | DiagnosticSeverity::Hint => ca(0x0a84ff, 0.18),
    }
}

fn range_tint(severity: DiagnosticSeverity) -> Hsla {
    match severity {
        DiagnosticSeverity::Error => ca(0xff453a, 0.26),
        DiagnosticSeverity::Warning => ca(0xffcc00, 0.2),
        DiagnosticSeverity::Info | DiagnosticSeverity::Hint => ca(0x0a84ff, 0.16),
    }
}

fn line_tint(severity: DiagnosticSeverity) -> Option<Hsla> {
    match severity {
        DiagnosticSeverity::Error => Some(ca(0xff453a, 0.11)),
        _ => None,
    }
}

pub fn convert(raw: Vec<lsp_types::Diagnostic>, text: Option<&Rope>) -> Vec<Item> {
    let column = |pos: lsp_types::Position| match text {
        Some(text) => lsp::from_lsp(text, pos),
        None => pos,
    };
    let mut items: Vec<Item> = raw
        .into_iter()
        .filter_map(|d| {
            let severity = d.severity.map(DiagnosticSeverity::from).unwrap_or(DiagnosticSeverity::Error);
            if severity == DiagnosticSeverity::Hint {
                return None;
            }
            let start = column(d.range.start);
            let end = column(d.range.end);
            let end = if (end.line, end.character) < (start.line, start.character) { start } else { end };
            Some(Item {
                severity,
                start,
                end,
                message: d.message.into(),
                source: d.source.map(SharedString::from),
            })
        })
        .collect();
    items.sort_by(|a, b| {
        (a.start.line, a.start.character, rank(a.severity)).cmp(&(b.start.line, b.start.character, rank(b.severity)))
    });
    items.dedup_by(|a, b| a.start == b.start && a.message == b.message);
    items
}

pub fn byte_range(text: &Rope, start: Position, end: Position) -> Option<Range<usize>> {
    let from = text.position_to_offset(&start);
    let to = text.position_to_offset(&end).max(from);
    if to > from {
        return Some(from..to);
    }
    let row = start.line as usize;
    if row >= text.lines_len() {
        return None;
    }
    let line = lsp::line_text(text, start.line);
    let line_start = text.line_start_offset(row);
    let col = from.saturating_sub(line_start).min(line.len());
    if let Some(next) = line.get(col..).and_then(|rest| rest.chars().next()) {
        return Some(from..from + next.len_utf8());
    }
    let prev = line.get(..col).and_then(|head| head.chars().next_back())?;
    Some(from - prev.len_utf8()..from)
}

struct Mark {
    collection: RangeDecorationCollection,
    severity: DiagnosticSeverity,
    message: SharedString,
}

pub struct Marks {
    entries: Vec<Mark>,
}

impl Marks {
    fn dispose(&self, cx: &mut App) {
        for mark in &self.entries {
            mark.collection.dispose(cx);
        }
    }
}

fn apply_marks(editor: &Entity<EditorState>, items: &[Item], old: Option<Rc<Marks>>, cx: &mut App) -> Rc<Marks> {
    if let Some(old) = old {
        old.dispose(cx);
    }
    let entries = editor.update(cx, |state, cx| {
        let text = state.text().clone();
        if let Some(set) = state.diagnostics_mut() {
            set.reset(&text);
            set.extend(items.iter().take(MAX_MARKS).map(|item| {
                let mut diagnostic = Diagnostic::new(item.start..item.end, item.message.clone()).with_severity(item.severity);
                diagnostic.source = item.source.clone();
                diagnostic
            }));
        }
        let entries: Vec<Mark> = items
            .iter()
            .take(MAX_MARKS)
            .filter_map(|item| {
                let range = byte_range(&text, item.start, item.end)?;
                let decoration = RangeDecoration::new(range)
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(range_tint(item.severity));
                Some(Mark {
                    collection: state.create_range_decorations_collection(vec![decoration], cx),
                    severity: item.severity,
                    message: item.headline(),
                })
            })
            .collect();
        cx.notify();
        entries
    });
    Rc::new(Marks { entries })
}

#[derive(Clone)]
pub enum Row {
    File { path: PathBuf, name: SharedString, dir: SharedString, errors: usize, warnings: usize },
    Issue { path: PathBuf, item: Item },
}

#[derive(Default)]
pub struct Store {
    files: BTreeMap<PathBuf, Vec<Item>>,
    marks: HashMap<PathBuf, Rc<Marks>>,
    rows: RefCell<Option<Rc<Vec<Row>>>>,
    errors: usize,
    warnings: usize,
}

impl Store {
    pub fn publish(
        &mut self,
        path: PathBuf,
        raw: Vec<lsp_types::Diagnostic>,
        editor: Option<&Entity<EditorState>>,
        cx: &mut App,
    ) {
        let items = {
            let text = editor.map(|editor| editor.read(cx).text().clone());
            convert(raw, text.as_ref())
        };
        if let Some(editor) = editor {
            let old = self.marks.remove(&path);
            if !items.is_empty() || old.is_some() {
                self.marks.insert(path.clone(), apply_marks(editor, &items, old, cx));
            }
        }
        self.set_items(path, items);
    }

    fn set_items(&mut self, path: PathBuf, items: Vec<Item>) {
        if items.is_empty() {
            if self.files.remove(&path).is_none() {
                return;
            }
        } else if self.files.get(&path) == Some(&items) {
            return;
        } else {
            self.files.insert(path, items);
        }
        self.recount();
    }

    pub fn attach(&mut self, path: &Path, editor: &Entity<EditorState>, cx: &mut App) {
        let Some(items) = self.files.get(path) else { return };
        let old = self.marks.remove(path);
        let marks = apply_marks(editor, items, old, cx);
        self.marks.insert(path.to_path_buf(), marks);
    }

    pub fn forget(&mut self, path: &Path) {
        self.marks.remove(path);
        if self.files.remove(path).is_some() {
            self.recount();
        }
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.marks.clear();
        self.recount();
    }

    fn recount(&mut self) {
        let all = self.files.values().flatten();
        let (errors, warnings) = all.fold((0, 0), |(e, w), item| match item.severity {
            DiagnosticSeverity::Error => (e + 1, w),
            DiagnosticSeverity::Warning => (e, w + 1),
            _ => (e, w),
        });
        self.errors = errors;
        self.warnings = warnings;
        *self.rows.borrow_mut() = None;
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.errors, self.warnings)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn marks(&self, path: &Path) -> Option<Rc<Marks>> {
        self.marks.get(path).filter(|marks| !marks.entries.is_empty()).cloned()
    }

    pub fn rows(&self, root: Option<&Path>) -> Rc<Vec<Row>> {
        if let Some(rows) = self.rows.borrow().as_ref() {
            return rows.clone();
        }
        let rows = Rc::new(build_rows(&self.files, root));
        *self.rows.borrow_mut() = Some(rows.clone());
        rows
    }
}

fn build_rows(files: &BTreeMap<PathBuf, Vec<Item>>, root: Option<&Path>) -> Vec<Row> {
    let mut ordered: Vec<(&PathBuf, &Vec<Item>)> = files.iter().collect();
    ordered.sort_by_key(|(path, items)| (!items.iter().any(|i| i.severity == DiagnosticSeverity::Error), *path));
    let mut rows = Vec::new();
    for (path, items) in ordered {
        let count = |severity| items.iter().filter(|i| i.severity == severity).count();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent = path.parent().unwrap_or(Path::new(""));
        let dir = root.and_then(|root| parent.strip_prefix(root).ok()).unwrap_or(parent);
        rows.push(Row::File {
            path: path.clone(),
            name: name.into(),
            dir: dir.to_string_lossy().into_owned().into(),
            errors: count(DiagnosticSeverity::Error),
            warnings: count(DiagnosticSeverity::Warning),
        });
        rows.extend(items.iter().map(|item| Row::Issue { path: path.clone(), item: item.clone() }));
    }
    rows
}

struct LineMark {
    severity: DiagnosticSeverity,
    message: SharedString,
    extra: usize,
}

struct Painted {
    top: Pixels,
    bottom: Pixels,
    end_x: Pixels,
    line_height: Pixels,
    mark: LineMark,
}

fn collect(editor: &Entity<EditorState>, marks: &Marks, cx: &App) -> Vec<Painted> {
    let state = editor.read(cx);
    let Some(line_height) = state.line_height() else { return Vec::new() };
    let Some(visible) = state.visible_row_range() else { return Vec::new() };
    let text = state.text();
    let mut lines: BTreeMap<u32, LineMark> = BTreeMap::new();
    for mark in &marks.entries {
        let Some(range) = mark.collection.get_ranges(cx).into_iter().next() else { continue };
        let row = text.offset_to_position(range.start).line;
        if (row as usize) + 1 < visible.start {
            continue;
        }
        let Some(entry) = lines.get_mut(&row) else {
            lines.insert(row, LineMark { severity: mark.severity, message: mark.message.clone(), extra: 0 });
            continue;
        };
        entry.extra += 1;
        if rank(mark.severity) < rank(entry.severity) {
            entry.severity = mark.severity;
            entry.message = mark.message.clone();
        }
    }
    let mut painted = Vec::new();
    for (row, mark) in lines {
        let row = row as usize;
        if row >= text.lines_len() {
            continue;
        }
        let start = text.line_start_offset(row);
        let end = text.line_end_offset(row);
        let Some(tail) = state.range_to_bounds(&(end..end)) else { continue };
        let top = state.range_to_bounds(&(start..start)).map_or(tail.top(), |head| head.top());
        painted.push(Painted { top, bottom: tail.bottom(), end_x: tail.right(), line_height, mark });
    }
    painted
}

fn paint(editor: &Entity<EditorState>, marks: &Marks, font_size: Pixels, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let painted = collect(editor, marks, cx);
    if painted.is_empty() {
        return;
    }
    let font = window.text_style().font();
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for line in painted {
            if let Some(tint) = line_tint(line.mark.severity) {
                let area = Bounds::from_corners(point(bounds.left(), line.top), point(bounds.right(), line.bottom));
                window.paint_quad(fill(area, tint));
            }
            let label: SharedString = match line.mark.extra {
                0 => line.mark.message.clone(),
                extra => format!("{}  +{extra}", line.mark.message).into(),
            };
            if label.is_empty() {
                continue;
            }
            let run = TextRun {
                len: label.len(),
                font: font.clone(),
                color: label_text(line.mark.severity),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(label, font_size, &[run], None);
            let width = shaped.width() + px(LABEL_PAD * 2.);
            let right_aligned = bounds.right() - width - px(SCROLLBAR_ROOM);
            let x = (line.end_x + px(LABEL_GAP)).max(right_aligned);
            let y = line.bottom - line.line_height;
            let banner = Bounds::from_corners(point(x, y + px(1.)), point(x + width, line.bottom - px(1.)));
            window.paint_quad(fill(banner, label_background(line.mark.severity)).corner_radii(px(4.)));
            let _ = shaped.paint(point(x + px(LABEL_PAD), y), line.line_height, TextAlign::Left, None, window, cx);
        }
    });
}

pub fn overlay(editor: Entity<EditorState>, marks: Rc<Marks>, font_size: Pixels) -> impl IntoElement {
    canvas(|_, _, _| {}, move |bounds, _, window, cx| paint(&editor, &marks, font_size, bounds, window, cx))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(line: u32, from: u32, to: u32, severity: lsp_types::DiagnosticSeverity, message: &str) -> lsp_types::Diagnostic {
        lsp_types::Diagnostic {
            range: lsp_types::Range::new(Position::new(line, from), Position::new(line, to)),
            severity: Some(severity),
            message: message.into(),
            ..Default::default()
        }
    }

    #[test]
    fn converts_utf16_columns_and_sorts() {
        let text = Rope::from("let 😀 = 1\nlet y: Int = \"s\"\n");
        let items = convert(
            vec![
                raw(1, 13, 16, lsp_types::DiagnosticSeverity::ERROR, "cannot convert\nnote: more"),
                raw(0, 7, 8, lsp_types::DiagnosticSeverity::WARNING, "unused"),
                raw(0, 0, 1, lsp_types::DiagnosticSeverity::HINT, "hint"),
            ],
            Some(&text),
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].severity, DiagnosticSeverity::Warning);
        assert_eq!(items[0].start, Position::new(0, 6));
        assert_eq!(items[1].headline().as_ref(), "cannot convert");
    }

    #[test]
    fn empty_ranges_expand_to_a_character() {
        let text = Rope::from("abc\n\nxyz");
        assert_eq!(byte_range(&text, Position::new(0, 1), Position::new(0, 1)), Some(1..2));
        assert_eq!(byte_range(&text, Position::new(0, 3), Position::new(0, 3)), Some(2..3));
        assert_eq!(byte_range(&text, Position::new(1, 0), Position::new(1, 0)), None);
        assert_eq!(byte_range(&text, Position::new(2, 0), Position::new(2, 2)), Some(5..7));
        assert_eq!(byte_range(&text, Position::new(9, 0), Position::new(9, 0)), None);
    }

    #[test]
    fn store_counts_and_groups_rows() {
        let mut store = Store::default();
        let error = vec![Item {
            severity: DiagnosticSeverity::Error,
            start: Position::new(3, 1),
            end: Position::new(3, 4),
            message: "boom".into(),
            source: None,
        }];
        let warning = vec![Item { severity: DiagnosticSeverity::Warning, message: "meh".into(), ..error[0].clone() }];
        store.set_items(PathBuf::from("/p/a.rs"), warning.clone());
        store.set_items(PathBuf::from("/p/src/b.rs"), error.clone());
        assert_eq!(store.counts(), (1, 1));
        let rows = store.rows(Some(Path::new("/p")));
        assert_eq!(rows.len(), 4);
        match &rows[0] {
            Row::File { name, dir, errors, .. } => {
                assert_eq!(name.as_ref(), "b.rs");
                assert_eq!(dir.as_ref(), "src");
                assert_eq!(*errors, 1);
            }
            Row::Issue { .. } => panic!("expected a file row"),
        }
        store.set_items(PathBuf::from("/p/src/b.rs"), Vec::new());
        assert_eq!(store.counts(), (0, 1));
        assert_eq!(store.rows(None).len(), 2);
        store.forget(Path::new("/p/a.rs"));
        assert!(store.is_empty());
    }
}
