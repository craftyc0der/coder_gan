use std::future::Future;
use std::pin::Pin;
use std::process::Command;
#[cfg(target_os = "macos")]
use std::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::config::{SplitDirection, TerminalPreference};

const MAX_RETRIES: u32 = 3;
const RETRY_BACKOFF_SECS: u64 = 1;

#[cfg(target_os = "macos")]
static ITERM2_WINDOW_ID: Mutex<Option<u32>> = Mutex::new(None);

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

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        "''".to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn wrap_for_tmux_shell(cmd: &str) -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    format!(
        "exec {} -l -c {}",
        shell_quote(&shell),
        shell_quote(cmd)
    )
}

#[cfg(target_os = "macos")]
fn iterm2_attach_command(session: &str) -> String {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|value| value.starts_with('/'))
        .unwrap_or_else(|| "/bin/zsh".to_string());
    format!(
        "{} -l -c {}",
        shell,
        shell_quote(&format!("exec tmux attach -t {session}"))
    )
}

#[cfg(target_os = "macos")]
fn iterm2_open_window_script(session: &str) -> String {
    let attach_cmd = iterm2_attach_command(session);
    format!(
        "tell application \"iTerm2\"\n\
         launch\n\
         set newWindow to (create window with default profile command \"{attach_cmd}\")\n\
         activate\n\
         return id of newWindow\n\
         end tell"
    )
}

#[cfg(target_os = "macos")]
fn iterm2_open_tab_script(window_id: u32, session: &str) -> String {
    let attach_cmd = iterm2_attach_command(session);
    format!(
        "tell application \"iTerm2\"\n\
         launch\n\
         tell window id {window_id}\n\
         create tab with default profile command \"{attach_cmd}\"\n\
         end tell\n\
         activate\n\
         return {window_id}\n\
         end tell"
    )
}

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
pub fn spawn_session(session: &str, cmd: &str, terminal: &TerminalPreference) -> Result<Option<u32>, InjectionError> {
    let wrapped_cmd = wrap_for_tmux_shell(cmd);

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
            &wrapped_cmd,
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

    Ok(open_terminal_window(session, terminal))
}

/// Spawn a detached tmux session with multiple panes for a worker group.
///
/// The first command in `cmds` creates the initial pane (window 0, pane 0).
/// Each subsequent command is added via `split-window` using the specified
/// layout direction.  Returns the terminal window handle (same semantics as
/// `spawn_session`).
pub fn spawn_group_session(
    session: &str,
    cmds: &[&str],
    layout: &SplitDirection,
    terminal: &TerminalPreference,
) -> Result<Option<u32>, InjectionError> {
    if cmds.is_empty() {
        return Err(InjectionError::TmuxCommand {
            step: "spawn_group_session".into(),
            detail: "no commands provided".into(),
        });
    }

    let wrapped_first_cmd = wrap_for_tmux_shell(cmds[0]);

    // Create the session with the first command
    let status = Command::new("tmux")
        .args(["new-session", "-d", "-s", session, "-x", "220", "-y", "50", &wrapped_first_cmd])
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

    // Add subsequent panes via split-window
    let split_flag = match layout {
        SplitDirection::Horizontal => "-h", // left|right
        SplitDirection::Vertical => "-v",   // top|bottom
    };
    for cmd in cmds.iter().skip(1) {
        let wrapped_cmd = wrap_for_tmux_shell(cmd);
        let status = Command::new("tmux")
            .args(["split-window", split_flag, "-t", session, &wrapped_cmd])
            .status()
            .map_err(|e| InjectionError::TmuxCommand {
                step: "split-window".into(),
                detail: e.to_string(),
            })?;
        if !status.success() {
            return Err(InjectionError::TmuxCommand {
                step: "split-window".into(),
                detail: format!("exited with {status}"),
            });
        }
    }

    // Balance pane sizes
    let layout_name = match layout {
        SplitDirection::Horizontal => "even-horizontal",
        SplitDirection::Vertical => "even-vertical",
    };
    let _ = Command::new("tmux")
        .args(["select-layout", "-t", session, layout_name])
        .status();

    // Apply session options (same as spawn_session)
    for (opt, val) in &[
        ("mouse", "on"),
        ("history-limit", "100000"),
        ("set-titles", "on"),
        ("set-titles-string", "#S"),
    ] {
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session, opt, val])
            .status();
    }

    Ok(open_terminal_window(session, terminal))
}

/// Open a visible terminal window attached to the given tmux session.
/// Returns a platform-specific handle (window ID on macOS, PID on Linux).
///
/// The `terminal` parameter controls which terminal emulator is used:
/// - `Auto`: Terminal.app on macOS, auto-detect on Linux.
/// - `Iterm2`: iTerm2 with native tmux integration (`tmux -CC`). macOS only.
/// - `Terminal`: Explicitly Terminal.app on macOS.
pub fn open_terminal_window(session: &str, terminal: &TerminalPreference) -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        if *terminal == TerminalPreference::Iterm2 {
            return open_iterm2_window(session);
        }

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
        let _ = terminal; // iTerm2 not available on Linux; ignore preference
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
        let _ = terminal;
        None
    }
}

/// Open an iTerm2 tab or window attached to a tmux session.
///
/// Uses AppleScript to tell iTerm2 to create a new window running the
/// tmux attach command inside the user's login shell. The tmux server-side session is
/// unchanged — all inject/capture operations continue to work normally.
///
/// Returns `None` on success because iTerm2 tabs may share a single window and
/// we do not want per-agent restarts to close every shared tab/window.
#[cfg(target_os = "macos")]
fn open_iterm2_window(session: &str) -> Option<u32> {
    let mut stored_window_id = ITERM2_WINDOW_ID.lock().ok()?;

    if let Some(window_id) = *stored_window_id {
        let script = iterm2_open_tab_script(window_id, session);
        if let Ok(status) = Command::new("osascript").args(["-e", &script]).output() {
            if status.status.success() {
                return None;
            }
        }
    }

    let script = iterm2_open_window_script(session);
    let status = Command::new("osascript").args(["-e", &script]).output().ok()?;

    if !status.status.success() {
        eprintln!(
            "[injector] iTerm2 AppleScript failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        );
        return None;
    }

    let window_id = String::from_utf8_lossy(&status.stdout)
        .trim()
        .parse::<u32>()
        .ok();
    *stored_window_id = window_id;

    // iTerm2 tabs share a single window, so we intentionally do not persist a
    // close handle for per-agent lifecycle management.
    None
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
/// macOS: tries both iTerm2 and Terminal.app to close the window by ID.
/// Linux: kills the terminal emulator process by PID.
/// Silently no-ops if the window/process no longer exists.
pub fn close_terminal_handle(handle: u32) {
    #[cfg(target_os = "macos")]
    {
        // Try iTerm2 first, then Terminal.app. One will match, the other
        // will silently no-op because the window ID won't be found.
        let iterm_script = format!(
            "tell application \"iTerm2\"\n\
             repeat with w in windows\n\
             if id of w is {handle} then\n\
             close w\n\
             return\n\
             end if\n\
             end repeat\n\
             end tell"
        );
        let _ = Command::new("osascript").args(["-e", &iterm_script]).status();

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

/// Send a native OS desktop notification if a supported notification tool is available.
///
/// - macOS  : uses `osascript` (always present)
/// - Linux  : tries `notify-send` (libnotify); silently skips if not installed
///
/// Failures are ignored — this is a best-effort side channel on top of the
/// existing tmux colour shift.
pub fn send_os_notification(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification {:?} with title {:?}",
            body, title
        );
        let _ = Command::new("osascript").args(["-e", &script]).status();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("notify-send")
            .args(["--urgency=critical", "--app-name=orchestrator", title, body])
            .status();
    }
}

/// Set a visual "needs attention" style on a tmux pane to alert the operator.
///
/// Applies three layers of visibility:
/// 1. Pane background — dark red tint on the specific pane content area
/// 2. Status bar      — red status bar on the session (visible in tab/title)
/// 3. Window rename   — "[⚠ INPUT NEEDED]" in the window title
///
/// Silently ignores failures; this is best-effort visual alerting.
pub fn set_pane_attention_style(target: &str, session: &str) {
    // 1. Dark red pane background (colour52 = dark red)
    let _ = Command::new("tmux")
        .args(["select-pane", "-t", target, "-P", "bg=colour52"])
        .status();

    // 2. Red status bar on the session
    let _ = Command::new("tmux")
        .args(["set-option", "-t", session, "status-style", "bg=red,fg=white,bold"])
        .status();

    // 3. Window title
    let _ = Command::new("tmux")
        .args(["rename-window", "-t", &format!("{}:0", session), "[⚠ INPUT NEEDED]"])
        .status();
}

/// Clear the "needs attention" visual style and restore session defaults.
///
/// Silently ignores failures.
pub fn clear_pane_attention_style(target: &str, session: &str) {
    // 1. Clear pane background
    let _ = Command::new("tmux")
        .args(["select-pane", "-t", target, "-P", ""])
        .status();

    // 2. Reset status bar to session default
    let _ = Command::new("tmux")
        .args(["set-option", "-u", "-t", session, "status-style"])
        .status();

    // 3. Re-enable automatic window renaming
    let _ = Command::new("tmux")
        .args(["set-window-option", "-t", &format!("{}:0", session), "automatic-rename", "on"])
        .status();
}

/// Check whether a specific tmux pane is still alive (its process has not exited).
///
/// Uses the `#{pane_dead}` format variable: "0" = alive, "1" = dead.
/// Returns `false` if the target doesn't exist or the command fails.
pub fn is_pane_alive(target: &str) -> bool {
    Command::new("tmux")
        .args(["display-message", "-t", target, "-p", "#{pane_dead}"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
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
    let wrapped_cmd = wrap_for_tmux_shell(cmd);
    let status = Command::new("tmux")
        .args(["respawn-pane", "-k", "-t", session, &wrapped_cmd])
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
        // Paste without -p (bracketed paste mode) — some agents like codex
        // don't process Enter correctly when bracketed paste is active.
        run_tmux(&["paste-buffer", "-t", session])?;
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
    fn spawn_session(&self, session: &str, cmd: &str, terminal: &TerminalPreference) -> Result<Option<u32>, InjectionError>;
    /// Spawn a tmux session with multiple panes for a worker group.
    /// The first command in `cmds` gets pane 0; subsequent commands are split
    /// in the given direction.  Returns the terminal window handle.
    fn spawn_group_session(
        &self,
        session: &str,
        cmds: &[&str],
        layout: &SplitDirection,
        terminal: &TerminalPreference,
    ) -> Result<Option<u32>, InjectionError>;
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
    /// Check whether a specific pane is still alive within a tmux session.
    fn is_pane_alive(&self, target: &str) -> bool;
    /// Apply the "needs attention" visual style to a pane (best-effort, no error).
    fn set_pane_attention_style(&self, target: &str, session: &str);
    /// Clear the "needs attention" visual style and restore defaults (best-effort).
    fn clear_pane_attention_style(&self, target: &str, session: &str);
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
    fn spawn_session(&self, session: &str, cmd: &str, terminal: &TerminalPreference) -> Result<Option<u32>, InjectionError> {
        spawn_session(session, cmd, terminal)
    }
    fn spawn_group_session(
        &self,
        session: &str,
        cmds: &[&str],
        layout: &SplitDirection,
        terminal: &TerminalPreference,
    ) -> Result<Option<u32>, InjectionError> {
        spawn_group_session(session, cmds, layout, terminal)
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
    fn is_pane_alive(&self, target: &str) -> bool {
        is_pane_alive(target)
    }
    fn set_pane_attention_style(&self, target: &str, session: &str) {
        set_pane_attention_style(target, session);
    }
    fn clear_pane_attention_style(&self, target: &str, session: &str) {
        clear_pane_attention_style(target, session);
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

#[cfg(test)]
mod tests {
    use super::wrap_for_tmux_shell;

    #[cfg(target_os = "macos")]
    use super::{iterm2_attach_command, iterm2_open_tab_script, iterm2_open_window_script};

    #[test]
    fn wrap_for_tmux_shell_uses_login_shell_and_quotes_command() {
        let wrapped = wrap_for_tmux_shell("cd '/tmp/a b' && codex");

        assert!(wrapped.contains(" -l -c "));
        assert!(wrapped.contains("codex"));
        assert!(wrapped.contains("cd '"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn iterm2_attach_command_uses_login_shell_for_tmux_cc() {
        let wrapped = iterm2_attach_command("demo-session");

        assert!(wrapped.starts_with('/'));
        assert!(wrapped.contains(" -l -c "));
        assert!(wrapped.contains("exec tmux attach -t demo-session"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn iterm2_open_window_script_launches_before_creating_window() {
        let script = iterm2_open_window_script("demo-session");
        let launch_idx = script.find("launch").unwrap();
        let create_idx = script.find("create window").unwrap();

        assert!(script.contains("tell application \"iTerm2\""));
        assert!(script.contains("launch"));
        assert!(script.contains("create window with default profile command"));
        assert!(script.contains("activate"));
        assert!(launch_idx < create_idx);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn iterm2_open_tab_script_targets_specific_window() {
        let script = iterm2_open_tab_script(42, "demo-session");

        assert!(script.contains("tell window id 42"));
        assert!(script.contains("create tab with default profile command"));
        assert!(script.contains("exec tmux attach -t demo-session"));
    }
}
