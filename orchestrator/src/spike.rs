use std::sync::Arc;
use tokio::time::{sleep, Duration};

use crate::config::ProjectConfig;
use crate::injector::{InjectorOps, InterruptKeys, RealInjector};
use crate::logger::{Event, Logger};

// ---------------------------------------------------------------------------
// Timing configuration — override with SpikeTimings::for_testing() in tests
// ---------------------------------------------------------------------------

/// Delay and polling parameters for a spike run.
/// Use [`SpikeTimings::for_testing`] in tests to zero out all sleeps.
pub struct SpikeTimings {
    /// How long to wait after spawning before injecting the validation prompt.
    pub agent_init_delay: Duration,
    /// Interval between validation-file poll rounds.
    pub poll_interval: Duration,
    /// Maximum poll rounds for the validation checkpoint (timeout = interval × rounds).
    pub poll_max_rounds: usize,
    /// Interval between per-payload file poll rounds.
    pub payload_poll_interval: Duration,
    /// Maximum poll rounds per payload.
    pub payload_poll_max_rounds: usize,
    /// How long to wait after `kill_session` before checking `has_session`.
    pub kill_settle_delay: Duration,
    /// How long to wait after respawn before confirming the session is alive.
    pub respawn_settle_delay: Duration,
    /// How long to wait after injecting the busy-prompt before sending interrupt.
    pub interrupt_busy_delay: Duration,
    /// If set, overrides `InterruptKeys::settle_ms` (useful for zeroing out in tests).
    pub interrupt_settle_override: Option<Duration>,
}

impl Default for SpikeTimings {
    fn default() -> Self {
        SpikeTimings {
            agent_init_delay: Duration::from_secs(8),
            poll_interval: Duration::from_secs(2),
            poll_max_rounds: 30,
            payload_poll_interval: Duration::from_secs(3),
            payload_poll_max_rounds: 20,
            kill_settle_delay: Duration::from_secs(2),
            respawn_settle_delay: Duration::from_secs(3),
            interrupt_busy_delay: Duration::from_secs(10),
            interrupt_settle_override: None,
        }
    }
}

impl SpikeTimings {
    /// Zero-delay timings for unit / integration tests.
    /// Also reduces poll round counts so tests don't loop unnecessarily.
    pub fn for_testing() -> Self {
        SpikeTimings {
            agent_init_delay: Duration::ZERO,
            poll_interval: Duration::ZERO,
            poll_max_rounds: 5,
            payload_poll_interval: Duration::ZERO,
            payload_poll_max_rounds: 2,
            kill_settle_delay: Duration::ZERO,
            respawn_settle_delay: Duration::ZERO,
            interrupt_busy_delay: Duration::ZERO,
            interrupt_settle_override: Some(Duration::ZERO),
        }
    }
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

/// Run the interrupt spike using real tmux.
/// Calls [`run_spike_interrupt_with_deps`] with [`RealInjector`] and default timings.
pub async fn run_spike_interrupt(config: ProjectConfig, agent_id_arg: Option<&str>) {
    if let Err(msg) = run_spike_interrupt_with_deps(
        config,
        agent_id_arg,
        &RealInjector,
        &SpikeTimings::default(),
    )
    .await
    {
        eprintln!("ERROR: {msg}");
        std::process::exit(1);
    }
}

/// Run the spike using real tmux.
/// Calls [`run_spike_with_deps`] with [`RealInjector`] and default timings.
pub async fn run_spike(config: ProjectConfig, agent_id_arg: Option<&str>) {
    if let Err(msg) =
        run_spike_with_deps(config, agent_id_arg, &RealInjector, &SpikeTimings::default()).await
    {
        eprintln!("ERROR: {msg}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Testable inner implementation
// ---------------------------------------------------------------------------

/// Run the spike against a configured agent, using the supplied injector and
/// timing parameters.
///
/// Returns `Ok(())` on success, `Err(message)` on a fatal failure.  Non-fatal
/// issues (individual payload timeouts, capture failures) are logged and
/// printed but do not cause an early return.
pub async fn run_spike_with_deps<I: InjectorOps>(
    config: ProjectConfig,
    agent_id_arg: Option<&str>,
    inj: &I,
    timings: &SpikeTimings,
) -> Result<(), String> {
    config
        .ensure_dirs()
        .map_err(|e| format!("ensure_dirs failed: {e}"))?;

    // --- Resolve target agent ---
    let agent = match agent_id_arg {
        Some(id) => match config.agents.iter().find(|a| a.id == id) {
            Some(a) => a.clone(),
            None => {
                return Err(format!(
                    "No agent with id '{}' in agents.toml. Available: {}",
                    id,
                    config
                        .agents
                        .iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        },
        None => {
            if config.agents.is_empty() {
                return Err("No agents configured in agents.toml".into());
            }
            config.agents[0].clone()
        }
    };

    let session = config.tmux_session_for(&agent.id);
    let logger = Arc::new(Logger::new(&config.log_dir, "spike_events.jsonl"));

    println!("=== orchestrator spike ===");
    println!("Project: {}", config.project_root.display());
    println!("Agent:   {} (command: '{}')", agent.id, agent.command);
    println!("Session: {session}");
    println!();

    // Kill any leftover session
    if inj.has_session(&session) {
        println!("Killing leftover tmux session '{session}'...");
        inj.kill_session(&session);
        sleep(Duration::from_millis(500)).await;
    }

    // --- Spawn ---
    println!(
        "Spawning tmux session '{session}' running '{}'...",
        agent.command
    );
    let terminal = agent.terminal.clone().unwrap_or_else(|| config.terminal.clone());
    if let Err(e) = inj.spawn_session(&session, &agent.command, &terminal) {
        return Err(format!("spawn_session failed: {e}"));
    }
    logger.log(Event::AgentSpawn {
        agent_id: agent.id.clone(),
    });
    println!("Session spawned. Waiting for agent to initialize...");
    sleep(timings.agent_init_delay).await;

    // --- Validation checkpoint ---
    let spike_output_dir = config.messages_dir.join("processed");
    let spike_file = spike_output_dir.join("spike-test.md");
    let _ = std::fs::remove_file(&spike_file);

    let prompt = format!(
        "Write a file containing exactly 'spike test passed' to the path '{}'. \
         Use the absolute path. Do not ask for confirmation, just do it.",
        spike_file.display()
    );

    println!("Injecting validation prompt...");
    if let Err(e) = inj.inject(&session, &prompt).await {
        logger.log(Event::SpikeInjectSent {
            agent_id: agent.id.clone(),
            detail: format!("failed: {e}"),
        });
        return Err(format!("validation inject failed: {e}"));
    }
    logger.log(Event::SpikeInjectSent {
        agent_id: agent.id.clone(),
        detail: "validation prompt".into(),
    });

    println!("Waiting for agent to act...");
    let mut found = false;
    for i in 0..timings.poll_max_rounds {
        sleep(timings.poll_interval).await;
        if spike_file.exists() {
            let content = std::fs::read_to_string(&spike_file).unwrap_or_default();
            println!("SPIKE VALIDATION PASSED — file written by agent:");
            println!("  path: {}", spike_file.display());
            println!("  content: {}", content.trim());
            logger.log(Event::SpikeInjectConfirmed {
                agent_id: agent.id.clone(),
                detail: "validation file created".into(),
            });
            found = true;
            break;
        }
        if i > 0 && i % 5 == 4 {
            println!("  still waiting...");
        }
    }

    if !found {
        if let Ok(pane) = inj.capture(&session) {
            let debug_path = config.transcript_dir.join("spike-failed-pane.txt");
            let _ = std::fs::write(&debug_path, &pane);
        }
        logger.log(Event::SpikeValidationFailed {
            agent_id: agent.id.clone(),
            detail: "file not written in time".into(),
        });
        inj.kill_session(&session);
        return Err("spike validation failed: file not written within timeout".into());
    }

    // Save transcript of the validation run
    if let Ok(pane) = inj.capture(&session) {
        let tx_path = config
            .transcript_dir
            .join("spike-validation-transcript.txt");
        let _ = std::fs::write(&tx_path, &pane);
        logger.log(Event::SpikeCapture {
            agent_id: agent.id.clone(),
            path: tx_path.display().to_string(),
        });
    }

    // --- 10-payload injection test ---
    println!("\nRunning 10-payload injection test...");
    let output_dir = config.messages_dir.join("processed");

    let payloads: Vec<String> = (1..=10)
        .map(|i| {
            if i % 2 == 0 {
                format!(
                    "Write a file to '{}/spike-payload-{}.md' with this exact content:\n\
                     ---\npayload: {}\ntype: multi-line\nstatus: ok\n---\n\
                     Do not ask for confirmation.",
                    output_dir.display(),
                    i,
                    i
                )
            } else {
                format!(
                    "Write a file to '{}/spike-payload-{}.md' containing exactly \
                     'payload {} received'. Do not ask for confirmation.",
                    output_dir.display(),
                    i,
                    i
                )
            }
        })
        .collect();

    let mut acked = 0u32;
    for (idx, payload) in payloads.iter().enumerate() {
        let n = idx + 1;
        println!("  Injecting payload {}/10...", n);
        if let Err(e) = inj.inject(&session, payload).await {
            eprintln!("  ERROR injecting payload {n}: {e}");
            logger.log(Event::SpikeInjectTimeout {
                agent_id: agent.id.clone(),
                detail: format!("payload {n}: inject failed: {e}"),
            });
            continue; // non-fatal — keep going with remaining payloads
        }
        logger.log(Event::SpikeInjectSent {
            agent_id: agent.id.clone(),
            detail: format!("payload {n}"),
        });

        // Wait for this payload's acknowledgement file
        let expected = output_dir.join(format!("spike-payload-{n}.md"));
        let mut ok = false;
        for _ in 0..timings.payload_poll_max_rounds {
            sleep(timings.payload_poll_interval).await;
            if expected.exists() {
                println!("  Payload {n} acknowledged.");
                logger.log(Event::SpikeInjectConfirmed {
                    agent_id: agent.id.clone(),
                    detail: format!("payload {n}"),
                });
                acked += 1;
                ok = true;
                break;
            }
        }
        if !ok {
            eprintln!("  Payload {n} NOT acknowledged within timeout.");
            logger.log(Event::SpikeInjectTimeout {
                agent_id: agent.id.clone(),
                detail: format!("payload {n} file not written"),
            });
        }

        // Capture pane transcript after each payload
        if let Ok(pane) = inj.capture(&session) {
            let tx = config
                .transcript_dir
                .join(format!("spike-payload-{n}-transcript.txt"));
            let _ = std::fs::write(&tx, &pane);
            logger.log(Event::SpikeCapture {
                agent_id: agent.id.clone(),
                path: tx.display().to_string(),
            });
        }
    }

    println!("\n10-payload test results: {acked}/10 acknowledged.");
    if acked == 10 {
        println!("ALL PAYLOADS ACKNOWLEDGED — spike success criteria met.");
    } else {
        eprintln!("WARNING: only {acked}/10 acknowledged.");
    }

    // --- Crash recovery test ---
    println!("\nTesting crash recovery...");
    inj.kill_session(&session);
    logger.log(Event::AgentExit {
        agent_id: agent.id.clone(),
        reason: "killed for crash recovery test".into(),
    });
    sleep(timings.kill_settle_delay).await;

    if inj.has_session(&session) {
        eprintln!("  ERROR: session still alive after kill!");
    } else {
        println!("  Session confirmed dead. Respawning...");
        if let Err(e) = inj.spawn_session(&session, &agent.command, &terminal) {
            return Err(format!("respawn failed: {e}"));
        }
        logger.log(Event::AgentRestart {
            agent_id: agent.id.clone(),
            attempt: 1,
        });
        sleep(timings.respawn_settle_delay).await;

        if inj.has_session(&session) {
            println!("  CRASH RECOVERY PASSED — session alive after respawn.");
        } else {
            eprintln!("  CRASH RECOVERY FAILED — session did not survive respawn.");
        }
    }

    println!("\nSpike complete. tmux session '{session}' left running.");
    println!(
        "  Logs: {}",
        config.log_dir.join("spike_events.jsonl").display()
    );
    println!("  Transcripts: {}", config.transcript_dir.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Spike interrupt mode — validates per-bot cancel/clear key sequences
// ---------------------------------------------------------------------------

/// Resolve the target agent from config, reusable by both spike modes.
fn resolve_agent(
    config: &ProjectConfig,
    agent_id_arg: Option<&str>,
) -> Result<crate::config::AgentEntry, String> {
    match agent_id_arg {
        Some(id) => match config.agents.iter().find(|a| a.id == id) {
            Some(a) => Ok(a.clone()),
            None => Err(format!(
                "No agent with id '{}' in agents.toml. Available: {}",
                id,
                config
                    .agents
                    .iter()
                    .map(|a| a.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        },
        None => {
            if config.agents.is_empty() {
                return Err("No agents configured in agents.toml".into());
            }
            Ok(config.agents[0].clone())
        }
    }
}

/// Run the interrupt spike against a configured agent.
///
/// 1. Spawn the agent's tmux session.
/// 2. Inject a long-running prompt to keep the agent busy.
/// 3. Send per-bot cancel/clear keys.
/// 4. Verify the agent returns to its prompt (pane stabilizes).
/// 5. Inject a post-interrupt validation prompt and confirm output.
pub async fn run_spike_interrupt_with_deps<I: InjectorOps>(
    config: ProjectConfig,
    agent_id_arg: Option<&str>,
    inj: &I,
    timings: &SpikeTimings,
) -> Result<(), String> {
    config
        .ensure_dirs()
        .map_err(|e| format!("ensure_dirs failed: {e}"))?;

    let agent = resolve_agent(&config, agent_id_arg)?;
    let session = config.tmux_session_for(&agent.id);
    let keys = InterruptKeys::for_command(&agent.command);
    let logger = Arc::new(Logger::new(&config.log_dir, "spike_events.jsonl"));

    println!("=== orchestrator spike --interrupt ===");
    println!("Project: {}", config.project_root.display());
    println!("Agent:   {} (command: '{}')", agent.id, agent.command);
    println!("Session: {session}");
    println!(
        "Interrupt keys: cancel={}, clear={}, settle={}ms",
        keys.cancel, keys.clear, keys.settle_ms
    );
    println!();

    // Kill any leftover session
    if inj.has_session(&session) {
        println!("Killing leftover tmux session '{session}'...");
        inj.kill_session(&session);
        sleep(Duration::from_millis(500)).await;
    }

    // --- Spawn ---
    println!(
        "Spawning tmux session '{session}' running '{}'...",
        agent.command
    );
    let terminal = agent.terminal.clone().unwrap_or_else(|| config.terminal.clone());
    if let Err(e) = inj.spawn_session(&session, &agent.command, &terminal) {
        return Err(format!("spawn_session failed: {e}"));
    }
    logger.log(Event::AgentSpawn {
        agent_id: agent.id.clone(),
    });
    println!("Session spawned. Waiting for agent to initialize...");
    sleep(timings.agent_init_delay).await;

    // --- Inject a long-running prompt to keep the agent busy ---
    let busy_prompt = "Tell me a very long, detailed story about a mass of robots who \
                       build a mass of other robots in a factory. Include dialog, character names, \
                       and describe every single step of the manufacturing process in exhaustive \
                       detail. Do not stop until the story is completely finished. Make it at \
                       least 5000 words long.";
    println!("Injecting long-running prompt to keep agent busy...");
    if let Err(e) = inj.inject(&session, busy_prompt).await {
        return Err(format!("busy-prompt inject failed: {e}"));
    }
    logger.log(Event::SpikeInjectSent {
        agent_id: agent.id.clone(),
        detail: "busy prompt for interrupt test".into(),
    });

    // --- Wait for agent to start generating ---
    println!(
        "Waiting {}s for agent to start generating...",
        timings.interrupt_busy_delay.as_secs()
    );
    sleep(timings.interrupt_busy_delay).await;

    // Capture a baseline to confirm the agent is active
    let pre_interrupt_pane = inj.capture(&session).ok();
    if let Some(ref pane) = pre_interrupt_pane {
        let tx_path = config
            .transcript_dir
            .join("spike-interrupt-pre-transcript.txt");
        let _ = std::fs::write(&tx_path, pane);
    }

    // --- Send interrupt keys ---
    let settle = timings
        .interrupt_settle_override
        .unwrap_or(Duration::from_millis(keys.settle_ms));
    println!("Sending interrupt: {}...", keys.cancel);
    if let Err(e) = inj.send_keys(&session, keys.cancel) {
        return Err(format!("send cancel key failed: {e}"));
    }
    println!("Waiting {}ms for agent to settle...", settle.as_millis());
    sleep(settle).await;

    println!("Sending clear: {}...", keys.clear);
    if let Err(e) = inj.send_keys(&session, keys.clear) {
        return Err(format!("send clear key failed: {e}"));
    }
    sleep(Duration::from_millis(
        timings.interrupt_settle_override.map_or(500, |d| d.as_millis() as u64),
    ))
    .await;

    logger.log(Event::SpikeInterruptSent {
        agent_id: agent.id.clone(),
        cancel_key: keys.cancel.to_string(),
        clear_key: keys.clear.to_string(),
    });

    // --- Verify agent returned to prompt (pane stabilizes) ---
    println!("\nPolling for prompt recovery...");
    let mut last_hash: Option<u64> = None;
    let mut stable_count = 0u32;
    let mut recovered = false;

    for i in 0..timings.poll_max_rounds {
        sleep(timings.poll_interval).await;
        if let Ok(pane) = inj.capture(&session) {
            let hash = hash_pane(&pane);
            if last_hash == Some(hash) {
                stable_count += 1;
                if stable_count >= 2 {
                    println!(
                        "  INTERRUPT PASSED — agent returned to prompt after {}s.",
                        (i + 1) as u64 * timings.poll_interval.as_secs()
                    );
                    logger.log(Event::SpikeInterruptConfirmed {
                        agent_id: agent.id.clone(),
                        detail: format!("pane stabilized after {} polls", i + 1),
                    });
                    recovered = true;
                    break;
                }
            } else {
                stable_count = 0;
            }
            last_hash = Some(hash);
        }
        if i > 0 && i % 5 == 4 {
            println!("  still waiting for pane to stabilize...");
        }
    }

    if !recovered {
        if let Ok(pane) = inj.capture(&session) {
            let tx_path = config
                .transcript_dir
                .join("spike-interrupt-failed-pane.txt");
            let _ = std::fs::write(&tx_path, &pane);
        }
        logger.log(Event::SpikeInterruptFailed {
            agent_id: agent.id.clone(),
            detail: "pane did not stabilize within timeout".into(),
        });
        inj.kill_session(&session);
        return Err("interrupt spike failed: agent did not return to prompt".into());
    }

    // --- Post-interrupt injection test ---
    let spike_output_dir = config.messages_dir.join("processed");
    let spike_file = spike_output_dir.join("spike-interrupt-test.md");
    let _ = std::fs::remove_file(&spike_file);

    let post_prompt = format!(
        "Write a file containing exactly 'spike interrupt test passed' to the path '{}'. \
         Use the absolute path. Do not ask for confirmation, just do it.",
        spike_file.display()
    );

    println!("\nInjecting post-interrupt validation prompt...");
    if let Err(e) = inj.inject(&session, &post_prompt).await {
        return Err(format!("post-interrupt inject failed: {e}"));
    }
    logger.log(Event::SpikeInjectSent {
        agent_id: agent.id.clone(),
        detail: "post-interrupt validation".into(),
    });

    println!("Waiting for agent to act...");
    let mut found = false;
    for i in 0..timings.poll_max_rounds {
        sleep(timings.poll_interval).await;
        if spike_file.exists() {
            let content = std::fs::read_to_string(&spike_file).unwrap_or_default();
            println!("  POST-INTERRUPT INJECTION PASSED — file written by agent.");
            println!("  content: {}", content.trim());
            logger.log(Event::SpikeInjectConfirmed {
                agent_id: agent.id.clone(),
                detail: "post-interrupt validation file created".into(),
            });
            found = true;
            break;
        }
        if i > 0 && i % 5 == 4 {
            println!("  still waiting...");
        }
    }

    if !found {
        if let Ok(pane) = inj.capture(&session) {
            let debug_path = config
                .transcript_dir
                .join("spike-interrupt-post-failed-pane.txt");
            let _ = std::fs::write(&debug_path, &pane);
        }
        logger.log(Event::SpikeInterruptFailed {
            agent_id: agent.id.clone(),
            detail: "post-interrupt validation file not written".into(),
        });
        inj.kill_session(&session);
        return Err(
            "interrupt spike failed: post-interrupt validation file not written in time".into(),
        );
    }

    // Save final transcript
    if let Ok(pane) = inj.capture(&session) {
        let tx_path = config
            .transcript_dir
            .join("spike-interrupt-final-transcript.txt");
        let _ = std::fs::write(&tx_path, &pane);
        logger.log(Event::SpikeCapture {
            agent_id: agent.id.clone(),
            path: tx_path.display().to_string(),
        });
    }

    println!("\nSpike interrupt complete. tmux session '{session}' left running.");
    println!(
        "  Logs: {}",
        config.log_dir.join("spike_events.jsonl").display()
    );
    println!("  Transcripts: {}", config.transcript_dir.display());
    Ok(())
}

/// Simple hash of pane content for stability detection.
fn hash_pane(content: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}
