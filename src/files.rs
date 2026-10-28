//! Lazily loaded project file tree, flattened into rows for a virtual list.

use std::path::{Path, PathBuf};

const IGNORED: &[&str] = &[".git", ".DS_Store", "node_modules", "target", ".build", ".gradle", ".idea"];

pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub expanded: bool,
    children: Option<Vec<Node>>,
}

#[derive(Clone)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub expanded: bool,
    pub depth: usize,
}

impl Node {
    pub fn root(path: &Path) -> Self {
        let mut node = Node::new(path.to_path_buf(), true);
        node.expanded = true;
        node.load();
        node
    }

    fn new(path: PathBuf, is_dir: bool) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Node { path, name, is_dir, expanded: false, children: None }
    }

    fn load(&mut self) {
        let mut children: Vec<Node> = match std::fs::read_dir(&self.path) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| !IGNORED.contains(&entry.file_name().to_string_lossy().as_ref()))
                .map(|entry| {
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    Node::new(entry.path(), is_dir)
                })
                .collect(),
            Err(err) => {
                eprintln!("forge: cannot read {}: {err}", self.path.display());
                Vec::new()
            }
        };
        children.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.children = Some(children);
    }

    /// Re-reads every loaded directory, keeping expansion state.
    pub fn refresh(&mut self) {
        if !self.is_dir || self.children.is_none() {
            return;
        }
        let old = self.children.take().unwrap_or_default();
        self.load();
        if let Some(children) = self.children.as_mut() {
            for child in children.iter_mut() {
                if let Some(prev) = old.iter().find(|o| o.path == child.path) {
                    child.expanded = prev.expanded;
                    if prev.children.is_some() {
                        child.load();
                        child.refresh();
                    }
                }
            }
        }
    }

    pub fn toggle(&mut self, path: &Path) -> bool {
        if let Some(node) = self.find_mut(path) {
            if node.is_dir {
                node.expanded = !node.expanded;
                if node.expanded && node.children.is_none() {
                    node.load();
                }
                return true;
            }
        }
        false
    }

    /// Expands every ancestor of `path` so it becomes visible.
    pub fn reveal(&mut self, path: &Path) {
        let Ok(relative) = path.strip_prefix(&self.path) else { return };
        let mut current = self.path.clone();
        let components: Vec<_> = relative.components().collect();
        for component in components.iter().take(components.len().saturating_sub(1)) {
            current.push(component);
            if let Some(node) = self.find_mut(&current) {
                node.expanded = true;
                if node.children.is_none() {
                    node.load();
                }
            }
        }
    }

    fn find_mut(&mut self, path: &Path) -> Option<&mut Node> {
        if self.path == path {
            return Some(self);
        }
        if !path.starts_with(&self.path) {
            return None;
        }
        self.children.as_mut()?.iter_mut().find_map(|child| child.find_mut(path))
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(children) = &self.children {
            for child in children {
                child.collect(0, &mut rows);
            }
        }
        rows
    }

    fn collect(&self, depth: usize, rows: &mut Vec<Row>) {
        rows.push(Row {
            path: self.path.clone(),
            name: self.name.clone(),
            is_dir: self.is_dir,
            expanded: self.expanded,
            depth,
        });
        if self.expanded {
            if let Some(children) = &self.children {
                for child in children {
                    child.collect(depth + 1, rows);
                }
            }
        }
    }
}

/// Language id for the editor's highlighter, derived from the file name.
pub fn language_for(path: &Path) -> &'static str {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    match name.as_str() {
        "makefile" | "gnumakefile" => return "make",
        "dockerfile" | ".zshrc" | ".bashrc" | ".zprofile" | ".bash_profile" | ".env" => return "bash",
        "cargo.lock" => return "toml",
        "cmakelists.txt" => return "cmake",
        "gemfile" | "podfile" | "fastfile" | "appfile" | "matchfile" | "rakefile" | "brewfile"
        | "dangerfile" | "vagrantfile" => return "ruby",
        "jenkinsfile" => return "groovy",
        _ => {}
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" => "rust",
        "swift" | "swiftinterface" => "swift",
        "kt" | "kts" => "kotlin",
        "java" => "java",
        "gradle" | "groovy" | "gvy" => "groovy",
        "xml" | "plist" | "xib" | "storyboard" | "entitlements" | "xcscheme" | "svg" | "csproj" => "xml",
        "properties" | "pro" => "properties",
        "rb" | "rake" | "gemspec" | "podspec" | "ru" => "ruby",
        "erb" => "erb",
        "dart" => "dart",
        "m" | "mm" => "objc",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "py" | "pyi" => "python",
        "go" => "go",
        "json" | "jsonc" | "json5" | "resolved" => "json",
        "toml" => "toml",
        "yml" | "yaml" => "yaml",
        "md" | "mdx" | "markdown" => "markdown",
        "sh" | "bash" | "zsh" | "fish" => "bash",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "css" | "scss" | "less" => "css",
        "html" | "htm" => "html",
        "sql" => "sql",
        "scala" | "sc" => "scala",
        "lua" => "lua",
        "php" => "php",
        "ex" | "exs" => "elixir",
        "zig" => "zig",
        "proto" => "proto",
        "graphql" | "gql" => "graphql",
        "cmake" => "cmake",
        "svelte" => "svelte",
        "astro" => "astro",
        "ejs" => "ejs",
        "diff" | "patch" => "diff",
        "ini" | "cfg" | "editorconfig" | "gitconfig" => "ini",
        _ => "text",
    }
}

/// Very rough binary sniff: NUL byte in the first 8 KiB.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

/// Write via a temp file + rename so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    // Follow symlinks so the link itself survives the rename.
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = resolved.as_path();
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.forge-tmp"));
    std::fs::write(&tmp, contents)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path)
}
