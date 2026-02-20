use std::future::Future;
use std::pin::Pin;
use std::process::Command;
use tokio::time::{sleep, Duration};

const MAX_RETRIES: u32 = 3;
const RETRY_BACKOFF_SECS: u64 = 1;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum InjectionError {
    TempFileWrite(String),
    TmuxCommand { step: String, detail: String },
    RetriesExhausted { attempts: u32, last_error: String },
}

impl std::fmt::Display for InjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InjectionError::TempFileWrite(e) => write!(f, "temp file write: {e}"),
            InjectionError::TmuxCommand { step, detail } => {
                write!(f, "tmux {step}: {detail}")
            }
            InjectionError::RetriesExhausted {
                attempts,
                last_error,
            } => write!(f, "failed after {attempts} attempts: {last_error}"),
        }
    }
}

// ---------------------------------------------------------------------------
// tmux primitives
// ---------------------------------------------------------------------------

/// Check if a tmux session exists.
pub fn has_session(session: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", session])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Spawn a detached tmux session running the given shell command,
/// then open a visible Terminal.app window attached to it.
/// Returns the Terminal.app window ID on success, or `None` if the window ID
/// could not be determined (e.g. on non-macOS systems or if osascript failed).
pub fn spawn_session(session: &str, cmd: &str) -> Result<Option<u32>, InjectionError> {
    // Create the detached session
    let status = Command::new("tmux")
        .args([
            "new-session", "-d", "-s", session, "-x", "200", "-y", "50", cmd,
        ])
        .status()
        .map_err(|e| InjectionError::TmuxCommand {
            step: "new-session".into(),
            detail: e.to_string(),
        })?;
    if !status.success() {
        return Err(InjectionError::TmuxCommand {
            step: "new-session".into(),
            detail: format!("exited with {status}"),
        });
    }

    // Open a new Terminal.app window attached to this tmux session and capture
    // the window ID so it can be closed later on shutdown.
    let script = format!(
        "tell application \"Terminal\"\n\
         activate\n\
         do script \"tmux attach -t {session}\"\n\
         return id of front window\n\
         end tell"
    );
    let window_id = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                s.parse::<u32>().ok()
            } else {
                None
            }
        });

    Ok(window_id)
}

/// Close a Terminal.app window by its window ID.
/// macOS only. Silently no-ops if the window no longer exists or osascript is
/// unavailable (e.g. on Linux).
pub fn close_terminal_window(window_id: u32) {
    // Guard against non-existent windows with a filter before closing.
    let script = format!(
        "tell application \"Terminal\"\n\
         set matchingWindows to windows whose id is {window_id}\n\
         if (count matchingWindows) > 0 then\n\
         close (first item of matchingWindows)\n\
         end if\n\
         end tell"
    );
    let _ = Command::new("osascript").args(["-e", &script]).status();
}

/// Kill a tmux session.
pub fn kill_session(session: &str) {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", session])
        .status();
}

/// Inject text into a tmux session (single attempt).
/// Uses send-keys which handles Enter naturally — the text is sent as
/// keyboard input followed by an Enter keystroke.
fn inject_once(session: &str, text: &str) -> Result<(), InjectionError> {
    // send-keys sends the text as literal keystrokes, then Enter submits it.
    // We use send-keys with -l (literal) to avoid interpreting special chars,
    // then a separate send-keys for Enter.
    let run_tmux = |args: &[&str]| -> Result<(), InjectionError> {
        let output = Command::new("tmux")
            .args(args)
            .output()
            .map_err(|e| InjectionError::TmuxCommand {
                step: args[0].to_string(),
                detail: e.to_string(),
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(InjectionError::TmuxCommand {
                step: args[0].to_string(),
                detail: format!(
                    "exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ),
            })
        }
    };

    // For long text, use load-buffer + paste-buffer (send-keys has length limits).
    // Then send Enter separately after a short delay for the paste to land.
    let tmp_path = std::env::temp_dir().join(format!(
        "coder-gan-inject-{}-{}.txt",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));

    std::fs::write(&tmp_path, text).map_err(|e| InjectionError::TempFileWrite(e.to_string()))?;

    let result = (|| {
        run_tmux(&["load-buffer", tmp_path.to_str().unwrap()])?;
        run_tmux(&["paste-buffer", "-t", session])?;
        // Wait for the paste to land in the terminal before sending Enter
        std::thread::sleep(std::time::Duration::from_millis(500));
        run_tmux(&["send-keys", "-t", session, "Enter"])?;
        Ok(())
    })();

    let _ = std::fs::remove_file(&tmp_path);
    result
}

/// Inject text into a tmux session with up to MAX_RETRIES attempts.
pub async fn inject(session: &str, text: &str) -> Result<(), InjectionError> {
    let mut last_err = String::new();
    for attempt in 1..=MAX_RETRIES {
        match inject_once(session, text) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e.to_string();
                if attempt < MAX_RETRIES {
                    sleep(Duration::from_secs(RETRY_BACKOFF_SECS * attempt as u64)).await;
                }
            }
        }
    }
    Err(InjectionError::RetriesExhausted {
        attempts: MAX_RETRIES,
        last_error: last_err,
    })
}

/// Capture the current pane content of a tmux session.
/// Uses `-S -500` to grab up to 500 lines of scrollback.
pub fn capture(session: &str) -> Result<String, InjectionError> {
    let output = Command::new("tmux")
        .args(["capture-pane", "-t", session, "-p", "-S", "-500"])
        .output()
        .map_err(|e| InjectionError::TmuxCommand {
            step: "capture-pane".into(),
            detail: e.to_string(),
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(InjectionError::TmuxCommand {
            step: "capture-pane".into(),
            detail: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Injectable interface (enables mocking in tests)
// ---------------------------------------------------------------------------

/// Abstraction over tmux operations so that tests can substitute a mock
/// without launching any real processes.
pub trait InjectorOps: Send + Sync {
    fn has_session(&self, session: &str) -> bool;
    fn kill_session(&self, session: &str);
    /// Spawn a tmux session. Returns the Terminal.app window ID if one was
    /// opened, or `None` (e.g. in tests or on non-macOS systems).
    fn spawn_session(&self, session: &str, cmd: &str) -> Result<Option<u32>, InjectionError>;
    /// Async inject — returns a boxed future so the trait is object-safe.
    fn inject<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>>;
    fn capture(&self, session: &str) -> Result<String, InjectionError>;
}

/// Real implementation that delegates to the tmux CLI functions above.
pub struct RealInjector;

impl InjectorOps for RealInjector {
    fn has_session(&self, session: &str) -> bool {
        has_session(session)
    }
    fn kill_session(&self, session: &str) {
        kill_session(session)
    }
    fn spawn_session(&self, session: &str, cmd: &str) -> Result<Option<u32>, InjectionError> {
        spawn_session(session, cmd)
    }
    fn inject<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>> {
        // Call the module-level free function.
        Box::pin(inject(session, text))
    }
    fn capture(&self, session: &str) -> Result<String, InjectionError> {
        capture(session)
    }
}
