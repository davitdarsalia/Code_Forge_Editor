use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 12;

fn store_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("Forge").join("recent.json"))
}

pub fn load() -> Vec<PathBuf> {
    let Some(path) = store_path() else { return Vec::new() };
    let Ok(bytes) = std::fs::read(&path) else { return Vec::new() };
    let list: Vec<PathBuf> = serde_json::from_slice(&bytes).unwrap_or_default();
    list.into_iter().filter(|p| p.exists()).collect()
}

pub fn push(project: &Path) -> Vec<PathBuf> {
    let mut list = load();
    list.retain(|p| p != project);
    list.insert(0, project.to_path_buf());
    list.truncate(MAX_RECENT);
    save(&list);
    list
}

pub fn clear() {
    save(&[]);
}

fn save(list: &[PathBuf]) {
    let Some(path) = store_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_vec_pretty(list) {
        Ok(bytes) => {
            if let Err(err) = std::fs::write(&path, bytes) {
                eprintln!("forge: failed to save recent projects: {err}");
            }
        }
        Err(err) => eprintln!("forge: failed to encode recent projects: {err}"),
    }
}
