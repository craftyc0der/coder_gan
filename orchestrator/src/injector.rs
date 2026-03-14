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
/// then open a visible terminal window attached to it.
/// Returns the terminal handle on success, or `None` if the handle
/// could not be determined (e.g. headless mode or unsupported OS).
pub fn spawn_session(session: &str, cmd: &str) -> Result<Option<u32>, InjectionError> {
    // Create the detached session
    let status = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            session,
            "-x",
            "200",
            "-y",
            "50",
            cmd,
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

    let mouse_status = Command::new("tmux")
        .args(["set-option", "-t", session, "mouse", "on"])
        .status()
        .map_err(|e| InjectionError::TmuxCommand {
            step: "set-option mouse".into(),
            detail: e.to_string(),
        })?;
    if !mouse_status.success() {
        return Err(InjectionError::TmuxCommand {
            step: "set-option mouse".into(),
            detail: format!("exited with {mouse_status}"),
        });
    }

    let history_status = Command::new("tmux")
        .args(["set-option", "-t", session, "history-limit", "100000"])
        .status()
        .map_err(|e| InjectionError::TmuxCommand {
            step: "set-option history-limit".into(),
            detail: e.to_string(),
        })?;
    if !history_status.success() {
        return Err(InjectionError::TmuxCommand {
            step: "set-option history-limit".into(),
            detail: format!("exited with {history_status}"),
        });
    }

    let title_status = Command::new("tmux")
        .args(["set-option", "-t", session, "set-titles", "on"])
        .status()
        .map_err(|e| InjectionError::TmuxCommand {
            step: "set-option set-titles".into(),
            detail: e.to_string(),
        })?;
    if !title_status.success() {
        return Err(InjectionError::TmuxCommand {
            step: "set-option set-titles".into(),
            detail: format!("exited with {title_status}"),
        });
    }

    let title_string_status = Command::new("tmux")
        .args(["set-option", "-t", session, "set-titles-string", "#S"])
        .status()
        .map_err(|e| InjectionError::TmuxCommand {
            step: "set-option set-titles-string".into(),
            detail: e.to_string(),
        })?;
    if !title_string_status.success() {
        return Err(InjectionError::TmuxCommand {
            step: "set-option set-titles-string".into(),
            detail: format!("exited with {title_string_status}"),
        });
    }

    Ok(open_terminal_window(session))
}

/// Open a visible terminal window attached to the given tmux session.
/// Returns a platform-specific handle (window ID on macOS, PID on Linux).
pub fn open_terminal_window(session: &str) -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"Terminal\"\n\
             activate\n\
             do script \"tmux attach -t {session}\"\n\
             return id of front window\n\
             end tell"
        );
        return Command::new("osascript")
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
    }

    #[cfg(target_os = "linux")]
    {
        // Check for graphical display
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return None;
        }

        if let Some((cmd, args)) = detect_terminal_emulator(session) {
            if let Ok(child) = Command::new(&cmd).args(&args).spawn() {
                return Some(child.id());
            }
        }
        None
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Detect an available terminal emulator and return the command and args needed
/// to open a window running `tmux attach -t <session>`.
#[cfg(target_os = "linux")]
pub fn detect_terminal_emulator(session: &str) -> Option<(String, Vec<String>)> {
    let attach_cmd = format!("tmux attach -t {session}");

    // 1. Check $TERMINAL
    if let Ok(term) = std::env::var("TERMINAL") {
        if !term.is_empty() {
            // Try to match known emulators for specific flags
            let term_name = std::path::Path::new(&term)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&term);

            match term_name {
                "ptyxis" => {
                    return Some((
                        term,
                        vec![
                            "-s".into(),
                            "--".into(),
                            "tmux".into(),
                            "attach".into(),
                            "-t".into(),
                            session.into(),
                        ],
                    ))
                }
                "gnome-terminal" => {
                    return Some((
                        term,
                        vec![
                            "--wait".into(),
                            "--".into(),
                            "tmux".into(),
                            "attach".into(),
                            "-t".into(),
                            session.into(),
                        ],
                    ))
                }
                "konsole" => {
                    return Some((
                        term,
                        vec![
                            "--nofork".into(),
                            "-e".into(),
                            "tmux".into(),
                            "attach".into(),
                            "-t".into(),
                            session.into(),
                        ],
                    ))
                }
                "xfce4-terminal" => {
                    return Some((
                        term,
                        vec!["--disable-server".into(), "-e".into(), attach_cmd],
                    ))
                }
                "alacritty" | "xterm" => {
                    return Some((
                        term,
                        vec![
                            "-e".into(),
                            "tmux".into(),
                            "attach".into(),
                            "-t".into(),
                            session.into(),
                        ],
                    ))
                }
                "kitty" => {
                    return Some((
                        term,
                        vec!["tmux".into(), "attach".into(), "-t".into(), session.into()],
                    ))
                }
                _ => {
                    // Unknown terminal, try generic -e flag
                    return Some((term, vec!["-e".into(), attach_cmd]));
                }
            }
        }
    }

    // 2. Fallback list
    let fallbacks = [
        ("ptyxis", vec!["-s", "--", "tmux", "attach", "-t", session]),
        (
            "gnome-terminal",
            vec!["--wait", "--", "tmux", "attach", "-t", session],
        ),
        (
            "konsole",
            vec!["--nofork", "-e", "tmux", "attach", "-t", session],
        ),
        (
            "xfce4-terminal",
            vec!["--disable-server", "-e", &attach_cmd],
        ),
        ("alacritty", vec!["-e", "tmux", "attach", "-t", session]),
        ("kitty", vec!["tmux", "attach", "-t", session]),
        ("xterm", vec!["-e", "tmux", "attach", "-t", session]),
    ];

    for (cmd, args) in fallbacks {
        if Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some((
                cmd.to_string(),
                args.into_iter().map(String::from).collect(),
            ));
        }
    }

    None
}

/// Close a terminal window by its handle.
/// macOS: closes the Terminal.app window by ID.
/// Linux: kills the terminal emulator process by PID.
/// Silently no-ops if the window/process no longer exists.
pub fn close_terminal_handle(handle: u32) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"Terminal\"\n\
             set matchingWindows to windows whose id is {handle}\n\
             if (count matchingWindows) > 0 then\n\
             close (first item of matchingWindows)\n\
             end if\n\
             end tell"
        );
        let _ = Command::new("osascript").args(["-e", &script]).status();
    }

    #[cfg(target_os = "linux")]
    {
        // Verify the PID is still a terminal emulator before killing
        if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{}/cmdline", handle)) {
            let is_terminal = cmdline.contains("terminal")
                || cmdline.contains("ptyxis")
                || cmdline.contains("konsole")
                || cmdline.contains("alacritty")
                || cmdline.contains("kitty")
                || cmdline.contains("xterm");

            if is_terminal {
                // Try SIGTERM first
                let _ = Command::new("kill")
                    .args(["-15", &handle.to_string()])
                    .status();

                // We could wait and SIGKILL, but for simplicity we just send SIGTERM.
                // The tmux session will be killed anyway, which usually causes the terminal to exit.
            }
        }
    }
}

/// Kill a tmux session.
pub fn kill_session(session: &str) {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", session])
        .status();
}

/// Kill the running process in the first pane of a tmux session and restart
/// it with a new command. The session and any attached terminals stay alive.
pub fn respawn_pane(session: &str, cmd: &str) -> Result<(), InjectionError> {
    let status = Command::new("tmux")
        .args(["respawn-pane", "-k", "-t", session, cmd])
        .status()
        .map_err(|e| InjectionError::TmuxCommand {
            step: "respawn-pane".into(),
            detail: e.to_string(),
        })?;
    if !status.success() {
        return Err(InjectionError::TmuxCommand {
            step: "respawn-pane".into(),
            detail: format!("exited with {status}"),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-bot interrupt key sequences
// ---------------------------------------------------------------------------

/// Bot-specific keys for interrupting an active generation and clearing
/// any partial input left on the command line.
pub struct InterruptKeys {
    /// tmux `send-keys` value to cancel the current generation.
    pub cancel: &'static str,
    /// tmux `send-keys` value to clear partial input after cancel.
    pub clear: &'static str,
    /// Milliseconds to wait after sending cancel before clearing / injecting.
    pub settle_ms: u64,
}

impl InterruptKeys {
    /// Derive the correct interrupt keys from an agent's `command` field.
    pub fn for_command(command: &str) -> Self {
        match command.split_whitespace().next().unwrap_or("") {
            "copilot" => InterruptKeys { cancel: "Escape", clear: "Escape", settle_ms: 2000 },
            "gemini"  => InterruptKeys { cancel: "C-c", clear: "C-c", settle_ms: 2000 },
            "cursor"  => InterruptKeys { cancel: "C-c", clear: "C-c", settle_ms: 2000 },
            _         => InterruptKeys { cancel: "C-c", clear: "C-u", settle_ms: 2000 },
        }
    }
}

/// Send a single tmux `send-keys` command to the given session.
pub fn send_keys(session: &str, keys: &str) -> Result<(), InjectionError> {
    let output = Command::new("tmux")
        .args(["send-keys", "-t", session, keys])
        .output()
        .map_err(|e| InjectionError::TmuxCommand {
            step: "send-keys".into(),
            detail: e.to_string(),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(InjectionError::TmuxCommand {
            step: "send-keys".into(),
            detail: format!(
                "exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
        })
    }
}

/// Interrupt the agent's current generation, wait for it to settle,
/// then inject the given text.  Single attempt, no retries.
pub fn inject_interrupt_once(session: &str, text: &str, keys: &InterruptKeys) -> Result<(), InjectionError> {
    // 1. Cancel current generation
    send_keys(session, keys.cancel)?;

    // 2. Wait for agent to process the interrupt
    std::thread::sleep(std::time::Duration::from_millis(keys.settle_ms));

    // 3. Clear any partial input left on the line
    send_keys(session, keys.clear)?;
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 4. Inject the payload
    inject_once(session, text)
}

/// Interrupt + inject with up to [`MAX_RETRIES`] attempts.
pub async fn inject_interrupt(
    session: &str,
    text: &str,
    keys: &InterruptKeys,
) -> Result<(), InjectionError> {
    let mut last_err = String::new();
    for attempt in 1..=MAX_RETRIES {
        match inject_interrupt_once(session, text, keys) {
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

// ---------------------------------------------------------------------------
// tmux injection primitives
// ---------------------------------------------------------------------------

/// Inject text into a tmux session (single attempt).
/// Uses send-keys which handles Enter naturally — the text is sent as
/// keyboard input followed by an Enter keystroke.
fn inject_once(session: &str, text: &str) -> Result<(), InjectionError> {
    // send-keys sends the text as literal keystrokes, then Enter submits it.
    // We use send-keys with -l (literal) to avoid interpreting special chars,
    // then a separate send-keys for Enter.
    let run_tmux =
        |args: &[&str]| -> Result<(), InjectionError> {
            let output = Command::new("tmux").args(args).output().map_err(|e| {
                InjectionError::TmuxCommand {
                    step: args[0].to_string(),
                    detail: e.to_string(),
                }
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
        run_tmux(&["paste-buffer", "-p", "-t", session])?;
        // Wait for the paste to land in the terminal before sending Enter
        std::thread::sleep(std::time::Duration::from_millis(1000));
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
    /// Spawn a tmux session. Returns the terminal handle if one was
    /// opened, or `None` (e.g. in tests or headless mode).
    fn spawn_session(&self, session: &str, cmd: &str) -> Result<Option<u32>, InjectionError>;
    /// Kill the running process inside the pane and restart it with a new
    /// command, keeping the tmux session (and any attached terminal) alive.
    fn respawn_pane(&self, session: &str, cmd: &str) -> Result<(), InjectionError>;
    /// Async inject — returns a boxed future so the trait is object-safe.
    fn inject<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>>;
    fn capture(&self, session: &str) -> Result<String, InjectionError>;
    /// Send a bare `send-keys` to the tmux session (e.g. for interrupt keys).
    fn send_keys(&self, session: &str, keys: &str) -> Result<(), InjectionError>;
    /// Interrupt the agent, wait for settle, then inject text.
    fn inject_interrupt<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
        keys: &'a InterruptKeys,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>>;
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
    fn respawn_pane(&self, session: &str, cmd: &str) -> Result<(), InjectionError> {
        respawn_pane(session, cmd)
    }
    fn inject<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>> {
        Box::pin(inject(session, text))
    }
    fn capture(&self, session: &str) -> Result<String, InjectionError> {
        capture(session)
    }
    fn send_keys(&self, session: &str, keys: &str) -> Result<(), InjectionError> {
        send_keys(session, keys)
    }
    fn inject_interrupt<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
        keys: &'a InterruptKeys,
    ) -> Pin<Box<dyn Future<Output = Result<(), InjectionError>> + Send + 'a>> {
        Box::pin(inject_interrupt(session, text, keys))
    }
}
