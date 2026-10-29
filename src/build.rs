//! Project build integration, no UI: detects Xcode and Gradle projects under
//! the open folder, turns a target + action into a command line, runs it
//! (cancellable, output streamed line by line) and parses compiler
//! diagnostics out of the output.

use std::collections::HashSet;
use std::io::{BufRead as _, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How deep below the project root detection looks for build files.
pub const DETECT_DEPTH: usize = 4;
/// Directories never worth descending into (outputs, caches, vendored code).
const SKIP_DIRS: &[&str] = &[
    "build", "Build", "DerivedData", "node_modules", "Pods", "Carthage", "target", "vendor",
    "fastlane", "out", "dist",
];
pub const GENERIC_SIMULATOR: &str = "generic/platform=iOS Simulator";

#[derive(Clone, Debug, PartialEq)]
pub enum XcodeContainer {
    Workspace(PathBuf),
    Project(PathBuf),
    /// A directory holding `Package.swift`.
    Package(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModuleKind {
    AndroidApp,
    AndroidLibrary,
    Jvm,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GradleModule {
    /// Gradle path without the leading colon, e.g. `app` or `core:domain`.
    pub path: String,
    pub kind: ModuleKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProjectKind {
    Xcode { container: XcodeContainer, schemes: Vec<String> },
    Gradle { modules: Vec<GradleModule> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuildProject {
    pub name: String,
    /// Directory the build command runs in.
    pub dir: PathBuf,
    pub kind: ProjectKind,
}

impl BuildProject {
    pub fn is_xcode(&self) -> bool {
        matches!(self.kind, ProjectKind::Xcode { .. })
    }

    /// Picker label for the project itself, e.g. "DMote (iOS)".
    pub fn label(&self) -> String {
        match &self.kind {
            ProjectKind::Xcode { container: XcodeContainer::Package(_), .. } => format!("{} (SwiftPM)", self.name),
            ProjectKind::Xcode { .. } => format!("{} (Xcode)", self.name),
            ProjectKind::Gradle { .. } => format!("{} (Gradle)", self.name),
        }
    }

    /// Selectable units: schemes for Xcode, modules for Gradle. `None` means
    /// "the whole project" (no schemes listed yet, or a single-root build).
    pub fn units(&self) -> Vec<Option<String>> {
        let names: Vec<String> = match &self.kind {
            ProjectKind::Xcode { schemes, .. } => schemes.clone(),
            ProjectKind::Gradle { modules } => modules.iter().map(|m| m.path.clone()).collect(),
        };
        if names.is_empty() { vec![None] } else { names.into_iter().map(Some).collect() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Destination {
    pub label: String,
    /// Value for `xcodebuild -destination`.
    pub spec: String,
}

impl Destination {
    pub fn generic_simulator() -> Self {
        Destination { label: "Any iOS Simulator".into(), spec: GENERIC_SIMULATOR.into() }
    }

    pub fn mac() -> Self {
        Destination { label: "My Mac".into(), spec: "platform=macOS".into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BuildAction {
    Build,
    Clean,
    /// Gradle `installDebug` (no Xcode equivalent yet).
    Install,
}

impl BuildAction {
    pub fn verb(self) -> &'static str {
        match self {
            BuildAction::Build => "Build",
            BuildAction::Clean => "Clean",
            BuildAction::Install => "Install",
        }
    }

    pub fn progressive(self) -> &'static str {
        match self {
            BuildAction::Build => "Building",
            BuildAction::Clean => "Cleaning",
            BuildAction::Install => "Installing",
        }
    }
}

// ---- Detection --------------------------------------------------------------

/// Finds build roots below `root` (depth-limited, build outputs ignored).
/// Fast: only reads directory listings and small settings files. Scheme
/// lists are filled in later by [`list_schemes`].
pub fn detect(root: &Path) -> Vec<BuildProject> {
    let mut found = Vec::new();
    walk(root, 0, &mut found);
    found
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<BuildProject>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut workspaces = Vec::new();
    let mut projects = Vec::new();
    let mut subdirs = Vec::new();
    let (mut has_package, mut has_gradlew, mut gradle_settings) = (false, false, None);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if is_dir {
            if name.ends_with(".xcworkspace") {
                workspaces.push(path);
            } else if name.ends_with(".xcodeproj") {
                projects.push(path);
            } else if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                subdirs.push(path);
            }
            continue;
        }
        match name.as_str() {
            "Package.swift" => has_package = true,
            "gradlew" => has_gradlew = true,
            "settings.gradle.kts" | "settings.gradle" => gradle_settings = Some(path),
            _ => {}
        }
    }
    workspaces.sort();
    projects.sort();
    subdirs.sort();

    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir_name = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // A workspace wraps its projects, so it wins; otherwise one entry per project.
    if let Some(ws) = workspaces.first() {
        found.push(xcode(stem(ws), dir, XcodeContainer::Workspace(ws.clone())));
    } else if !projects.is_empty() {
        for project in &projects {
            found.push(xcode(stem(project), dir, XcodeContainer::Project(project.clone())));
        }
    } else if has_package {
        found.push(xcode(dir_name.clone(), dir, XcodeContainer::Package(dir.to_path_buf())));
    }
    if let (true, Some(settings)) = (has_gradlew, gradle_settings) {
        let text = std::fs::read_to_string(&settings).unwrap_or_default();
        let name = gradle_root_name(&text).unwrap_or(dir_name);
        let modules = gradle_includes(&text)
            .into_iter()
            .map(|path| {
                let kind = module_kind(&dir.join(path.replace(':', "/")));
                GradleModule { path, kind }
            })
            .collect();
        found.push(BuildProject { name, dir: dir.to_path_buf(), kind: ProjectKind::Gradle { modules } });
    }

    if depth < DETECT_DEPTH {
        for sub in subdirs {
            walk(&sub, depth + 1, found);
        }
    }
}

fn xcode(name: String, dir: &Path, container: XcodeContainer) -> BuildProject {
    BuildProject { name, dir: dir.to_path_buf(), kind: ProjectKind::Xcode { container, schemes: Vec::new() } }
}

/// `rootProject.name = "X"` from a settings script.
pub fn gradle_root_name(settings: &str) -> Option<String> {
    settings.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("rootProject.name")?.trim_start().strip_prefix('=')?;
        quoted(rest).into_iter().next()
    })
}

/// Module paths from `include(":a", ":b")` / `include ':a'` lines.
pub fn gradle_includes(settings: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in settings.lines() {
        let code = line.split("//").next().unwrap_or("").trim();
        let Some(rest) = code.strip_prefix("include") else { continue };
        if !rest.starts_with(['(', ' ', '\t']) {
            continue;
        }
        for module in quoted(rest) {
            let module = module.trim_start_matches(':').to_string();
            if !module.is_empty() && !out.contains(&module) {
                out.push(module);
            }
        }
    }
    out
}

/// Every single- or double-quoted string literal on a line.
fn quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['"', '\'']) {
        let quote = rest[start..].chars().next().unwrap_or('"');
        let after = &rest[start + 1..];
        let Some(end) = after.find(quote) else { break };
        out.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

fn module_kind(module_dir: &Path) -> ModuleKind {
    let script = ["build.gradle.kts", "build.gradle"]
        .iter()
        .find_map(|f| std::fs::read_to_string(module_dir.join(f)).ok())
        .unwrap_or_default();
    classify_module(&script)
}

pub fn classify_module(script: &str) -> ModuleKind {
    if script.contains("com.android.application") || script.contains("android.application") {
        ModuleKind::AndroidApp
    } else if script.contains("com.android.library") || script.contains("android.library") {
        ModuleKind::AndroidLibrary
    } else {
        ModuleKind::Jvm
    }
}

/// Runs `xcodebuild -list -json` for a container. Slow (seconds): background only.
pub fn list_schemes(dir: &Path, container: &XcodeContainer) -> anyhow::Result<Vec<String>> {
    let mut command = Command::new("xcodebuild");
    command.current_dir(dir).arg("-list").arg("-json");
    match container {
        XcodeContainer::Workspace(p) => command.arg("-workspace").arg(p),
        XcodeContainer::Project(p) => command.arg("-project").arg(p),
        XcodeContainer::Package(_) => &mut command,
    };
    let output = command.stdin(Stdio::null()).stderr(Stdio::null()).output()?;
    if !output.status.success() {
        anyhow::bail!("xcodebuild -list exited with {}", output.status);
    }
    parse_scheme_list(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_scheme_list(json: &str) -> anyhow::Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let schemes = ["workspace", "project"]
        .iter()
        .find_map(|key| value.get(key)?.get("schemes")?.as_array())
        .map(|list| list.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Ok(schemes)
}

/// Booted simulators (`xcrun simctl list devices booted -j`). Background only.
pub fn booted_simulators() -> Vec<Destination> {
    let output = Command::new("xcrun")
        .args(["simctl", "list", "devices", "booted", "-j"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => parse_simulators(&String::from_utf8_lossy(&output.stdout)),
        Ok(output) => {
            eprintln!("forge: simctl exited with {}", output.status);
            Vec::new()
        }
        Err(err) => {
            eprintln!("forge: cannot run simctl: {err}");
            Vec::new()
        }
    }
}

pub fn parse_simulators(json: &str) -> Vec<Destination> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let Some(runtimes) = value.get("devices").and_then(|d| d.as_object()) else { return Vec::new() };
    let mut out = Vec::new();
    for devices in runtimes.values().filter_map(|v| v.as_array()) {
        for device in devices {
            let get = |k: &str| device.get(k).and_then(|v| v.as_str());
            if get("state") != Some("Booted") {
                continue;
            }
            if let (Some(name), Some(udid)) = (get("name"), get("udid")) {
                out.push(Destination { label: name.into(), spec: format!("platform=iOS Simulator,id={udid}") });
            }
        }
    }
    out
}

// ---- Commands ---------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct CommandLine {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl CommandLine {
    pub fn display(&self) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }));
        parts.join(" ")
    }
}

/// Command for `action` on one unit (scheme/module) of a project.
/// `None` when the action does not apply (Install on Xcode, on a library).
pub fn command_for(
    project: &BuildProject,
    unit: Option<&str>,
    destination: &Destination,
    action: BuildAction,
) -> Option<CommandLine> {
    match &project.kind {
        ProjectKind::Xcode { container, .. } => {
            let verb = match action {
                BuildAction::Build => "build",
                BuildAction::Clean => "clean",
                BuildAction::Install => return None,
            };
            let mut args = Vec::new();
            match container {
                XcodeContainer::Workspace(p) => args.extend(["-workspace".into(), p.display().to_string()]),
                XcodeContainer::Project(p) => args.extend(["-project".into(), p.display().to_string()]),
                XcodeContainer::Package(_) => {}
            }
            if let Some(scheme) = unit {
                args.extend(["-scheme".into(), scheme.to_string()]);
            }
            args.extend(["-destination".into(), destination.spec.clone(), verb.into()]);
            Some(CommandLine { program: "xcodebuild".into(), args, cwd: project.dir.clone() })
        }
        ProjectKind::Gradle { modules } => {
            let kind = unit.and_then(|u| modules.iter().find(|m| m.path == u)).map(|m| m.kind);
            let task = match (action, kind) {
                (BuildAction::Clean, _) => "clean",
                (BuildAction::Build, Some(ModuleKind::Jvm)) => "assemble",
                (BuildAction::Build, _) => "assembleDebug",
                (BuildAction::Install, Some(ModuleKind::AndroidApp) | None) => "installDebug",
                (BuildAction::Install, _) => return None,
            };
            let task = match unit {
                Some(module) => format!(":{module}:{task}"),
                None => task.to_string(),
            };
            Some(CommandLine {
                program: "/bin/sh".into(),
                args: vec![project.dir.join("gradlew").display().to_string(), task, "--console=plain".into()],
                cwd: project.dir.clone(),
            })
        }
    }
}

// ---- Diagnostics ------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Issue {
    pub severity: Severity,
    pub path: Option<PathBuf>,
    /// 1-based, as printed by the compiler (0 when unknown).
    pub line: u32,
    pub column: u32,
    pub message: String,
}

/// Drops ANSI escape sequences (Gradle and some tools colour their output).
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Recognises clang/swiftc/javac (`/p:12:5: error: m`), kotlinc
/// (`e: file:///p:12:5 m`, old `e: /p: (12, 5): m`) and location-less
/// `error: m` lines.
pub fn parse_issue(line: &str) -> Option<Issue> {
    let line = line.trim_end();
    parse_kotlin(line).or_else(|| parse_clang(line)).or_else(|| parse_bare(line))
}

fn parse_kotlin(line: &str) -> Option<Issue> {
    let (severity, rest) = if let Some(rest) = line.strip_prefix("e: ") {
        (Severity::Error, rest)
    } else if let Some(rest) = line.strip_prefix("w: ") {
        (Severity::Warning, rest)
    } else {
        return None;
    };
    let path_part = rest.strip_prefix("file://").unwrap_or(rest);
    if !path_part.starts_with('/') {
        return Some(Issue { severity, path: None, line: 0, column: 0, message: rest.to_string() });
    }
    // Old format: `/p/A.kt: (12, 5): message`.
    if let Some(ix) = path_part.find(": (") {
        let after = &path_part[ix + 3..];
        if let Some(close) = after.find("): ") {
            let mut nums = after[..close].split(',').map(|n| n.trim().parse::<u32>());
            if let (Some(Ok(l)), Some(Ok(c))) = (nums.next(), nums.next()) {
                return Some(Issue {
                    severity,
                    path: Some(PathBuf::from(&path_part[..ix])),
                    line: l,
                    column: c,
                    message: after[close + 3..].to_string(),
                });
            }
        }
    }
    // New format: `/p/A.kt:12:5 message`.
    let (location, message) = path_part.split_once(' ')?;
    let (path, line_no, column) = split_location(location)?;
    Some(Issue { severity, path: Some(path), line: line_no, column, message: message.to_string() })
}

/// `path:line[:col]` → parts. Splits from the right so paths keep colons.
fn split_location(location: &str) -> Option<(PathBuf, u32, u32)> {
    let (head, last) = location.rsplit_once(':')?;
    let last = last.parse::<u32>().ok()?;
    if let Some((path, mid)) = head.rsplit_once(':') {
        if let (Ok(line), false) = (mid.parse::<u32>(), path.is_empty()) {
            return Some((PathBuf::from(path), line, last));
        }
    }
    (!head.is_empty()).then(|| (PathBuf::from(head), last, 0))
}

fn parse_clang(line: &str) -> Option<Issue> {
    if !line.starts_with('/') {
        return None;
    }
    for (marker, severity) in [
        (": fatal error: ", Severity::Error),
        (": error: ", Severity::Error),
        (": warning: ", Severity::Warning),
    ] {
        if let Some(ix) = line.find(marker) {
            let (path, line_no, column) = split_location(&line[..ix])?;
            return Some(Issue {
                severity,
                path: Some(path),
                line: line_no,
                column,
                message: line[ix + marker.len()..].to_string(),
            });
        }
    }
    None
}

fn parse_bare(line: &str) -> Option<Issue> {
    let rest = line.strip_prefix("xcodebuild: ").unwrap_or(line);
    let message = rest.strip_prefix("error: ").or_else(|| rest.strip_prefix("fatal error: "))?;
    Some(Issue { severity: Severity::Error, path: None, line: 0, column: 0, message: message.to_string() })
}

/// Keeps first occurrences only: xcodebuild repeats diagnostics per arch.
#[derive(Default)]
pub struct IssueSet {
    seen: HashSet<Issue>,
}

impl IssueSet {
    pub fn insert(&mut self, issue: &Issue) -> bool {
        self.seen.insert(issue.clone())
    }
}

// ---- Running ----------------------------------------------------------------

pub enum RunEvent {
    Line(String, Option<Issue>),
    /// Exit code; `None` when killed by a signal.
    Exited(Option<i32>),
    Failed(String),
}

/// How long a stopped build gets to exit before its group is SIGKILLed.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// A running build. Dropping it does not stop the process; call [`stop`](Self::stop).
pub struct RunHandle {
    pid: i32,
    exited: Arc<AtomicBool>,
}

impl RunHandle {
    /// SIGTERMs the whole process group (xcodebuild/Gradle spawn helpers),
    /// escalating to SIGKILL if it is still alive after a grace period.
    pub fn stop(&self) {
        signal_group(self.pid, libc::SIGTERM);
        let (pid, exited) = (self.pid, self.exited.clone());
        let spawned = std::thread::Builder::new().name("forge-build-kill".into()).spawn(move || {
            std::thread::sleep(STOP_GRACE);
            if !exited.load(Ordering::SeqCst) {
                signal_group(pid, libc::SIGKILL);
            }
        });
        if let Err(err) = spawned {
            eprintln!("forge: cannot start build kill timer: {err}");
        }
    }
}

fn signal_group(pid: i32, signal: libc::c_int) {
    // SAFETY: plain syscall; a negative pid targets the process group we created.
    if unsafe { libc::kill(-pid, signal) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            eprintln!("forge: signalling build {pid} failed: {err}");
        }
    }
}

/// Spawns `command` through the user's login shell (so PATH, JAVA_HOME and
/// friends match the terminal) in its own process group. Output lines and
/// the exit arrive on `tx` from reader threads; nothing polls.
pub fn spawn(command: &CommandLine, tx: flume::Sender<RunEvent>) -> anyhow::Result<RunHandle> {
    use std::os::unix::process::CommandExt as _;
    let shell = login_shell(std::env::var("SHELL").ok().as_deref());
    let mut child = Command::new(shell);
    child
        .arg("-l")
        .arg("-c")
        .arg("exec \"$0\" \"$@\"")
        .arg(&command.program)
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if std::env::var_os("JAVA_HOME").is_none() {
        // Android Studio's bundled JDK, the usual one on a Mac without a system JDK.
        let jbr = Path::new("/Applications/Android Studio.app/Contents/jbr/Contents/Home");
        if jbr.is_dir() {
            child.env("JAVA_HOME", jbr);
        }
    }
    let mut child = child.spawn()?;
    let pid = child.id() as i32;
    let exited = Arc::new(AtomicBool::new(false));
    let exited_flag = exited.clone();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let readers: Vec<_> = [stdout.map(|s| Box::new(s) as Box<dyn Read + Send>), stderr.map(|s| Box::new(s) as _)]
        .into_iter()
        .flatten()
        .map(|stream| {
            let tx = tx.clone();
            std::thread::Builder::new().name("forge-build-read".into()).spawn(move || read_lines(stream, tx))
        })
        .collect::<Result<_, _>>()?;
    std::thread::Builder::new().name("forge-build-wait".into()).spawn(move || {
        for reader in readers {
            let _ = reader.join();
        }
        let event = match child.wait() {
            Ok(status) => RunEvent::Exited(status.code()),
            Err(err) => RunEvent::Failed(format!("wait failed: {err}")),
        };
        exited_flag.store(true, Ordering::SeqCst);
        let _ = tx.send(event);
    })?;
    Ok(RunHandle { pid, exited })
}

/// The user's shell when it is POSIX-like; `exec "$0" "$@"` (which keeps the
/// arguments out of shell parsing) means nothing to fish, csh or nushell.
fn login_shell(shell: Option<&str>) -> String {
    const POSIX: &[&str] = &["sh", "bash", "zsh", "ksh", "dash"];
    shell
        .filter(|s| s.starts_with('/') && POSIX.contains(&s.rsplit('/').next().unwrap_or_default()))
        .unwrap_or("/bin/zsh")
        .to_string()
}

fn read_lines(stream: Box<dyn Read + Send>, tx: flume::Sender<RunEvent>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => return,
            Ok(_) => {
                let text = strip_ansi(String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']));
                let issue = parse_issue(&text);
                if tx.send(RunEvent::Line(text, issue)).is_err() {
                    return;
                }
            }
            Err(err) => {
                eprintln!("forge: build output read failed: {err}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_shell_falls_back_for_non_posix_shells() {
        assert_eq!(login_shell(Some("/opt/homebrew/bin/bash")), "/opt/homebrew/bin/bash");
        assert_eq!(login_shell(Some("/bin/zsh")), "/bin/zsh");
        assert_eq!(login_shell(Some("/opt/homebrew/bin/fish")), "/bin/zsh");
        assert_eq!(login_shell(Some("/bin/tcsh")), "/bin/zsh");
        assert_eq!(login_shell(Some("nu")), "/bin/zsh");
        assert_eq!(login_shell(None), "/bin/zsh");
    }

    /// Scheme names and paths reach the program as argv, never as shell text.
    #[test]
    fn spawn_passes_hostile_arguments_verbatim() {
        let dir = temp_dir("argv");
        let marker = dir.join("pwned");
        let hostile = format!("x\"; touch {}; echo \"$(touch {})", marker.display(), marker.display());
        let command = CommandLine { program: "/bin/echo".into(), args: vec![hostile.clone()], cwd: dir.clone() };
        let (tx, rx) = flume::unbounded();
        spawn(&command, tx).unwrap();
        let mut lines = Vec::new();
        for event in rx.iter() {
            match event {
                RunEvent::Line(text, _) => lines.push(text),
                RunEvent::Exited(code) => {
                    assert_eq!(code, Some(0));
                    break;
                }
                RunEvent::Failed(message) => panic!("{message}"),
            }
        }
        assert!(lines.contains(&hostile), "{lines:?}");
        assert!(!marker.exists());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-build-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn detects_monorepo_layout() {
        let root = temp_dir("mono");
        // iOS app: project with an embedded workspace that must not count,
        // plus a local Swift package.
        std::fs::create_dir_all(root.join("DmoteIOS/DMote.xcodeproj/project.xcworkspace")).unwrap();
        touch(&root.join("DmoteIOS/DMoteCoreIOS/Package.swift"), "// swift-tools-version:5.9");
        touch(&root.join("DmoteIOS/.build/checkouts/Dep/Package.swift"), "");
        // Android app with two modules and a nested standalone Gradle build.
        touch(&root.join("DMoteAndroid/gradlew"), "#!/bin/sh");
        touch(
            &root.join("DMoteAndroid/settings.gradle.kts"),
            "rootProject.name = \"D-Mote\"\ninclude(\":app\")\ninclude(\":lib\") // comment\n",
        );
        touch(&root.join("DMoteAndroid/app/build.gradle.kts"), "plugins { alias(libs.plugins.android.application) }");
        touch(&root.join("DMoteAndroid/lib/build.gradle.kts"), "plugins { id(\"com.android.library\") }");
        touch(&root.join("DMoteAndroid/Core/gradlew"), "");
        touch(&root.join("DMoteAndroid/Core/settings.gradle"), "include ':domain', ':cast'");
        touch(&root.join("DMoteAndroid/build/gradlew"), "");
        touch(&root.join("DMoteAndroid/build/settings.gradle"), "");

        let mut found = detect(&root);
        found.sort_by(|a, b| a.dir.cmp(&b.dir));
        let names: Vec<_> = found.iter().map(|p| p.label()).collect();
        assert_eq!(names, ["D-Mote (Gradle)", "Core (Gradle)", "DMote (Xcode)", "DMoteCoreIOS (SwiftPM)"]);
        match &found[0].kind {
            ProjectKind::Gradle { modules } => {
                assert_eq!(modules[0], GradleModule { path: "app".into(), kind: ModuleKind::AndroidApp });
                assert_eq!(modules[1], GradleModule { path: "lib".into(), kind: ModuleKind::AndroidLibrary });
            }
            other => panic!("{other:?}"),
        }
        match &found[1].kind {
            ProjectKind::Gradle { modules } => {
                assert_eq!(modules.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(), ["domain", "cast"]);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&found[2].kind, ProjectKind::Xcode { container: XcodeContainer::Project(_), .. }));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_wins_over_project_and_package() {
        let root = temp_dir("ws");
        std::fs::create_dir_all(root.join("App.xcodeproj")).unwrap();
        std::fs::create_dir_all(root.join("App.xcworkspace")).unwrap();
        touch(&root.join("Package.swift"), "");
        let found = detect(&root);
        assert_eq!(found.len(), 1);
        assert!(matches!(&found[0].kind, ProjectKind::Xcode { container: XcodeContainer::Workspace(_), .. }));
        assert_eq!(found[0].name, "App");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gradle_needs_wrapper_and_settings() {
        let root = temp_dir("gradle");
        touch(&root.join("settings.gradle.kts"), "include(\":app\")");
        assert!(detect(&root).is_empty());
        touch(&root.join("gradlew"), "");
        assert_eq!(detect(&root).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn depth_is_limited() {
        let root = temp_dir("deep");
        touch(&root.join("a/b/c/d/e/f/Package.swift"), "");
        assert!(detect(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_settings() {
        let text = "rootProject.name = \"DMoteCoreAndroid\"\ninclude(\":dmotecore-domain\")          // DMoteCoreDomain\ninclude(\":a\", \":b:c\")\n// include(\":ignored\")\nincludeBuild(\"x\")";
        assert_eq!(gradle_root_name(text).as_deref(), Some("DMoteCoreAndroid"));
        assert_eq!(gradle_includes(text), ["dmotecore-domain", "a", "b:c"]);
    }

    #[test]
    fn parses_scheme_lists() {
        let project = r#"{"project":{"configurations":["Debug"],"name":"DMote","schemes":["DMote","DMote_Test"],"targets":["DMote"]}}"#;
        assert_eq!(parse_scheme_list(project).unwrap(), ["DMote", "DMote_Test"]);
        let workspace = r#"{"workspace":{"name":"App","schemes":["App"]}}"#;
        assert_eq!(parse_scheme_list(workspace).unwrap(), ["App"]);
    }

    #[test]
    fn parses_booted_simulators() {
        let json = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-18-0":[{"state":"Booted","name":"iPhone 16","udid":"ABC-123","isAvailable":true},{"state":"Shutdown","name":"iPad","udid":"X"}]}}"#;
        assert_eq!(
            parse_simulators(json),
            [Destination { label: "iPhone 16".into(), spec: "platform=iOS Simulator,id=ABC-123".into() }]
        );
    }

    #[test]
    fn builds_commands() {
        let xcode = BuildProject {
            name: "DMote".into(),
            dir: "/r/ios".into(),
            kind: ProjectKind::Xcode {
                container: XcodeContainer::Project("/r/ios/DMote.xcodeproj".into()),
                schemes: vec!["DMote".into()],
            },
        };
        let cmd = command_for(&xcode, Some("DMote"), &Destination::generic_simulator(), BuildAction::Build).unwrap();
        assert_eq!(
            cmd.display(),
            "xcodebuild -project /r/ios/DMote.xcodeproj -scheme DMote -destination \"generic/platform=iOS Simulator\" build"
        );
        assert!(command_for(&xcode, Some("DMote"), &Destination::mac(), BuildAction::Install).is_none());

        let gradle = BuildProject {
            name: "D-Mote".into(),
            dir: "/r/android".into(),
            kind: ProjectKind::Gradle {
                modules: vec![
                    GradleModule { path: "app".into(), kind: ModuleKind::AndroidApp },
                    GradleModule { path: "lib".into(), kind: ModuleKind::AndroidLibrary },
                ],
            },
        };
        let dest = Destination::generic_simulator();
        let task = |unit, action| command_for(&gradle, unit, &dest, action).map(|c| c.args[1].clone());
        assert_eq!(task(Some("app"), BuildAction::Build).as_deref(), Some(":app:assembleDebug"));
        assert_eq!(task(Some("app"), BuildAction::Install).as_deref(), Some(":app:installDebug"));
        assert_eq!(task(Some("lib"), BuildAction::Install), None);
        assert_eq!(task(None, BuildAction::Clean).as_deref(), Some("clean"));
    }

    #[test]
    fn parses_swift_and_clang_lines() {
        let issue = parse_issue("/Users/me/DMote/RemoteSession.swift:42:17: error: cannot find 'foo' in scope").unwrap();
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.path.as_deref(), Some(Path::new("/Users/me/DMote/RemoteSession.swift")));
        assert_eq!((issue.line, issue.column), (42, 17));
        assert_eq!(issue.message, "cannot find 'foo' in scope");

        let warn = parse_issue("/Users/me/App/View.swift:7:9: warning: variable 'x' was never mutated; consider changing to 'let' constant").unwrap();
        assert_eq!(warn.severity, Severity::Warning);
        assert_eq!((warn.line, warn.column), (7, 9));

        let fatal = parse_issue("/Users/me/a.m:3:9: fatal error: 'Foo.h' file not found").unwrap();
        assert_eq!(fatal.severity, Severity::Error);
        assert_eq!(fatal.message, "'Foo.h' file not found");

        let javac = parse_issue("/Users/me/app/src/main/java/A.java:12: error: ';' expected").unwrap();
        assert_eq!((javac.line, javac.column), (12, 0));

        assert!(parse_issue("/Users/me/a.swift:3:9: note: did you mean 'bar'?").is_none());
        assert!(parse_issue("CompileSwift normal arm64 /Users/me/a.swift").is_none());
    }

    #[test]
    fn parses_kotlin_lines() {
        let issue = parse_issue("e: file:///Users/me/DMoteAndroid/app/src/main/java/com/dlabs/MainActivity.kt:23:5 Unresolved reference 'foo'.").unwrap();
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(
            issue.path.as_deref(),
            Some(Path::new("/Users/me/DMoteAndroid/app/src/main/java/com/dlabs/MainActivity.kt"))
        );
        assert_eq!((issue.line, issue.column), (23, 5));
        assert_eq!(issue.message, "Unresolved reference 'foo'.");

        let warn = parse_issue("w: file:///Users/me/a/B.kt:10:13 Parameter 'x' is never used").unwrap();
        assert_eq!(warn.severity, Severity::Warning);

        let old = parse_issue("e: /Users/me/a/B.kt: (10, 13): Type mismatch: inferred type is String but Int was expected").unwrap();
        assert_eq!((old.line, old.column), (10, 13));
        assert_eq!(old.message, "Type mismatch: inferred type is String but Int was expected");

        let bare = parse_issue("e: Could not load module <Error module>").unwrap();
        assert!(bare.path.is_none());
    }

    #[test]
    fn parses_bare_errors_and_ansi() {
        let issue = parse_issue("xcodebuild: error: Unable to find a destination matching the provided destination specifier:").unwrap();
        assert!(issue.path.is_none());
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(strip_ansi("\u{1b}[31me: boom\u{1b}[0m"), "e: boom");
        assert!(parse_issue("** BUILD FAILED **").is_none());
        assert!(parse_issue("BUILD SUCCESSFUL in 4s").is_none());
    }

    #[test]
    fn dedups_issues() {
        let mut set = IssueSet::default();
        let issue = parse_issue("/a.swift:1:1: error: x").unwrap();
        assert!(set.insert(&issue));
        assert!(!set.insert(&issue));
    }

    #[test]
    fn runs_and_streams() {
        let (tx, rx) = flume::unbounded();
        let cmd = CommandLine {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo '/x/a.swift:1:2: error: bad'; echo out; exit 3".into()],
            cwd: std::env::temp_dir(),
        };
        spawn(&cmd, tx).unwrap();
        let events: Vec<_> = rx.iter().collect();
        let issues = events.iter().filter(|e| matches!(e, RunEvent::Line(_, Some(_)))).count();
        assert_eq!(issues, 1);
        assert!(matches!(events.last(), Some(RunEvent::Exited(Some(3)))));
    }
}
