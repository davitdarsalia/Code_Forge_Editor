use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::search::FileIndex;

const HEADER: &str = "forge-index 1";

pub fn cache_path(root: &Path) -> Option<PathBuf> {
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "root".into());
    let file = format!("{name}-{:016x}.txt", hasher.finish());
    dirs::cache_dir().map(|dir| dir.join("Forge").join("index").join(file))
}

pub fn load(root: &Path) -> Option<FileIndex> {
    load_from(&cache_path(root)?, root)
}

fn load_from(path: &Path, root: &Path) -> Option<FileIndex> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()? != HEADER || Path::new(lines.next()?) != root {
        return None;
    }
    let files = lines.filter(|l| !l.is_empty()).map(str::to_owned).collect();
    Some(FileIndex { root: root.to_path_buf(), files })
}

pub fn save(index: &FileIndex) -> anyhow::Result<PathBuf> {
    let path = cache_path(&index.root).ok_or_else(|| anyhow::anyhow!("no cache directory"))?;
    save_to(&path, index)?;
    Ok(path)
}

fn save_to(path: &Path, index: &FileIndex) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = String::with_capacity(index.files.iter().map(|f| f.len() + 1).sum::<usize>() + 256);
    text.push_str(HEADER);
    text.push('\n');
    text.push_str(&index.root.to_string_lossy());
    text.push('\n');
    for file in &index.files {
        text.push_str(file);
        text.push('\n');
    }
    crate::files::write_atomic(path, text.as_bytes())?;
    Ok(())
}

pub fn diff(old: &[String], new: &[String]) -> (usize, usize) {
    let (mut added, mut removed) = (0, 0);
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        match (old.get(i), new.get(j)) {
            (Some(a), Some(b)) if a == b => (i, j) = (i + 1, j + 1),
            (Some(a), Some(b)) if a < b => (removed, i) = (removed + 1, i + 1),
            (Some(_), Some(_)) | (None, Some(_)) => (added, j) = (added + 1, j + 1),
            (Some(_), None) => (removed, i) = (removed + 1, i + 1),
            (None, None) => break,
        }
    }
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("forge-index-cache-{}-{name}", std::process::id()))
    }

    fn index(root: &str, files: &[&str]) -> FileIndex {
        FileIndex { root: PathBuf::from(root), files: files.iter().map(|f| f.to_string()).collect() }
    }

    #[test]
    fn round_trips() {
        let path = temp_file("round");
        let original = index("/tmp/project", &["a.rs", "src/b.swift"]);
        save_to(&path, &original).unwrap();
        let loaded = load_from(&path, Path::new("/tmp/project")).unwrap();
        assert_eq!(loaded.files, original.files);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_other_root_and_bad_header() {
        let path = temp_file("reject");
        save_to(&path, &index("/tmp/one", &["a"])).unwrap();
        assert!(load_from(&path, Path::new("/tmp/two")).is_none());
        std::fs::write(&path, "something else\n/tmp/one\na\n").unwrap();
        assert!(load_from(&path, Path::new("/tmp/one")).is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn diff_counts_added_and_removed() {
        let old: Vec<String> = ["a", "b", "d"].iter().map(|s| s.to_string()).collect();
        let new: Vec<String> = ["a", "c", "d", "e"].iter().map(|s| s.to_string()).collect();
        assert_eq!(diff(&old, &new), (2, 1));
        assert_eq!(diff(&new, &new), (0, 0));
    }

    #[test]
    fn same_folder_name_different_paths_get_different_files() {
        assert_ne!(cache_path(Path::new("/a/app")), cache_path(Path::new("/b/app")));
    }
}
