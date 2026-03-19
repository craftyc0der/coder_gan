use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use orchestrator::injector::{InjectionError, InjectorOps, InterruptKeys};
use orchestrator::logger::Logger;
use orchestrator::supervisor::{AgentConfig, Registry};

#[derive(Default)]
struct MockInjector {
    spawned: Mutex<Vec<(String, String)>>,
    killed: Mutex<Vec<String>>,
    injected: Mutex<Vec<(String, String)>>,
    captured: Mutex<Vec<String>>,

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
        self.spawned
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
        },
        AgentConfig {
            agent_id: "tester".into(),
            cli_command: "echo".into(),
            tmux_session: "testproject-tester".into(),
            tmux_target: "testproject-tester".into(),
            inbox_dir: messages.join("to_tester"),
            allowed_write_dirs: vec![root.join("tests/")],
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
