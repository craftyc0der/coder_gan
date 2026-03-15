use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::supervisor::AgentConfig;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    NotInitialized(PathBuf),
    IoError(std::io::Error),
    TomlParse(String),
    MissingPromptFile(PathBuf),
    NoAgents,
    InvalidAgentId(String),
    SlackConfigError(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotInitialized(p) => write!(
                f,
                "No .orchestrator/ directory found at {}. Run 'orchestrator init' first.",
                p.display()
            ),
            ConfigError::IoError(e) => write!(f, "IO error: {e}"),
            ConfigError::TomlParse(e) => write!(f, "Failed to parse agents.toml: {e}"),
            ConfigError::MissingPromptFile(p) => {
                write!(f, "Prompt file not found: {}", p.display())
            }
            ConfigError::NoAgents => write!(f, "agents.toml contains no [[agents]] entries"),
            ConfigError::InvalidAgentId(id) => write!(
                f,
                "Invalid agent id '{}': must be alphanumeric, hyphens, or underscores only",
                id
            ),
            ConfigError::SlackConfigError(e) => write!(f, "Slack config error: {e}"),
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::IoError(e)
    }
}

// ---------------------------------------------------------------------------
// TOML schema
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AgentsToml {
    pub agents: Vec<AgentEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimerEntry {
    pub minutes: u64,
    pub prompt_file: String,
    #[serde(default)]
    pub interrupt: bool,
    #[serde(default)]
    pub include_agents: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentEntry {
    pub id: String,
    pub command: String,
    pub prompt_file: String,
    pub allowed_write_dirs: Vec<String>,
    #[serde(default)]
    pub agent_type: AgentType,
    #[serde(default)]
    pub slack: Option<SlackAgentConfig>,
    #[serde(default)]
    pub timers: Vec<TimerEntry>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AgentType {
    #[default]
    Cli,
    Slack,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SlackAgentConfig {
    pub config_file: String,
}

// ---------------------------------------------------------------------------
// Slack configuration (parsed from external slack_config.toml)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct SlackConfigToml {
    #[serde(default)]
    pub bot_token: Option<String>,
    #[serde(default)]
    pub bot_token_env: Option<String>,
    #[serde(default)]
    pub app_token: Option<String>,
    #[serde(default)]
    pub app_token_env: Option<String>,
    #[serde(default)]
    pub user_token: Option<String>,
    #[serde(default)]
    pub user_token_env: Option<String>,
    pub bot_user_id: String,
    #[serde(default)]
    pub watch_channels: Vec<String>,
    #[serde(default = "default_true")]
    pub watch_mentions: bool,
    #[serde(default = "default_true")]
    pub watch_replied_threads: bool,
    #[serde(default = "default_true")]
    pub watch_dms: bool,
    pub notification_channel: String,
    pub alert_user_id: String,
    #[serde(default = "default_min_message_length")]
    pub min_message_length: usize,
    #[serde(default)]
    pub ignore_bot_ids: Vec<String>,
    #[serde(default)]
    pub alert_keywords: Vec<String>,
}

fn default_true() -> bool {
    true
}
fn default_min_message_length() -> usize {
    20
}

/// Resolved Slack configuration with tokens loaded from env/inline.
#[derive(Debug, Clone)]
pub struct SlackConfig {
    pub bot_token: String,
    pub app_token: String,
    /// Optional user token (`xoxp-...`) for reading the installing user's DMs,
    /// private channels, and threads as if the bot were that user.
    pub user_token: Option<String>,
    pub bot_user_id: String,
    pub watch_channels: Vec<String>,
    pub watch_mentions: bool,
    pub watch_replied_threads: bool,
    pub watch_dms: bool,
    pub notification_channel: String,
    pub alert_user_id: String,
    pub min_message_length: usize,
    pub ignore_bot_ids: Vec<String>,
    pub alert_keywords: Vec<String>,
}

impl SlackConfig {
    /// Load and resolve a SlackConfig from the external TOML file.
    pub fn load(dot_dir: &Path, slack_agent_config: &SlackAgentConfig) -> Result<Self, ConfigError> {
        let config_path = dot_dir.join(&slack_agent_config.config_file)
            .canonicalize()
            .unwrap_or_else(|_| dot_dir.join(&slack_agent_config.config_file));

        // Try path relative to dot_dir first, then as-is
        let toml_str = std::fs::read_to_string(&config_path)
            .or_else(|_| std::fs::read_to_string(&slack_agent_config.config_file))
            .map_err(|e| ConfigError::SlackConfigError(
                format!("Failed to read {}: {e}", slack_agent_config.config_file)
            ))?;

        let raw: SlackConfigToml = toml::from_str(&toml_str)
            .map_err(|e| ConfigError::SlackConfigError(
                format!("Failed to parse slack config: {e}")
            ))?;

        let bot_token = resolve_token("bot_token", &raw.bot_token, &raw.bot_token_env)?;
        let app_token = resolve_token("app_token", &raw.app_token, &raw.app_token_env)?;
        let user_token = resolve_token("user_token", &raw.user_token, &raw.user_token_env).ok();

        Ok(SlackConfig {
            bot_token,
            app_token,
            user_token,
            bot_user_id: raw.bot_user_id,
            watch_channels: raw.watch_channels,
            watch_mentions: raw.watch_mentions,
            watch_replied_threads: raw.watch_replied_threads,
            watch_dms: raw.watch_dms,
            notification_channel: raw.notification_channel,
            alert_user_id: raw.alert_user_id,
            min_message_length: raw.min_message_length,
            ignore_bot_ids: raw.ignore_bot_ids,
            alert_keywords: raw.alert_keywords,
        })
    }
}

/// Resolve a token from env var or inline value.
fn resolve_token(
    name: &str,
    inline: &Option<String>,
    env_var_name: &Option<String>,
) -> Result<String, ConfigError> {
    // Env var takes priority
    if let Some(env_name) = env_var_name {
        if let Ok(val) = std::env::var(env_name) {
            if !val.is_empty() {
                return Ok(val);
            }
        }
    }
    // Fall back to inline
    if let Some(val) = inline {
        if !val.is_empty() {
            return Ok(val.clone());
        }
    }
    Err(ConfigError::SlackConfigError(format!(
        "No {name} provided. Set {name}_env to an env var name, or set {name} inline."
    )))
}

// ---------------------------------------------------------------------------
// Resolved project configuration
// ---------------------------------------------------------------------------

pub struct ProjectConfig {
    pub project_root: PathBuf,
    pub project_name: String,
    pub dot_dir: PathBuf,
    pub messages_dir: PathBuf,
    pub log_dir: PathBuf,
    pub state_path: PathBuf,
    pub transcript_dir: PathBuf,
    pub agents: Vec<AgentEntry>,
}

impl ProjectConfig {
    /// Load configuration from `<project_path>/.orchestrator/agents.toml`.
    pub fn load(project_path: &Path) -> Result<Self, ConfigError> {
        let project_root = std::fs::canonicalize(project_path).map_err(ConfigError::IoError)?;
        let dot_dir = project_root.join(".orchestrator");

        if !dot_dir.exists() {
            return Err(ConfigError::NotInitialized(project_root));
        }

        let toml_path = dot_dir.join("agents.toml");
        let toml_str = std::fs::read_to_string(&toml_path).map_err(ConfigError::IoError)?;
        let agents_toml: AgentsToml =
            toml::from_str(&toml_str).map_err(|e| ConfigError::TomlParse(e.to_string()))?;

        if agents_toml.agents.is_empty() {
            return Err(ConfigError::NoAgents);
        }

        // Validate agent IDs
        for agent in &agents_toml.agents {
            if !agent
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(ConfigError::InvalidAgentId(agent.id.clone()));
            }
        }

        let project_name = sanitize_project_name(&project_root);
        let messages_dir = dot_dir.join("messages");
        let log_dir = dot_dir.join("runtime/logs");
        let state_path = log_dir.join("state.json");
        let transcript_dir = log_dir.join("spike_transcripts");

        // Validate slack agents
        for agent in &agents_toml.agents {
            if agent.agent_type == AgentType::Slack {
                if agent.slack.is_none() {
                    return Err(ConfigError::SlackConfigError(format!(
                        "Agent '{}' has agent_type = \"slack\" but no [agents.slack] table",
                        agent.id
                    )));
                }
                if agent.command.is_empty() {
                    return Err(ConfigError::SlackConfigError(format!(
                        "Agent '{}' has agent_type = \"slack\" but command is empty. \
                         Set command to the CLI for the triage AI (e.g., \"claude\").",
                        agent.id
                    )));
                }
            }
        }

        // Validate timer entries
        let all_ids: Vec<&str> = agents_toml.agents.iter().map(|a| a.id.as_str()).collect();
        for agent in &agents_toml.agents {
            for timer in &agent.timers {
                // Validate timer prompt file exists
                let prompt_path = dot_dir.join(&timer.prompt_file);
                if !prompt_path.exists() {
                    return Err(ConfigError::MissingPromptFile(prompt_path));
                }
                // Validate include_agents references
                for ref_id in &timer.include_agents {
                    if !all_ids.contains(&ref_id.as_str()) {
                        return Err(ConfigError::InvalidAgentId(format!(
                            "timer include_agents '{}' on agent '{}' does not match any agent",
                            ref_id, agent.id
                        )));
                    }
                }
            }
        }

        Ok(ProjectConfig {
            project_root,
            project_name,
            dot_dir,
            messages_dir,
            log_dir,
            state_path,
            transcript_dir,
            agents: agents_toml.agents,
        })
    }

    /// Create all required directories under `.orchestrator/`.
    pub fn ensure_dirs(&self) -> Result<(), std::io::Error> {
        for agent in &self.agents {
            std::fs::create_dir_all(self.messages_dir.join(format!("to_{}", agent.id)))?;
        }
        std::fs::create_dir_all(self.messages_dir.join("processed"))?;
        std::fs::create_dir_all(self.messages_dir.join("dead_letter"))?;
        std::fs::create_dir_all(&self.log_dir)?;
        std::fs::create_dir_all(&self.transcript_dir)?;
        std::fs::create_dir_all(self.dot_dir.join("runtime/pids"))?;
        Ok(())
    }

    /// Convert agent entries into supervisor AgentConfig structs.
    pub fn agent_configs(&self) -> Vec<AgentConfig> {
        self.agents
            .iter()
            .map(|a| AgentConfig {
                agent_id: a.id.clone(),
                cli_command: a.command.clone(),
                tmux_session: self.tmux_session_for(&a.id),
                inbox_dir: self.messages_dir.join(format!("to_{}", a.id)),
                allowed_write_dirs: a
                    .allowed_write_dirs
                    .iter()
                    .map(|d| self.project_root.join(d))
                    .collect(),
            })
            .collect()
    }

    /// Read and render startup prompt files, substituting template variables.
    pub fn startup_prompts(&self) -> Result<HashMap<String, String>, ConfigError> {
        let mut prompts = HashMap::new();
        for agent in &self.agents {
            let prompt_path = self.dot_dir.join(&agent.prompt_file);
            if !prompt_path.exists() {
                return Err(ConfigError::MissingPromptFile(prompt_path));
            }
            let raw = std::fs::read_to_string(&prompt_path)?;
            let rendered = raw
                .replace("{{project_root}}", &self.project_root.display().to_string())
                .replace("{{messages_dir}}", &self.messages_dir.display().to_string())
                .replace("{{agent_id}}", &agent.id);
            prompts.insert(agent.id.clone(), rendered);
        }
        Ok(prompts)
    }

    /// Build resolved timer configs for all agents.
    /// Stores paths and template variables so prompts are read fresh at fire time.
    pub fn resolved_timers(&self) -> Result<Vec<ResolvedTimer>, ConfigError> {
        let mut timers = Vec::new();
        for agent in &self.agents {
            for timer in &agent.timers {
                let prompt_path = self.dot_dir.join(&timer.prompt_file);
                // Validate the file exists at load time
                if !prompt_path.exists() {
                    return Err(ConfigError::MissingPromptFile(prompt_path));
                }
                timers.push(ResolvedTimer {
                    agent_id: agent.id.clone(),
                    minutes: timer.minutes,
                    prompt_path,
                    project_root: self.project_root.display().to_string(),
                    messages_dir: self.messages_dir.display().to_string(),
                    interrupt: timer.interrupt,
                    include_agents: timer.include_agents.clone(),
                });
            }
        }
        Ok(timers)
    }

    /// Derive the tmux session name for a given agent.
    pub fn tmux_session_for(&self, agent_id: &str) -> String {
        format!("{}-{}", self.project_name, agent_id)
    }
}

/// A fully resolved timer ready for the timer loop.
/// Prompt file is read fresh each time the timer fires.
#[derive(Debug, Clone)]
pub struct ResolvedTimer {
    pub agent_id: String,
    pub minutes: u64,
    pub prompt_path: PathBuf,
    pub project_root: String,
    pub messages_dir: String,
    pub interrupt: bool,
    pub include_agents: Vec<String>,
}

impl ResolvedTimer {
    /// Read and render the prompt file. Called each time the timer fires
    /// so edits to the file take effect without an orchestrator restart.
    pub fn read_prompt(&self) -> Result<String, std::io::Error> {
        let raw = std::fs::read_to_string(&self.prompt_path)?;
        Ok(raw
            .replace("{{project_root}}", &self.project_root)
            .replace("{{messages_dir}}", &self.messages_dir)
            .replace("{{agent_id}}", &self.agent_id))
    }
}

// ---------------------------------------------------------------------------
// Project name sanitization
// ---------------------------------------------------------------------------

/// Derive a tmux-safe project name from the directory path.
/// Only alphanumeric and hyphens are kept; everything else becomes a hyphen.
fn sanitize_project_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");

    let sanitized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // Collapse consecutive hyphens and trim leading/trailing hyphens
    let mut result = String::new();
    let mut prev_hyphen = true; // treat start as if preceded by hyphen to trim leading
    for c in sanitized.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push('-');
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }

    // Trim trailing hyphen
    if result.ends_with('-') {
        result.pop();
    }

    if result.is_empty() {
        "project".to_string()
    } else {
        result
    }
}

// ---------------------------------------------------------------------------
// Scaffold / init
// ---------------------------------------------------------------------------

/// Create a new `.orchestrator/` directory with starter config and prompt files.
pub fn init_project(project_path: &Path) -> Result<(), ConfigError> {
    let dot_dir = project_path.join(".orchestrator");
    if dot_dir.join("agents.toml").exists() {
        eprintln!(
            "Warning: .orchestrator/agents.toml already exists at {}",
            project_path.display()
        );
        eprintln!("Skipping init to avoid overwriting existing config.");
        return Ok(());
    }

    let prompts_dir = dot_dir.join("prompts");
    std::fs::create_dir_all(&prompts_dir)?;
    std::fs::create_dir_all(dot_dir.join("messages/processed"))?;
    std::fs::create_dir_all(dot_dir.join("messages/dead_letter"))?;
    std::fs::create_dir_all(dot_dir.join("messages/to_coder"))?;
    std::fs::create_dir_all(dot_dir.join("messages/to_tester"))?;
    std::fs::create_dir_all(dot_dir.join("messages/to_reviewer"))?;
    std::fs::create_dir_all(dot_dir.join("runtime/logs/spike_transcripts"))?;
    std::fs::create_dir_all(dot_dir.join("runtime/pids"))?;

    // Write agents.toml
    std::fs::write(dot_dir.join("agents.toml"), DEFAULT_AGENTS_TOML)?;

    // Write default prompt files
    std::fs::write(prompts_dir.join("coder.md"), DEFAULT_CODER_PROMPT)?;
    std::fs::write(prompts_dir.join("tester.md"), DEFAULT_TESTER_PROMPT)?;
    std::fs::write(prompts_dir.join("reviewer.md"), DEFAULT_REVIEWER_PROMPT)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Startup validation warnings
// ---------------------------------------------------------------------------

/// Check agent commands for known configuration issues and return warning messages.
///
/// These are non-blocking informational warnings only — they never prevent startup.
/// Currently checks:
/// - Gemini without `--yolo` or `--approval-mode`: will block on action confirmations.
/// - Cursor without `agent` subcommand: will open the GUI instead of running as CLI.
pub fn check_agent_command_warnings(agents: &[AgentEntry]) -> Vec<String> {
    let mut warnings = Vec::new();
    for agent in agents {
        let cmd = agent.command.as_str();
        if cmd == "gemini" || cmd.starts_with("gemini ") {
            if !cmd.contains("--yolo") && !cmd.contains("--approval-mode") {
                warnings.push(format!(
                    "Warning: Agent '{}' uses gemini without --yolo. \
                     It may block on action confirmations.",
                    agent.id
                ));
            }
        }
        if cmd == "cursor" || cmd.starts_with("cursor ") {
            if cmd != "cursor agent" && !cmd.starts_with("cursor agent ") {
                warnings.push(format!(
                    "Warning: Agent '{}' uses cursor without 'agent' subcommand. \
                     Use 'cursor agent' for CLI mode; plain 'cursor' opens the GUI.",
                    agent.id
                ));
            }
        }
    }
    warnings
}

// ---------------------------------------------------------------------------
// Default config and prompts
// ---------------------------------------------------------------------------

const DEFAULT_AGENTS_TOML: &str = r#"# Orchestrator agent configuration
# Each [[agents]] block defines one autonomous agent.
# Tmux session names are auto-derived: <project-name>-<agent-id>
# Inbox directories are auto-derived: .orchestrator/messages/to_<agent-id>/
#
# Supported CLI tools and recommended flags:
#   claude:  claude --dangerously-skip-permissions
#   codex:   codex --approval-mode full-auto
#   copilot: copilot
#   cursor:  cursor agent                (CLI mode; required — plain 'cursor' opens the GUI)
#   gemini:  gemini --yolo
#            gemini --yolo --sandbox    (sandboxed; recommended for tester agents)
#            gemini --yolo -m gemini-2.5-pro  (specific model)
#
# IMPORTANT: Gemini agents must use --yolo or --approval-mode yolo for
# autonomous operation. Without it, Gemini will block on action confirmations.
# Cursor agents must use 'cursor agent' (not plain 'cursor') for CLI mode.

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

[[agents]]
id = "reviewer"
command = "copilot"
prompt_file = "prompts/reviewer.md"
allowed_write_dirs = ["review/"]
"#;

const DEFAULT_CODER_PROMPT: &str = r#"You are the CODER agent in a multi-agent coding system.

PROJECT ROOT: {{project_root}}
YOUR AGENT ID: {{agent_id}}

=== YOUR ROLE ===

You are responsible for writing implementation code.

WRITE TO: {{project_root}}/src/
DO NOT WRITE TO: tests/ or review/

=== HOW TO WORK WITH THE TESTER ===

You do NOT send the tester your source code directly. Instead, when you have
written or changed implementation code, send the tester a message that includes:

1. A description of what the code does and what behavior should be tested.
2. The public API definition — function signatures, input/output types, error
   cases, and any edge cases you are aware of.
3. Suggested test scenarios describing what a good test case should verify.
4. Any relevant requirements or context the tester needs to understand.

The tester will write tests based on your description, not by reading your
source. This keeps the tests honest — they validate behavior, not implementation
details. All context the tester needs should be included in your messages.

Example message to the tester:

  I've implemented the `parse_config(path: &str) -> Result<Config, ConfigError>`
  function in src/config.rs. It reads a TOML file and returns a Config struct.

  Please write tests that verify:
  - Valid TOML files parse successfully and all fields are populated.
  - Missing required fields return `ConfigError::MissingField`.
  - Malformed TOML returns `ConfigError::ParseError`.
  - The path argument handles both absolute and relative paths.

=== HOW TO SEND MESSAGES ===

Write a file to the recipient's inbox directory. Use this naming convention:
<timestamp>__from-{{agent_id}}__to-<recipient>__topic-<topic>.md

Inbox directories:
- {{messages_dir}}/to_tester/   (send test requests to the tester)
- {{messages_dir}}/to_reviewer/ (escalate disagreements to the reviewer)

=== CRITICAL REQUIREMENT: REPLY TO REQUESTER ===

Whenever you finish requested work, you MUST send a completion message directly
to the agent or operator who made the request. Do NOT simply complete the work
without notifying the requester.

Your completion message must be written to the requesting agent's inbox and must:
1. Confirm what was done.
2. Include any output, results, or next steps the requester needs to proceed.

Announcing "done" in your session output without sending a message to the
requesting agent's inbox is NOT sufficient and violates this requirement.

=== INCOMING MESSAGES ===

Messages from other agents will be pasted into this session with a header:
--- INCOMING MESSAGE ---
FROM: <agent>
TOPIC: <topic>
---

When the tester sends you questions or disagreements, answer them directly.
If you and the tester cannot agree, either of you can escalate to the reviewer
by writing to {{messages_dir}}/to_reviewer/ explaining the disagreement.

=== GETTING STARTED ===

Wait for instructions. All tasks and context will arrive via messages from
other agents or the operator. You may read the README.md to get your bearings,
but wait until you receive a request before starting work.
"#;

const DEFAULT_TESTER_PROMPT: &str = r#"You are the TESTER agent in a multi-agent coding system.

PROJECT ROOT: {{project_root}}
YOUR AGENT ID: {{agent_id}}

=== YOUR ROLE ===

You are responsible for writing tests that verify the implementation code works
correctly. You write tests based on API definitions and behavior descriptions
you receive from the coder — NOT by reading the source code directly.

WRITE TO: {{project_root}}/tests/
DO NOT WRITE TO: src/

=== HOW YOU RECEIVE WORK ===

The coder will send you messages describing:
1. What the code does and what behavior should be tested.
2. The public API — function signatures, types, error cases.
3. Suggested test scenarios.
4. Relevant requirements or context.

Use these descriptions to write thorough tests. All context you need will come
through messages. Your tests should validate behavior and contracts, not
implementation details. If the tests pass, the code works. If the tests fail,
the implementation has a bug.

=== ASKING QUESTIONS ===

If something is unclear or you disagree with the coder's API design, send your
questions directly to the coder. Be specific about what is ambiguous:

  I have a question about `parse_config`. Your API description doesn't mention
  what happens when the path is an empty string vs. missing entirely. Should
  those be different errors?

=== HANDLING DISAGREEMENTS ===

If you and the coder cannot resolve a disagreement after exchanging messages,
escalate to the reviewer. Write a message to the reviewer that includes:
1. A summary of the disagreement.
2. Your position and reasoning.
3. The coder's position (quote their message if helpful).
4. What you'd like the reviewer to decide.

The reviewer will moderate and send a decision back to both of you.

=== HOW TO SEND MESSAGES ===

Write a file to the recipient's inbox directory. Use this naming convention:
<timestamp>__from-{{agent_id}}__to-<recipient>__topic-<topic>.md

Inbox directories:
- {{messages_dir}}/to_coder/    (send questions or results to the coder)
- {{messages_dir}}/to_reviewer/ (escalate disagreements to the reviewer)

=== CRITICAL REQUIREMENT: REPLY TO REQUESTER ===

Whenever you finish requested work, you MUST send a completion message directly
to the agent or operator who made the request. Do NOT simply complete the work
without notifying the requester.

Your completion message must be written to the requesting agent's inbox and must:
1. Confirm what was done.
2. Include any output, results, or next steps the requester needs to proceed.

Announcing "done" in your session output without sending a message to the
requesting agent's inbox is NOT sufficient and violates this requirement.

=== INCOMING MESSAGES ===

Messages from other agents will be pasted into this session with a header:
--- INCOMING MESSAGE ---
FROM: <agent>
TOPIC: <topic>
---

=== GETTING STARTED ===

Wait for instructions. All tasks and context will arrive via messages from
the coder or the operator. You may read the README.md to get your bearings,
but wait until you receive a test request before writing tests.
"#;

const DEFAULT_REVIEWER_PROMPT: &str = r#"You are the REVIEWER agent in a multi-agent coding system.

PROJECT ROOT: {{project_root}}
YOUR AGENT ID: {{agent_id}}

=== YOUR ROLE ===

You are the moderator and quality gatekeeper. Your primary job is to respond
to review requests from other agents. You do NOT need to proactively write
review notes or store artifacts in the source tree.

1. DISPUTE RESOLUTION: When the coder and tester disagree, you review both
   positions and make a binding decision.
2. QUALITY REVIEW: When asked, review implementation code or tests for
   correctness and completeness.

Your responses are delivered via messages to the requesting agents — there is
no need to write review documents to disk unless explicitly asked to do so.

=== HOW DISPUTES WORK ===

When the coder and tester escalate a disagreement to you, they will send a
message explaining:
1. What the disagreement is about.
2. Each side's position and reasoning.
3. What they want you to decide.

Your job is to:
1. Read both positions carefully.
2. Make a clear decision based on the arguments presented.
3. Send your decision to BOTH the coder and the tester so they can proceed.

Be direct and specific. Don't just say "the coder is right" — explain why and
what the tester should change (or vice versa).

=== HOW TO SEND MESSAGES ===

Write a file to the recipient's inbox directory. Use this naming convention:
<timestamp>__from-{{agent_id}}__to-<recipient>__topic-<topic>.md

Inbox directories:
- {{messages_dir}}/to_coder/  (send decisions or feedback to the coder)
- {{messages_dir}}/to_tester/ (send decisions or feedback to the tester)

When resolving a dispute, send your decision to BOTH agents.

=== RESTARTING AGENTS (FRESH CONTEXT) ===

You can restart any agent with a clean slate by writing a message with the
special topic `_RESTART`. The orchestrator will kill the agent's session,
respawn it, and re-inject its original startup prompt — giving it a completely
fresh context window.

To restart an agent, write a file with topic-_RESTART to its inbox:
<timestamp>__from-{{agent_id}}__to-<recipient>__topic-_RESTART.md

The file content can be empty or contain a brief reason for the restart.

Examples:
- {{messages_dir}}/to_coder/<timestamp>__from-{{agent_id}}__to-coder__topic-_RESTART.md
- {{messages_dir}}/to_tester/<timestamp>__from-{{agent_id}}__to-tester__topic-_RESTART.md

WHEN TO RESTART: After a task has been completed successfully — once the coder
has finished implementation and the tester has confirmed tests pass — restart
both agents preemptively. This clears their context windows so they start the
next task fresh, without accumulated context from the previous task polluting
their reasoning. Do not wait to be asked; restart them as soon as a task is
fully done.

=== CRITICAL REQUIREMENT: REPLY TO REQUESTER ===

Whenever you finish requested work, you MUST send a completion message directly
to the agent or operator who made the request. Do NOT simply complete the work
without notifying the requester.

Your completion message must be written to the requesting agent's inbox and must:
1. Confirm what was done.
2. Include any output, results, or next steps the requester needs to proceed.

Announcing "done" in your session output without sending a message to the
requesting agent's inbox is NOT sufficient and violates this requirement.

=== INCOMING MESSAGES ===

Messages from other agents will be pasted into this session with a header:
--- INCOMING MESSAGE ---
FROM: <agent>
TOPIC: <topic>
---

=== GETTING STARTED ===

Wait for messages from the coder or tester before taking action. You act on
request, not proactively.
"#;

