use std::fs;
use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_id() -> u64 {
    TEST_COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn setup_git_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sl_git_{}_{}", name, unique_id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".git")).unwrap();
    dir
}

fn strip_ansi(s: &str) -> String {
    s.chars()
        .fold((String::new(), false), |(mut s, in_escape), c| {
            if c == '\x1b' {
                (s, true)
            } else if in_escape {
                (s, c != 'm')
            } else {
                s.push(c);
                (s, false)
            }
        })
        .0
}

fn run_statusline_branch(cwd: &str) -> String {
    let transcript = std::env::temp_dir().join(format!("sl_git_transcript_{}.jsonl", unique_id()));
    fs::write(&transcript, "").unwrap();

    let input = format!(
        r#"{{"cwd":"{}","transcript_path":"{}","model":{{"display_name":"test"}}}}"#,
        cwd,
        transcript.to_string_lossy()
    );

    let output = Command::new(env!("CARGO_BIN_EXE_statusline"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("Failed to run statusline");

    let _ = fs::remove_file(&transcript);

    let stripped = strip_ansi(&String::from_utf8_lossy(&output.stdout));

    // Output format: "dir | branch | +0 -0 | model | tokens"
    let parts: Vec<&str> = stripped.trim().split(" | ").collect();
    parts.get(1).unwrap_or(&"").to_string()
}

#[test]
fn test_normal_branch() {
    let dir = setup_git_dir("normal");
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    assert_eq!(run_statusline_branch(dir.to_str().unwrap()), "main");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_feature_branch() {
    let dir = setup_git_dir("feature");
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/feature/my-branch\n").unwrap();
    assert_eq!(run_statusline_branch(dir.to_str().unwrap()), "feature/my-branch");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_detached_head() {
    let dir = setup_git_dir("detached");
    fs::write(dir.join(".git/HEAD"), "abc123def456\n").unwrap();
    assert_eq!(run_statusline_branch(dir.to_str().unwrap()), "detached");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_rebasing() {
    let dir = setup_git_dir("rebase");
    fs::write(dir.join(".git/HEAD"), "abc123def456\n").unwrap();
    fs::create_dir_all(dir.join(".git/rebase-merge")).unwrap();
    assert_eq!(run_statusline_branch(dir.to_str().unwrap()), "rebasing");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_rebase_apply() {
    let dir = setup_git_dir("rebase_apply");
    fs::write(dir.join(".git/HEAD"), "abc123def456\n").unwrap();
    fs::create_dir_all(dir.join(".git/rebase-apply")).unwrap();
    assert_eq!(run_statusline_branch(dir.to_str().unwrap()), "rebasing");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_no_git() {
    let dir = setup_git_dir("nogit");
    fs::remove_dir_all(dir.join(".git")).unwrap();
    assert_eq!(run_statusline_branch(dir.to_str().unwrap()), "no-git");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_worktree() {
    let dir = setup_git_dir("worktree");
    let worktree_gitdir = dir.join(".git/worktrees/my-worktree");
    fs::create_dir_all(&worktree_gitdir).unwrap();
    fs::write(worktree_gitdir.join("HEAD"), "ref: refs/heads/worktree-branch\n").unwrap();

    let worktree_dir = dir.join("worktree");
    fs::create_dir_all(&worktree_dir).unwrap();
    fs::write(worktree_dir.join(".git"), "gitdir: ../.git/worktrees/my-worktree").unwrap();

    assert_eq!(run_statusline_branch(worktree_dir.to_str().unwrap()), "worktree-branch");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_branch_from_subdirectory() {
    let dir = setup_git_dir("subdir");
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/feature\n").unwrap();
    let sub = dir.join("a").join("b").join("c");
    fs::create_dir_all(&sub).unwrap();
    assert_eq!(run_statusline_branch(sub.to_str().unwrap()), "feature");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_worktree_rebasing() {
    let dir = setup_git_dir("worktree_rebase");
    let worktree_gitdir = dir.join(".git/worktrees/my-worktree");
    fs::create_dir_all(&worktree_gitdir).unwrap();
    fs::write(worktree_gitdir.join("HEAD"), "abc123\n").unwrap();
    fs::create_dir_all(worktree_gitdir.join("rebase-merge")).unwrap();

    let worktree_dir = dir.join("worktree");
    fs::create_dir_all(&worktree_dir).unwrap();
    fs::write(worktree_dir.join(".git"), "gitdir: ../.git/worktrees/my-worktree").unwrap();

    assert_eq!(run_statusline_branch(worktree_dir.to_str().unwrap()), "rebasing");
    let _ = fs::remove_dir_all(&dir);
}
