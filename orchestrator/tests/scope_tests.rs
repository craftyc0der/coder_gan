use std::path::Path;

use orchestrator::scope::{has_text_extension, is_in_excluded_dir};

fn root() -> &'static Path {
    Path::new("/project")
}

#[test]
fn excluded_target_dir() {
    let path = Path::new("/project/target/file.rs");
    assert!(is_in_excluded_dir(path, root()));
}

#[test]
fn excluded_git_dir() {
    let path = Path::new("/project/.git/config");
    assert!(is_in_excluded_dir(path, root()));
}

#[test]
fn excluded_node_modules_dir() {
    let path = Path::new("/project/node_modules/pkg/index.js");
    assert!(is_in_excluded_dir(path, root()));
}

#[test]
fn excluded_orchestrator_dir() {
    let path = Path::new("/project/.orchestrator/runtime/logs/events.jsonl");
    assert!(is_in_excluded_dir(path, root()));
}

#[test]
fn normal_src_dir_is_not_excluded() {
    let path = Path::new("/project/src/lib.rs");
    assert!(!is_in_excluded_dir(path, root()));
}

#[test]
fn file_at_project_root_not_excluded() {
    let path = Path::new("/project/README.md");
    assert!(!is_in_excluded_dir(path, root()));
}

#[test]
fn nested_target_dir_excluded() {
    let path = Path::new("/project/target/debug/build/foo/bar.rs");
    assert!(is_in_excluded_dir(path, root()));
}

#[test]
fn component_contains_excluded_name_but_not_equal() {
    let path = Path::new("/project/my_target/foo.rs");
    assert!(!is_in_excluded_dir(path, root()));
}

#[test]
fn outside_project_root_not_excluded() {
    let path = Path::new("/other/target/file.rs");
    assert!(!is_in_excluded_dir(path, root()));
}

#[test]
fn has_text_extension_rs() {
    let path = Path::new("/project/src/main.rs");
    assert!(has_text_extension(path));
}

#[test]
fn has_text_extension_toml() {
    let path = Path::new("/project/Cargo.toml");
    assert!(has_text_extension(path));
}

#[test]
fn has_text_extension_md() {
    let path = Path::new("/project/README.md");
    assert!(has_text_extension(path));
}

#[test]
fn has_text_extension_json() {
    let path = Path::new("/project/data.json");
    assert!(has_text_extension(path));
}

#[test]
fn has_text_extension_py() {
    let path = Path::new("/project/script.py");
    assert!(has_text_extension(path));
}

#[test]
fn has_text_extension_sh() {
    let path = Path::new("/project/run.sh");
    assert!(has_text_extension(path));
}

#[test]
fn has_text_extension_all_recognised() {
    for ext in &["txt", "yaml", "yml", "js", "ts", "go", "html", "css"] {
        let path_str = format!("/project/file.{ext}");
        let path = Path::new(&path_str);
        assert!(has_text_extension(path), ".{ext} should be recognised");
    }
}

#[test]
fn no_extension_is_false() {
    let path = Path::new("/project/Makefile");
    assert!(!has_text_extension(path));
}

#[test]
fn unknown_extension_is_false() {
    let path = Path::new("/project/binary.exe");
    assert!(!has_text_extension(path));
    let path = Path::new("/project/blob.bin");
    assert!(!has_text_extension(path));
    let path = Path::new("/project/lockfile.lock");
    assert!(!has_text_extension(path));
}

#[test]
fn extension_check_is_case_sensitive() {
    let path = Path::new("/project/MAIN.RS");
    assert!(!has_text_extension(path));
    let path = Path::new("/project/README.MD");
    assert!(!has_text_extension(path));
}
