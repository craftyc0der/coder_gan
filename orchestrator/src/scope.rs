use notify::{Event as NotifyEvent, EventKind, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::Arc;

use crate::logger::{Event, Logger};
use crate::supervisor::AgentConfig;

// Directories that are never interesting to watch (build artifacts, VCS, etc.)
const EXCLUDED_DIR_NAMES: &[&str] = &["target", ".git", "node_modules", ".orchestrator"];

/// Start a background thread that watches the project root for file writes
/// that fall outside every agent's `allowed_write_dirs`.
///
/// When a violation is detected a `scope_violation` event is logged.
/// This is an audit/alerting system — it does **not** block any writes.
pub fn start_scope_watcher(
    project_root: PathBuf,
    dot_dir: PathBuf,
    configs: Vec<AgentConfig>,
    logger: Arc<Logger>,
) {
    std::thread::spawn(move || {
        // Collect all allowed write dirs across every agent.
        let allowed_dirs: Vec<PathBuf> = configs
            .iter()
            .flat_map(|c| c.allowed_write_dirs.iter().cloned())
            .collect();

        let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();

        let mut watcher = match notify::recommended_watcher(move |res: Result<NotifyEvent, _>| {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Create(_)) {
                    for path in event.paths {
                        if path.is_file() {
                            let _ = tx.send(path);
                        }
                    }
                }
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[scope] failed to create filesystem watcher: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&project_root, RecursiveMode::Recursive) {
            eprintln!("[scope] failed to watch project root: {e}");
            return;
        }

        println!("[scope] watching {} for scope violations", project_root.display());

        for path in rx {
            // Skip .orchestrator/ — message queues and logs live there.
            if path.starts_with(&dot_dir) {
                continue;
            }

            // Skip excluded directories (build artifacts, VCS metadata, etc.)
            if is_in_excluded_dir(&path, &project_root) {
                continue;
            }

            // Only check files with text-file extensions agents would write.
            if !has_text_extension(&path) {
                continue;
            }

            // A file is compliant if it falls under at least one agent's allowed dir.
            let in_allowed = allowed_dirs.iter().any(|dir| path.starts_with(dir));

            if !in_allowed {
                let path_str = path.display().to_string();
                eprintln!(
                    "[scope] WARNING: file created outside all agents' allowed_write_dirs: {path_str}"
                );
                logger.log(Event::ScopeViolation {
                    path: path_str,
                    detail: "file created outside all agents' allowed_write_dirs".into(),
                });
            }
        }
    });
}

/// Returns true if any path component matches a known excluded directory name.
pub fn is_in_excluded_dir(path: &std::path::Path, project_root: &std::path::Path) -> bool {
    if let Ok(relative) = path.strip_prefix(project_root) {
        for component in relative.components() {
            if let std::path::Component::Normal(name) = component {
                if let Some(s) = name.to_str() {
                    if EXCLUDED_DIR_NAMES.contains(&s) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Returns true for file extensions that agents are likely to write.
pub fn has_text_extension(path: &std::path::Path) -> bool {
    const TEXT_EXTENSIONS: &[&str] = &[
        "rs", "toml", "md", "txt", "json", "yaml", "yml",
        "py", "js", "ts", "go", "sh", "html", "css",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| TEXT_EXTENSIONS.contains(&e))
        .unwrap_or(false)
}
