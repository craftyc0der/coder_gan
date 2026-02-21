# PRD: Gemini CLI Agent Support

## Problem Statement

The orchestrator is designed to be CLI-agnostic — the `command` field in `agents.toml` is passed directly to tmux as a shell command. In theory, adding Gemini CLI support is as simple as setting `command = "gemini"`. In practice, there are several considerations around default configuration, prompt compatibility, auto-approval modes, and documentation that should be addressed to make Gemini a first-class citizen alongside `claude`, `codex`, and `copilot`.

## Current State

- The orchestrator makes **zero assumptions** about the CLI tool. It spawns the command in a detached tmux session, injects text via `tmux load-buffer` + `paste-buffer` + `send-keys Enter`, and checks health via `tmux has-session`.
- The default `agents.toml` created by `orchestrator init` uses `claude`, `codex`, and `copilot`.
- Startup prompts are plain Markdown files injected as raw text after a 5-second init delay.
- Messages between agents use a simple text framing format (not CLI-specific).

## Gemini CLI Characteristics

Based on `gemini --help` (v0.29.5):

| Feature | Gemini CLI | Relevance |
|---|---|---|
| Interactive mode | Default (no flags needed) | ✅ Works with tmux injection |
| Auto-approve actions | `--yolo` or `--approval-mode yolo` | ⚠️ Needed for autonomous operation |
| Model selection | `-m <model>` | Optional, useful for config |
| Sandbox mode | `-s` / `--sandbox` | Safety option worth documenting |
| Non-interactive mode | `-p` / `--prompt` | Not needed (we want interactive) |
| Resume sessions | `--resume` | Could conflict with tmux respawn |
| Include directories | `--include-directories` | Useful for multi-dir projects |
| Output format | `--output-format text\|json` | Default text is fine for tmux |

### Key Differences from Claude/Codex/Copilot

1. **Auto-approval**: Gemini requires `--yolo` or `--approval-mode yolo` to avoid interactive confirmation prompts that would block the agent. Without this, the orchestrator would inject a message but Gemini would wait for the user to approve each action.

2. **Session resumption**: Gemini supports `--resume` which could cause confusion if a previous Gemini session exists in the project. When the orchestrator respawns an agent after a crash, Gemini might try to resume a stale session. This should be documented or mitigated.

3. **Sandbox mode**: Gemini's `--sandbox` flag runs code in a sandboxed environment. This is a useful safety feature for the tester agent role but may be too restrictive for the coder agent.

4. **Working directory**: Gemini infers its workspace from the current directory. Since tmux sessions are spawned with the project root as the working directory, this should work correctly.

## Proposed Approach

Since the orchestrator is already CLI-agnostic, the changes are primarily in **defaults, documentation, and validation** — not core logic.

---

## Implementation Steps

### Step 1: Update the default `agents.toml` template to include a Gemini example

The default template created by `orchestrator init` should show Gemini as a commented-out alternative, so users know it's supported:

- [ ] Add a comment block in the default `agents.toml` template showing Gemini configuration examples:
  ```toml
  # To use Gemini CLI, set the command with appropriate flags:
  # command = "gemini --yolo"
  # For sandboxed execution (recommended for tester agents):
  # command = "gemini --yolo --sandbox"
  # To specify a model:
  # command = "gemini --yolo -m gemini-2.5-pro"
  ```
- [ ] Keep the existing defaults (`claude`, `codex`, `copilot`) unchanged — this is additive

### Step 2: Add a Gemini-specific startup prompt template

The existing prompt templates (`coder.md`, `tester.md`, `reviewer.md`) are already CLI-agnostic plain Markdown. However, Gemini may benefit from slightly different phrasing. Provide optional Gemini-flavored templates:

- [ ] Create `prompts/coder_gemini.md` as an alternative template (or document that the existing `coder.md` works as-is with Gemini)
- [ ] Test that the existing prompts work correctly when injected into a Gemini CLI session
- [ ] If Gemini has any known quirks with large text injection (buffer limits, paste handling), document them

### Step 3: Validate Gemini CLI compatibility with tmux injection

The orchestrator injects text via `tmux load-buffer` → `paste-buffer` → `send-keys Enter`. This needs to be verified with Gemini:

- [ ] Run `orchestrator spike` with `command = "gemini --yolo"` to validate:
  - Session spawns correctly
  - Text injection works (startup prompt is received)
  - Gemini processes the injected prompt
  - Message injection works (inter-agent messages are received)
  - `tmux capture-pane` captures Gemini output correctly
- [ ] Run crash recovery spike: kill the Gemini process, verify respawn works
- [ ] Verify that Gemini does **not** auto-resume a stale session on respawn (document if it does and add `--no-resume` or similar mitigation)
- [ ] Document any differences in behavior vs. `claude` / `codex`

### Step 4: Document the `--yolo` / approval mode requirement

This is the most important practical consideration. Without `--yolo`, Gemini will block on action confirmations:

- [ ] Add a section in `README.md` explaining that Gemini agents **must** use `--yolo` or `--approval-mode yolo` for autonomous operation
- [ ] Add a section documenting the available approval modes and when each is appropriate:
  - `yolo` — fully autonomous (recommended for all orchestrator agents)
  - `auto_edit` — auto-approves file edits but prompts for other actions
  - `plan` — read-only mode (useful for reviewer agents)
- [ ] Consider adding a startup validation warning: if `command` contains `gemini` but does not include `--yolo` or `--approval-mode`, print a warning at `orchestrator run` time

### Step 5: Add optional startup validation for known CLI tools

Without adding hard dependencies on any CLI tool, add soft validation that prints helpful warnings:

- [ ] At `orchestrator run` startup, for each agent:
  - If `command` starts with `gemini` and doesn't contain `--yolo` or `--approval-mode`, warn: `"⚠ Agent '{id}' uses gemini without --yolo. It may block on action confirmations."`
  - If `command` starts with `claude` and doesn't contain `--dangerously-skip-permissions`, warn similarly (if applicable)
- [ ] These are **warnings only** — never block startup
- [ ] Implement as a simple string-matching helper, not a full CLI parser

### Step 6: Update documentation

- [ ] Update `README.md` with a "Supported CLI Tools" section listing `claude`, `codex`, `copilot`, `gemini` with recommended flags for each
- [ ] Update `CLAUDE.md` to mention Gemini as a supported agent command
- [ ] Update `ai_implementation/step1_brainstorming.md` agent table to include `gemini` as an option (or note it in a new section)

### Step 7: Add tests

- [ ] Add a config test verifying that `command = "gemini --yolo"` parses correctly from `agents.toml` (should already work, but worth an explicit test)
- [ ] Add a test for the startup validation warning logic (Step 5): config with `command = "gemini"` (no `--yolo`) triggers a warning
- [ ] Add a test confirming no warning is emitted for `command = "gemini --yolo"`
- [ ] Add a test confirming no warning is emitted for unknown commands (the validator should only warn for known CLI tools)

---

## Completion Checklist

### Configuration
- [ ] Default `agents.toml` template includes commented Gemini examples
- [ ] Gemini works with existing prompt templates (or alternatives provided)
- [ ] `--yolo` requirement is clearly documented

### Validation
- [ ] `orchestrator spike` passes with `command = "gemini --yolo"`
- [ ] Text injection works correctly with Gemini interactive mode
- [ ] Crash recovery / respawn works with Gemini
- [ ] No stale session resume issues on respawn

### Startup warnings
- [ ] Warning emitted for `gemini` without `--yolo`
- [ ] No warning for `gemini --yolo` or `gemini --approval-mode yolo`
- [ ] No warning for unknown/custom commands
- [ ] Warnings are non-blocking (never prevent startup)

### Tests
- [ ] Config parsing test for `gemini --yolo` command
- [ ] Startup validation warning tests (warn / no-warn / unknown-command)

### Documentation
- [ ] `README.md` updated with Gemini instructions and "Supported CLI Tools" section
- [ ] `CLAUDE.md` mentions Gemini support
- [ ] Default `agents.toml` has Gemini examples in comments

---

## Out of Scope

- Gemini-specific features (MCP servers, extensions, skills) — these are user-configured outside the orchestrator
- Enforcing `--yolo` at the orchestrator level (we warn, not block)
- Gemini API key management — the user is responsible for authenticating `gemini` before running the orchestrator
- Gemini-specific message formatting — the existing plain-text framing is sufficient
- Non-interactive / headless Gemini mode (`-p` flag) — the orchestrator requires interactive sessions
