use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::config::{ResolvedTimer, SplitDirection};
use crate::injector::{InterruptKeys, InjectorOps, RealInjector};
use crate::logger::{Event, Logger};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const HEALTH_POLL_INTERVAL_SECS: u64 = 2;
const MAX_RESTARTS_IN_WINDOW: u32 = 5;
const RESTART_WINDOW_SECS: i64 = 120; // 2 minutes
const AGENT_INIT_DELAY_SECS: u64 = 5;
const TRANSCRIPT_INTERVAL_SECS: u64 = 30;
const ACTIVITY_POLL_INTERVAL_SECS: u64 = 3;
const ATTENTION_POLL_INTERVAL_SECS: u64 = 3;
/// Number of trailing lines to scan for interactive prompts. Prompts always
/// appear at the very bottom of the terminal so we keep this small to avoid
/// matching prompt-like text that scrolled past in earlier output.
const ATTENTION_SCAN_LINES: usize = 4;

// ---------------------------------------------------------------------------
// Attention detection
// ---------------------------------------------------------------------------

/// Returns the set of terminal patterns that indicate the given CLI tool is
/// waiting for keyboard input from the user.
///
/// Patterns are matched against the last [`ATTENTION_SCAN_LINES`] lines of
/// the pane content so that prompt text embedded in earlier output is ignored.
///
/// IMPORTANT: Only include strings that appear literally at the interactive
/// prompt line itself — NOT phrases the agent might generate in its output.
/// When in doubt, leave the pattern out; false alerts are worse than a missed
/// one.
fn attention_patterns(cli_command: &str) -> &'static [&'static str] {
    match cli_command.split_whitespace().next().unwrap_or("") {
        "claude" => &[
            // The exact permission prompt Claude Code shows
            "Allow once",
            "Allow always",
            "Bypass permissions",
        ],
        "codex" => &[
            // Codex shows different prompt formats depending on context
            "(y/a/x/e/n)",
            "Yes, proceed (y)",
            "Press enter to confirm or esc to cancel",
        ],
        "copilot" => &[
            // Copilot CLI shows an interactive selection menu for permission prompts.
            // The navigation hint sits at the very bottom of the menu box.
            "Allow for the rest of the session",
            "to navigate",
        ],
        "cursor" => &[
            "Skip and Continue",
            "Run this command?",
            "Skip (esc or n)",
        ],
        "gemini" => &[
            // Gemini exact approval prompt
            "(y/n/always)",
        ],
        _ => &[
            // Fallback — very conservative, only exact bracket prompts on their own
            "(y/a/x/e/n)",
            "(y/n/always)",
        ],
    }
}

/// Scan the tail of a captured pane for any known attention pattern.
/// Returns the first matched pattern string, or `None` if none found.
fn detect_attention_pattern(content: &str, cli_command: &str) -> Option<&'static str> {
    let tail: String = content
        .lines()
        .rev()
        .take(ATTENTION_SCAN_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");

    for pattern in attention_patterns(cli_command) {
        if tail.contains(pattern) {
            return Some(pattern);
        }
    }
    None
}

/// Print a high-visibility attention banner to the orchestrator terminal.
fn print_attention_alert(agent_id: &str, tmux_session: &str, pattern: &str) {
    use std::io::Write;
    let banner = format!(
        "\n\
         ╔══════════════════════════════════════════════╗\n\
         ║  ⚠  AGENT NEEDS INPUT                       ║\n\
         ║  agent:   {:<35}║\n\
         ║  session: {:<35}║\n\
         ║  matched: {:<35}║\n\
         ╚══════════════════════════════════════════════╝\n",
        agent_id, tmux_session, pattern,
    );
    let _ = std::io::stderr().write_all(banner.as_bytes());
}

/// Per-agent state tracked by the attention loop (not persisted to state.json).
struct AgentAttentionState {
    /// Whether attention visuals are currently applied.
    active: bool,
    /// When we last fired an alert (for debounce).
    last_alerted: Option<DateTime<Utc>>,
    /// Pane hash from the previous poll cycle (to detect "stuck" state).
    prev_hash: Option<u64>,
    /// Number of consecutive polls where the hash has been stable.
    stable_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent_id: String,
    pub cli_command: String,
    /// The tmux session name — used for session-level operations (has-session, kill-session).
    pub tmux_session: String,
    /// Full tmux target for inject/capture. For standalone agents this equals
    /// `tmux_session`; for grouped agents it includes the pane index, e.g.
    /// `myproject-worker-1:0.1`.
    pub tmux_target: String,
    pub inbox_dir: PathBuf,
    pub allowed_write_dirs: Vec<PathBuf>,
}

/// A resolved worker group ready for the supervisor to spawn as a single
/// tmux session with multiple panes.
#[derive(Debug, Clone)]
pub struct WorkerGroupConfig {
    pub group_id: String,
    pub session_name: String,
    pub layout: SplitDirection,
    /// Ordered list of agents in this group (pane 0, 1, 2, ...).
    pub members: Vec<AgentConfig>,
}

// ---------------------------------------------------------------------------
// Agent state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Healthy,
    Degraded,
    Dead,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::Healthy => write!(f, "healthy"),
            AgentStatus::Degraded => write!(f, "degraded"),
            AgentStatus::Dead => write!(f, "dead"),
        }
    }
}

/// Agent activity state derived from tmux pane content changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentActivity {
    /// Pane content is changing between captures — agent is generating output.
    Busy,
    /// Pane content is stable — agent is waiting at a prompt.
    Idle,
    /// Not enough data to determine (e.g. just spawned).
    Unknown,
}

impl std::fmt::Display for AgentActivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentActivity::Busy => write!(f, "BUSY"),
            AgentActivity::Idle => write!(f, "IDLE"),
            AgentActivity::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub agent_id: String,
    pub tmux_session: String,
    /// Full pane target for inject/capture. Equals `tmux_session` for standalone
    /// agents; includes pane index for grouped agents (e.g. `session:0.1`).
    #[serde(default)]
    pub tmux_target: String,
    pub status: AgentStatus,
    pub restart_count: u32,
    pub last_start: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<DateTime<Utc>>,
    /// Terminal handle opened for this agent session.
    /// On macOS, this is the Terminal.app window ID and is persisted to state.json.
    /// On Linux, this is the terminal emulator PID and is NOT persisted (to avoid
    /// killing unrelated processes if PIDs are reused after a reboot).
    #[serde(alias = "terminal_window_id")]
    #[cfg_attr(target_os = "linux", serde(skip_serializing))]
    #[cfg_attr(
        not(target_os = "linux"),
        serde(skip_serializing_if = "Option::is_none")
    )]
    pub terminal_handle: Option<u32>,
    /// Timestamps of recent restarts (for windowed rate limiting).
    #[serde(skip)]
    pub restart_timestamps: Vec<DateTime<Utc>>,
    /// Current activity state (busy/idle) derived from pane content changes.
    #[serde(default = "default_activity")]
    pub activity: AgentActivity,
    /// Hash of last captured pane content for activity detection.
    #[serde(skip)]
    pub last_pane_hash: Option<u64>,
}

fn default_activity() -> AgentActivity {
    AgentActivity::Unknown
}

// ---------------------------------------------------------------------------
// Registry (shared state across the supervisor + watcher)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Registry {
    pub agents: Arc<Mutex<HashMap<String, AgentState>>>,
    configs: Arc<HashMap<String, AgentConfig>>,
    startup_prompts: Arc<Mutex<HashMap<String, String>>>,
    state_path: PathBuf,
    log_dir: PathBuf,
    logger: Arc<Logger>,
    injector: Arc<dyn InjectorOps>,
}

impl Registry {
    pub fn new(
        configs: Vec<AgentConfig>,
        state_path: PathBuf,
        log_dir: PathBuf,
        logger: Arc<Logger>,
    ) -> Self {
        Self::new_with_injector(configs, state_path, log_dir, logger, Arc::new(RealInjector))
    }

    pub fn new_with_injector(
        configs: Vec<AgentConfig>,
        state_path: PathBuf,
        log_dir: PathBuf,
        logger: Arc<Logger>,
        injector: Arc<dyn InjectorOps>,
    ) -> Self {
        let config_map: HashMap<String, AgentConfig> = configs
            .into_iter()
            .map(|c| (c.agent_id.clone(), c))
            .collect();
        Registry {
            agents: Arc::new(Mutex::new(HashMap::new())),
            configs: Arc::new(config_map),
            startup_prompts: Arc::new(Mutex::new(HashMap::new())),
            state_path,
            log_dir,
            logger,
            injector,
        }
    }

    /// Spawn all configured agents, injecting a startup prompt into each.
    ///
    /// Worker groups are spawned first as multi-pane tmux sessions; standalone
    /// agents are then spawned each in their own session.
    pub async fn spawn_all(
        &self,
        startup_prompts: &HashMap<String, String>,
        worker_groups: &[WorkerGroupConfig],
    ) {
        // Store prompts for later use by restart_agent
        {
            let mut prompts = self.startup_prompts.lock().await;
            *prompts = startup_prompts.clone();
        }

        // Collect agent IDs that belong to a group so we skip them in standalone loop
        let grouped_ids: std::collections::HashSet<String> = worker_groups
            .iter()
            .flat_map(|g| g.members.iter().map(|m| m.agent_id.clone()))
            .collect();

        // --- Phase 1: spawn all sessions (fast, no blocking) ---

        // Spawn worker group sessions (each session = multiple panes)
        for group in worker_groups {
            self.spawn_group(group).await;
        }

        // Spawn standalone agents
        for (id, config) in self.configs.iter() {
            if grouped_ids.contains(id) {
                continue;
            }
            self.spawn_agent(config).await;
        }

        // --- Phase 2: single wait for CLI tools to boot ---
        sleep(Duration::from_secs(AGENT_INIT_DELAY_SECS)).await;

        // --- Phase 3: inject all startup prompts (fast, no blocking between them) ---

        for group in worker_groups {
            for member in &group.members {
                if let Some(prompt) = startup_prompts.get(&member.agent_id) {
                    if let Err(e) = self.injector.inject(&member.tmux_target, prompt).await {
                        eprintln!(
                            "[supervisor] failed to inject startup prompt for {}: {e}",
                            member.agent_id
                        );
                    }
                }
            }
        }

        for (id, config) in self.configs.iter() {
            if grouped_ids.contains(id) {
                continue;
            }
            if let Some(prompt) = startup_prompts.get(id) {
                if let Err(e) = self.injector.inject(&config.tmux_target, prompt).await {
                    eprintln!("[supervisor] failed to inject startup prompt for {id}: {e}");
                }
            }
        }
    }

    /// Spawn a worker group as a single tmux session with multiple panes.
    async fn spawn_group(&self, group: &WorkerGroupConfig) {
        if group.members.is_empty() {
            return;
        }

        // Kill any leftover session
        if self.injector.has_session(&group.session_name) {
            self.injector.kill_session(&group.session_name);
            sleep(Duration::from_millis(500)).await;
        }

        let cmds: Vec<&str> = group.members.iter().map(|m| m.cli_command.as_str()).collect();

        match self
            .injector
            .spawn_group_session(&group.session_name, &cmds, &group.layout)
        {
            Ok(terminal_handle) => {
                let mut agents = self.agents.lock().await;
                for member in &group.members {
                    let state = AgentState {
                        agent_id: member.agent_id.clone(),
                        tmux_session: member.tmux_session.clone(),
                        tmux_target: member.tmux_target.clone(),
                        status: AgentStatus::Healthy,
                        restart_count: 0,
                        last_start: Utc::now(),
                        last_heartbeat: None,
                        terminal_handle,
                        restart_timestamps: Vec::new(),
                        activity: AgentActivity::Unknown,
                        last_pane_hash: None,
                    };
                    agents.insert(member.agent_id.clone(), state);
                    self.logger.log(Event::AgentSpawn {
                        agent_id: member.agent_id.clone(),
                    });
                    println!("[supervisor] spawned {} (group: {})", member.agent_id, group.group_id);
                }
                drop(agents);
                self.persist_state().await;
            }
            Err(e) => {
                eprintln!(
                    "[supervisor] failed to spawn group '{}': {e}",
                    group.group_id
                );
            }
        }
    }

    /// Spawn a single standalone agent in its own tmux session.
    async fn spawn_agent(&self, config: &AgentConfig) {
        // Kill leftover session if any
        if self.injector.has_session(&config.tmux_session) {
            self.injector.kill_session(&config.tmux_session);
            sleep(Duration::from_millis(500)).await;
        }

        match self
            .injector
            .spawn_session(&config.tmux_session, &config.cli_command)
        {
            Ok(terminal_handle) => {
                let state = AgentState {
                    agent_id: config.agent_id.clone(),
                    tmux_session: config.tmux_session.clone(),
                    tmux_target: config.tmux_target.clone(),
                    status: AgentStatus::Healthy,
                    restart_count: 0,
                    last_start: Utc::now(),
                    last_heartbeat: None,
                    terminal_handle,
                    restart_timestamps: Vec::new(),
                    activity: AgentActivity::Unknown,
                    last_pane_hash: None,
                };
                self.agents
                    .lock()
                    .await
                    .insert(config.agent_id.clone(), state);
                self.logger.log(Event::AgentSpawn {
                    agent_id: config.agent_id.clone(),
                });
                self.persist_state().await;
                println!("[supervisor] spawned {}", config.agent_id);
            }
            Err(e) => {
                eprintln!("[supervisor] failed to spawn {}: {e}", config.agent_id);
            }
        }
    }

    /// Run the health-check loop. Call this as a background tokio task.
    pub async fn health_loop(self) {
        loop {
            sleep(Duration::from_secs(HEALTH_POLL_INTERVAL_SECS)).await;

            let agent_ids: Vec<String> = {
                let agents = self.agents.lock().await;
                agents.keys().cloned().collect()
            };

            for id in &agent_ids {
                let (session, status) = {
                    let agents = self.agents.lock().await;
                    match agents.get(id) {
                        Some(a) => (a.tmux_session.clone(), a.status.clone()),
                        None => continue,
                    }
                };

                if status == AgentStatus::Degraded {
                    continue; // don't try to revive degraded agents
                }

                let alive = self.injector.has_session(&session);

                if alive {
                    // Update heartbeat
                    let mut agents = self.agents.lock().await;
                    if let Some(a) = agents.get_mut(id) {
                        a.last_heartbeat = Some(Utc::now());
                    }
                } else {
                    // Agent died — attempt restart with backoff
                    self.logger.log(Event::AgentExit {
                        agent_id: id.clone(),
                        reason: "tmux session gone".into(),
                    });

                    let should_restart = {
                        let mut agents = self.agents.lock().await;
                        if let Some(a) = agents.get_mut(id) {
                            a.status = AgentStatus::Dead;

                            // Prune old restart timestamps outside the window
                            let cutoff =
                                Utc::now() - chrono::Duration::seconds(RESTART_WINDOW_SECS);
                            a.restart_timestamps.retain(|t| *t > cutoff);

                            if a.restart_timestamps.len() as u32 >= MAX_RESTARTS_IN_WINDOW {
                                a.status = AgentStatus::Degraded;
                                self.logger.log(Event::AgentDegraded {
                                    agent_id: id.clone(),
                                    restart_count: a.restart_count,
                                });
                                eprintln!(
                                    "[supervisor] {} marked DEGRADED after {} restarts in {}s window",
                                    id, a.restart_count, RESTART_WINDOW_SECS
                                );
                                false
                            } else {
                                true
                            }
                        } else {
                            false
                        }
                    };

                    if should_restart {
                        if let Some(config) = self.configs.get(id) {
                            // Capture old window ID and restart count before sleeping.
                            let (attempt, old_handle) = {
                                let agents = self.agents.lock().await;
                                let (count, handle) = agents
                                    .get(id)
                                    .map(|a| (a.restart_count, a.terminal_handle))
                                    .unwrap_or((0, None));
                                (count, handle)
                            };
                            // Exponential backoff: 1s, 2s, 4s, ...
                            let backoff = Duration::from_secs(1 << attempt.min(4));
                            println!("[supervisor] {} died — restarting in {:?}...", id, backoff);
                            sleep(backoff).await;

                            match self
                                .injector
                                .spawn_session(&config.tmux_session, &config.cli_command)
                            {
                                Ok(new_handle) => {
                                    // Close the old terminal window now that a new
                                    // one has been opened for the restarted session.
                                    if let Some(handle) = old_handle {
                                        crate::injector::close_terminal_handle(handle);
                                    }
                                    let mut agents = self.agents.lock().await;
                                    if let Some(a) = agents.get_mut(id) {
                                        a.restart_count += 1;
                                        a.restart_timestamps.push(Utc::now());
                                        a.last_start = Utc::now();
                                        a.last_heartbeat = Some(Utc::now());
                                        a.status = AgentStatus::Healthy;
                                        a.terminal_handle = new_handle;
                                    }
                                    self.logger.log(Event::AgentRestart {
                                        agent_id: id.clone(),
                                        attempt: attempt + 1,
                                    });
                                    println!(
                                        "[supervisor] {} restarted (attempt {})",
                                        id,
                                        attempt + 1
                                    );
                                }
                                Err(e) => {
                                    eprintln!("[supervisor] failed to restart {}: {e}", id);
                                }
                            }
                        }
                    }

                    self.persist_state().await;
                }
            }
        }
    }

    /// Periodically capture each agent's tmux pane and append to a transcript log.
    /// Call this as a background tokio task.
    pub async fn transcript_loop(self) {
        loop {
            sleep(Duration::from_secs(TRANSCRIPT_INTERVAL_SECS)).await;

            let states: Vec<(String, String, AgentStatus)> = {
                let agents = self.agents.lock().await;
                agents
                    .values()
                    .map(|a| {
                        let target = if a.tmux_target.is_empty() {
                            a.tmux_session.clone()
                        } else {
                            a.tmux_target.clone()
                        };
                        (a.agent_id.clone(), target, a.status.clone())
                    })
                    .collect()
            };

            for (id, session, status) in &states {
                if *status == AgentStatus::Dead {
                    continue;
                }

                match self.injector.capture(session) {
                    Ok(content) => {
                        let transcript_path = self.log_dir.join(format!("{id}_transcript.log"));
                        let chars = content.len();
                        if let Ok(mut file) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&transcript_path)
                        {
                            let timestamp = Utc::now().to_rfc3339();
                            let _ = writeln!(file, "\n=== {timestamp} ===\n{content}");
                        }
                        self.logger.log(crate::logger::Event::TranscriptCaptured {
                            agent_id: id.clone(),
                            chars,
                        });
                    }
                    Err(e) => {
                        eprintln!("[supervisor] transcript capture failed for {id}: {e}");
                    }
                }
            }
        }
    }

    /// Lightweight activity detection loop. Captures each agent's pane every
    /// few seconds and compares content hashes to determine busy/idle state.
    /// Call this as a background tokio task.
    pub async fn activity_loop(self) {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        loop {
            sleep(Duration::from_secs(ACTIVITY_POLL_INTERVAL_SECS)).await;

            let states: Vec<(String, String, AgentStatus)> = {
                let agents = self.agents.lock().await;
                agents
                    .values()
                    .map(|a| {
                        let target = if a.tmux_target.is_empty() {
                            a.tmux_session.clone()
                        } else {
                            a.tmux_target.clone()
                        };
                        (a.agent_id.clone(), target, a.status.clone())
                    })
                    .collect()
            };

            for (id, session, status) in &states {
                if *status == AgentStatus::Dead {
                    continue;
                }

                if let Ok(content) = self.injector.capture(&session) {
                    let mut hasher = DefaultHasher::new();
                    content.hash(&mut hasher);
                    let current_hash = hasher.finish();

                    let mut agents = self.agents.lock().await;
                    if let Some(state) = agents.get_mut(id) {
                        state.activity = match state.last_pane_hash {
                            Some(prev) if prev == current_hash => AgentActivity::Idle,
                            Some(_) => AgentActivity::Busy,
                            None => AgentActivity::Unknown,
                        };
                        state.last_pane_hash = Some(current_hash);
                    }
                }
            }
        }
    }

    /// Attention detection loop. Polls each agent's pane every few seconds and
    /// fires a visual + audible alert when an interactive prompt is detected.
    ///
    /// An alert fires when:
    ///   1. The pane content matches a known "waiting for input" pattern for
    ///      the agent's CLI tool, AND
    ///   2. The pane hash is unchanged from the previous poll (agent is stuck,
    ///      not just transiently printing prompt-like text), AND
    ///   3. The agent hasn't been alerted within the debounce window.
    ///
    /// When the pane content changes (agent unblocked), the visual is cleared.
    pub async fn attention_loop(self) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut attn_states: HashMap<String, AgentAttentionState> = HashMap::new();

        loop {
            sleep(Duration::from_secs(ATTENTION_POLL_INTERVAL_SECS)).await;

            // Collect (agent_id, tmux_target, tmux_session, cli_command, status, last_start) snapshots
            let snapshots: Vec<(String, String, String, String, AgentStatus, DateTime<Utc>)> = {
                let agents = self.agents.lock().await;
                agents
                    .values()
                    .filter_map(|a| {
                        let config = self.configs.get(&a.agent_id)?;
                        let target = if a.tmux_target.is_empty() {
                            a.tmux_session.clone()
                        } else {
                            a.tmux_target.clone()
                        };
                        Some((
                            a.agent_id.clone(),
                            target,
                            a.tmux_session.clone(),
                            config.cli_command.clone(),
                            a.status.clone(),
                            a.last_start,
                        ))
                    })
                    .collect()
            };

            for (id, target, session, cli_cmd, status, last_start) in &snapshots {
                if *status == AgentStatus::Dead {
                    continue;
                }

                // Skip agents that spawned less than 30 seconds ago
                if (Utc::now() - *last_start).num_seconds() < 30 {
                    continue;
                }

                let content = match self.injector.capture(target) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let mut hasher = DefaultHasher::new();
                content.hash(&mut hasher);
                let current_hash = hasher.finish();

                let state = attn_states.entry(id.clone()).or_insert(AgentAttentionState {
                    active: false,
                    last_alerted: None,
                    prev_hash: None,
                    stable_count: 0,
                });

                let hash_stable = state.prev_hash == Some(current_hash);
                state.prev_hash = Some(current_hash);

                if hash_stable {
                    state.stable_count += 1;

                    if state.stable_count >= 1 {
                        if let Some(pattern) = detect_attention_pattern(&content, cli_cmd) {
                            if !state.active {
                                // First detection — red pane + OS notification
                                print_attention_alert(id, session, pattern);
                                self.injector.set_pane_attention_style(target, session);
                                crate::injector::send_os_notification(
                                    "Agent needs input",
                                    &format!("{} — {}", id, pattern),
                                );
                                state.active = true;
                                state.last_alerted = Some(Utc::now());
                                self.logger.log(Event::AgentNeedsAttention {
                                    agent_id: id.clone(),
                                    pattern: pattern.to_string(),
                                    source: "auto-detected".to_string(),
                                });
                            }
                        }
                    }
                } else {
                    state.stable_count = 0;
                    if state.active {
                        // Pane changed — agent is no longer blocked; clear visuals
                        self.injector.clear_pane_attention_style(target, session);
                        state.active = false;
                        self.logger.log(Event::AgentAttentionResolved {
                            agent_id: id.clone(),
                        });
                        eprintln!("[attention] {} unblocked — cleared alert", id);
                    }
                }
            }
        }
    }

    /// Fire an operator alert for an agent-initiated _ATTENTION request.
    /// Called by the message watcher when it sees a _ATTENTION topic message.
    pub async fn fire_attention_alert(
        &self,
        agent_id: &str,
        message_content: &str,
    ) {
        let (target, session) = {
            let agents = self.agents.lock().await;
            match agents.get(agent_id) {
                Some(a) => {
                    let tgt = if a.tmux_target.is_empty() {
                        a.tmux_session.clone()
                    } else {
                        a.tmux_target.clone()
                    };
                    (tgt, a.tmux_session.clone())
                }
                None => return,
            }
        };

        // Trim message for the banner (first line, max 60 chars)
        let summary: String = message_content
            .lines()
            .next()
            .unwrap_or("(no message)")
            .chars()
            .take(60)
            .collect();

        use std::io::Write;
        let banner = format!(
            "\n\
             ╔══════════════════════════════════════════════╗\n\
             ║  ⚠  AGENT REQUESTING ATTENTION              ║\n\
             ║  agent:   {:<35}║\n\
             ║  session: {:<35}║\n\
             ║  message: {:<35}║\n\
             ╚══════════════════════════════════════════════╝\n",
            agent_id, session, summary,
        );
        let _ = std::io::stderr().write_all(banner.as_bytes());

        self.injector.set_pane_attention_style(&target, &session);
        crate::injector::send_os_notification(
            "Agent requesting attention",
            &format!("{} — {}", agent_id, summary),
        );
        self.logger.log(Event::AgentNeedsAttention {
            agent_id: agent_id.to_string(),
            pattern: summary,
            source: "agent-initiated".to_string(),
        });
    }

    /// Kill all agent tmux sessions and close their terminal windows.
    pub async fn kill_all(&self) {
        let agents = self.agents.lock().await;
        let mut killed_sessions = std::collections::HashSet::new();
        for (id, state) in agents.iter() {
            println!("[supervisor] killing {}", id);
            if killed_sessions.insert(state.tmux_session.clone()) {
                self.injector.kill_session(&state.tmux_session);
            }
            if let Some(handle) = state.terminal_handle {
                crate::injector::close_terminal_handle(handle);
            }
        }
    }

    /// Write current agent states to state.json.
    async fn persist_state(&self) {
        let agents = self.agents.lock().await;
        let snapshot: HashMap<&String, &AgentState> = agents.iter().collect();
        if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
            let _ = std::fs::write(&self.state_path, json);
        }
    }

    /// Look up the tmux target (pane-specific if grouped) for a given agent_id.
    /// Used by the timer loop for injection.
    pub async fn session_for(&self, agent_id: &str) -> Option<String> {
        let agents = self.agents.lock().await;
        agents.get(agent_id).map(|a| {
            if a.tmux_target.is_empty() {
                a.tmux_session.clone()
            } else {
                a.tmux_target.clone()
            }
        })
    }

    /// Restart an agent with a fresh context: respawn the pane process
    /// within the existing tmux session and re-inject the startup prompt.
    /// The terminal window stays open.
    pub async fn restart_agent(&self, agent_id: &str) -> Result<(), String> {
        let config = self
            .configs
            .get(agent_id)
            .ok_or_else(|| format!("unknown agent: {agent_id}"))?;

        // Respawn the pane — kills the running process and starts the CLI
        // command fresh, keeping the tmux session (and terminal) intact.
        self.injector
            .respawn_pane(&config.tmux_session, &config.cli_command)
            .map_err(|e| format!("failed to respawn pane for {agent_id}: {e}"))?;

        // Update state
        {
            let mut agents = self.agents.lock().await;
            if let Some(state) = agents.get_mut(agent_id) {
                state.status = AgentStatus::Healthy;
                state.last_start = Utc::now();
                state.last_heartbeat = Some(Utc::now());
                // Don't increment restart_count or push to restart_timestamps;
                // this is a deliberate restart, not a crash recovery.
            }
        }

        self.logger.log(Event::AgentSpawn {
            agent_id: agent_id.to_string(),
        });

        // Re-inject startup prompt
        let prompts = self.startup_prompts.lock().await;
        if let Some(prompt) = prompts.get(agent_id) {
            sleep(Duration::from_secs(AGENT_INIT_DELAY_SECS)).await;
            if let Err(e) = self.injector.inject(&config.tmux_target, prompt).await {
                eprintln!("[supervisor] failed to inject startup prompt for {agent_id}: {e}");
            }
        }

        self.persist_state().await;
        println!("[supervisor] {agent_id} restarted with fresh context");
        Ok(())
    }

    /// Run the timer loop: periodically injects timer prompts into agent tmux
    /// sessions.  Each timer fires at its configured `minutes` interval.
    /// If `include_agents` is non-empty, a status footer showing those agents'
    /// last transcript sizes is appended to the prompt.
    pub async fn timer_loop(self, timers: Vec<ResolvedTimer>, logger: Arc<Logger>) {
        use std::time::Instant;
        use tokio::time::interval;

        if timers.is_empty() {
            return; // nothing to do
        }

        // Track last-fire time per (agent_id, index) so we can stagger.
        let mut last_fired: Vec<Instant> = vec![Instant::now(); timers.len()];

        // Tick every 30 seconds to check if any timer is due.
        let mut tick = interval(Duration::from_secs(30));

        loop {
            tick.tick().await;

            let now = Instant::now();

            for (i, timer) in timers.iter().enumerate() {
                let interval_dur = Duration::from_secs(timer.minutes * 60);
                if now.duration_since(last_fired[i]) < interval_dur {
                    continue;
                }

                last_fired[i] = now;

                // Build the prompt with optional status footer
                let mut prompt = match timer.read_prompt() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!(
                            "[timer] failed to read prompt for '{}': {e}",
                            timer.agent_id
                        );
                        continue;
                    }
                };
                if !timer.include_agents.is_empty() {
                    prompt.push_str("\n\n--- AGENT STATUS ---\n");
                    let agents = self.agents.lock().await;
                    for ref_id in &timer.include_agents {
                        let status_line = if let Some(state) = agents.get(ref_id) {
                            format!("- {}: {} | {} (started {})
", ref_id, state.activity, state.status, state.last_start.format("%H:%M:%S UTC"))
                        } else {
                            format!("- {}: unknown\n", ref_id)
                        };
                        prompt.push_str(&status_line);
                    }
                    prompt.push_str("--- END STATUS ---\n");
                }

                // Frame it as a timer message
                let framed = format!(
                    "--- TIMER MESSAGE ---\nTO: {}\nTOPIC: _TIMER\n\n---\n\n{}\n",
                    timer.agent_id, prompt
                );

                // Look up the session and command
                let session = match self.session_for(&timer.agent_id).await {
                    Some(s) => s,
                    None => {
                        eprintln!("[timer] no session for '{}', skipping", timer.agent_id);
                        continue;
                    }
                };

                logger.log(Event::TimerFired {
                    agent_id: timer.agent_id.clone(),
                    minutes: timer.minutes,
                    prompt_file: String::new(),
                });

                // Inject (with interrupt if configured)
                let result = if timer.interrupt {
                    let cmd = self.configs.get(&timer.agent_id)
                        .map(|c| c.cli_command.as_str())
                        .unwrap_or("");
                    let keys = InterruptKeys::for_command(cmd);
                    self.injector.inject_interrupt(&session, &framed, &keys).await
                } else {
                    self.injector.inject(&session, &framed).await
                };

                match result {
                    Ok(()) => println!(
                        "[timer] fired {}m timer for '{}'",
                        timer.minutes, timer.agent_id
                    ),
                    Err(e) => eprintln!(
                        "[timer] failed to inject {}m timer for '{}': {e}",
                        timer.minutes, timer.agent_id
                    ),
                }
            }
        }
    }
}
