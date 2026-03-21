use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::supervisor::{AgentConfig, WorkerGroupConfig};

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
    InvalidWorkerGroup(String),
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
            ConfigError::InvalidWorkerGroup(e) => write!(f, "Invalid worker_group: {e}"),
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

/// How panes are arranged in a worker-group tmux session.
/// `Horizontal` splits left|right; `Vertical` splits top|bottom.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    #[default]
    Horizontal,
    Vertical,
}

/// A named group of agents that are always launched together in the same tmux
/// session, arranged side-by-side according to `layout`.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkerGroupEntry {
    /// Logical name for this group (used to name the tmux session).
    pub id: String,
    /// Ordered list of agent IDs that belong to this group.
    pub agents: Vec<String>,
    /// How to split the tmux window: horizontal (left|right) or vertical (top|bottom).
    #[serde(default)]
    pub layout: SplitDirection,
    /// How many instances of this group to launch.
    #[serde(default = "default_count")]
    pub count: u32,
}

fn default_count() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
pub struct AgentsToml {
    pub agents: Vec<AgentEntry>,
    #[serde(default)]
    pub worker_groups: Vec<WorkerGroupEntry>,
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
    /// Optional git branch for this agent's worktree. Supports `{{branch}}`
    /// template variable which is replaced with the `--branch` CLI value.
    /// When omitted, defaults to `<feature>/<agent_id>`.
    #[serde(default)]
    pub branch: Option<String>,
    /// Optional prompt file appended to the startup prompt when `--worktree`
    /// is active. Path is relative to `.orchestrator/` (e.g. `prompts/coder-worktree.md`).
    #[serde(default)]
    pub worktree_prompt_file: Option<String>,
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
    pub worker_groups: Vec<WorkerGroupEntry>,
    /// Worktree mode: set when `--worktree --branch <name>` is used.
    /// Contains the feature/branch name (e.g. "PR-123").
    pub worktree_feature: Option<String>,
    /// Resolved worktree info per agent (populated after setup_worktrees).
    pub worktrees: Vec<crate::worktree::AgentWorktree>,
}

#[derive(Debug, Clone)]
struct PromptTemplateContext {
    project_root: String,
    messages_dir: String,
    agent_id: String,
    instance_suffix: String,
    instance_index: String,
    group_count: String,
    peer_ids: String,
    peer_inboxes: String,
    worker_inboxes: String,
    worker_instance_vars: Vec<(String, String)>,
    my_branch: String,
    other_branches: String,
    worktree_root: String,
}

fn apply_prompt_template_variables(raw: String, context: &PromptTemplateContext) -> String {
    let mut rendered = raw
        .replace("{{project_root}}", &context.project_root)
        .replace("{{messages_dir}}", &context.messages_dir)
        .replace("{{agent_id}}", &context.agent_id)
        .replace("{{instance_suffix}}", &context.instance_suffix)
        .replace("{{instance_index}}", &context.instance_index)
        .replace("{{group_count}}", &context.group_count)
        .replace("{{peer_ids}}", &context.peer_ids)
        .replace("{{peer_inboxes}}", &context.peer_inboxes)
        .replace("{{worker_inboxes}}", &context.worker_inboxes)
        .replace("{{my_branch}}", &context.my_branch)
        .replace("{{other_branches}}", &context.other_branches)
        .replace("{{worktree_root}}", &context.worktree_root);

    for (var, value) in &context.worker_instance_vars {
        rendered = rendered.replace(var, value);
    }

    rendered
}

fn render_prompt_template(
    raw: String,
    context: &PromptTemplateContext,
    worktree_prompt: &str,
) -> String {
    let with_worktree = if raw.contains("{{worktree_prompt}}") {
        raw.replace("{{worktree_prompt}}", worktree_prompt)
    } else if !worktree_prompt.is_empty() {
        format!("{}\n\n{}", raw, worktree_prompt)
    } else {
        raw
    };

    apply_prompt_template_variables(with_worktree, context)
}

impl ProjectConfig {
    fn prompt_project_root(&self) -> String {
        if self.worktree_feature.is_some() {
            ".".to_string()
        } else {
            self.project_root.display().to_string()
        }
    }

    fn prompt_messages_dir(&self) -> String {
        if self.worktree_feature.is_some() {
            ".orchestrator/messages".to_string()
        } else {
            self.messages_dir.display().to_string()
        }
    }

    fn prompt_worktree_root(&self, worktree: Option<&crate::worktree::AgentWorktree>) -> String {
        match (self.worktree_feature.is_some(), worktree) {
            (true, Some(_)) => ".".to_string(),
            (_, Some(wt)) => wt.worktree_path.display().to_string(),
            _ => String::new(),
        }
    }

    fn worker_inbox_template_vars(&self) -> (String, Vec<(String, String)>) {
        let prompt_messages_dir = self.prompt_messages_dir();
        let mut worker_inboxes_all = Vec::new();
        let mut worker_instance_vars = Vec::new();

        for group in &self.worker_groups {
            for instance in 1..=group.count {
                let mut instance_lines = Vec::new();
                for agent_id in &group.agents {
                    let expanded = expand_agent_id(agent_id, instance, group.count);
                    instance_lines.push(format!("- {}/to_{}/", prompt_messages_dir, expanded));
                }
                let block = instance_lines.join("\n");
                worker_inboxes_all.push(block.clone());
                worker_instance_vars.push((format!("{{{{worker_{}_inboxes}}}}", instance), block));
            }
        }

        (worker_inboxes_all.join("\n"), worker_instance_vars)
    }

    fn prompt_template_context(
        &self,
        agent_id: &str,
        instance: u32,
        group_count: u32,
        peer_ids: Vec<String>,
        peer_inboxes: Vec<String>,
        worker_inboxes: &str,
        worker_instance_vars: &[(String, String)],
        worktree: Option<&crate::worktree::AgentWorktree>,
    ) -> PromptTemplateContext {
        let instance_suffix = if group_count == 1 {
            String::new()
        } else {
            format!("-{}", instance)
        };

        let other_branches = if self.worktree_feature.is_some() {
            crate::worktree::format_other_branches(&self.worktrees, agent_id)
        } else {
            String::new()
        };

        PromptTemplateContext {
            project_root: self.prompt_project_root(),
            messages_dir: self.prompt_messages_dir(),
            agent_id: agent_id.to_string(),
            instance_suffix,
            instance_index: instance.to_string(),
            group_count: group_count.to_string(),
            peer_ids: peer_ids.join(", "),
            peer_inboxes: peer_inboxes.join("\n"),
            worker_inboxes: worker_inboxes.to_string(),
            worker_instance_vars: worker_instance_vars.to_vec(),
            my_branch: worktree.map(|wt| wt.branch.clone()).unwrap_or_default(),
            other_branches,
            worktree_root: self.prompt_worktree_root(worktree),
        }
    }

    fn load_worktree_prompt(
        &self,
        agent: &AgentEntry,
        context: &PromptTemplateContext,
    ) -> Result<String, ConfigError> {
        match &agent.worktree_prompt_file {
            Some(file) => {
                let path = self.dot_dir.join(file);
                if !path.exists() {
                    return Err(ConfigError::MissingPromptFile(path));
                }
                let raw = std::fs::read_to_string(&path)?;
                Ok(apply_prompt_template_variables(raw, context))
            }
            None => Ok(String::new()),
        }
    }

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

        // Validate worker groups
        let all_ids: Vec<&str> = agents_toml.agents.iter().map(|a| a.id.as_str()).collect();
        for group in &agents_toml.worker_groups {
            if group.agents.is_empty() {
                return Err(ConfigError::InvalidWorkerGroup(format!(
                    "worker_group '{}' has no agents listed",
                    group.id
                )));
            }
            if group.count == 0 {
                return Err(ConfigError::InvalidWorkerGroup(format!(
                    "worker_group '{}' has count = 0; set count >= 1",
                    group.id
                )));
            }
            for agent_ref in &group.agents {
                if !all_ids.contains(&agent_ref.as_str()) {
                    return Err(ConfigError::InvalidWorkerGroup(format!(
                        "worker_group '{}' references agent '{}' which is not defined in [[agents]]",
                        group.id, agent_ref
                    )));
                }
            }
        }

        // Validate that no agent appears in more than one worker group
        {
            let mut seen = std::collections::HashSet::new();
            for group in &agents_toml.worker_groups {
                for agent_ref in &group.agents {
                    if !seen.insert(agent_ref.as_str()) {
                        return Err(ConfigError::InvalidWorkerGroup(format!(
                            "agent '{}' appears in multiple worker groups; \
                             each agent may belong to at most one group",
                            agent_ref
                        )));
                    }
                }
            }
        }

        // Validate timer entries
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
            worker_groups: agents_toml.worker_groups,
            worktree_feature: None,
            worktrees: Vec::new(),
        })
    }

    /// Create all required directories under `.orchestrator/`.
    pub fn ensure_dirs(&self) -> Result<(), std::io::Error> {
        // Standalone agents and template agents not referenced in any group
        let grouped_ids: std::collections::HashSet<&str> = self
            .worker_groups
            .iter()
            .flat_map(|g| g.agents.iter().map(|a| a.as_str()))
            .collect();

        for agent in &self.agents {
            if !grouped_ids.contains(agent.id.as_str()) {
                std::fs::create_dir_all(self.messages_dir.join(format!("to_{}", agent.id)))?;
            }
        }

        // Expanded group agent IDs
        for group in &self.worker_groups {
            for instance in 1..=group.count {
                for agent_id in &group.agents {
                    let expanded_id = expand_agent_id(agent_id, instance, group.count);
                    std::fs::create_dir_all(
                        self.messages_dir.join(format!("to_{}", expanded_id)),
                    )?;
                }
            }
        }

        std::fs::create_dir_all(self.messages_dir.join("processed"))?;
        std::fs::create_dir_all(self.messages_dir.join("dead_letter"))?;
        std::fs::create_dir_all(&self.log_dir)?;
        std::fs::create_dir_all(&self.transcript_dir)?;
        std::fs::create_dir_all(self.dot_dir.join("runtime/pids"))?;
        Ok(())
    }

    /// Convert agent entries into supervisor AgentConfig structs.
    ///
    /// Standalone agents (not referenced in any worker_group) get their own session.
    /// Grouped agents are expanded per-instance with suffixed IDs when count > 1,
    /// and their tmux_target points at a specific pane within the shared session.
    pub fn agent_configs(&self) -> Vec<AgentConfig> {
        let grouped_ids: std::collections::HashSet<&str> = self
            .worker_groups
            .iter()
            .flat_map(|g| g.agents.iter().map(|a| a.as_str()))
            .collect();

        // Build worktree lookup: agent_id -> worktree info
        let wt_map: HashMap<&str, &crate::worktree::AgentWorktree> = self
            .worktrees
            .iter()
            .map(|wt| (wt.agent_id.as_str(), wt))
            .collect();

        let mut configs = Vec::new();

        // Standalone agents
        for a in &self.agents {
            if grouped_ids.contains(a.id.as_str()) {
                continue;
            }
            let session = self.tmux_session_for(&a.id);
            let working_dir = wt_map.get(a.id.as_str()).map(|wt| wt.worktree_path.clone());
            // When in worktree mode, resolve allowed_write_dirs relative to the
            // worktree root instead of the main project root.
            let base_root = working_dir.as_deref().unwrap_or(&self.project_root);
            configs.push(AgentConfig {
                agent_id: a.id.clone(),
                cli_command: a.command.clone(),
                tmux_session: session.clone(),
                tmux_target: session,
                inbox_dir: self.messages_dir.join(format!("to_{}", a.id)),
                allowed_write_dirs: a
                    .allowed_write_dirs
                    .iter()
                    .map(|d| base_root.join(d))
                    .collect(),
                working_dir,
            });
        }

        // Grouped agents — expanded per instance
        let agent_map: HashMap<&str, &AgentEntry> =
            self.agents.iter().map(|a| (a.id.as_str(), a)).collect();

        for group in &self.worker_groups {
            for instance in 1..=group.count {
                let session = self.group_session_for(&group.id, instance, group.count);
                for (pane_idx, agent_id) in group.agents.iter().enumerate() {
                    let a = match agent_map.get(agent_id.as_str()) {
                        Some(a) => a,
                        None => continue,
                    };
                    let expanded_id = expand_agent_id(agent_id, instance, group.count);
                    let tmux_target = format!("{}:0.{}", session, pane_idx);
                    // For grouped agents, look up worktree by expanded ID first,
                    // then fall back to base agent ID.
                    let working_dir = wt_map
                        .get(expanded_id.as_str())
                        .or_else(|| wt_map.get(agent_id.as_str()))
                        .map(|wt| wt.worktree_path.clone());
                    let base_root = working_dir.as_deref().unwrap_or(&self.project_root);
                    configs.push(AgentConfig {
                        agent_id: expanded_id.clone(),
                        cli_command: a.command.clone(),
                        tmux_session: session.clone(),
                        tmux_target,
                        inbox_dir: self.messages_dir.join(format!("to_{}", expanded_id)),
                        allowed_write_dirs: a
                            .allowed_write_dirs
                            .iter()
                            .map(|d| base_root.join(d))
                            .collect(),
                        working_dir,
                    });
                }
            }
        }

        configs
    }

    /// Build the list of WorkerGroupConfigs for the supervisor to spawn as
    /// grouped tmux sessions.  Each config describes one session instance.
    pub fn worker_group_configs(&self) -> Vec<WorkerGroupConfig> {
        let agent_map: HashMap<&str, &AgentEntry> =
            self.agents.iter().map(|a| (a.id.as_str(), a)).collect();

        // Build worktree lookup
        let wt_map: HashMap<&str, &crate::worktree::AgentWorktree> = self
            .worktrees
            .iter()
            .map(|wt| (wt.agent_id.as_str(), wt))
            .collect();

        let mut groups = Vec::new();
        for group in &self.worker_groups {
            for instance in 1..=group.count {
                let session = self.group_session_for(&group.id, instance, group.count);
                let mut members = Vec::new();
                for (pane_idx, agent_id) in group.agents.iter().enumerate() {
                    let a = match agent_map.get(agent_id.as_str()) {
                        Some(a) => a,
                        None => continue,
                    };
                    let expanded_id = expand_agent_id(agent_id, instance, group.count);
                    let tmux_target = format!("{}:0.{}", session, pane_idx);
                    let working_dir = wt_map
                        .get(expanded_id.as_str())
                        .or_else(|| wt_map.get(agent_id.as_str()))
                        .map(|wt| wt.worktree_path.clone());
                    let base_root = working_dir.as_deref().unwrap_or(&self.project_root);
                    members.push(AgentConfig {
                        agent_id: expanded_id.clone(),
                        cli_command: a.command.clone(),
                        tmux_session: session.clone(),
                        tmux_target,
                        inbox_dir: self.messages_dir.join(format!("to_{}", expanded_id)),
                        allowed_write_dirs: a
                            .allowed_write_dirs
                            .iter()
                            .map(|d| base_root.join(d))
                            .collect(),
                        working_dir,
                    });
                }
                groups.push(WorkerGroupConfig {
                    group_id: group.id.clone(),
                    session_name: session,
                    layout: group.layout.clone(),
                    members,
                });
            }
        }
        groups
    }

    /// Read and render startup prompt files, substituting template variables.
    ///
    /// For grouped agents, each instance gets its own rendered prompt with the
    /// expanded agent_id (e.g. `coder-1`, `coder-2`) substituted.
    pub fn startup_prompts(&self) -> Result<HashMap<String, String>, ConfigError> {
        let mut prompts = HashMap::new();

        let grouped_ids: std::collections::HashSet<&str> = self
            .worker_groups
            .iter()
            .flat_map(|g| g.agents.iter().map(|a| a.as_str()))
            .collect();
        let (worker_inboxes_rendered, worker_instance_vars) = self.worker_inbox_template_vars();

        // Build worktree lookup for branch info
        let wt_map: HashMap<&str, &crate::worktree::AgentWorktree> = self
            .worktrees
            .iter()
            .map(|wt| (wt.agent_id.as_str(), wt))
            .collect();

        // Standalone agents
        for agent in &self.agents {
            if grouped_ids.contains(agent.id.as_str()) {
                continue;
            }
            let prompt_path = self.dot_dir.join(&agent.prompt_file);
            if !prompt_path.exists() {
                return Err(ConfigError::MissingPromptFile(prompt_path));
            }
            let raw = std::fs::read_to_string(&prompt_path)?;
            let context = self.prompt_template_context(
                &agent.id,
                1,
                1,
                Vec::new(),
                Vec::new(),
                &worker_inboxes_rendered,
                &worker_instance_vars,
                wt_map.get(agent.id.as_str()).copied(),
            );

            let worktree_prompt = if self.worktree_feature.is_some() {
                self.load_worktree_prompt(agent, &context)?
            } else {
                String::new()
            };

            let rendered = render_prompt_template(raw, &context, &worktree_prompt);

            prompts.insert(agent.id.clone(), rendered);
        }

        // Grouped agents — one rendered prompt per instance
        let agent_map: HashMap<&str, &AgentEntry> =
            self.agents.iter().map(|a| (a.id.as_str(), a)).collect();

        for group in &self.worker_groups {
            for instance in 1..=group.count {
                // Build peer inbox list for this instance (all group members
                // except the current agent being rendered).
                for agent_id in &group.agents {
                    let a = match agent_map.get(agent_id.as_str()) {
                        Some(a) => a,
                        None => continue,
                    };
                    let prompt_path = self.dot_dir.join(&a.prompt_file);
                    if !prompt_path.exists() {
                        return Err(ConfigError::MissingPromptFile(prompt_path));
                    }
                    let raw = std::fs::read_to_string(&prompt_path)?;
                    let expanded_id = expand_agent_id(agent_id, instance, group.count);

                    let worktree = wt_map
                        .get(expanded_id.as_str())
                        .or_else(|| wt_map.get(agent_id.as_str()))
                        .copied();

                    // Render peer inboxes: every other agent in this group instance
                    let peer_inboxes: Vec<String> = group
                        .agents
                        .iter()
                        .filter(|peer| peer.as_str() != agent_id.as_str())
                        .map(|peer| {
                            let peer_expanded = expand_agent_id(peer, instance, group.count);
                            format!(
                                "- {}/to_{}/",
                                self.prompt_messages_dir(),
                                peer_expanded
                            )
                        })
                        .collect();

                    // Render peer IDs: every other agent in this group instance
                    let peer_ids: Vec<String> = group
                        .agents
                        .iter()
                        .filter(|peer| peer.as_str() != agent_id.as_str())
                        .map(|peer| expand_agent_id(peer, instance, group.count))
                        .collect();

                    let context = self.prompt_template_context(
                        &expanded_id,
                        instance,
                        group.count,
                        peer_ids,
                        peer_inboxes,
                        &worker_inboxes_rendered,
                        &worker_instance_vars,
                        worktree,
                    );

                    let worktree_prompt = if self.worktree_feature.is_some() {
                        self.load_worktree_prompt(a, &context)?
                    } else {
                        String::new()
                    };

                    let rendered = render_prompt_template(raw, &context, &worktree_prompt);

                    prompts.insert(expanded_id, rendered);
                }
            }
        }

        Ok(prompts)
    }

    /// Build resolved timer configs for all agents.
    /// Stores paths and template variables so prompts are read fresh at fire time.
    pub fn resolved_timers(&self) -> Result<Vec<ResolvedTimer>, ConfigError> {
        let mut timers = Vec::new();
        let grouped_ids: std::collections::HashSet<&str> = self
            .worker_groups
            .iter()
            .flat_map(|g| g.agents.iter().map(|a| a.as_str()))
            .collect();
        let agent_map: HashMap<&str, &AgentEntry> =
            self.agents.iter().map(|a| (a.id.as_str(), a)).collect();
        let wt_map: HashMap<&str, &crate::worktree::AgentWorktree> = self
            .worktrees
            .iter()
            .map(|wt| (wt.agent_id.as_str(), wt))
            .collect();
        let (worker_inboxes_rendered, worker_instance_vars) = self.worker_inbox_template_vars();

        for agent in &self.agents {
            if grouped_ids.contains(agent.id.as_str()) {
                continue;
            }

            for timer in &agent.timers {
                let prompt_path = self.dot_dir.join(&timer.prompt_file);
                if !prompt_path.exists() {
                    return Err(ConfigError::MissingPromptFile(prompt_path));
                }

                let context = self.prompt_template_context(
                    &agent.id,
                    1,
                    1,
                    Vec::new(),
                    Vec::new(),
                    &worker_inboxes_rendered,
                    &worker_instance_vars,
                    wt_map.get(agent.id.as_str()).copied(),
                );
                let worktree_prompt_path = agent
                    .worktree_prompt_file
                    .as_ref()
                    .map(|file| self.dot_dir.join(file));
                if let Some(path) = &worktree_prompt_path {
                    if !path.exists() {
                        return Err(ConfigError::MissingPromptFile(path.clone()));
                    }
                }

                timers.push(ResolvedTimer {
                    agent_id: agent.id.clone(),
                    minutes: timer.minutes,
                    prompt_path,
                    interrupt: timer.interrupt,
                    include_agents: timer.include_agents.clone(),
                    template_context: context,
                    worktree_prompt_path,
                });
            }
        }

        for group in &self.worker_groups {
            for instance in 1..=group.count {
                for agent_id in &group.agents {
                    let agent = match agent_map.get(agent_id.as_str()) {
                        Some(agent) => *agent,
                        None => continue,
                    };
                    let expanded_id = expand_agent_id(agent_id, instance, group.count);
                    let peer_inboxes: Vec<String> = group
                        .agents
                        .iter()
                        .filter(|peer| peer.as_str() != agent_id.as_str())
                        .map(|peer| {
                            let peer_expanded = expand_agent_id(peer, instance, group.count);
                            format!("- {}/to_{}/", self.prompt_messages_dir(), peer_expanded)
                        })
                        .collect();
                    let peer_ids: Vec<String> = group
                        .agents
                        .iter()
                        .filter(|peer| peer.as_str() != agent_id.as_str())
                        .map(|peer| expand_agent_id(peer, instance, group.count))
                        .collect();
                    let context = self.prompt_template_context(
                        &expanded_id,
                        instance,
                        group.count,
                        peer_ids,
                        peer_inboxes,
                        &worker_inboxes_rendered,
                        &worker_instance_vars,
                        wt_map
                            .get(expanded_id.as_str())
                            .or_else(|| wt_map.get(agent_id.as_str()))
                            .copied(),
                    );
                    let worktree_prompt_path = agent
                        .worktree_prompt_file
                        .as_ref()
                        .map(|file| self.dot_dir.join(file));
                    if let Some(path) = &worktree_prompt_path {
                        if !path.exists() {
                            return Err(ConfigError::MissingPromptFile(path.clone()));
                        }
                    }

                    for timer in &agent.timers {
                        let prompt_path = self.dot_dir.join(&timer.prompt_file);
                        if !prompt_path.exists() {
                            return Err(ConfigError::MissingPromptFile(prompt_path));
                        }

                        timers.push(ResolvedTimer {
                            agent_id: expanded_id.clone(),
                            minutes: timer.minutes,
                            prompt_path,
                            interrupt: timer.interrupt,
                            include_agents: timer.include_agents.clone(),
                            template_context: context.clone(),
                            worktree_prompt_path: worktree_prompt_path.clone(),
                        });
                    }
                }
            }
        }

        Ok(timers)
    }

    /// Derive the tmux session name for a standalone agent.
    /// When worktree mode is active, the feature name is included:
    /// `<project>-<feature>-<agent>` instead of `<project>-<agent>`.
    pub fn tmux_session_for(&self, agent_id: &str) -> String {
        match &self.worktree_feature {
            Some(feature) => format!(
                "{}-{}-{}",
                self.project_name,
                sanitize_tmux_component(feature),
                agent_id
            ),
            None => format!("{}-{}", self.project_name, agent_id),
        }
    }

    /// Derive the tmux session name for a worker group instance.
    /// When count == 1, no numeric suffix is appended.
    /// When worktree mode is active, the feature name is included.
    pub fn group_session_for(&self, group_id: &str, instance: u32, total: u32) -> String {
        let base = match &self.worktree_feature {
            Some(feature) => format!("{}-{}", self.project_name, sanitize_tmux_component(feature)),
            None => self.project_name.clone(),
        };
        if total == 1 {
            format!("{}-{}", base, group_id)
        } else {
            format!("{}-{}-{}", base, group_id, instance)
        }
    }
}

/// A fully resolved timer ready for the timer loop.
/// Prompt file is read fresh each time the timer fires.
#[derive(Debug, Clone)]
pub struct ResolvedTimer {
    pub agent_id: String,
    pub minutes: u64,
    pub prompt_path: PathBuf,
    pub interrupt: bool,
    pub include_agents: Vec<String>,
    template_context: PromptTemplateContext,
    worktree_prompt_path: Option<PathBuf>,
}

impl ResolvedTimer {
    pub fn new_basic(
        agent_id: String,
        minutes: u64,
        prompt_path: PathBuf,
        project_root: String,
        messages_dir: String,
        interrupt: bool,
        include_agents: Vec<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.clone(),
            minutes,
            prompt_path,
            interrupt,
            include_agents,
            template_context: PromptTemplateContext {
                project_root,
                messages_dir,
                agent_id,
                instance_suffix: String::new(),
                instance_index: "1".to_string(),
                group_count: "1".to_string(),
                peer_ids: String::new(),
                peer_inboxes: String::new(),
                worker_inboxes: String::new(),
                worker_instance_vars: Vec::new(),
                my_branch: String::new(),
                other_branches: String::new(),
                worktree_root: String::new(),
            },
            worktree_prompt_path: None,
        }
    }

    /// Read and render the prompt file. Called each time the timer fires
    /// so edits to the file take effect without an orchestrator restart.
    pub fn read_prompt(&self) -> Result<String, std::io::Error> {
        let raw = std::fs::read_to_string(&self.prompt_path)?;
        let worktree_prompt = match &self.worktree_prompt_path {
            Some(path) => {
                let raw_worktree_prompt = std::fs::read_to_string(path)?;
                apply_prompt_template_variables(raw_worktree_prompt, &self.template_context)
            }
            None => String::new(),
        };
        Ok(render_prompt_template(
            raw,
            &self.template_context,
            &worktree_prompt,
        ))
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

    sanitize_tmux_component(raw)
}

fn sanitize_tmux_component(raw: &str) -> String {
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

/// Expand an agent ID for a specific group instance.
/// When total == 1, the ID is unchanged (no numeric suffix).
/// When total > 1, appends `-{instance}` (e.g. `coder-1`, `coder-2`).
pub fn expand_agent_id(agent_id: &str, instance: u32, total: u32) -> String {
    if total == 1 {
        agent_id.to_string()
    } else {
        format!("{}-{}", agent_id, instance)
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
    std::fs::write(
        prompts_dir.join("reviewer_status_check.md"),
        DEFAULT_REVIEWER_STATUS_CHECK_PROMPT,
    )?;

    // Write worktree-specific prompt files
    std::fs::write(prompts_dir.join("coder-worktree.md"), DEFAULT_CODER_WORKTREE_PROMPT)?;
    std::fs::write(prompts_dir.join("tester-worktree.md"), DEFAULT_TESTER_WORKTREE_PROMPT)?;
    std::fs::write(prompts_dir.join("reviewer-worktree.md"), DEFAULT_REVIEWER_WORKTREE_PROMPT)?;

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
#
# Worker groups
# =============
# [[worker_groups]] defines named sets of agents that always launch together
# in the same tmux session, shown side-by-side in a split pane layout.
#
#   id      – name of the group (used in the tmux session name)
#   agents  – ordered list of agent IDs to place in the session
#   layout  – "horizontal" (left|right split, default) or "vertical" (top|bottom)
#   count   – how many parallel instances of this group to launch (default: 1)
#
# Example: request 2 parallel coder+tester pairs
#
#   [[worker_groups]]
#   id      = "worker"
#   agents  = ["coder", "tester"]
#   layout  = "horizontal"
#   count   = 2
#
# With count = 2 this creates sessions <project>-worker-1 and <project>-worker-2,
# each containing a coder pane and a tester pane side-by-side.
# Agent IDs become coder-1/coder-2/tester-1/tester-2 with matching inboxes.
# With count = 1 (default) the session is named <project>-worker and IDs are unchanged.

[[agents]]
id = "coder"
command = "claude"
prompt_file = "prompts/coder.md"
worktree_prompt_file = "prompts/coder-worktree.md"
allowed_write_dirs = ["src/"]

[[agents]]
id = "tester"
command = "codex"
prompt_file = "prompts/tester.md"
worktree_prompt_file = "prompts/tester-worktree.md"
allowed_write_dirs = ["tests/"]

[[agents]]
id = "reviewer"
command = "copilot"
prompt_file = "prompts/reviewer.md"
worktree_prompt_file = "prompts/reviewer-worktree.md"
allowed_write_dirs = ["review/"]

# Re-inject the full reviewer prompt every 30 minutes to prevent context drift
[[agents.timers]]
minutes = 30
prompt_file = "prompts/reviewer.md"
interrupt = false

# Every 5 minutes, inject agent statuses and ask if there's work to assign
[[agents.timers]]
minutes = 5
prompt_file = "prompts/reviewer_status_check.md"
interrupt = false
include_agents = ["coder", "tester"]

# Worker group: coder + tester always launch together side-by-side.
# Set count = 2 to run two parallel coder+tester pairs.
[[worker_groups]]
id = "worker"
agents = ["coder", "tester"]
layout = "horizontal"
count = 1
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

Valid TOML files parse successfully and all fields are populated.
Missing required fields return `ConfigError::MissingField`.
Malformed TOML returns `ConfigError::ParseError`.

- The path argument handles both absolute and relative paths.

=== HOW TO SEND MESSAGES ===

Write a file to the recipient's inbox directory. Use this naming convention:
<timestamp>**from-{{agent_id}}**to-<recipient>\_\_topic-<topic>.md

Inbox directories:

{{peer_inboxes}}
- {{messages_dir}}/to_reviewer/

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

{{worktree_prompt}}
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
<timestamp>**from-{{agent_id}}**to-<recipient>\_\_topic-<topic>.md

Inbox directories:

{{peer_inboxes}}
- {{messages_dir}}/to_reviewer/

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

{{worktree_prompt}}
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
2. Consider the requirements and context provided in the messages.
3. Make a clear decision.
4. Send your decision to BOTH the coder and the tester so they can proceed.

Be direct and specific. Don't just say "the coder is right" — explain why and
what the tester should change (or vice versa).

=== HOW TO SEND MESSAGES ===

Write a file to the recipient's inbox directory. Use this naming convention:
<timestamp>**from-{{agent_id}}**to-<recipient>\_\_topic-<topic>.md

Inbox directories:

{{worker_inboxes}}

Always use the exact agent ID from the incoming message's FROM field to
construct the correct inbox path. When resolving a dispute, send your
decision to BOTH agents.

=== RESTARTING AGENTS (FRESH CONTEXT) ===

You can restart any agent with a clean slate by writing a message with the
special topic `_RESTART`. The orchestrator will kill the agent's session,
respawn it, and re-inject its original startup prompt — giving it a completely
fresh context window.

To restart an agent, write a file with topic-_RESTART to its inbox:
<timestamp>__from-{{agent_id}}__to-<recipient>__topic-_RESTART.md

The file content can be empty or contain a brief reason for the restart.

Examples (use the exact agent ID from the inbox directories above):
  <timestamp>__from-{{agent_id}}__to-<agent-id>__topic-_RESTART.md

Write to the agent's inbox directory listed above.

WHEN TO RESTART: After a task has been completed successfully and has been
fully accepted — once the coder has finished implementation, the tester has
confirmed tests pass, and the reviewer has accepted all changes — restart both
agents preemptively. This clears their context windows so they start the next
task fresh, without accumulated context from the previous task polluting their
reasoning. Do not wait to be asked; restart them as soon as a task is fully
done. You SHOULD ALWAYS ask the agents if they are complete and wait for a
response before restarting them. Demand that they respond to you.

=== INTERRUPTING AGENTS (URGENT MESSAGES) ===

You can interrupt an agent's current work by writing a message with the
special topic `_INTERRUPT`. The orchestrator will:

1. Cancel the agent's current generation (Ctrl+C or equivalent).
2. Flush any queued pending messages.
3. Deliver your interrupt message immediately.

To interrupt an agent, use topic-_INTERRUPT in the filename:
<timestamp>__from-{{agent_id}}__to-<recipient>__topic-_INTERRUPT.md

The file content should contain the new instructions you want the agent
to act on immediately.

WHEN TO INTERRUPT:
- An agent is working on something that is no longer needed (e.g., requirements changed).
- You need an agent to drop what it's doing and handle something urgent.
- An agent appears stuck in a loop or producing incorrect output.


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
request, not proactively. All context you need will be provided in the messages
you receive.

{{worktree_prompt}}
"#;

const DEFAULT_REVIEWER_STATUS_CHECK_PROMPT: &str = r#"=== AGENT STATUS CHECK ===

Below is the current status of the other agents in the system. Review their
activity states and consider:

1. Are any agents IDLE with no assigned work? If so, do you have tasks to
   assign them?
2. Are any agents BUSY on something that should be reprioritized?
3. Has any agent been idle for too long, suggesting it may need a restart?

If you have work to assign to an idle agent, send them a message now with
clear instructions. If all agents are productively busy, no action is needed.
"#;

const DEFAULT_CODER_WORKTREE_PROMPT: &str = r#"
=== WORKTREE MODE — GIT WORKFLOW ===

You are working in a git worktree with your own dedicated branch.

YOUR BRANCH: {{my_branch}}
YOUR WORKTREE: {{worktree_root}}

Other agents and their branches:
{{other_branches}}

CRITICAL RULES FOR WORKTREE MODE:

1. COMMIT YOUR WORK. You are on your own branch. You MUST `git add` and
   `git commit` your changes frequently — at minimum after every logical
   unit of work. Uncommitted code is invisible to the reviewer and other
   agents. Small, frequent commits are better than one large commit.

2. INCLUDE YOUR BRANCH IN EVERY MESSAGE. When you send a message to any
   other agent, always include the line:
     BRANCH: {{my_branch}}
   This tells the recipient where to find your code. Without this, they
   cannot review or test your work.

3. MERGE THE REVIEWER'S BRANCH BEFORE STARTING. Before you begin any new
   work, pull in the latest approved code from the reviewer's branch:
     git merge <reviewer-branch> --no-edit
   If there are merge conflicts, resolve them, commit, and then proceed.
   The reviewer's branch contains the accepted, tested codebase. Starting
   from it ensures you are building on approved work, not stale code.

4. WHEN TO MERGE AGAIN. Any time the reviewer tells you they have approved
   and committed new work, merge their branch again before continuing.

=== WORKFLOW SUMMARY ===

  START:   git merge <reviewer-branch> --no-edit
  WORK:    write code → git add -A → git commit -m "description"
  NOTIFY:  send message including BRANCH: {{my_branch}}
  REPEAT:  merge reviewer branch when told there are new approvals
"#;

const DEFAULT_TESTER_WORKTREE_PROMPT: &str = r#"
=== WORKTREE MODE — GIT WORKFLOW ===

You are working in a git worktree shared with your coder partner.

YOUR BRANCH: {{my_branch}}
YOUR WORKTREE: {{worktree_root}}

Other agents and their branches:
{{other_branches}}

CRITICAL RULES FOR WORKTREE MODE:

1. COMMIT YOUR WORK. You MUST `git add` and `git commit` your test files
   after writing or updating them. Uncommitted tests are invisible to the
   reviewer.

2. INCLUDE YOUR BRANCH IN EVERY MESSAGE. When you send a message to any
   other agent, always include the line:
     BRANCH: {{my_branch}}
   This tells the recipient where to find your tests.

3. YOU DO NOT NEED TO MERGE. Your coder partner shares the same branch
   and worktree. Their implementation code is already available to you.
   The coder is responsible for merging the reviewer's approved code into
   your shared branch when needed.

=== WORKFLOW SUMMARY ===

  WORK:    write tests → run tests → git add -A → git commit -m "description"
  NOTIFY:  send message including BRANCH: {{my_branch}}
"#;

const DEFAULT_REVIEWER_WORKTREE_PROMPT: &str = r#"
=== WORKTREE MODE — GIT WORKFLOW (REVIEWER) ===

You are working in a git worktree with your own dedicated branch. Your branch
is the source of truth — it contains the accepted, approved codebase. Workers
merge YOUR branch to get the latest approved code before they start working.

YOUR BRANCH: {{my_branch}}
YOUR WORKTREE: {{worktree_root}}

Other agents and their branches:
{{other_branches}}

=== HOW TO REVIEW CODE IN WORKTREE MODE ===

When a worker (coder or tester) tells you their work is ready for review,
they will include their branch name. Use this workflow:

1. MERGE WITHOUT COMMITTING to inspect their changes:
     git merge --no-commit --no-ff <worker-branch>

   This stages their changes in your working tree without creating a commit,
   so you can inspect, build, and test before deciding.

2. REVIEW the merged code:
   - Read the changed files.
   - Run the build: does it compile?
   - Run the tests: do they pass?
   - Check for correctness, edge cases, and code quality.

3. DECIDE:

   IF APPROVED — commit the merge to accept it into the canonical branch:
     git commit -m "Merge <worker-branch>: <brief description of what was accepted>"

   IF REJECTED — abort the merge and discard the changes:
     git merge --abort
   Then send the worker a message explaining what needs to be fixed.

=== CRITICAL RULES ===

1. ALWAYS MERGE WITHOUT COMMITTING FIRST. Never do a bare `git merge` that
   auto-commits. You must inspect and test before accepting.

2. CONFIRM WORKERS HAVE COMMITTED. Before you try to merge a worker's
   branch, verify they have actually committed their code. If their message
   says "I've made changes" but doesn't mention committing, reply asking
   them to `git add` and `git commit` first. You cannot merge uncommitted
   work.

3. MERGE ALL WORKER BRANCHES BEFORE RESTARTING AN AGENT. Before you send
   a _RESTART to any worker, make sure you have already merged (or
   explicitly rejected) all of their committed work. A restart gives the
   agent a blank slate — any unmerged committed code on their branch is
   still there, but the agent will lose context about what it was working
   on. If you restart a worker without merging their work, you risk
   orphaning completed code.

4. TELL WORKERS WHEN YOU APPROVE. After you commit a merge, send a message
   to all workers telling them you have new approved code on your branch.
   They need to know so they can merge your branch and build on the latest
   accepted state. Include:
     BRANCH: {{my_branch}}
     STATUS: Approved and merged. Please `git merge {{my_branch}}`
             before continuing your work.

5. YOUR BRANCH IS CANONICAL. Workers merge YOUR branch to start from a
   known-good state. Never force-push or rewrite history on your branch.

=== REVIEW WORKFLOW SUMMARY ===

  RECEIVE:  worker says "ready for review" with their BRANCH name
  MERGE:    git merge --no-commit --no-ff <branch>
  TEST:     build + run tests
  APPROVE:  git commit -m "Merge <branch>: ..."  → notify all workers
  REJECT:   git merge --abort → send feedback with required fixes
"#;

