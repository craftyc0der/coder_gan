use std::path::{Path, PathBuf};

use tempfile::TempDir;

use orchestrator::config::{AgentEntry, ConfigError, ProjectConfig, SplitDirection, WorkerGroupEntry};
use orchestrator::config::{init_project, expand_agent_id};

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
    make_config_with_groups(tmp, agents, vec![])
}

fn make_config_with_groups(
    tmp: &TempDir,
    agents: Vec<AgentEntry>,
    worker_groups: Vec<WorkerGroupEntry>,
) -> ProjectConfig {
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
        worker_groups,
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
    let prompt = timers[0].read_prompt().unwrap();
    assert!(prompt.contains("root="));
    assert!(prompt.contains("id=coder"));
}

// ---------------------------------------------------------------------------
// expand_agent_id
// ---------------------------------------------------------------------------

#[test]
fn expand_agent_id_no_suffix_when_count_is_one() {
    assert_eq!(expand_agent_id("coder", 1, 1), "coder");
}

#[test]
fn expand_agent_id_appends_instance_when_count_greater_than_one() {
    assert_eq!(expand_agent_id("coder", 1, 2), "coder-1");
    assert_eq!(expand_agent_id("coder", 2, 2), "coder-2");
    assert_eq!(expand_agent_id("tester", 3, 5), "tester-3");
}

// ---------------------------------------------------------------------------
// agent_configs with worker groups
// ---------------------------------------------------------------------------

fn make_group_agents() -> Vec<AgentEntry> {
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
        AgentEntry {
            id: "reviewer".into(),
            command: "copilot".into(),
            prompt_file: "prompts/reviewer.md".into(),
            allowed_write_dirs: vec!["/".into()],
            agent_type: Default::default(),
            slack: None,
            timers: vec![],
        },
    ]
}

fn make_worker_group(count: u32) -> WorkerGroupEntry {
    WorkerGroupEntry {
        id: "worker".into(),
        agents: vec!["coder".into(), "tester".into()],
        layout: SplitDirection::Horizontal,
        count,
    }
}

#[test]
fn agent_configs_with_group_count_one_preserves_original_ids() {
    let tmp = TempDir::new().unwrap();
    let config = make_config_with_groups(
        &tmp,
        make_group_agents(),
        vec![make_worker_group(1)],
    );

    let cfgs = config.agent_configs();
    // reviewer is standalone, coder+tester are grouped
    assert_eq!(cfgs.len(), 3);

    let reviewer = cfgs.iter().find(|c| c.agent_id == "reviewer").unwrap();
    assert_eq!(reviewer.tmux_session, "testproject-reviewer");
    assert_eq!(reviewer.tmux_target, "testproject-reviewer");

    let coder = cfgs.iter().find(|c| c.agent_id == "coder").unwrap();
    assert_eq!(coder.tmux_session, "testproject-worker");
    assert_eq!(coder.tmux_target, "testproject-worker:0.0");
    assert_eq!(coder.inbox_dir, config.messages_dir.join("to_coder"));

    let tester = cfgs.iter().find(|c| c.agent_id == "tester").unwrap();
    assert_eq!(tester.tmux_session, "testproject-worker");
    assert_eq!(tester.tmux_target, "testproject-worker:0.1");
    assert_eq!(tester.inbox_dir, config.messages_dir.join("to_tester"));
}

#[test]
fn agent_configs_with_group_count_two_expands_ids() {
    let tmp = TempDir::new().unwrap();
    let config = make_config_with_groups(
        &tmp,
        make_group_agents(),
        vec![make_worker_group(2)],
    );

    let cfgs = config.agent_configs();
    // 1 standalone (reviewer) + 2 instances × 2 members = 5
    assert_eq!(cfgs.len(), 5);

    let coder1 = cfgs.iter().find(|c| c.agent_id == "coder-1").unwrap();
    assert_eq!(coder1.tmux_session, "testproject-worker-1");
    assert_eq!(coder1.tmux_target, "testproject-worker-1:0.0");
    assert_eq!(coder1.inbox_dir, config.messages_dir.join("to_coder-1"));
    assert_eq!(coder1.cli_command, "claude");

    let tester2 = cfgs.iter().find(|c| c.agent_id == "tester-2").unwrap();
    assert_eq!(tester2.tmux_session, "testproject-worker-2");
    assert_eq!(tester2.tmux_target, "testproject-worker-2:0.1");
    assert_eq!(tester2.inbox_dir, config.messages_dir.join("to_tester-2"));
    assert_eq!(tester2.cli_command, "codex");
}

// ---------------------------------------------------------------------------
// worker_group_configs
// ---------------------------------------------------------------------------

#[test]
fn worker_group_configs_single_instance() {
    let tmp = TempDir::new().unwrap();
    let config = make_config_with_groups(
        &tmp,
        make_group_agents(),
        vec![make_worker_group(1)],
    );

    let groups = config.worker_group_configs();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].group_id, "worker");
    assert_eq!(groups[0].session_name, "testproject-worker");
    assert_eq!(groups[0].layout, SplitDirection::Horizontal);
    assert_eq!(groups[0].members.len(), 2);
    assert_eq!(groups[0].members[0].agent_id, "coder");
    assert_eq!(groups[0].members[1].agent_id, "tester");
}

#[test]
fn worker_group_configs_multiple_instances() {
    let tmp = TempDir::new().unwrap();
    let config = make_config_with_groups(
        &tmp,
        make_group_agents(),
        vec![make_worker_group(3)],
    );

    let groups = config.worker_group_configs();
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].session_name, "testproject-worker-1");
    assert_eq!(groups[1].session_name, "testproject-worker-2");
    assert_eq!(groups[2].session_name, "testproject-worker-3");
    for g in &groups {
        assert_eq!(g.members.len(), 2);
        assert_eq!(g.layout, SplitDirection::Horizontal);
    }
}

#[test]
fn worker_group_configs_vertical_layout_propagated() {
    let tmp = TempDir::new().unwrap();
    let group = WorkerGroupEntry {
        id: "pair".into(),
        agents: vec!["coder".into(), "tester".into()],
        layout: SplitDirection::Vertical,
        count: 1,
    };
    let config = make_config_with_groups(&tmp, make_group_agents(), vec![group]);

    let groups = config.worker_group_configs();
    assert_eq!(groups[0].layout, SplitDirection::Vertical);
}

// ---------------------------------------------------------------------------
// startup_prompts with grouped agents
// ---------------------------------------------------------------------------

#[test]
fn startup_prompts_grouped_agents_render_peer_inboxes() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "id={{agent_id}} peers={{peer_inboxes}}");
    write_prompt(&dot_dir, "tester.md", "id={{agent_id}} peers={{peer_inboxes}}");
    write_prompt(&dot_dir, "reviewer.md", "id={{agent_id}}");

    let config = make_config_with_groups(
        &tmp,
        make_group_agents(),
        vec![make_worker_group(1)],
    );

    let prompts = config.startup_prompts().unwrap();

    let coder_prompt = prompts.get("coder").unwrap();
    assert!(coder_prompt.contains("id=coder"));
    assert!(coder_prompt.contains("to_tester"));
    assert!(!coder_prompt.contains("to_coder"));

    let tester_prompt = prompts.get("tester").unwrap();
    assert!(tester_prompt.contains("id=tester"));
    assert!(tester_prompt.contains("to_coder"));
    assert!(!tester_prompt.contains("to_tester"));
}

#[test]
fn startup_prompts_grouped_agents_with_count_two_expand_ids() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "id={{agent_id}} suffix={{instance_suffix}}");
    write_prompt(&dot_dir, "tester.md", "id={{agent_id}}");
    write_prompt(&dot_dir, "reviewer.md", "id={{agent_id}}");

    let config = make_config_with_groups(
        &tmp,
        make_group_agents(),
        vec![make_worker_group(2)],
    );

    let prompts = config.startup_prompts().unwrap();

    assert!(prompts.contains_key("coder-1"));
    assert!(prompts.contains_key("coder-2"));
    assert!(prompts.contains_key("tester-1"));
    assert!(prompts.contains_key("tester-2"));
    assert!(prompts.contains_key("reviewer"));

    let c1 = prompts.get("coder-1").unwrap();
    assert!(c1.contains("id=coder-1"));
    assert!(c1.contains("suffix=-1"));

    let c2 = prompts.get("coder-2").unwrap();
    assert!(c2.contains("id=coder-2"));
    assert!(c2.contains("suffix=-2"));
}

#[test]
fn startup_prompts_worker_inboxes_variable_rendered_for_standalone() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "id={{agent_id}}");
    write_prompt(&dot_dir, "tester.md", "id={{agent_id}}");
    write_prompt(
        &dot_dir,
        "reviewer.md",
        "workers={{worker_inboxes}}",
    );

    let config = make_config_with_groups(
        &tmp,
        make_group_agents(),
        vec![make_worker_group(1)],
    );

    let prompts = config.startup_prompts().unwrap();
    let reviewer_prompt = prompts.get("reviewer").unwrap();
    assert!(reviewer_prompt.contains("to_coder"));
    assert!(reviewer_prompt.contains("to_tester"));
}

// ---------------------------------------------------------------------------
// ensure_dirs with groups
// ---------------------------------------------------------------------------

#[test]
fn ensure_dirs_creates_expanded_group_inboxes() {
    let tmp = TempDir::new().unwrap();
    let config = make_config_with_groups(
        &tmp,
        make_group_agents(),
        vec![make_worker_group(2)],
    );

    config.ensure_dirs().unwrap();

    // Expanded group inboxes
    assert!(config.messages_dir.join("to_coder-1").is_dir());
    assert!(config.messages_dir.join("to_coder-2").is_dir());
    assert!(config.messages_dir.join("to_tester-1").is_dir());
    assert!(config.messages_dir.join("to_tester-2").is_dir());
    // Standalone reviewer
    assert!(config.messages_dir.join("to_reviewer").is_dir());
    // Grouped agents should NOT have unsuffixed inbox dirs
    assert!(!config.messages_dir.join("to_coder").is_dir());
    assert!(!config.messages_dir.join("to_tester").is_dir());
}

#[test]
fn ensure_dirs_with_count_one_uses_original_ids() {
    let tmp = TempDir::new().unwrap();
    let config = make_config_with_groups(
        &tmp,
        make_group_agents(),
        vec![make_worker_group(1)],
    );

    config.ensure_dirs().unwrap();

    assert!(config.messages_dir.join("to_coder").is_dir());
    assert!(config.messages_dir.join("to_tester").is_dir());
    assert!(config.messages_dir.join("to_reviewer").is_dir());
}

// ---------------------------------------------------------------------------
// Worker group validation (via ProjectConfig::load)
// ---------------------------------------------------------------------------

#[test]
fn load_rejects_worker_group_with_empty_agents() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");

    let toml = r#"
        [[agents]]
        id = "coder"
        command = "claude"
        prompt_file = "prompts/coder.md"
        allowed_write_dirs = ["src/"]

        [[worker_groups]]
        id = "empty"
        agents = []
    "#;
    write_agents_toml(&dot_dir, toml);

    match ProjectConfig::load(tmp.path()) {
        Err(ConfigError::InvalidWorkerGroup(msg)) => {
            assert!(msg.contains("no agents listed"));
        }
        Err(e) => panic!("expected InvalidWorkerGroup, got {e}"),
        Ok(_) => panic!("expected InvalidWorkerGroup, got Ok"),
    }
}

#[test]
fn load_rejects_worker_group_with_count_zero() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");

    let toml = r#"
        [[agents]]
        id = "coder"
        command = "claude"
        prompt_file = "prompts/coder.md"
        allowed_write_dirs = ["src/"]

        [[worker_groups]]
        id = "bad"
        agents = ["coder"]
        count = 0
    "#;
    write_agents_toml(&dot_dir, toml);

    match ProjectConfig::load(tmp.path()) {
        Err(ConfigError::InvalidWorkerGroup(msg)) => {
            assert!(msg.contains("count = 0"));
        }
        Err(e) => panic!("expected InvalidWorkerGroup, got {e}"),
        Ok(_) => panic!("expected InvalidWorkerGroup, got Ok"),
    }
}

#[test]
fn load_rejects_worker_group_with_unknown_agent_ref() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");

    let toml = r#"
        [[agents]]
        id = "coder"
        command = "claude"
        prompt_file = "prompts/coder.md"
        allowed_write_dirs = ["src/"]

        [[worker_groups]]
        id = "bad"
        agents = ["coder", "ghost"]
    "#;
    write_agents_toml(&dot_dir, toml);

    match ProjectConfig::load(tmp.path()) {
        Err(ConfigError::InvalidWorkerGroup(msg)) => {
            assert!(msg.contains("ghost"));
        }
        Err(e) => panic!("expected InvalidWorkerGroup, got {e}"),
        Ok(_) => panic!("expected InvalidWorkerGroup, got Ok"),
    }
}

#[test]
fn load_rejects_agent_in_multiple_worker_groups() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");
    write_prompt(&dot_dir, "tester.md", "hello");

    let toml = r#"
        [[agents]]
        id = "coder"
        command = "claude"
        prompt_file = "prompts/coder.md"
        allowed_write_dirs = ["src/"]

        [[agents]]
        id = "tester"
        command = "codex"
        prompt_file = "prompts/tester.md"
        allowed_write_dirs = ["tests/"]

        [[worker_groups]]
        id = "first"
        agents = ["coder"]

        [[worker_groups]]
        id = "second"
        agents = ["coder", "tester"]
    "#;
    write_agents_toml(&dot_dir, toml);

    match ProjectConfig::load(tmp.path()) {
        Err(ConfigError::InvalidWorkerGroup(msg)) => {
            assert!(msg.contains("appears in multiple worker groups"));
            assert!(msg.contains("coder"));
        }
        Err(e) => panic!("expected InvalidWorkerGroup, got {e}"),
        Ok(_) => panic!("expected InvalidWorkerGroup, got Ok"),
    }
}

#[test]
fn load_accepts_valid_worker_group() {
    let tmp = TempDir::new().unwrap();
    let dot_dir = make_dot_dir(tmp.path());
    write_prompt(&dot_dir, "coder.md", "hello");
    write_prompt(&dot_dir, "tester.md", "hello");

    let toml = r#"
        [[agents]]
        id = "coder"
        command = "claude"
        prompt_file = "prompts/coder.md"
        allowed_write_dirs = ["src/"]

        [[agents]]
        id = "tester"
        command = "codex"
        prompt_file = "prompts/tester.md"
        allowed_write_dirs = ["tests/"]

        [[worker_groups]]
        id = "worker"
        agents = ["coder", "tester"]
        layout = "vertical"
        count = 2
    "#;
    write_agents_toml(&dot_dir, toml);

    let config = ProjectConfig::load(tmp.path()).unwrap();
    assert_eq!(config.worker_groups.len(), 1);
    assert_eq!(config.worker_groups[0].id, "worker");
    assert_eq!(config.worker_groups[0].count, 2);
    assert_eq!(config.worker_groups[0].layout, SplitDirection::Vertical);
}

// ---------------------------------------------------------------------------
// group_session_for
// ---------------------------------------------------------------------------

#[test]
fn group_session_for_no_suffix_when_total_one() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    assert_eq!(config.group_session_for("worker", 1, 1), "testproject-worker");
}

#[test]
fn group_session_for_appends_instance_when_total_greater_than_one() {
    let tmp = TempDir::new().unwrap();
    let config = make_config(&tmp, make_agents());
    assert_eq!(config.group_session_for("worker", 1, 2), "testproject-worker-1");
    assert_eq!(config.group_session_for("worker", 2, 2), "testproject-worker-2");
}
