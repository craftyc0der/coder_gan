//! Slack agent: WebSocket watcher + triage AI composite.
//!
//! Opens a Socket Mode WebSocket to Slack, filters inbound messages against
//! configured watch sources, writes qualifying messages to the triage AI's
//! inbox for analysis, and posts structured reports to a private notification
//! channel when the AI determines a response is needed.

use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::config::SlackConfig;
use crate::logger::{Event, Logger};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const RECONNECT_BASE_SECS: u64 = 1;
const MAX_RECONNECT_BACKOFF_EXP: u32 = 4; // cap at 16s
const MAX_RECONNECT_FAILURES: u32 = 5;
const RECONNECT_WINDOW_SECS: i64 = 120;
const INBOX_POLL_INTERVAL_SECS: u64 = 2;

// ---------------------------------------------------------------------------
// SlackWatcher
// ---------------------------------------------------------------------------

pub struct SlackWatcher {
    config: SlackConfig,
    agent_id: String,
    inbox_dir: PathBuf,
    messages_dir: PathBuf,
    logger: Arc<Logger>,
    seen_ts: Arc<Mutex<HashSet<String>>>,
    /// Track recent reconnect timestamps for degraded detection.
    reconnect_timestamps: Arc<Mutex<Vec<chrono::DateTime<Utc>>>>,
}

// ---------------------------------------------------------------------------
// Slack API response types
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct ConnectionOpenResponse {
    ok: bool,
    url: Option<String>,
    error: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ChatPostMessageResponse {
    ok: bool,
    error: Option<String>,
    ts: Option<String>,
}

// ---------------------------------------------------------------------------
// Socket Mode envelope / event types
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct SocketEnvelope {
    envelope_id: Option<String>,
    #[serde(rename = "type")]
    envelope_type: Option<String>,
    payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct SlackMessage {
    channel: String,
    user: String,
    text: String,
    ts: String,
    thread_ts: Option<String>,
    channel_type: Option<String>,
}

// ---------------------------------------------------------------------------
// Filter outcome (for logging)
// ---------------------------------------------------------------------------

enum FilterResult {
    Pass,
    BotSelf,
    IgnoredBot,
    Duplicate,
    TooShort,
    NoMatchingSource,
}

#[derive(Debug, Clone)]
enum WatchSource {
    Channel,
    Mention,
    #[allow(dead_code)]
    RepliedThread,
    Dm,
}

impl std::fmt::Display for WatchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchSource::Channel => write!(f, "channel"),
            WatchSource::Mention => write!(f, "mention"),
            WatchSource::RepliedThread => write!(f, "replied_thread"),
            WatchSource::Dm => write!(f, "dm"),
        }
    }
}

impl SlackWatcher {
    pub fn new(
        config: SlackConfig,
        agent_id: String,
        messages_dir: PathBuf,
        logger: Arc<Logger>,
    ) -> Self {
        let inbox_dir = messages_dir.join(format!("to_{agent_id}"));
        SlackWatcher {
            config,
            agent_id,
            inbox_dir,
            messages_dir,
            logger,
            seen_ts: Arc::new(Mutex::new(HashSet::new())),
            reconnect_timestamps: Arc::new(Mutex::new(Vec::new())),
        }
    }

    // ------------------------------------------------------------------
    // Connection
    // ------------------------------------------------------------------

    /// Request a WebSocket URL from `apps.connections.open`.
    async fn get_ws_url(&self) -> Result<String, String> {
        let client = reqwest::Client::new();
        let resp = client
            .post("https://slack.com/api/apps.connections.open")
            .bearer_auth(&self.config.app_token)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let body: ConnectionOpenResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        if !body.ok {
            return Err(format!(
                "apps.connections.open failed: {}",
                body.error.unwrap_or_else(|| "unknown".into())
            ));
        }

        body.url.ok_or_else(|| "No URL in response".into())
    }

    // ------------------------------------------------------------------
    // Main run loop (connect + process events)
    // ------------------------------------------------------------------

    /// Main entry point — connects and processes events in a loop with
    /// auto-reconnection and backoff.
    pub async fn run(&self) {
        let mut consecutive_failures: u32 = 0;

        loop {
            // Check degraded state
            {
                let mut timestamps = self.reconnect_timestamps.lock().await;
                let cutoff = Utc::now() - chrono::Duration::seconds(RECONNECT_WINDOW_SECS);
                timestamps.retain(|t| *t > cutoff);
                if timestamps.len() as u32 >= MAX_RECONNECT_FAILURES {
                    eprintln!(
                        "[slack] {} marked DEGRADED after {} reconnect failures in {}s window",
                        self.agent_id,
                        timestamps.len(),
                        RECONNECT_WINDOW_SECS
                    );
                    // Log and stop — supervisor can detect via health check
                    return;
                }
            }

            match self.connect_and_process().await {
                Ok(()) => {
                    // Clean disconnect (e.g., server asked us to reconnect)
                    consecutive_failures = 0;
                    println!("[slack] {} disconnected cleanly, reconnecting...", self.agent_id);
                }
                Err(e) => {
                    consecutive_failures += 1;
                    self.reconnect_timestamps.lock().await.push(Utc::now());
                    self.logger.log(Event::SlackDisconnected {
                        agent_id: self.agent_id.clone(),
                        reason: e.clone(),
                    });
                    eprintln!(
                        "[slack] {} connection error (attempt {}): {e}",
                        self.agent_id, consecutive_failures
                    );
                }
            }

            let backoff = Duration::from_secs(
                RECONNECT_BASE_SECS << consecutive_failures.min(MAX_RECONNECT_BACKOFF_EXP),
            );
            println!(
                "[slack] {} reconnecting in {:?}...",
                self.agent_id, backoff
            );
            sleep(backoff).await;
        }
    }

    /// Connect to the WebSocket and process events until disconnection.
    async fn connect_and_process(&self) -> Result<(), String> {
        let ws_url = self.get_ws_url().await?;

        // Validate the URL before connecting
        let _parsed = url::Url::parse(&ws_url)
            .map_err(|e| format!("Invalid WebSocket URL: {e}"))?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| format!("WebSocket connect failed: {e}"))?;

        let channels_str = self.config.watch_channels.join(", ");
        println!(
            "[slack] {} connected — watching channels: [{}]",
            self.agent_id, channels_str
        );
        self.logger.log(Event::SlackConnected {
            agent_id: self.agent_id.clone(),
            channels: channels_str.clone(),
        });

        let (mut write, mut read) = ws_stream.split();

        while let Some(msg_result) = read.next().await {
            let msg = match msg_result {
                Ok(m) => m,
                Err(e) => return Err(format!("WebSocket read error: {e}")),
            };

            match msg {
                WsMessage::Text(text) => {
                    self.handle_socket_event(&text, &mut write).await?;
                }
                WsMessage::Ping(data) => {
                    let _ = write.send(WsMessage::Pong(data)).await;
                }
                WsMessage::Close(_) => {
                    return Ok(());
                }
                _ => {}
            }
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // Socket Mode event handling
    // ------------------------------------------------------------------

    async fn handle_socket_event<S>(
        &self,
        raw: &str,
        write: &mut S,
    ) -> Result<(), String>
    where
        S: futures_util::Sink<WsMessage> + Unpin,
        S::Error: std::fmt::Display,
    {
        let envelope: SocketEnvelope = match serde_json::from_str(raw) {
            Ok(e) => e,
            Err(_) => return Ok(()), // silently skip unparseable
        };

        // Acknowledge envelope immediately (Socket Mode requirement)
        if let Some(ref eid) = envelope.envelope_id {
            let ack = serde_json::json!({ "envelope_id": eid });
            write
                .send(WsMessage::Text(ack.to_string().into()))
                .await
                .map_err(|e| format!("Failed to send ack: {e}"))?;
        }

        // Only process events_api envelopes
        let envelope_type = envelope.envelope_type.as_deref().unwrap_or("");
        if envelope_type != "events_api" {
            return Ok(());
        }

        let payload = match envelope.payload {
            Some(p) => p,
            None => return Ok(()),
        };

        // Extract the inner event
        let event = match payload.get("event") {
            Some(e) => e,
            None => return Ok(()),
        };

        let event_type = event
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if event_type != "message" {
            return Ok(());
        }

        // Skip message subtypes that aren't real user messages
        if let Some(subtype) = event.get("subtype").and_then(|v| v.as_str()) {
            match subtype {
                "message_changed" | "message_deleted" | "channel_join"
                | "channel_leave" | "bot_message" => return Ok(()),
                _ => {}
            }
        }

        let msg = SlackMessage {
            channel: event
                .get("channel")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            user: event
                .get("user")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            text: event
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            ts: event
                .get("ts")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            thread_ts: event
                .get("thread_ts")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            channel_type: event
                .get("channel_type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        };

        self.process_message(msg).await;

        Ok(())
    }

    // ------------------------------------------------------------------
    // Message processing pipeline
    // ------------------------------------------------------------------

    async fn process_message(&self, msg: SlackMessage) {
        // 1. Filter
        let filter_result = self.filter_message(&msg).await;
        match filter_result {
            FilterResult::Pass => {}
            FilterResult::BotSelf => {
                self.logger.log(Event::SlackMessageFiltered {
                    agent_id: self.agent_id.clone(),
                    channel: msg.channel.clone(),
                    reason: "bot_self".into(),
                });
                return;
            }
            FilterResult::IgnoredBot => {
                self.logger.log(Event::SlackMessageFiltered {
                    agent_id: self.agent_id.clone(),
                    channel: msg.channel.clone(),
                    reason: "ignored_bot".into(),
                });
                return;
            }
            FilterResult::Duplicate => {
                self.logger.log(Event::SlackMessageFiltered {
                    agent_id: self.agent_id.clone(),
                    channel: msg.channel.clone(),
                    reason: "duplicate".into(),
                });
                return;
            }
            FilterResult::TooShort => {
                self.logger.log(Event::SlackMessageFiltered {
                    agent_id: self.agent_id.clone(),
                    channel: msg.channel.clone(),
                    reason: "too_short".into(),
                });
                return;
            }
            FilterResult::NoMatchingSource => {
                self.logger.log(Event::SlackMessageFiltered {
                    agent_id: self.agent_id.clone(),
                    channel: msg.channel.clone(),
                    reason: "no_matching_source".into(),
                });
                return;
            }
        }

        // 2. Determine watch source
        let source = self.match_watch_source(&msg);
        let source = match source {
            Some(s) => s,
            None => return, // shouldn't happen after filter pass
        };

        self.logger.log(Event::SlackMessageReceived {
            agent_id: self.agent_id.clone(),
            channel: msg.channel.clone(),
            author: msg.user.clone(),
            ts: msg.ts.clone(),
        });

        // 3. Write to triage AI inbox
        match self.write_inbox_message(&msg, &source).await {
            Ok(filename) => {
                self.logger.log(Event::SlackMessageRouted {
                    agent_id: self.agent_id.clone(),
                    source: source.to_string(),
                    filename,
                });
            }
            Err(e) => {
                eprintln!(
                    "[slack] {} failed to write inbox message: {e}",
                    self.agent_id
                );
            }
        }
    }

    /// Check if a message should be filtered out.
    async fn filter_message(&self, msg: &SlackMessage) -> FilterResult {
        // Bot's own messages
        if msg.user == self.config.bot_user_id {
            return FilterResult::BotSelf;
        }

        // Ignored bots
        if self.config.ignore_bot_ids.contains(&msg.user) {
            return FilterResult::IgnoredBot;
        }

        // Dedup by timestamp
        {
            let mut seen = self.seen_ts.lock().await;
            if seen.contains(&msg.ts) {
                return FilterResult::Duplicate;
            }
            seen.insert(msg.ts.clone());
        }

        // Minimum length
        if msg.text.len() < self.config.min_message_length {
            return FilterResult::TooShort;
        }

        // Must match at least one watch source
        if self.match_watch_source(msg).is_none() {
            return FilterResult::NoMatchingSource;
        }

        FilterResult::Pass
    }

    /// Determine which watch source a message matches (first match wins).
    fn match_watch_source(&self, msg: &SlackMessage) -> Option<WatchSource> {
        // Channel watch
        if self.config.watch_channels.contains(&msg.channel) {
            return Some(WatchSource::Channel);
        }

        // Mention watch
        if self.config.watch_mentions {
            let mention_tag = format!("<@{}>", self.config.alert_user_id);
            if msg.text.contains(&mention_tag) {
                return Some(WatchSource::Mention);
            }
        }

        // DM watch
        if self.config.watch_dms {
            if let Some(ref ct) = msg.channel_type {
                if ct == "im" || ct == "mpim" {
                    return Some(WatchSource::Dm);
                }
            }
        }

        // Replied thread watch — would require tracking thread participation,
        // which needs API calls. We check for the thread_ts presence and
        // defer full thread-participant checking to a future enhancement.
        // For now, thread messages in watched channels are caught by the
        // channel watch above.

        None
    }

    /// Find keywords that match in the message text (case-insensitive).
    fn matched_keywords(&self, text: &str) -> Vec<String> {
        let lower = text.to_lowercase();
        self.config
            .alert_keywords
            .iter()
            .filter(|kw| lower.contains(&kw.to_lowercase()))
            .cloned()
            .collect()
    }

    // ------------------------------------------------------------------
    // Inbox writing
    // ------------------------------------------------------------------

    /// Write a message file to the triage AI's inbox for processing.
    /// Uses atomic write (temp file + rename).
    async fn write_inbox_message(
        &self,
        msg: &SlackMessage,
        source: &WatchSource,
    ) -> Result<String, String> {
        let now = Utc::now().format("%Y-%m-%dT%H-%M-%SZ");
        let filename = format!(
            "{now}__from-slack__to-{agent}__topic-slack-triage.md",
            agent = self.agent_id,
        );

        let permalink = generate_permalink(&msg.channel, &msg.ts);
        let keywords = self.matched_keywords(&msg.text);
        let keywords_line = if keywords.is_empty() {
            String::new()
        } else {
            format!("Keywords matched: {}\n", keywords.join(", "))
        };

        // Escape template variables in the Slack message text to prevent
        // prompt injection via Slack messages (security: untrusted input).
        let escaped_text = msg.text.replace("{{", "{ {").replace("}}", "} }");

        let content = format!(
            "--- SLACK MESSAGE ---\n\
             Channel: {channel}\n\
             Author: <@{user}>\n\
             Time: {time}\n\
             Thread: {permalink}\n\
             Source: {source}\n\
             {keywords_line}\
             \n\
             {text}\n\
             ---\n\
             \n\
             Please analyze this message:\n\
             1. Does this message need a response from me? (YES/NO)\n\
             2. If YES: Summarize the issue concisely\n\
             3. Cross-reference with Notion project docs for context (use the Notion MCP)\n\
             4. Assess urgency (P0-P4)\n\
             5. Draft a suggested response\n\
             6. Format your findings as a report\n\
             7. If NO: Briefly state why no response is needed (for logging only)\n",
            channel = msg.channel,
            user = msg.user,
            time = Utc::now().to_rfc3339(),
            permalink = permalink,
            source = source,
            keywords_line = keywords_line,
            text = escaped_text,
        );

        // Atomic write: temp file → rename
        let dest = self.inbox_dir.join(&filename);
        let tmp = self
            .inbox_dir
            .join(format!(".tmp_{}", content_hash(&content)));

        std::fs::create_dir_all(&self.inbox_dir)
            .map_err(|e| format!("Failed to create inbox dir: {e}"))?;
        std::fs::write(&tmp, &content)
            .map_err(|e| format!("Failed to write temp file: {e}"))?;
        std::fs::rename(&tmp, &dest)
            .map_err(|e| format!("Failed to rename to inbox: {e}"))?;

        Ok(filename)
    }

    // ------------------------------------------------------------------
    // Response inbox watching
    // ------------------------------------------------------------------

    /// Watch the agent's inbox for response files from the triage AI.
    /// When a response arrives, parse it and decide whether to post a
    /// notification.
    pub async fn watch_response_inbox(&self) {
        let response_dir = self.messages_dir.join(format!("to_{}", self.agent_id));
        let processed_dir = self.messages_dir.join("processed");
        let _ = std::fs::create_dir_all(&response_dir);
        let _ = std::fs::create_dir_all(&processed_dir);

        loop {
            sleep(Duration::from_secs(INBOX_POLL_INTERVAL_SECS)).await;

            let entries = match std::fs::read_dir(&response_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                let filename = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                // Skip temp files and non-md/txt files
                if filename.starts_with(".tmp_") {
                    continue;
                }
                if !filename.ends_with(".md") && !filename.ends_with(".txt") {
                    continue;
                }

                // Skip triage request files (we wrote these ourselves)
                if filename.contains("topic-slack-triage") {
                    continue;
                }

                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                // Parse "needs response" gating
                let needs_response = parse_needs_response(&content);

                if needs_response {
                    match self.post_notification(&content).await {
                        Ok(_) => {
                            self.logger.log(Event::SlackNotificationPosted {
                                agent_id: self.agent_id.clone(),
                                notification_channel: self.config.notification_channel.clone(),
                            });
                        }
                        Err(e) => {
                            self.logger.log(Event::SlackNotificationFailed {
                                agent_id: self.agent_id.clone(),
                                error: e.clone(),
                            });
                            eprintln!(
                                "[slack] {} notification post failed: {e}",
                                self.agent_id
                            );
                        }
                    }
                } else {
                    self.logger.log(Event::SlackResponseSkipped {
                        agent_id: self.agent_id.clone(),
                        channel: String::new(),
                        reason: "AI assessed: no response needed".into(),
                    });
                }

                // Move to processed
                let dest = processed_dir.join(&filename);
                let _ = std::fs::rename(&path, &dest);
            }
        }
    }

    // ------------------------------------------------------------------
    // Notification posting
    // ------------------------------------------------------------------

    /// Post a structured notification to the private notification channel.
    /// This is the ONLY channel the bot ever writes to.
    async fn post_notification(&self, report: &str) -> Result<(), String> {
        let text = format!(
            "{report}\n\ncc <@{user}>",
            report = report,
            user = self.config.alert_user_id,
        );

        let client = reqwest::Client::new();
        let resp = client
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&self.config.bot_token)
            .json(&serde_json::json!({
                "channel": self.config.notification_channel,
                "text": text,
                "unfurl_links": false,
                "unfurl_media": false,
            }))
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let body: ChatPostMessageResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        if !body.ok {
            return Err(format!(
                "chat.postMessage failed: {}",
                body.error.unwrap_or_else(|| "unknown".into())
            ));
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Generate a Slack permalink from channel ID and message timestamp.
pub fn generate_permalink(channel: &str, ts: &str) -> String {
    // Slack permalinks use the timestamp without the dot
    let ts_nodot = ts.replace('.', "");
    format!(
        "https://slack.com/archives/{channel}/p{ts}",
        channel = channel,
        ts = ts_nodot,
    )
}

/// Parse whether the triage AI response indicates a response is needed.
/// Looks for "YES" or "NO" patterns near the beginning of the response.
pub fn parse_needs_response(content: &str) -> bool {
    let upper = content.to_uppercase();
    // Look for explicit YES/NO patterns
    // The triage prompt asks: "Does this message need a response from me? (YES/NO)"
    if upper.contains("NEEDS RESPONSE: YES")
        || upper.contains("**YES**")
        || upper.contains("RESPONSE NEEDED: YES")
    {
        return true;
    }
    if upper.contains("NEEDS RESPONSE: NO")
        || upper.contains("**NO**")
        || upper.contains("RESPONSE NEEDED: NO")
    {
        return false;
    }
    // Fallback: look for YES as a standalone word near the top
    for line in content.lines().take(10) {
        let trimmed = line.trim().to_uppercase();
        if trimmed == "YES" || trimmed.starts_with("YES ") || trimmed.starts_with("YES,") {
            return true;
        }
        if trimmed == "NO" || trimmed.starts_with("NO ") || trimmed.starts_with("NO,") {
            return false;
        }
    }
    // Default: don't post if unclear
    false
}

/// SHA-256 hash of content for dedup / temp file naming.
fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())[..16].to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_permalink() {
        assert_eq!(
            generate_permalink("C01ABC", "1709388600.000000"),
            "https://slack.com/archives/C01ABC/p1709388600000000"
        );
    }

    #[test]
    fn test_generate_permalink_no_dot() {
        assert_eq!(
            generate_permalink("C99", "123456"),
            "https://slack.com/archives/C99/p123456"
        );
    }

    #[test]
    fn test_parse_needs_response_yes() {
        assert!(parse_needs_response("Needs Response: YES\n\nSummary: ..."));
        assert!(parse_needs_response("**YES**\n\nThe message requires..."));
        assert!(parse_needs_response("Response Needed: YES"));
    }

    #[test]
    fn test_parse_needs_response_no() {
        assert!(!parse_needs_response("Needs Response: NO\n\nThis is just FYI"));
        assert!(!parse_needs_response("**NO**\n\nNo action needed"));
        assert!(!parse_needs_response("Response Needed: NO"));
    }

    #[test]
    fn test_parse_needs_response_standalone_yes() {
        assert!(parse_needs_response("YES\n\nThe pipeline is broken..."));
        assert!(parse_needs_response("YES, this needs attention"));
    }

    #[test]
    fn test_parse_needs_response_standalone_no() {
        assert!(!parse_needs_response("NO\n\nJust a status update"));
        assert!(!parse_needs_response("NO, this is informational"));
    }

    #[test]
    fn test_parse_needs_response_default_false() {
        assert!(!parse_needs_response("Some random text without YES or NO markers"));
    }

    #[test]
    fn test_content_hash_deterministic() {
        let h1 = content_hash("hello world");
        let h2 = content_hash("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
    }

    #[test]
    fn test_content_hash_different() {
        let h1 = content_hash("hello");
        let h2 = content_hash("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_template_var_escaping() {
        // Verify that template variables in slack messages get escaped
        let text = "Hey {{project_root}}, check the {{agent_id}} status";
        let escaped = text.replace("{{", "{ {").replace("}}", "} }");
        assert!(!escaped.contains("{{"));
        assert!(!escaped.contains("}}"));
        assert!(escaped.contains("{ {project_root} }"));
    }

    #[test]
    fn test_watch_source_display() {
        assert_eq!(WatchSource::Channel.to_string(), "channel");
        assert_eq!(WatchSource::Mention.to_string(), "mention");
        assert_eq!(WatchSource::RepliedThread.to_string(), "replied_thread");
        assert_eq!(WatchSource::Dm.to_string(), "dm");
    }
}
