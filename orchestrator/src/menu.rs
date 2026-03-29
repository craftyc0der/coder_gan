use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::config::ProjectConfig;
use crate::logger::{Event, Logger};
use crate::supervisor::Registry;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Prompt for a single line of input. Returns the trimmed string,
/// or `None` on EOF. Empty input (just Enter) returns `Some("")`.
async fn prompt_line(
    lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
    prompt: &str,
) -> Option<String> {
    print!("{}", prompt);
    let _ = std::io::Write::flush(&mut std::io::stdout());
    match lines.next_line().await {
        Ok(Some(l)) => Some(l.trim().to_string()),
        _ => None,
    }
}

/// Returns true if the input is a "back" command (empty, "b", "back").
fn is_back(input: &str) -> bool {
    matches!(input, "" | "b" | "B" | "back")
}

fn parse_group_instance(base_agent_id: &str, live_agent_id: &str) -> Option<u32> {
    if live_agent_id == base_agent_id {
        return Some(1);
    }

    live_agent_id
        .strip_prefix(base_agent_id)
        .and_then(|suffix| suffix.strip_prefix('-'))
        .and_then(|suffix| suffix.parse::<u32>().ok())
}

fn live_group_instances_from_ids(group_agent_ids: &[String], live_agent_ids: &[String]) -> Vec<u32> {
    let mut instances = BTreeSet::new();

    for live_agent_id in live_agent_ids {
        for group_agent_id in group_agent_ids {
            if let Some(instance) = parse_group_instance(group_agent_id, live_agent_id) {
                instances.insert(instance);
                break;
            }
        }
    }

    instances.into_iter().collect()
}

async fn live_group_instances(registry: &Registry, group: &crate::config::WorkerGroupEntry) -> Vec<u32> {
    let agents = registry.agents.lock().await;
    let live_agent_ids: Vec<String> = agents.keys().cloned().collect();
    drop(agents);
    live_group_instances_from_ids(&group.agents, &live_agent_ids)
}

fn highest_instances_to_remove(live_instances: &[u32], target_count: u32) -> Vec<u32> {
    let remove_count = live_instances.len().saturating_sub(target_count as usize);
    live_instances
        .iter()
        .rev()
        .take(remove_count)
        .copied()
        .collect()
}

// ---------------------------------------------------------------------------
// Main menu
// ---------------------------------------------------------------------------

/// Run the interactive CLI menu loop. Returns when the user chooses exit
/// or Ctrl+C is caught by the caller.
pub async fn run_menu(
    registry: Registry,
    logger: Arc<Logger>,
    config: &mut ProjectConfig,
) {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    print_main_menu(registry.is_paused());

    loop {
        let input = match prompt_line(&mut lines, "> ").await {
            Some(s) => s,
            None => break, // EOF
        };

        match input.as_str() {
            "1" => handle_pause_unpause(&registry, &logger).await,
            "2" => handle_stream_logs(&config.log_dir, &mut lines).await,
            "3" => handle_list_messages(&config.messages_dir, &mut lines).await,
            "4" => handle_modify_workers(&registry, &logger, config, &mut lines).await,
            "5" => handle_send_system_prompts(&registry, &logger, config).await,
            "6" => handle_restart_agent(&registry, &mut lines).await,
            "7" => handle_status(&registry).await,
            "8" => break, // exit — same as Ctrl+C
            "" => {} // just redraw menu
            _ => println!("  Unknown option: {}", input),
        }

        println!();
        print_main_menu(registry.is_paused());
    }
}

fn print_main_menu(paused: bool) {
    let pause_label = if paused { "Unpause" } else { "Pause" };
    println!("╔══════════════════════════════════════╗");
    println!(
        "║  Orchestrator Menu{}  ║",
        if paused { " [PAUSED]" } else { "          " }
    );
    println!("╠══════════════════════════════════════╣");
    println!("║  1) {:<33}║", pause_label);
    println!("║  2) Stream logs                      ║");
    println!("║  3) List recent messages              ║");
    println!("║  4) Modify workers (scale teams)     ║");
    println!("║  5) Send system prompts              ║");
    println!("║  6) Restart agent                    ║");
    println!("║  7) Status                           ║");
    println!("║  8) Exit (shutdown all agents)       ║");
    println!("╚══════════════════════════════════════╝");
}

// ---------------------------------------------------------------------------
// 1) Pause / Unpause
// ---------------------------------------------------------------------------

async fn handle_pause_unpause(registry: &Registry, logger: &Logger) {
    if registry.is_paused() {
        registry.set_paused(false);
        logger.log(Event::OrchestratorUnpaused);
        println!("\n  Unpaused. Queued messages will now be forwarded and timers resume.");
    } else {
        registry.set_paused(true);
        logger.log(Event::OrchestratorPaused);
        println!("\n  Paused. Messages will be queued, timers suspended.");
        println!("  Agents will finish their current tasks and become idle.");
    }
}

// ---------------------------------------------------------------------------
// 2) Stream logs
// ---------------------------------------------------------------------------

async fn handle_stream_logs(
    log_dir: &Path,
    lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
) {
    let events_path = log_dir.join("events.jsonl");
    if !events_path.exists() {
        println!("  No event log found.");
        return;
    }

    println!("\n  Streaming logs (press Enter to stop)...\n");

    // Show the last 20 lines first
    let content = match std::fs::read_to_string(&events_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  Failed to read log: {e}");
            return;
        }
    };

    let log_lines: Vec<&str> = content.lines().collect();
    let start = if log_lines.len() > 20 {
        log_lines.len() - 20
    } else {
        0
    };
    for line in &log_lines[start..] {
        println!("  {}", line);
    }

    let last_len = content.len();

    // Tail the file until user presses Enter
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_clone = stop.clone();
    let path_clone = events_path.clone();

    let tail_handle = tokio::spawn(async move {
        let mut pos = last_len;
        loop {
            if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            if let Ok(content) = std::fs::read_to_string(&path_clone) {
                if content.len() > pos {
                    let new_data = &content[pos..];
                    for line in new_data.lines() {
                        if !line.is_empty() {
                            println!("  {}", line);
                        }
                    }
                    pos = content.len();
                }
            }
        }
    });

    // Wait for Enter to go back
    let _ = lines.next_line().await;

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = tail_handle.await;
    println!("  Stopped streaming.");
}

// ---------------------------------------------------------------------------
// 3) List recent messages
// ---------------------------------------------------------------------------

async fn handle_list_messages(
    messages_dir: &Path,
    lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
) {
    let processed_dir = messages_dir.join("processed");

    // Collect messages from all to_* dirs and processed/
    let mut all_messages: Vec<(std::time::SystemTime, PathBuf, bool)> = Vec::new();

    // From to_* directories (pending)
    if let Ok(entries) = std::fs::read_dir(messages_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_str().unwrap_or("");
            if name_str.starts_with("to_") && entry.path().is_dir() {
                if let Ok(inbox_entries) = std::fs::read_dir(entry.path()) {
                    for ie in inbox_entries.flatten() {
                        let ie_name = ie.file_name();
                        if ie_name.to_str().map(|n| n.starts_with('.')).unwrap_or(true) {
                            continue;
                        }
                        let mtime = ie
                            .metadata()
                            .and_then(|m| m.modified())
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                        all_messages.push((mtime, ie.path(), false));
                    }
                }
            }
        }
    }

    // From processed/
    if let Ok(entries) = std::fs::read_dir(&processed_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_str().map(|n| n.starts_with('.')).unwrap_or(true) {
                continue;
            }
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            all_messages.push((mtime, entry.path(), true));
        }
    }

    // Sort by modification time descending
    all_messages.sort_by(|a, b| b.0.cmp(&a.0));

    // Take last 20
    let display: Vec<_> = all_messages.into_iter().take(20).collect();

    if display.is_empty() {
        println!("\n  No messages found.");
        return;
    }

    loop {
        println!("\n  Recent messages (newest first):");
        println!("  {:<4} {:<10} {:<50}", "#", "STATUS", "FILENAME");
        println!("  {}", "-".repeat(66));

        for (i, (_, path, processed)) in display.iter().enumerate() {
            let fname = path
                .file_name()
                .and_then(|n: &std::ffi::OsStr| n.to_str())
                .unwrap_or("?");
            let status = if *processed { "delivered" } else { "pending" };
            println!("  {:<4} {:<10} {}", i + 1, status, fname);
        }

        let input = match prompt_line(lines, "\n  Enter # to read, or Enter to go back: ").await {
            Some(s) => s,
            None => return,
        };

        if is_back(&input) {
            return;
        }

        if let Ok(num) = input.parse::<usize>() {
            if num >= 1 && num <= display.len() {
                let (_, ref path, _) = display[num - 1];
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        let name = path
                            .file_name()
                            .and_then(|n: &std::ffi::OsStr| n.to_str())
                            .unwrap_or("?");
                        println!("\n  --- {} ---", name);
                        for line in content.lines() {
                            println!("  {}", line);
                        }
                        println!("  --- end ---");
                    }
                    Err(e) => println!("  Failed to read: {e}"),
                }
                // After displaying, loop back to the message list
            } else {
                println!("  Invalid number.");
            }
        } else {
            println!("  Invalid input.");
        }
    }
}

// ---------------------------------------------------------------------------
// 4) Modify workers (scale teams)
// ---------------------------------------------------------------------------

async fn handle_modify_workers(
    registry: &Registry,
    logger: &Logger,
    config: &mut ProjectConfig,
    lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
) {
    if config.worker_groups.is_empty() {
        println!("\n  No worker groups configured. Nothing to modify.");
        return;
    }

    loop {
        println!("\n  Worker groups:");
        for (i, group) in config.worker_groups.iter().enumerate() {
            let current = live_group_instances(registry, group).await.len();

            println!(
                "  {}) {} — agents: [{}], current instances: {}",
                i + 1,
                group.id,
                group.agents.join(", "),
                current
            );
        }

        let input =
            match prompt_line(lines, "\n  Enter group # to modify, or Enter to go back: ").await {
                Some(s) => s,
                None => return,
            };

        if is_back(&input) {
            return;
        }

        let group_idx = match input.parse::<usize>() {
            Ok(n) if n >= 1 && n <= config.worker_groups.len() => n - 1,
            _ => {
                println!("  Invalid selection.");
                continue;
            }
        };

        let (group_id, group_agents) = {
            let group = &config.worker_groups[group_idx];
            (group.id.clone(), group.agents.clone())
        };

        let live_instances = live_group_instances(registry, &config.worker_groups[group_idx]).await;
        let old_count = live_instances.len() as u32;

        let count_input = match prompt_line(
            lines,
            &format!(
                "  Enter new team count (currently: {}), or Enter to go back: ",
                old_count
            ),
        )
        .await
        {
            Some(s) => s,
            None => return,
        };

        if is_back(&count_input) {
            continue; // back to group list
        }

        let new_count = match count_input.parse::<u32>() {
            Ok(n) if n >= 1 => n,
            _ => {
                println!("  Invalid count. Must be >= 1.");
                continue;
            }
        };

        if new_count == old_count {
            println!("  No change.");
            continue;
        }

        if new_count > old_count {
            // Scale up: spawn new group instances
            println!("  Scaling up {} → {} teams...", old_count, new_count);

            let new_instances: Vec<u32> = ((old_count + 1)..=new_count).collect();

            if let Some(feature_name) = config.worktree_feature.clone() {
                let specs = config.worktree_specs_for_group_instances(&group_id, &new_instances, new_count);
                match crate::worktree::setup_worktrees(&config.project_root, &feature_name, &specs) {
                    Ok(mut worktrees) => {
                        let new_agent_ids: HashSet<String> =
                            worktrees.iter().map(|wt| wt.agent_id.clone()).collect();
                        config
                            .worktrees
                            .retain(|wt| !new_agent_ids.contains(&wt.agent_id));
                        config.worktrees.append(&mut worktrees);
                    }
                    Err(e) => {
                        eprintln!("  Failed to create worktrees: {e}");
                        continue;
                    }
                }
            }

            if let Some(group) = config.worker_groups.get_mut(group_idx) {
                group.count = new_count;
            }

            let group_configs =
                config.worker_group_configs_for_instances(&group_id, &new_instances, new_count);
            let prompts = config
                .startup_prompts_for_group_instances(&group_id, &new_instances, new_count)
                .unwrap_or_default();

            for wg_config in group_configs {
                for m in &wg_config.members {
                    let _ = std::fs::create_dir_all(&m.inbox_dir);
                }

                registry.spawn_and_prompt_group(&wg_config, &prompts).await;
            }

            logger.log(Event::WorkersScaled {
                group_id: group_id.clone(),
                old_count,
                new_count,
            });
            println!("  Scaled up to {} teams.", new_count);
        } else {
            // Scale down: kill highest ordinal instances
            println!("  Scaling down {} → {} teams...", old_count, new_count);

            let instances_to_remove = highest_instances_to_remove(&live_instances, new_count);
            let mut agent_ids_to_remove = HashSet::new();
            let mut sessions_to_kill = HashSet::new();
            let mut terminal_handles_to_close = HashSet::new();

            {
                let agents = registry.agents.lock().await;
                for instance in &instances_to_remove {
                    for (agent_id, state) in agents.iter() {
                        let matches_instance = group_agents.iter().any(|base_agent_id| {
                            parse_group_instance(base_agent_id, agent_id) == Some(*instance)
                        });
                        if matches_instance {
                            agent_ids_to_remove.insert(agent_id.clone());
                            sessions_to_kill.insert(state.tmux_session.clone());
                            if let Some(handle) = state.terminal_handle {
                                terminal_handles_to_close.insert(handle);
                            }
                        }
                    }
                }
            }

            for session in &sessions_to_kill {
                if crate::injector::has_session(session) {
                    crate::injector::kill_session(session);
                    println!("  Killed session: {}", session);
                }
            }
            for handle in terminal_handles_to_close {
                crate::injector::close_terminal_handle(handle);
            }
            let mut agent_ids_to_remove: Vec<String> = agent_ids_to_remove.into_iter().collect();
            agent_ids_to_remove.sort();
            let mut session_names: Vec<String> = sessions_to_kill.into_iter().collect();
            session_names.sort();
            registry
                .unregister_group_runtime(&agent_ids_to_remove, &session_names)
                .await;

            let removed_agent_ids: HashSet<String> = agent_ids_to_remove.iter().cloned().collect();
            config
                .worktrees
                .retain(|wt| !removed_agent_ids.contains(&wt.agent_id));
            if let Some(group) = config.worker_groups.get_mut(group_idx) {
                group.count = new_count;
            }

            logger.log(Event::WorkersScaled {
                group_id: group_id.clone(),
                old_count,
                new_count,
            });
            println!("  Scaled down to {} teams.", new_count);
        }

        // After scaling, loop back to show updated group list
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{highest_instances_to_remove, live_group_instances_from_ids};

    #[test]
    fn live_group_instances_detects_all_suffixes() {
        let group_agents = vec!["coder".to_string(), "tester".to_string()];
        let live_agent_ids = vec![
            "coder-1".to_string(),
            "tester-1".to_string(),
            "coder-2".to_string(),
            "tester-2".to_string(),
            "coder-3".to_string(),
            "tester-3".to_string(),
        ];

        assert_eq!(
            live_group_instances_from_ids(&group_agents, &live_agent_ids),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn live_group_instances_handles_mixed_unsuffixed_and_suffixed_agents() {
        let group_agents = vec!["coder".to_string(), "tester".to_string()];
        let live_agent_ids = vec![
            "coder-1".to_string(),
            "tester-1".to_string(),
            "coder-3".to_string(),
            "tester-3".to_string(),
            "reviewer".to_string(),
        ];

        assert_eq!(
            live_group_instances_from_ids(&group_agents, &live_agent_ids),
            vec![1, 3]
        );
    }

    #[test]
    fn highest_instances_to_remove_prefers_highest_ordinals() {
        assert_eq!(highest_instances_to_remove(&[1, 2, 3], 1), vec![3, 2]);
        assert_eq!(highest_instances_to_remove(&[1, 2, 3], 2), vec![3]);
        assert_eq!(highest_instances_to_remove(&[1], 1), Vec::<u32>::new());
    }
}

// ---------------------------------------------------------------------------
// 5) Send system prompts
// ---------------------------------------------------------------------------

async fn handle_send_system_prompts(
    registry: &Registry,
    logger: &Logger,
    config: &ProjectConfig,
) {
    println!("\n  Re-reading prompt files from disk and re-injecting...");

    let prompts = match config.startup_prompts() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("  Failed to load prompts: {e}");
            return;
        }
    };

    registry.resend_system_prompts(&prompts).await;
    logger.log(Event::SystemPromptsResent);
    println!("  System prompts resent to all active agents.");
}

// ---------------------------------------------------------------------------
// 6) Restart agent
// ---------------------------------------------------------------------------

async fn handle_restart_agent(
    registry: &Registry,
    lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>,
) {
    loop {
        let agents = registry.agents.lock().await;
        if agents.is_empty() {
            println!("\n  No agents registered.");
            return;
        }

        let mut sorted: Vec<_> = agents.iter().collect();
        sorted.sort_by_key(|(id, _)| (*id).clone());

        println!("\n  Agents:");
        for (i, (id, state)) in sorted.iter().enumerate() {
            println!(
                "  {}) {} — {} / {}",
                i + 1,
                id,
                state.status,
                state.activity,
            );
        }

        let agent_ids: Vec<String> = sorted.iter().map(|(id, _)| (*id).clone()).collect();
        drop(agents);

        let input =
            match prompt_line(lines, "\n  Enter # to restart, or Enter to go back: ").await {
                Some(s) => s,
                None => return,
            };

        if is_back(&input) {
            return;
        }

        let idx = match input.parse::<usize>() {
            Ok(n) if n >= 1 && n <= agent_ids.len() => n - 1,
            _ => {
                println!("  Invalid selection.");
                continue;
            }
        };

        let agent_id = &agent_ids[idx];
        println!("  Restarting {}...", agent_id);
        match registry.restart_agent(agent_id).await {
            Ok(()) => println!("  {} restarted successfully.", agent_id),
            Err(e) => eprintln!("  Failed to restart {}: {e}", agent_id),
        }

        // After restarting, loop back to show updated agent list
    }
}

// ---------------------------------------------------------------------------
// 7) Status
// ---------------------------------------------------------------------------

async fn handle_status(registry: &Registry) {
    let agents = registry.agents.lock().await;

    if agents.is_empty() {
        println!("\n  No agents registered.");
        return;
    }

    println!();
    println!(
        "  {:<16} {:<28} {:<10} {:<8} {:<8} STARTED",
        "AGENT", "SESSION", "STATUS", "ACTIVE", "RESTARTS"
    );
    println!("  {}", "-".repeat(86));

    let mut sorted: Vec<_> = agents.iter().collect();
    sorted.sort_by_key(|(id, _)| (*id).clone());

    for (id, state) in &sorted {
        println!(
            "  {:<16} {:<28} {:<10} {:<8} {:<8} {}",
            id,
            state.tmux_session,
            state.status,
            state.activity,
            state.restart_count,
            state.last_start.format("%H:%M:%S UTC"),
        );
    }

    let paused = registry.is_paused();
    println!(
        "\n  Orchestrator: {}",
        if paused { "PAUSED" } else { "RUNNING" }
    );
}
