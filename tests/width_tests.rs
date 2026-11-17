//! `$COLUMNS`-aware fitting: dir/branch shrink so later fields stay visible.

use std::fs;
use std::io::Write;
use std::process::Command;

const DIR: &str = "slice-pba-transaction-service";
const BRANCH: &str = "fix/unlinked-auth-clearing-reversal";

fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' { out.push(c); continue; }
        match it.next() {
            Some('[') => { for n in it.by_ref() { if n == 'm' { break; } } }
            Some(']') => {
                while let Some(n) = it.next() {
                    if n == '\x1b' && it.peek() == Some(&'\\') { it.next(); break; }
                }
            }
            _ => {}
        }
    }
    out
}

/// Raw stdout, run in a sandbox HOME (default config unless `config` is
/// given) with a git repo that has an origin remote. `columns: None` runs
/// without `$COLUMNS`.
fn run(name: &str, columns: Option<&str>, config: Option<&str>) -> String {
    let id = format!("sl_width_{}_{}", name, std::process::id());
    let root = std::env::temp_dir().join(&id);
    let _ = fs::remove_dir_all(&root);
    let repo = root.join(DIR);
    let home = root.join("home");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(repo.join(".git/HEAD"), format!("ref: refs/heads/{}\n", BRANCH)).unwrap();
    fs::write(
        repo.join(".git/config"),
        "[remote \"origin\"]\n\turl = git@github.com:user/repo.git\n",
    ).unwrap();
    if let Some(cfg) = config {
        fs::write(home.join(".claude/statusline.json"), cfg).unwrap();
    }
    // Unique stem: the binary caches per transcript stem in /tmp.
    let transcript = root.join(format!("{id}.jsonl"));
    fs::write(&transcript, "").unwrap();

    let input = format!(
        r#"{{"cwd":"{}","transcript_path":"{}","model":{{"display_name":"Opus"}},
            "context_window":{{"context_window_size":200000,"current_usage":{{"input_tokens":64000,
            "output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#,
        repo.display(), transcript.display()
    );

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_statusline"));
    cmd.env("HOME", &home).env_remove("COLUMNS");
    if let Some(c) = columns { cmd.env("COLUMNS", c); }
    let output = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("failed to run statusline");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_file(format!("/tmp/statusline_cache_{id}.json"));

    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// First output line, ANSI stripped, default config.
fn first_line(name: &str, columns: Option<&str>) -> String {
    strip(run(name, columns, None).lines().next().unwrap_or(""))
}

#[test]
fn untouched_without_columns() {
    let line = first_line("unset", None);
    assert!(line.contains(DIR) && line.contains(BRANCH), "{line}");
}

#[test]
fn untouched_when_wide_enough() {
    let line = first_line("wide", Some("200"));
    assert!(line.contains(DIR) && line.contains(BRANCH), "{line}");
}

#[test]
fn shrinks_dir_and_branch_to_fit_and_keeps_tokens() {
    let line = first_line("narrow", Some("80"));
    // default right_margin is 4
    assert!(line.chars().count() <= 76, "{} cells: {line}", line.chars().count());
    assert!(line.ends_with("64k/200k"), "{line}");
    assert!(line.starts_with("slice-") && line.contains("fix/"), "{line}");
    assert_eq!(line.matches('…').count(), 2, "{line}");
}

#[test]
fn ignores_garbage_columns() {
    let line = first_line("garbage", Some("abc"));
    assert!(line.contains(DIR) && line.contains(BRANCH), "{line}");
}

#[test]
fn custom_right_margin_is_honoured() {
    let cfg = r#"{"lines":[{"fields":["dir","branch","diff","model","tokens"],"separator":"|"}],"right_margin":20}"#;
    let line = strip(run("margin", Some("100"), Some(cfg)).lines().next().unwrap());
    assert!(line.chars().count() <= 80, "{} cells: {line}", line.chars().count());
    assert!(line.contains('…'), "{line}");
}

#[test]
fn line_without_dir_or_branch_is_left_alone() {
    let cfg = r#"{"lines":[{"fields":["model","tokens"],"separator":"|"}]}"#;
    let wide = run("nodir_wide", None, Some(cfg));
    let narrow = run("nodir_narrow", Some("10"), Some(cfg));
    assert_eq!(wide, narrow);
}

#[test]
fn hyperlink_survives_truncation() {
    let raw = run("link", Some("80"), None);
    let first = raw.lines().next().unwrap();
    assert!(first.contains("\x1b]8;;https://github.com/user/repo\x1b\\"), "{first:?}");
    assert!(strip(first).starts_with("slice-"), "{first:?}");
    assert!(strip(first).contains('…'), "{first:?}");
}
