use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::build::XcodeContainer;

pub const CONFIG_FILE: &str = "buildServer.json";
pub const NEEDS_BUILD_NOTICE: &str = "Build once (⇧⌘B) to enable full Swift checking";
pub const MISSING_TOOL_NOTICE: &str =
    "Install xcode-build-server (`brew install xcode-build-server`) for full Swift checking in Xcode projects";
const TOOL: &str = "xcode-build-server";
const LOG_MANIFEST: &str = "Logs/Build/LogStoreManifest.plist";
const DEFAULT_IOS_TARGET: &str = "17.0";
const PROJECT_SCAN_DEPTH: usize = 2;
const ERROR_TAIL: usize = 3;
const ANCESTOR_LEVELS: usize = 2;
const TEST_PRODUCT_TYPES: &[&str] =
    &["com.apple.product-type.bundle.unit-test", "com.apple.product-type.bundle.ui-testing"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Ios,
    Macos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sdk {
    pub path: String,
    pub platform: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setup {
    Ready,
    NeedsBuild,
    MissingTool,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub setup: Setup,
    pub changed: bool,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub dir: PathBuf,
    pub container: XcodeContainer,
    pub scheme: Option<String>,
}

impl Target {
    pub fn new(dir: PathBuf, container: &XcodeContainer, scheme: Option<String>) -> Option<Target> {
        match container {
            XcodeContainer::Workspace(_) | XcodeContainer::Project(_) => {
                Some(Target { dir, container: container.clone(), scheme })
            }
            XcodeContainer::Package(_) => None,
        }
    }

    fn workspace(&self) -> PathBuf {
        match &self.container {
            XcodeContainer::Project(p) => p.join("project.xcworkspace"),
            XcodeContainer::Workspace(p) | XcodeContainer::Package(p) => p.clone(),
        }
    }
}

pub fn sourcekit_options(root: &Path) -> Value {
    let mut options = json!({ "backgroundIndexing": true });
    if let Some(fallback) = fallback_settings(root) {
        options["fallbackBuildSystem"] = fallback;
    }
    options
}

fn fallback_settings(root: &Path) -> Option<Value> {
    let (platform, deployment) = detect_platform(root)?;
    let sdk = sdk(platform)?;
    let mut flags = Vec::new();
    if platform == Platform::Ios {
        flags.extend(["-target".to_owned(), ios_triple(&deployment)]);
    }
    let frameworks = sdk.platform.join("Developer/Library/Frameworks");
    let libraries = sdk.platform.join("Developer/usr/lib");
    flags.extend(["-F".to_owned(), frameworks.display().to_string()]);
    flags.extend(["-I".to_owned(), libraries.display().to_string()]);
    Some(json!({ "sdk": sdk.path, "swiftCompilerFlags": flags }))
}

fn ios_triple(deployment: &str) -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{arch}-apple-ios{deployment}-simulator")
}

fn nearby(root: &Path) -> impl Iterator<Item = &Path> {
    root.ancestors().take(ANCESTOR_LEVELS + 1)
}

pub fn detect_platform(root: &Path) -> Option<(Platform, String)> {
    let mut projects = nearby(root).skip(1).map(projects_in).find(|found| !found.is_empty()).unwrap_or_default();
    if projects.is_empty() {
        find_projects(root, 0, &mut projects);
    }
    projects.sort();
    projects.iter().find_map(|project| {
        let text = std::fs::read_to_string(project.join("project.pbxproj")).ok()?;
        platform_from_pbxproj(&text)
    })
}

fn projects_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "xcodeproj")).collect();
    found.sort();
    found
}

pub fn test_root(root: &Path, path: &Path) -> Option<PathBuf> {
    if !root.join(CONFIG_FILE).is_file() {
        return None;
    }
    let first = path.strip_prefix(root).ok()?.components().next()?.as_os_str().to_str()?.to_owned();
    if Path::new(&first).extension().is_some() || !root.join(&first).is_dir() {
        return None;
    }
    let is_test = projects_in(root).iter().any(|project| test_targets(project).contains(&first));
    (is_test && !built_modules(root).contains(&first)).then(|| root.join(first))
}

fn compile_cache_dir(root: &Path) -> Option<PathBuf> {
    let name = root.to_str()?.replace('/', "-");
    Some(dirs::home_dir()?.join("Library/Caches").join(TOOL).join(name))
}

fn built_modules(root: &Path) -> Vec<String> {
    static CACHE: Mutex<Option<HashMap<PathBuf, (SystemTime, Vec<String>)>>> = Mutex::new(None);
    let Some(dir) = compile_cache_dir(root) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let files: Vec<(PathBuf, SystemTime)> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("compile_file"))
        .filter_map(|e| Some((e.path(), e.metadata().and_then(|m| m.modified()).ok()?)))
        .collect();
    let Some(newest) = files.iter().map(|(_, at)| *at).max() else { return Vec::new() };
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((_, modules)) = cache.get(&dir).filter(|(at, _)| *at == newest) {
        return modules.clone();
    }
    let mut modules: Vec<String> = files
        .iter()
        .filter_map(|(path, _)| std::fs::read_to_string(path).ok())
        .flat_map(|text| module_names(&text))
        .collect();
    modules.sort();
    modules.dedup();
    cache.insert(dir, (newest, modules.clone()));
    modules
}

pub fn module_names(compile_file: &str) -> Vec<String> {
    const KEY: &str = "\"module_name\":";
    compile_file
        .match_indices(KEY)
        .filter_map(|(at, _)| {
            let rest = compile_file[at + KEY.len()..].trim_start().strip_prefix('"')?;
            Some(rest[..rest.find('"')?].to_owned())
        })
        .collect()
}

fn test_targets(project: &Path) -> Vec<String> {
    static CACHE: Mutex<Option<HashMap<PathBuf, (SystemTime, Vec<String>)>>> = Mutex::new(None);
    let file = project.join("project.pbxproj");
    let Ok(modified) = std::fs::metadata(&file).and_then(|m| m.modified()) else { return Vec::new() };
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((_, names)) = cache.get(&file).filter(|(at, _)| *at == modified) {
        return names.clone();
    }
    let names = std::fs::read_to_string(&file).map(|text| test_target_names(&text)).unwrap_or_default();
    cache.insert(file, (modified, names.clone()));
    names
}

pub fn test_target_names(pbxproj: &str) -> Vec<String> {
    let mut names = Vec::new();
    let (mut native, mut name, mut test) = (false, None::<String>, false);
    for line in pbxproj.lines() {
        let trimmed = line.trim();
        if trimmed.ends_with("= {") && line.starts_with("\t\t") && !line.starts_with("\t\t\t") {
            (native, name, test) = (false, None, false);
        } else if line == "\t\t};" {
            if let (true, true, Some(target)) = (native, test, name.take()) {
                names.push(target);
            }
            native = false;
        } else if trimmed == "isa = PBXNativeTarget;" {
            native = true;
        } else if let Some(value) = trimmed.strip_prefix("name = ") {
            name = Some(value.trim_end_matches(';').trim_matches('"').to_owned());
        } else if let Some(value) = trimmed.strip_prefix("productType = ") {
            test = TEST_PRODUCT_TYPES.contains(&value.trim_end_matches(';').trim_matches('"'));
        }
    }
    names
}

fn find_projects(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".xcodeproj") {
            found.push(entry.path());
        } else if !name.starts_with('.') && !name.ends_with(".xcworkspace") && !crate::build::is_skipped_dir(&name) {
            subdirs.push(entry.path());
        }
    }
    if found.is_empty() && depth < PROJECT_SCAN_DEPTH {
        subdirs.sort();
        for sub in subdirs {
            find_projects(&sub, depth + 1, found);
        }
    }
}

pub fn platform_from_pbxproj(text: &str) -> Option<(Platform, String)> {
    let values = |key: &str| -> Vec<String> {
        text.lines()
            .filter_map(|line| line.trim().strip_prefix(key)?.trim_start().strip_prefix('='))
            .map(|value| value.trim().trim_end_matches(';').trim().trim_matches('"').to_owned())
            .collect()
    };
    let sdks = values("SDKROOT");
    if sdks.iter().any(|s| s == "iphoneos") {
        let deployment = values("IPHONEOS_DEPLOYMENT_TARGET")
            .into_iter()
            .min_by(|a, b| version_key(a).cmp(&version_key(b)))
            .unwrap_or_else(|| DEFAULT_IOS_TARGET.to_owned());
        return Some((Platform::Ios, deployment));
    }
    sdks.iter().any(|s| s == "macosx").then(|| (Platform::Macos, String::new()))
}

fn version_key(version: &str) -> Vec<u32> {
    version.split('.').map(|part| part.parse().unwrap_or(0)).collect()
}

fn sdk(platform: Platform) -> Option<Sdk> {
    static IOS: OnceLock<Option<Sdk>> = OnceLock::new();
    static MAC: OnceLock<Option<Sdk>> = OnceLock::new();
    let (cell, name) = match platform {
        Platform::Ios => (&IOS, "iphonesimulator"),
        Platform::Macos => (&MAC, "macosx"),
    };
    cell.get_or_init(|| {
        let path = xcrun_sdk(name, "--show-sdk-path")?;
        let platform = PathBuf::from(xcrun_sdk(name, "--show-sdk-platform-path")?);
        Some(Sdk { path, platform })
    })
    .clone()
}

fn xcrun_sdk(sdk: &str, query: &str) -> Option<String> {
    let output = Command::new("/usr/bin/xcrun").args(["--sdk", sdk, query]).stdin(Stdio::null()).output();
    match output {
        Ok(out) if out.status.success() => {
            let value = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            (!value.is_empty()).then_some(value)
        }
        Ok(out) => {
            eprintln!("forge: xcrun --sdk {sdk} {query} exited with {}", out.status);
            None
        }
        Err(err) => {
            eprintln!("forge: cannot run xcrun: {err}");
            None
        }
    }
}

fn read_config(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(err) => {
            eprintln!("forge: unreadable {}: {err}", path.display());
            None
        }
    }
}

fn needs_config(existing: Option<&Value>, target: &Target, rebind: bool) -> bool {
    let Some(config) = existing else { return true };
    if config.get("kind").and_then(Value::as_str) != Some("xcode") || !rebind {
        return false;
    }
    let same_workspace = config.get("workspace").and_then(Value::as_str).map(Path::new) == Some(&target.workspace());
    let scheme = config.get("scheme").and_then(Value::as_str);
    same_workspace && target.scheme.is_some() && scheme != target.scheme.as_deref()
}

fn has_build_logs(config: &Value) -> bool {
    match config.get("build_root").and_then(Value::as_str) {
        Some(root) => Path::new(root).join(LOG_MANIFEST).is_file(),
        None => true,
    }
}

pub fn prepare(target: &Target, rebind: bool) -> Prepared {
    let path = target.dir.join(CONFIG_FILE);
    let existing = read_config(&path);
    let mut changed = false;
    if needs_config(existing.as_ref(), target, rebind) {
        if let Err(setup) = configure(target) {
            return Prepared { setup, changed: false };
        }
        changed = true;
        exclude_from_git(&target.dir);
    }
    let setup = match read_config(&path) {
        Some(config) if has_build_logs(&config) => Setup::Ready,
        Some(_) => Setup::NeedsBuild,
        None => Setup::Failed(format!("{TOOL} did not write {}", path.display())),
    };
    Prepared { setup, changed }
}

fn configure(target: &Target) -> Result<(), Setup> {
    let tool = crate::tools::which(TOOL).ok_or(Setup::MissingTool)?;
    let mut command = Command::new(tool);
    command.current_dir(&target.dir).arg("config");
    match &target.container {
        XcodeContainer::Workspace(p) => command.arg("-workspace").arg(p),
        XcodeContainer::Project(p) => command.arg("-project").arg(p),
        XcodeContainer::Package(_) => return Err(Setup::Failed("SwiftPM packages need no build server".into())),
    };
    if let Some(scheme) = &target.scheme {
        command.arg("-scheme").arg(scheme);
    }
    let path_env = std::env::join_paths(crate::tools::search_dirs()).unwrap_or_default();
    let output = command
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| Setup::Failed(format!("cannot run {TOOL}: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    let mut lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect();
    let tail = lines.split_off(lines.len().saturating_sub(ERROR_TAIL)).join(" / ");
    Err(Setup::Failed(format!("{TOOL} config exited with {}: {tail}", output.status)))
}

fn git(dir: &Path, args: &[&str]) -> Option<std::process::Output> {
    match Command::new("git").current_dir(dir).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(output) => Some(output),
        Err(err) => {
            eprintln!("forge: cannot run git: {err}");
            None
        }
    }
}

pub fn exclude_line(prefix: &str) -> String {
    format!("/{prefix}{CONFIG_FILE}")
}

fn exclude_from_git(dir: &Path) {
    let Some(out) = git(dir, &["rev-parse", "--git-path", "info/exclude", "--show-prefix"]) else { return };
    if !out.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut lines = text.lines();
    let (Some(exclude), prefix) = (lines.next(), lines.next().unwrap_or("")) else { return };
    let tracked = git(dir, &["ls-files", "--error-unmatch", CONFIG_FILE]).is_some_and(|o| o.status.success());
    let ignored = git(dir, &["check-ignore", "-q", CONFIG_FILE]).is_some_and(|o| o.status.success());
    if tracked || ignored {
        return;
    }
    if let Err(err) = append_line(&dir.join(exclude), &exclude_line(prefix)) {
        eprintln!("forge: cannot update {exclude}: {err}");
    }
}

pub fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    if existing.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    crate::files::write_atomic(path, format!("{existing}{separator}{line}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-xcode-lsp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pbxproj_reveals_ios_and_lowest_deployment_target() {
        let text = "\t\t\t\tIPHONEOS_DEPLOYMENT_TARGET = 16.4;\n\t\t\t\tSDKROOT = iphoneos;\n\t\t\t\tIPHONEOS_DEPLOYMENT_TARGET = 16.0;\n";
        assert_eq!(platform_from_pbxproj(text), Some((Platform::Ios, "16.0".into())));
        assert_eq!(platform_from_pbxproj("SDKROOT = iphoneos;\n"), Some((Platform::Ios, DEFAULT_IOS_TARGET.into())));
        assert_eq!(platform_from_pbxproj("\tSDKROOT = macosx;\n"), Some((Platform::Macos, String::new())));
        assert_eq!(platform_from_pbxproj("name = x;\n"), None);
        assert!(version_key("16.10") > version_key("16.4"));
    }

    #[test]
    fn platform_detection_finds_nested_projects() {
        let dir = temp("detect");
        let project = dir.join("App/App.xcodeproj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("project.pbxproj"), "SDKROOT = iphoneos;\nIPHONEOS_DEPLOYMENT_TARGET = 15.0;\n").unwrap();
        assert_eq!(detect_platform(&dir), Some((Platform::Ios, "15.0".into())));
        assert_eq!(detect_platform(&dir.join("App")), Some((Platform::Ios, "15.0".into())));
        assert_eq!(detect_platform(&dir.join("missing")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_targets_route_to_their_own_root() {
        let text = "\t\tA /* App */ = {\n\t\t\tisa = PBXNativeTarget;\n\t\t\tname = App;\n\t\t\tproductType = \"com.apple.product-type.application\";\n\t\t};\n\t\tB /* AppTests */ = {\n\t\t\tisa = PBXNativeTarget;\n\t\t\tname = AppTests;\n\t\t\tproductType = \"com.apple.product-type.bundle.unit-test\";\n\t\t};\n\t\tC /* UI */ = {\n\t\t\tisa = PBXNativeTarget;\n\t\t\tname = UI;\n\t\t\tproductType = \"com.apple.product-type.bundle.ui-testing\";\n\t\t};\n";
        assert_eq!(test_target_names(text), vec!["AppTests".to_owned(), "UI".to_owned()]);
        assert_eq!(
            module_names("[{\"module_name\": \"App\", \"x\": 1}, {\"module_name\":\"AppTests\"}]"),
            vec!["App".to_owned(), "AppTests".to_owned()]
        );
        let dir = temp("tests");
        std::fs::create_dir_all(dir.join("App.xcodeproj")).unwrap();
        std::fs::create_dir_all(dir.join("AppTests")).unwrap();
        std::fs::create_dir_all(dir.join("App")).unwrap();
        std::fs::write(dir.join("App.xcodeproj/project.pbxproj"), text).unwrap();
        let file = dir.join("AppTests/ATests.swift");
        assert_eq!(test_root(&dir, &file), None);
        std::fs::write(dir.join(CONFIG_FILE), "{}").unwrap();
        assert_eq!(test_root(&dir, &file), Some(dir.join("AppTests")));
        assert_eq!(test_root(&dir, &dir.join("App/A.swift")), None);
        assert_eq!(test_root(&dir, &dir.join("Top.swift")), None);
        assert_eq!(detect_platform(&dir.join("AppTests")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn triples_target_the_simulator() {
        let triple = ios_triple("16.0");
        assert!(triple.ends_with("-apple-ios16.0-simulator"));
        assert!(!triple.starts_with("aarch64"));
    }

    #[test]
    fn config_is_only_regenerated_when_missing_or_rebound() {
        let dir = PathBuf::from("/p");
        let target = Target::new(dir.clone(), &XcodeContainer::Project(dir.join("A.xcodeproj")), Some("A".into())).unwrap();
        assert!(needs_config(None, &target, false));
        let bound = json!({ "kind": "xcode", "workspace": "/p/A.xcodeproj/project.xcworkspace", "scheme": "B" });
        assert!(!needs_config(Some(&bound), &target, false));
        assert!(needs_config(Some(&bound), &target, true));
        let same = json!({ "kind": "xcode", "workspace": "/p/A.xcodeproj/project.xcworkspace", "scheme": "A" });
        assert!(!needs_config(Some(&same), &target, true));
        let manual = json!({ "kind": "manual" });
        assert!(!needs_config(Some(&manual), &target, true));
        let other = json!({ "kind": "xcode", "workspace": "/q/B.xcworkspace", "scheme": "B" });
        assert!(!needs_config(Some(&other), &target, true));
        assert!(Target::new(dir.clone(), &XcodeContainer::Package(dir), None).is_none());
    }

    #[test]
    fn build_logs_decide_readiness() {
        let dir = temp("logs");
        let config = json!({ "kind": "xcode", "build_root": dir.display().to_string() });
        assert!(!has_build_logs(&config));
        std::fs::create_dir_all(dir.join("Logs/Build")).unwrap();
        std::fs::write(dir.join(LOG_MANIFEST), "").unwrap();
        assert!(has_build_logs(&config));
        assert!(has_build_logs(&json!({ "kind": "manual" })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exclude_lines_are_appended_once() {
        let dir = temp("exclude");
        let path = dir.join("info/exclude");
        append_line(&path, &exclude_line("App/")).unwrap();
        append_line(&path, &exclude_line("App/")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "/App/buildServer.json\n");
        std::fs::write(&path, "# c").unwrap();
        append_line(&path, &exclude_line("")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# c\n/buildServer.json\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn options_enable_indexing_and_simulator_fallback() {
        let dir = temp("options");
        let project = dir.join("App.xcodeproj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("project.pbxproj"), "SDKROOT = iphoneos;\nIPHONEOS_DEPLOYMENT_TARGET = 16.0;\n").unwrap();
        let options = sourcekit_options(&dir);
        assert_eq!(options["backgroundIndexing"], true);
        if sdk(Platform::Ios).is_some() {
            let flags: Vec<&str> = options["fallbackBuildSystem"]["swiftCompilerFlags"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert!(flags.iter().any(|f| f.ends_with("-apple-ios16.0-simulator")));
            assert!(flags.iter().any(|f| f.ends_with("Developer/Library/Frameworks")));
            assert!(options["fallbackBuildSystem"]["sdk"].as_str().is_some_and(|s| s.contains("iPhoneSimulator")));
        }
        let _ = std::fs::remove_dir_all(&dir);
        let bare = temp("bare").join("a/b/c");
        std::fs::create_dir_all(&bare).unwrap();
        assert!(sourcekit_options(&bare).get("fallbackBuildSystem").is_none());
        let _ = std::fs::remove_dir_all(temp("bare"));
    }

    #[test]
    #[ignore]
    fn real_xcode_project_gets_a_build_server() {
        let dir = PathBuf::from(std::env::var("FORGE_XCODE_PROJECT_DIR").expect("set FORGE_XCODE_PROJECT_DIR"));
        let project = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "xcodeproj"))
            .expect("no .xcodeproj");
        let scheme = std::env::var("FORGE_XCODE_SCHEME").ok();
        let target = Target::new(dir.clone(), &XcodeContainer::Project(project), scheme).unwrap();
        let prepared = prepare(&target, false);
        assert!(matches!(prepared.setup, Setup::Ready | Setup::NeedsBuild), "{prepared:?}");
        assert!(dir.join(CONFIG_FILE).is_file());
        let ignored = git(&dir, &["check-ignore", "-q", CONFIG_FILE]).is_some_and(|o| o.status.success());
        let tracked = git(&dir, &["ls-files", "--error-unmatch", CONFIG_FILE]).is_some_and(|o| o.status.success());
        assert!(ignored || tracked);
    }
}
