use std::process::Command;

use orchestrator::worktree::{ensure_dot_orchestrator_symlink, setup_worktrees, WorktreeSpec};
use tempfile::TempDir;

fn run_git(repo: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn setup_worktrees_symlinks_dot_orchestrator_into_worktree() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();

    run_git(repo, &["init"]);
    run_git(repo, &["config", "user.email", "test@example.com"]);
    run_git(repo, &["config", "user.name", "Test User"]);

    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    run_git(repo, &["add", "README.md"]);
    run_git(repo, &["commit", "-m", "initial"]);

    let dot = repo.join(".orchestrator");
    std::fs::create_dir_all(dot.join("prompts")).unwrap();
    std::fs::create_dir_all(dot.join("messages/to_coder")).unwrap();
    std::fs::create_dir_all(dot.join("runtime/logs")).unwrap();
    std::fs::write(dot.join("agents.toml"), "[[agents]]\nid = \"coder\"\n").unwrap();
    std::fs::write(dot.join("prompts/coder.md"), "prompt\n").unwrap();

    let specs = vec![WorktreeSpec {
        worktree_id: "reviewer".into(),
        agent_ids: vec!["reviewer".into()],
        branch_override: None,
    }];

    let results = setup_worktrees(repo, "feature-x", &specs).unwrap();
    let worktree_root = &results[0].worktree_path;
    let worktree_dot = worktree_root.join(".orchestrator");

    #[cfg(unix)]
    {
        let dot_meta = std::fs::symlink_metadata(&worktree_dot).unwrap();
        assert!(dot_meta.file_type().is_symlink());

        let dot_target = std::fs::read_link(&worktree_dot).unwrap();
        assert_eq!(dot_target, dot);
    }
}

#[test]
fn ensure_dot_orchestrator_symlink_repairs_replaced_path() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    let worktree_root = repo.join("worktree");
    let source_dot = repo.join(".orchestrator");

    std::fs::create_dir_all(&source_dot).unwrap();
    std::fs::create_dir_all(&worktree_root).unwrap();

    let replaced = worktree_root.join(".orchestrator");
    std::fs::create_dir_all(&replaced).unwrap();
    std::fs::write(replaced.join("junk.txt"), "junk\n").unwrap();

    ensure_dot_orchestrator_symlink(&source_dot, &worktree_root).unwrap();

    #[cfg(unix)]
    {
        let metadata = std::fs::symlink_metadata(&replaced).unwrap();
        assert!(metadata.file_type().is_symlink());
        assert_eq!(std::fs::read_link(&replaced).unwrap(), source_dot);
    }
}
