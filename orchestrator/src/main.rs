use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use orchestrator::config::{self as config, ProjectConfig};
#[cfg(feature = "slack")]
use orchestrator::config::{AgentType, SlackConfig};
use orchestrator::injector;
use orchestrator::logger::{Event, Logger};
use orchestrator::scope;
#[cfg(feature = "slack")]
use orchestrator::slack::SlackWatcher;
use orchestrator::spike;
use orchestrator::supervisor::Registry;
use orchestrator::watcher::MessageWatcher;

#[derive(Parser)]
#[command(name = "orchestrator")]
#[command(about = "Multi-agent coding orchestration system")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new .orchestrator/ configuration in a project
    Init {
        /// Project directory (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Run the tmux spike to validate interactive session control
    Spike {
        /// Project directory (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Agent to run spike against (defaults to first agent in config)
        #[arg(long)]
        agent: Option<String>,
        /// Test interrupt key sequences instead of normal injection
        #[arg(long)]
        interrupt: bool,
    },
    /// Launch all agents and start the message routing loop
    Run {
        /// Project directory (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Show current agent health and status
    Status {
        /// Project directory (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Gracefully shut down all agents
    Stop {
        /// Project directory (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a CLI path argument to an absolute path.
fn resolve_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap().join(path)
    }
}

/// Load ProjectConfig or print an error and exit.
fn load_config_or_exit(project_path: &Path) -> ProjectConfig {
    match ProjectConfig::load(project_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path } => {
            let project_path = resolve_path(&path);
            cmd_init(&project_path);
        }
        Commands::Spike { path, agent, interrupt } => {
            let project_path = resolve_path(&path);
            let config = load_config_or_exit(&project_path);
            if interrupt {
                spike::run_spike_interrupt(config, agent.as_deref()).await;
            } else {
                spike::run_spike(config, agent.as_deref()).await;
            }
        }
        Commands::Run { path } => {
            let project_path = resolve_path(&path);
            let config = load_config_or_exit(&project_path);
            run_orchestrator(config).await;
        }
        Commands::Status { path } => {
            let project_path = resolve_path(&path);
            let config = load_config_or_exit(&project_path);
            cmd_status(&config);
        }
        Commands::Stop { path } => {
            let project_path = resolve_path(&path);
            let config = load_config_or_exit(&project_path);
            cmd_stop(&config);
        }
    }
}

// ---------------------------------------------------------------------------
// `init` subcommand
// ---------------------------------------------------------------------------

fn cmd_init(project_path: &Path) {
    match config::init_project(project_path) {
        Ok(()) => {
            println!("Initialized .orchestrator/ in {}", project_path.display());
            println!();
            println!("  Config:  .orchestrator/agents.toml");
            println!("  Prompts: .orchestrator/prompts/*.md");
            println!();
            println!("Edit agents.toml to configure your agents,");
            println!("then run: orchestrator run {}", project_path.display());
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// `run` subcommand — full orchestrator
// ---------------------------------------------------------------------------

async fn run_orchestrator(config: ProjectConfig) {
    config.ensure_dirs().expect("failed to create directories");

    // Print non-blocking warnings for known configuration issues
    for warning in config::check_agent_command_warnings(&config.agents) {
        eprintln!("{warning}");
    }

    let logger = Arc::new(Logger::new(&config.log_dir, "events.jsonl"));
    logger.log(Event::OrchestratorStart);

    println!("=== orchestrator ===");
    println!("Project:   {}", config.project_root.display());
    println!(
        "Config:    {}",
        config.dot_dir.join("agents.toml").display()
    );
    println!("Event log: {}", logger.path().display());
    println!(
        "Agents:    {}",
        config
            .agents
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!();

    // Build agent configs and registry
    let agent_cfgs = config.agent_configs();
    let registry = Registry::new(
        agent_cfgs,
        config.state_path.clone(),
        config.log_dir.clone(),
        logger.clone(),
    );

    // Load startup prompts from prompt files
    let prompts = match config.startup_prompts() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Warning: failed to load prompts: {e}");
            HashMap::new()
        }
    };

    // Spawn all agents with startup prompts
    println!("Spawning agents...");
    registry.spawn_all(&prompts).await;
    println!("All agents spawned.\n");

    // Spawn Slack WebSocket watchers for slack-type agents
    #[cfg(feature = "slack")]
    {
        let mut slack_handles = Vec::new();
        for agent in &config.agents {
            if agent.agent_type != AgentType::Slack {
                continue;
            }
            let slack_agent_cfg = match &agent.slack {
                Some(s) => s,
                None => continue,
            };
            let slack_config = match SlackConfig::load(&config.dot_dir, slack_agent_cfg) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[slack] Failed to load config for {}: {e}", agent.id);
                    continue;
                }
            };
            let watcher = SlackWatcher::new(
                slack_config,
                agent.id.clone(),
                config.messages_dir.clone(),
                logger.clone(),
            );
            let watcher = Arc::new(watcher);

            // Spawn WebSocket event loop
            let ws_watcher = Arc::clone(&watcher);
            slack_handles.push(tokio::spawn(async move {
                ws_watcher.run().await;
            }));

            // Spawn response inbox watcher
            let inbox_watcher = Arc::clone(&watcher);
            slack_handles.push(tokio::spawn(async move {
                inbox_watcher.watch_response_inbox().await;
            }));

            println!("[slack] Started watcher for agent '{}'", agent.id);
        }
    }

    // Start the filesystem message watcher
    let msg_watcher = Arc::new(MessageWatcher::new(
        registry.clone(),
        logger.clone(),
        config.messages_dir.clone(),
    ));
    msg_watcher.start().await;
    println!("Message watcher started.\n");

    // Start the supervisor health loop in the background
    let health_registry = registry.clone();
    let health_handle = tokio::spawn(async move {
        health_registry.health_loop().await;
    });

    // Start the per-agent transcript capture loop
    let transcript_registry = registry.clone();
    let transcript_handle = tokio::spawn(async move {
        transcript_registry.transcript_loop().await;
    });

    // Start the activity detection loop (pane hash diff every 3s)
    let activity_registry = registry.clone();
    let activity_handle = tokio::spawn(async move {
        activity_registry.activity_loop().await;
    });

    // Start the timer loop for recurring prompt injections
    let timer_handle = match config.resolved_timers() {
        Ok(timers) if !timers.is_empty() => {
            println!(
                "Timers:    {} configured",
                timers.len()
            );
            let timer_registry = registry.clone();
            let timer_logger = logger.clone();
            Some(tokio::spawn(async move {
                timer_registry.timer_loop(timers, timer_logger).await;
            }))
        }
        Ok(_) => None,
        Err(e) => {
            eprintln!("Warning: failed to load timer prompts: {e}");
            None
        }
    };

    // Start the scope enforcement watcher
    scope::start_scope_watcher(
        config.project_root.clone(),
        config.dot_dir.clone(),
        config.agent_configs(),
        logger.clone(),
    );

    println!("Orchestrator running. Press Ctrl+C to stop.");
    println!();

    // Wait for SIGINT/SIGTERM
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");

    println!("\nShutting down...");
    health_handle.abort();
    transcript_handle.abort();
    activity_handle.abort();
    if let Some(h) = timer_handle {
        h.abort();
    }
    registry.kill_all().await;
    logger.log(Event::OrchestratorStop);
    println!("All agents killed. Goodbye.");
}

// ---------------------------------------------------------------------------
// `status` subcommand
// ---------------------------------------------------------------------------

fn cmd_status(config: &ProjectConfig) {
    if !config.state_path.exists() {
        println!(
            "No state file found at {}. Is the orchestrator running?",
            config.state_path.display()
        );
        return;
    }

    let content = match std::fs::read_to_string(&config.state_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read state.json: {e}");
            return;
        }
    };

    let state: HashMap<String, serde_json::Value> = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to parse state.json: {e}");
            return;
        }
    };

    println!(
        "{:<12} {:<28} {:<10} {:<10} {:<24}",
        "AGENT", "TMUX SESSION", "STATUS", "RESTARTS", "LAST START"
    );
    println!("{}", "-".repeat(84));

    for (id, val) in &state {
        let session = val["tmux_session"].as_str().unwrap_or("?");
        let status = val["status"].as_str().unwrap_or("?");
        let restarts = val["restart_count"].as_u64().unwrap_or(0);
        let last_start = val["last_start"].as_str().unwrap_or("?");
        println!(
            "{:<12} {:<28} {:<10} {:<10} {:<24}",
            id, session, status, restarts, last_start
        );
    }
}

// ---------------------------------------------------------------------------
// `stop` subcommand
// ---------------------------------------------------------------------------

fn cmd_stop(config: &ProjectConfig) {
    // Try to read session names from state.json first
    if config.state_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config.state_path) {
            if let Ok(state) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&content)
            {
                for val in state.values() {
                    if let Some(session) = val["tmux_session"].as_str() {
                        if injector::has_session(session) {
                            println!("Killing tmux session: {session}");
                            injector::kill_session(session);
                        } else {
                            println!("Session not found (already dead?): {session}");
                        }
                    }
                    if let Some(handle) = val["terminal_handle"]
                        .as_u64()
                        .or_else(|| val["terminal_window_id"].as_u64())
                        .map(|id| id as u32)
                    {
                        println!("Closing terminal window: {handle}");
                        injector::close_terminal_handle(handle);
                    }
                }
                let logger = Logger::new(&config.log_dir, "events.jsonl");
                logger.log(Event::OrchestratorStop);
                println!("All agents stopped.");
                return;
            }
        }
    }

    // Fallback: derive session names from config
    println!("No state.json found. Attempting to kill sessions from config...");
    for agent in &config.agents {
        let session = config.tmux_session_for(&agent.id);
        if injector::has_session(&session) {
            println!("Killing tmux session: {session}");
            injector::kill_session(&session);
        } else {
            println!("Session not found: {session}");
        }
    }

    let logger = Logger::new(&config.log_dir, "events.jsonl");
    logger.log(Event::OrchestratorStop);
    println!("All agents stopped.");
}
