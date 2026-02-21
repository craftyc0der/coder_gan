use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::injector::{InjectorOps, RealInjector};
use crate::logger::{Event, Logger};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const HEALTH_POLL_INTERVAL_SECS: u64 = 2;
const MAX_RESTARTS_IN_WINDOW: u32 = 5;
const RESTART_WINDOW_SECS: i64 = 120; // 2 minutes
const AGENT_INIT_DELAY_SECS: u64 = 5;
const TRANSCRIPT_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent_id: String,
    pub cli_command: String,
    pub tmux_session: String,
    pub inbox_dir: PathBuf,
    pub allowed_write_dirs: Vec<PathBuf>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub agent_id: String,
    pub tmux_session: String,
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
}

// ---------------------------------------------------------------------------
// Registry (shared state across the supervisor + watcher)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Registry {
    pub agents: Arc<Mutex<HashMap<String, AgentState>>>,
    configs: Arc<HashMap<String, AgentConfig>>,
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
            state_path,
            log_dir,
            logger,
            injector,
        }
    }

    /// Spawn all configured agents, injecting a startup prompt into each.
    pub async fn spawn_all(&self, startup_prompts: &HashMap<String, String>) {
        for (id, config) in self.configs.iter() {
            self.spawn_agent(config).await;

            // Inject startup prompt after init delay
            if let Some(prompt) = startup_prompts.get(id) {
                sleep(Duration::from_secs(AGENT_INIT_DELAY_SECS)).await;
                if let Err(e) = self.injector.inject(&config.tmux_session, prompt).await {
                    eprintln!("[supervisor] failed to inject startup prompt for {id}: {e}");
                }
            }
        }
    }

    /// Spawn a single agent.
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
                    status: AgentStatus::Healthy,
                    restart_count: 0,
                    last_start: Utc::now(),
                    last_heartbeat: None,
                    terminal_handle,
                    restart_timestamps: Vec::new(),
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
                    .map(|a| (a.agent_id.clone(), a.tmux_session.clone(), a.status.clone()))
                    .collect()
            };

            for (id, session, status) in &states {
                if *status == AgentStatus::Dead {
                    continue; // session is gone
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

    /// Kill all agent tmux sessions and close their terminal windows.
    pub async fn kill_all(&self) {
        let agents = self.agents.lock().await;
        for (id, state) in agents.iter() {
            println!("[supervisor] killing {}", id);
            self.injector.kill_session(&state.tmux_session);
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

    /// Look up the tmux session name for a given agent_id.
    pub async fn session_for(&self, agent_id: &str) -> Option<String> {
        let agents = self.agents.lock().await;
        agents.get(agent_id).map(|a| a.tmux_session.clone())
    }
}
