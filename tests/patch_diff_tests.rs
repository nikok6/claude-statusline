// Tests for structuredPatch-based diff counting and transcript-tree awareness.
//
// These scenarios are distilled from a real Claude Code transcript where the
// statusline reported +732 -565 for a session whose true diff was +318 -1405:
//
//   1. Some records carry an empty (or missing) `originalFile` even though the
//      file had content — diffing against "" turned a deletion-heavy rewrite
//      into pure additions. The record's `structuredPatch` is authoritative.
//   2. The transcript is a tree (`uuid`/`parentUuid`), not a list. Sessions
//      that were rewound leave abandoned branches whose edits were rolled
//      back and must not be counted. The active path is the parent chain of
//      the last uuid-bearing entry in the file.
//   3. `structuredPatch` lines store tabs as two spaces, while file content
//      keeps real tabs — matching must normalize.

use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_id() -> u64 {
    TEST_COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn create_test_transcript(entries: &[String]) -> String {
    let tmp_dir = std::env::temp_dir().join("statusline_test");
    let _ = fs::create_dir_all(&tmp_dir);
    let path = tmp_dir.join(format!("patch_test_{}_{}.jsonl", std::process::id(), unique_id()));
    let mut file = File::create(&path).unwrap();
    for entry in entries {
        writeln!(file, "{}", entry).unwrap();
    }
    path.to_string_lossy().to_string()
}

/// Build a structuredPatch hunk. `lines` use unified-diff prefixes
/// (' ' context, '+' add, '-' delete); old/new line counts are derived.
fn hunk(old_start: u64, new_start: u64, lines: &[&str]) -> Value {
    let old_lines = lines.iter().filter(|l| l.starts_with(' ') || l.starts_with('-')).count();
    let new_lines = lines.iter().filter(|l| l.starts_with(' ') || l.starts_with('+')).count();
    json!({
        "oldStart": old_start,
        "oldLines": old_lines,
        "newStart": new_start,
        "newLines": new_lines,
        "lines": lines,
    })
}

/// Edit-tool result entry. `original_file: None` omits the key entirely
/// (some real records lack it); `Some("")` mimics the empty-string quirk.
fn edit_entry(
    uuid: &str,
    parent: Option<&str>,
    file_path: &str,
    old_string: &str,
    new_string: &str,
    original_file: Option<&str>,
    patch: &[Value],
) -> String {
    let mut result = json!({
        "filePath": file_path,
        "oldString": old_string,
        "newString": new_string,
        "replaceAll": false,
        "userModified": false,
        "structuredPatch": patch,
    });
    if let Some(orig) = original_file {
        result["originalFile"] = json!(orig);
    }
    let obj = json!({
        "uuid": uuid,
        "parentUuid": parent,
        "type": "user",
        "toolUseResult": result,
    });
    serde_json::to_string(&obj).unwrap()
}

/// Write-tool result entry ("update" of an existing file).
fn write_entry(
    uuid: &str,
    parent: Option<&str>,
    file_path: &str,
    original_file: Option<&str>,
    content: &str,
    patch: &[Value],
) -> String {
    let mut result = json!({
        "type": "update",
        "filePath": file_path,
        "content": content,
        "userModified": false,
        "structuredPatch": patch,
    });
    if let Some(orig) = original_file {
        result["originalFile"] = json!(orig);
    }
    let obj = json!({
        "uuid": uuid,
        "parentUuid": parent,
        "type": "user",
        "toolUseResult": result,
    });
    serde_json::to_string(&obj).unwrap()
}

/// Plain assistant message — a tree node with no file edit.
fn message_entry(uuid: &str, parent: Option<&str>) -> String {
    let obj = json!({
        "uuid": uuid,
        "parentUuid": parent,
        "type": "assistant",
        "message": {"role": "assistant", "content": []},
    });
    serde_json::to_string(&obj).unwrap()
}

/// Metadata line without a uuid (e.g. mode/lastPrompt markers in real transcripts).
fn meta_line() -> String {
    serde_json::to_string(&json!({"type": "permission-mode", "permissionMode": "default"})).unwrap()
}

fn run_statusline(transcript_path: &str, test_file: &str) -> (usize, usize) {
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

    let mut added = 0;
    let mut removed = 0;

    for (i, c) in stripped.chars().enumerate() {
        if c == '+' {
            let num: String = stripped[i + 1..].chars().take_while(|c| c.is_numeric()).collect();
            if let Ok(n) = num.parse() {
                added = n;
            }
        } else if c == '-' {
            let num: String = stripped[i + 1..].chars().take_while(|c| c.is_numeric()).collect();
            if !num.is_empty()
                && let Ok(n) = num.parse() {
                    removed = n;
                }
        }
    }

    let _ = fs::remove_file(test_file);
    (added, removed)
}

#[test]
fn test_write_with_empty_original_uses_patch() {
    // Real-world bug: a Write record whose originalFile is "" despite the file
    // having had 5 lines. The structuredPatch carries the true diff (+1 -3).
    // Diffing content against the empty originalFile would wrongly give +3 -0.
    let test_file = std::env::temp_dir().join(format!("patch_write_empty_orig_{}.txt", unique_id()));
    let content = "keep1\nkeep2\nnew1\n";
    fs::write(&test_file, content).unwrap();

    let transcript = create_test_transcript(&[write_entry(
        "w1",
        None,
        test_file.to_str().unwrap(),
        Some(""),
        content,
        &[hunk(1, 1, &[" keep1", " keep2", "-old1", "-old2", "-old3", "+new1"])],
    )]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    assert_eq!((added, removed), (1, 3), "Write with empty originalFile must count from structuredPatch");
}

#[test]
fn test_write_with_missing_original_uses_patch() {
    // Same as above, but the originalFile key is absent entirely.
    let test_file = std::env::temp_dir().join(format!("patch_write_no_orig_{}.txt", unique_id()));
    let content = "keep1\nnew1\n";
    fs::write(&test_file, content).unwrap();

    let transcript = create_test_transcript(&[write_entry(
        "w1",
        None,
        test_file.to_str().unwrap(),
        None,
        content,
        &[hunk(1, 1, &[" keep1", "-old1", "-old2", "+new1"])],
    )]);

    let (added, removed) = run_statusline(&transcript, test_file.to_str().unwrap());
    assert_eq!((added, removed), (1, 2), "Write with missing originalFile must count from structuredPatch");
}

#[test]
fn test_abandoned_branch_edits_ignored() {
    // The session was rewound: edit "b" (adding 5 lines) sits on an abandoned
    // branch and was rolled back. Active path is root -> a -> c -> m.
    // True diff: "x1\nbase\n" -> "z1\nbase\n" = +1 -1.
    let test_file = std::env::temp_dir().join(format!("patch_branch_{}.txt", unique_id()));
    fs::write(&test_file, "z1\nbase\n").unwrap();
    let fp = test_file.to_str().unwrap();

    let transcript = create_test_transcript(&[
        edit_entry(
            "a", None, fp,
            "x1\n", "y1\n",
            Some("x1\nbase\n"),
            &[hunk(1, 1, &["-x1", "+y1", " base"])],
        ),
        // Abandoned branch: also a child of "a", superseded by "c".
        edit_entry(
            "b", Some("a"), fp,
            "y1\n", "y1\nb1\nb2\nb3\nb4\nb5\n",
            Some("y1\nbase\n"),
            &[hunk(1, 1, &[" y1", "+b1", "+b2", "+b3", "+b4", "+b5", " base"])],
        ),
        // Active branch.
        edit_entry(
            "c", Some("a"), fp,
            "y1\n", "z1\n",
            Some("y1\nbase\n"),
            &[hunk(1, 1, &["-y1", "+z1", " base"])],
        ),
        // Active leaf is a plain message; trailing metadata line has no uuid.
        message_entry("m", Some("c")),
        meta_line(),
    ]);

    let (added, removed) = run_statusline(&transcript, fp);
    assert_eq!((added, removed), (1, 1), "Edits on abandoned (rewound) branches must not be counted");
}

#[test]
fn test_same_line_twice_with_patches_nets() {
    // Two patch-bearing edits to the same line must net to +1 -1,
    // not +2 -2 (i.e. the implementation must not just sum patch hunks).
    let test_file = std::env::temp_dir().join(format!("patch_same_line_{}.txt", unique_id()));
    fs::write(&test_file, "line1\nfinal\nline3\n").unwrap();
    let fp = test_file.to_str().unwrap();

    let transcript = create_test_transcript(&[
        edit_entry(
            "a", None, fp,
            "orig\n", "mid\n",
            Some("line1\norig\nline3\n"),
            &[hunk(1, 1, &[" line1", "-orig", "+mid", " line3"])],
        ),
        edit_entry(
            "b", Some("a"), fp,
            "mid\n", "final\n",
            Some("line1\nmid\nline3\n"),
            &[hunk(1, 1, &[" line1", "-mid", "+final", " line3"])],
        ),
    ]);

    let (added, removed) = run_statusline(&transcript, fp);
    assert_eq!((added, removed), (1, 1), "Same line edited twice must net to +1 -1");
}

#[test]
fn test_revert_with_patches_is_zero() {
    // Edit then revert, both with patches: net must be +0 -0.
    let test_file = std::env::temp_dir().join(format!("patch_revert_{}.txt", unique_id()));
    fs::write(&test_file, "l1\norig\nl3\n").unwrap();
    let fp = test_file.to_str().unwrap();

    let transcript = create_test_transcript(&[
        edit_entry(
            "a", None, fp,
            "orig\n", "tmp\n",
            Some("l1\norig\nl3\n"),
            &[hunk(1, 1, &[" l1", "-orig", "+tmp", " l3"])],
        ),
        edit_entry(
            "b", Some("a"), fp,
            "tmp\n", "orig\n",
            Some("l1\ntmp\nl3\n"),
            &[hunk(1, 1, &[" l1", "-tmp", "+orig", " l3"])],
        ),
    ]);

    let (added, removed) = run_statusline(&transcript, fp);
    assert_eq!((added, removed), (0, 0), "Edit then revert must net to zero");
}

#[test]
fn test_tab_normalized_patch_lines() {
    // structuredPatch lines store tabs as two spaces while originalFile keeps
    // real tabs. Patch application must match them up. True diff: -1.
    let test_file = std::env::temp_dir().join(format!("patch_tabs_{}.go", unique_id()));
    fs::write(&test_file, "func main() {\n\tx := 1\n}\n").unwrap();
    let fp = test_file.to_str().unwrap();

    let transcript = create_test_transcript(&[edit_entry(
        "a", None, fp,
        "\ty := 2\n", "",
        Some("func main() {\n\tx := 1\n\ty := 2\n}\n"),
        &[hunk(1, 1, &[" func main() {", "   x := 1", "-  y := 2", " }"])],
    )]);

    let (added, removed) = run_statusline(&transcript, fp);
    assert_eq!((added, removed), (0, 1), "Tab-normalized patch lines must still apply");
}

#[test]
fn test_empty_original_multi_edit_chain() {
    // Mirrors the real authentication.go case: several edits whose
    // originalFile is empty, followed by one with it populated. The before
    // state must be recovered by walking patches backward from the anchor.
    // Base "a\nb\nc\nd\n" -> final "a\nd\ne\n" = +1 -2.
    let test_file = std::env::temp_dir().join(format!("patch_multi_edit_{}.txt", unique_id()));
    fs::write(&test_file, "a\nd\ne\n").unwrap();
    let fp = test_file.to_str().unwrap();

    let transcript = create_test_transcript(&[
        edit_entry(
            "a", None, fp,
            "b\n", "",
            Some(""),
            &[hunk(1, 1, &[" a", "-b", " c", " d"])],
        ),
        edit_entry(
            "b", Some("a"), fp,
            "c\n", "",
            Some(""),
            &[hunk(1, 1, &[" a", "-c", " d"])],
        ),
        edit_entry(
            "c", Some("b"), fp,
            "d\n", "d\ne\n",
            Some("a\nd\n"),
            &[hunk(1, 1, &[" a", " d", "+e"])],
        ),
    ]);

    let (added, removed) = run_statusline(&transcript, fp);
    assert_eq!((added, removed), (1, 2), "Empty-originalFile edits must reconstruct via patches");
}

#[test]
fn test_multi_hunk_patch() {
    // A single record whose patch has two hunks at different offsets.
    // l1..l9 with l2 -> x2 and x9 inserted before l9 = +2 -1.
    let test_file = std::env::temp_dir().join(format!("patch_multi_hunk_{}.txt", unique_id()));
    let original = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n";
    let content = "l1\nx2\nl3\nl4\nl5\nl6\nl7\nl8\nx9\nl9\n";
    fs::write(&test_file, content).unwrap();
    let fp = test_file.to_str().unwrap();

    let transcript = create_test_transcript(&[write_entry(
        "w1", None, fp,
        Some(original),
        content,
        &[
            hunk(1, 1, &[" l1", "-l2", "+x2", " l3"]),
            hunk(7, 7, &[" l7", " l8", "+x9", " l9"]),
        ],
    )]);

    let (added, removed) = run_statusline(&transcript, fp);
    assert_eq!((added, removed), (2, 1), "Multi-hunk patches must apply with correct offsets");
}
