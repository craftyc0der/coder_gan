use chrono::Utc;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event")]
pub enum Event {
    #[serde(rename = "agent_spawn")]
    AgentSpawn { agent_id: String },

    #[serde(rename = "agent_exit")]
    AgentExit {
        agent_id: String,
        reason: String,
    },

    #[serde(rename = "agent_restart")]
    AgentRestart {
        agent_id: String,
        attempt: u32,
    },

    #[serde(rename = "agent_degraded")]
    AgentDegraded {
        agent_id: String,
        restart_count: u32,
    },

    #[serde(rename = "message_received")]
    MessageReceived {
        filename: String,
        sender: String,
        recipient: String,
        topic: String,
    },

    #[serde(rename = "message_injected")]
    MessageInjected {
        filename: String,
        recipient: String,
    },

    #[serde(rename = "message_failed")]
    MessageFailed {
        filename: String,
        recipient: String,
        error: String,
    },

    #[serde(rename = "message_dead_letter")]
    MessageDeadLetter {
        filename: String,
        reason: String,
    },

    #[serde(rename = "agent_restart_requested")]
    AgentRestartRequested {
        agent_id: String,
        requested_by: String,
    },

    #[serde(rename = "orchestrator_start")]
    OrchestratorStart,

    #[serde(rename = "orchestrator_stop")]
    OrchestratorStop,

    #[serde(rename = "transcript_captured")]
    TranscriptCaptured {
        agent_id: String,
        chars: usize,
    },

    #[serde(rename = "scope_violation")]
    ScopeViolation {
        path: String,
        detail: String,
    },

    // Spike-specific events
    #[serde(rename = "spike_inject_sent")]
    SpikeInjectSent {
        agent_id: String,
        detail: String,
    },

    #[serde(rename = "spike_inject_confirmed")]
    SpikeInjectConfirmed {
        agent_id: String,
        detail: String,
    },

    #[serde(rename = "spike_inject_timeout")]
    SpikeInjectTimeout {
        agent_id: String,
        detail: String,
    },

    #[serde(rename = "spike_validation_failed")]
    SpikeValidationFailed {
        agent_id: String,
        detail: String,
    },

    #[serde(rename = "spike_capture")]
    SpikeCapture {
        agent_id: String,
        path: String,
    },

    #[serde(rename = "spike_interrupt_sent")]
    SpikeInterruptSent {
        agent_id: String,
        cancel_key: String,
        clear_key: String,
    },

    #[serde(rename = "spike_interrupt_confirmed")]
    SpikeInterruptConfirmed {
        agent_id: String,
        detail: String,
    },

    #[serde(rename = "spike_interrupt_failed")]
    SpikeInterruptFailed {
        agent_id: String,
        detail: String,
    },

    #[serde(rename = "timer_fired")]
    TimerFired {
        agent_id: String,
        minutes: u64,
        prompt_file: String,
    },

    // Slack agent events
    #[cfg(feature = "slack")]
    #[serde(rename = "slack_connected")]
    SlackConnected {
        agent_id: String,
        channels: String,
    },

    #[cfg(feature = "slack")]
    #[serde(rename = "slack_disconnected")]
    SlackDisconnected {
        agent_id: String,
        reason: String,
    },

    #[cfg(feature = "slack")]
    #[serde(rename = "slack_message_received")]
    SlackMessageReceived {
        agent_id: String,
        channel: String,
        author: String,
        ts: String,
    },

    #[cfg(feature = "slack")]
    #[serde(rename = "slack_message_filtered")]
    SlackMessageFiltered {
        agent_id: String,
        channel: String,
        reason: String,
    },

    #[cfg(feature = "slack")]
    #[serde(rename = "slack_message_routed")]
    SlackMessageRouted {
        agent_id: String,
        source: String,
        filename: String,
    },

    #[cfg(feature = "slack")]
    #[serde(rename = "slack_response_skipped")]
    SlackResponseSkipped {
        agent_id: String,
        channel: String,
        reason: String,
    },

    #[cfg(feature = "slack")]
    #[serde(rename = "slack_notification_posted")]
    SlackNotificationPosted {
        agent_id: String,
        notification_channel: String,
    },

    #[cfg(feature = "slack")]
    #[serde(rename = "slack_notification_failed")]
    SlackNotificationFailed {
        agent_id: String,
        error: String,
    },
}

// ---------------------------------------------------------------------------
// Log entry (timestamp wrapper)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct LogEntry {
    timestamp: String,
    #[serde(flatten)]
    event: Event,
}

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

pub struct Logger {
    path: PathBuf,
    file: Mutex<Option<std::fs::File>>,
}

impl Logger {
    pub fn new(log_dir: &Path, filename: &str) -> Self {
        let path = log_dir.join(filename);
        let _ = std::fs::create_dir_all(log_dir);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        Logger {
            path,
            file: Mutex::new(file),
        }
    }

    pub fn log(&self, event: Event) {
        let entry = LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            event,
        };
        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(_) => return,
        };

        if let Ok(mut guard) = self.file.lock() {
            if let Some(ref mut f) = *guard {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
