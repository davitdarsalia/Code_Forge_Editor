use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_AGENT: &str = "claude-code";
const CUSTOM_PREFIX: &str = "custom:";
const NPX: &str = "npx";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomAgent {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub via_npx: bool,
}

impl AgentSpec {
    pub fn command_line(&self) -> String {
        let program = self.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        std::iter::once(program).chain(self.args.iter().cloned()).collect::<Vec<_>>().join(" ")
    }
}

struct Builtin {
    id: &'static str,
    name: &'static str,
    binary: &'static str,
    binary_args: &'static [&'static str],
    package: Option<&'static str>,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        id: DEFAULT_AGENT,
        name: "Claude Code",
        binary: "claude-agent-acp",
        binary_args: &[],
        package: Some("@agentclientprotocol/claude-agent-acp@0.99.1"),
    },
    Builtin { id: "gemini", name: "Gemini CLI", binary: "gemini", binary_args: &["--acp"], package: Some("@google/gemini-cli@0.66.0") },
    Builtin {
        id: "codex",
        name: "Codex",
        binary: "codex-acp",
        binary_args: &[],
        package: Some("@agentclientprotocol/codex-acp@2.5.0"),
    },
    Builtin { id: "opencode", name: "OpenCode", binary: "opencode", binary_args: &["acp"], package: None },
    Builtin { id: "goose", name: "Goose", binary: "goose", binary_args: &["acp"], package: None },
];

pub fn custom_id(name: &str) -> String {
    format!("{CUSTOM_PREFIX}{name}")
}

fn resolve_command(command: &str, which: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let path = Path::new(command);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    if let Some(home) = command.strip_prefix("~/") {
        let full = dirs::home_dir()?.join(home);
        return full.is_file().then_some(full);
    }
    which(command)
}

pub fn detect(custom: &[CustomAgent], which: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<AgentSpec> {
    let npx = which(NPX);
    let mut out = Vec::new();
    for builtin in BUILTINS {
        let args = |extra: &[&str]| extra.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        if let Some(program) = which(builtin.binary) {
            out.push(AgentSpec {
                id: builtin.id.into(),
                name: builtin.name.into(),
                program,
                args: args(builtin.binary_args),
                env: BTreeMap::new(),
                via_npx: false,
            });
        } else if let (Some(package), Some(npx)) = (builtin.package, npx.as_ref()) {
            let mut all = vec!["-y".to_string(), package.to_string()];
            all.extend(args(builtin.binary_args));
            out.push(AgentSpec {
                id: builtin.id.into(),
                name: builtin.name.into(),
                program: npx.clone(),
                args: all,
                env: BTreeMap::new(),
                via_npx: true,
            });
        }
    }
    for agent in custom {
        let name = agent.name.trim();
        if name.is_empty() || out.iter().any(|spec| spec.name == name) {
            continue;
        }
        let Some(program) = resolve_command(agent.command.trim(), which) else { continue };
        out.push(AgentSpec {
            id: custom_id(name),
            name: name.into(),
            program,
            args: agent.args.clone(),
            env: agent.env.clone(),
            via_npx: false,
        });
    }
    out
}

pub fn select<'a>(agents: &'a [AgentSpec], preferred: &str) -> Option<&'a AgentSpec> {
    agents
        .iter()
        .find(|a| a.id == preferred)
        .or_else(|| agents.iter().find(|a| a.id == DEFAULT_AGENT))
        .or_else(|| agents.first())
}

pub fn search_path() -> std::ffi::OsString {
    std::env::join_paths(crate::tools::search_dirs()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(found: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |name| found.contains(&name).then(|| PathBuf::from("/opt/bin").join(name))
    }

    #[test]
    fn npx_makes_package_agents_available() {
        let agents = detect(&[], &fake(&["npx"]));
        let ids: Vec<_> = agents.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["claude-code", "gemini", "codex"]);
        let claude = &agents[0];
        assert!(claude.via_npx);
        assert_eq!(claude.program, PathBuf::from("/opt/bin/npx"));
        assert_eq!(claude.args, vec!["-y", "@agentclientprotocol/claude-agent-acp@0.99.1"]);
        assert_eq!(agents[1].args, vec!["-y", "@google/gemini-cli@0.66.0", "--acp"]);
    }

    #[test]
    fn installed_binaries_win_over_npx() {
        let agents = detect(&[], &fake(&["npx", "gemini", "goose"]));
        let gemini = agents.iter().find(|a| a.id == "gemini").unwrap();
        assert!(!gemini.via_npx);
        assert_eq!(gemini.args, vec!["--acp"]);
        assert_eq!(gemini.command_line(), "gemini --acp");
        let goose = agents.iter().find(|a| a.id == "goose").unwrap();
        assert_eq!(goose.args, vec!["acp"]);
        assert!(agents.iter().all(|a| a.id != "opencode"));
    }

    #[test]
    fn nothing_is_shown_without_commands() {
        assert!(detect(&[], &fake(&[])).is_empty());
    }

    #[test]
    fn custom_agents_need_a_resolvable_command_and_unique_name() {
        let custom = vec![
            CustomAgent { name: "Mine".into(), command: "my-agent".into(), args: vec!["--acp".into()], ..Default::default() },
            CustomAgent { name: "Missing".into(), command: "nope".into(), ..Default::default() },
            CustomAgent { name: "".into(), command: "my-agent".into(), ..Default::default() },
            CustomAgent { name: "Goose".into(), command: "my-agent".into(), ..Default::default() },
        ];
        let agents = detect(&custom, &fake(&["my-agent", "goose"]));
        let ids: Vec<_> = agents.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["goose", "custom:Mine"]);
        assert_eq!(agents[1].args, vec!["--acp"]);
    }

    #[test]
    fn selection_falls_back_to_claude_then_first() {
        let agents = detect(&[], &fake(&["npx", "goose"]));
        assert_eq!(select(&agents, "goose").unwrap().id, "goose");
        assert_eq!(select(&agents, "unknown").unwrap().id, "claude-code");
        let only_goose = detect(&[], &fake(&["goose"]));
        assert_eq!(select(&only_goose, "unknown").unwrap().id, "goose");
        assert!(select(&[], DEFAULT_AGENT).is_none());
    }
}
