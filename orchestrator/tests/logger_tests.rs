use std::fs;
use std::path::Path;

use serde_json::Value;
use tempfile::TempDir;

use orchestrator::logger::{Event, Logger};

fn read_lines(path: &Path) -> Vec<String> {
    let contents = fs::read_to_string(path).unwrap_or_default();
    contents.lines().map(|l| l.to_string()).collect()
}

fn parse_line(line: &str) -> Value {
    serde_json::from_str::<Value>(line).expect("line should be valid JSON")
}

#[test]
fn new_creates_file_when_dir_exists() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    let logger = Logger::new(&log_dir, "events.jsonl");
    let path = logger.path();
    assert_eq!(path, log_dir.join("events.jsonl"));
    assert!(path.exists(), "log file should exist after new");
}

#[test]
fn new_creates_dir_when_missing() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");

    let logger = Logger::new(&log_dir, "events.jsonl");
    let path = logger.path();
    assert!(log_dir.is_dir(), "log dir should be created");
    assert!(path.exists(), "log file should exist after new");
}

#[test]
fn path_returns_absolute_path() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");
    assert_eq!(logger.path(), log_dir.join("events.jsonl"));
}

#[test]
fn log_agent_spawn_writes_event_and_agent_id() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");

    logger.log(Event::AgentSpawn {
        agent_id: "coder".into(),
    });

    let lines = read_lines(logger.path());
    assert_eq!(lines.len(), 1);
    let v = parse_line(&lines[0]);
    assert_eq!(v["event"], "agent_spawn");
    assert_eq!(v["agent_id"], "coder");
    assert!(v["timestamp"].as_str().unwrap_or_default().len() > 0);
}

#[test]
fn log_agent_exit_writes_reason() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");

    logger.log(Event::AgentExit {
        agent_id: "coder".into(),
        reason: "panic".into(),
    });

    let lines = read_lines(logger.path());
    let v = parse_line(&lines[0]);
    assert_eq!(v["event"], "agent_exit");
    assert_eq!(v["agent_id"], "coder");
    assert_eq!(v["reason"], "panic");
}

#[test]
fn log_agent_restart_writes_event_and_attempt() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");

    logger.log(Event::AgentRestart {
        agent_id: "coder".into(),
        attempt: 2,
    });

    let lines = read_lines(logger.path());
    let v = parse_line(&lines[0]);
    assert_eq!(v["event"], "agent_restart");
    assert_eq!(v["agent_id"], "coder");
    assert_eq!(v["attempt"], 2);
}

#[test]
fn log_orchestrator_start_has_only_timestamp_and_event() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");

    logger.log(Event::OrchestratorStart);

    let lines = read_lines(logger.path());
    let v = parse_line(&lines[0]);
    assert_eq!(v["event"], "orchestrator_start");
    assert!(v["timestamp"].as_str().unwrap_or_default().len() > 0);
    let obj = v.as_object().unwrap();
    assert_eq!(obj.len(), 2);
}

#[test]
fn log_orchestrator_stop_writes_event() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");

    logger.log(Event::OrchestratorStop);

    let lines = read_lines(logger.path());
    let v = parse_line(&lines[0]);
    assert_eq!(v["event"], "orchestrator_stop");
}

#[test]
fn log_scope_violation_writes_path_and_detail() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");

    logger.log(Event::ScopeViolation {
        path: "/tmp/file.txt".into(),
        detail: "out of scope".into(),
    });

    let lines = read_lines(logger.path());
    let v = parse_line(&lines[0]);
    assert_eq!(v["event"], "scope_violation");
    assert_eq!(v["path"], "/tmp/file.txt");
    assert_eq!(v["detail"], "out of scope");
}

#[test]
fn log_transcript_captured_writes_char_count() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");

    logger.log(Event::TranscriptCaptured {
        agent_id: "tester".into(),
        chars: 42,
    });

    let lines = read_lines(logger.path());
    let v = parse_line(&lines[0]);
    assert_eq!(v["event"], "transcript_captured");
    assert_eq!(v["agent_id"], "tester");
    assert_eq!(v["chars"], 42);
}

#[test]
fn multiple_logs_append_in_order() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");

    logger.log(Event::OrchestratorStart);
    logger.log(Event::AgentSpawn {
        agent_id: "coder".into(),
    });
    logger.log(Event::OrchestratorStop);

    let lines = read_lines(logger.path());
    assert_eq!(lines.len(), 3);
    let events: Vec<String> = lines
        .iter()
        .map(|l| parse_line(l)["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(events, vec!["orchestrator_start", "agent_spawn", "orchestrator_stop"]);
}

#[test]
fn each_line_is_valid_json_and_has_timestamp() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger = Logger::new(&log_dir, "events.jsonl");

    logger.log(Event::AgentSpawn {
        agent_id: "coder".into(),
    });
    logger.log(Event::AgentRestart {
        agent_id: "coder".into(),
        attempt: 2,
    });

    let lines = read_lines(logger.path());
    assert_eq!(lines.len(), 2);
    for line in lines {
        let v = parse_line(&line);
        let ts = v["timestamp"].as_str().unwrap_or_default();
        assert!(!ts.is_empty());
    }
}

#[test]
fn two_loggers_append_to_same_file() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let logger_a = Logger::new(&log_dir, "events.jsonl");
    let logger_b = Logger::new(&log_dir, "events.jsonl");

    logger_a.log(Event::OrchestratorStart);
    logger_b.log(Event::OrchestratorStop);

    let lines = read_lines(logger_a.path());
    assert_eq!(lines.len(), 2);
}

#[test]
fn new_with_uncreatable_dir_does_not_panic_and_log_is_noop() {
    let tmp = TempDir::new().unwrap();
    let file_as_dir = tmp.path().join("not_a_dir");
    fs::write(&file_as_dir, "file").unwrap();

    let bad_dir = file_as_dir.join("logs");
    let logger = Logger::new(&bad_dir, "events.jsonl");

    logger.log(Event::OrchestratorStart);
    assert!(!logger.path().exists());
}
