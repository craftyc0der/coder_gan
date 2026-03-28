use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use orchestrator::config::{self as config, ProjectConfig};
use orchestrator::menu;
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
        /// Enable git worktree mode: each agent gets its own worktree checkout
        #[arg(long)]
        worktree: bool,
        /// Feature/ticket name used as the branch prefix (e.g. PR-123).
        /// Required when --worktree is used. Decorates tmux session names and
        /// is used to derive per-agent branch names.
        #[arg(long)]
        branch: Option<String>,
        /// Name this orchestrator session so it can be resumed later.
        /// Saved to runtime/sessions/<name>.json after all agents start.
        #[arg(long)]
        session: Option<String>,
        /// Resume a previously saved orchestrator session by name.
        /// Loads runtime/sessions/<name>.json and starts each agent with its
        /// vendor resume flag, restoring prior context.
        #[arg(long)]
        resume: Option<String>,
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
        Commands::Run { path, worktree, branch, session, resume } => {
            let project_path = resolve_path(&path);
            let mut config = load_config_or_exit(&project_path);

            // --resume: load saved session and inject resume IDs into config
            if let Some(ref resume_name) = resume {
                use orchestrator::session::OrchestratorSession;
                match OrchestratorSession::load(&config.sessions_dir, resume_name) {
                    Ok(saved) => {
                        println!("Resuming session '{}'...", saved.name);
                        for (agent_id, info) in &saved.agents {
                            if let Some(ref id) = info.session_id {
                                config.resume_ids.insert(agent_id.clone(), id.clone());
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error loading session '{}': {e}", resume_name);
                        std::process::exit(1);
                    }
                }
            }

            // Determine the effective session name for saving:
            // --resume implies we keep the same name; --session overrides.
            let effective_session = session.as_deref().or(resume.as_deref());

            // Set up worktree mode if requested
            if worktree {
                let feature_name = match branch {
                    Some(ref b) => b.clone(),
                    None => {
                        eprintln!("Error: --worktree requires --branch <name>");
                        std::process::exit(1);
                    }
                };
                config.worktree_feature = Some(feature_name.clone());

                // Build worktree specs: one per standalone agent, one per
                // group instance (all agents in a group share a worktree).
                let grouped_ids: std::collections::HashSet<&str> = config
                    .worker_groups
                    .iter()
                    .flat_map(|g| g.agents.iter().map(|a| a.as_str()))
                    .collect();

                let mut specs: Vec<orchestrator::worktree::WorktreeSpec> = Vec::new();

                // Standalone agents: one worktree each
                for a in &config.agents {
                    if grouped_ids.contains(a.id.as_str()) {
                        continue;
                    }
                    specs.push(orchestrator::worktree::WorktreeSpec {
                        worktree_id: a.id.clone(),
                        agent_ids: vec![a.id.clone()],
                        branch_override: a.branch.clone(),
                    });
                }

                // Worker groups: one worktree per group instance, shared by
                // all agents in that instance.
                for group in &config.worker_groups {
                    for instance in 1..=group.count {
                        let wt_id = orchestrator::config::expand_agent_id(
                            &group.id, instance, group.count,
                        );
                        let agent_ids: Vec<String> = group
                            .agents
                            .iter()
                            .map(|a| {
                                orchestrator::config::expand_agent_id(a, instance, group.count)
                            })
                            .collect();
                        specs.push(orchestrator::worktree::WorktreeSpec {
                            worktree_id: wt_id,
                            agent_ids,
                            branch_override: None,
                        });
                    }
                }

                match orchestrator::worktree::setup_worktrees(
                    &config.project_root,
                    &feature_name,
                    &specs,
                ) {
                    Ok(worktrees) => {
                        config.worktrees = worktrees;
                        println!("Worktree mode: feature={}", feature_name);
                    }
                    Err(e) => {
                        eprintln!("Error setting up worktrees: {e}");
                        std::process::exit(1);
                    }
                }
            } else if let Some(ref b) = branch {
                // --branch without --worktree: just set the feature name
                // for tmux session decoration
                config.worktree_feature = Some(b.clone());
            }

            run_orchestrator(config, effective_session.map(str::to_string)).await;
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

async fn run_orchestrator(mut config: ProjectConfig, session_name: Option<String>) {
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

    // Print worker groups summary
    if !config.worker_groups.is_empty() {
        for group in &config.worker_groups {
            println!(
                "Group:     {} ({}) × {} — [{}]",
                group.id,
                match group.layout {
                    orchestrator::config::SplitDirection::Horizontal => "horizontal",
                    orchestrator::config::SplitDirection::Vertical => "vertical",
                },
                group.count,
                group.agents.join(", ")
            );
        }
    }

    // Print standalone agents
    let grouped_ids: std::collections::HashSet<&str> = config
        .worker_groups
        .iter()
        .flat_map(|g| g.agents.iter().map(|a| a.as_str()))
        .collect();
    let standalone: Vec<&str> = config
        .agents
        .iter()
        .filter(|a| !grouped_ids.contains(a.id.as_str()))
        .map(|a| a.id.as_str())
        .collect();
    if !standalone.is_empty() {
        println!("Agents:    {}", standalone.join(", "));
    }
    println!();

    // Build agent configs and registry
    let agent_cfgs = config.agent_configs();
    let group_cfgs = config.worker_group_configs();
    let registry = Registry::new(
        agent_cfgs,
        config.state_path.clone(),
        config.pids_dir.clone(),
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
    registry
        .spawn_all(
            &prompts,
            &group_cfgs,
            session_name.as_deref(),
            Some(&config.sessions_dir),
        )
        .await;
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
    let worktree_roots: Vec<std::path::PathBuf> = config
        .worktrees
        .iter()
        .map(|wt| wt.worktree_path.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let msg_watcher = Arc::new(
        MessageWatcher::new(
            registry.clone(),
            logger.clone(),
            config.messages_dir.clone(),
        )
        .with_worktree_symlink_watch(config.dot_dir.clone(), worktree_roots),
    );
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

    // Start the attention detection loop (interactive prompt scanning every 3s)
    let attention_registry = registry.clone();
    let attention_handle = tokio::spawn(async move {
        attention_registry.attention_loop().await;
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

    println!("Orchestrator running.\n");

    // Run the interactive CLI menu (blocks until user quits or Ctrl+C)
    let menu_registry = registry.clone();
    let menu_logger = logger.clone();
    tokio::select! {
        _ = menu::run_menu(menu_registry, menu_logger, &mut config) => {},
        _ = tokio::signal::ctrl_c() => {},
    }

    println!("\nShutting down...");
    health_handle.abort();
    transcript_handle.abort();
    activity_handle.abort();
    attention_handle.abort();
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
