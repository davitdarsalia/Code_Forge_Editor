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
        "gradlew" => return "bash",
        ".gitignore" | ".dockerignore" | ".npmignore" | ".slugignore" | ".gcloudignore" => return "gitignore",
        ".gitmodules" => return "ini",
        _ => {}
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" => "rust",
        "swift" | "swiftinterface" => "swift",
        "kt" | "kts" => "kotlin",
        "java" => "java",
        "gradle" | "groovy" | "gvy" => "groovy",
        "xml" | "plist" | "xib" | "storyboard" | "entitlements" | "xcscheme" | "svg" | "csproj"
        | "stringsdict" | "xcworkspacedata" | "xcbkptlist" | "xcsettings" | "xcprivacy" | "iml" => "xml",
        "pbxproj" | "strings" => "openstep_plist",
        "xcconfig" => "xcconfig",
        "properties" => "properties",
        "pro" => "proguard",
        "bat" | "cmd" => "batch",
        "rb" | "rake" | "gemspec" | "podspec" | "ru" | "rbi" | "rbs" => "ruby",
        "erb" | "rhtml" => "erb",
        "dart" => "dart",
        "m" | "mm" => "objc",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "py" | "pyi" => "python",
        "go" => "go",
        "json" | "jsonc" | "json5" | "resolved" | "xcstrings" => "json",
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

pub fn detect_language(path: &Path, text: &str) -> &'static str {
    let language = language_for(path);
    if language != "text" {
        return language;
    }
    let first = text.lines().next().unwrap_or_default();
    let Some(shebang) = first.strip_prefix("#!") else { return language };
    let mut words = shebang.split_whitespace();
    let program = words.next().unwrap_or_default();
    let program = if program.ends_with("/env") { words.find(|w| !w.starts_with('-')).unwrap_or_default() } else { program };
    let interpreter = program.rsplit('/').next().unwrap_or_default();
    match interpreter.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.') {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" => "bash",
        "ruby" => "ruby",
        "python" => "python",
        "node" => "javascript",
        "lua" => "lua",
        "php" => "php",
        "groovy" => "groovy",
        "kotlin" => "kotlin",
        "swift" => "swift",
        _ => language,
    }
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = resolved.as_path();
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.forge-tmp"));
    let result = write_synced(&tmp, path, contents).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        if let Err(err) = std::fs::remove_file(&tmp) {
            if err.kind() != std::io::ErrorKind::NotFound {
                eprintln!("forge: cannot remove {}: {err}", tmp.display());
            }
        }
    }
    result
}

fn write_synced(tmp: &Path, original: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::File::create(tmp)?;
    file.write_all(contents)?;
    if let Ok(meta) = std::fs::metadata(original) {
        file.set_permissions(meta.permissions())?;
    }
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_contents_and_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("forge-files-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("run.sh");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(!dir.join(".run.sh.forge-tmp").exists());
        assert!(write_atomic(&dir.join("missing/x.txt"), b"x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dmote_files_map_to_highlighted_languages() {
        let cases = [
            ("DMote/App/DMoteApp.swift", "swift"),
            ("DMoteCoreIOS/Package.swift", "swift"),
            ("app/src/main/java/com/dlabs/dmoteapp/MainActivity.kt", "kotlin"),
            ("app/build.gradle.kts", "kotlin"),
            ("settings.gradle", "groovy"),
            ("app/src/main/AndroidManifest.xml", "xml"),
            ("app/src/main/res/values/strings.xml", "xml"),
            ("DMote/Info.plist", "xml"),
            ("DMote/DMote.entitlements", "xml"),
            ("DMote/PrivacyInfo.xcprivacy", "xml"),
            ("DMote.xcodeproj/xcshareddata/xcschemes/DMote.xcscheme", "xml"),
            ("DMote.xcodeproj/project.xcworkspace/contents.xcworkspacedata", "xml"),
            ("Breakpoints_v2.xcbkptlist", "xml"),
            (".idea/DMote_Android.iml", "xml"),
            ("en.lproj/Localizable.stringsdict", "xml"),
            ("DMote.xcodeproj/project.pbxproj", "openstep_plist"),
            ("en.lproj/Localizable.strings", "openstep_plist"),
            ("DMote/Config/Keys.xcconfig", "xcconfig"),
            ("Localization/Localizable.xcstrings", "json"),
            ("Package.resolved", "json"),
            ("Assets.xcassets/AppIcon.appiconset/Contents.json", "json"),
            ("gradle.properties", "properties"),
            ("app/proguard-rules.pro", "proguard"),
            ("dmotecore/consumer-rules.pro", "proguard"),
            ("gradle/libs.versions.toml", "toml"),
            (".github/workflows/ci.yml", "yaml"),
            ("fastlane/Fastfile", "ruby"),
            ("fastlane/Appfile", "ruby"),
            ("Gemfile", "ruby"),
            ("scripts/screens.sh", "bash"),
            ("gradlew", "bash"),
            ("gradlew.bat", "batch"),
            ("Makefile", "make"),
            ("README.md", "markdown"),
            (".gitignore", "gitignore"),
            ("scripts/resize.py", "python"),
        ];
        for (name, expected) in cases {
            assert_eq!(language_for(Path::new(name)), expected, "{name}");
        }
    }

    #[test]
    fn extensionless_scripts_use_the_shebang() {
        let run = Path::new("fastlane/run");
        assert_eq!(detect_language(run, "#!/bin/bash\nset -e\n"), "bash");
        assert_eq!(detect_language(run, "#!/usr/bin/env -S ruby -w\n"), "ruby");
        assert_eq!(detect_language(run, "#!/usr/bin/env python3\n"), "python");
        assert_eq!(detect_language(run, "plain notes\n"), "text");
        assert_eq!(detect_language(Path::new("x.swift"), "#!/usr/bin/env bash\n"), "swift");
    }
}
