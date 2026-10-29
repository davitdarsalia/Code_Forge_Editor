# Architecture Report - forge

* **Date:** 2026-10-29
* **Model:** claude-sonnet-5-5
* **PAAD version:** 2.0.2 (specialist/verifier passes done inline by one reviewer, not via subagents)
* **Commit:** ff3d0a0010f02c2e4361ad3c0762f4c985beb0bb
* **Languages:** Rust 2024 on GPUI (via `gpui-kit` 0.8.1), tree-sitter grammars, libgit2, small C grammar shims (`build.rs`)
* **Key directories:** `src/` (25 flat modules, 11.2k lines), `grammars/`, `assets/`, `macos/`, `docs/`
* **Scope:** full repo

## Repo Overview

Forge is a single-crate macOS code editor. `main.rs` (612 lines) registers actions/menus and opens windows. `workspace.rs` (2762 lines) is the root view. Around it sit self-contained feature modules: `build`/`build_panel` (Xcode/Gradle), `git` (libgit2 + CLI for network ops), `search`/`search_panel`, `quick_open`, `symbols`, `lsp` (hand-rolled JSON-RPC client), `format`, `terminal` (pty + vt100), `preview` (wry webview), `settings`, `index_cache`, `scheme_cache`. There is no `CLAUDE.md`/`AGENTS.md`; `docs/ai-integration.md` is a research note only. Only 31 `#[test]`s outside `workspace.rs`, 1 inside it.

## Strengths

### [S-1] Feature modules are mostly self-contained and testable without GPUI
- **Category:** S1 Clear modular boundaries / S11 Testability
- **Impact:** High
- **Explanation:** `build.rs` (858 lines, 15 tests), `git.rs`, `search.rs`, `format.rs`, `files.rs`, `index_cache.rs`, `scheme_cache.rs` have pure-Rust APIs and temp-dir fixtures. The UI wrappers (`build_panel`, `search_panel`, `quick_open`, `terminal`) are separate `Entity` views with their own state.
- **Evidence:** `src/build.rs:524` `pub fn spawn(command, tx: flume::Sender<RunEvent>)` returns events over a channel; `src/git.rs` 9 tests on real repos.
- **Found by:** Structure & Boundaries, Security & Code Quality

### [S-2] Data-loss protections on the save path
- **Category:** S7 Robust error handling
- **Impact:** High
- **Explanation:** Atomic temp+fsync+rename that preserves permissions and resolves symlinks; external-change detection using mtime then content hash; conflict state blocks autosave until the user forces; non-UTF-8/binary/oversize files refuse to open.
- **Evidence:** `src/files.rs:239-262` (`write_atomic`, `write_synced`), `src/workspace.rs:185-195` (`external_change`), `src/workspace.rs:564-585` (size/binary/UTF-8 guards).
- **Found by:** Error Handling, Security & Code Quality

### [S-3] Stale-result discipline in async index work
- **Category:** S12 Resilience / S14 Pragmatic abstractions
- **Impact:** Medium
- **Explanation:** Background results are dropped when the root changed, tasks are stored in owned `Option<Task>` slots (drop = cancel), search uses a shared cancel flag.
- **Evidence:** `src/workspace.rs:395-420` (`if this.root.as_ref() != Some(&project) { return; }`), `src/search.rs:200-208`.
- **Found by:** Coupling & Dependencies

### [S-4] Heavy work already mostly off the UI thread
- **Category:** S3 Loose coupling
- **Impact:** Medium
- **Explanation:** Index, git state, clone, format, symbols, build detection, scheme listing, settings save all use `cx.background_spawn`; LSP I/O, pty, build runner use dedicated threads with channels back to the UI.
- **Evidence:** `src/workspace.rs:402,450,476,1022,1186`; `src/format.rs:245-261`; `src/quick_open.rs:300`; `src/settings.rs:144`.
- **Found by:** Integration & Data

### [S-5] LSP client has a real error taxonomy and lifecycle management
- **Category:** S7 / S12
- **Impact:** Medium
- **Explanation:** `ServerError::{Busy, Failed}` maps ContentModified to a user-friendly "still indexing"; init timeout, idle shutdown reaper, stderr tail for diagnostics.
- **Evidence:** `src/lsp.rs:19-22` constants, `src/lsp.rs:~410-425` (`dispatch`), `src/workspace.rs:733-736`.
- **Found by:** Error Handling

### [S-6] Declarative command registry with a binding test; named constants for timings
- **Category:** S9 Configuration discipline
- **Impact:** Low
- **Evidence:** `src/commands.rs` (3 tests; registry vs bindings), `src/workspace.rs:39-45` named delays, `src/quick_open.rs:25`, `src/search_panel.rs:22`.
- **Found by:** Error Handling

### [S-7] Dependency hygiene and release profile
- **Category:** S5
- **Impact:** Low
- **Evidence:** `Cargo.toml`: libgit2 vendored without network features, `lto = "thin"`, `codegen-units = 1`, `strip`, `panic = "abort"`; focused crate list.
- **Found by:** Coupling & Dependencies

## Flaws/Risks

### [F-1] `Workspace` is a god object (state, orchestration and 40% of the file is rendering)
- **Category:** 2 God object / 11 Low cohesion
- **Impact:** High
- **Explanation:** One struct with ~55 fields owns window layout, documents, autosave, external-reload, navigation history, git state, clone dialog, indexing + index log, LSP manager, preview, terminal, notices. Every feature change lands here (shotgun surgery magnet) and it blocks parallel work (merge conflicts).
- **Evidence:** `src/workspace.rs:111-166` (struct), 2762 lines total, ~45 `render_*` and method clusters: clone dialog `:2455-2620`, changes panel `:1725-1910`, index status/log `:2186-2312`, welcome `:2348-2454`, git ops `:1005-1200`, nav history `:660-760`, doc lifecycle `:541-870`. Only one `#[test]` in the file.
- **Found by:** Structure & Boundaries, Coupling

### [F-2] Synchronous filesystem work on the UI thread
- **Category:** 12 Hidden side effects / 25 (logic in UI layer); violates the stated goal
- **Impact:** High
- **Explanation:** Despite S-4, several hot paths run blocking I/O (some with `fsync`) inside GPUI callbacks, so a slow disk, network mount or huge file freezes typing/rendering.
- **Evidence:**
  - `open_file` does `metadata` + `read` + hash + `detect_language` on the UI thread: `src/workspace.rs:564-588`.
  - `save_doc` hashes the full buffer, calls `external_change` (`metadata` + `read_to_string`), then `files::write_atomic` (canonicalize, create, `sync_all`, rename) all inline: `src/workspace.rs:826-860`; `schedule_save` (`:817-825`) fires it from a timer on the main thread; `save_all` loops every dirty doc on window deactivation (`:258`) and before every git op (`run_git`).
  - `reload_changed_documents` stats every doc and `read_to_string`s changed ones on window activation: `src/workspace.rs:875-888`, called at `:248`.
  - File tree `Node::load` is a blocking `read_dir` + sort, reached from `toggle` (`src/files.rs:78-83`), `reveal` (`:91-100`), `refresh` (`:59-70`, recursive over all expanded dirs) and called from `workspace.rs:640,991,1000,1293`.
  - `editor.read(cx).value()` copies the whole buffer each autosave (`:828`).
  - `lsp::Manager`/`Server` (`Rc`/`RefCell`/`Cell`, `src/lsp.rs:209-215`) are main-thread-only by construction, so any server start/IO there must be audited.
- **Found by:** Error Handling, Structure, Integration & Data
- **Parallel work:** being addressed by w2-ui-thread; re-review after merge.

### [F-3] Language servers are hard-wired to two languages via a closed enum and string matches
- **Category:** 7 / 13 Inconsistent boundaries; blocks the stated roadmap (many LSPs)
- **Impact:** High
- **Explanation:** Adding a language means editing `Kind`, five `match` arms, the extension map, the workspace notification text and the formatter table. There is no data-driven server spec (command, args, root markers, init options, language ids).
- **Evidence:** `src/lsp.rs:25-84` (`enum Kind { Swift, Rust }`, `for_language`, `server_name`, `language_id`, `marker`, `prefers_topmost`, `command`, `initialization_options` all `match self`); `src/workspace.rs:589` (`lsp::Kind::for_language(language)`), `:709` ("Go to Definition works in Swift and Rust files"); `src/lsp.rs:13` imports `crate::format` just to reuse `which`/`search_dirs`. Capability set is hard-coded to `definition` only (`:~270`).
- **Found by:** Coupling, Structure
- **Parallel work:** w2-diagnostics edits `lsp.rs`/workspace; sequence after it.

### [F-4] Language identity is a stringly-typed value spread across five modules
- **Category:** 13 Inconsistent boundaries / 28 Magic strings / 9 Shotgun surgery
- **Impact:** Medium
- **Explanation:** `"rust"`, `"swift"` etc. are bare `&'static str` mapped by extension in `files.rs`, grammars registered in `languages.rs`, formatter candidates in `format.rs`, LSP in `lsp.rs`, icons in `file_icons.rs`. No single `LanguageSpec` (id, extensions, shebangs, grammar, formatter, server, comment tokens). Syntax/theme-per-language and new LSPs will each touch all of these.
- **Evidence:** `src/files.rs:144-232` (`language_for`, `detect_language`, `"rs" => "rust"` `:161`, `"swift" => "swift"` `:162,:230`); `src/languages.rs:19-60` grammar vec built in code with `include_str!` queries; `src/format.rs:39-41` per-language tool table; `src/lsp.rs:42-52`.
- **Found by:** Structure, Coupling
- **Parallel work:** w2-syntax and w2-binsize touch `languages.rs`/`Cargo.toml`.

### [F-5] Inconsistent threading/async model with no central executor or cancellation
- **Category:** 6 Leaky abstractions / 34 Inconsistent conventions
- **Impact:** Medium
- **Explanation:** Three models coexist: GPUI `background_spawn`, ad-hoc `std::thread::spawn`, and dedicated threads with flume channels. Cancellation is per-site (drop task, `AtomicBool`, kill thread). `search` spawns `num_cpus` OS threads per query; `format` spawns 3 threads per run; each LSP server spawns 3 unbounded threads.
- **Evidence:** `src/search.rs:204-208` (thread per CPU per search), `src/format.rs:245,253,261`, `src/lsp.rs:245,256,290`, `src/terminal.rs:86`, `src/build.rs:503,556,559` vs `src/workspace.rs:402` etc.
- **Found by:** Coupling, Integration & Data

### [F-6] Async results lack ticket/epoch checks outside indexing; task slots are overloaded
- **Category:** 27 Temporal coupling / 26 Poor transactional boundaries
- **Impact:** Medium
- **Explanation:** The project's own rule ("async work that can go stale carries a ticket") is applied to the index but not git: a git refresh started for an old root can overwrite `repo`/`git_marks` after `open_project` cleared them. `git_task` is used both as a debounce timer and the refresh task; `git_busy`/`clone_busy` booleans stand in for a state machine.
- **Evidence:** `src/workspace.rs:1005-1035` (`schedule_git_refresh` and `refresh_git` both assign `git_task`; the `update` closure applies `state` with no root check, unlike `:417`), `:362` (`self.repo = None` on open_project), `:1186` clone.
- **Found by:** Coupling, Error Handling

### [F-7] No logging facade; errors often go to `eprintln!` or are discarded
- **Category:** 21 No observability / 34 Inconsistent error/logging / 20
- **Impact:** Medium
- **Explanation:** 42 `eprintln!` calls and ~20 `let _ =`/`.ok()` in `workspace.rs`; no levels, no log file, no structured diagnostics. Background failures (git refresh `.ok()` at `:1022`, tree read errors) surface nowhere for the user. Grows worse with many LSPs, each needing per-server logs.
- **Evidence:** `src/files.rs:47` (`eprintln!("forge: cannot read ...")`), `src/files.rs:247`, `src/workspace.rs:1022` (`git::read_state(&root).ok()`), `src/startup.rs` as the only instrumentation.
- **Found by:** Error Handling

### [F-8] Flat single crate: no enforced layering between logic and GPUI
- **Category:** 3 Tight coupling / 29 Utility dumping ground
- **Impact:** Medium
- **Explanation:** All 25 modules are siblings in one crate, so nothing stops logic modules importing gpui or each other. `format.rs` doubles as the tool-discovery utility (`which`, `search_dirs`, xcrun lookup) used by `lsp.rs`; the xcrun `--find` lookup is duplicated. Compile times and test isolation suffer, and the parallel branches all collide in `src/`.
- **Evidence:** `src/main.rs:3-25` flat `mod` list; `src/format.rs:139-165` (`search_dirs`, `which`, xcrun) vs `src/lsp.rs:84-92` (`xcode_sourcekit_lsp` duplicates the OnceLock + xcrun pattern); `src/lsp.rs:13`.
- **Found by:** Structure, Coupling

### [F-9] Critical editor behaviour is untested
- **Category:** 32 Missing tests
- **Impact:** Medium
- **Explanation:** Save/conflict/reload, navigation history, tab management, git result application have no tests because they live inside `Workspace` methods needing a GPUI context. `external_change` and `hash_text` are testable free fns but also untested. Test fixtures (temp dirs with `remove_dir_all(...).unwrap()`) are hand-rolled in 6 modules.
- **Evidence:** 1 test in `src/workspace.rs`; fixtures at `src/build.rs:642-719`, `src/git.rs:501-549`, `src/lsp.rs:621-671`, `src/scheme_cache.rs:138-172`, `src/search.rs:287-293`, `src/files.rs:274-283`.
- **Found by:** Security & Code Quality

### [F-10] Mixed global state
- **Category:** 1 Global mutable state
- **Impact:** Low
- **Evidence:** `src/icons.rs:10` `static SCALE_BITS: AtomicU32`, `src/settings.rs:74` `apply_statics`, `src/workspace.rs:1027` function-local `static GIT_READY`, `OnceLock` caches in `format.rs:156` / `lsp.rs:84` (tool paths never re-detected after install).
- **Found by:** Structure

### [F-11] No architecture/steering docs; stale-truth risk for contributors and agents
- **Category:** (documentation gap)
- **Impact:** Low
- **Evidence:** no `CLAUDE.md`/`AGENTS.md`/ADRs in repo root; `docs/ai-integration.md` is research only.
- **Found by:** Structure

## Coverage Checklist

Statuses count this report's findings only. No security-sensitive findings were routed to `paad/security/`.

### Flaw/Risk Types 1-34
| # | Type | Status | Finding |
|---|------|--------|---------|
| 1 | Global mutable state | Observed | F-10 |
| 2 | God object | Observed | F-1 |
| 3 | Tight coupling | Observed | F-8 |
| 4 | High/unstable deps | Not observed | - |
| 5 | Circular deps | Not observed | - |
| 6 | Leaky abstractions | Observed | F-5 |
| 7 | Over-abstraction | Not observed | - |
| 8 | Premature optimization | Not observed | - |
| 9 | Shotgun surgery | Observed | F-4 |
| 10 | Feature envy / anemic | Not observed | - |
| 11 | Low cohesion | Observed | F-1 |
| 12 | Hidden side effects | Observed | F-2 |
| 13 | Inconsistent boundaries | Observed | F-3, F-4 |
| 14-19 | Distributed-system types | Not applicable (single-process desktop app) | - |
| 20 | Weak error handling | Observed | F-7 |
| 21 | No observability plan | Observed | F-7 |
| 22 | Configuration sprawl | Not observed | - |
| 23 | DI misuse | Not observed | - |
| 24 | Inconsistent API contracts | Not applicable | - |
| 25 | Business logic in UI | Observed | F-2 |
| 26 | Poor transactional boundaries | Observed | F-6 |
| 27 | Temporal coupling | Observed | F-6 |
| 28 | Magic numbers/strings | Observed | F-4 |
| 29 | Utility dumping ground | Observed | F-8 |
| 30 | Security as afterthought | Not observed | - |
| 31 | Dead code / unused deps | Not assessed | - |
| 32 | Missing test coverage | Observed | F-9 |
| 33 | Hard-coded credentials | Not observed | - |
| 34 | Inconsistent error/logging | Observed | F-5, F-7 |

### Strength Categories S1-S14
| # | Category | Status | Finding |
|---|----------|--------|---------|
| S1 | Clear modular boundaries | Observed (feature modules) | S-1 |
| S2 | High cohesion | Observed (feature modules, not Workspace) | S-1 |
| S3 | Loose coupling | Observed | S-4 |
| S4 | Stable dependency direction | Not observed (flat crate, unenforced) | - |
| S5 | Dependency hygiene | Observed | S-7 |
| S6 | Consistent API contracts | Not applicable | - |
| S7 | Robust error handling | Observed | S-2, S-5 |
| S8 | Observability present | Not observed (startup marks only) | - |
| S9 | Configuration discipline | Observed | S-6 |
| S10 | Security built-in | Not assessed | - |
| S11 | Testability & coverage | Observed (feature modules) | S-1 |
| S12 | Resilience patterns | Observed | S-3, S-5 |
| S13 | Domain modeling | Not observed | - |
| S14 | Pragmatic abstractions | Observed | S-3 |

## Hotspots
1. `src/workspace.rs` - god object, UI-thread I/O on save/open/reload, the merge-conflict centre for every parallel branch.
2. `src/lsp.rs` + `src/files.rs` + `src/languages.rs` + `src/format.rs` - the language/LSP extension axis; closed enums and string matches.
3. `src/files.rs` `Node::load`/`write_atomic` - blocking tree reads and fsync reached from UI callbacks.

## Next Questions
1. Where is `Manager::server_for`/`Server::start` invoked (UI thread or background), and does `xcrun --find` or the 60s init ever block a frame?
2. What should the per-language contract be (grammar, theme scope, formatter, server, markers) and who owns it?
3. How large can a buffer get before per-keystroke autosave hashing/copying is noticeable (MAX_FILE_BYTES)?
4. Is multi-window state (several `Workspace`s) sharing LSP servers or one `Manager` each?
5. Does the tree need to be watcher-driven rather than rescanned on window activation?

## Fix Plan (requested addition; incremental, each step independently mergeable)

Sequencing assumes the parallel branches (w2-ui-thread, w2-diagnostics, w2-syntax, w2-binsize, w2-perf) merge first. Conflict risk is relative to those branches.

1. **Re-baseline after merges** (no code). Re-run line references; confirm F-2 items the ui-thread branch already fixed.
2. **Extract `tools.rs` (which/search_dirs/xcrun lookup) out of `format.rs`** - mechanical move, dedupes `lsp.rs:84`. Conflict: low-medium (format.rs/lsp.rs, diagnostics branch). Fixes F-8 part, prerequisite for step 4.
3. **Pure `document.rs`**: move `Document`, `hash_text`, `mtime`, `external_change`, nav-history stack with unit tests. Conflict: HIGH with w2-ui-thread (same functions); do only after it merges. Addresses F-1/F-9.
4. **`LanguageSpec` registry** (`LanguageId` enum or interned id; extensions, shebangs, grammar, formatter ref, server ref) replacing `files::language_for`/`detect_language` string matches. Conflict: HIGH with w2-syntax; do after. Addresses F-4.
5. **Data-driven `ServerSpec`** (command, args, markers, topmost, init options, language ids) replacing `lsp::Kind` matches; generic notification text. Conflict: HIGH with w2-diagnostics; do after. Addresses F-3.
6. **Epoch/ticket helper** (`TaskSlot` with generation + root check) applied to git refresh, clone, preview, definition; replace `git_busy`/`clone_busy` flags. Conflict: medium (workspace.rs). Addresses F-6.
7. **Split `Workspace` into child entities, one per PR**, in this order of lowest coupling: (a) `CloneDialog`, (b) `ChangesPanel` (git state + commit input), (c) `IndexStatus`/log, (d) `Welcome`, (e) `DocumentStore` (open/save/reload/autosave), (f) layout/panel sizes. Each emits events to Workspace. Conflict: HIGH for e, medium for others; run alone, not in parallel with other workspace.rs edits. Addresses F-1.
8. **Logging facade** (`log` + a file/stderr sink, per-server LSP log channel); replace `eprintln!` and `.ok()` on background failures. Conflict: low (many one-line edits; do last in a quiet window). Addresses F-7.
9. **Shared bounded executor / thread budget** for search, format, LSP I/O; document the one allowed threading pattern. Conflict: medium with w2-perf (search.rs). Addresses F-5.
10. **Shared test-fixture helper** (`TempDir` RAII) and tests for save/conflict/reload via the extracted document module. Conflict: low. Addresses F-9.
11. **Optional last:** split into workspace crates (`forge-core`: files, git, search, build, format, lsp, index; `forge`: GPUI views) to enforce dependency direction. Conflict: very high; only when no branches are in flight.
12. Add a short `CLAUDE.md` with module map, threading rules (no blocking I/O in UI callbacks) and the language-add checklist. Conflict: none.

## Analysis Metadata

- **Agents dispatched:** none (single reviewer covered Structure, Coupling, Integration/Data, Error Handling, Security/Quality inline; no subagents)
- **Scope:** all of `src/` (25 files, 11,197 lines), `Cargo.toml`, `docs/`; git log of last 15 commits for refactor context
- **Raw findings:** 18
- **Verified findings:** 18 (each re-read at the cited lines; not independently verified by a separate agent)
- **Filtered out:** 0
- **By impact:** 2 high, 6 medium, 3 low (flaws); 7 strengths
- **Steering files consulted:** none found
