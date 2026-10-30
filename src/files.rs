use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
        self.children = Some(list_dir(&self.path));
    }

    pub fn loaded_dirs(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        self.collect_loaded(&mut out);
        out
    }

    fn collect_loaded(&self, out: &mut Vec<PathBuf>) {
        if !self.is_dir {
            return;
        }
        if let Some(children) = self.children.as_ref() {
            out.push(self.path.clone());
            for child in children {
                child.collect_loaded(out);
            }
        }
    }

    pub fn apply_listings(&mut self, listings: &mut HashMap<PathBuf, Vec<Node>>) {
        if !self.is_dir || self.children.is_none() {
            return;
        }
        let Some(fresh) = listings.remove(&self.path) else { return };
        let mut old = self.children.take().unwrap_or_default();
        let mut fresh = fresh;
        for child in fresh.iter_mut() {
            if let Some(prev) = old.iter_mut().find(|o| o.path == child.path) {
                child.expanded = prev.expanded;
                if prev.children.is_some() {
                    child.children = prev.children.take();
                    child.apply_listings(listings);
                }
            }
        }
        self.children = Some(fresh);
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
        "m" | "mm" | "pch" => "objc",
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
        "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "inl" | "ipp" | "tpp" | "ixx" | "cppm" => "cpp",
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
    if language == "c" && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("h")) {
        return sniff_header(text);
    }
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

fn sniff_header(text: &str) -> &'static str {
    let head = &text[..text.len().min(32 * 1024)];
    let objc = ["@interface", "@protocol", "@import", "#import", "@class ", "NS_ASSUME_NONNULL", "@end"];
    if objc.iter().any(|marker| head.contains(marker)) {
        return "objc";
    }
    let cpp_lines = ["namespace ", "template <", "template<", "class ", "#include <string>", "#include <vector>", "#include <memory>", "public:", "private:", "std::", "constexpr ", "nullptr", "extern \"C++\""];
    if head.lines().any(|line| {
        let line = line.trim_start();
        cpp_lines.iter().any(|marker| line.starts_with(marker) || (marker.ends_with("::") || marker.ends_with("nullptr")) && line.contains(marker))
    }) {
        return "cpp";
    }
    "c"
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

pub fn list_dir(path: &Path) -> Vec<Node> {
    let mut children: Vec<Node> = match std::fs::read_dir(path) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| !IGNORED.contains(&entry.file_name().to_string_lossy().as_ref()))
            .map(|entry| {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                Node::new(entry.path(), is_dir)
            })
            .collect(),
        Err(err) => {
            eprintln!("forge: cannot read {}: {err}", path.display());
            Vec::new()
        }
    };
    children.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    children
}

#[derive(Default)]
pub struct WriteGate {
    last: Mutex<u64>,
}

impl WriteGate {
    pub fn write(&self, seq: u64, path: &Path, contents: &[u8]) -> std::io::Result<bool> {
        let mut last = self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if seq <= *last {
            return Ok(false);
        }
        write_atomic(path, contents)?;
        *last = seq;
        Ok(true)
    }
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
    fn write_gate_drops_older_sequence() {
        let dir = std::env::temp_dir().join(format!("forge-files-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        let gate = WriteGate::default();
        assert!(gate.write(2, &path, b"newer").unwrap());
        assert!(!gate.write(1, &path, b"older").unwrap());
        assert!(!gate.write(2, &path, b"dup").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer");
        assert!(gate.write(3, &path, b"latest").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "latest");
        assert!(gate.write(4, &dir.join("missing/x.txt"), b"x").is_err());
        assert!(gate.write(4, &path, b"retry").unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_gate_orders_concurrent_writers() {
        let dir = std::env::temp_dir().join(format!("forge-files-gate-mt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.txt");
        let gate = std::sync::Arc::new(WriteGate::default());
        let handles: Vec<_> = (1..=16u64)
            .rev()
            .map(|seq| {
                let (gate, path) = (gate.clone(), path.clone());
                std::thread::spawn(move || gate.write(seq, &path, format!("v{seq}").as_bytes()).unwrap())
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v16");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn apply_listings_keeps_expansion_and_loaded_children() {
        let dir = std::env::temp_dir().join(format!("forge-files-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/f.txt"), "").unwrap();
        let mut tree = Node::root(&dir);
        assert!(tree.toggle(&dir.join("a")));
        assert!(tree.toggle(&dir.join("a/b")));
        std::fs::write(dir.join("a/b/g.txt"), "").unwrap();
        std::fs::write(dir.join("new.txt"), "").unwrap();
        let mut listings: HashMap<PathBuf, Vec<Node>> =
            tree.loaded_dirs().into_iter().map(|p| { let l = list_dir(&p); (p, l) }).collect();
        assert_eq!(listings.len(), 3);
        tree.apply_listings(&mut listings);
        let names: Vec<String> = tree.rows().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["a", "b", "f.txt", "g.txt", "new.txt"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
    fn headers_are_sniffed_for_cpp_and_objc() {
        let h = Path::new("a.h");
        assert_eq!(detect_language(h, "#include <stdio.h>\nint f(void);\n"), "c");
        assert_eq!(detect_language(h, "#pragma once\nnamespace a { class B; }\n"), "cpp");
        assert_eq!(detect_language(h, "#include <vector>\nstd::vector<int> v;\n"), "cpp");
        assert_eq!(detect_language(h, "#import <Foundation/Foundation.h>\n@interface A : NSObject\n@end\n"), "objc");
        assert_eq!(detect_language(Path::new("a.hpp"), "int x;\n"), "cpp");
        assert_eq!(detect_language(Path::new("a.mm"), "int x;\n"), "objc");
    }

    #[test]
    fn gradle_and_web_files_map_to_expected_grammars() {
        let cases = [
            ("build.gradle", "groovy"),
            ("build.gradle.kts", "kotlin"),
            ("settings.gradle.kts", "kotlin"),
            ("settings.gradle", "groovy"),
            ("gradle.properties", "properties"),
            ("gradle/libs.versions.toml", "toml"),
            ("gradle/wrapper/gradle-wrapper.properties", "properties"),
            ("main.go", "go"),
            ("a.js", "javascript"),
            ("a.jsx", "javascript"),
            ("a.ts", "typescript"),
            ("a.tsx", "tsx"),
            ("a.cc", "cpp"),
            ("a.mm", "objc"),
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
