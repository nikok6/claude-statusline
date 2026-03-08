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
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    // CSI sequence: \x1b[...m
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if next == 'm' { break; }
                    }
                }
                Some(']') => {
                    // OSC sequence: \x1b]...ST (where ST is \x1b\\)
                    loop {
                        match chars.next() {
                            Some('\x1b') if chars.peek() == Some(&'\\') => {
                                chars.next();
                                break;
                            }
                            None => break,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn run_statusline_raw(cwd: &str) -> String {
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

    String::from_utf8_lossy(&output.stdout).to_string()
}

fn run_statusline_branch(cwd: &str) -> String {
    let raw = run_statusline_raw(cwd);
    let stripped = strip_ansi(&raw);
    let parts: Vec<&str> = stripped.trim().split(" | ").collect();
    parts.get(1).unwrap_or(&"").to_string()
}

fn extract_osc8_url(raw: &str) -> Option<String> {
    // Find first OSC 8 link: \x1b]8;;URL\x1b\\
    let marker = "\x1b]8;;";
    let start = raw.find(marker)? + marker.len();
    let end = raw[start..].find("\x1b\\")?;
    let url = &raw[start..start + end];
    if url.is_empty() { None } else { Some(url.to_string()) }
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

#[test]
fn test_remote_url_ssh() {
    let dir = setup_git_dir("remote_ssh");
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        dir.join(".git/config"),
        "[remote \"origin\"]\n\turl = git@github.com:user/repo.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
    ).unwrap();
    let raw = run_statusline_raw(dir.to_str().unwrap());
    assert_eq!(extract_osc8_url(&raw), Some("https://github.com/user/repo".to_string()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_remote_url_https() {
    let dir = setup_git_dir("remote_https");
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        dir.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://github.com/user/repo.git\n",
    ).unwrap();
    let raw = run_statusline_raw(dir.to_str().unwrap());
    assert_eq!(extract_osc8_url(&raw), Some("https://github.com/user/repo".to_string()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_remote_url_https_no_dotgit() {
    let dir = setup_git_dir("remote_nodotgit");
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        dir.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://github.com/user/repo\n",
    ).unwrap();
    let raw = run_statusline_raw(dir.to_str().unwrap());
    assert_eq!(extract_osc8_url(&raw), Some("https://github.com/user/repo".to_string()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_no_remote_no_link() {
    let dir = setup_git_dir("no_remote");
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(dir.join(".git/config"), "[core]\n\tbare = false\n").unwrap();
    let raw = run_statusline_raw(dir.to_str().unwrap());
    assert_eq!(extract_osc8_url(&raw), None);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_no_git_no_link() {
    let dir = setup_git_dir("nogit_link");
    fs::remove_dir_all(dir.join(".git")).unwrap();
    let raw = run_statusline_raw(dir.to_str().unwrap());
    assert_eq!(extract_osc8_url(&raw), None);
    let _ = fs::remove_dir_all(&dir);
}
