# Step 5 — Recurring Timers, Forced Interrupts & Agent Activity State

## Problem Statement

Today the orchestrator can only deliver messages when an agent finishes its current task, because messages are pasted into the tmux input buffer and submitted with `Enter`. If the agent is mid-generation, the message silently queues in the terminal's input buffer and is only processed when the agent finishes and returns its prompt. This leads to three problems:

1. **Delayed course-correction** — an agent may spend 10+ minutes working in the wrong direction before it notices a new message telling it to change course.
2. **No recurring reminders** — there is no way to periodically nudge agents with standing instructions (e.g. "run tests every 15 minutes", "check for lint errors", "summarize your progress").
3. **Enqueued messages block interrupts** — if the orchestrator injects messages directly into the tmux input buffer, those messages silently queue up while the agent is busy. When an urgent `_INTERRUPT` arrives and the orchestrator sends `Ctrl+C`, the previously-enqueued messages fire first (the terminal replays its input buffer), and the interrupt message arrives *last* — defeating the purpose entirely. The last message in the buffer is the important one, not the first.

Additionally, there is currently no way for the orchestrator — or agents themselves — to know whether another agent is busy or idle.

## Goal

1. Add a **timer system** to `agents.toml` that fires recurring prompt injections at configurable intervals per agent.
2. Add a **forced interrupt (`_INTERRUPT`)** message topic that cancels the agent's current generation via `Ctrl+C` before injecting the message, so new instructions take effect immediately.
3. Add **agent activity detection** by observing tmux pane output changes to determine `busy` vs `idle` state for each agent.
4. Add an optional **`include_agents`** array on each timer entry that appends a footer showing the current activity state of the specified agents.

## Scope

### In scope

- New `[[agents.timers]]` TOML array in agent config
- New `_INTERRUPT` topic handling in watcher/injector (analogous to existing `_RESTART`)
- New `inject_interrupt()` function in injector that sends per-bot cancel keys before pasting
- Agent activity state tracking (`busy`/`idle`) via tmux pane content diffing
- Optional `include_agents` array per timer in `agents.toml`
- New logger events for timer fires, interrupt injections, and activity state changes
- Tests for all new functionality

### Out of scope

- Web/UI dashboard for agent status

## Current State

### Affected Files

| File | Current Purpose | Changes Needed |
|------|----------------|----------------|
| `orchestrator/src/config.rs` | Parses `agents.toml`, defines `AgentEntry` | Add `timers` field to `AgentEntry`; add `TimerEntry` struct with `include_agents` |
| `orchestrator/src/injector.rs` | tmux injection via `load-buffer`/`paste-buffer`/`send-keys Enter` | Add `InterruptKeys` per-bot strategy; add `inject_interrupt()` with cancel/clear/inject sequence |
| `orchestrator/src/supervisor.rs` | Agent lifecycle, health loop, transcript loop | Add activity state tracking; add `timer_loop()` background task; expose `agent_activity()` |
| `orchestrator/src/watcher.rs` | Message routing, `_RESTART` topic handling | Add `_INTERRUPT` topic handling; append agent status footer for timers with `include_agents` |
| `orchestrator/src/logger.rs` | Structured JSON event logging | Add `TimerFired`, `MessageInterrupted`, `AgentActivityChanged` events |
| `orchestrator/src/main.rs` | CLI entry point | Spawn `timer_loop` alongside `health_loop` and `transcript_loop` |
| `orchestrator/tests/config_tests.rs` | Config parsing tests | Add tests for timer parsing including `include_agents` validation |
| `orchestrator/tests/watcher_tests.rs` | Message routing tests | Add tests for `_INTERRUPT` routing and status footer |
| `orchestrator/tests/supervisor_tests.rs` | Supervisor lifecycle tests | Add tests for activity detection and timer loop |
| `orchestrator/tests/injector_tests.rs` | Injector tmux tests | Add tests for `inject_interrupt` |

### Key Findings

- Current injection (`inject_once` in `injector.rs`) uses `load-buffer` → `paste-buffer -p` → sleep(1s) → `send-keys Enter`. No interrupt logic exists.
- `_RESTART` topic is handled as a special case in `watcher.rs:route_message()` — we follow the same pattern for `_INTERRUPT`.
- `transcript_loop` already captures pane content every 30 seconds via `injector.capture()`. Activity detection can piggyback on this mechanism by comparing consecutive captures.
- The `InjectorOps` trait must be extended with `inject_interrupt()` to keep testability via `MockInjector`.
- All 5 supported CLI bots (claude, codex, copilot, gemini, cursor agent) can be interrupted, but use **different keys**: Copilot uses `Esc`, the rest use `Ctrl+C`. See Section 3 for the per-bot interrupt strategy.

## Technical Design

### 1. Timer Configuration (`config.rs`)

New TOML schema for per-agent timers:

```toml
[[agents]]
id = "coder"
command = "claude"
prompt_file = "prompts/coder.md"
allowed_write_dirs = ["orchestrator/src/"]

[[agents.timers]]              # NEW: optional array
minutes = 15
prompt_file = "prompts/reminders/run_tests.md"

[[agents.timers]]
minutes = 30
prompt_file = "prompts/reminders/progress_summary.md"
interrupt = true               # optional, default false — force-interrupt before injection
include_agents = ["tester", "reviewer"]  # optional — append these agents' activity status to the timer prompt
```

`include_agents` is an optional array of agent IDs. When present, the timer prompt will have a status footer appended showing the current activity state (`busy`/`idle`/`unknown`) of the listed agents. This lets you configure exactly which agents' states matter for a given reminder — e.g. a "run tests" timer might only care about the tester, while a "progress summary" timer might want all agents.

New structs:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct TimerEntry {
    pub minutes: u64,
    pub prompt_file: String,
    #[serde(default)]
    pub interrupt: bool,
    #[serde(default)]
    pub include_agents: Vec<String>,  // agent IDs whose status to append
}

// Added field to AgentEntry:
pub struct AgentEntry {
    // ... existing fields ...
    #[serde(default)]
    pub timers: Vec<TimerEntry>,
}
```

Validation in `load()`: every ID in `include_agents` must match an `id` in `[[agents]]`.

Template variables in timer prompt files: `{{project_root}}`, `{{agent_id}}`, `{{messages_dir}}` (same as startup prompts).

### 2. Orchestrator-Managed Message Queue (`watcher.rs`)

**Core principle**: never inject directly into the tmux input buffer. Instead, all messages (file-based and timer-generated) are enqueued in an in-memory per-agent queue owned by the orchestrator. A continuous drain loop watches for idle agents and injects **one message at a time** only when the agent is ready.

This eliminates the "enqueued messages block interrupts" problem entirely — the orchestrator controls what's in the queue, so it can flush, reorder, or concatenate at will before anything touches tmux.

#### Queue structure

```rust
/// Per-agent FIFO queue managed by the orchestrator.
/// Messages are only injected into tmux when the agent is idle.
struct AgentQueue {
    /// Pending messages waiting to be injected.
    pending: VecDeque<QueuedMessage>,
}

struct QueuedMessage {
    meta: MessageMeta,       // parsed filename metadata
    framed_content: String,  // ready-to-inject text with header
    is_interrupt: bool,      // true for _INTERRUPT topic messages
    include_agents: Vec<String>,  // agent IDs whose status to append (from timer config)
}
```

#### Drain loop

A continuously-running async loop (separate from the routing loop) checks agent activity and drains queues:

```rust
/// Runs forever. Checks agent activity every ~2s and drains one message
/// per idle agent per iteration.
async fn drain_loop(self: Arc<Self>) {
    loop {
        sleep(Duration::from_secs(2)).await;

        let mut queues = self.queues.lock().await;
        for (agent_id, queue) in queues.iter_mut() {
            if queue.pending.is_empty() {
                continue;
            }

            let activity = self.registry.agent_activity(agent_id).await;
            if activity == AgentActivity::Idle {
                // Agent is waiting for input — safe to inject
                if let Some(msg) = queue.pending.pop_front() {
                    let session = self.registry.session_for(agent_id).await;
                    if let Some(session) = session {
                        drop(queues); // release lock during injection
                        let _ = self.injector.inject(&session, &msg.framed_content).await;
                        queues = self.queues.lock().await;
                    }
                }
            }
        }
    }
}
```

#### Interrupt: flush + concatenate + force-inject

When an `_INTERRUPT` message arrives, the drain loop is bypassed entirely:

1. **Drain the queue** — pop all pending messages for this agent
2. **Concatenate** — combine all pending message contents into a single payload, with the interrupt message appended last (it's the most important one)
3. **Ctrl+C** the agent to cancel current generation
4. **Inject the combined payload** as a single tmux paste

```rust
async fn handle_interrupt(&self, agent_id: &str, interrupt_msg: QueuedMessage) {
    let mut queues = self.queues.lock().await;
    let queue = queues.entry(agent_id.to_string()).or_default();

    // 1. Drain all pending messages
    let pending: Vec<QueuedMessage> = queue.pending.drain(..).collect();
    drop(queues);

    // 2. Build combined payload
    let mut combined = String::new();
    if !pending.is_empty() {
        combined.push_str("--- QUEUED MESSAGES (delivered early due to interrupt) ---\n\n");
        for (i, msg) in pending.iter().enumerate() {
            combined.push_str(&format!("=== Message {}/{} ===\n", i + 1, pending.len()));
            combined.push_str(&msg.framed_content);
            combined.push_str("\n\n");
        }
        combined.push_str("--- END QUEUED MESSAGES ---\n\n");
    }
    // Interrupt message comes last — it's the most important
    combined.push_str(&interrupt_msg.framed_content);

    // 3. Cancel current work with bot-specific keys, then inject
    let session = self.registry.session_for(agent_id).await;
    let keys = self.registry.interrupt_keys_for(agent_id).await;
    if let (Some(session), Some(keys)) = (session, keys) {
        let _ = self.injector.inject_interrupt(&session, &combined, &keys).await;
    }
}
```

This means:
- **Normal messages**: enqueued, injected one-at-a-time when agent is idle. Nothing ever piles up in tmux's input buffer.
- **Interrupt messages**: all pending messages are concatenated together with the interrupt message at the end, Ctrl+C fires, single combined payload is injected. No queued messages are lost; the urgent instruction arrives last (which is what the agent reads most recently and acts on).

#### Routing change

The `routing_loop` no longer calls `inject()` directly. It always enqueues:

```rust
// In routing_loop, replace direct injection with:
let is_interrupt = meta.topic.to_ascii_lowercase().ends_with("_interrupt")
    || meta.topic.eq_ignore_ascii_case("_interrupt");

let queued = QueuedMessage {
    meta: meta.clone(),
    framed_content: framed,
    is_interrupt,
};

if is_interrupt {
    // Bypass the queue — flush + concatenate + force-inject immediately
    self.handle_interrupt(&meta.recipient, queued).await;
} else {
    // Enqueue for the drain loop to pick up when agent is idle
    let mut queues = self.queues.lock().await;
    queues.entry(meta.recipient.clone()).or_default().pending.push_back(queued);
}

// Move file to processed
let _ = std::fs::rename(&meta.path, self.processed_dir.join(&meta.filename));
```

### 3. Per-Bot Interrupt Strategy (`injector.rs`)

The interrupt key sequence is **not universal** across bots. Each bot has its own TUI with different keybindings:

| Bot (`command`) | Cancel key | Clear input key | Notes |
|-----------------|-----------|----------------|-------|
| `claude` | `C-c` | `C-u` | Standard readline-style input |
| `codex` | `C-c` | `C-u` | Standard readline-style input |
| `copilot` | `Escape` | `Escape` | Custom TUI; Esc stops current work and clears |
| `gemini` | `C-c` | `C-c` | Custom TUI with vim-mode editor; second Ctrl+C clears |
| `cursor agent` | `C-c` | `C-c` | Custom TUI; Enter queues, Cmd+Enter sends immediately |

Derive the cancel/clear keys from the `command` field in `AgentEntry`:

```rust
pub struct InterruptKeys {
    pub cancel: &'static str,     // tmux send-keys value to cancel generation
    pub clear: &'static str,      // tmux send-keys value to clear partial input
    pub settle_ms: u64,           // ms to wait after cancel before injecting
}

impl InterruptKeys {
    pub fn for_command(command: &str) -> Self {
        match command.split_whitespace().next().unwrap_or("") {
            "copilot" => InterruptKeys { cancel: "Escape", clear: "Escape", settle_ms: 2000 },
            "gemini"  => InterruptKeys { cancel: "C-c", clear: "C-c", settle_ms: 2000 },
            "cursor"  => InterruptKeys { cancel: "C-c", clear: "C-c", settle_ms: 2000 },
            _         => InterruptKeys { cancel: "C-c", clear: "C-u", settle_ms: 2000 },
        }
    }
}
```

The injector's `inject_interrupt` uses the bot-specific keys:

```rust
/// Interrupt the agent's current generation, wait for it to settle,
/// then inject the given text. Uses bot-specific cancel/clear keys.
fn inject_interrupt_once(session: &str, text: &str, keys: &InterruptKeys) -> Result<(), InjectionError> {
    let run_tmux = |args: &[&str]| -> Result<(), InjectionError> { /* same helper */ };

    // 1. Send cancel key to stop current generation
    run_tmux(&["send-keys", "-t", session, keys.cancel])?;

    // 2. Wait for agent to process the interrupt
    std::thread::sleep(std::time::Duration::from_millis(keys.settle_ms));

    // 3. Clear any partial input left on the line
    run_tmux(&["send-keys", "-t", session, keys.clear])?;
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 4. Inject the combined payload
    inject_once(session, text)
}
```

Add to `InjectorOps` trait:

```rust
pub trait InjectorOps: Send + Sync {
    // ... existing methods ...
    fn inject_interrupt<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
        keys: &'a InterruptKeys,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>>;
}
```

The watcher/queue must pass the correct `InterruptKeys` when calling `inject_interrupt`. Derive from the agent's `command` at startup and store alongside the tmux session name in the queue metadata.

### 4. Agent Activity Detection (`supervisor.rs`)
    Busy,
    Idle,
    Unknown,
}

// New fields on AgentState:
pub struct AgentState {
    // ... existing fields ...
    #[serde(skip)]
    pub activity: AgentActivity,
    #[serde(skip)]
    last_pane_hash: Option<String>,
    #[serde(skip)]
    stable_count: u32,  // consecutive polls with same hash
}
```

**Detection logic** — integrated into the existing `transcript_loop` (already runs every 30s, already calls `capture()`):

```rust
// Inside transcript_loop, after capturing pane content:
let current_hash = format!("{:x}", Sha256::digest(content.as_bytes()));
let mut agents = self.agents.lock().await;
if let Some(state) = agents.get_mut(&id) {
    let old_activity = state.activity.clone();
    match &state.last_pane_hash {
        Some(prev) if *prev == current_hash => {
            state.stable_count += 1;
            if state.stable_count >= 2 {  // ~60s of no change → idle
                state.activity = AgentActivity::Idle;
            }
        }
        _ => {
            state.stable_count = 0;
            state.activity = AgentActivity::Busy;
        }
    }
    state.last_pane_hash = Some(current_hash);
    if state.activity != old_activity {
        // Log the transition
        self.logger.log(Event::AgentActivityChanged {
            agent_id: id.clone(),
            activity: state.activity.clone(),
        });
    }
}
```

Expose a public method for the watcher to query specific agents' activity:

```rust
impl Registry {
    /// Return the current activity state for the given agent IDs.
    pub async fn agent_activities(&self, ids: &[String]) -> Vec<(String, AgentActivity)> {
        let agents = self.agents.lock().await;
        ids.iter()
            .filter_map(|id| {
                agents.get(id.as_str())
                    .map(|state| (id.clone(), state.activity.clone()))
            })
            .collect()
    }
}
```

### 5. `_INTERRUPT` Topic Detection (`watcher.rs`)

The `_INTERRUPT` topic is detected in `route_message()` — analogous to existing `_RESTART` — but instead of calling the injector directly, it triggers the queue's `handle_interrupt()` flow (see Section 2):

```rust
// After the existing _RESTART check:
let is_interrupt = meta.topic.to_ascii_lowercase().ends_with("_interrupt")
    || meta.topic.eq_ignore_ascii_case("_interrupt");

// Route through the orchestrator queue (Section 2):
if is_interrupt {
    // Bypass queue — flush pending, concatenate, Ctrl+C, inject combined payload
    self.handle_interrupt(&meta.recipient, queued_msg).await;
} else {
    // Enqueue — drain loop will inject when agent is idle
    let mut queues = self.queues.lock().await;
    queues.entry(meta.recipient.clone()).or_default().pending.push_back(queued_msg);
}
```

Message filename examples:
- Normal (queued): `...topic-review_feedback.md` → enqueued, injected when agent is idle
- Interrupt (immediate): `...topic-urgent_review_INTERRUPT.md` → flushes queue, Ctrl+C, injects combined payload

### 6. Agent Status Footer (timer-driven)

When a timer has `include_agents` configured, the status footer is appended to the timer prompt at injection time (in the `drain_loop`, not at enqueue time, so the activity snapshot is fresh).

Example output for a timer with `include_agents = ["tester", "reviewer"]`:

```
--- TIMER REMINDER (30 min) ---

[timer prompt content here]

--- AGENT STATUS ---
tester: busy
reviewer: idle
---
```

Implementation in `drain_loop()`, when injecting a queued timer message:

```rust
// At injection time in drain_loop, if the queued message has include_agents:
if !queued.include_agents.is_empty() {
    let activities = self.registry.agent_activities(&queued.include_agents).await;
    if !activities.is_empty() {
        let status_lines: Vec<String> = activities.iter()
            .map(|(id, act)| format!("{}: {}", id, act))
            .collect();
        payload.push_str(&format!(
            "\n\n--- AGENT STATUS ---\n{}\n---",
            status_lines.join("\n")
        ));
    }
}
```

New method on `Registry`:

```rust
impl Registry {
    /// Return the current activity state for the given agent IDs.
    pub async fn agent_activities(&self, ids: &[String]) -> Vec<(String, AgentActivity)> {
        let agents = self.agents.lock().await;
        ids.iter()
            .filter_map(|id| {
                agents.get(id.as_str())
                    .map(|state| (id.clone(), state.activity.clone()))
            })
            .collect()
    }
}
```

The `QueuedMessage` struct carries the `include_agents` list so the drain loop knows which agents to query:

```rust
struct QueuedMessage {
    meta: MessageMeta,
    framed_content: String,
    is_interrupt: bool,
    include_agents: Vec<String>,  // empty for file-based messages; populated for timers
}
```

### 7. Timer Loop (`supervisor.rs`)

New async loop that fires timer prompts at their configured intervals. Timer prompts are routed through the orchestrator queue (Section 2) — they are **never** injected directly into tmux:

```rust
pub async fn timer_loop(self, timer_configs: Vec<TimerConfig>, watcher: Arc<MessageWatcher>) {
    // TimerConfig = { agent_id, minutes, prompt_content, interrupt }
    let mut last_fired: Vec<Instant> = vec![Instant::now(); timer_configs.len()];

    loop {
        sleep(Duration::from_secs(30)).await;  // check every 30s

        let now = Instant::now();
        for (i, timer) in timer_configs.iter().enumerate() {
            let interval = Duration::from_secs(timer.minutes * 60);
            if now.duration_since(last_fired[i]) >= interval {
                let framed = format!(
                    "--- TIMER REMINDER ({} min) ---\n\n{}",
                    timer.minutes, timer.prompt_content
                );

                // Route through the orchestrator queue, not directly into tmux
                let queued = QueuedMessage {
                    meta: timer_meta(&timer),
                    framed_content: framed,
                    is_interrupt: timer.interrupt,
                };

                if timer.interrupt {
                    watcher.handle_interrupt(&timer.agent_id, queued).await;
                } else {
                    watcher.enqueue(&timer.agent_id, queued).await;
                }

                self.logger.log(Event::TimerFired {
                    agent_id: timer.agent_id.clone(),
                    minutes: timer.minutes,
                    interrupt: timer.interrupt,
                });

                last_fired[i] = now;
            }
        }
    }
}
```

### 8. New Logger Events (`logger.rs`)

```rust
#[serde(rename = "timer_fired")]
TimerFired {
    agent_id: String,
    minutes: u64,
    interrupt: bool,
},

#[serde(rename = "message_interrupted")]
MessageInterrupted {
    filename: String,
    recipient: String,
    pending_flushed: usize,  // number of queued messages that were concatenated
},

#[serde(rename = "agent_activity_changed")]
AgentActivityChanged {
    agent_id: String,
    activity: String,  // "busy", "idle", "unknown"
},
```

### 9. Spike Interrupt Mode (`spike.rs`, `main.rs`)

The existing `orchestrator spike` command validates basic tmux injection (spawn, inject a prompt, verify file output, 10-payload burst, crash recovery). A new `--interrupt` flag extends this to empirically test the per-bot interrupt key sequences from Section 3.

#### CLI Change (`main.rs`)

```rust
Commands::Spike {
    #[arg(default_value = ".")]
    path: PathBuf,
    #[arg(long)]
    agent: Option<String>,
    /// Test interrupt key sequences instead of normal injection
    #[arg(long)]
    interrupt: bool,
},
```

When `--interrupt` is passed, `main.rs` calls `spike::run_spike_interrupt()` instead of `spike::run_spike()`.

#### Flow: `run_spike_interrupt()` (`spike.rs`)

The interrupt spike validates that the cancel/clear key sequence for a given bot actually works in a live tmux session. It follows the same structure as the existing spike (resolve agent, spawn session, run test, report results).

**Steps:**

1. **Resolve agent & spawn session** — identical to existing spike (reuse `run_spike_with_deps` resolution logic).

2. **Inject a long-running prompt** — send the agent a task that will keep it busy for at least 30 seconds:
   ```
   "List every file in the entire filesystem recursively and print all paths. 
    Do not stop until you have listed every single file."
   ```
   This ensures the agent is actively generating when we try to interrupt it.

3. **Wait for agent to start working** — sleep `timings.agent_init_delay` (8s default), then verify the pane content is changing (agent is busy).

4. **Send interrupt keys** — derive `InterruptKeys::for_command(&agent.command)` and execute the cancel/clear sequence via tmux:
   ```rust
   let keys = InterruptKeys::for_command(&agent.command);
   // Cancel
   tmux send-keys -t {session} {keys.cancel}
   sleep(keys.settle_ms)
   // Clear
   tmux send-keys -t {session} {keys.clear}
   sleep(500ms)
   ```

5. **Verify agent returned to prompt** — poll pane content for up to `timings.poll_max_rounds` iterations. The agent is "back at prompt" when the pane content stabilizes (same hash for 2+ consecutive captures) AND the last line looks like a prompt (contains `>`, `$`, `❯`, or the agent's known prompt indicator). Log `SpikeInterruptConfirmed` or `SpikeInterruptFailed`.

6. **Post-interrupt injection test** — inject a validation prompt identical to the existing spike ("Write a file containing exactly 'spike interrupt test passed' to ..."). Poll for the output file. This confirms the agent accepts new input after being interrupted.

7. **Report results** — print pass/fail for each phase: interrupt acknowledged, prompt recovered, post-interrupt injection accepted.

#### New `SpikeTimings` Fields

```rust
pub struct SpikeTimings {
    // ... existing fields ...
    /// How long to wait after injecting the busy-prompt before sending interrupt
    pub interrupt_busy_delay: Duration,   // default: 10s
}

impl Default for SpikeTimings {
    fn default() -> Self {
        Self {
            // ... existing defaults ...
            interrupt_busy_delay: Duration::from_secs(10),
        }
    }
}
```

#### New Logger Events

```rust
#[serde(rename = "spike_interrupt_sent")]
SpikeInterruptSent {
    agent_id: String,
    cancel_key: String,
    clear_key: String,
},

#[serde(rename = "spike_interrupt_confirmed")]
SpikeInterruptConfirmed {
    agent_id: String,
    detail: String,
},

#[serde(rename = "spike_interrupt_failed")]
SpikeInterruptFailed {
    agent_id: String,
    detail: String,
},
```

#### Example Output

```
=== orchestrator spike --interrupt ===
Project: /Users/josh/code/myproject
Agent:   coder (command: 'claude')
Session: myproject-coder
Interrupt keys: cancel=C-c, clear=C-u, settle=2000ms

Spawning tmux session 'myproject-coder' running 'claude'...
Session spawned. Waiting for agent to initialize...
Injecting long-running prompt to keep agent busy...
Waiting 10s for agent to start generating...
  Agent is generating (pane content changing).

Sending interrupt: C-c...
Waiting 2000ms for agent to settle...
Sending clear: C-u...
Waiting 500ms...

Polling for prompt recovery...
  INTERRUPT PASSED — agent returned to prompt after 4s.

Injecting post-interrupt validation prompt...
Waiting for agent to act...
  POST-INTERRUPT INJECTION PASSED — file written by agent.

Spike interrupt complete. tmux session 'myproject-coder' left running.
  Logs: .orchestrator/runtime/logs/spike_events.jsonl
  Transcripts: .orchestrator/runtime/logs/spike_transcripts/
```

## Implementation Checklist

### Phase 0: Spike Interrupt Mode (de-risk first)

- [ ] Add `--interrupt` flag to `Commands::Spike` in `main.rs`
- [ ] Add `interrupt_busy_delay` field to `SpikeTimings` (default 10s)
- [ ] Add `InterruptKeys` struct and `for_command()` to `injector.rs`
- [ ] Add `SpikeInterruptSent`, `SpikeInterruptConfirmed`, `SpikeInterruptFailed` events to `logger.rs`
- [ ] Implement `run_spike_interrupt()` in `spike.rs` (spawn, busy-prompt, cancel/clear, verify prompt recovery, post-interrupt inject)
- [ ] Add `inject_interrupt_once()` to `injector.rs` (used by spike directly, later by queue)
- [ ] Run spike --interrupt against each bot to validate keys empirically
- [ ] Add spike interrupt unit test in `spike_tests.rs` with `MockInjector`

### Phase 1: Config & Data Model

- [ ] Add `TimerEntry` struct to `config.rs`
- [ ] Add `timers: Vec<TimerEntry>` field to `AgentEntry` (serde default empty vec)
- [ ] Add `include_agents: Vec<String>` field to `TimerEntry` (serde default empty vec)
- [ ] Validate `include_agents` entries reference valid agent IDs in `load()`
- [ ] Validate timer entries (minutes > 0, prompt_file exists) in `load()`
- [ ] Render template variables in timer prompt files
- [ ] Update default `agents.toml` template with commented-out timer examples
- [ ] Add config parsing tests for timers including `include_agents`

### Phase 2: Activity Detection

- [ ] Add `AgentActivity` enum (`Busy`, `Idle`, `Unknown`) to `supervisor.rs`
- [ ] Add `activity`, `last_pane_hash`, `stable_count` fields to `AgentState`
- [ ] Integrate activity detection into `transcript_loop` using pane content hashing
- [ ] Add `agent_activity()` method to `Registry` (single agent lookup)
- [ ] Add `agent_activities()` method to `Registry` (lookup by list of IDs)
- [ ] Add `AgentActivityChanged` event to `logger.rs`
- [ ] Add activity tests to `supervisor_tests.rs`

### Phase 3: Orchestrator-Managed Queue

- [ ] Add `QueuedMessage` struct and `AgentQueue` to `watcher.rs`
- [ ] Refactor `routing_loop` to always enqueue instead of injecting directly
- [ ] Implement `drain_loop()` — continuous async loop that injects one message per idle agent per iteration
- [ ] Implement `handle_interrupt()` — flush queue, concatenate pending + interrupt, Ctrl+C, inject combined
- [ ] Implement `enqueue()` public method for timer loop to push messages
- [ ] Move file to `processed/` at enqueue time (not injection time)
- [ ] Add queue tests: enqueue, drain on idle, interrupt flushes and concatenates

### Phase 4: Interrupt Injection

- [ ] Add `InterruptKeys` struct to `injector.rs` with `for_command()` constructor
- [ ] Add `inject_interrupt_once()` function to `injector.rs` (takes `InterruptKeys`)
- [ ] Add `inject_interrupt()` async function with retry logic
- [ ] Add `inject_interrupt()` to `InjectorOps` trait (takes `InterruptKeys` param)
- [ ] Implement `inject_interrupt()` on `RealInjector`
- [ ] Add `inject_interrupt()` to `MockInjector` in tests
- [ ] Add `MessageInterrupted` event to `logger.rs`
- [ ] Add injector interrupt tests for each bot type (claude, copilot, gemini, cursor agent)

### Phase 5: `_INTERRUPT` Topic & Status Footer

- [ ] Add `_INTERRUPT` topic detection in `watcher.rs:route_message()`
- [ ] Route `_INTERRUPT` messages through `handle_interrupt()` (queue flush + concatenate)
- [ ] Route normal messages through `enqueue()` (drain loop handles injection)
- [ ] Append agent status footer at drain time when `include_agents` is non-empty on the queued message
- [ ] Add watcher tests for `_INTERRUPT` routing with queue concatenation
- [ ] Add watcher tests for status footer injection based on `include_agents`

### Phase 6: Timer Loop

- [ ] Build `TimerConfig` structs from parsed `AgentEntry` timers in `main.rs`
- [ ] Implement `timer_loop()` in `supervisor.rs` — routes through watcher queue, not direct injection
- [ ] Spawn `timer_loop` as background tokio task in `main.rs` (alongside health/transcript)
- [ ] Add `TimerFired` event to `logger.rs`
- [ ] Add timer loop tests with `tokio::time::pause()` + `advance()`

### Phase 7: Integration & Polish

- [ ] Update `status` CLI command to show agent activity state
- [ ] Update `state.json` to include activity (for external tooling)
- [ ] End-to-end test: timer fires, interrupt injects, queue concatenates, status footer appears
- [ ] Update CLAUDE.md with new config format documentation

## Testing Plan

### Unit Tests

| Test Case | File | Description |
|-----------|------|-------------|
| `parse_timer_config` | `config_tests.rs` | Parse `agents.toml` with timers array |
| `parse_timer_config_empty` | `config_tests.rs` | Parse `agents.toml` without timers (defaults to empty) |
| `parse_timer_include_agents` | `config_tests.rs` | Parse timer with `include_agents = ["tester"]` |
| `timer_validates_include_agents` | `config_tests.rs` | Reject timer with `include_agents` referencing unknown agent ID |
| `timer_validates_minutes` | `config_tests.rs` | Reject timer with `minutes = 0` |
| `timer_validates_prompt_file` | `config_tests.rs` | Reject timer pointing to missing prompt |
| `inject_interrupt_sends_ctrl_c` | `injector_tests.rs` | Verify `C-c` is sent for claude/codex before message paste |
| `inject_interrupt_sends_esc_for_copilot` | `injector_tests.rs` | Verify `Escape` is sent for copilot |
| `inject_interrupt_keys_for_cursor` | `injector_tests.rs` | Verify correct keys for `cursor agent` |
| `interrupt_keys_for_command` | `injector_tests.rs` | `InterruptKeys::for_command()` returns correct keys per bot |
| `activity_detection_busy` | `supervisor_tests.rs` | Changing pane content → `Busy` |
| `activity_detection_idle` | `supervisor_tests.rs` | Stable pane content for 2+ polls → `Idle` |
| `agent_activities` | `supervisor_tests.rs` | Returns activity for requested agent IDs |
| `queue_enqueue_and_drain` | `watcher_tests.rs` | Enqueue message, agent becomes idle, drain loop injects it |
| `queue_interrupt_flushes` | `watcher_tests.rs` | `_INTERRUPT` drains pending, concatenates all into single payload |
| `queue_interrupt_no_pending` | `watcher_tests.rs` | `_INTERRUPT` with empty queue just Ctrl+C + injects interrupt alone |
| `queue_concatenation_format` | `watcher_tests.rs` | Verify combined payload format: queued messages header → each message → interrupt at end |
| `queue_does_not_inject_while_busy` | `watcher_tests.rs` | Drain loop skips agents with `Busy` activity |
| `route_interrupt_suffix` | `watcher_tests.rs` | `topic-review_INTERRUPT` detected as interrupt |
| `route_normal_message` | `watcher_tests.rs` | Normal topic enqueues (does not inject directly) |
| `status_footer_appended` | `watcher_tests.rs` | Timer with `include_agents=["tester"]` appends footer at drain time |
| `status_footer_omitted` | `watcher_tests.rs` | Timer with empty `include_agents` (default) has no footer |
| `timer_fires_at_interval` | `supervisor_tests.rs` | Timer fires after configured minutes, uses `tokio::time::pause()` |
| `timer_routes_through_queue` | `supervisor_tests.rs` | Timer prompt goes through watcher queue, not direct injection |
| `timer_interrupt_flag` | `supervisor_tests.rs` | Timer with `interrupt = true` calls `handle_interrupt()` via queue |
| `spike_interrupt_sends_keys` | `spike_tests.rs` | `run_spike_interrupt` calls `inject_interrupt_once` with correct `InterruptKeys` for agent command |
| `spike_interrupt_verifies_recovery` | `spike_tests.rs` | Passes when pane stabilizes after interrupt; fails when pane keeps changing |
| `spike_interrupt_post_inject` | `spike_tests.rs` | After interrupt, normal `inject()` succeeds and agent writes validation file |

### Integration Tests

| Test Case | Description |
|-----------|-------------|
| Timer + activity | Timer fires, agent is busy → queued; agent becomes idle → drained |
| Interrupt + pending queue | 3 messages queued + interrupt arrives → combined payload with interrupt last |
| Message + status footer | Timer fires with `include_agents=["tester", "reviewer"]`, verify footer contains those agents' states |
| _INTERRUPT + _RESTART coexist | Both special topics work independently in same session |
| spike --interrupt per bot | Run `spike --interrupt --agent X` against each configured bot type to validate keys empirically |

## Success Criteria

- [ ] Messages are never injected directly into tmux — always routed through orchestrator queue
- [ ] Drain loop injects one message per idle agent per cycle (~2s polling)
- [ ] `_INTERRUPT` messages flush the queue, concatenate pending + interrupt, and force-inject immediately
- [ ] Timers fire at configured intervals (±30s tolerance from polling)
- [ ] Agent activity is accurately tracked as `busy`/`idle` based on pane output
- [ ] Status footer is appended when timer has `include_agents` configured
- [ ] All existing tests continue to pass (no regressions)
- [ ] `cargo test` passes with new tests covering all new functionality
- [ ] `cargo clippy` passes with no new warnings
- [ ] `spike --interrupt` passes for each supported bot type (cancel → prompt recovery → post-interrupt inject)

## Open Questions

1. **Activity detection granularity**: The current transcript loop runs every 30s. The drain loop polls every 2s, but it relies on activity state from transcript captures. Should activity detection run more frequently (e.g. every 5s) in a separate sub-loop for faster busy→idle transitions? Trade-off: more tmux capture calls vs faster drain response.

2. **Interrupt settling delay**: The 2000ms default wait after cancel should be sufficient for all bots, but may need tuning per bot. The `InterruptKeys::settle_ms` field is already per-bot configurable.

3. **Timer prompt interaction with activity**: Should timers skip firing if the agent is idle (already waiting for input, no work to interrupt)? Or always fire regardless?

4. **Copilot/Cursor native enqueueing**: Copilot CLI and Cursor Agent CLI support native message enqueueing (just sending text while busy; Cursor has Enter to queue, Cmd+Enter to send immediately). Should we detect these bot types and use enqueue-style delivery instead of the queue+drain approach? (marked out of scope for this step, but worth discussing)

5. **Queue persistence**: Should the in-memory queue be persisted to disk (e.g. in `runtime/state.json`) so messages survive an orchestrator restart? Currently, pending messages would be lost if the orchestrator crashes.

6. **`Unknown` activity on startup**: Activity starts as `Unknown` and stays that way until the transcript loop has run 2+ captures (~60s). During this window, the drain loop won't inject any queued messages (it only drains on `Idle`). Should the drain loop treat `Unknown` as `Idle` to avoid a startup delay? Or should it wait until activity detection has initialized?
