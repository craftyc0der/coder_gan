🤖 Coder GAN: Generative Adversarial Network for Software Development

A multi-agent orchestration framework inspired by GANs. Like a generative adversarial network, competing agents drive quality:

• Generator (Coder): Writes implementation code
• Discriminator (Tester): Validates and challenges the implementation
• Moderator (Reviewer): Resolves disputes and ensures quality

🔒 Architectural Integrity: Each agent has restricted file system access—coders can't peek at tests, testers can't see implementation details before review. This prevents "cheating" and forces genuine problem-solving, mimicking real exam conditions where students can't see the answer key.

🦀 Built with Rust: Generic orchestrator works with any project. Agents communicate via filesystem message queues and run in isolated tmux sessions.

Perfect for: Autonomous coding experiments, multi-agent AI research, adversarial code quality systems.

## Project Overview

**coder_gan** is a generic multi-agent coding orchestration tool. A Rust orchestrator can be pointed at **any project directory**, launches and supervises configurable autonomous coding agents, enables agent-to-agent communication via filesystem message queues, and injects messages into live interactive terminal sessions using tmux.

Each project defines its own agent roles, CLI commands, and startup prompts via a `.orchestrator/` configuration directory.

See `ai_implementation/step1_brainstorming.md` for the full PRD.

## Repository Layout

```
coder_gan/
├── orchestrator/         # Rust orchestrator (the generic tool)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs       # CLI entry point (init, spike, run, status, stop)
│       ├── config.rs     # Project config loading, TOML parsing, path resolution
│       ├── spike.rs      # tmux spike for de-risking injection
│       ├── injector.rs   # tmux inject/capture with retry logic
│       ├── logger.rs     # Structured JSON line event logger
│       ├── supervisor.rs # Agent spawning, health loop, state persistence
│       └── watcher.rs    # Filesystem message watcher with routing & dedup
└── ai_implementation/    # Design docs and brainstorming
```

## Per-Project `.orchestrator/` Layout

When you run `orchestrator init <project-path>`, this is created inside the target project:

```
<project>/
├── .orchestrator/
│   ├── agents.toml              # Agent definitions (editable)
│   ├── prompts/
│   │   ├── coder.md             # Startup prompt for coder agent
│   │   ├── tester.md            # Startup prompt for tester agent
│   │   └── reviewer.md          # Startup prompt for reviewer agent
│   ├── messages/
│   │   ├── to_coder/            # Inbox per agent (auto-derived from agents.toml)
│   │   ├── to_tester/
│   │   ├── to_reviewer/
│   │   ├── processed/
│   │   └── dead_letter/
│   └── runtime/
│       ├── logs/
│       │   ├── events.jsonl
│       │   ├── state.json
│       │   └── spike_transcripts/
│       └── pids/
```

## `agents.toml` Format

```toml
[[agents]]
id = "coder"
command = "claude"
prompt_file = "prompts/coder.md"        # relative to .orchestrator/
worktree_prompt_file = "prompts/coder-worktree.md"
allowed_write_dirs = ["orchestrator/src/"]            # relative to project root

[[agents]]
id = "tester"
command = "codex"
prompt_file = "prompts/tester.md"
worktree_prompt_file = "prompts/tester-worktree.md"
allowed_write_dirs = ["orchestrator/tests/"]

[[agents]]
id = "reviewer"
command = "copilot"
prompt_file = "prompts/reviewer.md"
worktree_prompt_file = "prompts/reviewer-worktree.md"

# Worker group: coder + tester launch together in a split-pane tmux session.
# Set count = 2 to run two parallel coder+tester pairs.
[[worker_groups]]
id = "worker"
agents = ["coder", "tester"]
layout = "horizontal"
count = 2
```

Optional terminal setting (top-level or per-agent):

```toml
terminal = "iterm2"   # project-wide default on macOS

[[agents]]
id = "coder"
command = "claude"
prompt_file = "prompts/coder.md"
worktree_prompt_file = "prompts/coder-worktree.md"
allowed_write_dirs = ["orchestrator/src/"]
terminal = "terminal"  # optional override for this agent only
```

- **Tmux session names** are auto-derived: `{project-dir-name}-{agent-id}` (e.g., `myproject-coder`)
- **Inbox directories** are auto-derived: `.orchestrator/messages/to_{agent_id}/`
- **Prompt template variables**: `{{project_root}}`, `{{messages_dir}}`, `{{agent_id}}`, `{{instance_suffix}}`, `{{peer_inboxes}}`, `{{peer_ids}}`, `{{instance_index}}`, `{{group_count}}`, `{{worker_inboxes}}`
- **Worker groups**: Agents listed in `[[worker_groups]]` launch together in a single tmux session with split panes. When `count > 1`, each instance gets a numeric suffix (e.g., `coder-1`, `coder-2`)
- **Worktree prompts**: When `--worktree` mode is active, the `worktree_prompt_file` is appended to each agent's startup prompt with git branch/worktree variables
- **Supported `terminal` values**: `auto` (default), `iterm2`, `terminal`

## Rust Orchestrator Architecture

The orchestrator has 8 core modules in `orchestrator/src/`:

1. **Config** (`config.rs`) — loads `agents.toml`, resolves all paths relative to `.orchestrator/`, renders prompt templates with group-aware variables, scaffolds new projects via `init`
2. **Process Supervisor** (`supervisor.rs`) — spawns agent tmux sessions (standalone and grouped), tracks restart counts, respawns crashed agents with exponential backoff (cap: 5 restarts in 2 minutes, then mark degraded), runs attention detection loop to alert when agents need user input
3. **Message Watcher** (`watcher.rs`) — uses `notify` crate to detect new files in `messages/to_*` directories, routes messages to agent sessions, SHA-256 dedup, backpressure handling
4. **Injector** (`injector.rs`) — sends message content into tmux sessions via `tmux load-buffer` + `paste-buffer` + `send-keys Enter`; retry logic (3 attempts, 1s backoff). Spawns group sessions with split panes. Sets visual attention styling (red pane/status bar) on blocked agents. Cross-platform terminal window launching (Terminal.app on macOS, various emulators on Linux).
5. **Event Logger** (`logger.rs`) — appends structured JSON lines to `events.jsonl` for all spawn/exit/restart and message routing events
6. **Spike** (`spike.rs`) — de-risking tool to validate tmux injection against any configured agent
7. **Worktree** (`worktree.rs`) — git worktree setup: creates per-agent branches and worktree directories, symlinks `.orchestrator/` for shared message queues
8. **Scope** (`scope.rs`) — enforces per-agent write directory restrictions

## Message Protocol

- **File format**: `.md` or `.txt` (optionally `.json` envelope)
- **Naming convention**: `2026-02-20T12-34-56Z__from-coder__to-tester__topic-tests.md`
- **Atomic writes**: writers must write to a temp file then rename into the inbox
- **Delivery**: at-least-once; deduplicate by content hash
- **Lifecycle**: new file → inject → move to `processed/` on success, or `dead_letter/` after retries exhausted

## OS & Terminal Support

The orchestrator supports cross-platform terminal window launching to visualize agent sessions:

- **macOS**: Supports `Terminal.app` and `iTerm2`.
- **Linux**: Automatically detects and launches the appropriate terminal emulator with no-fork flags. Supported emulators include:
  - `ptyxis` (Fedora GNOME default)
  - `gnome-terminal` (Ubuntu/GNOME)
  - `konsole` (KDE)
  - `xfce4-terminal` (XFCE)
  - `alacritty`
  - `kitty`
  - `xterm`

### Using iTerm2 on macOS

If you want agent sessions to open in iTerm2 instead of Terminal.app, set `terminal = "iterm2"` in `.orchestrator/agents.toml`.

Project-wide default:

```toml
terminal = "iterm2"

[[agents]]
id = "coder"
command = "claude"
prompt_file = "prompts/coder.md"
allowed_write_dirs = ["src/"]
```

Per-agent override:

```toml
[[agents]]
id = "reviewer"
command = "cursor agent"
prompt_file = "prompts/reviewer.md"
allowed_write_dirs = ["/"]
terminal = "iterm2"
```

Behavior:

- `iterm2` uses iTerm2 tabs/windows running regular `tmux attach`.
- `auto` remains the default and uses Terminal.app on macOS.
- `terminal` forces Terminal.app for that project or agent.
- On non-macOS platforms, `iterm2` falls back to the normal auto-detected terminal behavior.

Typical workflow:

```bash
cd orchestrator
cargo run -- run /path/to/project
```

When the target project's `agents.toml` has `terminal = "iterm2"`, the orchestrator will open the agent sessions in iTerm2.

## Build Commands

```bash
cd orchestrator
cargo build                                    # build orchestrator
cargo run -- init /path/to/project             # scaffold .orchestrator/ in a project
cargo run -- run /path/to/project              # launch all agents
cargo run -- run . --worktree --branch PR-123  # launch with git worktrees per agent
cargo run -- spike /path/to/project            # test injection (first agent)
cargo run -- spike /path/to/project --agent tester  # test specific agent
cargo run -- status /path/to/project           # check agent health
cargo run -- stop /path/to/project             # clean shutdown
cargo test                                     # run tests
```

All path arguments default to `.` (current directory) if omitted.

## Worker Groups

Worker groups let you pair agents that should run side-by-side in a single tmux session with split panes. The `count` field controls how many parallel instances of the group to spawn.

With `count = 2` and agents `["coder", "tester"]`, the orchestrator creates:
- `coder-gan-worker-1` session: `coder-1` (left pane) + `tester-1` (right pane)
- `coder-gan-worker-2` session: `coder-2` (left pane) + `tester-2` (right pane)
- `coder-gan-reviewer` session: standalone reviewer

Each instance gets its own inbox directories (`to_coder-1/`, `to_tester-1/`, etc.) and prompt templates are rendered with instance-specific variables.

## Worktree Mode

Use `--worktree --branch <name>` to give each agent its own git worktree and branch. This lets agents edit files in parallel without merge conflicts.

```bash
cargo run -- run . --worktree --branch JOM/PR-1057
```

This creates:
- `coder_gan-worktrees/JOM/PR-1057/coder-1/` → branch `JOM/PR-1057/coder-1`
- `coder_gan-worktrees/JOM/PR-1057/tester-1/` → branch `JOM/PR-1057/tester-1`
- `coder_gan-worktrees/JOM/PR-1057/reviewer/` → branch `JOM/PR-1057/reviewer`

Each worktree gets a symlink to the main `.orchestrator/` directory so all agents share message queues and config. Worktree-specific prompt addenda (git workflow instructions) are appended to each agent's startup prompt automatically.

Coder agents merge the reviewer's branch to pull in approved code. Tester agents share a branch/worktree with their coder partner, so they don't need to merge — the coder handles it for the pair.

## Attention Detection

The supervisor polls agent tmux panes every 3 seconds looking for CLI permission prompts (e.g., "Allow once", "Run this command?", "(y/a/x/e/n)"). When a prompt is detected:

1. The pane background turns **dark red** and the status bar turns red
2. The window title changes to **[⚠ INPUT NEEDED]**
3. An **OS notification** is sent (macOS/Linux)

When the agent resumes (pane content changes), the styling automatically clears.

---
