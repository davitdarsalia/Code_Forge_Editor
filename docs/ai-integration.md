# Claude integration plan for Forge

Status: research only, nothing implemented. Researched 2026-10-29. Versions below come from
`cargo search`/`cargo info`/`npm view` on that date; re-check before adding anything.

Goal: put Claude into Forge without breaking the product promises (Sublime-fast startup, tiny RAM,
no blocking UI thread, no polling). The agent should cost nothing until the user opens it.

---

## 1. Options

### (a) Claude Code headless / Claude Agent SDK as a sidecar (stream-json over stdio)

How it works: spawn `claude -p` (or a small Node/Python program using the Agent SDK) and talk
newline-delimited JSON over stdin/stdout.

- CLI flags (from code.claude.com/docs/en/headless):
  `-p`, `--output-format text|json|stream-json`, `--input-format stream-json`, `--verbose`,
  `--include-partial-messages` (token deltas as `stream_event`), `--allowedTools "Read,Edit,Bash(git diff *)"`,
  `--permission-mode` (`acceptEdits`, `auto`, `dontAsk`, ...), `--permission-prompt-tool <mcp tool>`,
  `--permission-prompts none` (v2.1.259+), `--mcp-config <file-or-json>`, `--resume <session_id>`,
  `--continue`, `--append-system-prompt`, `--max-budget-usd`, `--bare` (skip hooks/plugins/CLAUDE.md
  discovery; API key only; recommended for scripted use and "will become the default for -p").
- Stream: first `system/init` (model, tools, `mcp_servers`, `capabilities`), then `assistant`/`user`
  messages, `system/api_retry`, `permission_denied`, last line `result` with `session_id`,
  `total_cost_usd` and per-model cost.
- Agent SDK packages: `@anthropic-ai/claude-agent-sdk` 0.3.312 (TS), `claude-agent-sdk` (Python).
  They wrap the same binary (current CLI `@anthropic-ai/claude-code` 2.1.312) and add a
  `canUseTool` permission callback, hooks, typed messages.
- Pros: full Claude Code harness (tools, subagents, compaction, CLAUDE.md, skills) for free; cost in
  `result`. Cons: we own a bespoke, unversioned-ish JSON schema; permission prompts need either
  an MCP permission tool or the SDK (= a Node/Python sidecar on top of the CLI); every UI event is
  ours to map. Node/Bun runtime required (~100-200 MB RSS per agent process, outside Forge).

### (b) Agent Client Protocol (ACP) — what Zed uses

ACP is JSON-RPC 2.0 between an editor (client) and an agent subprocess over stdio. Spec is versioned
`protocol/v1` (agentclientprotocol.com), v2 is a draft behind `unstable_protocol_v2`.

- Agent methods: `initialize`, `authenticate`, `session/new`, `session/load`, `session/prompt`,
  `session/cancel` (notification), `session/set_mode`.
- Client (editor) methods: `session/update` notifications (agent/user/thought message chunks,
  tool calls + updates, plans, available commands, mode changes, compaction notices),
  `session/request_permission`, `fs/read_text_file`, `fs/write_text_file`,
  `terminal/create|output|wait_for_exit|kill|release`, `elicitation/create`.
- `session/new` takes an `mcpServers` list, so the editor can hand its own MCP server to the agent.
- Claude adapter: `@agentclientprotocol/claude-agent-acp` **0.99.0** (renamed from
  `@zed-industries/claude-agent-acp` / `@zed-industries/claude-code-acp`, both deprecated). It wraps
  the Claude Agent SDK and vendors the Claude Code CLI. Zed launches it as a managed npm package
  with its own Node and always uses that managed copy, not a global `claude`.
- Rust: `agent-client-protocol` **3.6.0** (official rust-sdk repo, Apache-2.0, MSRV 1.88).
  Runtime-neutral ("the embedding runtime must drive the resulting connection future"), which fits
  GPUI's executor without tokio. Features: `process` (spawn agent: `AcpAgent`/`AcpAgentConfig`,
  uses async-process/async-io — both already in our Cargo.lock), `stdio`, `unstable_*`
  (`unstable_end_turn_token_usage` for usage per turn, `unstable_session_fork`, `unstable_subagents`,
  `unstable_mcp_over_acp`). Client is built with `Client.builder().connect_with(transport, ..)`;
  sessions via `cx.build_session_cwd()` → `ActiveSession::send_prompt`.
  Companion: `agent-client-protocol-rmcp` 4.0.6 (rmcp bridge; its major differs from the core crate,
  check compatibility when adding).
- Community alternative: `claude-code-acp-rs` 0.1.22 (Rust ACP agent wrapping Claude Code; default
  feature `bundled-cli`). Interesting to drop Node later; too young to depend on now.
- Pros: the agent UI contract (diffs, permissions, plans, terminals, file I/O through the editor)
  is standardised; Forge gets every ACP agent (Gemini CLI, Codex adapters, ...) for free; Zed proves
  the model. Cons: still needs Node for the adapter; cost is not first-class in v1 (only via unstable
  usage feature or adapter-specific metadata — verify).

### (c) Direct Messages API from Rust with our own tool loop

- No official Anthropic Rust SDK found. Crates: `anthropic` 0.0.8 (stale), `misanthropic`
  1.0.0-alpha.25, `async-anthropic` 0.6.0 (fork `async-llm` 0.11.1), `anthropic-sdk-rust` 0.1.1.
  All community; most lag current API shapes. Safer: `reqwest` 0.13.5 + `eventsource-stream`
  0.2.3 (or `reqwest-eventsource` 0.6.0) against `POST /v1/messages` with `stream: true`.
- Current API facts to respect (Oct 2026): models `claude-opus-5-5` ($4/$20 per MTok),
  `claude-sonnet-5-5` ($2/$10), `claude-haiku-5-5` ($0.10/$0.50); 1M context. Thinking:
  `{type:"adaptive"}` only; `budget_tokens` and (on Opus 5.5) `disabled` return 400; depth via
  `output_config.effort` (Opus 5.5 defaults to `medium`). Forced `tool_choice any/tool` returns 400
  on 5.5 models — use `auto` + `strict: true`. No assistant prefill. `temperature/top_p/top_k` 400
  from API version 2026-10-21. Handle `stop_reason: "refusal"`. Prompt caching (`cache_control`)
  is essential for a loop. Usage is in every response, so exact cost is trivial.
- Pros: zero sidecar, smallest RAM, total UI control, works offline of Node. Cons: we rebuild the
  harness (edit tool, bash sandboxing, permissions, compaction, context gathering, subagents) and
  chase API drift ourselves. Months of work to approach Claude Code quality.
- Good fit for small, latency-sensitive features only: inline "explain/rename/write commit message",
  quick-fix on a diagnostic. Use Haiku/Sonnet, one call, no tool loop.

### (d) Forge as an MCP server (editor context for Claude Code in the terminal)

Claude Code's IDE integrations (VS Code/JetBrains) run a local MCP server named `ide`:
loopback `ws://127.0.0.1:<random 10000-65535>`, a fresh token in `~/.claude/ide/<port>.lock`
(0600, dir 0700; `$CLAUDE_CONFIG_DIR/ide/` if set), presented as header
`X-Claude-Code-Ide-Authorization`. Model-visible tools: `getDiagnostics`, `executeCode`; the rest is
internal RPC (open diff, current selection, save file). The CLI attaches the selection + active file
to each prompt. The lock-file JSON fields and internal RPC names are **not documented**; emulating
`ide` means reverse-engineering and can break on any CLI release.

Documented, stable path instead: a normal MCP server registered with `claude mcp add`, `.mcp.json`
or `--mcp-config`, or passed in ACP `session/new.mcpServers`.

- Rust: `rmcp` **3.8.0** (official modelcontextprotocol/rust-sdk). Default features
  `base64, macros, server`; transports `transport-io` (stdio), `transport-streamable-http-server`
  (loopback HTTP). rmcp needs tokio (already transitive in our lock at 1.54.1) — run one
  `current_thread` tokio runtime on a dedicated thread, started only when the server is enabled.
- Tools Forge would expose: `forge_open_files`, `forge_active_editor` (path, cursor, selection text),
  `forge_diagnostics` (when we have LSP), `forge_open_file(path, line)`, `forge_show_diff(path, new_text)`
  (opens review UI, returns accepted/rejected), `forge_git_status`. Read-only by default.
- Pros: tiny, useful even with no agent panel (user runs `claude` in Forge's terminal). Cons: no
  chat UI in Forge by itself.

---

## 2. Recommendation

**Primary: ACP client in Forge hosting `@agentclientprotocol/claude-agent-acp`, plus an rmcp-based
Forge MCP server passed into each ACP session (and usable from terminal `claude`). Keep (c) only for
one-shot micro-features later.**

Why:
1. **RAM/startup**: the Node agent process is spawned lazily on first open of the Agent panel and
   killed on idle/close; Forge's own cost is a JSON-RPC connection and some views. Option (c) is
   lighter on paper but the harness we would have to write is the expensive part.
2. **Quality for free**: Claude Code's tools, compaction, CLAUDE.md, skills and subagents come with
   the adapter; we never chase Messages API drift (thinking/effort/tool_choice changes above show how
   fast it moves).
3. **Runtime fit**: `agent-client-protocol` is runtime-neutral and its `process` feature uses
   async-process/async-io already in our tree, so it runs on GPUI's `background_spawn` with no tokio.
4. **Editor-owned edits**: with `fs.writeTextFile` capability, the agent's writes come to Forge,
   which is exactly where inline diff review lives. Permission prompts arrive as
   `session/request_permission` — a typed request, not stream scraping.
5. **Portable**: the same panel hosts other ACP agents; the MCP server works for terminal Claude Code,
   Agent SDK sidecars and any MCP client.
6. **Why not (a)**: same Node dependency as ACP but an ad-hoc protocol and no standard permission /
   file-write channel; ACP's adapter is (a) already done.
7. **Why not emulate the `ide` lock-file server first**: undocumented; do it only as an optional
   experiment after the documented MCP server ships.

Constraints to honour:
- **Auth**: Anthropic's Agent SDK terms: "Unless previously approved, Anthropic does not allow third
  party developers to offer claude.ai login or rate limits for their products". Forge should default
  to the user's own `ANTHROPIC_API_KEY` (stored in macOS Keychain, never logged) or Bedrock/Vertex
  env. If the user already has a logged-in Claude Code, the adapter may pick it up, but Forge must not
  offer its own claude.ai login flow. Verify the adapter's auth behaviour before shipping.
- **Branding**: label it "Claude Agent" (or "Claude" inside an "Agents" menu). Not "Claude Code",
  no Claude Code-styled art.
- **Node**: detect `node` on PATH; if missing, show a notice with install hint. Install the adapter
  into `~/Library/Application Support/Forge/agents/` with a pinned version (`npm install --prefix`),
  never auto-update silently.

---

## 3. UX

**Agent panel (right sidebar)** — a new spring-animated panel next to the inspector, toggled with
`⌘⇧A` (proposed; check for conflicts). Contents: session list at top, transcript (`uniform_list` of
message/tool-call rows; thought chunks collapsed), prompt box at bottom with `@file` mentions (fuzzy
via existing nucleo index) and "attach selection". Shows the agent's plan (`session/update` plan
entries) as a checklist. Stop button = `session/cancel`.

**Inline diff review** — agent writes (`fs/write_text_file`) are not written to disk immediately:
Forge stages them as pending hunks (diff via `similar`, already transitive at 2.7.0; direct dep
`similar = "3.2"` later), shows red/green gutter hunks in the editor with Accept / Reject per hunk
and Accept all / Reject all in the panel, then uses the existing atomic write in `files.rs`.
A "Review" tab lists every changed file of the task (like the Changes navigator). Optional
"auto-accept edits" mode maps to the agent's `acceptEdits` mode.

**Per-task agents** — each task = one ACP session with its own cwd; the panel's session list is the
task list. One agent process can host multiple sessions; cap concurrently running turns (e.g. 3).
`session/load` restores past tasks after restart (if the adapter advertises `loadSession`).

**Background agents** — a task keeps running when the panel is closed; status bar shows a small
spinner + count; completion or a pending permission raises a workspace notice (no polling: driven by
`session/update` and `request_permission` messages). Optional: run a task in a git worktree
(`git worktree add` via `git.rs`) so its edits never touch the user's working copy until merged.

**Permission prompts** — `session/request_permission` renders an inline card in the transcript (tool
name, command or file, diff preview) with the options the agent sends (allow once / always / reject).
Background tasks badge the session and post a notice instead of stealing focus. Remember
"always allow" per project in Forge settings; never auto-allow Bash by default.

**Cost display** — per-turn and per-session tokens and USD in the panel header; daily total in the
status bar tooltip. Sources: ACP `unstable_end_turn_token_usage` or adapter metadata (verify which
the adapter sends); for (c) features compute from `usage` × price table (keep the table in one
constants module; Opus 5.5 $4/$20, Sonnet 5.5 $2/$10, Haiku 5.5 $0.10/$0.50 per MTok; cache reads
cheaper). Optional per-session budget that cancels the turn when exceeded. Model + effort picker
in the prompt box (ACP modes / `session/set_mode` where available).

**Editor context** — Forge MCP server provides open files, selection, diagnostics; "Ask Claude about
selection" (`⌘L` proposed) opens the panel with the selection attached.

---

## 4. Packages to add later (do NOT add now)

Rust (`Cargo.toml`):

| Crate | Version | Features | Phase | Note |
|---|---|---|---|---|
| `agent-client-protocol` | 3.6.0 | `process` (+ `unstable_end_turn_token_usage` if needed) | 1 | ACP client, runtime-neutral |
| `rmcp` | 3.8.0 | `server`, `macros`, `transport-io`, `transport-streamable-http-server` | 2 | Forge MCP server |
| `tokio` | 1.x (already transitive 1.54.1) | `rt`, `net`, `io-util` | 2 | only for rmcp, one current_thread runtime on its own thread |
| `similar` | 3.2.1 | default | 1 | hunk diffing for review UI |
| `schemars` | 1.x (already transitive 1.2.3) | — | 2 | MCP tool schemas (pulled by rmcp) |
| `security-framework` | latest | — | 1 | Keychain for API key (or reuse an existing keyring crate) |
| `agent-client-protocol-rmcp` | 4.0.6 | — | optional | only if we serve MCP over ACP; verify version pairing |
| `reqwest` | 0.13.5 | `json`, `stream`, `rustls` | 4 | direct API micro-features |
| `eventsource-stream` | 0.2.3 | — | 4 | SSE parsing for Messages streaming |

External (installed by Forge on demand, not bundled):

| Package | Version | Use |
|---|---|---|
| `@agentclientprotocol/claude-agent-acp` | 0.99.0 | ACP adapter (vendors Claude Code CLI) |
| Node.js | current LTS | adapter runtime |
| `@anthropic-ai/claude-code` | 2.1.312 | only for users running terminal `claude` against Forge's MCP server |
| `@anthropic-ai/claude-agent-sdk` | 0.3.312 | not needed with ACP; reference only |

---

## 5. Phased plan

**Phase 0 — spike (1-2 days, throwaway branch).** `agent-client-protocol` + `process`: spawn the
adapter, `initialize`, `session/new` (cwd = project root), `session/prompt`, print `session/update`
to stderr. Measure: Forge RSS delta, adapter RSS, time to first token. Confirm auth behaviour,
whether usage/cost arrives, and `loadSession` support.

**Phase 1 — Agent panel MVP.** New modules `agent/mod.rs` (connection + session state, all on
`background_spawn`), `agent/panel.rs` (UI), `agent/review.rs` (pending hunks). Lazy spawn, kill on
idle (e.g. 10 min, via executor timer started on last activity, not polling). Transcript, cancel,
permission cards, inline diff accept/reject via `fs/write_text_file`, API key in Keychain,
Node/adapter install flow with notices on failure. Tests: transcript reducer, hunk accept/reject,
permission-option mapping (pure logic, with a fake ACP agent over in-memory `Channel` transport).

**Phase 2 — Forge MCP server.** `mcp/server.rs` with rmcp: active editor/selection, open files,
git status, open file at line, show diff (review UI). Passed to ACP sessions via `mcpServers`; also
"Copy `claude mcp add` command" so terminal Claude Code can use it. Off by default; loopback only;
random token.

**Phase 3 — multi-task and background.** Session list, `session/load` resume, concurrent tasks,
optional per-task git worktrees, status bar indicator, notices, cost totals and budgets,
model/effort picker.

**Phase 4 — direct API micro-features (optional).** reqwest + SSE: commit message from staged diff,
explain selection, quick fix for a diagnostic; Haiku 5.5/Sonnet 5.5, prompt caching, `refusal`
handling. Only if the ACP path feels too heavy for one-liners.

**Phase 5 — experiments.** Emulate the `ide` lock-file server so a plain terminal `claude` auto-
connects; evaluate a Rust ACP agent (`claude-code-acp-rs`) to remove Node.

## Open questions / unverified
- Exact auth behaviour of `claude-agent-acp` 0.99 (API key vs existing Claude Code login) and whether
  using an existing subscription login is acceptable for Forge's distribution.
- Which usage/cost fields the adapter emits over ACP.
- `agent-client-protocol` 3.x client API details beyond docs.rs front page (builder/session names
  above are from that page; read the source before coding).
- Version pairing of `agent-client-protocol-rmcp` 4.0.6 with core 3.6.0.

Sources: code.claude.com/docs/en/headless, /ide-integrations, /agent-sdk/overview;
agentclientprotocol.com/protocol/overview; docs.rs/agent-client-protocol/3.6.0; zed.dev/docs/ai/external-agents;
zed.dev/blog/claude-code-via-acp; crates.io and npm registry lookups (2026-10-29).
