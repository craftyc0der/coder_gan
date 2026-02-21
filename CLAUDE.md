# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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
allowed_write_dirs = ["src/"]            # relative to project root

[[agents]]
id = "tester"
command = "codex"
prompt_file = "prompts/tester.md"
allowed_write_dirs = ["tests/"]

[[agents]]
id = "reviewer"
command = "claude"
prompt_file = "prompts/reviewer.md"
allowed_write_dirs = ["review/"]
```

- **Tmux session names** are auto-derived: `{project-dir-name}-{agent-id}` (e.g., `myproject-coder`)
- **Inbox directories** are auto-derived: `.orchestrator/messages/to_{agent_id}/`
- **Prompt template variables**: `{{project_root}}`, `{{messages_dir}}`, `{{agent_id}}`

## Rust Orchestrator Architecture

The orchestrator has 6 core modules in `orchestrator/src/`:

1. **Config** (`config.rs`) — loads `agents.toml`, resolves all paths relative to `.orchestrator/`, renders prompt templates, scaffolds new projects via `init`
2. **Process Supervisor** (`supervisor.rs`) — spawns agent tmux sessions, tracks restart counts, respawns crashed agents with exponential backoff (cap: 5 restarts in 2 minutes, then mark degraded)
3. **Message Watcher** (`watcher.rs`) — uses `notify` crate to detect new files in `messages/to_*` directories, routes messages to agent sessions, SHA-256 dedup, backpressure handling
4. **Injector** (`injector.rs`) — sends message content into tmux sessions via `tmux load-buffer` + `paste-buffer` + `send-keys Enter`; retry logic (3 attempts, 1s backoff). Also handles cross-platform terminal window launching (Terminal.app on macOS, various emulators on Linux).
5. **Event Logger** (`logger.rs`) — appends structured JSON lines to `events.jsonl` for all spawn/exit/restart and message routing events
6. **Spike** (`spike.rs`) — de-risking tool to validate tmux injection against any configured agent

## Message Protocol

- **File format**: `.md` or `.txt` (optionally `.json` envelope)
- **Naming convention**: `2026-02-20T12-34-56Z__from-coder__to-tester__topic-tests.md`
- **Atomic writes**: writers must write to a temp file then rename into the inbox
- **Delivery**: at-least-once; deduplicate by content hash
- **Lifecycle**: new file → inject → move to `processed/` on success, or `dead_letter/` after retries exhausted

## Build Commands

```bash
cd orchestrator
cargo build                                    # build orchestrator
cargo run -- init /path/to/project             # scaffold .orchestrator/ in a project
cargo run -- run /path/to/project              # launch all agents
cargo run -- spike /path/to/project            # test injection (first agent)
cargo run -- spike /path/to/project --agent tester  # test specific agent
cargo run -- status /path/to/project           # check agent health
cargo run -- stop /path/to/project             # clean shutdown
cargo test                                     # run tests
```

All path arguments default to `.` (current directory) if omitted.

---
