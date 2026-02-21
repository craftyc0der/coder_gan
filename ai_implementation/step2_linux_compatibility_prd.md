# PRD: Linux Cross-Platform Compatibility

## Problem Statement

The orchestrator currently uses **macOS-only** mechanisms to open visible terminal windows for agent tmux sessions. Specifically, it calls `osascript` (AppleScript) to open `Terminal.app` windows and track their window IDs for lifecycle management. On Linux, `osascript` and `Terminal.app` do not exist, so agents are spawned as invisible detached tmux sessions with no way for the user to observe them in a terminal window.

## Affected Code Locations

All references are in `orchestrator/src/`:

### 1. `injector.rs` — `spawn_session()` (lines 48–93)

- After creating the detached tmux session, runs an AppleScript via `osascript` to:
  - Open a `Terminal.app` window
  - Attach it to the tmux session
  - Return the window ID (`u32`)
- **Linux has no equivalent of `osascript` or `Terminal.app`.**

### 2. `injector.rs` — `close_terminal_window()` (lines 95–109)

- Runs an AppleScript to close a `Terminal.app` window by its ID.
- Called during shutdown and respawn to clean up old windows.

### 3. `supervisor.rs` — `AgentState.terminal_window_id` (line 62–66)

- Stores the `Option<u32>` window ID per agent.
- Serialized to `state.json` so `orchestrator stop` can close windows even after restart.

### 4. `supervisor.rs` — `kill_all()` (lines 337–347)

- Iterates agents and calls `close_terminal_window()` for each window ID.

### 5. `supervisor.rs` — respawn logic in `health_loop()` (lines 254–263)

- After respawning, closes the **old** Terminal.app window and stores the new window ID.

### 6. `main.rs` — `stop` command (lines 303–306)

- Reads `terminal_window_id` from `state.json` and calls `close_terminal_window()`.

## Proposed Approach

Replace the macOS-only `Terminal.app` / `osascript` mechanism with a **platform-adaptive terminal launcher** that works on both macOS and Linux.

### Strategy: Conditional compilation + pluggable terminal launcher

Use Rust's `#[cfg(target_os = ...)]` to select the appropriate implementation at compile time, with a shared trait/interface.

### Linux Terminal Window Approach

On Linux, open a new terminal emulator window running `tmux attach -t <session>`. Use a priority-ordered list of common terminal emulators:

1. Check the `$TERMINAL` environment variable first (user preference)
2. Then try in order: `ptyxis`, `gnome-terminal`, `konsole`, `xfce4-terminal`, `alacritty`, `kitty`, `xterm`
3. Use the first one found on `$PATH`

**Important: Many Linux terminal emulators fork/daemonize immediately** (notably `gnome-terminal`, often `konsole`, `xfce4-terminal`). The PID returned by `Command::spawn()` is typically a short-lived launcher process, not the actual window. This makes PID-based tracking unreliable for cleanup.

#### Mitigation: Use no-fork / wait flags

Each terminal emulator must be launched with flags that prevent daemonization so the child PID remains tied to the window lifetime:

| Emulator         | Flag                               |
| ---------------- | ---------------------------------- |
| `ptyxis`         | `-s`                               |
| `gnome-terminal` | `--wait`                           |
| `konsole`        | `--nofork`                         |
| `xfce4-terminal` | `--disable-server`                 |
| `alacritty`      | _(does not fork — no flag needed)_ |
| `kitty`          | _(does not fork — no flag needed)_ |
| `xterm`          | _(does not fork — no flag needed)_ |

If a terminal emulator is detected but its no-fork behavior cannot be guaranteed, treat the terminal handle as **untracked** (return `None`) and skip PID-based cleanup for that agent.

#### Display detection (headless fallback)

Before attempting to launch any terminal emulator, check for a graphical display:

1. Check `$DISPLAY` (X11) or `$WAYLAND_DISPLAY` (Wayland)
2. If neither is set, skip terminal window launch entirely and return `None`
3. If a display variable is set but the emulator launch **fails** (e.g. display unavailable), treat it as non-fatal — fall back to detached-only mode and return `None`

This ensures headless servers, SSH sessions without X-forwarding, and CI environments work without error.

If no terminal emulator is found or no display is available, fall back to detached-only mode (no visible window), matching current Linux behavior.

### Window ID → Process ID

- macOS: continue using `terminal_window_id: Option<u32>` (AppleScript window ID)
- Linux: track the terminal emulator child PID in-memory via `terminal_window_id: Option<u32>` for same-session cleanup

#### Critical: Do NOT persist Linux PIDs to `state.json`

PIDs are reused by the OS. If the orchestrator restarts and reads a stale PID from `state.json`, calling `kill()` on it could terminate an unrelated process. Therefore:

- **macOS**: continue persisting `terminal_window_id` to `state.json` (AppleScript window IDs are stable and scoped to Terminal.app)
- **Linux**: store the PID in the in-memory `AgentState` only; serialize as `None` to `state.json` via `#[serde(skip_serializing_if = ...)]` or a platform-conditional serialization strategy
- The `orchestrator stop` command on Linux will only kill tmux sessions (which is safe and sufficient — closing the tmux session will cause the attached terminal emulator window to exit on its own)

The `AgentState` struct and `state.json` schema should use a unified field (e.g., rename to `terminal_handle: Option<u32>`) or add a platform-specific field.

---

## Implementation Steps

### Step 1: Refactor `injector.rs` — Extract platform-specific terminal launching

[x] Create a new function `open_terminal_window(session: &str) -> Option<u32>` that encapsulates the "open a visible terminal attached to this tmux session" logic
[x] Create a new function `close_terminal_handle(handle: u32)` that encapsulates closing/killing the terminal
[x] Move the existing `osascript` logic into a `#[cfg(target_os = "macos")]` block inside these new functions
[x] Add a `#[cfg(target_os = "linux")]` block that:

- Checks for a graphical display (`$DISPLAY` or `$WAYLAND_DISPLAY`); returns `None` early if neither is set
- Detects an available terminal emulator (check `$TERMINAL`, then fallback list)
- Spawns it with **no-fork/wait flags** (e.g., `gnome-terminal --wait`, `konsole --nofork`) so the child PID stays tied to the window
- Returns the child PID as `Option<u32>`, or `None` if the emulator's no-fork behavior can't be guaranteed
- Treats any launch failure as non-fatal (returns `None`, agent still runs in detached tmux)
  [x] Add a `#[cfg(not(any(target_os = "macos", target_os = "linux")))]` fallback that returns `None`

### Step 2: Update `spawn_session()` in `injector.rs`

[x] Replace the inline AppleScript block with a call to `open_terminal_window(session)`
[x] Ensure the return type and semantics remain `Result<Option<u32>, InjectionError>`

### Step 3: Update `close_terminal_window()` in `injector.rs`

[x] Rename to `close_terminal_handle()` (or keep name but make it platform-aware)
[x] macOS path: existing AppleScript logic (unchanged)
[x] Linux path: `kill(pid, SIGTERM)` with a fallback to `SIGKILL` after a timeout
[x] Linux path: before killing, verify the PID still corresponds to a terminal emulator process (check `/proc/<pid>/cmdline`) to avoid killing unrelated processes
[x] Ensure it silently no-ops if the process is already dead

### Step 4: Update `supervisor.rs` — `AgentState`

[x] Rename `terminal_window_id` to `terminal_handle` (or keep both with `#[serde(alias)]` for backward compat with existing `state.json` files)
[x] On Linux: do **not** persist `terminal_handle` to `state.json` (PIDs are reused by the OS and stale PIDs could kill unrelated processes). Use `#[serde(skip_serializing_if = ...)]` or a platform-conditional serialization strategy so Linux always writes `None`
[x] On macOS: continue persisting the handle (AppleScript window IDs are stable)
[x] Update all reads/writes of this field in `spawn_agent()`, `health_loop()`, `kill_all()`, and `persist_state()`
[x] Update doc comments to reflect cross-platform semantics

### Step 5: Update `main.rs` — `stop` command

[x] Update the `state.json` deserialization to read the renamed/aliased field
[x] Call the new `close_terminal_handle()` instead of `close_terminal_window()`
[x] On Linux: the `stop` command should only kill tmux sessions (killing the tmux session causes the attached terminal emulator window to exit on its own); do not attempt PID-based cleanup from persisted state

### Step 6: Add a terminal emulator detection utility

[x] Create a helper function `detect_terminal_emulator() -> Option<(String, Vec<String>)>` that returns the command and args needed to open a terminal window running a given command
[x] Terminal-specific arg formats (with no-fork flags):

- `ptyxis -s -- tmux attach -t SESSION`
- `gnome-terminal --wait -- tmux attach -t SESSION`
- `konsole --nofork -e tmux attach -t SESSION`
- `xfce4-terminal --disable-server -e "tmux attach -t SESSION"`
- `alacritty -e tmux attach -t SESSION` _(does not fork)_
- `kitty tmux attach -t SESSION` _(does not fork)_
- `xterm -e tmux attach -t SESSION` _(does not fork)_
  [x] If `$TERMINAL` is set but is an unknown emulator, launch it with `-e` as a best-effort flag; if it fails, fall back to the next known emulator in the list instead of aborting
  [x] Cache the result so detection only runs once per orchestrator invocation

### Step 7: Update `InjectorOps` trait and `RealInjector`

[x] Update doc comments on `spawn_session()` to say "Returns the terminal handle" instead of "Terminal.app window ID"
[x] No signature changes needed (already returns `Option<u32>`)

### Step 8: Update tests

#### `orchestrator/tests/supervisor_tests.rs`

[x] Rename `terminal_window_ids` → `terminal_handles` (or equivalent) in mock injector
[x] Rename `set_terminal_window_id()` → `set_terminal_handle()` in mock
[x] Rename `set_terminal_window_id_queue()` → `set_terminal_handle_queue()` in mock
[x] Update `spawn_all_records_terminal_window_id_when_present` test name and assertions
[x] Update `spawn_all_omits_terminal_window_id_when_none` test name and assertions
[x] Update `kill_all_with_terminal_window_id_does_not_panic` test name and assertions
[x] Update `close_terminal_window_is_noop_and_does_not_panic` test — call new function name
[x] Update `kill_all_after_respawn_uses_updated_terminal_window_id` test name and assertions

#### `orchestrator/tests/spike_tests.rs`

[x] Update comment on line 56 referencing "Terminal.app"

#### `orchestrator/tests/watcher_tests.rs`

[x] Update comment on line 34 referencing "Terminal.app"

#### New tests to add

[x] Unit test for `detect_terminal_emulator()` — mock `$TERMINAL` env var, verify it's selected first
[x] Unit test for `detect_terminal_emulator()` — when `$TERMINAL` is set but fails to execute, verify fallback to next known emulator
[x] Unit test for `detect_terminal_emulator()` — when no terminal is found, returns `None`
[x] Unit test for display detection — when `$DISPLAY` and `$WAYLAND_DISPLAY` are both unset, returns `None` without attempting emulator launch
[x] Unit test for `close_terminal_handle()` with an already-dead PID — should not panic
[x] Unit test (Linux) for `close_terminal_handle()` — verify `/proc/<pid>/cmdline` is checked before killing (don't kill unrelated processes)
[x] Unit test to verify Linux terminal handles are **not** persisted to `state.json` (serialize as `None`)
[x] Integration test (Linux CI): `spawn_session()` on Linux returns `None` for terminal handle when no display is available (headless)

### Step 9: Update documentation and comments

[x] Update `CLAUDE.md` architecture section to mention cross-platform terminal support
[x] Update `README.md` if it references macOS or Terminal.app
[x] Update inline doc comments in `injector.rs`, `supervisor.rs`, and `main.rs`

### Step 10: CI / Build verification

[x] Verify `cargo build` succeeds on Linux (current dev machine)
[x] Verify `cargo test` passes on Linux
[x] Verify `cargo build` still succeeds on macOS (if available; otherwise note for manual verification)
[x] Verify `cargo clippy` produces no new warnings

---

## Completion Checklist

### Core functionality

[x] `spawn_session()` uses platform-appropriate terminal launcher
[x] `close_terminal_handle()` uses platform-appropriate cleanup
[x] Terminal emulator detection works on Linux (priority list)
[x] Linux emulators launched with no-fork/wait flags (`--wait`, `--nofork`, `--disable-server`)
[x] `$TERMINAL` env var is respected as first choice; failure falls back to known emulator list
[x] Display detection: `$DISPLAY` / `$WAYLAND_DISPLAY` checked before any emulator launch
[x] Graceful fallback when no terminal emulator is available (headless mode)
[x] Graceful fallback when emulator exists but launch fails (no display, permission error, etc.)
[x] `state.json` field renamed/aliased for cross-platform semantics
[x] Linux terminal PIDs are **not** persisted to `state.json` (in-memory only)
[x] Linux `close_terminal_handle()` verifies PID identity via `/proc/<pid>/cmdline` before killing

### Backward compatibility

[x] Existing macOS behavior is preserved (AppleScript path unchanged)
[x] Existing `state.json` files with `terminal_window_id` can still be read (serde alias)
[x] `InjectorOps` trait signature unchanged (`Option<u32>` return)

### Tests

[x] All existing tests pass on Linux
[x] All existing tests pass on macOS (or are `#[cfg]`-gated appropriately)
[x] New unit tests for terminal detection added
[x] New unit test for `$TERMINAL` fallback on failure added
[x] New unit test for display detection (headless → `None`) added
[x] New unit test for Linux close-handle (kill PID with identity verification) added
[x] New unit test verifying Linux handles not persisted to `state.json`
[x] Mock injector in test files updated to use new naming

### Documentation

[x] `CLAUDE.md` updated
[x] Inline code comments updated (no more "macOS only" where it's now cross-platform)
[x] `README.md` updated if applicable

### Build & CI

[x] `cargo build` passes on Linux
[x] `cargo test` passes on Linux
[x] `cargo clippy` clean on Linux
[x] No regressions on macOS (manual or CI verification)

---

## Out of Scope

- Windows support (can be added later with a `#[cfg(target_os = "windows")]` block using `cmd.exe` / Windows Terminal)
- Wayland-only environments without X11 (most terminal emulators still work under XWayland)
- Custom terminal emulator configuration beyond `$TERMINAL` env var
