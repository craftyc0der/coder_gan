use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::path::PathBuf;

use tempfile::TempDir;

use orchestrator::config::{AgentEntry, ProjectConfig};
use orchestrator::injector::{InjectionError, InjectorOps, InterruptKeys};
use orchestrator::spike::{run_spike_with_deps, run_spike_interrupt_with_deps, SpikeTimings};

#[derive(Default)]
struct MockInjector {
    spawned: Mutex<Vec<(String, String)>>,
    injected: Mutex<Vec<(String, String)>>,
    captured: Mutex<Vec<String>>,
    killed: Mutex<Vec<String>>,
    sent_keys: Mutex<Vec<(String, String)>>,

    spawn_error: Option<String>,
    capture_result: Option<String>,
    /// Sequence of capture results — if set, capture() pops from front.
    /// Falls back to `capture_result` when empty.
    capture_sequence: Mutex<Vec<String>>,
    inject_fail_at: Option<usize>,
    inject_count: AtomicUsize,

    alive_after_kill: bool,
    validation_file: Option<PathBuf>,
    /// Second validation file (for interrupt post-inject test).
    interrupt_validation_file: Option<PathBuf>,
    has_session_calls: Mutex<u32>,
}

impl InjectorOps for MockInjector {
    fn has_session(&self, _session: &str) -> bool {
        let mut calls = self.has_session_calls.lock().unwrap();
        *calls += 1;
        let killed = self.killed.lock().unwrap();
        if killed.is_empty() {
            false
        } else {
            self.alive_after_kill
        }
    }

    fn kill_session(&self, session: &str) {
        self.killed.lock().unwrap().push(session.to_string());
    }

    fn spawn_session(&self, session: &str, cmd: &str) -> Result<Option<u32>, InjectionError> {
        self.spawned
            .lock()
            .unwrap()
            .push((session.to_string(), cmd.to_string()));
        match &self.spawn_error {
            Some(msg) => Err(InjectionError::TmuxCommand {
                step: "new-session".into(),
                detail: msg.clone(),
            }),
            None => Ok(None), // No terminal window in tests
        }
    }

    fn respawn_pane(&self, _session: &str, _cmd: &str) -> Result<(), InjectionError> {
        Ok(())
    }

    fn inject<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>> {
        self.injected
            .lock()
            .unwrap()
            .push((session.to_string(), text.to_string()));
        let idx = self.inject_count.fetch_add(1, Ordering::SeqCst) + 1;
        if idx == 1 {
            if let Some(path) = &self.validation_file {
                let _ = std::fs::write(path, "validation ok");
            }
        }
        // Write interrupt validation file on the second inject call
        if idx == 2 {
            if let Some(path) = &self.interrupt_validation_file {
                let _ = std::fs::write(path, "spike interrupt test passed");
            }
        }
        let result = match self.inject_fail_at {
            Some(fail_at) if fail_at == idx => Err(InjectionError::TmuxCommand {
                step: "inject".into(),
                detail: "mock inject failure".into(),
            }),
            _ => Ok(()),
        };
        Box::pin(async move { result })
    }

    fn capture(&self, session: &str) -> Result<String, InjectionError> {
        self.captured.lock().unwrap().push(session.to_string());
        // Try capture_sequence first
        {
            let mut seq = self.capture_sequence.lock().unwrap();
            if !seq.is_empty() {
                return Ok(seq.remove(0));
            }
        }
        match &self.capture_result {
            Some(text) => Ok(text.clone()),
            None => Err(InjectionError::TmuxCommand {
                step: "capture-pane".into(),
                detail: "mock: no capture configured".into(),
            }),
        }
    }

    fn send_keys(&self, session: &str, keys: &str) -> Result<(), InjectionError> {
        self.sent_keys
            .lock()
            .unwrap()
            .push((session.to_string(), keys.to_string()));
        Ok(())
    }

    fn inject_interrupt<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
        keys: &'a InterruptKeys,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>> {
        // Record the keys that were sent
        self.sent_keys
            .lock()
            .unwrap()
            .push((session.to_string(), keys.cancel.to_string()));
        self.sent_keys
            .lock()
            .unwrap()
            .push((session.to_string(), keys.clear.to_string()));
        // Delegate to normal inject
        self.inject(session, text)
    }

    fn spawn_group_session(
        &self,
        _session: &str,
        _cmds: &[&str],
        _layout: &orchestrator::config::SplitDirection,
    ) -> Result<Option<u32>, InjectionError> {
        Ok(None)
    }

    fn is_pane_alive(&self, _target: &str) -> bool {
        true
    }
    fn set_pane_attention_style(&self, _target: &str, _session: &str) {}
    fn clear_pane_attention_style(&self, _target: &str, _session: &str) {}
}

fn make_config(tmp: &TempDir, agents: Vec<AgentEntry>) -> ProjectConfig {
    let root = tmp.path().to_path_buf();
    let dot = root.join(".orchestrator");
    std::fs::create_dir_all(dot.join("messages/processed")).unwrap();
    std::fs::create_dir_all(dot.join("runtime/logs/spike_transcripts")).unwrap();
    std::fs::create_dir_all(dot.join("runtime/pids")).unwrap();
    std::fs::create_dir_all(dot.join("messages/to_coder")).unwrap();

    ProjectConfig {
        project_root: root.clone(),
        project_name: "testproject".into(),
        dot_dir: dot.clone(),
        messages_dir: dot.join("messages"),
        log_dir: dot.join("runtime/logs"),
        state_path: dot.join("runtime/logs/state.json"),
        transcript_dir: dot.join("runtime/logs/spike_transcripts"),
        agents,
        worker_groups: vec![],
        worktree_feature: None,
        worktrees: vec![],
    }
}

fn make_agents() -> Vec<AgentEntry> {
    vec![
        AgentEntry {
            id: "coder".into(),
            command: "echo".into(),
            prompt_file: "prompts/coder.md".into(),
            allowed_write_dirs: vec!["src/".into()],
            agent_type: Default::default(),
            slack: None,
            timers: vec![],
            branch: None,
            worktree_prompt_file: None,
        },
        AgentEntry {
            id: "tester".into(),
            command: "echo".into(),
            prompt_file: "prompts/tester.md".into(),
            allowed_write_dirs: vec!["tests/".into()],
            agent_type: Default::default(),
            slack: None,
            timers: vec![],
            branch: None,
            worktree_prompt_file: None,
        },
    ]
}

fn read_first_file(dir: &std::path::Path) -> Option<String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.path());
    let first = entries.first()?;
    std::fs::read_to_string(first.path()).ok()
}

#[tokio::test]
async fn agent_selection_defaults_to_first() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    let inj = MockInjector {
        capture_result: Some("pane output".into()),
        validation_file: Some(config.messages_dir.join("processed/spike-test.md")),
        ..Default::default()
    };

    let result = run_spike_with_deps(config, None, &inj, &SpikeTimings::for_testing()).await;
    assert!(result.is_ok());

    let spawned = inj.spawned.lock().unwrap();
    assert!(!spawned.is_empty());
    assert!(spawned[0].0.contains("coder"));
}

#[tokio::test]
async fn agent_selection_named_agent() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    let inj = MockInjector {
        capture_result: Some("pane output".into()),
        validation_file: Some(config.messages_dir.join("processed/spike-test.md")),
        ..Default::default()
    };

    let result = run_spike_with_deps(
        config,
        Some("tester"),
        &inj,
        &SpikeTimings::for_testing(),
    )
    .await;
    assert!(result.is_ok());

    let spawned = inj.spawned.lock().unwrap();
    assert!(!spawned.is_empty());
    assert!(spawned[0].0.contains("tester"));
}

#[tokio::test]
async fn agent_selection_unknown_agent_returns_err() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    let inj = MockInjector::default();

    let result = run_spike_with_deps(
        config,
        Some("bogus"),
        &inj,
        &SpikeTimings::for_testing(),
    )
    .await;
    assert!(result.is_err());
    assert!(inj.spawned.lock().unwrap().is_empty());
}

#[tokio::test]
async fn empty_agents_returns_err() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, Vec::new());
    let inj = MockInjector::default();

    let result = run_spike_with_deps(config, None, &inj, &SpikeTimings::for_testing()).await;
    assert!(result.is_err());
    assert!(inj.spawned.lock().unwrap().is_empty());
}

#[tokio::test]
async fn validation_checkpoint_succeeds_when_file_exists() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    let inj = MockInjector {
        capture_result: Some("pane output".into()),
        validation_file: Some(config.messages_dir.join("processed/spike-test.md")),
        ..Default::default()
    };

    let result = run_spike_with_deps(config, None, &inj, &SpikeTimings::for_testing()).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn validation_timeout_returns_err_and_logs_event() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    let log_dir = config.log_dir.clone();
    let inj = MockInjector::default();

    let result = run_spike_with_deps(config, None, &inj, &SpikeTimings::for_testing()).await;
    assert!(result.is_err());
    assert!(inj.killed.lock().unwrap().len() >= 1);

    let events_path = log_dir.join("spike_events.jsonl");
    let events = std::fs::read_to_string(events_path).unwrap_or_default();
    assert!(events.contains("\"spike_validation_failed\""));
}

#[tokio::test]
async fn stress_test_injects_10_payloads_with_alternating_lines() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    let inj = MockInjector {
        capture_result: Some("pane output".into()),
        validation_file: Some(config.messages_dir.join("processed/spike-test.md")),
        ..Default::default()
    };

    let result = run_spike_with_deps(config, None, &inj, &SpikeTimings::for_testing()).await;
    assert!(result.is_ok());

    let injected = inj.injected.lock().unwrap();
    assert!(injected.len() >= 11);
    let last_ten: Vec<&String> = injected
        .iter()
        .rev()
        .take(10)
        .map(|(_, text)| text)
        .collect();
    assert_eq!(last_ten.len(), 10);

    for i in 0..9 {
        let has_nl = last_ten[i].contains('\n');
        let next_has_nl = last_ten[i + 1].contains('\n');
        assert_ne!(has_nl, next_has_nl);
    }
}

#[tokio::test]
async fn inject_failure_is_non_fatal() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    let inj = MockInjector {
        capture_result: Some("pane output".into()),
        inject_fail_at: Some(4),
        validation_file: Some(config.messages_dir.join("processed/spike-test.md")),
        ..Default::default()
    };

    let result = run_spike_with_deps(config, None, &inj, &SpikeTimings::for_testing()).await;
    assert!(result.is_ok());
    assert!(inj.injected.lock().unwrap().len() >= 11);
}

#[tokio::test]
async fn crash_recovery_kills_and_respawns() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    let inj = MockInjector {
        capture_result: Some("pane output".into()),
        alive_after_kill: false,
        validation_file: Some(config.messages_dir.join("processed/spike-test.md")),
        ..Default::default()
    };

    let result = run_spike_with_deps(config, None, &inj, &SpikeTimings::for_testing()).await;
    assert!(result.is_ok());

    let killed = inj.killed.lock().unwrap();
    assert!(!killed.is_empty());
    let spawned = inj.spawned.lock().unwrap();
    assert!(spawned.len() >= 2);
}

#[tokio::test]
async fn capture_writes_transcript_file() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    let transcript_dir = config.transcript_dir.clone();
    let inj = MockInjector {
        capture_result: Some("pane output".into()),
        validation_file: Some(config.messages_dir.join("processed/spike-test.md")),
        ..Default::default()
    };

    let result = run_spike_with_deps(config, None, &inj, &SpikeTimings::for_testing()).await;
    assert!(result.is_ok());

    let contents = read_first_file(&transcript_dir).unwrap_or_default();
    assert!(contents.contains("pane output"));
}

// ---------------------------------------------------------------------------
// Interrupt spike tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn interrupt_spike_sends_correct_keys_for_claude() {
    let tmp = TempDir::new().unwrap();
    // Use "claude" as command to test C-c / C-u keys
    let agents = vec![AgentEntry {
        id: "coder".into(),
        command: "claude".into(),
        prompt_file: "prompts/coder.md".into(),
        allowed_write_dirs: vec!["src/".into()],
        agent_type: Default::default(),
        slack: None,
        timers: vec![],
        branch: None,
        worktree_prompt_file: None,
    }];
    let config = make_config(&tmp, agents);
    let interrupt_file = config.messages_dir.join("processed/spike-interrupt-test.md");

    // capture_sequence: first call is pre-interrupt baseline, then 3 stable calls
    // for prompt recovery (need stable_count >= 2), then captures for post-inject polling
    let inj = MockInjector {
        capture_sequence: Mutex::new(vec![
            "agent busy generating...".into(), // pre-interrupt baseline
            "$ ".into(),                        // poll 1: sets last_hash
            "$ ".into(),                        // poll 2: stable_count = 1
            "$ ".into(),                        // poll 3: stable_count = 2 → recovered
            "$ ".into(),                        // post-interrupt transcript
            "$ done".into(),                    // post-interrupt poll captures
        ]),
        interrupt_validation_file: Some(interrupt_file),
        ..Default::default()
    };

    let result = run_spike_interrupt_with_deps(
        config,
        None,
        &inj,
        &SpikeTimings::for_testing(),
    )
    .await;
    assert!(result.is_ok(), "spike interrupt failed: {:?}", result);

    // Verify cancel and clear keys were sent
    let sent = inj.sent_keys.lock().unwrap();
    assert!(sent.len() >= 2, "expected at least 2 send_keys calls, got {}", sent.len());
    assert_eq!(sent[0].1, "C-c", "cancel key should be C-c for claude");
    assert_eq!(sent[1].1, "C-u", "clear key should be C-u for claude");
}

#[tokio::test]
async fn interrupt_spike_sends_escape_for_copilot() {
    let tmp = TempDir::new().unwrap();
    let agents = vec![AgentEntry {
        id: "coder".into(),
        command: "copilot".into(),
        prompt_file: "prompts/coder.md".into(),
        allowed_write_dirs: vec!["src/".into()],
        agent_type: Default::default(),
        slack: None,
        timers: vec![],
        branch: None,
        worktree_prompt_file: None,
    }];
    let config = make_config(&tmp, agents);
    let interrupt_file = config.messages_dir.join("processed/spike-interrupt-test.md");

    let inj = MockInjector {
        capture_sequence: Mutex::new(vec![
            "copilot running...".into(),
            "> ".into(),
            "> ".into(),
            "> ".into(),
            "> ".into(),
            "> done".into(),
        ]),
        interrupt_validation_file: Some(interrupt_file),
        ..Default::default()
    };

    let result = run_spike_interrupt_with_deps(
        config,
        None,
        &inj,
        &SpikeTimings::for_testing(),
    )
    .await;
    assert!(result.is_ok());

    let sent = inj.sent_keys.lock().unwrap();
    assert!(sent.len() >= 2);
    assert_eq!(sent[0].1, "Escape", "cancel key should be Escape for copilot");
    assert_eq!(sent[1].1, "Escape", "clear key should be Escape for copilot");
}

#[tokio::test]
async fn interrupt_spike_fails_when_pane_never_stabilizes() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());

    // Every capture returns different content — pane never stabilizes
    let inj = MockInjector {
        capture_sequence: Mutex::new(vec![
            "output 1".into(),
            "output 2".into(),
            "output 3".into(),
            "output 4".into(),
            "output 5".into(),
            "output 6".into(),
            "output 7".into(),
        ]),
        ..Default::default()
    };

    let result = run_spike_interrupt_with_deps(
        config,
        None,
        &inj,
        &SpikeTimings::for_testing(),
    )
    .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("did not return to prompt"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn interrupt_spike_post_inject_succeeds() {
    let tmp = TempDir::new().unwrap();
    let agents = vec![AgentEntry {
        id: "coder".into(),
        command: "gemini".into(),
        prompt_file: "prompts/coder.md".into(),
        allowed_write_dirs: vec!["src/".into()],
        agent_type: Default::default(),
        slack: None,
        timers: vec![],
        branch: None,
        worktree_prompt_file: None,
    }];
    let config = make_config(&tmp, agents);
    let interrupt_file = config.messages_dir.join("processed/spike-interrupt-test.md");

    let inj = MockInjector {
        capture_sequence: Mutex::new(vec![
            "gemini thinking...".into(),
            "❯ ".into(),
            "❯ ".into(),
            "❯ ".into(),
            "❯ ".into(),
            "❯ done".into(),
        ]),
        interrupt_validation_file: Some(interrupt_file.clone()),
        ..Default::default()
    };

    let result = run_spike_interrupt_with_deps(
        config,
        None,
        &inj,
        &SpikeTimings::for_testing(),
    )
    .await;
    assert!(result.is_ok());

    // Verify the interrupt validation file was created
    assert!(interrupt_file.exists(), "interrupt validation file should exist");
    let content = std::fs::read_to_string(&interrupt_file).unwrap();
    assert!(content.contains("spike interrupt test passed"));

    // Verify gemini-specific keys (C-c / C-c)
    let sent = inj.sent_keys.lock().unwrap();
    assert!(sent.len() >= 2);
    assert_eq!(sent[0].1, "C-c");
    assert_eq!(sent[1].1, "C-c");
}

#[tokio::test]
async fn interrupt_keys_for_command_returns_correct_keys() {
    // claude / codex — default
    let keys = InterruptKeys::for_command("claude");
    assert_eq!(keys.cancel, "C-c");
    assert_eq!(keys.clear, "C-u");

    let keys = InterruptKeys::for_command("codex");
    assert_eq!(keys.cancel, "C-c");
    assert_eq!(keys.clear, "C-u");

    // copilot
    let keys = InterruptKeys::for_command("copilot");
    assert_eq!(keys.cancel, "Escape");
    assert_eq!(keys.clear, "Escape");

    // gemini
    let keys = InterruptKeys::for_command("gemini");
    assert_eq!(keys.cancel, "C-c");
    assert_eq!(keys.clear, "C-c");

    // cursor agent (multi-word command)
    let keys = InterruptKeys::for_command("cursor agent");
    assert_eq!(keys.cancel, "C-c");
    assert_eq!(keys.clear, "C-c");
}
