use std::path::{Path, PathBuf};

use tempfile::TempDir;

use orchestrator::config::{AgentEntry, ConfigError, ProjectConfig};
use orchestrator::config::init_project;

fn write_agents_toml(dot_dir: &Path, contents: &str) {
    let agents_path = dot_dir.join("agents.toml");
    std::fs::write(agents_path, contents).unwrap();
}

fn minimal_agents_toml(agent_id: &str) -> String {
    format!(
        r#"[[agents]]
            id = "{}"
            command = "claude"
            prompt_file = "prompts/coder.md"
            allowed_write_dirs = ["src/"]
        "#,
        agent_id
    )
}

fn make_dot_dir(project_root: &Path) -> PathBuf {
    let dot_dir = project_root.join(".orchestrator");
    std::fs::create_dir_all(dot_dir.join("prompts")).unwrap();
    std::fs::create_dir_all(dot_dir.join("messages")).unwrap();
    std::fs::create_dir_all(dot_dir.join("runtime/logs/spike_transcripts")).unwrap();
    std::fs::create_dir_all(dot_dir.join("runtime/pids")).unwrap();
    dot_dir
}

fn write_prompt(dot_dir: &Path, name: &str, content: &str) {
    let path = dot_dir.join("prompts").join(name);
    std::fs::write(path, content).unwrap();
}

fn make_config(tmp: &TempDir, agents: Vec<AgentEntry>) -> ProjectConfig {
    let root = tmp.path().to_path_buf();
    let dot = root.join(".orchestrator");
    ProjectConfig {
        project_root: root.clone(),
        project_name: "testproject".into(),
        dot_dir: dot.clone(),
        messages_dir: dot.join("messages"),
        log_dir: dot.join("runtime/logs"),
        state_path: dot.join("runtime/logs/state.json"),
        transcript_dir: dot.join("runtime/logs/spike_transcripts"),
        agents,
    }
}

fn make_agents() -> Vec<AgentEntry> {
    vec![
        AgentEntry {
            id: "coder".into(),
            command: "claude".into(),
            prompt_file: "prompts/coder.md".into(),
            allowed_write_dirs: vec!["src/".into()],
            agent_type: Default::default(),
            slack: None,
            timers: vec![],
        },
        AgentEntry {
            id: "tester".into(),
            command: "codex".into(),
            prompt_file: "prompts/tester.md".into(),
            allowed_write_dirs: vec!["tests/".into()],
            agent_type: Default::default(),
            slack: None,
            timers: vec![],
        },
    ]
}

#[test]
fn load_succeeds_with_valid_agents_toml() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");
    write_agents_toml(&dot_dir, &minimal_agents_toml("coder"));

    let config = ProjectConfig::load(tmp.path()).unwrap();
    let expected_root = tmp.path().canonicalize().unwrap();
    assert_eq!(config.project_root, expected_root);
    assert_eq!(config.dot_dir, expected_root.join(".orchestrator"));
    assert_eq!(config.messages_dir, config.dot_dir.join("messages"));
    assert_eq!(config.log_dir, config.dot_dir.join("runtime/logs"));
    assert_eq!(config.state_path, config.log_dir.join("state.json"));
    assert_eq!(config.transcript_dir, config.log_dir.join("spike_transcripts"));
    assert_eq!(config.agents.len(), 1);
    assert_eq!(config.agents[0].id, "coder");
}

#[test]
fn load_returns_not_initialized_when_dot_dir_missing() {
    let tmp = TempDir::new().unwrap();

    match ProjectConfig::load(tmp.path()) {
        Err(err) => match err {
        ConfigError::NotInitialized(path) => {
            let root = tmp.path().canonicalize().unwrap();
            let dot = root.join(".orchestrator");
            assert!(path == root || path == dot);
        }
        other => panic!("expected NotInitialized, got {other:?}"),
        },
        Ok(_) => panic!("expected NotInitialized, got Ok"),
    }
}

#[test]
fn load_returns_toml_parse_on_invalid_toml() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_agents_toml(&dot_dir, "this is not valid toml =");

    match ProjectConfig::load(tmp.path()) {
        Err(err) => match err {
        ConfigError::TomlParse(_) => {}
        other => panic!("expected TomlParse, got {other:?}"),
        },
        Ok(_) => panic!("expected TomlParse, got Ok"),
    }
}

#[test]
fn load_returns_no_agents_when_empty_array() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_agents_toml(&dot_dir, "agents = []");

    match ProjectConfig::load(tmp.path()) {
        Err(err) => match err {
        ConfigError::NoAgents => {}
        other => panic!("expected NoAgents, got {other:?}"),
        },
        Ok(_) => panic!("expected NoAgents, got Ok"),
    }
}

#[test]
fn load_accepts_agent_id_with_underscore() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");
    write_agents_toml(&dot_dir, &minimal_agents_toml("my_agent"));

    let config = ProjectConfig::load(tmp.path()).unwrap();
    assert_eq!(config.agents[0].id, "my_agent");
}

#[test]
fn load_rejects_agent_id_with_space() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_agents_toml(&dot_dir, &minimal_agents_toml("my agent"));

    match ProjectConfig::load(tmp.path()) {
        Err(err) => match err {
        ConfigError::InvalidAgentId(id) => assert_eq!(id, "my agent"),
        other => panic!("expected InvalidAgentId, got {other:?}"),
        },
        Ok(_) => panic!("expected InvalidAgentId, got Ok"),
    }
}

#[test]
fn load_accepts_agent_id_with_hyphen() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");
    write_agents_toml(&dot_dir, &minimal_agents_toml("my-agent"));

    let config = ProjectConfig::load(tmp.path()).unwrap();
    assert_eq!(config.agents[0].id, "my-agent");
}

#[test]
fn project_name_sanitizes_underscore_to_hyphen() {
    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("my_project");
    std::fs::create_dir_all(&project_root).unwrap();
    let dot_dir = make_dot_dir(&project_root);
    write_prompt(&dot_dir, "coder.md", "hello");
    write_agents_toml(&dot_dir, &minimal_agents_toml("coder"));

    let config = ProjectConfig::load(&project_root).unwrap();
    assert_eq!(config.project_name, "my-project");
}

#[test]
fn project_name_falls_back_to_project_when_only_hyphens() {
    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("---");
    std::fs::create_dir_all(&project_root).unwrap();
    let dot_dir = make_dot_dir(&project_root);
    write_prompt(&dot_dir, "coder.md", "hello");
    write_agents_toml(&dot_dir, &minimal_agents_toml("coder"));

    let config = ProjectConfig::load(&project_root).unwrap();
    assert_eq!(config.project_name, "project");
}

#[test]
fn project_name_collapses_consecutive_hyphens() {
    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("my--project");
    std::fs::create_dir_all(&project_root).unwrap();
    let dot_dir = make_dot_dir(&project_root);
    write_prompt(&dot_dir, "coder.md", "hello");
    write_agents_toml(&dot_dir, &minimal_agents_toml("coder"));

    let config = ProjectConfig::load(&project_root).unwrap();
    assert_eq!(config.project_name, "my-project");
}

#[test]
fn project_name_trims_leading_and_trailing_hyphens() {
    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("-myproject-");
    std::fs::create_dir_all(&project_root).unwrap();
    let dot_dir = make_dot_dir(&project_root);
    write_prompt(&dot_dir, "coder.md", "hello");
    write_agents_toml(&dot_dir, &minimal_agents_toml("coder"));

    let config = ProjectConfig::load(&project_root).unwrap();
    assert_eq!(config.project_name, "myproject");
}

#[test]
fn init_project_creates_expected_dirs_and_files() {
    let tmp = TempDir::new().unwrap();
    init_project(tmp.path()).unwrap();

    let dot = tmp.path().join(".orchestrator");
    let expected_dirs = vec![
        dot.join("prompts"),
        dot.join("messages/processed"),
        dot.join("messages/dead_letter"),
        dot.join("messages/to_coder"),
        dot.join("messages/to_tester"),
        dot.join("messages/to_reviewer"),
        dot.join("runtime/logs/spike_transcripts"),
        dot.join("runtime/pids"),
    ];
    for dir in expected_dirs {
        assert!(dir.is_dir(), "expected dir: {}", dir.display());
    }

    let expected_files = vec![
        dot.join("agents.toml"),
        dot.join("prompts/coder.md"),
        dot.join("prompts/tester.md"),
        dot.join("prompts/reviewer.md"),
    ];
    for file in expected_files {
        assert!(file.is_file(), "expected file: {}", file.display());
    }
}

#[test]
fn init_project_is_idempotent_and_does_not_overwrite() {
    let tmp = TempDir::new().unwrap();
    init_project(tmp.path()).unwrap();

    let dot = tmp.path().join(".orchestrator");
    let agents_path = dot.join("agents.toml");
    std::fs::write(&agents_path, "custom = true").unwrap();

    init_project(tmp.path()).unwrap();

    let contents = std::fs::read_to_string(&agents_path).unwrap();
    assert_eq!(contents, "custom = true");
}

#[test]
fn ensure_dirs_creates_required_subdirs_and_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());

    config.ensure_dirs().unwrap();
    config.ensure_dirs().unwrap();

    let expected_dirs = vec![
        config.messages_dir.join("processed"),
        config.messages_dir.join("dead_letter"),
        config.messages_dir.join("to_coder"),
        config.messages_dir.join("to_tester"),
        config.log_dir.clone(),
        config.transcript_dir.clone(),
        config.dot_dir.join("runtime/pids"),
    ];
    for dir in expected_dirs {
        assert!(dir.is_dir(), "expected dir: {}", dir.display());
    }
}

#[test]
fn startup_prompts_substitutes_template_variables() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(
        &dot_dir,
        "coder.md",
        "root={{project_root}} messages={{messages_dir}} id={{agent_id}}",
    );
    write_agents_toml(&dot_dir, &minimal_agents_toml("coder"));

    let config = ProjectConfig::load(tmp.path()).unwrap();
    let prompts = config.startup_prompts().unwrap();
    let rendered = prompts.get("coder").unwrap();

    assert!(rendered.contains(config.project_root.to_str().unwrap()));
    assert!(rendered.contains(config.messages_dir.to_str().unwrap()));
    assert!(rendered.contains("id=coder"));
}

#[test]
fn startup_prompts_returns_missing_prompt_file() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_agents_toml(&dot_dir, &minimal_agents_toml("coder"));

    let config = ProjectConfig::load(tmp.path()).unwrap();
    let err = config.startup_prompts().unwrap_err();
    match err {
        ConfigError::MissingPromptFile(path) => {
            assert!(path.ends_with("prompts/coder.md"));
        }
        other => panic!("expected MissingPromptFile, got {other:?}"),
    }
}

#[test]
fn tmux_session_for_uses_project_name_and_agent_id() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());

    let session = config.tmux_session_for("coder");
    assert_eq!(session, "testproject-coder");
}

#[test]
fn agent_configs_resolve_inbox_and_allowed_write_dirs() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());

    let agents = config.agent_configs();
    assert_eq!(agents.len(), 2);

    let coder = agents.iter().find(|a| a.agent_id == "coder").unwrap();
    assert_eq!(coder.tmux_session, "testproject-coder");
    assert_eq!(coder.inbox_dir, config.messages_dir.join("to_coder"));
    assert_eq!(coder.allowed_write_dirs[0], tmp.path().join("src/"));
}

#[test]
fn load_parses_timer_entries() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "startup prompt");
    write_prompt(&dot_dir, "status_check.md", "check status");

    let toml = r#"
        [[agents]]
        id = "coder"
        command = "claude"
        prompt_file = "prompts/coder.md"
        allowed_write_dirs = ["src/"]

        [[agents.timers]]
        minutes = 5
        prompt_file = "prompts/status_check.md"
        interrupt = false
        include_agents = ["coder"]

        [[agents.timers]]
        minutes = 30
        prompt_file = "prompts/coder.md"
        interrupt = true
    "#;
    write_agents_toml(&dot_dir, toml);

    let config = ProjectConfig::load(tmp.path()).unwrap();
    assert_eq!(config.agents[0].timers.len(), 2);
    assert_eq!(config.agents[0].timers[0].minutes, 5);
    assert!(!config.agents[0].timers[0].interrupt);
    assert_eq!(config.agents[0].timers[0].include_agents, vec!["coder"]);
    assert_eq!(config.agents[0].timers[1].minutes, 30);
    assert!(config.agents[0].timers[1].interrupt);
    assert!(config.agents[0].timers[1].include_agents.is_empty());
}

#[test]
fn load_defaults_timers_to_empty() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");
    write_agents_toml(&dot_dir, &minimal_agents_toml("coder"));

    let config = ProjectConfig::load(tmp.path()).unwrap();
    assert!(config.agents[0].timers.is_empty());
}

#[test]
fn load_rejects_timer_with_missing_prompt_file() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");

    let toml = r#"
        [[agents]]
        id = "coder"
        command = "claude"
        prompt_file = "prompts/coder.md"
        allowed_write_dirs = ["src/"]

        [[agents.timers]]
        minutes = 10
        prompt_file = "prompts/nonexistent.md"
    "#;
    write_agents_toml(&dot_dir, toml);

    match ProjectConfig::load(tmp.path()) {
        Err(ConfigError::MissingPromptFile(_)) => {}
        Err(e) => panic!("expected MissingPromptFile, got {e}"),
        Ok(_) => panic!("expected MissingPromptFile, got Ok"),
    }
}

#[test]
fn load_rejects_timer_with_invalid_include_agents() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");

    let toml = r#"
        [[agents]]
        id = "coder"
        command = "claude"
        prompt_file = "prompts/coder.md"
        allowed_write_dirs = ["src/"]

        [[agents.timers]]
        minutes = 10
        prompt_file = "prompts/coder.md"
        include_agents = ["nonexistent_agent"]
    "#;
    write_agents_toml(&dot_dir, toml);

    match ProjectConfig::load(tmp.path()) {
        Err(ConfigError::InvalidAgentId(msg)) => {
            assert!(msg.contains("nonexistent_agent"));
        }
        Err(e) => panic!("expected InvalidAgentId, got {e}"),
        Ok(_) => panic!("expected InvalidAgentId, got Ok"),
    }
}

#[test]
fn resolved_timers_renders_template_variables() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "root={{project_root}} id={{agent_id}}");

    let toml = r#"
        [[agents]]
        id = "coder"
        command = "claude"
        prompt_file = "prompts/coder.md"
        allowed_write_dirs = ["src/"]

        [[agents.timers]]
        minutes = 5
        prompt_file = "prompts/coder.md"
    "#;
    write_agents_toml(&dot_dir, toml);

    let config = ProjectConfig::load(tmp.path()).unwrap();
    let timers = config.resolved_timers().unwrap();
    assert_eq!(timers.len(), 1);
    assert_eq!(timers[0].agent_id, "coder");
    assert_eq!(timers[0].minutes, 5);
    assert!(timers[0].prompt.contains("root="));
    assert!(timers[0].prompt.contains("id=coder"));
}
