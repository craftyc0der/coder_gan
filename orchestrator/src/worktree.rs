use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum WorktreeError {
    NotGitRepo(PathBuf),
    GitCommand { step: String, detail: String },
    IoError(std::io::Error),
}

impl std::fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorktreeError::NotGitRepo(p) => {
                write!(f, "not a git repository: {}", p.display())
            }
            WorktreeError::GitCommand { step, detail } => {
                write!(f, "git {step}: {detail}")
            }
            WorktreeError::IoError(e) => write!(f, "IO error: {e}"),
        }
    }
}

impl From<std::io::Error> for WorktreeError {
    fn from(e: std::io::Error) -> Self {
        WorktreeError::IoError(e)
    }
}

// ---------------------------------------------------------------------------
// Worktree configuration
// ---------------------------------------------------------------------------

/// Runtime worktree configuration, built from CLI flags and agents.toml.
#[derive(Debug, Clone)]
pub struct WorktreeConfig {
    /// The feature/ticket name from --branch (e.g. "PR-123").
    pub feature_name: String,
}

/// Resolved worktree info for a single agent.
#[derive(Debug, Clone)]
pub struct AgentWorktree {
    pub agent_id: String,
    /// The git branch for this agent's worktree.
    pub branch: String,
    /// The absolute path to this agent's worktree directory.
    pub worktree_path: PathBuf,
}

// ---------------------------------------------------------------------------
// Worktree operations
// ---------------------------------------------------------------------------

/// Verify the project root is a git repository.
fn verify_git_repo(project_root: &Path) -> Result<(), WorktreeError> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(project_root)
        .output()
        .map_err(|e| WorktreeError::GitCommand {
            step: "rev-parse".into(),
            detail: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(WorktreeError::NotGitRepo(project_root.to_path_buf()));
    }
    Ok(())
}

/// Compute the worktree directory path for an agent.
///
/// Layout: `<project_root>-worktrees/<feature>/<agent_id>/`
///
/// Example: if project is `/home/user/myproject` and feature is `PR-123`,
/// agent `coder` gets `/home/user/myproject-worktrees/PR-123/coder/`.
pub fn worktree_path(project_root: &Path, feature_name: &str, agent_id: &str) -> PathBuf {
    let parent = project_root.parent().unwrap_or(project_root);
    let project_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    parent
        .join(format!("{}-worktrees", project_name))
        .join(feature_name)
        .join(agent_id)
}

/// Compute the default branch name for an agent.
///
/// Default: `<feature>/<agent_id>` (e.g. `PR-123/coder`).
/// If the agent has a `branch` field in agents.toml, that is used instead,
/// with `{{branch}}` substituted for the feature name.
pub fn resolve_branch(feature_name: &str, agent_id: &str, agent_branch: Option<&str>) -> String {
    match agent_branch {
        Some(pattern) => pattern.replace("{{branch}}", feature_name),
        None => format!("{}/{}", feature_name, agent_id),
    }
}

/// Create a git worktree for an agent. Creates the branch if it doesn't exist.
///
/// Runs: `git worktree add <path> -B <branch>`
/// The `-B` flag creates or resets the branch to the current HEAD.
pub fn create_worktree(
    project_root: &Path,
    wt_path: &Path,
    branch: &str,
) -> Result<(), WorktreeError> {
    // Create parent directory
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // If the worktree already exists, skip creation
    if wt_path.join(".git").exists() {
        println!(
            "[worktree] already exists: {} (branch: {})",
            wt_path.display(),
            branch
        );
        return Ok(());
    }

    let output = Command::new("git")
        .args(["worktree", "add", "-B", branch, &wt_path.display().to_string()])
        .current_dir(project_root)
        .output()
        .map_err(|e| WorktreeError::GitCommand {
            step: "worktree add".into(),
            detail: e.to_string(),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitCommand {
            step: "worktree add".into(),
            detail: format!("branch '{}', path '{}': {}", branch, wt_path.display(), stderr.trim()),
        });
    }

    println!(
        "[worktree] created: {} (branch: {})",
        wt_path.display(),
        branch
    );
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<(), WorktreeError> {
    if !path.exists() && path.symlink_metadata().is_err() {
        return Ok(());
    }

    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        std::fs::remove_file(path)?;
    } else if metadata.is_dir() {
        std::fs::remove_dir_all(path)?;
    }

    Ok(())
}

fn symlink_points_to(path: &Path, expected_target: &Path) -> bool {
    match std::fs::read_link(path) {
        Ok(target) => target == expected_target,
        Err(_) => false,
    }
}

#[cfg(unix)]
fn symlink_path(source: &Path, dest: &Path) -> Result<(), WorktreeError> {
    std::os::unix::fs::symlink(source, dest)?;
    Ok(())
}

#[cfg(not(unix))]
fn symlink_path(_source: &Path, _dest: &Path) -> Result<(), WorktreeError> {
    Err(WorktreeError::GitCommand {
        step: "symlink .orchestrator".into(),
        detail: "symlinks are not supported on this platform".into(),
    })
}

pub fn ensure_dot_orchestrator_symlink(
    source: &Path,
    wt_path: &Path,
) -> Result<(), WorktreeError> {
    if !source.exists() {
        return Ok(());
    }

    let dest = wt_path.join(".orchestrator");

    let needs_repair = match dest.symlink_metadata() {
        Ok(metadata) => !metadata.file_type().is_symlink() || !symlink_points_to(&dest, source),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(err) => return Err(WorktreeError::IoError(err)),
    };

    if needs_repair {
        remove_path_if_exists(&dest)?;
        symlink_path(source, &dest)?;
        println!(
            "[worktree] symlinked .orchestrator/ -> {}",
            source.display()
        );
    }

    Ok(())
}

/// Specification for a single worktree to create.
///
/// A worktree can be shared by multiple agents (e.g. all members of a worker
/// group instance share one worktree directory and branch).
#[derive(Debug, Clone)]
pub struct WorktreeSpec {
    /// Logical ID for the worktree (e.g. "reviewer", "worker-1").
    /// Used for directory naming and default branch naming.
    pub worktree_id: String,
    /// Agent IDs (potentially expanded) that will share this worktree.
    pub agent_ids: Vec<String>,
    /// Optional branch pattern override (supports `{{branch}}` template).
    pub branch_override: Option<String>,
}

/// Set up worktrees from specs. Returns the resolved worktree info per agent.
///
/// Each spec creates one worktree directory. All agents listed in a spec share
/// the same directory and branch. The `.orchestrator/` directory from the main
/// project is symlinked into each worktree so agents share message queues and
/// config.
pub fn setup_worktrees(
    project_root: &Path,
    feature_name: &str,
    specs: &[WorktreeSpec],
) -> Result<Vec<AgentWorktree>, WorktreeError> {
    verify_git_repo(project_root)?;

    let mut results = Vec::new();

    for spec in specs {
        let branch = resolve_branch(feature_name, &spec.worktree_id, spec.branch_override.as_deref());
        let wt_path = worktree_path(project_root, feature_name, &spec.worktree_id);

        create_worktree(project_root, &wt_path, &branch)?;

        // Git worktrees do not bring along ignored files, so link the entire
        // .orchestrator directory back to the main project.
        ensure_dot_orchestrator_symlink(&project_root.join(".orchestrator"), &wt_path)?;

        // Create an AgentWorktree entry for every agent that shares this
        // worktree, all pointing at the same path and branch.
        for agent_id in &spec.agent_ids {
            results.push(AgentWorktree {
                agent_id: agent_id.clone(),
                branch: branch.clone(),
                worktree_path: wt_path.clone(),
            });
        }
    }

    Ok(results)
}

/// Format the "other branches" string for prompt templates.
///
/// Returns a newline-separated list like:
/// ```text
/// - coder: PR-123/coder
/// - tester: PR-123/tester
/// - reviewer: PR-123/reviewer
/// ```
pub fn format_other_branches(
    worktrees: &[AgentWorktree],
    exclude_agent_id: &str,
) -> String {
    worktrees
        .iter()
        .filter(|wt| wt.agent_id != exclude_agent_id)
        .map(|wt| format!("- {}: {}", wt.agent_id, wt.branch))
        .collect::<Vec<_>>()
        .join("\n")
}
