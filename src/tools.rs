use std::path::PathBuf;
use std::process::Command;

const EXTRA_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];
const XCRUN: &str = "/usr/bin/xcrun";

pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(EXTRA_BIN_DIRS.iter().map(PathBuf::from));
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join(".local/bin"));
    }
    dirs
}

pub fn which(name: &str) -> Option<PathBuf> {
    search_dirs().into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

pub fn xcrun_find(binary: &str) -> Option<PathBuf> {
    let out = Command::new(XCRUN).args(["--find", binary]).output().ok()?;
    let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    (out.status.success() && path.is_file()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_system_tools_and_search_dirs_cover_brew_and_cargo() {
        assert!(which("sh").is_some_and(|p| p.ends_with("sh") && p.is_file()));
        assert!(which("forge-definitely-not-a-tool").is_none());
        let dirs = search_dirs();
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(dirs.contains(&PathBuf::from("/usr/bin")));
        if let Some(home) = dirs::home_dir() {
            assert!(dirs.contains(&home.join(".cargo/bin")));
        }
    }

    #[test]
    fn xcrun_find_rejects_unknown_tools() {
        assert!(xcrun_find("forge-definitely-not-a-tool").is_none());
    }
}
