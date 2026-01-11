use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_id() -> u64 {
    TEST_COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn create_test_transcript(entries: &[&str]) -> String {
    let tmp_dir = std::env::temp_dir().join("statusline_test");
    let _ = fs::create_dir_all(&tmp_dir);
    let path = tmp_dir.join(format!("test_{}_{}.jsonl", std::process::id(), unique_id()));
    let mut file = File::create(&path).unwrap();
    for entry in entries {
        writeln!(file, "{}", entry).unwrap();
    }
    path.to_string_lossy().to_string()
}

fn write_entry(file_path: &str, original: &str, content: &str) -> String {
    // Use serde_json to properly escape strings
    let obj = serde_json::json!({
        "toolUseResult": {
            "filePath": file_path,
            "originalFile": original,
            "content": content
        }
    });
    serde_json::to_string(&obj).unwrap()
}

fn edit_entry(file_path: &str, old_str: &str, new_str: &str) -> String {
    let obj = serde_json::json!({
        "toolUseResult": {
            "filePath": file_path,
            "oldString": old_str,
            "newString": new_str
        }
    });
    serde_json::to_string(&obj).unwrap()
}

fn run_statusline(transcript_path: &str, test_file: &str) -> (usize, usize) {
    // Create a minimal input JSON
    let input = format!(
        r#"{{"cwd":"/tmp","transcript_path":"{}","model":{{"display_name":"test"}}}}"#,
        transcript_path
    );

    let output = Command::new(env!("CARGO_BIN_EXE_statusline"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to run statusline");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Strip ANSI codes for parsing
    let stripped: String = stdout
        .chars()
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
        .0;

    // Parse +N and -N from output like "main | +5 -2 | test | ..."
    let mut added = 0;
    let mut removed = 0;

    for (i, c) in stripped.chars().enumerate() {
        if c == '+' {
            let num: String = stripped[i+1..].chars().take_while(|c| c.is_numeric()).collect();
            if let Ok(n) = num.parse() {
                added = n;
            }
        } else if c == '-' {
            let num: String = stripped[i+1..].chars().take_while(|c| c.is_numeric()).collect();
            if !num.is_empty() {
                if let Ok(n) = num.parse() {
                    removed = n;
                }
            }
        }
    }

    let _ = fs::remove_file(test_file);
    (added, removed)
}

#[test]
fn test_write_new_file() {
    let test_file = std::env::temp_dir().join(format!("test_write_new_{}.txt", unique_id()));
    let content = "line1\nline2\nline3\nline4\nline5\n";
    fs::write(&test_file, content).unwrap();

    let transcript = create_test_transcript(&[
        &write_entry(test_file.to_str().unwrap(), "", content),
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    assert_eq!(added, 5, "Should add 5 lines");
    assert_eq!(removed, 0, "Should remove 0 lines");
}

#[test]
fn test_edit_adds_lines() {
    let test_file = std::env::temp_dir().join(format!("test_edit_add_{}.txt", unique_id()));
    fs::write(&test_file, "exists").unwrap();

    let transcript = create_test_transcript(&[
        &edit_entry(test_file.to_str().unwrap(), "line2\n", "line2\nline3\nline4\nline5\n"),
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    assert_eq!(added, 3, "Should add 3 lines");
    assert_eq!(removed, 0, "Should remove 0 lines");
}

#[test]
fn test_edit_removes_lines() {
    let test_file = std::env::temp_dir().join(format!("test_edit_remove_{}.txt", unique_id()));
    fs::write(&test_file, "exists").unwrap();

    let transcript = create_test_transcript(&[
        &edit_entry(test_file.to_str().unwrap(), "line1\nline2\nline3\n", "line1\n"),
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    assert_eq!(added, 0, "Should add 0 lines");
    assert_eq!(removed, 2, "Should remove 2 lines");
}

#[test]
fn test_write_then_delete_is_zero() {
    let test_file = std::env::temp_dir().join(format!("test_write_delete_{}.txt", unique_id()));
    // Don't create the file - simulating deletion
    // Make sure it doesn't exist from previous runs
    let _ = fs::remove_file(&test_file);

    let transcript = create_test_transcript(&[
        &write_entry(test_file.to_str().unwrap(), "", "line1\nline2\nline3\n"),
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    assert_eq!(added, 0, "Should add 0 lines (file deleted)");
    assert_eq!(removed, 0, "Should remove 0 lines (file deleted)");
}

#[test]
fn test_multiple_writes_same_file() {
    let test_file = std::env::temp_dir().join(format!("test_multi_write_{}.txt", unique_id()));
    let final_content = "a\nb\nc\nd\ne\n";
    fs::write(&test_file, final_content).unwrap();

    let transcript = create_test_transcript(&[
        &write_entry(test_file.to_str().unwrap(), "", "a\nb\n"),
        &write_entry(test_file.to_str().unwrap(), "a\nb\n", "a\nb\nc\n"),
        &write_entry(test_file.to_str().unwrap(), "a\nb\nc\n", final_content),
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    // Original was "", final is 5 lines -> +5
    assert_eq!(added, 5, "Should add 5 lines total");
    assert_eq!(removed, 0, "Should remove 0 lines");
}

#[test]
fn test_write_and_edit_combined() {
    let test_file = std::env::temp_dir().join(format!("test_write_edit_{}.txt", unique_id()));
    fs::write(&test_file, "a\nb\nc\nd\ne\n").unwrap();

    let transcript = create_test_transcript(&[
        &write_entry(test_file.to_str().unwrap(), "", "a\nb\nc\n"),
        &edit_entry(test_file.to_str().unwrap(), "c\n", "c\nd\ne\n"),
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    // Write: +3 (empty -> 3 lines)
    // Edit: +2 (c -> c,d,e)
    assert_eq!(added, 5, "Should add 5 lines total");
    assert_eq!(removed, 0, "Should remove 0 lines");
}

#[test]
fn test_multi_session_interference() {
    // Scenario: Write 5 lines, another session adds 5 lines (not in transcript),
    // then we edit to add 5 more lines → should be +10 (only our changes)
    let test_file = std::env::temp_dir().join(format!("test_multi_session_{}.txt", unique_id()));

    // Final state on disk: 15 lines (our 5 + other session's 5 + our edit's 5)
    // But we simulate this by creating the file with 15 lines
    let final_on_disk = "line1\nline2\nline3\nline4\nline5\nother1\nother2\nother3\nother4\nother5\nedit1\nedit2\nedit3\nedit4\nedit5\n";
    fs::write(&test_file, final_on_disk).unwrap();

    // Our transcript only has:
    // 1. Write 5 lines (original empty)
    // 2. Edit adding 5 lines (oldString->newString delta)
    let transcript = create_test_transcript(&[
        &write_entry(test_file.to_str().unwrap(), "", "line1\nline2\nline3\nline4\nline5\n"),
        // Edit: other session added lines, then we add 5 more
        &edit_entry(test_file.to_str().unwrap(), "other5\n", "other5\nedit1\nedit2\nedit3\nedit4\nedit5\n"),
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    // Our changes: Write +5, Edit +5 = +10 total
    // Other session's 5 lines should NOT be counted
    assert_eq!(added, 10, "Should add 10 lines (only our changes)");
    assert_eq!(removed, 0, "Should remove 0 lines");
}

#[test]
fn test_other_session_deletes_we_write_again() {
    // Scenario: Write file, another session deletes it, we write again
    // Should count only our writes
    let test_file = std::env::temp_dir().join(format!("test_delete_rewrite_{}.txt", unique_id()));

    // Final state: file exists with our second write
    let final_content = "new1\nnew2\nnew3\n";
    fs::write(&test_file, final_content).unwrap();

    // Transcript:
    // 1. First write (5 lines)
    // 2. Second write (3 lines) - after other session deleted
    let transcript = create_test_transcript(&[
        &write_entry(test_file.to_str().unwrap(), "", "old1\nold2\nold3\nold4\nold5\n"),
        &write_entry(test_file.to_str().unwrap(), "", final_content), // original is "" because file was deleted
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    // First original was "", last content is 3 lines -> +3
    assert_eq!(added, 3, "Should add 3 lines (last write content)");
    assert_eq!(removed, 0, "Should remove 0 lines");
}

#[test]
fn test_edit_replaces_lines_same_count() {
    // Scenario: Edit that replaces 3 lines with 3 different lines
    // Should show +3 -3 (like GitHub diff)
    let test_file = std::env::temp_dir().join(format!("test_edit_replace_{}.txt", unique_id()));
    fs::write(&test_file, "exists").unwrap();

    let transcript = create_test_transcript(&[
        &edit_entry(
            test_file.to_str().unwrap(),
            "old1\nold2\nold3\n",
            "new1\nnew2\nnew3\n"
        ),
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    // Replacing 3 lines with 3 different lines = +3 -3
    assert_eq!(added, 3, "Should add 3 lines");
    assert_eq!(removed, 3, "Should remove 3 lines");
}

#[test]
fn test_edit_existing_file_not_created_by_us() {
    // Scenario: Edit an existing file we didn't create (no prior Write in transcript)
    let test_file = std::env::temp_dir().join(format!("test_edit_existing_{}.txt", unique_id()));

    // File exists on disk (created by someone else or pre-existing)
    fs::write(&test_file, "pre-existing content\n").unwrap();

    // We only edit it, no Write entry
    let transcript = create_test_transcript(&[
        &edit_entry(
            test_file.to_str().unwrap(),
            "pre-existing content\n",
            "pre-existing content\nadded line1\nadded line2\n"
        ),
    ]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    // Edit adds 2 lines
    assert_eq!(added, 2, "Should add 2 lines");
    assert_eq!(removed, 0, "Should remove 0 lines");
}
