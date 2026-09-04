# AGENTS.md
Guidance for agentic coding assistants working in this repository.

## 1) Scope and priorities
- Project: `rift-wm` (Rust 2024), a macOS tiling window manager.
- Binaries: `rift` and `rift-cli`.
- Architecture: actor mesh + reactor + layout engine + macOS system bindings.
- Keep diffs small, behavior-aware, and aligned with existing patterns.
- Prefer extending existing modules over introducing parallel abstractions.

## 2) Cursor/Copilot rule files
- `.cursor/rules/`: not present.
- `.cursorrules`: not present.
- `.github/copilot-instructions.md`: not present.
- No extra repository rule files exist today; use this file as the agent guide.

## 3) Toolchain and CI baseline
- CI runs on `macos-14`.
- CI format check: `cargo fmt --all --check --verbose` on the stable toolchain (`.github/workflows/rust.yml`); `rustfmt.toml`'s unstable options are silently ignored there, so a nightly-only formatting must still satisfy stable.
- CI compile check: `cargo check --locked`.
- CI tests: `cargo nextest run`.
- `build.rs` links macOS frameworks, so non-macOS builds may fail.

## 4) Build / format / lint commands
Run commands from repo root.

### Recommended local gate
```bash
cargo fmt --all --check --verbose
cargo check --locked
cargo nextest run
```

### Build
```bash
cargo check --locked
cargo build --bins
cargo build --release --locked --bins
```

### Run binaries
```bash
cargo run --bin rift -- --help
cargo run --bin rift-cli -- --help
```

### Format and lint
```bash
cargo fmt --all --check --verbose
cargo fmt --all
cargo clippy --workspace --all-targets
```

## 5) Test commands (single-test focused)

### Full suites
```bash
cargo nextest run
cargo test
```

### Single test
```bash
cargo nextest run it_ignores_stale_resize_events
cargo test actor::reactor::tests::it_ignores_stale_resize_events -- --exact --nocapture
```

### Single module
```bash
cargo test actor::reactor::tests -- --nocapture
cargo test layout_engine::engine::tests -- --nocapture
```

### Discover tests
```bash
cargo nextest list
cargo test -- --list
cargo test -- --list | rg reactor::tests
```

### Debugging
```bash
RUST_LOG=debug cargo nextest run <filter>
RUST_LOG=trace cargo test <filter> -- --nocapture
```

## 6) Architecture edit map
- Entrypoints: `src/bin/rift.rs`, `src/bin/rift-cli.rs`.
- Reactor core: `src/actor/reactor.rs`, event handlers in `src/actor/reactor/events/`.
- Layout engine: `src/layout_engine/engine.rs`, systems in `src/layout_engine/systems/`.
- Config model/parsing: `src/common/config.rs`, defaults in `rift.default.toml`.
- IPC protocol/plumbing: `src/ipc/protocol.rs`, `src/ipc.rs`.
- System/FFI integration: `src/sys/*`.

## 7) Formatting and import conventions
- Respect `rustfmt.toml`; do not hand-format against tool output.
- Imports are grouped/configured by rustfmt (`StdExternalCrate`, module granularity).
- Preferred order: `std/core` -> external crates -> `crate::...`.
- Avoid wildcard imports unless already idiomatic in local tests/modules.
- Alias conflicting names explicitly for readability.

## 8) Naming and types
- `PascalCase`: structs/enums/traits.
- `snake_case`: functions/modules/variables/tests.
- `UPPER_SNAKE_CASE`: constants/statics.
- Prefer domain wrappers (`SpaceId`, `WindowId`, `WindowServerId`) over raw integers.
- Keep serialized command/config names in `snake_case` for compatibility.
- Reuse existing command/config enums before creating new parallel types.

## 9) Error handling and logging
- Avoid panic paths in normal runtime behavior.
- Use explicit `Result` surfaces (`io::Result`, typed domain errors, etc.).
- Follow existing patterns:
  - `anyhow::Result` and `anyhow::bail!` for parsing/validation.
  - `thiserror::Error` for domain-specific error enums.
  - `map_err(...)` for clear user/operator diagnostics.
- `unwrap`/`expect` are acceptable in tests and hard startup invariants.
- Use `tracing` for internal diagnostics.
- Keep `println!`/`eprintln!` mostly for CLI output boundaries.

## 10) Unsafe / FFI rules
- Minimize unsafe surface area; keep unsafe blocks tight.
- Add `// SAFETY:` comments for non-obvious invariants.
- Preserve ABI-critical attrs/signatures (`#[repr(C)]`, extern layouts).
- Do not casually remove existing `#[allow(...)]` guards in FFI-heavy files.
- After FFI edits, run targeted tests plus `cargo check --locked`.

## 11) Change patterns to follow
When adding a CLI/runtime command, update applicable layers:
1. CLI parse/map in `src/bin/rift-cli.rs`.
2. Protocol request/response types in `src/ipc/protocol.rs` (if wire shape changes).
3. Reactor/model command enums and handlers.
4. Config layer if behavior is configurable.
5. Tests for mapping + behavior + regressions.

When adding a config field, update:
1. Structs/defaults/serde in `src/common/config.rs`.
2. Docs/defaults in `rift.default.toml`.
3. Runtime reload/apply consumers.
4. Validation tests (positive + negative paths).

## 12) Testing and delivery expectations
- Add tests near changed logic (`#[cfg(test)]` modules).
- Prefer behavior/regression assertions over implementation-detail checks.
- For parser/config changes, include negative-path tests.
- For layout/reactor changes, assert visible behavior/state transitions.
- Before finalizing: run format check, compile check, and targeted tests.
- If anything was not run, state exactly what and why.

## 13) Current multi-display handoff context
- There is in-progress, not-yet-working work in the tree for display-owned virtual workspaces. Treat the worktree as intentional and incomplete; do not revert it blindly.
- User-reported current state: after building and running, dragging a window from one display to another still did not work, and the new workspace implementation is considered not working by the user.
- The attempted implementation touched:
  - `src/model/virtual_workspace.rs`
  - `src/layout_engine/engine.rs`
  - `src/actor/reactor.rs`
  - `src/actor/reactor/events/command.rs`
  - `src/actor/reactor/query.rs`
  - `src/actor/wm_controller.rs`
  - `src/common/config.rs`
  - `src/bin/rift-cli.rs`
  - `src/ipc.rs`
  - `src/ipc/protocol.rs`
  - `src/model/reactor.rs`
  - `src/model/server.rs`
- New concepts already added in code:
  - `virtual_workspaces.ownership = "space" | "display"`
  - explicit assignment tracking intended to stop app-rule snap-back
  - display-targeted app rules via `display_uuid`
  - display-scoped workspace commands through reactor/CLI
- Important caution: code compiles and some focused unit tests pass, but that does not mean runtime behavior is correct. Prioritize real runtime semantics over preserving the current partial design.
- Likely failure area for drag behavior:
  - drag/drop path in `src/actor/reactor/events/window.rs` and `src/actor/reactor/events/drag.rs`
  - drag finalization / space resolution in `src/actor/reactor.rs`
  - divergence between physical drag behavior and explicit `move_window_to_display` behavior
- Likely failure area for workspace semantics:
  - current implementation may still be too `SpaceId`-centric even when display-owned mode is enabled
  - command/query surfaces may exist without the runtime model being coherent
- Live user config was also edited during testing:
  - `/Users/sami/.config/rift/config.toml`
  - it was switched to `ownership = "display"`
  - temporary test keybinds were added for display move/workspace testing
  - if behavior seems confusing, inspect that live config first before assuming default config behavior
- If continuing this feature, first reproduce manually in the running app, then trace actual runtime state transitions with logs before making more structural edits.

## 14) Rift relaunch — animation latch fix (rift-ship-01, 2026-09-02)
- Symptom: after a relaunch, the next tab move snapped instantly instead of tweening. Three mechanisms combine to produce it:
  1. `window.frame_monotonic` is set to `target_frame` when an animation is *created*, and `AnimationManager::animate_layout` skips any later layout whose target `same_as` it (`src/actor/reactor/animation.rs:258`).
  2. `WindowTxStore::insert` records the same target, and `animate_layout` skips again when `TransactionManager::get_target_frame` matches (`src/actor/reactor/transaction_manager.rs:70`, `src/model/tx_store.rs:25`).
  3. The app actor coalesces `Request::AnimationFrame` into `pending_frames: HashMap<WindowId, PendingFrame>` and writes at most one frame per window per drained batch (`State::handle_request_batch` at `src/actor/app.rs:575`, `flush_all_frames` at `:644`).
- Landed in this ship: two bounded guards. (a) App actor: `Request::AnimationFrame` flushes a pending frame whose `txid` differs, so a mid-flight replacement cannot have its last write dropped — frames within one animation (same txid) still coalesce, preserving backpressure. (b) Reactor: one-frame bool guard `Reactor::suppress_next_redundant_animation_check` (src/actor/reactor.rs, src/actor/reactor/managers.rs, src/actor/reactor/animation.rs) armed at startup (relaunch) and consumed by the first `apply_layout` batch that actually changes a frame (a startup layout that writes nothing does not burn it), bypassing `same_as`/`get_target_frame` once so the post-relaunch tween cannot be latched instant. See inline `ponytail:` / `rift-ship-01:` comments for ceiling.
- Verification for this ship: (a) trigger a layout change that replaces an in-flight animation for the same window — app actor should emit two writes with different `txid`s rather than one; (b) after a relaunch, a tab drag that previously latched instant should deliver ~30 `AnimationFrame` ticks at ~10 ms rather than a single `anim.none` snap (default `animation_duration` 0.3 × `animation_fps` 100.0, `src/common/config.rs`).
- Still open — start Ship 2 here if needed. The optimism in (1) and (2) is normally self-correcting via `EndWindowAnimation` → `WindowFrameChanged(.., Requested(true), ..)` → `classify_window_frame_change` (src/actor/reactor/events/window.rs) which reassigns `frame_monotonic` to the AX read-back frame and clears the tx target. Latch persists only when that reconciliation does not run (e.g., `Animation::carry_over` dropping a window listed in `handled_windows` without `end()`, or tx target surviving when read-back `!same_as` target) — confirm which fires before changing predicates further.
- Ponytail ceiling: bool + HashMap guards only; upgrade to per-window `VecDeque`/timer queue or generation counter only if measured coalescing persists.
- Deferred (follow-up ships, do not fix here):
  - Narrow `is_window_visible` gate in `Reactor::remove_windows_missing_from_active_space_snapshot` (src/actor/reactor.rs:693) still evicts windows from the layout on a partial WindowServer snapshot. The membership loss that made that eviction destructive is fixed: removal stashes the last live assignment (`WindowStore::last_workspace`) and re-admission prefers it over the active workspace (`WorkspaceStore::remembered_workspace_assignment`, regression tests in `src/model/virtual_workspace.rs`).
  - Ghostty rekey fallback in `src/actor/reactor/events/window_discovery.rs:518` (`same_pid.len() == 1`) plus unrestricted `focused_window()` at `:480` and `:492` feeding `reconcile_restored_window` (src/layout_engine/engine/persistence/reconcile.rs:201).
- The originating investigation write-up (`data/rift-relaunch/report.md`) is not committed to this repo; everything needed for follow-up ships is inlined above rather than cited by section number.
