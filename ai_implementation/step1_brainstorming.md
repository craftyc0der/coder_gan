# coder_gan — Product Requirements Document (v3)

## Vision

A **generic multi-agent coding orchestration tool**. A Rust orchestrator can be pointed at **any project directory**, launches and supervises configurable autonomous coding agents, enables agent-to-agent communication via filesystem message queues, and injects messages into live interactive terminal sessions using tmux.

Each project defines its own agent roles, CLI commands, and startup prompts via a `.orchestrator/` configuration directory. The orchestrator itself is project-agnostic.

---

## Goals

1. Point the orchestrator at any project directory and launch configurable agents.
2. Allow agent-to-agent and operator-to-agent communication through file drops.
3. Inject message content into each live interactive terminal session via tmux.
4. Detect agent exit/crash and restart that agent automatically.
5. Keep an auditable message/event log.

## Non-Goals (for MVP)

1. Distributed multi-host orchestration.
2. Full sandbox security hardening.
3. Advanced scheduling/prioritization of tasks.
4. Rich UI beyond terminal + files.

---

## Per-Project `.orchestrator/` Layout

When you run `orchestrator init <project-path>`, this is created inside the target project:

```text
<project>/
├── .orchestrator/
│   ├── agents.toml              # Agent definitions (editable by user)
│   ├── prompts/
│   │   ├── coder.md             # Startup prompt for coder agent
│   │   ├── tester.md            # Startup prompt for tester agent
│   │   └── reviewer.md          # Startup prompt for reviewer agent
│   ├── messages/
│   │   ├── to_coder/            # Inbox per agent (auto-derived from agents.toml)
│   │   ├── to_tester/
│   │   ├── to_reviewer/
│   │   ├── processed/           # Successfully delivered messages
│   │   └── dead_letter/         # Failed messages after retry exhaustion
│   └── runtime/
│       ├── logs/
│       │   ├── events.jsonl     # Structured event log
│       │   ├── state.json       # Live agent state snapshot
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

- **Agent IDs, commands, and roles** are entirely user-configurable — add, remove, or rename agents freely.
- **Tmux session names** are auto-derived: `{project-dir-name}-{agent-id}` (e.g., `myproject-coder`).
- **Inbox directories** are auto-derived: `.orchestrator/messages/to_{agent_id}/`.
- **Prompt template variables**: `{{project_root}}`, `{{messages_dir}}`, `{{agent_id}}` are substituted at startup.

---

## Agent Roles (Default Configuration)

The default `agents.toml` ships three agents, but these are just defaults — users can define any roles.

| Agent | Role | Writes to | Must not write to | CLI |
|-------|------|-----------|-------------------|-----|
| coder | Implementation | `src/` | `tests/`, `review/` | `claude` |
| tester | Tests | `tests/` | `src/`, `review/` | `codex` |
| reviewer | Code review | `review/` | `src/`, `tests/` | `claude` |

### Scope Rules

- Enforced via role prompts (each agent is told its allowed write directories).
- `allowed_write_dirs` in `agents.toml` is stored for future scope enforcement.
- Agents communicate by writing files to each other's inbox directories.

---

## Rust Orchestrator Architecture

The orchestrator lives in `orchestrator/` and has 6 core modules in `orchestrator/src/`:

```text
orchestrator/
├── Cargo.toml
└── src/
    ├── main.rs       # CLI entry point (init, spike, run, status, stop)
    ├── config.rs     # Project config loading, TOML parsing, path resolution, init scaffold
    ├── spike.rs      # De-risking tool to validate tmux injection against any configured agent
    ├── injector.rs   # tmux inject/capture with retry logic
    ├── logger.rs     # Structured JSON line event logger
    ├── supervisor.rs # Agent spawning, health loop, state persistence
    └── watcher.rs    # Filesystem message watcher with routing & dedup
```

### Module Details

1. **Config** (`config.rs`)
   - `ProjectConfig::load(path)` — loads `agents.toml`, resolves all paths relative to `.orchestrator/`, validates agent IDs
   - `init_project(path)` — scaffolds `.orchestrator/` with default `agents.toml` and prompt files
   - `startup_prompts()` — reads prompt files and substitutes `{{project_root}}`, `{{messages_dir}}`, `{{agent_id}}`
   - `tmux_session_for(agent_id)` — derives session name from sanitized project dir name + agent ID
   - `ensure_dirs()` — creates all required subdirectories

2. **Process Supervisor** (`supervisor.rs`)
   - `AgentConfig` struct: `agent_id`, `cli_command`, `tmux_session`, `inbox_dir`, `allowed_write_dirs`
   - `AgentState` struct: `status` (Healthy/Degraded/Dead), `restart_count`, `last_start`, `last_heartbeat`, windowed `restart_timestamps`
   - `Registry` with `spawn_all()`, `health_loop()`, `kill_all()`, `session_for()`, `persist_state()`
   - Health loop: 2s polling via `tmux has-session`, exponential backoff restarts, degraded marking after 5 restarts in 2 minutes

3. **Message Watcher** (`watcher.rs`)
   - Uses `notify` crate to watch all `messages/to_*` directories for Create/Modify events
   - Parses structured filenames (`<timestamp>__from-<sender>__to-<recipient>__topic-<topic>.md`) with fallback to parent directory
   - Routes messages to recipient's tmux session via injector
   - SHA-256 content deduplication
   - Backpressure: queues messages if inbox has >5 unprocessed files
   - Moves files to `processed/` on success, `dead_letter/` on failure

4. **Injector** (`injector.rs`)
   - `spawn_session(session, cmd)` — creates detached tmux session
   - `inject(session, text)` — temp file → `load-buffer` → `paste-buffer` → `send-keys Enter`, with 3-attempt retry and 1s backoff
   - `capture(session)` — `capture-pane -p -S -500`
   - `has_session(session)`, `kill_session(session)`

5. **Event Logger** (`logger.rs`)
   - Appends structured JSON lines with ISO 8601 timestamps
   - Typed events: `agent_spawn`, `agent_exit`, `agent_restart`, `agent_degraded`, `message_received`, `message_injected`, `message_failed`, `message_dead_letter`, `orchestrator_start`, `orchestrator_stop`, plus spike-specific events

6. **Spike** (`spike.rs`)
   - `run_spike(config, agent_id)` — tests tmux injection against any configured agent
   - Validation: injects prompt, waits up to 60s for agent to write a file
   - 10-payload stress test (alternating single-line / multi-line)
   - Crash recovery test: kill session → detect death → respawn
   - Saves pane transcripts and logs events to `spike_events.jsonl`

### Process Lifecycle

- If an agent exits unexpectedly, the supervisor restarts it.
- Exponential backoff: 1s, 2s, 4s, 8s, 16s between restarts.
- Cap: 5 restarts in 2 minutes → mark degraded and stop retrying.
- Other agents keep running when one fails.
- On orchestrator shutdown (Ctrl+C / SIGTERM): kill all tmux sessions, write final state.

---

## Message Protocol (Filesystem)

### Input Contract

- Accepted files: `.md`, `.txt`, optional `.json` envelope.
- Writers should write to temp file then rename into inbox for atomic insert.
- Naming convention: `2026-02-20T12-34-56Z__from-coder__to-tester__topic-tests.md`

### Processing Rules

1. Detect new file in `.orchestrator/messages/to_<agent>/`.
2. Parse filename to extract sender, recipient, and topic.
3. Deduplicate by SHA-256 content hash.
4. Inject into recipient's tmux session with framing header (`--- INCOMING MESSAGE ---`).
5. Move to `messages/processed/` on success.
6. Move to `messages/dead_letter/` after 3 failed injection attempts.

### Delivery Semantics

- **At-least-once** delivery.
- Deduplicate by content hash to prevent re-injection of identical messages.

---

## CLI Commands

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

## Open Questions

1. **Authentication/session persistence**
   - Where do auth tokens live for each agent CLI?
   - What happens when a respawned process needs re-authentication?

2. **Message framing + prompt boundaries**
   - Current approach: prepend `--- INCOMING MESSAGE ---` header with FROM/TOPIC fields.
   - May need refinement if agents get confused by injected text.

3. **Concurrency hazards**
   - Current approach: backpressure queue if inbox >5 files, drain chronologically.
   - May need tuning based on real-world agent throughput.

4. **Reviewer agent uncertainty**
   - Default config uses `claude` as the reviewer command (placeholder).
   - Users should set this to whatever CLI they want for code review.

---

## MVP Acceptance Criteria

1. `orchestrator init` scaffolds `.orchestrator/` in any project directory.
2. `orchestrator run` starts all configured agents and reports healthy state.
3. File dropped in each `messages/to_*` inbox gets injected and archived.
4. Killing any one agent triggers auto-respawn within <5s.
5. Message/event log provides replayable audit trail.
6. `orchestrator spike` validates tmux injection for any configured agent.
7. `orchestrator status` shows agent health table.
8. `orchestrator stop` cleanly shuts down all agents.

---

## Implementation Plan

### Phase 1: Project Setup — Configure for Rust ✓

- [x] **1.1** Created `orchestrator/` directory with `Cargo.toml` (tokio, notify, serde, serde_json, chrono, clap, uuid, sha2, toml)
- [x] **1.2** Created `orchestrator/src/main.rs` with clap CLI: `init`, `spike`, `run`, `status`, `stop`
- [x] **1.3** Created directory scaffold via `init` subcommand
- [x] **1.4** Verified `cargo build` and `cargo run -- --help`

### Phase 2: tmux Spike — De-risk Interactive Session Control ✓

- [x] **2.1** `spike.rs` — spawns detached tmux session for any configured agent
- [x] **2.2** `injector.rs` — tmux inject via temp file → `load-buffer` → `paste-buffer` → `send-keys Enter`
- [x] **2.3** `injector.rs` — tmux capture via `capture-pane -p -S -500`
- [x] **2.4** `cargo run -- spike` injects a validation prompt
- [x] **2.5** Validation checkpoint: waits up to 60s for agent to write file
- [x] **2.6** 10-payload injection test (alternating single-line / multi-line)
- [x] **2.7** Crash recovery test: kill session → detect death → respawn
- [x] **2.8** JSON line logging to `.orchestrator/runtime/logs/spike_events.jsonl`

### Phase 3: Reusable Orchestrator Modules ✓

- [x] **3.1** `injector.rs` — generalized tmux inject/capture with retry (3 attempts, 1s backoff)
- [x] **3.2** `logger.rs` — structured JSON line logger with typed events (configurable filename)
- [x] **3.3** `supervisor.rs` — `AgentConfig`, `AgentState`, `Registry`, `spawn_agent`
- [x] **3.4** Supervisor health loop: 2s polling, exponential backoff restarts, degraded marking
- [x] **3.5** State persistence to `.orchestrator/runtime/logs/state.json`

### Phase 4: Message Watcher — Full Agent Communication Loop ✓

- [x] **4.1** `watcher.rs` — `notify` crate watching all `messages/to_*` for Create/Modify events
- [x] **4.2** Message routing: parse filename → inject into recipient session → move to `processed/` or `dead_letter/`
- [x] **4.3** SHA-256 deduplication
- [x] **4.4** Backpressure: queue if inbox > 5 files, drain in chronological order

### Phase 5: Generic Project Configuration ✓

- [x] **5.1** `config.rs` — `ProjectConfig`, `AgentsToml`, TOML loading, path resolution
- [x] **5.2** `init` subcommand scaffolds `.orchestrator/` with `agents.toml` and prompt files
- [x] **5.3** Prompt template variables: `{{project_root}}`, `{{messages_dir}}`, `{{agent_id}}`
- [x] **5.4** All subcommands accept `[path]` argument (defaults to `.`)
- [x] **5.5** Tmux session names auto-derived from project directory name + agent ID
- [x] **5.6** `stop` reads sessions from `state.json` with config-based fallback
- [x] **5.7** `spike.rs` rewritten to use `injector.rs` (no more duplicate tmux helpers)

### Phase 6: Remaining Work

- [ ] **6.1** Write a seed spec and validate full multi-agent loop: Coder → Tester → Reviewer → Coder
- [ ] **6.2** Add per-agent transcript logging (periodic `capture-pane` → `runtime/logs/<agent>_transcript.log`)
- [ ] **6.3** Add scope enforcement checks (verify agents don't write to forbidden dirs)
- [ ] **6.4** Final stability validation: run full system, confirm all messages route correctly
