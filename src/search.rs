//! Project file index and full-text search, built on ripgrep's crates.
//!
//! Indexing walks the tree in parallel (respecting .gitignore). Search reuses
//! the index and fans the file list out over every core; results stream back
//! in batches so the UI fills in while the search is still running.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use grep_matcher::Matcher as _;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::UTF8};
use ignore::{WalkBuilder, WalkState};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Results beyond this are dropped; nobody reads 20k matches.
pub const MAX_MATCHES: usize = 20_000;
const MAX_LINE_CHARS: usize = 240;
const BATCH: usize = 8;

pub struct FileIndex {
    pub root: PathBuf,
    /// Paths relative to `root`, sorted.
    pub files: Vec<String>,
}

impl FileIndex {
    pub fn absolute(&self, ix: usize) -> PathBuf {
        self.root.join(&self.files[ix])
    }
}

fn walker(root: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .threads(num_cpus::get())
        .filter_entry(|entry| entry.file_name() != ".git");
    builder
}

/// Per-thread buffer that hands its paths over when the walker drops it.
struct Collector {
    local: Vec<String>,
    tx: flume::Sender<Vec<String>>,
}

impl Drop for Collector {
    fn drop(&mut self) {
        if !self.local.is_empty() {
            let _ = self.tx.send(std::mem::take(&mut self.local));
        }
    }
}

/// Walks `root` on all cores and returns every non-ignored file.
pub fn build_index(root: &Path) -> FileIndex {
    let (tx, rx) = flume::unbounded::<Vec<String>>();
    walker(root).build_parallel().run(|| {
        let mut collector = Collector { local: Vec::with_capacity(256), tx: tx.clone() };
        let root = root.to_path_buf();
        Box::new(move |entry| {
            if let Ok(entry) = entry {
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    if let Ok(relative) = entry.path().strip_prefix(&root) {
                        collector.local.push(relative.to_string_lossy().into_owned());
                    }
                }
            }
            WalkState::Continue
        })
    });
    drop(tx);
    let mut files: Vec<String> = rx.drain().flatten().collect();
    files.sort_unstable();
    FileIndex { root: root.to_path_buf(), files }
}

// ---- Quick open (⌘P) --------------------------------------------------------

pub struct FuzzyHit {
    pub ix: usize,
    /// Char indices in the relative path that matched, for highlighting.
    pub indices: Vec<u32>,
}

pub fn fuzzy(index: &FileIndex, query: &str, limit: usize) -> Vec<FuzzyHit> {
    if query.trim().is_empty() {
        return (0..index.files.len().min(limit)).map(|ix| FuzzyHit { ix, indices: Vec::new() }).collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = index
        .files
        .iter()
        .enumerate()
        .filter_map(|(ix, path)| {
            let score = pattern.score(Utf32Str::new(path, &mut buf), &mut matcher)?;
            // Prefer shorter paths on ties: the file you mean is usually shallow.
            Some((score, ix))
        })
        .collect();
    scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(index.files[a.1].len().cmp(&index.files[b.1].len())));
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(_, ix)| {
            let mut indices = Vec::new();
            pattern.indices(Utf32Str::new(&index.files[ix], &mut buf), &mut matcher, &mut indices);
            indices.sort_unstable();
            indices.dedup();
            FuzzyHit { ix, indices }
        })
        .collect()
}

/// Fuzzy-ranks arbitrary labels (commands, symbols); `ix` points into `items`.
/// An empty query keeps the original order.
pub fn fuzzy_list<'a>(items: impl IntoIterator<Item = &'a str>, query: &str, limit: usize) -> Vec<FuzzyHit> {
    let items: Vec<&str> = items.into_iter().collect();
    if query.trim().is_empty() {
        return (0..items.len().min(limit)).map(|ix| FuzzyHit { ix, indices: Vec::new() }).collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(ix, item)| Some((pattern.score(Utf32Str::new(item, &mut buf), &mut matcher)?, ix)))
        .collect();
    scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(_, ix)| {
            let mut indices = Vec::new();
            pattern.indices(Utf32Str::new(items[ix], &mut buf), &mut matcher, &mut indices);
            indices.sort_unstable();
            indices.dedup();
            FuzzyHit { ix, indices }
        })
        .collect()
}

// ---- Full-text search -------------------------------------------------------

#[derive(Clone, Copy, Default, PartialEq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
}

#[derive(Clone)]
pub struct LineMatch {
    pub line: u64,
    /// Char column of the first match in the original line.
    pub column: usize,
    pub text: String,
    /// Byte ranges inside `text`.
    pub ranges: Vec<(usize, usize)>,
}

pub struct FileMatches {
    pub path: String,
    pub matches: Vec<LineMatch>,
}

/// Handle to a running search; dropping it does not stop it, `cancel` does.
pub struct SearchHandle {
    cancelled: Arc<AtomicBool>,
}

impl SearchHandle {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

/// Starts a parallel search over `index`. Batches of results arrive on the
/// returned channel; it closes when every worker has finished.
pub fn search(
    index: Arc<FileIndex>,
    query: &str,
    options: SearchOptions,
) -> anyhow::Result<(SearchHandle, flume::Receiver<Vec<FileMatches>>)> {
    let matcher = RegexMatcherBuilder::new()
        .case_smart(false)
        .case_insensitive(!options.case_sensitive)
        .fixed_strings(!options.regex)
        .word(options.whole_word)
        .build(query)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let next = Arc::new(AtomicUsize::new(0));
    let found = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = flume::unbounded();

    for _ in 0..num_cpus::get().max(1) {
        let (index, matcher, cancelled, next, found, tx) =
            (index.clone(), matcher.clone(), cancelled.clone(), next.clone(), found.clone(), tx.clone());
        std::thread::spawn(move || {
            let mut searcher = SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(0))
                .line_number(true)
                .build();
            let mut batch = Vec::new();
            loop {
                if cancelled.load(Ordering::Relaxed) || found.load(Ordering::Relaxed) >= MAX_MATCHES {
                    break;
                }
                let ix = next.fetch_add(1, Ordering::Relaxed);
                let Some(relative) = index.files.get(ix) else { break };
                let path = index.root.join(relative);
                let mut matches = Vec::new();
                let _ = searcher.search_path(
                    &matcher,
                    &path,
                    UTF8(|line_number, line| {
                        let line = line.trim_end_matches(['\n', '\r']);
                        let mut ranges = Vec::new();
                        let _ = matcher.find_iter(line.as_bytes(), |m| {
                            ranges.push((m.start(), m.end()));
                            true
                        });
                        let column = ranges.first().map_or(0, |r| line[..r.0].chars().count());
                        let (text, ranges) = clip_line(line, ranges);
                        matches.push(LineMatch { line: line_number, column, text, ranges });
                        Ok(!cancelled.load(Ordering::Relaxed))
                    }),
                );
                if !matches.is_empty() {
                    found.fetch_add(matches.len(), Ordering::Relaxed);
                    batch.push(FileMatches { path: relative.clone(), matches });
                    if batch.len() >= BATCH {
                        if tx.send(std::mem::take(&mut batch)).is_err() {
                            return;
                        }
                    }
                }
            }
            if !batch.is_empty() {
                let _ = tx.send(batch);
            }
        });
    }
    Ok((SearchHandle { cancelled }, rx))
}

/// Long lines (minified files) are cut around the first match.
fn clip_line(line: &str, ranges: Vec<(usize, usize)>) -> (String, Vec<(usize, usize)>) {
    let trimmed_start = line.len() - line.trim_start().len();
    let first = ranges.first().map_or(0, |r| r.0);
    let mut start = trimmed_start.min(first);
    if line.len() - start > MAX_LINE_CHARS && first > 40 {
        start = first - 40;
    }
    while !line.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + MAX_LINE_CHARS).min(line.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let text = line[start..end].to_string();
    let ranges = ranges
        .into_iter()
        .filter(|r| r.0 >= start && r.0 < end)
        .map(|r| (r.0 - start, r.1.min(end) - start))
        .collect();
    (text, ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    hello_world();\n}\n").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn Hello() {}\n").unwrap();
        std::fs::write(dir.join("ignored/x.rs"), "hello\n").unwrap();
        dir
    }

    #[test]
    fn index_respects_gitignore() {
        let dir = fixture();
        let index = build_index(&dir);
        assert!(index.files.contains(&"src/main.rs".to_string()));
        assert!(!index.files.iter().any(|f| f.starts_with("ignored")));
    }

    #[test]
    fn search_finds_case_insensitive_matches() {
        let dir = fixture();
        let index = Arc::new(build_index(&dir));
        let (_handle, rx) = search(index, "hello", SearchOptions::default()).unwrap();
        let results: Vec<FileMatches> = rx.iter().flatten().collect();
        let total: usize = results.iter().map(|f| f.matches.len()).sum();
        assert_eq!(total, 2);
        let main = results.iter().find(|f| f.path == "src/main.rs").unwrap();
        assert_eq!(main.matches[0].line, 2);
        assert_eq!(main.matches[0].ranges, vec![(0, 5)]);
    }

    #[test]
    fn fuzzy_ranks_file_name() {
        let dir = fixture();
        let index = build_index(&dir);
        let hits = fuzzy(&index, "main", 10);
        assert_eq!(index.files[hits[0].ix], "src/main.rs");
    }

    #[test]
    fn fuzzy_list_filters_labels() {
        let items = ["Toggle Terminal", "Save All", "Toggle Navigator"];
        let hits = fuzzy_list(items, "tnav", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].ix, 2);
        assert_eq!(fuzzy_list(items, "", 2).len(), 2);
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// `FORGE_BENCH_ROOT=/some/repo cargo test --release bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_index_and_search() {
        let Ok(root) = std::env::var("FORGE_BENCH_ROOT") else { return };
        let started = std::time::Instant::now();
        let index = Arc::new(build_index(Path::new(&root)));
        println!("index: {} files in {:?}", index.files.len(), started.elapsed());
        let started = std::time::Instant::now();
        let (_h, rx) = search(index.clone(), "func", SearchOptions::default()).unwrap();
        let total: usize = rx.iter().flatten().map(|f| f.matches.len()).sum();
        println!("search 'func': {total} matches in {:?}", started.elapsed());
        let started = std::time::Instant::now();
        let hits = fuzzy(&index, "remsess", 60);
        println!("fuzzy: {} hits in {:?}", hits.len(), started.elapsed());
    }
}
