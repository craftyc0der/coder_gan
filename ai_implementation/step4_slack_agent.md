# PRD: Slack Agent Type

## Problem Statement

The orchestrator currently supports interactive CLI-based agents (`claude`, `codex`, `copilot`, `cursor agent`, `gemini`) that operate in tmux sessions. These agents are reactive — they respond to injected prompts and inter-agent messages but have no awareness of external communication channels.

Teams using Slack need a way to monitor channels for important messages (questions, incidents, blockers, requests) and get intelligent, AI-researched responses surfaced — without manually watching dozens of Slack channels. Today this requires a human to constantly monitor Slack, context-switch, research answers, and respond. A Slack agent that watches channels, summarizes inbound messages, cross-references project knowledge (via Notion MCP), determines whether a response is needed, and surfaces actionable reports to the user's private notification channel would save significant time and reduce missed critical messages.

## Goal

Add a new **Slack agent type** to the orchestrator that:

1. Reads Slack configuration (tokens, channels, URLs) from the `agents.toml` config.
2. Opens a Slack WebSocket connection (Socket Mode) to watch specified channels in real time.
3. Routes interesting messages to its own dedicated triage AI (in a tmux session, separate from coding agents) for summarization, research via Notion MCP, and importance assessment.
4. When the AI determines a message needs a response, posts a structured report to the user's **private notification channel** with: issue summary, research context, suggested response, and a link to the original message — pinging the user's phone via Slack notification. The bot **only** writes to this private notification channel and never posts in the watched channels.

## Scope

### In Scope

- New `slack` agent type recognized by the orchestrator
- Slack-specific config fields in `agents.toml` (or a referenced TOML/env config file)
- A new `slack.rs` module implementing WebSocket-based Slack channel monitoring
- Integration with the existing agent/tmux/injector pipeline for AI processing
- AI-driven "needs response" assessment — only notify when action is required
- Posting structured reports to the user's private notification channel (the only channel the bot writes to)
- Notion MCP cross-referencing via the backing AI agent's prompt
- Slack message deduplication (don't re-process edited messages or bot's own posts)
- Secure credential handling (env vars, file reference — never inline tokens in TOML)

### Out of Scope

- Slack OAuth app distribution / install flow (assumes a pre-configured Slack app with Bot Token + App-Level Token)
- Interactive Slack modals or slash commands (bot is read-only on watched channels, write-only to notification channel)
- Posting in watched channels (bot never replies inline — all output goes to the private notification channel)
- Multi-workspace Slack support (single workspace per agent instance)
- Proactively discovering threads to watch (only watches threads the user has already replied to)
- Notion MCP server implementation (assumes it's available to the backing AI agent)
- Rich Slack Block Kit message formatting beyond basic structured text (can iterate later)

---

## Current State

### Affected Files

| File | Current Purpose | Changes Needed |
|---|---|---|
| `orchestrator/Cargo.toml` | Rust dependencies | Add Slack WebSocket + TLS deps (`tungstenite`, `tokio-tungstenite`, `reqwest`, `url`) |
| `orchestrator/src/lib.rs` | Module declarations | Add `pub mod slack;` |
| `orchestrator/src/config.rs` | TOML parsing, `AgentEntry` struct | Add optional `slack_config` field (or `agent_type` discriminator) to `AgentEntry`; parse Slack-specific config |
| `orchestrator/src/supervisor.rs` | Agent spawning and health loop | Handle `slack` agent type: spawn the Slack watcher task instead of (or alongside) a tmux session |
| `orchestrator/src/slack.rs` | *(new file)* | Slack WebSocket client, message filtering, alert posting |
| `orchestrator/src/main.rs` | CLI entry point | No changes expected (agents are data-driven from `agents.toml`) |
| `orchestrator/src/watcher.rs` | Filesystem message watcher | No changes (Slack agent uses the same message queue to communicate with other agents) |
| `orchestrator/tests/slack_tests.rs` | *(new file)* | Unit/integration tests for Slack agent module |

### Key Findings

- The orchestrator is already agent-type-agnostic: `AgentEntry.command` is a free string. A Slack agent doesn't fit the "CLI in a tmux session" model — it's a long-running async task, not an interactive terminal.
- The existing `AgentConfig` struct carries `tmux_session` and `cli_command` which don't apply to a Slack agent. The agent type needs a discriminator so the supervisor knows whether to spawn a tmux session or a Slack watcher task.
- The filesystem message queue (`messages/to_<agent>/`) can still be used for other agents to send messages *to* the Slack agent (e.g., "post this to #general").
- The Slack agent needs its own dedicated AI agent for summarization/research — separate from the coding agents. Rather than requiring two entries in `agents.toml`, the `slack` agent type is a **composite**: a single config entry that internally spawns both a WebSocket watcher (async task) and a dedicated AI tmux session for triage. This keeps configuration simple — one `[[agents]]` block, one agent.

---

## Technical Design

### Architecture Overview

```
                        Single "slack" agent entry
                  ┌─────────────────────────────────────────────────────┐
Slack Sources     │  Orchestrator                                       │
─────────────     │                                                     │
  #engineering    │  ┌──────────────────┐      ┌──────────────────────┐ │
  #incidents      │  │  SlackWatcher    │      │  Triage AI (tmux)    │ │
  #deployments    │  │  (WebSocket)     │      │  claude / copilot    │ │
  @mentions    ──▶│  │                  │      │  with Notion MCP     │ │
  replied threads │  │  1. Receive msg  │      │                      │ │
  DMs/PMs         │  │  2. Filter       │      │                      │ │
                  │  │  3. Write inbox ─┼─────▶│  4. Summarize        │ │
                  │  │                  │      │  5. Research (MCP)   │ │
                  │  │  7. Read resp  ◀─┼──────│  6. Assess + respond │ │
                  │  │  8. Post notif   │      │                      │ │
                  │  └──────────────────┘      └──────────────────────┘ │
                  └─────────────────────────────────────────────────────┘
                           │
                           ▼
                    Private Notification Channel
                    ┌──────────────────────┐
                    │ 🚨 Needs Response      │
                    │ Issue: ...            │
                    │ Research: ...         │
                    │ Suggested reply: ...  │
                    │ Link: <original msg>  │
                    │ @josh                 │
                    └──────────────────────┘
```

### Agent Configuration (`agents.toml`)

The Slack agent is a **composite agent type** — a single `[[agents]]` entry that internally spawns both a Slack WebSocket watcher and a dedicated AI tmux session for triage. No separate backing agent entry is needed. Credentials are stored in a separate file (or env vars) to avoid leaking tokens in version-controlled TOML.

```toml
[[agents]]
id = "slack-watcher"
agent_type = "slack"                         # composite: WebSocket + dedicated AI tmux
command = "cursor"                           # AI CLI for the triage tmux session
prompt_file = "prompts/slack-watcher.md"     # startup prompt for the triage AI
allowed_write_dirs = []

# Slack-specific configuration
[agents.slack]
config_file = ".orchestrator/slack_config.toml"   # external config with secrets (MUST be in .gitignore)
```

### Slack Config File (`.orchestrator/slack_config.toml`)

Separated from `agents.toml` so it can be `.gitignore`'d:

```toml
# Slack Bot Token (xoxb-...) — requires channels:read, channels:history,
# chat:write, users:read scopes
bot_token_env = "SLACK_BOT_TOKEN"          # read from env var (preferred)
# bot_token = "xoxb-..."                  # or inline (not recommended)

# Slack App-Level Token (xapp-...) — required for Socket Mode
app_token_env = "SLACK_APP_TOKEN"
# app_token = "xapp-..."

# Bot's own user ID (to filter out its own messages)
bot_user_id = "U0BOTID123"

# --- Watch sources ---

# Channels to monitor for all new messages (by ID for stability, names as comments)
watch_channels = [
    "C01ENGINEERING",   # #engineering
    "C02INCIDENTS",     # #incidents
    "C03DEPLOYMENTS",   # #deployments
]

# Watch for @mentions of the user anywhere in the workspace
watch_mentions = true

# Watch threads the user has previously replied to
watch_replied_threads = true

# Watch all DMs/PMs the user is part of
watch_dms = true

# Private notification channel — the ONLY channel the bot writes to.
# This should be a private channel that pings your phone.
notification_channel = "C04MYNOTIFS"     # #josh-notifications

# Your Slack user ID (for @mentions in notification posts)
alert_user_id = "U01ABC123"

# Optional: minimum message length to consider (filter noise)
min_message_length = 20

# Optional: ignore messages from these bot user IDs
ignore_bot_ids = ["U0OTHERBOT"]

# Optional: keywords that always trigger an alert (case-insensitive)
alert_keywords = ["urgent", "incident", "outage", "blocked", "help", "p0", "p1"]
```

### Token Resolution

Tokens are resolved in priority order:
1. `*_env` field → read the named environment variable
2. `*` field (e.g., `bot_token`) → use the inline value directly
3. If neither is set → return a config error at startup

This keeps secrets out of committed files while allowing local dev convenience.

### New `agent_type` Discriminator

Add an optional `agent_type` field to `AgentEntry`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AgentEntry {
    pub id: String,
    pub command: String,
    pub prompt_file: String,
    pub allowed_write_dirs: Vec<String>,
    #[serde(default = "default_agent_type")]
    pub agent_type: AgentType,
    #[serde(default)]
    pub slack: Option<SlackAgentConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub enum AgentType {
    #[default]
    #[serde(rename = "cli")]
    Cli,
    #[serde(rename = "slack")]
    Slack,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SlackAgentConfig {
    pub config_file: String,          // path to slack_config.toml (must be in .gitignore)
}
```

### Slack Module (`slack.rs`)

Core components:

```rust
pub struct SlackWatcher {
    config: SlackConfig,              // parsed from slack_config.toml
    agent_config: SlackAgentConfig,   // from agents.toml
    messages_dir: PathBuf,            // .orchestrator/messages/
    logger: Arc<Logger>,
    seen_messages: HashSet<String>,   // dedup by Slack message ts
}

pub struct SlackConfig {
    pub bot_token: String,
    pub app_token: String,
    pub bot_user_id: String,
    // Watch sources
    pub watch_channels: Vec<String>,
    pub watch_mentions: bool,           // @mentions of alert_user_id anywhere
    pub watch_replied_threads: bool,    // threads the user has replied to
    pub watch_dms: bool,                // DMs/PMs the user is in
    // Output
    pub notification_channel: String,   // private channel — only place the bot writes
    pub alert_user_id: String,          // Slack user ID to @mention + track mentions/threads for
    pub min_message_length: usize,
    pub ignore_bot_ids: Vec<String>,
    pub alert_keywords: Vec<String>,
}
```

**Key methods:**

1. **`connect()`** — Establish WebSocket connection via Slack Socket Mode (`wss://wss-primary.slack.com/link`). Uses the App-Level Token to get the WebSocket URL via `apps.connections.open`, then connects.

2. **`run_loop()`** — Main event loop:
   - Receive events from WebSocket
   - Match event against watch sources:
     - Channel messages: event channel is in `watch_channels`
     - Mentions: message text contains `<@alert_user_id>` and `watch_mentions` is true
     - Replied threads: message is in a thread where `alert_user_id` has previously replied and `watch_replied_threads` is true
     - DMs: event is in an IM/MPIM conversation the user is part of and `watch_dms` is true
   - Filter: skip bot messages, messages below min length, already-seen timestamps, messages from ignored bots
   - For qualifying messages: write a message file to the triage agent's inbox with context (channel, author, thread link, message text, watch source)
   - Acknowledge the event envelope back to Slack

3. **`post_notification(report: &str)`** — Post a formatted message to the **private notification channel** (the only channel the bot ever writes to) via `chat.postMessage` API:
   - Format: summary, research findings, suggested reply, permalink to original
   - Mention the configured user (`<@alert_user_id>`)
   - Use `unfurl_links: false` to keep the post clean
   - Only called when the backing agent determines the message needs a response

4. **`watch_response_inbox()`** — Watch the Slack agent's own inbox (`messages/to_slack-watcher/`) for responses from the triage AI. When a response file appears, check if the agent assessed it as needing a response. If yes, parse it and call `post_notification()`. If the agent determined no response is needed, log and discard.

### Message Flow (Detailed)

1. **Slack → Watcher**: WebSocket event arrives with a new message in a watched channel.

2. **Watcher → Match + Filter**: First, determine if the message matches any watch source:
   - **Channel watch**: message is in a `watch_channels` channel
   - **Mention watch**: message contains `<@alert_user_id>` anywhere in the workspace
   - **Replied thread watch**: message is in a thread where `alert_user_id` has replied before
   - **DM watch**: message is in a DM/group DM the user is part of

   Then skip if:
   - Message is from the bot itself (`bot_user_id`)
   - Message is from an ignored bot
   - Message timestamp already seen (dedup)
   - Message text is shorter than `min_message_length`
   - Message doesn't match any watch source

3. **Watcher → Backing Agent Inbox**: Write a message file:
   ```
   --- SLACK MESSAGE ---
   Channel: #engineering (C01ENGINEERING)
   Author: @jane (U012345)
   Time: 2026-03-02T14:30:00Z
   Thread: https://myteam.slack.com/archives/C01ENGINEERING/p1709388600000000
   Keywords matched: incident, blocked

   The deploy pipeline is blocked — CI is failing on the integration tests
   after the latest merge to main. @josh can you take a look? This is
   blocking the 2.1 release.
   ---

   Please analyze this message:
   1. Does this message need a response from me? (YES/NO)
   2. If YES: Summarize the issue concisely
   3. Cross-reference with Notion project docs for context (use the Notion MCP)
   4. Assess urgency (P0-P4)
   5. Draft a suggested response
   6. Format your findings as a report
   7. If NO: Briefly state why no response is needed (for logging only)
   ```

   File written to: `.orchestrator/messages/to_slack-watcher/<timestamp>__from-slack-watcher__to-slack-watcher__topic-slack-triage.md`

4. **Triage AI Processes**: The message is injected into the Slack agent's own dedicated tmux session (spawned internally from `command` in the agent entry). This triage AI (e.g., Claude with Notion MCP access) is exclusively dedicated to Slack triage — it is not shared with the coder, tester, or reviewer agents. It researches and writes a response file to the Slack agent's inbox.

5. **Response → Notification (only if response needed)**: The Slack watcher reads the response file from its inbox. If the backing agent assessed "YES — needs response", the watcher posts to the **private notification channel** (the only channel the bot ever writes to):
   ```
   🚨 *Needs Response*

   *Channel:* <#C01ENGINEERING|engineering>
   *From:* @jane
   *Urgency:* P1 — Blocking release

   *Summary:*
   CI integration tests failing after latest merge to main, blocking 2.1 release deploy pipeline.

   *Research (Notion):*
   - Release 2.1 tracker shows target date March 3
   - Integration test suite was last updated in step_12 (auth refactor)
   - Known flaky test: `test_concurrent_sessions` — documented in Notion runbook

   *Suggested Response:*
   > Looking into it now. The integration test failures look related to the auth refactor
   > in step_12 — there's a known flaky test documented in the runbook. I'll check if
   > that's the cause and push a fix. ETA ~30min.

   *Original:* https://myteam.slack.com/archives/C01ENGINEERING/p1709388600000000

   cc <@U01ABC123>
   ```

   If the backing agent assessed "NO — no response needed", the watcher logs the decision and moves the response file to `processed/` without posting anything.

### Supervisor Integration

The supervisor needs to handle Slack agents differently from CLI agents:

```rust
// In supervisor.rs spawn logic:
match agent_entry.agent_type {
    AgentType::Cli => {
        // Existing logic: spawn tmux session, inject prompt
        let handle = injector::spawn_session(&config.tmux_session, &config.cli_command)?;
        // ...
    }
    AgentType::Slack => {
        // 1. Spawn the triage AI in a tmux session (uses command + prompt_file)
        let handle = injector::spawn_session(&config.tmux_session, &config.cli_command)?;
        // Inject startup prompt after init delay
        inject_prompt(&config.tmux_session, &prompt)?;

        // 2. Spawn the Slack WebSocket watcher as a tokio task
        let slack_config = load_slack_config(&agent_entry.slack)?;
        let watcher = SlackWatcher::new(slack_config, config.clone(), messages_dir, logger);
        tokio::spawn(async move {
            watcher.run_loop().await;
        });
    }
}
```

**Health monitoring for Slack agents:**
- Track WebSocket connection state (connected/reconnecting/dead)
- Expose via `orchestrator status` alongside tmux agent health
- Auto-reconnect on WebSocket disconnection with exponential backoff (same 1s/2s/4s/8s/16s pattern)
- Mark degraded after 5 reconnection failures in 2 minutes (same policy as CLI agents)

### Event Logging

New event types for the logger:

| Event | Fields | When |
|---|---|---|
| `slack_connected` | `agent_id`, `channels` | WebSocket connection established |
| `slack_disconnected` | `agent_id`, `reason` | WebSocket dropped |
| `slack_message_received` | `agent_id`, `channel`, `author`, `ts` | Message received from watched channel |
| `slack_message_filtered` | `agent_id`, `channel`, `reason` | Message skipped (bot, dup, too short) |
| `slack_message_routed` | `agent_id`, `source` (channel/mention/thread/dm), `filename` | Message written to triage AI inbox |
| `slack_response_skipped` | `agent_id`, `channel`, `reason` | Backing agent determined no response needed |
| `slack_notification_posted` | `agent_id`, `notification_channel`, `thread_ts` | Report posted to private notification channel |
| `slack_notification_failed` | `agent_id`, `error` | Failed to post to notification channel |

### Security Considerations

1. **Token storage**: Bot and App tokens must never be committed to version control. The `config_file` approach with `_env` suffix fields encourages env-var-based secrets. The `slack_config.toml` file **must** be listed in `.gitignore`. The `orchestrator init` command should add it to `.gitignore` automatically, and the orchestrator should warn at startup if `.orchestrator/slack_config.toml` exists but is not gitignored.

2. **Token scoping**: Document minimum required Slack app scopes:
   - `channels:history` — read messages in public channels
   - `channels:read` — list channels
   - `chat:write` — post to private notification channel
   - `users:read` — resolve user display names
   - `connections:write` — Socket Mode WebSocket
   - `im:history` — read DMs (required if `watch_dms = true`)
   - `im:read` — list DM conversations (required if `watch_dms = true`)
   - `mpim:history` — read group DMs (required if `watch_dms = true`)
   - `mpim:read` — list group DM conversations (required if `watch_dms = true`)

3. **No user impersonation**: The bot posts as itself, never impersonates users.

4. **Write isolation**: The bot **only** writes to the configured private notification channel. It never posts, reacts, or replies in the watched channels. This is enforced at the code level — the only `chat.postMessage` call targets `notification_channel`.

5. **Rate limiting**: Respect Slack API rate limits (Tier 2: ~20 req/min for `chat.postMessage`). Batch notifications if volume is high.

6. **Input sanitization**: Slack message content is untrusted input. When writing to the backing agent's inbox, escape any template variables (`{{...}}`) to prevent prompt injection via Slack messages.

7. **Gitignore enforcement**: The `slack_config.toml` file contains secrets and must be in `.gitignore`. The orchestrator should validate this at startup and refuse to run if the file exists but is not gitignored.

---

## Implementation Checklist

### Phase 1: Config & Data Model

- [ ] Add `agent_type` field to `AgentEntry` in `config.rs` (default: `cli`)
- [ ] Add `SlackAgentConfig` struct and `slack` optional table to `AgentEntry`
- [ ] Add `SlackConfig` struct for the external `slack_config.toml`
- [ ] Add token resolution logic (`_env` → env var, or inline value)
- [ ] Add validation: if `agent_type = "slack"`, require `slack` table and a non-empty `command` (for the triage AI tmux session)
- [ ] Add `.orchestrator/slack_config.toml` to the `.gitignore` template in `init_project()`
- [ ] Add startup check: warn/error if `slack_config.toml` exists but is not gitignored

### Phase 2: Slack WebSocket Client (`slack.rs`)

- [ ] Implement `apps.connections.open` API call to get WebSocket URL
- [ ] Implement WebSocket connection with `tokio-tungstenite`
- [ ] Implement Socket Mode event parsing (envelope acknowledgment + event extraction)
- [ ] Implement message filtering (bot self, ignored bots, dedup by ts, min length, thread replies)
- [ ] Implement keyword matching for priority escalation
- [ ] Implement inbox file writing (atomic temp-file → rename into own agent's inbox for triage AI injection)
- [ ] Implement watch source matching (channels, mentions, replied threads, DMs)
- [ ] Implement "needs response" assessment parsing from backing agent response
- [ ] Implement `chat.postMessage` for notification channel posting (the only channel the bot writes to)
- [ ] Implement response inbox watching (poll or `notify` on `messages/to_<slack-agent>/`)
- [ ] Implement reconnection with exponential backoff
- [ ] Implement permalink generation for original messages

### Phase 3: Supervisor Integration

- [ ] Branch `spawn_all()` on `agent_type`: CLI → existing tmux logic, Slack → spawn tmux session (triage AI) + WebSocket watcher task
- [ ] Add composite health tracking (tmux session state + WebSocket connection state)
- [ ] Expose Slack agent status in `orchestrator status` output (both tmux + WebSocket)
- [ ] Handle Slack agent shutdown in `kill_all()` (close WebSocket, cancel task, kill tmux session)

### Phase 4: Event Logging

- [ ] Add `slack_connected`, `slack_disconnected`, `slack_message_received`, `slack_message_filtered`, `slack_message_routed`, `slack_response_skipped`, `slack_notification_posted`, `slack_notification_failed` event types to `logger.rs`
- [ ] Log all Slack events to `events.jsonl`

### Phase 5: Prompt & Template

- [ ] Create `prompts/slack-triage.md` — startup prompt for the triage AI with instructions on Notion MCP usage, triage format, "needs response" assessment, and response structure
- [ ] Add template variables: `{{notification_channel}}`, `{{watch_channels}}`

### Phase 6: Dependencies & Build

- [ ] Add `tokio-tungstenite` (WebSocket), `reqwest` (HTTP API calls), `url` crates to `Cargo.toml`
- [ ] Feature-gate Slack deps behind a `slack` cargo feature so builds without Slack support stay lean:
  ```toml
  [features]
  default = []
  slack = ["tokio-tungstenite", "reqwest", "url"]
  ```

### Phase 7: Testing

- [ ] Unit tests for Slack config parsing and token resolution
- [ ] Unit tests for message filtering logic
- [ ] Unit tests for alert message formatting
- [ ] Unit tests for permalink generation
- [ ] Integration test: mock WebSocket server → watcher → inbox file written
- [ ] Integration test: response file in inbox → alert posted (mock HTTP)
- [ ] Test `agent_type` config parsing (CLI default, explicit Slack)
- [ ] Test validation errors (missing `slack` table, empty `command`)
- [ ] Unit tests for watch source matching (channels, mentions, replied threads, DMs)

---

## Testing Plan

### Unit Tests

| Test Case | File | Description |
|---|---|---|
| `test_slack_config_parse` | `tests/slack_tests.rs` | Parse a valid `slack_config.toml` with all fields |
| `test_slack_config_env_resolution` | `tests/slack_tests.rs` | Token resolved from env var takes priority over inline |
| `test_slack_config_missing_token` | `tests/slack_tests.rs` | Error when neither env var nor inline token is set |
| `test_message_filter_bot_self` | `tests/slack_tests.rs` | Messages from bot's own user ID are filtered |
| `test_message_filter_ignored_bot` | `tests/slack_tests.rs` | Messages from ignored bot IDs are filtered |
| `test_message_filter_dedup` | `tests/slack_tests.rs` | Duplicate message timestamps are filtered |
| `test_message_filter_min_length` | `tests/slack_tests.rs` | Short messages below threshold are filtered |
| `test_message_filter_pass` | `tests/slack_tests.rs` | Qualifying messages pass all filters |
| `test_keyword_match` | `tests/slack_tests.rs` | Alert keywords trigger priority escalation |
| `test_notification_format` | `tests/slack_tests.rs` | Notification message formatted with all required sections |
| `test_permalink_generation` | `tests/slack_tests.rs` | Slack permalink constructed correctly from channel + ts |
| `test_template_var_escaping` | `tests/slack_tests.rs` | `{{...}}` in Slack messages escaped before writing to inbox |
| `test_agent_type_default` | `tests/config_tests.rs` | Missing `agent_type` defaults to `cli` |
| `test_agent_type_slack_valid` | `tests/config_tests.rs` | Slack agent with valid config parses correctly |
| `test_agent_type_slack_missing_table` | `tests/config_tests.rs` | Error when `agent_type = "slack"` but no `[agents.slack]` table |
| `test_slack_agent_missing_command` | `tests/config_tests.rs` | Error when `agent_type = "slack"` but `command` is empty |

### Integration Tests

| Test Case | Description |
|---|---|
| `test_slack_watcher_to_inbox` | Mock WebSocket sends message → watcher writes file to backing agent inbox with correct format |
| `test_response_needs_reply_posts` | Write "needs response" response file to Slack agent inbox → watcher posts to notification channel via mock `chat.postMessage` |
| `test_response_no_reply_skips` | Write "no response needed" response file → watcher logs and moves to processed, no Slack post |
| `test_reconnect_on_disconnect` | Drop mock WebSocket → watcher reconnects with backoff |
| `test_end_to_end_flow` | Full flow: mock Slack message → inbox → mock backing agent response → alert posted |

---

## Success Criteria

- [ ] A Slack agent defined in `agents.toml` connects to Slack via Socket Mode on `orchestrator run`
- [ ] Messages in watched channels are filtered and routed to the backing agent's inbox
- [ ] When the backing agent determines a response is needed, the report is posted to the private notification channel with the required format (summary, research, suggested reply, link, user ping)
- [ ] When the backing agent determines no response is needed, nothing is posted (logged only)
- [ ] The bot never writes to any channel other than the configured private notification channel
- [ ] `orchestrator status` shows Slack agent connection state
- [ ] `orchestrator stop` cleanly shuts down the Slack WebSocket
- [ ] Tokens are never logged or persisted in plain text beyond the config file
- [ ] All existing CLI agent functionality is unaffected (backward compatible)
- [ ] `cargo build` without `--features slack` compiles without Slack dependencies

---

## Open Questions

1. ~~**Thread replies**~~: Resolved — the watcher monitors threads where the user is mentioned or has replied (`watch_mentions`, `watch_replied_threads`), and all messages (including thread replies) in watched channels.

2. **Response timeout**: If the backing agent doesn't respond within N minutes, should the Slack agent post a "still researching" message? Or silently drop? Suggest: configurable timeout (default 5 min), post a brief "received, investigating" acknowledgment immediately.

3. **Alert deduplication**: If the same Slack thread generates multiple messages (e.g., a conversation), should each message trigger a separate alert, or should the watcher batch thread messages and send a single summary? Suggest: batch messages within a thread over a 60-second window.

4. ~~**Bidirectional responses**~~: Resolved — the bot only writes to the private notification channel, never to watched channels.

5. **Multiple Slack agents**: Can a project have multiple Slack agents watching different channel sets? The config model supports it (each gets its own triage AI tmux session), but should we test/document this explicitly?

6. ~~**Private channels / DMs**~~: Resolved — DMs are a first-class watch source via `watch_dms = true`. Requires `im:history`, `im:read`, `mpim:history`, `mpim:read` scopes.

7. **Notion MCP availability**: The triage AI needs Notion MCP configured in its environment. Should the Slack agent validate this, or is that the user's responsibility? Suggest: user's responsibility — the prompt template documents the expectation.

8. **Replied-thread tracking**: To watch threads the user has replied to, the watcher needs to track which threads the user has participated in. Options: (a) maintain an in-memory set updated from real-time events, (b) query `conversations.replies` on startup to seed the set. Suggest: (a) for simplicity, accepting that threads replied to before the watcher started won't be tracked until a new message triggers a channel event.
