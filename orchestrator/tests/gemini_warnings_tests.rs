use orchestrator::config::{check_agent_command_warnings, AgentEntry, AgentsToml};

fn agent(id: &str, command: &str) -> AgentEntry {
    AgentEntry {
        id: id.to_string(),
        command: command.to_string(),
        prompt_file: "prompts/coder.md".to_string(),
        allowed_write_dirs: vec!["src/".to_string()],
    }
}

#[test]
fn warns_for_gemini_without_yolo_or_approval_mode() {
    let agents = vec![agent("coder", "gemini")];

    let warnings = check_agent_command_warnings(&agents);

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("coder"));
    assert!(warnings[0].contains("--yolo"));
}

#[test]
fn warns_for_gemini_with_unrelated_flags() {
    let agents = vec![agent("coder", "gemini --sandbox")];

    let warnings = check_agent_command_warnings(&agents);

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("coder"));
}

#[test]
fn warns_for_gemini_with_sandbox_and_model_without_yolo() {
    let agents = vec![agent("tester", "gemini --sandbox -m gemini-2.5-pro")];

    let warnings = check_agent_command_warnings(&agents);

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("tester"));
}

#[test]
fn no_warning_for_gemini_with_yolo() {
    let agents = vec![agent("coder", "gemini --yolo")];

    let warnings = check_agent_command_warnings(&agents);

    assert!(warnings.is_empty());
}

#[test]
fn no_warning_for_gemini_with_yolo_and_sandbox() {
    let agents = vec![agent("tester", "gemini --yolo --sandbox")];

    let warnings = check_agent_command_warnings(&agents);

    assert!(warnings.is_empty());
}

#[test]
fn no_warning_for_gemini_with_approval_mode() {
    let agents = vec![agent("coder", "gemini --approval-mode yolo")];

    let warnings = check_agent_command_warnings(&agents);

    assert!(warnings.is_empty());
}

#[test]
fn no_warning_for_non_gemini_commands() {
    let agents = vec![
        agent("coder", "claude"),
        agent("tester", "codex --approval-mode full-auto"),
        agent("reviewer", "my-custom-gemini-wrapper"),
        agent("helper", "cursor agent"),
    ];

    let warnings = check_agent_command_warnings(&agents);

    assert!(warnings.is_empty());
}

#[test]
fn no_warning_when_command_contains_gemini_but_not_at_start() {
    let agents = vec![agent("coder", "my-gemini-fork")];

    let warnings = check_agent_command_warnings(&agents);

    assert!(warnings.is_empty());
}

#[test]
fn mixed_agents_only_warns_for_missing_flags() {
    let agents = vec![
        agent("coder", "gemini --sandbox"),
        agent("tester", "gemini --yolo"),
        agent("reviewer", "claude"),
    ];

    let warnings = check_agent_command_warnings(&agents);

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("coder"));
}

#[test]
fn empty_and_leading_space_commands_do_not_warn() {
    let agents = vec![agent("coder", ""), agent("tester", "  gemini")];

    let warnings = check_agent_command_warnings(&agents);

    assert!(warnings.is_empty());
}

#[test]
fn toml_parses_gemini_commands() {
    let toml_str = r#"
[[agents]]
id = "coder"
command = "gemini --yolo"
prompt_file = "prompts/coder.md"
allowed_write_dirs = ["src/"]

[[agents]]
id = "tester"
command = "gemini --yolo --sandbox"
prompt_file = "prompts/tester.md"
allowed_write_dirs = ["tests/"]
"#;

    let parsed: AgentsToml = toml::from_str(toml_str).expect("toml should parse");

    assert_eq!(parsed.agents.len(), 2);
    assert_eq!(parsed.agents[0].command, "gemini --yolo");
    assert_eq!(parsed.agents[1].command, "gemini --yolo --sandbox");

    let warnings = check_agent_command_warnings(&parsed.agents);
    assert!(warnings.is_empty());
}

#[test]
fn empty_agents_slice_is_ok() {
    let warnings = check_agent_command_warnings(&[]);

    assert!(warnings.is_empty());
}

// ---------------------------------------------------------------------------
// Cursor warnings
// ---------------------------------------------------------------------------

#[test]
fn warns_for_cursor_without_agent_subcommand() {
    let agents = vec![agent("coder", "cursor")];

    let warnings = check_agent_command_warnings(&agents);

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("coder"));
    assert!(warnings[0].contains("cursor"));
    assert!(warnings[0].contains("agent"));
}

#[test]
fn warns_for_cursor_with_flags_but_no_agent_subcommand() {
    let agents = vec![agent("coder", "cursor --some-flag")];

    let warnings = check_agent_command_warnings(&agents);

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("coder"));
}

#[test]
fn no_warning_for_cursor_agent() {
    let agents = vec![agent("coder", "cursor agent")];

    let warnings = check_agent_command_warnings(&agents);

    assert!(warnings.is_empty());
}

#[test]
fn no_warning_for_cursor_agent_with_flags() {
    let agents = vec![agent("tester", "cursor agent --model gpt-4")];

    let warnings = check_agent_command_warnings(&agents);

    assert!(warnings.is_empty());
}

#[test]
fn no_warning_when_command_contains_cursor_but_not_at_start() {
    let agents = vec![agent("coder", "my-cursor-wrapper")];

    let warnings = check_agent_command_warnings(&agents);

    assert!(warnings.is_empty());
}

#[test]
fn mixed_agents_warns_for_both_gemini_and_cursor() {
    let agents = vec![
        agent("coder", "gemini --sandbox"),
        agent("tester", "cursor"),
        agent("reviewer", "cursor agent"),
    ];

    let warnings = check_agent_command_warnings(&agents);

    assert_eq!(warnings.len(), 2);
    assert!(warnings[0].contains("coder"));
    assert!(warnings[0].contains("gemini"));
    assert!(warnings[1].contains("tester"));
    assert!(warnings[1].contains("cursor"));
}

#[test]
fn toml_parses_cursor_agent_command() {
    let toml_str = r#"
[[agents]]
id = "coder"
command = "cursor agent"
prompt_file = "prompts/coder.md"
allowed_write_dirs = ["src/"]
"#;

    let parsed: AgentsToml = toml::from_str(toml_str).expect("toml should parse");

    assert_eq!(parsed.agents.len(), 1);
    assert_eq!(parsed.agents[0].command, "cursor agent");

    let warnings = check_agent_command_warnings(&parsed.agents);
    assert!(warnings.is_empty());
}
