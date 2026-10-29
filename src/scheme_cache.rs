//! Disk cache for `xcodebuild -list`.
//!
//! Listing schemes resolves Swift packages and is expensive: on DMote one call
//! takes ~10 s wall, ~21 s CPU and ~200 MB, and it ran for every Xcode
//! container on every project open. A scheme list only changes when the
//! project file, a scheme file or the package manifest changes, so the cache
//! keys each container by a fingerprint of those files' mtimes and re-runs
//! `xcodebuild` only when the fingerprint moves.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::build::{self, XcodeContainer};

#[derive(Default, Serialize, Deserialize)]
struct Store {
    entries: HashMap<PathBuf, Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    fingerprint: u64,
    schemes: Vec<String>,
}

fn store_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("Forge").join("schemes.json"))
}

/// Cached `build::list_schemes`. Background only (stat calls, maybe a process).
pub fn list_schemes(dir: &Path, container: &XcodeContainer) -> anyhow::Result<Vec<String>> {
    let key = container_path(container).to_path_buf();
    let fingerprint = fingerprint(container);
    let path = store_path();
    let cached = path.as_deref().map(load).unwrap_or_default();
    if let Some(entry) = cached.entries.get(&key).filter(|e| e.fingerprint == fingerprint) {
        return Ok(entry.schemes.clone());
    }
    let schemes = build::list_schemes(dir, container)?;
    if let Some(path) = path {
        // Containers are listed in parallel: re-read under a lock so one
        // listing's write does not drop another's.
        static WRITE: Mutex<()> = Mutex::new(());
        let _guard = WRITE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut store = load(&path);
        // Drop entries whose container is gone so the file stays small.
        store.entries.retain(|p, _| p.exists());
        store.entries.insert(key, Entry { fingerprint, schemes: schemes.clone() });
        save(&path, &store);
    }
    Ok(schemes)
}

fn load(path: &Path) -> Store {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            eprintln!("forge: ignoring unreadable scheme cache {}: {err}", path.display());
            Store::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Store::default(),
        Err(err) => {
            eprintln!("forge: cannot read scheme cache {}: {err}", path.display());
            Store::default()
        }
    }
}

fn save(path: &Path, store: &Store) {
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            eprintln!("forge: cannot create {}: {err}", parent.display());
            return;
        }
    }
    let result = serde_json::to_vec(store)
        .map_err(std::io::Error::other)
        .and_then(|bytes| crate::files::write_atomic(path, &bytes));
    if let Err(err) = result {
        eprintln!("forge: cannot write scheme cache {}: {err}", path.display());
    }
}

fn container_path(container: &XcodeContainer) -> &Path {
    match container {
        XcodeContainer::Workspace(p) | XcodeContainer::Project(p) | XcodeContainer::Package(p) => p,
    }
}

/// Hash of (path, mtime) for every file whose change can change the scheme
/// list. Directory mtimes catch scheme files being added, removed or renamed.
pub fn fingerprint(container: &XcodeContainer) -> u64 {
    let mut inputs = Vec::new();
    match container {
        XcodeContainer::Project(project) => project_inputs(project, &mut inputs),
        XcodeContainer::Workspace(workspace) => {
            inputs.push(workspace.join("contents.xcworkspacedata"));
            scheme_dirs(workspace, &mut inputs);
            // Workspaces list the schemes of the projects they wrap, which
            // in practice sit next to them.
            if let Some(parent) = workspace.parent() {
                inputs.push(parent.to_path_buf());
                for project in children(parent).filter(|p| p.extension().is_some_and(|e| e == "xcodeproj")) {
                    project_inputs(&project, &mut inputs);
                }
            }
        }
        XcodeContainer::Package(dir) => {
            inputs.push(dir.join("Package.swift"));
            scheme_dirs(&dir.join(".swiftpm/xcode"), &mut inputs);
        }
    }
    inputs.sort();
    let mut hasher = DefaultHasher::new();
    for path in &inputs {
        path.hash(&mut hasher);
        mtime_nanos(path).hash(&mut hasher);
    }
    hasher.finish()
}

fn project_inputs(project: &Path, inputs: &mut Vec<PathBuf>) {
    inputs.push(project.join("project.pbxproj"));
    scheme_dirs(project, inputs);
}

/// Shared and per-user scheme folders of an .xcodeproj / .xcworkspace.
fn scheme_dirs(container: &Path, inputs: &mut Vec<PathBuf>) {
    inputs.push(container.join("xcshareddata/xcschemes"));
    let user_data = container.join("xcuserdata");
    inputs.push(user_data.clone());
    for user in children(&user_data) {
        inputs.push(user.join("xcschemes"));
    }
}

fn children(dir: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|entry| entry.path())
}

/// `None` for a missing file, so creating one changes the fingerprint too.
fn mtime_nanos(path: &Path) -> Option<u128> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(modified.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-scheme-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fingerprint_moves_when_schemes_or_project_change() {
        let root = temp_dir("project");
        let project = root.join("App.xcodeproj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("project.pbxproj"), "{}").unwrap();
        let container = XcodeContainer::Project(project.clone());

        let base = fingerprint(&container);
        assert_eq!(base, fingerprint(&container), "stable while nothing changes");

        let shared = project.join("xcshareddata/xcschemes");
        std::fs::create_dir_all(&shared).unwrap();
        let with_dir = fingerprint(&container);
        assert_ne!(base, with_dir, "a new scheme folder counts");

        std::fs::create_dir_all(project.join("xcuserdata/me.xcuserdatad/xcschemes")).unwrap();
        assert_ne!(with_dir, fingerprint(&container), "per-user schemes count");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn package_fingerprint_tracks_manifest() {
        let root = temp_dir("package");
        let container = XcodeContainer::Package(root.clone());
        let missing = fingerprint(&container);
        std::fs::write(root.join("Package.swift"), "// swift-tools-version:5.9").unwrap();
        assert_ne!(missing, fingerprint(&container));
        let _ = std::fs::remove_dir_all(&root);
    }
}
