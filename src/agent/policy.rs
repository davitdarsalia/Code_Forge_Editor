use std::path::{Component, Path, PathBuf};

use super::protocol::{PermissionKind, PermissionOption};

const REDACTED: &str = "[redacted]";
const SECRET_PREFIXES: &[&str] = &["sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "AIza", "glpat-"];
const SECRET_NAMES: &[&str] = &["KEY", "TOKEN", "SECRET", "PASSWORD", "AUTH"];

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn resolve_existing(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&existing) {
            let mut full = real;
            for part in rest.iter().rev() {
                full.push(part);
            }
            return full;
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

pub fn resolve_path(root: &Path, requested: &Path, full_access: bool) -> Result<PathBuf, String> {
    let joined = if requested.is_absolute() { requested.to_path_buf() } else { root.join(requested) };
    let resolved = resolve_existing(&normalize(&joined));
    let root = resolve_existing(&normalize(root));
    if full_access || resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(format!("{} is outside the project; turn on Full access to allow it", requested.display()))
    }
}

pub fn auto_pick(options: &[PermissionOption]) -> Option<String> {
    [PermissionKind::AllowOnce, PermissionKind::AllowAlways].iter().find_map(|kind| {
        options.iter().find(|option| option.kind == *kind).map(|option| option.option_id.clone())
    })
}

pub fn read_text(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    if meta.len() > crate::document::MAX_FILE_BYTES {
        return Err(format!("{} is too large to read", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    if crate::files::looks_binary(&bytes) {
        return Err(format!("{} is a binary file", path.display()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn slice_lines(text: &str, line: Option<u32>, limit: Option<u32>) -> String {
    if line.is_none() && limit.is_none() {
        return text.to_string();
    }
    let skip = line.map(|l| l.saturating_sub(1) as usize).unwrap_or(0);
    let take = limit.map(|l| l as usize).unwrap_or(usize::MAX);
    text.split_inclusive('\n').skip(skip).take(take).collect()
}

fn looks_secret(word: &str) -> bool {
    let trimmed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
    if let Some((name, value)) = trimmed.split_once('=') {
        let upper = name.to_ascii_uppercase();
        if !value.is_empty() && SECRET_NAMES.iter().any(|n| upper.contains(n)) {
            return true;
        }
    }
    SECRET_PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix) && trimmed.len() >= prefix.len() + 12)
}

pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if looks_secret(word) {
            match word.split_once('=') {
                Some((name, _)) if !name.is_empty() => {
                    out.push_str(name);
                    out.push('=');
                    out.push_str(REDACTED);
                }
                _ => out.push_str(REDACTED),
            }
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for ch in text.chars() {
        if ch.is_whitespace() || ch == '"' || ch == '\'' || ch == ',' {
            flush(&mut word, &mut out);
            out.push(ch);
        } else {
            word.push(ch);
        }
    }
    flush(&mut word, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-agent-policy-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn paths_inside_the_project_resolve() {
        let root = temp_root("inside");
        std::fs::write(root.join("src/a.rs"), "").unwrap();
        assert_eq!(resolve_path(&root, &root.join("src/a.rs"), false), Ok(root.join("src/a.rs")));
        assert_eq!(resolve_path(&root, Path::new("src/new/b.rs"), false), Ok(root.join("src/new/b.rs")));
        assert_eq!(resolve_path(&root, &root.join("src/../src/./a.rs"), false), Ok(root.join("src/a.rs")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn escapes_are_rejected_unless_full_access() {
        let root = temp_root("escape");
        let outside = root.join("../elsewhere.txt");
        assert!(resolve_path(&root, &outside, false).is_err());
        assert!(resolve_path(&root, Path::new("/etc/hosts"), false).is_err());
        assert!(resolve_path(&root, Path::new("../../x"), false).is_err());
        assert!(resolve_path(&root, Path::new("/etc/hosts"), true).is_ok());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn symlinks_out_of_the_project_are_rejected() {
        let root = temp_root("link");
        let target = temp_root("link-target");
        std::os::unix::fs::symlink(&target, root.join("out")).unwrap();
        assert!(resolve_path(&root, &root.join("out/secret.txt"), false).is_err());
        assert_eq!(resolve_path(&root, &root.join("out/secret.txt"), true), Ok(target.join("secret.txt")));
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&target).unwrap();
    }

    #[test]
    fn auto_pick_prefers_allow_once_and_never_rejects() {
        let option = |id: &str, kind| PermissionOption { option_id: id.into(), name: id.into(), kind };
        let options = vec![
            option("reject", PermissionKind::RejectOnce),
            option("always", PermissionKind::AllowAlways),
            option("once", PermissionKind::AllowOnce),
        ];
        assert_eq!(auto_pick(&options), Some("once".to_string()));
        assert_eq!(auto_pick(&options[..2]), Some("always".to_string()));
        assert_eq!(auto_pick(&options[..1]), None);
        assert_eq!(auto_pick(&[]), None);
    }

    #[test]
    fn slice_lines_is_one_based_with_limit() {
        let text = "a\nb\nc\nd";
        assert_eq!(slice_lines(text, None, None), text);
        assert_eq!(slice_lines(text, Some(2), Some(2)), "b\nc\n");
        assert_eq!(slice_lines(text, Some(3), None), "c\nd");
        assert_eq!(slice_lines(text, None, Some(1)), "a\n");
        assert_eq!(slice_lines(text, Some(9), None), "");
    }

    #[test]
    fn redact_hides_keys_and_tokens() {
        let text = "failed: ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnop and token ghp_0123456789abcdefXYZ, ok";
        let clean = redact(text);
        assert!(!clean.contains("sk-ant"));
        assert!(!clean.contains("ghp_0123"));
        assert!(clean.contains("ANTHROPIC_API_KEY=[redacted]"));
        assert!(clean.ends_with(", ok"));
        assert_eq!(redact("plain words stay"), "plain words stay");
        assert_eq!(redact("sk-short"), "sk-short");
    }
}
