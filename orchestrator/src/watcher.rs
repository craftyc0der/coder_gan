use notify::{Event as NotifyEvent, EventKind, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::injector::{InjectorOps, RealInjector};
use crate::logger::{Event, Logger};
use crate::supervisor::Registry;

const BACKPRESSURE_THRESHOLD: usize = 5;

// ---------------------------------------------------------------------------
// Message metadata parsed from filename
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MessageMeta {
    pub filename: String,
    pub path: PathBuf,
    pub sender: String,
    pub recipient: String,
    pub topic: String,
}

/// Parse the naming convention: `<timestamp>__from-<sender>__to-<recipient>__topic-<topic>.md`
///
/// Also handles a common variant where agents use hyphens instead of `__` as
/// field separators (e.g. `20260312-115239-from-reviewer-to-tester__topic-foo.md`).
///
/// Falls back to extracting the recipient from the parent directory name.
pub fn parse_message(path: &Path) -> Option<MessageMeta> {
    let filename = path.file_name()?.to_str()?.to_string();

    // Strip file extension for field extraction
    let stem = filename
        .trim_end_matches(".md")
        .trim_end_matches(".txt")
        .trim_end_matches(".json");

    // Try structured naming convention first (fields separated by __)
    let parts: Vec<&str> = filename.split("__").collect();
    if parts.len() >= 3 {
        let sender = parts
            .iter()
            .find(|p| p.starts_with("from-"))
            .map(|p| p.trim_start_matches("from-").to_string())
            .unwrap_or_else(|| "unknown".into());
        let recipient = parts
            .iter()
            .find(|p| p.starts_with("to-"))
            .map(|p| p.trim_start_matches("to-").to_string())
            .unwrap_or_else(|| "unknown".into());
        let topic = parts
            .iter()
            .find(|p| p.starts_with("topic-"))
            .map(|p| {
                p.trim_start_matches("topic-")
                    .trim_end_matches(".md")
                    .trim_end_matches(".txt")
                    .trim_end_matches(".json")
                    .to_string()
            })
            .unwrap_or_else(|| "general".into());

        return Some(MessageMeta {
            filename,
            path: path.to_path_buf(),
            sender,
            recipient,
            topic,
        });
    }

    // Fuzzy fallback: scan the stem for "from-" and "to-" fields even when
    // agents used hyphens instead of __ as separators.
    let sender = extract_field_fuzzy(stem, "from-", &["to-", "topic-"]);
    let recipient = extract_field_fuzzy(stem, "to-", &["from-", "topic-"]);
    let topic = extract_field_fuzzy(stem, "topic-", &["from-", "to-"]);

    if sender.is_some() || recipient.is_some() {
        return Some(MessageMeta {
            filename,
            path: path.to_path_buf(),
            sender: sender.unwrap_or_else(|| "unknown".into()),
            recipient: recipient.unwrap_or_else(|| "unknown".into()),
            topic: topic.unwrap_or_else(|| "general".into()),
        });
    }

    // Last resort: derive recipient from parent dir name (to_coder → coder)
    let parent = path.parent()?.file_name()?.to_str()?;
    if parent.starts_with("to_") {
        let recipient = parent.trim_start_matches("to_").to_string();
        return Some(MessageMeta {
            filename,
            path: path.to_path_buf(),
            sender: "unknown".into(),
            recipient,
            topic: "general".into(),
        });
    }

    None
}

/// Extract a field value from a filename stem by looking for a known prefix
/// and treating the value as everything up to the next known prefix (or end).
///
/// For example, in `20260312-from-reviewer-to-tester_1-topic-foo`:
///   extract_field_fuzzy(stem, "from-", &["to-", "topic-"]) → Some("reviewer")
///   extract_field_fuzzy(stem, "to-", &["from-", "topic-"]) → Some("tester_1")
fn extract_field_fuzzy(stem: &str, prefix: &str, stop_prefixes: &[&str]) -> Option<String> {
    // Find the last occurrence of the prefix preceded by a separator or at start.
    // We search for "-from-", "-to-", "-topic-" (with leading separator) to avoid
    // matching inside words. Also try "__from-" etc.
    let search_with_sep = format!("-{}", prefix);
    let search_with_dunder = format!("__{}", prefix);

    let value_start = stem
        .find(&search_with_sep)
        .map(|i| i + search_with_sep.len())
        .or_else(|| {
            stem.find(&search_with_dunder)
                .map(|i| i + search_with_dunder.len())
        })
        .or_else(|| {
            // Also match at start of string
            if stem.starts_with(prefix) {
                Some(prefix.len())
            } else {
                None
            }
        })?;

    let rest = &stem[value_start..];

    // Find where the value ends: at the next known field prefix (with separator)
    let mut end = rest.len();
    for stop in stop_prefixes {
        let stop_with_sep = format!("-{}", stop);
        let stop_with_dunder = format!("__{}", stop);
        if let Some(i) = rest.find(&stop_with_sep) {
            end = end.min(i);
        }
        if let Some(i) = rest.find(&stop_with_dunder) {
            end = end.min(i);
        }
    }

    let value = rest[..end].trim_matches('-').trim_matches('_');
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

// ---------------------------------------------------------------------------
// Watcher
// ---------------------------------------------------------------------------

pub struct MessageWatcher {
    registry: Registry,
    logger: Arc<Logger>,
    messages_dir: PathBuf,
    processed_dir: PathBuf,
    dead_letter_dir: PathBuf,
    seen_hashes: Arc<Mutex<HashSet<String>>>,
    queues: Arc<Mutex<HashMap<String, VecDeque<MessageMeta>>>>,
    injector: Arc<dyn InjectorOps>,
    shared_dot_dir: Option<PathBuf>,
    worktree_roots: Vec<PathBuf>,
}

impl MessageWatcher {
    pub fn new(
        registry: Registry,
        logger: Arc<Logger>,
        messages_dir: PathBuf,
    ) -> Self {
        Self::new_with_injector(registry, logger, messages_dir, Arc::new(RealInjector))
    }

    pub fn new_with_injector(
        registry: Registry,
        logger: Arc<Logger>,
        messages_dir: PathBuf,
        injector: Arc<dyn InjectorOps>,
    ) -> Self {
        let processed_dir = messages_dir.join("processed");
        let dead_letter_dir = messages_dir.join("dead_letter");
        MessageWatcher {
            registry,
            logger,
            messages_dir,
            processed_dir,
            dead_letter_dir,
            seen_hashes: Arc::new(Mutex::new(HashSet::new())),
            queues: Arc::new(Mutex::new(HashMap::new())),
            injector,
            shared_dot_dir: None,
            worktree_roots: Vec::new(),
        }
    }

    pub fn with_worktree_symlink_watch(
        mut self,
        shared_dot_dir: PathBuf,
        worktree_roots: Vec<PathBuf>,
    ) -> Self {
        self.shared_dot_dir = Some(shared_dot_dir);
        self.worktree_roots = worktree_roots;
        self
    }

    /// Start watching all `messages/to_*` directories.
    /// This spawns a background tokio task and returns immediately.
    pub async fn start(self: Arc<Self>) {
        self.start_worktree_symlink_watcher();

        // Use a std::sync channel so the notify callback (which runs on a
        // plain OS thread) can send without needing a tokio runtime handle.
        let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();

        let messages_dir = self.messages_dir.clone();
        std::thread::spawn(move || {
            let mut watcher = notify::recommended_watcher(move |res: Result<NotifyEvent, _>| {
                if let Ok(event) = res {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) => {
                            for path in event.paths {
                                if path.is_file() {
                                    let _ = tx.send(path);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            })
            .expect("failed to create filesystem watcher");

            // Watch each to_* directory
            for entry in std::fs::read_dir(&messages_dir).expect("can't read messages dir").flatten() {
                let name = entry.file_name();
                let name_str = name.to_str().unwrap_or("");
                if name_str.starts_with("to_") && entry.path().is_dir() {
                    watcher
                        .watch(&entry.path(), RecursiveMode::NonRecursive)
                        .expect("failed to watch directory");
                    println!("[watcher] watching {}", entry.path().display());
                }
            }

            // Block this thread to keep the watcher alive
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        });

        // Spawn the routing loop — polls the std channel from async context
        let watcher = self.clone();
        tokio::spawn(async move {
            watcher.routing_loop(rx).await;
        });
    }

    fn start_worktree_symlink_watcher(self: &Arc<Self>) {
        let Some(shared_dot_dir) = self.shared_dot_dir.clone() else {
            return;
        };
        if self.worktree_roots.is_empty() {
            return;
        }

        let worktree_roots = self.worktree_roots.clone();
        std::thread::spawn(move || {
            for root in &worktree_roots {
                let _ = crate::worktree::ensure_dot_orchestrator_symlink(&shared_dot_dir, root);
            }

            let callback_roots = worktree_roots.clone();
            let callback_shared_dot_dir = shared_dot_dir.clone();

            let mut watcher = notify::recommended_watcher(move |res: Result<NotifyEvent, _>| {
                if let Ok(event) = res {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                            for path in event.paths {
                                maybe_repair_worktree_dot_orchestrator(
                                    &path,
                                    &callback_shared_dot_dir,
                                    &callback_roots,
                                );
                            }
                        }
                        _ => {}
                    }
                }
            })
            .expect("failed to create worktree symlink watcher");

            for root in &worktree_roots {
                watcher
                    .watch(root, RecursiveMode::NonRecursive)
                    .expect("failed to watch worktree root");
                println!("[watcher] watching worktree symlink in {}", root.display());
            }

            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        });
    }

    async fn routing_loop(self: Arc<Self>, rx: std::sync::mpsc::Receiver<PathBuf>) {
        loop {
            // Poll the sync channel without blocking the tokio runtime
            let path = match rx.try_recv() {
                Ok(p) => p,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    sleep(Duration::from_millis(250)).await;
                    continue;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    eprintln!("[watcher] channel disconnected, stopping");
                    break;
                }
            };

            // Skip .gitkeep and hidden files
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            // Small delay to let atomic renames finish
            sleep(Duration::from_millis(200)).await;

            if !path.exists() {
                continue;
            }

            println!("[watcher] detected file: {}", path.display());

            let meta = match parse_message(&path) {
                Some(m) => m,
                None => {
                    eprintln!("[watcher] could not parse message: {}", path.display());
                    continue;
                }
            };

            self.logger.log(Event::MessageReceived {
                filename: meta.filename.clone(),
                sender: meta.sender.clone(),
                recipient: meta.recipient.clone(),
                topic: meta.topic.clone(),
            });

            // Backpressure: queue if inbox is overloaded
            let inbox_count = self.count_inbox(&meta.recipient).await;
            if inbox_count > BACKPRESSURE_THRESHOLD {
                println!(
                    "[watcher] backpressure: queuing message for {} (inbox has {} files)",
                    meta.recipient, inbox_count
                );
                let mut queues = self.queues.lock().await;
                queues
                    .entry(meta.recipient.clone())
                    .or_default()
                    .push_back(meta);
                continue;
            }

            self.route_message(meta).await;

            // Drain any queued messages for recipients that are below threshold
            self.drain_queues().await;
        }
    }

    pub async fn route_message(&self, meta: MessageMeta) {
        // Ensure destination dirs exist (creates them if missing).
        let _ = std::fs::create_dir_all(&self.processed_dir);
        let _ = std::fs::create_dir_all(&self.dead_letter_dir);

        // Deduplication by content hash — lives here so it applies whether
        // called from routing_loop or directly in tests.
        // Skip dedup for special topics (_RESTART, _INTERRUPT) — these are
        // commands that must always be processed regardless of content.
        let is_special = meta.topic.eq_ignore_ascii_case("_restart")
            || meta.topic.ends_with("_RESTART")
            || meta.topic.eq_ignore_ascii_case("_interrupt")
            || meta.topic.ends_with("_INTERRUPT")
            || meta.topic.eq_ignore_ascii_case("_timer")
            || meta.topic.ends_with("_TIMER")
            || meta.topic.eq_ignore_ascii_case("_attention")
            || meta.topic.ends_with("_ATTENTION");

        if !is_special {
            if let Ok(bytes) = std::fs::read(&meta.path) {
                let hash = format!("{:x}", Sha256::digest(&bytes));
                let mut seen = self.seen_hashes.lock().await;
                if seen.contains(&hash) {
                    println!("[watcher] skipping duplicate: {}", meta.filename);
                    let _ = std::fs::rename(&meta.path, self.processed_dir.join(&meta.filename));
                    return;
                }
                seen.insert(hash);
            }
        }

        // Handle _RESTART topic: restart the recipient agent with fresh context
        if meta.topic.eq_ignore_ascii_case("_restart") {
            println!(
                "[watcher] restart requested for '{}' by '{}'",
                meta.recipient, meta.sender
            );
            self.logger.log(Event::AgentRestartRequested {
                agent_id: meta.recipient.clone(),
                requested_by: meta.sender.clone(),
            });
            match self.registry.restart_agent(&meta.recipient).await {
                Ok(()) => {
                    let _ = std::fs::rename(&meta.path, self.processed_dir.join(&meta.filename));
                    println!(
                        "[watcher] {} restarted (requested by {})",
                        meta.recipient, meta.sender
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[watcher] failed to restart '{}': {e} — dead-lettering",
                        meta.recipient
                    );
                    let _ =
                        std::fs::rename(&meta.path, self.dead_letter_dir.join(&meta.filename));
                }
            }
            return;
        }

        // Handle _ATTENTION topic: agent is requesting operator attention.
        // Read the message, fire a visual alert, then move to processed/.
        // The agent continues running — this is a non-blocking signal to the operator.
        if meta.topic.eq_ignore_ascii_case("_attention") {
            println!(
                "[watcher] attention requested by '{}' (sender: '{}')",
                meta.recipient, meta.sender
            );
            let content = std::fs::read_to_string(&meta.path).unwrap_or_default();
            self.registry
                .fire_attention_alert(&meta.recipient, &content)
                .await;
            let _ = std::fs::rename(&meta.path, self.processed_dir.join(&meta.filename));
            return;
        }

        let session = match self.registry.session_for(&meta.recipient).await {
            Some(s) => s,
            None => {
                eprintln!(
                    "[watcher] no session for recipient '{}', dead-lettering: {}",
                    meta.recipient, meta.filename
                );
                self.logger.log(Event::MessageDeadLetter {
                    filename: meta.filename.clone(),
                    reason: format!("unknown recipient: {}", meta.recipient),
                });
                let _ = std::fs::rename(&meta.path, self.dead_letter_dir.join(&meta.filename));
                return;
            }
        };

        // Read file content
        let content = match std::fs::read_to_string(&meta.path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[watcher] failed to read {}: {e}", meta.filename);
                let _ = std::fs::rename(&meta.path, self.dead_letter_dir.join(&meta.filename));
                return;
            }
        };

        // Inject with framing header
        let framed = format!(
            "--- INCOMING MESSAGE ---\nFROM: {}\nTOPIC: {}\nFILE: {}\n---\n\n{}",
            meta.sender, meta.topic, meta.filename, content
        );

        match self.injector.inject(&session, &framed).await {
            Ok(()) => {
                self.logger.log(Event::MessageInjected {
                    filename: meta.filename.clone(),
                    recipient: meta.recipient.clone(),
                });
                let _ = std::fs::rename(&meta.path, self.processed_dir.join(&meta.filename));
                println!(
                    "[watcher] routed {} → {} ({})",
                    meta.sender, meta.recipient, meta.topic
                );
            }
            Err(e) => {
                self.logger.log(Event::MessageFailed {
                    filename: meta.filename.clone(),
                    recipient: meta.recipient.clone(),
                    error: e.to_string(),
                });
                eprintln!(
                    "[watcher] injection failed for {}: {e} — dead-lettering",
                    meta.filename
                );
                let _ = std::fs::rename(&meta.path, self.dead_letter_dir.join(&meta.filename));
            }
        }
    }

    pub async fn count_inbox(&self, recipient: &str) -> usize {
        let inbox = self.messages_dir.join(format!("to_{recipient}"));
        std::fs::read_dir(inbox)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.file_name()
                            .to_str()
                            .map(|n| !n.starts_with('.'))
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    async fn drain_queues(&self) {
        let mut queues = self.queues.lock().await;
        let recipients: Vec<String> = queues.keys().cloned().collect();
        for recipient in recipients {
            let count = self.count_inbox(&recipient).await;
            if count <= BACKPRESSURE_THRESHOLD {
                if let Some(queue) = queues.get_mut(&recipient) {
                    if let Some(meta) = queue.pop_front() {
                        drop(queues);
                        self.route_message(meta).await;
                        queues = self.queues.lock().await;
                    }
                }
            }
        }
    }
}

pub fn maybe_repair_worktree_dot_orchestrator(
    changed_path: &Path,
    shared_dot_dir: &Path,
    worktree_roots: &[PathBuf],
) -> bool {
    for root in worktree_roots {
        let dot_path = root.join(".orchestrator");
        if changed_path == dot_path || changed_path == root || changed_path.starts_with(&dot_path) {
            let metadata = match dot_path.symlink_metadata() {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return false,
                Err(err) => {
                    eprintln!(
                        "[watcher] failed to inspect .orchestrator path for {}: {err}",
                        root.display()
                    );
                    return false;
                }
            };

            if metadata.file_type().is_symlink() {
                return false;
            }

            if let Err(err) = crate::worktree::ensure_dot_orchestrator_symlink(shared_dot_dir, root) {
                eprintln!(
                    "[watcher] failed to repair .orchestrator symlink for {}: {err}",
                    root.display()
                );
            }
            return true;
        }
    }

    false
}
