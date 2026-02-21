use std::fs;
use std::path::Path;
use std::sync::Mutex;

use orchestrator::injector::{inject, spawn_session};
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

    let result = {
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

        let r = inject("demo-session", "hello world").await;

        match old_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        match old_tmux_log {
            Some(value) => std::env::set_var("TMUX_LOG", value),
            None => std::env::remove_var("TMUX_LOG"),
        }

        r
    };

    result.unwrap();

    let log = fs::read_to_string(log_path).unwrap();
    let lines: Vec<&str> = log.lines().collect();

    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("load-buffer "));
    assert_eq!(lines[1], "paste-buffer -p -t demo-session");
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

        let r = spawn_session("demo-session", "echo hi");

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
        "new-session -d -s demo-session -x 200 -y 50 echo hi"
    );
    assert_eq!(lines[1], "set-option -t demo-session mouse on");
    assert_eq!(lines[2], "set-option -t demo-session history-limit 100000");
    assert_eq!(lines[3], "set-option -t demo-session set-titles on");
    assert_eq!(lines[4], "set-option -t demo-session set-titles-string #S");
}
