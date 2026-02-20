use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use orchestrator::injector::{InjectionError, InjectorOps};
use orchestrator::logger::Logger;
use orchestrator::supervisor::{AgentConfig, Registry};
use orchestrator::watcher::{parse_message, MessageWatcher};

#[derive(Default)]
struct MockInjector {
    injected: Mutex<Vec<(String, String)>>,
    inject_fail: Mutex<bool>,
}

impl MockInjector {
    fn set_inject_fail(&self, value: bool) {
        *self.inject_fail.lock().unwrap() = value;
    }
}

impl InjectorOps for MockInjector {
    fn has_session(&self, _session: &str) -> bool {
        true
    }

    fn kill_session(&self, _session: &str) {}

    fn spawn_session(&self, _session: &str, _cmd: &str) -> Result<Option<u32>, InjectionError> {
        Ok(None) // No Terminal.app window in tests
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
        let fail = *self.inject_fail.lock().unwrap();
        let result = if fail {
            Err(InjectionError::TmuxCommand {
                step: "inject".into(),
                detail: "mock inject fail".into(),
            })
        } else {
            Ok(())
        };
        Box::pin(async move { result })
    }

    fn capture(&self, _session: &str) -> Result<String, InjectionError> {
        Ok("".into())
    }
}

fn meta_fields(path: &Path) -> (String, String, String, String, String) {
    let meta = parse_message(path).expect("expected message meta");
    (
        meta.filename,
        meta.sender,
        meta.recipient,
        meta.topic,
        meta.path.to_string_lossy().to_string(),
    )
}

async fn make_registry(tmp: &TempDir, injector: Arc<dyn InjectorOps>) -> Registry {
    let root = tmp.path().to_path_buf();
    let messages = root.join(".orchestrator/messages");
    let log_dir = root.join(".orchestrator/runtime/logs");
    let state_path = log_dir.join("state.json");
    let logger = Arc::new(Logger::new(&log_dir, "events.jsonl"));

    let configs = vec![AgentConfig {
        agent_id: "coder".into(),
        cli_command: "echo".into(),
        tmux_session: "testproject-coder".into(),
        inbox_dir: messages.join("to_coder"),
        allowed_write_dirs: vec![root.join("src/")],
    }];

    let registry = Registry::new_with_injector(configs, state_path, log_dir, logger, injector);
    registry.spawn_all(&HashMap::new()).await;
    registry
}

async fn make_watcher(
    tmp: &TempDir,
    registry_injector: Arc<dyn InjectorOps>,
    watcher_injector: Arc<dyn InjectorOps>,
) -> (Arc<MessageWatcher>, PathBuf) {
    let messages_dir = tmp.path().join(".orchestrator/messages");
    let logger = Arc::new(Logger::new(&tmp.path().join(".orchestrator/runtime/logs"), "events.jsonl"));
    let registry = make_registry(tmp, registry_injector).await;
    (
        Arc::new(MessageWatcher::new_with_injector(
            registry,
            logger,
            messages_dir.clone(),
            watcher_injector,
        )),
        messages_dir,
    )
}

fn write_inbox(messages_dir: &Path, filename: &str, content: &str) -> PathBuf {
    let inbox = messages_dir.join("to_coder");
    std::fs::create_dir_all(&inbox).unwrap();
    let path = inbox.join(filename);
    std::fs::write(&path, content).unwrap();
    path
}

fn find_named_file(root: &Path, filename: &str) -> Option<PathBuf> {
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find_named_file(&path, filename) {
                    return Some(found);
                }
            } else if path.file_name().and_then(|n| n.to_str()) == Some(filename) {
                return Some(path);
            }
        }
    }
    None
}

#[test]
fn parse_message_canonical_filename() {
    let path = Path::new(
        "/tmp/2026-02-20T12-34-56Z__from-coder__to-tester__topic-tests.md",
    );
    let (filename, sender, recipient, topic, _) = meta_fields(path);
    assert_eq!(filename, "2026-02-20T12-34-56Z__from-coder__to-tester__topic-tests.md");
    assert_eq!(sender, "coder");
    assert_eq!(recipient, "tester");
    assert_eq!(topic, "tests");
}

#[test]
fn parse_message_parts_out_of_order() {
    let path = Path::new(
        "/tmp/2026-02-20T12-34-56Z__topic-build__to-coder__from-tester",
    );
    let (_filename, sender, recipient, topic, _) = meta_fields(path);
    assert_eq!(sender, "tester");
    assert_eq!(recipient, "coder");
    assert_eq!(topic, "build");
}

#[test]
fn parse_message_strips_txt_extension() {
    let path = Path::new(
        "/tmp/2026-02-20T12-34-56Z__from-coder__to-tester__topic-status.txt",
    );
    let (_filename, _sender, _recipient, topic, _) = meta_fields(path);
    assert_eq!(topic, "status");
}

#[test]
fn parse_message_strips_json_extension() {
    let path = Path::new(
        "/tmp/2026-02-20T12-34-56Z__from-coder__to-tester__topic-status.json",
    );
    let (_filename, _sender, _recipient, topic, _) = meta_fields(path);
    assert_eq!(topic, "status");
}

#[test]
fn parse_message_falls_back_to_parent_dir_recipient() {
    let path = Path::new("/tmp/to_coder/2026-02-20T12-34-56Z.msg");
    let (_filename, _sender, recipient, _topic, _) = meta_fields(path);
    assert_eq!(recipient, "coder");
}

#[test]
fn parse_message_returns_none_when_no_match() {
    let path = Path::new("/tmp/inbox/unknown_file.md");
    assert!(parse_message(path).is_none());
}

#[tokio::test]
async fn route_message_known_recipient_injects_and_moves_processed() {
    let tmp = TempDir::new().unwrap();
    let registry_injector = Arc::new(MockInjector::default());
    let watcher_injector = Arc::new(MockInjector::default());
    let (watcher, messages_dir) =
        make_watcher(&tmp, registry_injector, watcher_injector.clone()).await;

    let filename = "2026-02-20T12-34-56Z__from-coder__to-coder__topic-tests.md";
    let path = write_inbox(&messages_dir, filename, "hello world");
    let meta = parse_message(&path).unwrap();

    watcher.route_message(meta).await;

    let processed = find_named_file(&messages_dir, filename).expect("expected processed file");
    assert!(
        processed.to_string_lossy().contains("processed"),
        "expected processed dir, got {}",
        processed.display()
    );

    let injected = watcher_injector.injected.lock().unwrap();
    assert_eq!(injected.len(), 1);
    assert!(injected[0].1.starts_with("--- INCOMING MESSAGE ---\nFROM: coder\nTOPIC: tests\nFILE: "));
    assert!(injected[0].1.contains("hello world"));
}

#[tokio::test]
async fn route_message_unknown_recipient_dead_letters() {
    let tmp = TempDir::new().unwrap();
    let registry_injector = Arc::new(MockInjector::default());
    let watcher_injector = Arc::new(MockInjector::default());
    let (watcher, messages_dir) =
        make_watcher(&tmp, registry_injector, watcher_injector.clone()).await;

    let filename = "2026-02-20T12-34-56Z__from-coder__to-ghost__topic-tests.md";
    let inbox = messages_dir.join("to_ghost");
    std::fs::create_dir_all(&inbox).unwrap();
    let path = inbox.join(filename);
    std::fs::write(&path, "hello").unwrap();

    let meta = parse_message(&path).unwrap();
    watcher.route_message(meta).await;

    let dead = find_named_file(&messages_dir, filename).expect("expected dead-letter file");
    assert!(
        dead.to_string_lossy().contains("dead_letter"),
        "expected dead_letter dir, got {}",
        dead.display()
    );
    assert!(watcher_injector.injected.lock().unwrap().is_empty());

    let log_path = tmp
        .path()
        .join(".orchestrator/runtime/logs/events.jsonl");
    let events: Vec<serde_json::Value> = std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(
        events.iter().any(|v| v["event"] == "message_dead_letter"),
        "expected message_dead_letter event"
    );
}

#[tokio::test]
async fn route_message_inject_failure_dead_letters() {
    let tmp = TempDir::new().unwrap();
    let registry_injector = Arc::new(MockInjector::default());
    let watcher_injector = Arc::new(MockInjector::default());
    watcher_injector.set_inject_fail(true);
    let (watcher, messages_dir) =
        make_watcher(&tmp, registry_injector, watcher_injector.clone()).await;

    let filename = "2026-02-20T12-34-56Z__from-coder__to-coder__topic-tests.md";
    let path = write_inbox(&messages_dir, filename, "hello");
    let meta = parse_message(&path).unwrap();

    watcher.route_message(meta).await;

    let dead = find_named_file(&messages_dir, filename).expect("expected dead-letter file");
    assert!(
        dead.to_string_lossy().contains("dead_letter"),
        "expected dead_letter dir, got {}",
        dead.display()
    );

    let log_path = tmp
        .path()
        .join(".orchestrator/runtime/logs/events.jsonl");
    let events: Vec<serde_json::Value> = std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(
        events.iter().any(|v| v["event"] == "message_failed"),
        "expected message_failed event"
    );
}

#[tokio::test]
async fn route_message_deduplicates_by_content() {
    let tmp = TempDir::new().unwrap();
    let registry_injector = Arc::new(MockInjector::default());
    let watcher_injector = Arc::new(MockInjector::default());
    let (watcher, messages_dir) =
        make_watcher(&tmp, registry_injector, watcher_injector.clone()).await;

    let filename1 = "2026-02-20T12-34-56Z__from-coder__to-coder__topic-tests.md";
    let filename2 = "2026-02-20T12-34-57Z__from-coder__to-coder__topic-tests.md";

    let path1 = write_inbox(&messages_dir, filename1, "same content");
    let meta1 = parse_message(&path1).unwrap();
    watcher.route_message(meta1).await;

    let path2 = write_inbox(&messages_dir, filename2, "same content");
    let meta2 = parse_message(&path2).unwrap();
    watcher.route_message(meta2).await;

    let injected = watcher_injector.injected.lock().unwrap();
    assert_eq!(injected.len(), 1);

    let processed2 = find_named_file(&messages_dir, filename2).expect("expected processed file");
    assert!(processed2.to_string_lossy().contains("processed"));
}

#[tokio::test]
async fn count_inbox_counts_non_hidden_files() {
    let tmp = TempDir::new().unwrap();
    let registry_injector = Arc::new(MockInjector::default());
    let watcher_injector = Arc::new(MockInjector::default());
    let (watcher, messages_dir) =
        make_watcher(&tmp, registry_injector, watcher_injector).await;

    let inbox = messages_dir.join("to_coder");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join("a.md"), "a").unwrap();
    std::fs::write(inbox.join("b.md"), "b").unwrap();
    std::fs::write(inbox.join(".hidden"), "c").unwrap();

    let count = watcher.count_inbox("coder").await;
    assert_eq!(count, 2);
}

#[tokio::test]
async fn route_message_frames_header_format() {
    let tmp = TempDir::new().unwrap();
    let registry_injector = Arc::new(MockInjector::default());
    let watcher_injector = Arc::new(MockInjector::default());
    let (watcher, messages_dir) =
        make_watcher(&tmp, registry_injector, watcher_injector.clone()).await;

    let filename = "2026-02-20T12-34-56Z__from-coder__to-coder__topic-tests.md";
    let path = write_inbox(&messages_dir, filename, "body");
    let meta = parse_message(&path).unwrap();

    watcher.route_message(meta).await;

    let injected = watcher_injector.injected.lock().unwrap();
    assert_eq!(injected.len(), 1);
    assert!(
        injected[0]
            .1
            .starts_with("--- INCOMING MESSAGE ---\nFROM: coder\nTOPIC: tests\nFILE: ")
    );
}
