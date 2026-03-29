/// Agent session management — per-vendor adapters and orchestrator-level
/// session records that allow all agents to be resumed by a single name.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Per-agent session adapter
// ---------------------------------------------------------------------------

/// Encapsulates everything that differs between CLI tools when managing
/// session lifecycle: how to name a session at spawn time, how to build a
/// resume command, and how to locate the vendor session ID after startup.
pub trait SessionAdapter: Send + Sync {
    /// Extra CLI arguments to append to the spawn command.
    /// For Claude this is `["--name", "<session_name>"]`; most others return
    /// an empty vec because they don't support naming at startup.
    fn spawn_extra_args(&self, session_name: &str) -> Vec<String>;

    /// Build the full CLI command string that resumes a previous session.
    /// `base_command` is the raw command from `agents.toml` (e.g. `"claude"`).
    /// `session_id` is whatever the vendor uses as the stable handle.
    fn resume_command(&self, base_command: &str, session_id: &str) -> String;

    /// Attempt to locate the vendor session ID after the agent has started.
    ///
    /// `session_name` is the tmux session name we assigned (stable across
    /// restarts; for Claude it IS the resume handle).
    /// `spawned_after` is used to filter filesystem entries — only look at
    /// session files written after this instant.
    fn extract_session_id(
        &self,
        session_name: &str,
        spawned_after: SystemTime,
    ) -> Option<String>;
}

// ---------------------------------------------------------------------------
// Claude
// ---------------------------------------------------------------------------

struct ClaudeAdapter;

impl SessionAdapter for ClaudeAdapter {
    fn spawn_extra_args(&self, session_name: &str) -> Vec<String> {
        vec!["--name".to_string(), session_name.to_string()]
    }

    fn resume_command(&self, base_command: &str, session_id: &str) -> String {
        format!("{} --resume {}", base_command, shell_quote(session_id))
    }

    fn extract_session_id(&self, session_name: &str, _spawned_after: SystemTime) -> Option<String> {
        // Claude was started with `--name <session_name>` so the name IS the
        // stable resume handle — no filesystem scan required.
        Some(session_name.to_string())
    }
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

struct GeminiAdapter;

impl SessionAdapter for GeminiAdapter {
    fn spawn_extra_args(&self, _session_name: &str) -> Vec<String> {
        vec![]
    }

    fn resume_command(&self, base_command: &str, session_id: &str) -> String {
        format!("{} --resume {}", base_command, session_id)
    }

    fn extract_session_id(&self, _session_name: &str, spawned_after: SystemTime) -> Option<String> {
        // Gemini stores sessions under ~/.gemini/tmp/<project_hash>/chats/.
        // We don't know the hash, so we walk all project dirs.
        let base = home_dir()?.join(".gemini").join("tmp");
        newest_file_id_in_tree(&base, spawned_after, extract_uuid_from_stem)
    }
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

struct CodexAdapter;

impl SessionAdapter for CodexAdapter {
    fn spawn_extra_args(&self, _session_name: &str) -> Vec<String> {
        vec![]
    }

    fn resume_command(&self, base_command: &str, session_id: &str) -> String {
        // Codex uses a subcommand, not a flag: `codex resume <id>`
        let binary = base_command.split_whitespace().next().unwrap_or("codex");
        format!("{} resume {}", binary, session_id)
    }

    fn extract_session_id(&self, _session_name: &str, spawned_after: SystemTime) -> Option<String> {
        // ~/.codex/sessions/YYYY/MM/DD/rollout-<uuid>.jsonl
        let base = home_dir()?.join(".codex").join("sessions");
        newest_file_id_in_tree(&base, spawned_after, |path| {
            let stem = path.file_stem()?.to_str()?;
            // filename: rollout-<uuid>
            stem.strip_prefix("rollout-").map(str::to_string)
        })
    }
}

// ---------------------------------------------------------------------------
// Copilot
// ---------------------------------------------------------------------------

struct CopilotAdapter;

impl SessionAdapter for CopilotAdapter {
    fn spawn_extra_args(&self, _session_name: &str) -> Vec<String> {
        vec![]
    }

    fn resume_command(&self, base_command: &str, session_id: &str) -> String {
        format!("{} --resume {}", base_command, session_id)
    }

    fn extract_session_id(&self, _session_name: &str, spawned_after: SystemTime) -> Option<String> {
        // Copilot creates a per-session directory: ~/.copilot/session-state/<id>/
        let base = home_dir()?.join(".copilot").join("session-state");
        newest_dir_id_in(&base, spawned_after)
    }
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

struct CursorAdapter;

impl SessionAdapter for CursorAdapter {
    fn spawn_extra_args(&self, _session_name: &str) -> Vec<String> {
        vec![]
    }

    fn resume_command(&self, base_command: &str, session_id: &str) -> String {
        format!("{} --resume {}", base_command, session_id)
    }

    fn extract_session_id(&self, _session_name: &str, _spawned_after: SystemTime) -> Option<String> {
        // Cursor's session storage location is not yet publicly documented.
        // The UUID is printed when a session exits; extraction from the live
        // pane will be added in a future step.
        None
    }
}

// ---------------------------------------------------------------------------
// Fallback
// ---------------------------------------------------------------------------

struct UnknownAdapter;

impl SessionAdapter for UnknownAdapter {
    fn spawn_extra_args(&self, _: &str) -> Vec<String> {
        vec![]
    }

    fn resume_command(&self, base_command: &str, session_id: &str) -> String {
        format!("{} --resume {}", base_command, session_id)
    }

    fn extract_session_id(&self, _: &str, _: SystemTime) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Return the correct adapter for the given CLI command string.
pub fn adapter_for_command(cli_command: &str) -> Box<dyn SessionAdapter> {
    match cli_command.split_whitespace().next().unwrap_or("") {
        "claude" => Box::new(ClaudeAdapter),
        "gemini" => Box::new(GeminiAdapter),
        "codex" => Box::new(CodexAdapter),
        "copilot" => Box::new(CopilotAdapter),
        "cursor" => Box::new(CursorAdapter),
        _ => Box::new(UnknownAdapter),
    }
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Recursively walk `root`, find the file with the newest mtime that was
/// written after `spawned_after`, apply `extract_id` to its path, and return
/// the result.
fn newest_file_id_in_tree(
    root: &Path,
    spawned_after: SystemTime,
    extract_id: impl Fn(&Path) -> Option<String>,
) -> Option<String> {
    let mut best: Option<(SystemTime, String)> = None;

    walk_files(root, &mut |path, mtime| {
        if mtime <= spawned_after {
            return;
        }
        let Some(id) = extract_id(path) else { return };
        let update = best.as_ref().is_none_or(|(t, _)| mtime > *t);
        if update {
            best = Some((mtime, id));
        }
    });

    best.map(|(_, id)| id)
}

/// Find the newest immediate sub-directory of `base` that was modified after
/// `spawned_after`, and return its name as the session ID.
fn newest_dir_id_in(base: &Path, spawned_after: SystemTime) -> Option<String> {
    let mut best: Option<(SystemTime, String)> = None;

    let Ok(entries) = std::fs::read_dir(base) else {
        return None;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        let Ok(mtime) = meta.modified() else { continue };
        if mtime <= spawned_after {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let update = best.as_ref().is_none_or(|(t, _)| mtime > *t);
        if update {
            best = Some((mtime, name));
        }
    }

    best.map(|(_, id)| id)
}

fn walk_files(dir: &Path, cb: &mut impl FnMut(&Path, SystemTime)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            walk_files(&path, cb);
        } else if meta.is_file() {
            if let Ok(mtime) = meta.modified() {
                cb(&path, mtime);
            }
        }
    }
}

/// Try to pull a UUID (8-4-4-4-12) out of a file's stem.
fn extract_uuid_from_stem(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    looks_like_uuid(stem).then(|| stem.to_string())
}

fn looks_like_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().filter(|&c| c == '-').count() == 4
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

// ---------------------------------------------------------------------------
// Per-agent session record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSessionInfo {
    /// The CLI tool name (e.g. `"claude"`, `"gemini"`).
    pub cli_command: String,
    /// Stable human-readable name assigned at spawn (tmux session name).
    pub session_name: String,
    /// Vendor session ID extracted after startup. `None` if extraction failed
    /// (e.g. Cursor before pane-capture extraction is implemented).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Orchestrator session (one name → all agent sessions)
// ---------------------------------------------------------------------------

/// A saved orchestrator session: a single human-readable name that maps to
/// the individual vendor session handles for every agent.
///
/// Stored at `runtime/sessions/<name>.json`.
/// Resume with: `orchestrator run --resume <name>`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorSession {
    /// The name passed to `--session` (or `--resume`).
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// agent_id → session info.
    pub agents: HashMap<String, AgentSessionInfo>,
}

impl OrchestratorSession {
    pub fn new(name: String, agents: HashMap<String, AgentSessionInfo>) -> Self {
        Self {
            name,
            created_at: Utc::now(),
            agents,
        }
    }

    /// Persist to `<sessions_dir>/<name>.json`.
    pub fn save(&self, sessions_dir: &Path) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(sessions_dir)?;
        let path = sessions_dir.join(format!("{}.json", self.name));
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Load from `<sessions_dir>/<name>.json`.
    pub fn load(sessions_dir: &Path, name: &str) -> Result<Self, std::io::Error> {
        let path = sessions_dir.join(format!("{}.json", name));
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content).map_err(std::io::Error::other)
    }
}
