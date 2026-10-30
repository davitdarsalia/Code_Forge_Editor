use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui_kit::component::input::Position;

use crate::files;

pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_HISTORY: usize = 100;

#[derive(Clone, PartialEq)]
pub struct NavPoint {
    pub path: PathBuf,
    pub position: Position,
}

pub fn push_history(stack: &mut Vec<NavPoint>, point: NavPoint) {
    if stack.last() != Some(&point) {
        stack.push(point);
        if stack.len() > MAX_HISTORY {
            stack.remove(0);
        }
    }
}

pub fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[derive(Debug, PartialEq)]
pub enum ExternalChange {
    None,
    Touched(Option<SystemTime>),
    Changed,
}

pub fn external_change(path: &Path, known_mtime: Option<SystemTime>, saved_hash: u64) -> ExternalChange {
    let current = mtime(path);
    if current.is_none() || current == known_mtime {
        return ExternalChange::None;
    }
    match std::fs::read_to_string(path) {
        Ok(text) if hash_text(&text) == saved_hash => ExternalChange::Touched(current),
        Ok(_) => ExternalChange::Changed,
        Err(_) => ExternalChange::None,
    }
}

pub struct LoadedText {
    pub text: String,
    pub saved_hash: u64,
    pub language: &'static str,
    pub mtime: Option<SystemTime>,
}

pub fn load_text(path: &Path) -> Result<LoadedText, String> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > MAX_FILE_BYTES {
        return Err(format!("{} is too large to open", path.display()));
    }
    let before = mtime(path);
    let bytes = std::fs::read(path).map_err(|err| format!("Cannot open {}: {err}", path.display()))?;
    if files::looks_binary(&bytes) {
        return Err(format!("{} looks like a binary file", path.display()));
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        format!(
            "{} is not UTF-8 text; not opened to avoid corrupting it",
            path.display()
        )
    })?;
    let saved_hash = hash_text(&text);
    let language = files::detect_language(path, &text);
    Ok(LoadedText {
        text,
        saved_hash,
        language,
        mtime: before,
    })
}

#[derive(Debug, PartialEq)]
pub enum SaveOutcome {
    Unchanged,
    Conflict,
    Superseded,
    Written {
        hash: u64,
        mtime: Option<SystemTime>,
    },
    Failed {
        message: String,
        touched: Option<Option<SystemTime>>,
    },
}

pub fn run_save(
    text: &str,
    path: &Path,
    known_mtime: Option<SystemTime>,
    saved_hash: u64,
    check_external: bool,
    seq: u64,
    gate: &files::WriteGate,
) -> SaveOutcome {
    let hash = hash_text(text);
    if hash == saved_hash {
        return SaveOutcome::Unchanged;
    }
    let mut touched = None;
    if check_external {
        match external_change(path, known_mtime, saved_hash) {
            ExternalChange::None => {}
            ExternalChange::Touched(mtime) => touched = Some(mtime),
            ExternalChange::Changed => return SaveOutcome::Conflict,
        }
    }
    match gate.write(seq, path, text.as_bytes()) {
        Ok(true) => SaveOutcome::Written {
            hash,
            mtime: mtime(path),
        },
        Ok(false) => SaveOutcome::Superseded,
        Err(err) => SaveOutcome::Failed {
            message: format!("Save failed for {}: {err}", path.display()),
            touched,
        },
    }
}

pub struct ReloadProbe {
    pub id: usize,
    pub path: PathBuf,
    pub known_mtime: Option<SystemTime>,
    pub saved_hash: u64,
}

pub struct ReloadChange {
    pub id: usize,
    pub mtime: Option<SystemTime>,
    pub hash: u64,
    pub text: Option<String>,
}

pub fn probe_reloads(probes: Vec<ReloadProbe>) -> Vec<ReloadChange> {
    let mut changes = Vec::new();
    for probe in probes {
        let current = mtime(&probe.path);
        if current.is_none() || current == probe.known_mtime {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&probe.path) else {
            continue;
        };
        let hash = hash_text(&text);
        changes.push(ReloadChange {
            id: probe.id,
            mtime: current,
            hash,
            text: (hash != probe.saved_hash).then_some(text),
        });
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn external_change_detects_rewrites_but_not_touches() {
        let dir = std::env::temp_dir().join(format!("forge-ws-ext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "one").unwrap();
        let loaded = mtime(&path);
        let saved_hash = hash_text("one");
        assert_eq!(external_change(&path, loaded, saved_hash), ExternalChange::None);

        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        assert!(matches!(
            external_change(&path, Some(old), saved_hash),
            ExternalChange::Touched(Some(_))
        ));

        std::fs::write(&path, "two").unwrap();
        assert_eq!(external_change(&path, Some(old), saved_hash), ExternalChange::Changed);

        std::fs::remove_file(&path).unwrap();
        assert_eq!(external_change(&path, loaded, saved_hash), ExternalChange::None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_save_orders_unchanged_conflict_and_superseded() {
        let dir = std::env::temp_dir().join(format!("forge-ws-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.txt");
        std::fs::write(&path, "base").unwrap();
        let gate = files::WriteGate::default();
        let known = mtime(&path);
        let base = hash_text("base");

        assert_eq!(
            run_save("base", &path, known, base, true, 1, &gate),
            SaveOutcome::Unchanged
        );

        let outcome = run_save("edit2", &path, known, base, true, 2, &gate);
        assert!(matches!(outcome, SaveOutcome::Written { hash, .. } if hash == hash_text("edit2")));
        assert_eq!(
            run_save("edit1", &path, known, base, false, 1, &gate),
            SaveOutcome::Superseded
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "edit2");

        let old = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
        assert_eq!(
            run_save("edit3", &path, old, base, true, 3, &gate),
            SaveOutcome::Conflict
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "edit2");
        assert!(matches!(
            run_save("edit3", &path, old, base, false, 3, &gate),
            SaveOutcome::Written { .. }
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "edit3");

        let missing = dir.join("nope/x.txt");
        assert!(matches!(
            run_save("x", &missing, None, base, false, 4, &gate),
            SaveOutcome::Failed { .. }
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn push_history_skips_duplicates_and_caps_length() {
        let point = |line: u32| NavPoint {
            path: PathBuf::from("/p/a.rs"),
            position: Position { line, character: 0 },
        };
        let mut stack = Vec::new();
        push_history(&mut stack, point(1));
        push_history(&mut stack, point(1));
        assert_eq!(stack.len(), 1);
        push_history(&mut stack, point(2));
        push_history(&mut stack, point(1));
        assert_eq!(stack.len(), 3);
        for line in 0..(MAX_HISTORY as u32 + 10) {
            push_history(&mut stack, point(line + 10));
        }
        assert_eq!(stack.len(), MAX_HISTORY);
        assert!(stack.last() == Some(&point(MAX_HISTORY as u32 + 19)));
        assert!(stack.first() == Some(&point(20)));
    }

    #[test]
    fn load_text_rejects_binary_and_non_utf8() {
        let dir = std::env::temp_dir().join(format!("forge-ws-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("a.rs");
        std::fs::write(&ok, "fn main() {}\n").unwrap();
        let loaded = load_text(&ok).unwrap();
        assert_eq!(loaded.saved_hash, hash_text(&loaded.text));
        assert_eq!(loaded.language, "rust");
        let bin = dir.join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2, 0, 255]).unwrap();
        assert!(load_text(&bin).is_err());
        assert!(load_text(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn probe_reloads_reports_rewrites_and_touches_only() {
        let dir = std::env::temp_dir().join(format!("forge-doc-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let same = dir.join("same.txt");
        let touched = dir.join("touched.txt");
        let rewritten = dir.join("rewritten.txt");
        for path in [&same, &touched, &rewritten] {
            std::fs::write(path, "base").unwrap();
        }
        std::fs::write(&rewritten, "changed").unwrap();
        let old = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
        let base = hash_text("base");
        let probes = vec![
            ReloadProbe {
                id: 0,
                path: same.clone(),
                known_mtime: mtime(&same),
                saved_hash: base,
            },
            ReloadProbe {
                id: 1,
                path: touched.clone(),
                known_mtime: old,
                saved_hash: base,
            },
            ReloadProbe {
                id: 2,
                path: rewritten.clone(),
                known_mtime: old,
                saved_hash: base,
            },
            ReloadProbe {
                id: 3,
                path: dir.join("missing"),
                known_mtime: old,
                saved_hash: base,
            },
        ];
        let changes = probe_reloads(probes);
        assert_eq!(changes.iter().map(|c| c.id).collect::<Vec<_>>(), vec![1, 2]);
        assert!(changes[0].text.is_none() && changes[0].hash == base);
        assert_eq!(changes[1].text.as_deref(), Some("changed"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
