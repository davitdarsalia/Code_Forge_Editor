use std::ops::Range;
use std::time::Duration;

use similar::{ChangeTag, DiffTag, TextDiff};

const TIMEOUT: Duration = Duration::from_millis(60);
const CONTEXT: usize = 2;
pub const MAX_ROWS: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Same,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Line { kind: Kind, number: Option<usize>, text: String },
    Gap(usize),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub rows: Vec<Row>,
    pub added: usize,
    pub removed: usize,
    pub truncated: bool,
}

pub fn summarize(old: &str, new: &str) -> Summary {
    let diff = TextDiff::configure().timeout(TIMEOUT).diff_lines(old, new);
    let mut summary = Summary::default();
    let mut last_old_end: Option<usize> = None;
    for group in diff.grouped_ops(CONTEXT) {
        let start = group.first().map(|op| op.old_range().start).unwrap_or(0);
        match last_old_end {
            Some(end) if start > end => summary.rows.push(Row::Gap(start - end)),
            None if start > 0 => summary.rows.push(Row::Gap(start)),
            _ => {}
        }
        for op in &group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches(['\n', '\r']).to_string();
                let (kind, number) = match change.tag() {
                    ChangeTag::Equal => (Kind::Same, change.new_index()),
                    ChangeTag::Insert => {
                        summary.added += 1;
                        (Kind::Added, change.new_index())
                    }
                    ChangeTag::Delete => {
                        summary.removed += 1;
                        (Kind::Removed, change.old_index())
                    }
                };
                if summary.rows.len() < MAX_ROWS {
                    summary.rows.push(Row::Line { kind, number: number.map(|n| n + 1), text });
                } else {
                    summary.truncated = true;
                }
            }
        }
        last_old_end = group.last().map(|op| op.old_range().end);
    }
    summary
}

pub fn changed_lines(old: &str, new: &str) -> Vec<Range<usize>> {
    let diff = TextDiff::configure().timeout(TIMEOUT).diff_lines(old, new);
    let mut out: Vec<Range<usize>> = Vec::new();
    for op in diff.ops() {
        let range = op.new_range();
        let range = match op.tag() {
            DiffTag::Equal => continue,
            DiffTag::Delete => range.start..range.start + 1,
            DiffTag::Insert | DiffTag::Replace => range,
        };
        match out.last_mut() {
            Some(last) if last.end >= range.start => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_marks_added_and_removed_lines_with_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nd\nE\nf\ng\nh\n";
        let summary = summarize(old, new);
        assert_eq!((summary.added, summary.removed), (1, 1));
        assert_eq!(summary.rows[0], Row::Gap(2));
        let kinds: Vec<_> = summary.rows[1..]
            .iter()
            .map(|row| match row {
                Row::Line { kind, .. } => *kind,
                Row::Gap(_) => panic!("unexpected gap"),
            })
            .collect();
        assert_eq!(kinds, vec![Kind::Same, Kind::Same, Kind::Removed, Kind::Added, Kind::Same, Kind::Same]);
        assert_eq!(summary.rows[4], Row::Line { kind: Kind::Added, number: Some(5), text: "E".into() });
    }

    #[test]
    fn new_files_are_all_additions() {
        let summary = summarize("", "x\ny\n");
        assert_eq!((summary.added, summary.removed), (2, 0));
        assert!(!summary.truncated);
    }

    #[test]
    fn changed_lines_merge_adjacent_hunks() {
        assert_eq!(changed_lines("a\nb\nc\n", "a\nB\nC\n"), vec![1..3]);
        assert_eq!(changed_lines("a\nb\nc\nd\n", "A\nb\nc\nD\n"), vec![0..1, 3..4]);
        assert_eq!(changed_lines("a\nb\nc\n", "a\nc\n"), vec![1..2]);
        assert!(changed_lines("same\n", "same\n").is_empty());
    }
}
