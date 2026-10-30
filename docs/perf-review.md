# Forge performance review

Target: start like Sublime Text. Measured with `scripts/bench.sh <project> [runs] [binary]` (release build, `FORGE_TRACE_STARTUP=1`). Set `FORGE_BENCH_FILE=<file>` to also open a file at launch (5k/50k-line generators: see below).

## Caveat on numbers

The review machine had load average 35-60 (parallel release builds from other agents), so absolute timings below are inflated and noisy (same binary: open_paths 275 ms to 1170 ms between runs). Treat them as relative breakdowns; rerun `scripts/bench.sh` on an idle machine for the final table. Sublime Text is not installed (`/Applications/Sublime Text.app` absent), so no side-by-side was possible; published figures for Sublime are roughly 30-50 MB RSS and under 100 ms to first window.

## Measurements (release, 136-file repo, loaded machine)

| Metric | Value |
|---|---|
| Binary size | 61.3 MB (stripped, thin LTO) |
| RSS after 3 s settle | 88-97 MB |
| Idle CPU (10 s, `ps time` delta) | 0.0-0.7 % |
| main to `app.run` callback | 420-650 ms under load (dyld of frameworks plus platform init; WebKit no longer linked) |
| `gpui_kit::init` | ~130 ms under load |
| actions registered to `Workspace::new` | ~155 ms (window creation, Metal layer) |
| window opened to first frame drawn | ~35 ms |
| background index walk (136 files) | ~25 ms |
| index/git results applied | gated on window activation: 465 ms after the walk finished in one trace (walk done 1009 ms, "index ready" 1475 ms, "window active" 1481 ms) |
| terminal first shell output | ~700 ms after first frame (spawned after first frame, fine) |
| fuzzy, 200k synthetic paths, after parallelising | 21-38 ms (`cargo test bench_fuzzy_synthetic -- --ignored --nocapture`) |

Linked at load (`otool -L`): Metal, AppKit and friends. WebKit was linked via `wry` and is gone since the preview was removed; CloudKit and CoreData remain, pulled in by gpui (`objc2-app-kit`).

## Per-keystroke path (reasoning from gpui-base/gpui-component sources)

- Tree-sitter highlighting is incremental: edits are fed to `Tree::edit`, reparse uses a timeout with background fallback (`gpui-component/src/highlighter/highlighter.rs`, `input_adapter.rs`). No full reparse per key. Good.
- Forge side, `InputEvent::Change` in `workspace.rs` (`open_file` subscription): marks LSP stale, `schedule_save` (debounce timer, re-armed per key). Each calls `cx.notify()` on the whole `Workspace`, so every keystroke re-renders the full workspace view (tabs, tree, panels), not just the editor. This is the largest avoidable per-key cost.
- `save_doc` (after debounce): `editor.value()` copies the whole rope into a `String` and hashes it; on a 50k-line file that is a multi-MB copy plus hash. Now async per the UI-thread branch for the write, but the copy and hash still occur on the UI thread unless that branch moved them.

## Prioritized bottlenecks

| # | Bottleneck | Expected gain | Owner file | Proposal |
|---|---|---|---|---|
| 1 | Index/git results apply only after window activation (~450 ms in trace) | 300-450 ms to "index ready" | `workspace.rs` (activation observer, `reindex`/`apply_index` task), `main.rs` | Find why foreground tasks wait for activation (the `cx.spawn` result update appears to be starved until the app becomes active). Try `cx.activate(true)` after the window opens (tried; inconclusive on a loaded box, no change committed), or order `activate` so the window is key before spawning index work. Verify with the `window active=` mark vs `index ready`. |
| 2 | WebKit, CloudKit, CoreData linked at load via wry | 40-120 ms launch, ~10-15 MB RSS | `preview.rs`, `build.rs` | WebKit resolved by removing the preview and `wry`; CloudKit and CoreData still come from gpui `objc2-app-kit`. |
| 3 | Whole-Workspace `cx.notify()` per keystroke (`schedule_save`) | lower per-key frame cost on big windows | `workspace.rs` | Only notify when `dirty` flips false to true (tab dot) instead of on every Change. |
| 4 | Full-text copy and hash on save | removes multi-MB UI-thread copy per debounce on 50k-line files | `workspace.rs` | Hash from the rope chunks without building a `String`; or use the editor's change counter/version for dirty detection instead of content hash. |
| 5 | main to `app.run` and `gpui_kit::init` (~130 ms), window creation (~155 ms) | partly framework load (see #2); remainder GPUI/Metal | `main.rs` | Items to check on an idle machine with `sample`: font loading in `gpui_kit::init`, `languages::register()` (all grammars built eagerly, should be lazy per language, owner `languages.rs` / binary-size agent). |

Lower priority: idle CPU ~0.6 % is the GPUI display-link tick plus cursor blink; no Forge-side timers fire at idle (all `timer()` uses are debounces). Reactivation triggers a full re-walk (`reindex`, throttled to once per 2 s), which on large repos will cost CPU on every Cmd-Tab back; consider comparing directory mtimes or using FSEvents instead. Text search spawns `num_cpus` raw threads per query and does not skip huge files; a per-file size cap (e.g. 2 MB) would bound worst cases.

## Memory

RSS ~96 MB is mostly the 61 MB binary image plus Metal/GPUI atlases. Per-document cost is the rope plus a tree-sitter tree (roughly 10-30x source size for the tree on dense code); closing tabs drops both. The file index is a `Vec<String>` (about 100 bytes per path; 100k files is about 10 MB); a shared prefix table would halve it. git2 repo handle is opened in background tasks, not kept resident.

## What was changed on this branch

- `scripts/bench.sh`: repeatable launch benchmark (median first frame, open_paths, git ready, index ready, settled RSS, idle CPU, binary size).
- `src/search.rs`: `fuzzy` scores in parallel (up to 8 threads) once the index has 20k+ files; synthetic 200k-path bench added as an ignored test.
- Startup marks already cover every phase; none added.

## Test files

5k/50k-line Rust files can be generated with a short script (five-line functions repeated) and passed via `FORGE_BENCH_FILE`; file-open cost is visible as the delta in `first frame drawn`.
