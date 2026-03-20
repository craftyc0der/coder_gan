use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use orchestrator::config::{ResolvedTimer, SplitDirection};
use orchestrator::injector::{InjectionError, InjectorOps, InterruptKeys};
use orchestrator::logger::Logger;
use orchestrator::supervisor::{
    AgentActivity, AgentConfig, Registry, WorkerGroupConfig, attention_patterns,
    detect_attention_pattern,
};

#[derive(Default)]
struct MockInjector {
    spawned: Mutex<Vec<(String, String)>>,
    respawned: Mutex<Vec<(String, String)>>,
    killed: Mutex<Vec<String>>,
    injected: Mutex<Vec<(String, String)>>,
    captured: Mutex<Vec<String>>,
    pane_alive_queue: Mutex<HashMap<String, Vec<bool>>>,

    spawn_error_for: Mutex<HashSet<String>>,
    capture_error: Mutex<bool>,
    terminal_handles: Mutex<HashMap<String, Option<u32>>>,
    terminal_handle_queue: Mutex<HashMap<String, Vec<Option<u32>>>>,

    has_session_queue: Mutex<Vec<bool>>,
    default_has_session: Mutex<bool>,

    inject_count: AtomicUsize,
}

impl MockInjector {
    fn set_has_session_queue(&self, values: Vec<bool>) {
        *self.has_session_queue.lock().unwrap() = values;
    }

    fn set_default_has_session(&self, value: bool) {
        *self.default_has_session.lock().unwrap() = value;
    }

    fn set_pane_alive_queue(&self, target: &str, values: Vec<bool>) {
        self.pane_alive_queue
            .lock()
            .unwrap()
            .insert(target.to_string(), values);
    }

    fn add_spawn_error_for(&self, session: &str) {
        self.spawn_error_for
            .lock()
            .unwrap()
            .insert(session.to_string());
    }

    fn set_capture_error(&self, value: bool) {
        *self.capture_error.lock().unwrap() = value;
    }

    fn set_terminal_handle(&self, session: &str, window_id: Option<u32>) {
        self.terminal_handles
            .lock()
            .unwrap()
            .insert(session.to_string(), window_id);
    }

    #[cfg(not(target_os = "linux"))]
    #[allow(dead_code)]
    fn set_terminal_handle_queue(&self, session: &str, values: Vec<Option<u32>>) {
        self.terminal_handle_queue
            .lock()
            .unwrap()
            .insert(session.to_string(), values);
    }
}

impl InjectorOps for MockInjector {
    fn has_session(&self, _session: &str) -> bool {
        let mut queue = self.has_session_queue.lock().unwrap();
        if !queue.is_empty() {
            queue.remove(0)
        } else {
            *self.default_has_session.lock().unwrap()
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
        if self.spawn_error_for.lock().unwrap().contains(session) {
            Err(InjectionError::TmuxCommand {
                step: "new-session".into(),
                detail: "mock spawn error".into(),
            })
        } else {
            if let Some(values) = self.terminal_handle_queue.lock().unwrap().get_mut(session) {
                if !values.is_empty() {
                    return Ok(values.remove(0));
                }
            }
            let ids = self.terminal_handles.lock().unwrap();
            Ok(ids.get(session).cloned().unwrap_or(None))
        }
    }

    fn respawn_pane(&self, session: &str, cmd: &str) -> Result<(), InjectionError> {
        self.respawned
            .lock()
            .unwrap()
            .push((session.to_string(), cmd.to_string()));
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
        self.inject_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn capture(&self, session: &str) -> Result<String, InjectionError> {
        self.captured.lock().unwrap().push(session.to_string());
        if *self.capture_error.lock().unwrap() {
            Err(InjectionError::TmuxCommand {
                step: "capture-pane".into(),
                detail: "mock capture error".into(),
            })
        } else {
            Ok("mock transcript".into())
        }
    }

    fn send_keys(&self, _session: &str, _keys: &str) -> Result<(), InjectionError> {
        Ok(())
    }

    fn is_pane_alive(&self, target: &str) -> bool {
        let mut queue = self.pane_alive_queue.lock().unwrap();
        match queue.get_mut(target) {
            Some(values) if !values.is_empty() => values.remove(0),
            _ => true,
        }
    }

    fn inject_interrupt<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
        _keys: &'a InterruptKeys,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>> {
        self.inject(session, text)
    }

    fn spawn_group_session(
        &self,
        session: &str,
        cmds: &[&str],
        _layout: &orchestrator::config::SplitDirection,
    ) -> Result<Option<u32>, InjectionError> {
        for cmd in cmds {
            self.spawned.lock().unwrap().push((session.to_string(), cmd.to_string()));
        }
        Ok(None)
    }

    fn set_pane_attention_style(&self, _target: &str, _session: &str) {}
    fn clear_pane_attention_style(&self, _target: &str, _session: &str) {}
}

fn make_agents(tmp: &TempDir) -> Vec<AgentConfig> {
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");
    vec![
        AgentConfig {
            agent_id: "coder".into(),
            cli_command: "echo".into(),
            tmux_session: "testproject-coder".into(),
            tmux_target: "testproject-coder".into(),
            inbox_dir: messages.join("to_coder"),
            allowed_write_dirs: vec![root.join("src/")],
            working_dir: None,
        },
        AgentConfig {
            agent_id: "tester".into(),
            cli_command: "echo".into(),
            tmux_session: "testproject-tester".into(),
            tmux_target: "testproject-tester".into(),
            inbox_dir: messages.join("to_tester"),
            allowed_write_dirs: vec![root.join("tests/")],
            working_dir: None,
        },
    ]
}

fn make_registry(
    tmp: &TempDir,
    injector: Arc<dyn InjectorOps>,
) -> (Registry, Arc<Logger>, PathBuf, PathBuf) {
    let log_dir = tmp.path().join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));
    let configs = make_agents(tmp);
    let registry = Registry::new_with_injector(
        configs,
        state_path.clone(),
        log_dir.clone(),
        logger.clone(),
        injector,
    );
    (registry, logger, log_dir, state_path)
}

fn read_events(path: &std::path::Path) -> Vec<serde_json::Value> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect()
}

#[tokio::test]
async fn spawn_all_spawns_each_agent_and_records_state() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let (registry, _logger, _log_dir, state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;

    let spawned = injector.spawned.lock().unwrap();
    assert_eq!(spawned.len(), 2);

    let state_contents = std::fs::read_to_string(state_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    let mut statuses: Vec<String> = Vec::new();
    if let serde_json::Value::Object(map) = json {
        for (_k, v) in map.iter() {
            if let Some(status) = v.get("status").and_then(|s| s.as_str()) {
                statuses.push(status.to_string());
            }
        }
    }
    assert!(!statuses.is_empty());
    assert!(statuses
        .iter()
        .all(|s| s.eq_ignore_ascii_case("healthy") || s == "Healthy"));
}

#[tokio::test]
async fn spawn_all_kills_existing_session_before_spawn() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_has_session_queue(vec![true, true]);

    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());

    let handle = tokio::spawn(async move { registry.spawn_all(&HashMap::new(), &[]).await });
    tokio::time::advance(Duration::from_secs(1)).await;
    handle.await.unwrap();

    let killed = injector.killed.lock().unwrap();
    assert_eq!(killed.len(), 2);
    let spawned = injector.spawned.lock().unwrap();
    assert_eq!(spawned.len(), 2);
}

#[tokio::test]
async fn spawn_all_injects_startup_prompt_after_delay() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());

    let mut prompts = HashMap::new();
    prompts.insert("coder".to_string(), "hello".to_string());

    let handle = tokio::spawn(async move { registry.spawn_all(&prompts, &[]).await });
    tokio::time::advance(Duration::from_secs(6)).await;
    handle.await.unwrap();

    let injected = injector.injected.lock().unwrap();
    assert_eq!(injected.len(), 1);
    assert!(injected[0].0.contains("coder"));
}

#[tokio::test]
async fn spawn_all_without_prompt_does_not_inject() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());

    let handle = tokio::spawn(async move { registry.spawn_all(&HashMap::new(), &[]).await });
    tokio::time::advance(Duration::from_secs(1)).await;
    handle.await.unwrap();

    let injected = injector.injected.lock().unwrap();
    assert!(injected.is_empty());
}

#[tokio::test]
async fn spawn_all_spawn_failure_is_non_fatal() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.add_spawn_error_for("testproject-coder");
    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;

    let spawned = injector.spawned.lock().unwrap();
    assert_eq!(spawned.len(), 2);
}

#[tokio::test]
async fn health_loop_alive_agent_does_not_restart() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_default_has_session(true);

    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());
    registry.spawn_all(&HashMap::new(), &[]).await;

    let reg_clone = registry.clone();
    let handle = tokio::spawn(async move { reg_clone.health_loop().await });

    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::task::yield_now().await;

    let spawned = injector.spawned.lock().unwrap();
    assert_eq!(spawned.len(), 2);

    handle.abort();
}

#[tokio::test]
async fn health_loop_dead_agent_restarts_and_logs() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_default_has_session(false);

    let (registry, _logger, log_dir, state_path) = make_registry(&tmp, injector.clone());
    registry.spawn_all(&HashMap::new(), &[]).await;

    let reg_clone = registry.clone();
    let handle = tokio::spawn(async move { reg_clone.health_loop().await });

    for _ in 0..3 {
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;
    }

    let spawned = injector.spawned.lock().unwrap();
    assert!(spawned.len() >= 3);

    let events = read_events(&log_dir.join("events.jsonl"));
    let event_names: Vec<String> = events
        .iter()
        .filter_map(|v| {
            v.get("event")
                .and_then(|e| e.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(event_names.contains(&"agent_exit".to_string()));
    assert!(event_names.contains(&"agent_restart".to_string()));

    let state_contents = std::fs::read_to_string(state_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    let mut restart_counts: Vec<u64> = Vec::new();
    if let serde_json::Value::Object(map) = json {
        for (_k, v) in map.iter() {
            if let Some(count) = v.get("restart_count").and_then(|c| c.as_u64()) {
                restart_counts.push(count);
            }
        }
    }
    assert!(!restart_counts.is_empty());
    assert!(restart_counts.iter().any(|c| *c >= 1));

    handle.abort();
}

#[tokio::test]
async fn health_loop_degrades_after_five_deaths() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_default_has_session(false);

    let (registry, _logger, log_dir, state_path) = make_registry(&tmp, injector.clone());
    registry.spawn_all(&HashMap::new(), &[]).await;

    let reg_clone = registry.clone();
    let handle = tokio::spawn(async move { reg_clone.health_loop().await });

    let mut degraded = false;
    for _ in 0..40 {
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        let events = read_events(&log_dir.join("events.jsonl"));
        if events
            .iter()
            .any(|v| v.get("event").and_then(|e| e.as_str()) == Some("agent_degraded"))
        {
            degraded = true;
            break;
        }
    }
    assert!(degraded, "expected agent_degraded event");

    let state_contents = std::fs::read_to_string(state_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    let mut statuses: Vec<String> = Vec::new();
    if let serde_json::Value::Object(map) = json {
        for (_k, v) in map.iter() {
            if let Some(status) = v.get("status").and_then(|s| s.as_str()) {
                statuses.push(status.to_string());
            }
        }
    }
    assert!(!statuses.is_empty());
    assert!(statuses.iter().any(|s| s.eq_ignore_ascii_case("degraded")));

    let spawned_before = injector.spawned.lock().unwrap().len();
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;
    let spawned_after = injector.spawned.lock().unwrap().len();
    assert_eq!(spawned_before, spawned_after);

    handle.abort();
}

#[tokio::test]
async fn kill_all_calls_kill_session_for_each_agent() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;
    registry.kill_all().await;

    let killed = injector.killed.lock().unwrap();
    assert_eq!(killed.len(), 2);
}

#[tokio::test]
#[cfg(not(target_os = "linux"))]
async fn spawn_all_records_terminal_handle_when_present() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_terminal_handle("testproject-coder", Some(42));
    let (registry, _logger, _log_dir, state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;

    let state_contents = std::fs::read_to_string(state_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    let map = json.as_object().unwrap();

    let coder_state = map.get("coder").unwrap().as_object().unwrap();
    assert_eq!(
        coder_state.get("terminal_handle").and_then(|v| v.as_u64()),
        Some(42)
    );

    let tester_state = map.get("tester").unwrap().as_object().unwrap();
    assert!(
        !tester_state.contains_key("terminal_handle"),
        "expected terminal_handle to be omitted when None"
    );
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn spawn_all_omits_terminal_handle_on_linux() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_terminal_handle("testproject-coder", Some(42));
    let (registry, _logger, _log_dir, state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;

    let state_contents = std::fs::read_to_string(state_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    let map = json.as_object().unwrap();

    let coder_state = map.get("coder").unwrap().as_object().unwrap();
    assert!(
        !coder_state.contains_key("terminal_handle"),
        "expected terminal_handle to be omitted on Linux"
    );
}

#[tokio::test]
async fn spawn_all_omits_terminal_handle_when_none() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let (registry, _logger, _log_dir, state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;

    let state_contents = std::fs::read_to_string(state_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    let map = json.as_object().unwrap();
    for (_id, state) in map.iter() {
        let state_obj = state.as_object().unwrap();
        assert!(
            !state_obj.contains_key("terminal_handle"),
            "expected terminal_handle to be omitted when None"
        );
    }
}

#[tokio::test]
async fn kill_all_with_terminal_handle_does_not_panic() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_terminal_handle("testproject-coder", Some(7));
    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;
    registry.kill_all().await;

    let killed = injector.killed.lock().unwrap();
    assert_eq!(killed.len(), 2);
}

#[test]
fn close_terminal_handle_is_noop_and_does_not_panic() {
    let result = std::panic::catch_unwind(|| {
        orchestrator::injector::close_terminal_handle(0);
        orchestrator::injector::close_terminal_handle(42);
        orchestrator::injector::close_terminal_handle(u32::MAX);
    });
    assert!(result.is_ok());
}

#[tokio::test]
#[cfg(not(target_os = "linux"))]
async fn kill_all_after_respawn_uses_updated_terminal_handle() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_default_has_session(false);
    injector.set_terminal_handle_queue(
        "testproject-coder",
        vec![
            Some(1),
            Some(2),
            Some(2),
            Some(2),
            Some(2),
            Some(2),
            Some(2),
            Some(2),
        ],
    );

    let (registry, _logger, _log_dir, state_path) = make_registry(&tmp, injector.clone());
    registry.spawn_all(&HashMap::new(), &[]).await;

    let reg_clone = registry.clone();
    let handle = tokio::spawn(async move { reg_clone.health_loop().await });

    let mut terminal_id = None;
    for _ in 0..5 {
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;

        let state_contents = std::fs::read_to_string(&state_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
        let map = json.as_object().unwrap();
        let coder_state = map.get("coder").unwrap().as_object().unwrap();
        terminal_id = coder_state.get("terminal_handle").and_then(|v| v.as_u64());
        if terminal_id == Some(2) {
            break;
        }
    }
    assert_eq!(terminal_id, Some(2));

    handle.abort();
}

#[tokio::test]
async fn session_for_returns_correct_session_name() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;
    let session = registry.session_for("coder").await;
    assert_eq!(session, Some("testproject-coder".to_string()));
}

#[tokio::test]
async fn session_for_unknown_agent_returns_none() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;
    let session = registry.session_for("unknown").await;
    assert_eq!(session, None);
}

#[tokio::test]
async fn session_for_grouped_agent_returns_pane_target() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");

    let coder_cfg = AgentConfig {
        agent_id: "coder".into(),
        cli_command: "claude".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.0".into(),
        inbox_dir: messages.join("to_coder"),
        allowed_write_dirs: vec![],
            working_dir: None,
    };
    let tester_cfg = AgentConfig {
        agent_id: "tester".into(),
        cli_command: "codex".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.1".into(),
        inbox_dir: messages.join("to_tester"),
        allowed_write_dirs: vec![],
            working_dir: None,
    };

    let groups = vec![WorkerGroupConfig {
        group_id: "worker".into(),
        session_name: "testproject-worker".into(),
        layout: SplitDirection::Horizontal,
        members: vec![coder_cfg.clone(), tester_cfg],
    }];

    let log_dir = tmp.path().join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));
    let registry = Registry::new_with_injector(
        vec![coder_cfg, groups[0].members[1].clone()],
        state_path,
        log_dir,
        logger,
        injector,
    );

    registry.spawn_all(&HashMap::new(), &groups).await;

    let session = registry.session_for("coder").await;
    assert_eq!(session, Some("testproject-worker:0.0".to_string()));
}

#[tokio::test]
async fn transcript_loop_captures_and_appends() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let (registry, _logger, log_dir, _state_path) = make_registry(&tmp, injector.clone());

    registry.spawn_all(&HashMap::new(), &[]).await;

    let reg_clone = registry.clone();
    let handle = tokio::spawn(async move { reg_clone.transcript_loop().await });

    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::task::yield_now().await;

    fn visit_dir(dir: &std::path::Path, found: &mut bool) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit_dir(&path, found);
                } else if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .ends_with("_transcript.log")
                {
                    let contents = std::fs::read_to_string(&path).unwrap_or_default();
                    if contents.contains("mock transcript") {
                        *found = true;
                        return;
                    }
                }
            }
        }
    }

    let mut found = false;
    visit_dir(tmp.path(), &mut found);
    assert!(found, "expected transcript content in *_transcript.log");

    let captured = injector.captured.lock().unwrap();
    assert!(captured.iter().any(|s| s.contains("coder")));

    let events = read_events(&log_dir.join("events.jsonl"));
    assert!(
        events
            .iter()
            .any(|v| v.get("event").and_then(|e| e.as_str()) == Some("transcript_captured")),
        "expected transcript_captured event"
    );

    handle.abort();
}

#[tokio::test]
async fn transcript_loop_skips_dead_agents() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_default_has_session(false);
    injector.set_capture_error(false);
    injector.add_spawn_error_for("testproject-coder");
    injector.add_spawn_error_for("testproject-tester");

    let (registry, _logger, _log_dir, _state_path) = make_registry(&tmp, injector.clone());
    registry.spawn_all(&HashMap::new(), &[]).await;

    let reg_clone = registry.clone();
    let health = tokio::spawn(async move { reg_clone.health_loop().await });

    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;

    let reg_clone = registry.clone();
    let transcript = tokio::spawn(async move { reg_clone.transcript_loop().await });

    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::task::yield_now().await;

    let captured = injector.captured.lock().unwrap();
    assert!(captured.is_empty());

    health.abort();
    transcript.abort();
}

// ---------------------------------------------------------------------------
// spawn_all with worker groups
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_all_with_worker_group_calls_spawn_group_session() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");
    let worktree_dir = root.join("worktrees/worker-1");

    let coder_cfg = AgentConfig {
        agent_id: "coder".into(),
        cli_command: "claude".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.0".into(),
        inbox_dir: messages.join("to_coder"),
        allowed_write_dirs: vec![root.join("src/")],
        working_dir: Some(worktree_dir.clone()),
    };
    let tester_cfg = AgentConfig {
        agent_id: "tester".into(),
        cli_command: "codex".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.1".into(),
        inbox_dir: messages.join("to_tester"),
        allowed_write_dirs: vec![root.join("tests/")],
            working_dir: None,
    };

    let groups = vec![WorkerGroupConfig {
        group_id: "worker".into(),
        session_name: "testproject-worker".into(),
        layout: SplitDirection::Horizontal,
        members: vec![coder_cfg.clone(), tester_cfg.clone()],
    }];

    let log_dir = tmp.path().join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));
    let registry = Registry::new_with_injector(
        vec![coder_cfg, tester_cfg],
        state_path.clone(),
        log_dir,
        logger,
        injector.clone(),
    );

    let mut prompts = HashMap::new();
    prompts.insert("coder".to_string(), "hello coder".to_string());
    prompts.insert("tester".to_string(), "hello tester".to_string());

    let handle = tokio::spawn(async move { registry.spawn_all(&prompts, &groups).await });
    tokio::time::advance(Duration::from_secs(6)).await;
    handle.await.unwrap();

    // spawn_group_session records each command as a (session, cmd) pair
    let spawned = injector.spawned.lock().unwrap();
    assert_eq!(spawned.len(), 2);
    assert_eq!(spawned[0].0, "testproject-worker");
    assert_eq!(spawned[0].1, "claude");
    assert_eq!(spawned[1].0, "testproject-worker");
    assert_eq!(spawned[1].1, "codex");

    // Prompts should be injected to the pane targets
    let injected = injector.injected.lock().unwrap();
    assert_eq!(injected.len(), 2);
    let targets: Vec<&str> = injected.iter().map(|(t, _)| t.as_str()).collect();
    assert!(targets.contains(&"testproject-worker:0.0"));
    assert!(targets.contains(&"testproject-worker:0.1"));
}

#[tokio::test]
async fn spawn_all_with_groups_records_state_for_all_members() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");

    let coder_cfg = AgentConfig {
        agent_id: "coder".into(),
        cli_command: "claude".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.0".into(),
        inbox_dir: messages.join("to_coder"),
        allowed_write_dirs: vec![],
            working_dir: None,
    };
    let tester_cfg = AgentConfig {
        agent_id: "tester".into(),
        cli_command: "codex".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.1".into(),
        inbox_dir: messages.join("to_tester"),
        allowed_write_dirs: vec![],
            working_dir: None,
    };

    let groups = vec![WorkerGroupConfig {
        group_id: "worker".into(),
        session_name: "testproject-worker".into(),
        layout: SplitDirection::Horizontal,
        members: vec![coder_cfg.clone(), tester_cfg.clone()],
    }];

    let log_dir = tmp.path().join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));
    let registry = Registry::new_with_injector(
        vec![coder_cfg, tester_cfg],
        state_path.clone(),
        log_dir,
        logger,
        injector,
    );

    registry.spawn_all(&HashMap::new(), &groups).await;

    let state_contents = std::fs::read_to_string(&state_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&state_contents).unwrap();
    let map = json.as_object().unwrap();
    assert!(map.contains_key("coder"));
    assert!(map.contains_key("tester"));

    // Both agents share the same tmux_session but have distinct tmux_target
    let coder_state = map.get("coder").unwrap();
    assert_eq!(coder_state["tmux_session"], "testproject-worker");
    assert_eq!(coder_state["tmux_target"], "testproject-worker:0.0");
    let tester_state = map.get("tester").unwrap();
    assert_eq!(tester_state["tmux_session"], "testproject-worker");
    assert_eq!(tester_state["tmux_target"], "testproject-worker:0.1");
}

#[tokio::test]
async fn kill_all_deduplicates_shared_sessions() {
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");

    let coder_cfg = AgentConfig {
        agent_id: "coder".into(),
        cli_command: "claude".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.0".into(),
        inbox_dir: messages.join("to_coder"),
        allowed_write_dirs: vec![],
            working_dir: None,
    };
    let tester_cfg = AgentConfig {
        agent_id: "tester".into(),
        cli_command: "codex".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.1".into(),
        inbox_dir: messages.join("to_tester"),
        allowed_write_dirs: vec![],
            working_dir: None,
    };

    let groups = vec![WorkerGroupConfig {
        group_id: "worker".into(),
        session_name: "testproject-worker".into(),
        layout: SplitDirection::Horizontal,
        members: vec![coder_cfg.clone(), tester_cfg.clone()],
    }];

    let log_dir = tmp.path().join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));
    let registry = Registry::new_with_injector(
        vec![coder_cfg, tester_cfg],
        state_path,
        log_dir,
        logger,
        injector.clone(),
    );

    registry.spawn_all(&HashMap::new(), &groups).await;
    registry.kill_all().await;

    let killed = injector.killed.lock().unwrap();
    // Should only kill the shared session once, not twice
    assert_eq!(killed.len(), 1);
    assert_eq!(killed[0], "testproject-worker");
}

#[tokio::test]
async fn health_loop_respawns_dead_group_pane_and_reinjects_prompt() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_has_session_queue(vec![false]);
    injector.set_default_has_session(true);
    injector.set_pane_alive_queue("testproject-worker:0.0", vec![false, true]);
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");
    let worktree_dir = root.join("worktrees/worker-1");

    let coder_cfg = AgentConfig {
        agent_id: "coder".into(),
        cli_command: "claude".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.0".into(),
        inbox_dir: messages.join("to_coder"),
        allowed_write_dirs: vec![root.join("src/")],
        working_dir: Some(worktree_dir.clone()),
    };
    let tester_cfg = AgentConfig {
        agent_id: "tester".into(),
        cli_command: "codex".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.1".into(),
        inbox_dir: messages.join("to_tester"),
        allowed_write_dirs: vec![root.join("tests/")],
            working_dir: None,
    };

    let groups = vec![WorkerGroupConfig {
        group_id: "worker".into(),
        session_name: "testproject-worker".into(),
        layout: SplitDirection::Horizontal,
        members: vec![coder_cfg.clone(), tester_cfg],
    }];

    let log_dir = tmp.path().join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));
    let registry = Registry::new_with_injector(
        vec![coder_cfg, groups[0].members[1].clone()],
        state_path.clone(),
        log_dir,
        logger,
        injector.clone(),
    );

    let mut prompts = HashMap::new();
    prompts.insert("coder".to_string(), "restore coder".to_string());

    let spawn = tokio::spawn({
        let registry = registry.clone();
        let groups = groups.clone();
        let prompts = prompts.clone();
        async move { registry.spawn_all(&prompts, &groups).await }
    });
    tokio::time::advance(Duration::from_secs(6)).await;
    spawn.await.unwrap();

    let health = tokio::spawn({
        let registry = registry.clone();
        async move { registry.health_loop().await }
    });

    tokio::task::yield_now().await;
    let mut ready = false;
    for _ in 0..12 {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;

        let respawned_len = injector.respawned.lock().unwrap().len();
        let prompt_reinjected = injector
            .injected
            .lock()
            .unwrap()
            .iter()
            .any(|(target, body)| target == "testproject-worker:0.0" && body == "restore coder");
        if respawned_len == 1 && prompt_reinjected {
            ready = true;
            break;
        }
    }
    assert!(ready, "expected pane respawn and prompt reinjection");

    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;

    let respawned = injector.respawned.lock().unwrap();
    assert_eq!(respawned.len(), 1);
    assert_eq!(respawned[0].0, "testproject-worker:0.0");
    assert_eq!(
        respawned[0].1,
        format!("cd {} && claude", worktree_dir.display())
    );
    drop(respawned);

    let injected = injector.injected.lock().unwrap();
    assert!(
        injected
            .iter()
            .any(|(target, body)| target == "testproject-worker:0.0" && body == "restore coder")
    );
    assert!(
        !injected
            .iter()
            .any(|(target, body)| target == "testproject-worker:0.1" && body == "restore coder")
    );
    drop(injected);

    let agents = registry.agents.lock().await;
    let coder_state = agents.get("coder").unwrap();
    assert!(
        coder_state.status.to_string().eq_ignore_ascii_case("healthy"),
        "unexpected status: {}",
        coder_state.status
    );
    assert_eq!(coder_state.restart_count, 1);
    drop(agents);

    health.abort();
}

#[tokio::test]
async fn health_loop_restarts_dead_group_session_once_for_shared_session() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    injector.set_has_session_queue(vec![false, false, false]);
    injector.set_default_has_session(true);
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");

    let coder_cfg = AgentConfig {
        agent_id: "coder".into(),
        cli_command: "claude".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.0".into(),
        inbox_dir: messages.join("to_coder"),
        allowed_write_dirs: vec![root.join("src/")],
            working_dir: None,
    };
    let tester_cfg = AgentConfig {
        agent_id: "tester".into(),
        cli_command: "codex".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.1".into(),
        inbox_dir: messages.join("to_tester"),
        allowed_write_dirs: vec![root.join("tests/")],
            working_dir: None,
    };

    let groups = vec![WorkerGroupConfig {
        group_id: "worker".into(),
        session_name: "testproject-worker".into(),
        layout: SplitDirection::Horizontal,
        members: vec![coder_cfg.clone(), tester_cfg.clone()],
    }];

    let log_dir = tmp.path().join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));
    let registry = Registry::new_with_injector(
        vec![coder_cfg.clone(), tester_cfg.clone()],
        state_path,
        log_dir,
        logger,
        injector.clone(),
    );

    let mut prompts = HashMap::new();
    prompts.insert("coder".to_string(), "restore coder".to_string());
    prompts.insert("tester".to_string(), "restore tester".to_string());

    let spawn = tokio::spawn({
        let registry = registry.clone();
        let groups = groups.clone();
        let prompts = prompts.clone();
        async move { registry.spawn_all(&prompts, &groups).await }
    });
    tokio::time::advance(Duration::from_secs(6)).await;
    spawn.await.unwrap();

    let health = tokio::spawn({
        let registry = registry.clone();
        async move { registry.health_loop().await }
    });

    let mut ready = false;
    for _ in 0..16 {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;

        let spawned_len = injector.spawned.lock().unwrap().len();
        let injected = injector.injected.lock().unwrap().clone();
        let coder_reinjected = injected
            .iter()
            .any(|(target, body)| target == "testproject-worker:0.0" && body == "restore coder");
        let tester_reinjected = injected
            .iter()
            .any(|(target, body)| target == "testproject-worker:0.1" && body == "restore tester");
        if spawned_len == 4 && coder_reinjected && tester_reinjected {
            ready = true;
            break;
        }
    }
    assert!(ready, "expected a single group respawn with prompt reinjection");

    let spawned = injector.spawned.lock().unwrap();
    assert_eq!(spawned.len(), 4);
    let worker_spawns = spawned
        .iter()
        .filter(|(session, _)| session == "testproject-worker")
        .count();
    assert_eq!(worker_spawns, 4, "group should spawn exactly twice total");
    drop(spawned);

    let agents = registry.agents.lock().await;
    assert_eq!(agents.get("coder").unwrap().restart_count, 1);
    assert_eq!(agents.get("tester").unwrap().restart_count, 1);
    drop(agents);

    health.abort();
}

#[tokio::test]
async fn restart_agent_uses_grouped_tmux_target_and_reinjects_prompt() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");
    let worktree_dir = root.join("worktrees/worker-1");

    let coder_cfg = AgentConfig {
        agent_id: "coder".into(),
        cli_command: "claude".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.0".into(),
        inbox_dir: messages.join("to_coder"),
        allowed_write_dirs: vec![root.join("src/")],
        working_dir: Some(worktree_dir.clone()),
    };
    let tester_cfg = AgentConfig {
        agent_id: "tester".into(),
        cli_command: "codex".into(),
        tmux_session: "testproject-worker".into(),
        tmux_target: "testproject-worker:0.1".into(),
        inbox_dir: messages.join("to_tester"),
        allowed_write_dirs: vec![root.join("tests/")],
            working_dir: None,
    };

    let groups = vec![WorkerGroupConfig {
        group_id: "worker".into(),
        session_name: "testproject-worker".into(),
        layout: SplitDirection::Horizontal,
        members: vec![coder_cfg.clone(), tester_cfg.clone()],
    }];

    let log_dir = tmp.path().join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));
    let registry = Registry::new_with_injector(
        vec![coder_cfg, tester_cfg],
        state_path,
        log_dir,
        logger,
        injector.clone(),
    );

    let mut prompts = HashMap::new();
    prompts.insert("coder".to_string(), "fresh coder context".to_string());

    let spawn = tokio::spawn({
        let registry = registry.clone();
        let groups = groups.clone();
        let prompts = prompts.clone();
        async move { registry.spawn_all(&prompts, &groups).await }
    });
    tokio::time::advance(Duration::from_secs(6)).await;
    spawn.await.unwrap();

    let restart = tokio::spawn({
        let registry = registry.clone();
        async move { registry.restart_agent("coder").await }
    });
    tokio::time::advance(Duration::from_secs(6)).await;
    restart.await.unwrap().unwrap();

    let respawned = injector.respawned.lock().unwrap();
    assert_eq!(respawned.len(), 1);
    assert_eq!(respawned[0].0, "testproject-worker:0.0");
    assert_eq!(
        respawned[0].1,
        format!("cd {} && claude", worktree_dir.display())
    );
    drop(respawned);

    let injected = injector.injected.lock().unwrap();
    assert!(
        injected
            .iter()
            .any(|(target, body)| target == "testproject-worker:0.0"
                && body == "fresh coder context")
    );
}

#[tokio::test]
async fn timer_loop_expands_grouped_include_agents_and_preserves_exact_matches() {
    tokio::time::pause();
    let tmp = TempDir::new().unwrap();
    let injector = Arc::new(MockInjector::default());
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");

    let coder_1 = AgentConfig {
        agent_id: "coder-1".into(),
        cli_command: "claude".into(),
        tmux_session: "testproject-coder".into(),
        tmux_target: "testproject-coder:0.0".into(),
        inbox_dir: messages.join("to_coder-1"),
        allowed_write_dirs: vec![],
            working_dir: None,
    };
    let coder_2 = AgentConfig {
        agent_id: "coder-2".into(),
        cli_command: "claude".into(),
        tmux_session: "testproject-coder".into(),
        tmux_target: "testproject-coder:0.1".into(),
        inbox_dir: messages.join("to_coder-2"),
        allowed_write_dirs: vec![],
            working_dir: None,
    };
    let reviewer = AgentConfig {
        agent_id: "reviewer".into(),
        cli_command: "codex".into(),
        tmux_session: "testproject-reviewer".into(),
        tmux_target: "testproject-reviewer".into(),
        inbox_dir: messages.join("to_reviewer"),
        allowed_write_dirs: vec![],
            working_dir: None,
    };

    let groups = vec![WorkerGroupConfig {
        group_id: "coder".into(),
        session_name: "testproject-coder".into(),
        layout: SplitDirection::Horizontal,
        members: vec![coder_1.clone(), coder_2.clone()],
    }];

    let log_dir = tmp.path().join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));
    let registry = Registry::new_with_injector(
        vec![coder_1, coder_2, reviewer],
        state_path,
        log_dir,
        logger.clone(),
        injector.clone(),
    );

    let spawn = tokio::spawn({
        let registry = registry.clone();
        let groups = groups.clone();
        async move { registry.spawn_all(&HashMap::new(), &groups).await }
    });
    tokio::time::advance(Duration::from_secs(6)).await;
    spawn.await.unwrap();

    {
        let mut agents = registry.agents.lock().await;
        let coder_1 = agents.get_mut("coder-1").unwrap();
        coder_1.activity = AgentActivity::Busy;
        let coder_2 = agents.get_mut("coder-2").unwrap();
        coder_2.activity = AgentActivity::Idle;
        let reviewer = agents.get_mut("reviewer").unwrap();
        reviewer.activity = AgentActivity::Unknown;
    }

    let prompt_path = tmp.path().join("timer_prompt.md");
    std::fs::write(&prompt_path, "status sweep").unwrap();

    let timers = vec![ResolvedTimer::new_basic(
        "reviewer".into(),
        0,
        prompt_path,
        root.display().to_string(),
        messages.display().to_string(),
        false,
        vec!["coder".into(), "reviewer".into(), "ghost".into()],
    )];

    let timer_task = tokio::spawn({
        let registry = registry.clone();
        let logger = logger.clone();
        async move { registry.timer_loop(timers, logger).await }
    });
    tokio::task::yield_now().await;

    let mut injected_body = None;
    for _ in 0..8 {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        if let Some((_, body)) = injector.injected.lock().unwrap().last().cloned() {
            if body.contains("--- AGENT STATUS ---") {
                injected_body = Some(body);
                break;
            }
        }
    }

    let injected = injector.injected.lock().unwrap();
    assert!(
        injected
            .iter()
            .any(|(target, _)| target == "testproject-reviewer"),
        "timer should inject into the exact reviewer session"
    );
    drop(injected);

    let body = injected_body.expect("expected timer injection with status footer");
    assert!(body.contains("- coder-1: BUSY | healthy"));
    assert!(body.contains("- coder-2: IDLE | healthy"));
    assert!(body.contains("- reviewer: UNKNOWN | healthy"));
    assert!(body.contains("- ghost: unknown"));

    timer_task.abort();
}

// ---------------------------------------------------------------------------
// Attention detection
// ---------------------------------------------------------------------------

#[test]
fn attention_patterns_claude_includes_permission_prompts() {
    let patterns = attention_patterns("claude --model opus");
    assert!(patterns.iter().any(|p| p.contains("Allow once")));
    assert!(patterns.iter().any(|p| p.contains("Allow always")));
}

#[test]
fn attention_patterns_codex_includes_confirmation_prompts() {
    let patterns = attention_patterns("codex --model gpt-5");
    assert!(patterns.iter().any(|p| p.contains("(y/a/x/e/n)")));
}

#[test]
fn attention_patterns_copilot_includes_navigation_hint() {
    let patterns = attention_patterns("copilot");
    assert!(patterns.iter().any(|p| p.contains("to navigate")));
    assert!(patterns.iter().any(|p| p.contains("Write to this file?")));
}

#[test]
fn attention_patterns_cursor_includes_skip_prompt() {
    let patterns = attention_patterns("cursor agent");
    assert!(patterns.iter().any(|p| p.contains("Skip and Continue")));
    assert!(patterns.iter().any(|p| p.contains("Write to this file?")));
}

#[test]
fn attention_patterns_gemini_includes_approval_prompt() {
    let patterns = attention_patterns("gemini --model 2.5-pro");
    assert!(patterns.iter().any(|p| p.contains("(y/n/always)")));
}

#[test]
fn attention_patterns_unknown_returns_conservative_fallback() {
    let patterns = attention_patterns("unknown-tool");
    assert!(!patterns.is_empty());
    assert!(patterns.iter().any(|p| p.contains("(y/a/x/e/n)")));
}

#[test]
fn detect_attention_pattern_matches_in_tail_lines() {
    let content = "some output\nmore output\nthinking...\nAllow once\n";
    let result = detect_attention_pattern(content, "claude");
    assert_eq!(result, Some("Allow once"));
}

#[test]
fn detect_attention_pattern_ignores_old_scrollback() {
    // Pattern appears only in early output, not in the tail
    let mut content = String::from("Allow once\n");
    for _ in 0..20 {
        content.push_str("normal output line\n");
    }
    let result = detect_attention_pattern(&content, "claude");
    assert_eq!(result, None);
}

#[test]
fn detect_attention_pattern_returns_none_for_no_match() {
    let content = "working...\nstill working...\nalmost done\n";
    let result = detect_attention_pattern(content, "claude");
    assert_eq!(result, None);
}

#[test]
fn detect_attention_pattern_codex_prompt() {
    let content = "analyzing changes...\n\nApply these changes? (y/a/x/e/n)\n";
    let result = detect_attention_pattern(content, "codex");
    assert_eq!(result, Some("(y/a/x/e/n)"));
}

#[test]
fn detect_attention_pattern_gemini_prompt() {
    let content = "Do you approve this action? (y/n/always)\n";
    let result = detect_attention_pattern(content, "gemini");
    assert_eq!(result, Some("(y/n/always)"));
}

#[test]
fn detect_attention_pattern_cursor_prompt() {
    let content = "Would you like to proceed?\nSkip and Continue\n";
    let result = detect_attention_pattern(content, "cursor agent");
    assert_eq!(result, Some("Skip and Continue"));
}

#[test]
fn detect_attention_pattern_cursor_write_prompt() {
    let content = "Write to this file?\nProceed (y)\nReject & propose changes\nRun Everything\n";
    let result = detect_attention_pattern(content, "cursor agent");
    assert_eq!(result, Some("Write to this file?"));
}

#[test]
fn detect_attention_pattern_copilot_write_prompt() {
    let content = "Write to this file?\nin /tmp/package.json\nProceed (y)\nRun Everything\n";
    let result = detect_attention_pattern(content, "copilot");
    assert_eq!(result, Some("Write to this file?"));
}
