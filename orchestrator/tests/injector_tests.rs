use std::fs;
use std::path::Path;
use std::sync::Mutex;

#[cfg(target_os = "linux")]
use orchestrator::injector::detect_terminal_emulator;
use orchestrator::config::TerminalPreference;
use orchestrator::injector::{close_terminal_handle, inject, open_terminal_window, spawn_session};
use tempfile::TempDir;

// Serialize PATH/TMUX_LOG manipulation across tests to prevent race conditions.
// Both tests manipulate global process env vars, so they must not run concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[tokio::test]
async fn inject_uses_bracketed_paste_and_enter() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let tmux_path = bin_dir.join("tmux");
    let log_path = tmp.path().join("tmux.log");

    fs::write(
        &tmux_path,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\n' \"$*\" >> \"$TMUX_LOG\"\n",
    )
    .unwrap();
    make_executable(&tmux_path);

    let (old_path, old_tmux_log) = {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_path = std::env::var("PATH").ok();
        let old_tmux_log = std::env::var("TMUX_LOG").ok();
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                old_path.as_deref().unwrap_or("")
            ),
        );
        std::env::set_var("TMUX_LOG", &log_path);
        (old_path, old_tmux_log)
    };

    let result = inject("demo-session", "hello world").await;

    {
        let _guard = ENV_LOCK.lock().unwrap();
        match old_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        match old_tmux_log {
            Some(value) => std::env::set_var("TMUX_LOG", value),
            None => std::env::remove_var("TMUX_LOG"),
        }
    }

    result.unwrap();

    let log = fs::read_to_string(log_path).unwrap();
    let lines: Vec<&str> = log.lines().collect();

    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("load-buffer "));
    assert_eq!(lines[1], "paste-buffer -t demo-session");
    assert_eq!(lines[2], "send-keys -t demo-session Enter");
}

#[test]
fn spawn_session_enables_mouse_and_history_scrollback() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let tmux_path = bin_dir.join("tmux");
    let log_path = tmp.path().join("tmux.log");

    fs::write(
        &tmux_path,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\n' \"$*\" >> \"$TMUX_LOG\"\n",
    )
    .unwrap();
    make_executable(&tmux_path);

    let result = {
        let _guard = ENV_LOCK.lock().unwrap();

        let old_path = std::env::var("PATH").ok();
        let old_tmux_log = std::env::var("TMUX_LOG").ok();
        let old_display = std::env::var("DISPLAY").ok();
        let old_wayland = std::env::var("WAYLAND_DISPLAY").ok();

        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                old_path.as_deref().unwrap_or("")
            ),
        );
        std::env::set_var("TMUX_LOG", &log_path);
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");

        let r = spawn_session("demo-session", "echo hi", &TerminalPreference::Auto);

        match old_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        match old_tmux_log {
            Some(value) => std::env::set_var("TMUX_LOG", value),
            None => std::env::remove_var("TMUX_LOG"),
        }
        match old_display {
            Some(value) => std::env::set_var("DISPLAY", value),
            None => std::env::remove_var("DISPLAY"),
        }
        match old_wayland {
            Some(value) => std::env::set_var("WAYLAND_DISPLAY", value),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }

        r
    };

    assert!(result.is_ok());

    let log = fs::read_to_string(log_path).unwrap();
    let lines: Vec<&str> = log.lines().collect();

    assert_eq!(lines.len(), 5);
    assert_eq!(
        lines[0],
        "new-session -d -s demo-session -x 200 -y 50 exec '/bin/zsh' -l -c 'echo hi'"
    );
    assert_eq!(lines[1], "set-option -t demo-session mouse on");
    assert_eq!(lines[2], "set-option -t demo-session history-limit 100000");
    assert_eq!(lines[3], "set-option -t demo-session set-titles on");
    assert_eq!(lines[4], "set-option -t demo-session set-titles-string #S");
}

#[test]
#[cfg(target_os = "linux")]
fn detect_terminal_emulator_with_terminal_env() {
    let _guard = ENV_LOCK.lock().unwrap();

    let orig_term = std::env::var("TERMINAL");

    std::env::set_var("TERMINAL", "gnome-terminal");
    let result = detect_terminal_emulator("test-session");
    assert!(result.is_some());
    let (cmd, args) = result.unwrap();
    assert_eq!(cmd, "gnome-terminal");
    assert_eq!(args[0], "--wait");

    std::env::set_var("TERMINAL", "my-custom-term");
    let result = detect_terminal_emulator("test-session");
    assert!(result.is_some());
    let (cmd, args) = result.unwrap();
    assert_eq!(cmd, "my-custom-term");
    assert_eq!(args[0], "-e");

    match orig_term {
        Ok(val) => std::env::set_var("TERMINAL", val),
        Err(_) => std::env::remove_var("TERMINAL"),
    }
}

#[test]
#[cfg(target_os = "linux")]
fn open_terminal_window_headless() {
    let _guard = ENV_LOCK.lock().unwrap();

    let orig_display = std::env::var_os("DISPLAY");
    let orig_wayland = std::env::var_os("WAYLAND_DISPLAY");

    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");

    let result = open_terminal_window("test-session", &TerminalPreference::Auto);
    assert_eq!(result, None);

    if let Some(val) = orig_display {
        std::env::set_var("DISPLAY", val);
    }
    if let Some(val) = orig_wayland {
        std::env::set_var("WAYLAND_DISPLAY", val);
    }
}

#[test]
#[cfg(target_os = "linux")]
fn close_terminal_handle_linux_identity_verification() {
    let my_pid = std::process::id();
    close_terminal_handle(my_pid);
    assert!(true);
}

#[test]
#[cfg(target_os = "macos")]
fn open_terminal_window_macos_returns_none_on_osascript_failure() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let osascript_path = bin_dir.join("osascript");
    fs::write(&osascript_path, "#!/usr/bin/env bash\nexit 1\n").unwrap();
    make_executable(&osascript_path);

    let result = {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_path = std::env::var("PATH").ok();

        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                old_path.as_deref().unwrap_or("")
            ),
        );

        let r = open_terminal_window("test-session", &TerminalPreference::Auto);

        match old_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }

        r
    };

    assert_eq!(result, None);
}

#[test]
#[cfg(target_os = "macos")]
fn open_terminal_window_macos_parses_window_id() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let osascript_path = bin_dir.join("osascript");
    fs::write(&osascript_path, "#!/usr/bin/env bash\necho 42\n").unwrap();
    make_executable(&osascript_path);

    let result = {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_path = std::env::var("PATH").ok();

        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                old_path.as_deref().unwrap_or("")
            ),
        );

        let r = open_terminal_window("test-session", &TerminalPreference::Auto);

        match old_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }

        r
    };

    assert_eq!(result, Some(42));
}

#[test]
#[cfg(target_os = "macos")]
fn close_terminal_handle_macos_noop_for_nonexistent_window() {
    let _guard = ENV_LOCK.lock().unwrap();
    close_terminal_handle(999999);
}
