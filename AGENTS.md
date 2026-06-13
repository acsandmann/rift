# Agent Instructions for Rift

Guidance for AI agents working in this repository. Rift is a tiling window manager for macOS written in Rust, focusing on performance, usability, and leveraging private APIs.

## Core Commands

Always run formatting and verify tests pass before reporting work as complete.

```bash
# 1. Format code using the nightly toolchain (configured in rustfmt.toml)
cargo +nightly fmt --all

# 2. Run all unit and integration tests
cargo test

# 3. Run a specific test
cargo test <test_name_pattern>

# 4. Check formatting without modifying files
cargo +nightly fmt --all --check

# 5. Build the project
cargo build
```

## Local Git Hooks

Rift uses local Git hooks stored in the repository. Once cloned, you should configure your local git client to use them:
```bash
git config core.hooksPath .githooks
```
- **Pre-commit:** Automatically runs `cargo +nightly fmt --all` and stages any formatted changes.
- **Pre-push:** Automatically runs `cargo test` and prevents pushing if any tests fail.

## Project Rules

- **Strict Formatting:** Code must be formatted using the nightly Rust toolchain: `cargo +nightly fmt --all`. Settings are defined in `rustfmt.toml`.
- **Testing:** Always write unit tests for new behavior or bug fixes. Tests are usually co-located in the module file inside a `mod tests` block.
- **Private macOS APIs:** Rift interacts with undocumented private macOS APIs (such as Skylight, SkyLight, SLS, etc.) to control spaces, displays, and windows. Be extremely cautious when modifying these bindings.
- **Error Handling:** Prefer `anyhow` for applications/CLI errors, and clean `Result` propagation inside the layout engine core.

## Commit Messages

Angular format `type(scope): description`.
* **Types:** `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`.
* **Examples:**
  - `feat(layout): add master-stack layout custom settings`
  - `fix(hotkey): handle keycode translation on ISO keyboards`
  - `ci(githooks): add pre-commit and pre-push hooks`
* **Rule:** **Never** use `fix(test):` or `fix(e2e):` — test changes must use `test:` type.

## Anti-patterns

| Avoid | Do instead |
| --- | --- |
| Running `cargo fmt` without `+nightly` | Use `cargo +nightly fmt` to support custom formatting rules |
| Committing directly to `main` without formatting | Ensure the pre-commit hook is active locally via `git config core.hooksPath .githooks` |
| Over-relying on public Cocoa APIs for workspace management | Leverage the custom private API bindings under `src/sys/` |
