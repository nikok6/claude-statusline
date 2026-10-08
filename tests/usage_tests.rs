//! End-to-end tests for the opt-in usage tracker.
//!
//! Each test runs the real `statusline` binary with `$HOME` pointed at an
//! isolated sandbox containing a `track_usage`-enabled config and some fake
//! transcripts, then asserts on the `usage-summary.json` / `usage-sessions.json`
//! it writes. Because the scratch cache is scoped to `$HOME`, sandboxes are
//! fully isolated and these tests are parallel-safe with no shared state.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_id() -> u64 {
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Creates an isolated fake `$HOME` with a statusline.json enabling usage
/// tracking. `lines` is empty so the binary does no git/diff/token rendering —
/// only the usage side effect runs.
fn sandbox_home(track_usage: Value) -> PathBuf {
    let home = std::env::temp_dir().join(format!("sl_usage_{}_{}", std::process::id(), unique_id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(home.join(".claude/projects")).unwrap();
    let config = json!({ "lines": [], "track_usage": track_usage });
    fs::write(home.join(".claude/statusline.json"), config.to_string()).unwrap();
    home
}

fn enabled(tz: &str) -> PathBuf {
    sandbox_home(json!({ "enabled": true, "timezone": tz }))
}

/// One assistant transcript line carrying a usage block.
#[allow(clippy::too_many_arguments)]
fn assistant_line(
    ts: &str, session: &str, cwd: &str, model: &str,
    input: u64, output: u64, cache_5m: u64, cache_1h: u64, cache_read: u64,
) -> String {
    json!({
        "type": "assistant",
        "timestamp": ts,
        "sessionId": session,
        "cwd": cwd,
        "message": {
            "model": model,
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_creation_input_tokens": cache_5m + cache_1h,
                "cache_read_input_tokens": cache_read,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": cache_5m,
                    "ephemeral_1h_input_tokens": cache_1h
                }
            }
        }
    })
    .to_string()
}

/// Writes JSONL lines into a transcript under `projects/proj/<name>.jsonl`.
/// The tracker only scans files nested one directory under `projects/`.
fn write_transcript(home: &Path, name: &str, lines: &[String]) -> PathBuf {
    let dir = home.join(".claude/projects/proj");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.jsonl"));
    let mut f = File::create(&path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    path
}

/// Writes JSONL into a subagent transcript at
/// `projects/proj/<session>/subagents/<name>.jsonl` — the nested location
/// Claude Code uses for subagent (sidechain) conversations.
fn write_subagent_transcript(home: &Path, session: &str, name: &str, lines: &[String]) -> PathBuf {
    let dir = home.join(format!(".claude/projects/proj/{session}/subagents"));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.jsonl"));
    let mut f = File::create(&path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    path
}

fn append_lines(path: &Path, lines: &[String]) {
    let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
}

/// Runs the statusline binary with `$HOME` set to the sandbox.
fn run(home: &Path) {
    let input = r#"{"cwd":"/tmp","transcript_path":"/tmp/none","model":{"display_name":"test"}}"#;
    let output = Command::new(env!("CARGO_BIN_EXE_statusline"))
        .env("HOME", home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .expect("failed to run statusline");
    assert!(
        output.status.success(),
        "statusline exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_json(path: &Path) -> Value {
    let s = fs::read_to_string(path).unwrap_or_else(|_| panic!("missing file {path:?}"));
    serde_json::from_str(&s).unwrap()
}

fn summary(home: &Path) -> Value {
    read_json(&home.join(".claude/usage/usage-summary.json"))
}

fn sessions(home: &Path) -> Value {
    read_json(&home.join(".claude/usage/usage-sessions.json"))
}

/// Looks up a daily/weekly/monthly bucket by its key.
fn bucket<'a>(summary: &'a Value, period: &str, key: &str) -> &'a Value {
    summary[period]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["key"].as_str() == Some(key))
        .unwrap_or_else(|| panic!("no {period} bucket {key}"))
}

fn has_bucket(summary: &Value, period: &str, key: &str) -> bool {
    summary[period]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["key"].as_str() == Some(key))
}

fn u(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or_else(|| panic!("{key} not a u64: {v}"))
}

fn approx(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-6, "expected {expected}, got {actual}");
}

fn cost(v: &Value) -> f64 {
    v["cost_usd"].as_f64().unwrap()
}

#[test]
fn single_entry_folds_into_totals_buckets_and_session() {
    let home = enabled("UTC");
    // 1M tokens in each category at opus-4-7 rates:
    // input 5 + output 25 + cache_5m 6.25 + cache_1h 10 + cache_read 0.5 = 46.75
    let line = assistant_line(
        "2099-03-15T10:00:00Z", "sess-aaa", "/work/alpha",
        "claude-opus-4-7", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
    );
    write_transcript(&home, "t1", &[line]);
    run(&home);

    let s = summary(&home);
    assert_eq!(s["timezone"].as_str(), Some("UTC"));

    let totals = &s["totals"];
    assert_eq!(u(totals, "input_tokens"), 1_000_000);
    assert_eq!(u(totals, "output_tokens"), 1_000_000);
    assert_eq!(u(totals, "cache_creation_tokens"), 2_000_000); // reported 5m+1h total
    assert_eq!(u(totals, "cache_read_tokens"), 1_000_000);
    approx(cost(totals), 46.75);
    approx(cost(&totals["by_model"]["claude-opus-4-7"]), 46.75);

    // ISO keys (Python isocalendar: 2099-03-15 -> 2099-W11).
    approx(cost(bucket(&s, "daily", "2099-03-15")), 46.75);
    approx(cost(bucket(&s, "weekly", "2099-W11")), 46.75);
    approx(cost(bucket(&s, "monthly", "2099-03")), 46.75);

    let sess = sessions(&home);
    let a = &sess["sessions"]["sess-aaa"];
    assert_eq!(u(a, "input_tokens"), 1_000_000);
    assert_eq!(a["first_seen"].as_str(), Some("2099-03-15T10:00:00Z"));
    assert_eq!(a["last_seen"].as_str(), Some("2099-03-15T10:00:00Z"));
    assert_eq!(a["cwd"].as_str(), Some("/work/alpha"));

    fs::remove_dir_all(&home).ok();
}

#[test]
fn aggregates_across_days_models_and_sessions() {
    let home = enabled("UTC");
    let lines = vec![
        assistant_line("2099-03-15T10:00:00Z", "s1", "/w", "claude-opus-4-7", 1_000_000, 0, 0, 0, 0),
        assistant_line("2099-03-20T10:00:00Z", "s1", "/w", "claude-sonnet-4-6", 1_000_000, 0, 0, 0, 0),
        assistant_line("2099-03-20T11:00:00Z", "s2", "/w", "claude-opus-4-7", 1_000_000, 0, 0, 0, 0),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let s = summary(&home);
    // Totals: opus 5 + sonnet 3 + opus 5 = 13.00 over 3M input.
    assert_eq!(u(&s["totals"], "input_tokens"), 3_000_000);
    approx(cost(&s["totals"]), 13.0);
    approx(cost(&s["totals"]["by_model"]["claude-opus-4-7"]), 10.0);
    approx(cost(&s["totals"]["by_model"]["claude-sonnet-4-6"]), 3.0);

    // Daily split: 03-15 has only the first opus; 03-20 has sonnet + opus.
    approx(cost(bucket(&s, "daily", "2099-03-15")), 5.0);
    approx(cost(bucket(&s, "daily", "2099-03-20")), 8.0);
    // Both March days share the same month bucket.
    approx(cost(bucket(&s, "monthly", "2099-03")), 13.0);

    let sess = sessions(&home);
    assert_eq!(u(&sess["sessions"]["s1"], "input_tokens"), 2_000_000);
    assert_eq!(u(&sess["sessions"]["s2"], "input_tokens"), 1_000_000);
    assert_eq!(sess["sessions"]["s1"]["first_seen"].as_str(), Some("2099-03-15T10:00:00Z"));
    assert_eq!(sess["sessions"]["s1"]["last_seen"].as_str(), Some("2099-03-20T10:00:00Z"));

    fs::remove_dir_all(&home).ok();
}

#[test]
fn subagent_transcript_usage_folds_into_parent_session() {
    let home = enabled("UTC");
    // Main session line under projects/proj/<session>.jsonl ...
    let main = assistant_line(
        "2099-03-15T10:00:00Z", "sess-main", "/work/alpha", "claude-opus-4-7", 1_000_000, 0, 0, 0, 0,
    );
    write_transcript(&home, "sess-main", &[main]);
    // ... and a subagent line nested under projects/proj/sess-main/subagents/.
    // It shares the parent sessionId; its usage is billed but lives only here.
    let sub = assistant_line(
        "2099-03-15T10:05:00Z", "sess-main", "/work/alpha", "claude-opus-4-7", 2_000_000, 0, 0, 0, 0,
    );
    write_subagent_transcript(&home, "sess-main", "agent-abc", &[sub]);
    run(&home);

    let s = summary(&home);
    // Both the main (1M) and the subagent (2M) tokens are counted.
    assert_eq!(u(&s["totals"], "input_tokens"), 3_000_000);
    approx(cost(&s["totals"]), 15.0); // 1M + 2M input at opus-4-7 $5/M
    approx(cost(bucket(&s, "daily", "2099-03-15")), 15.0);

    // They share one sessionId, so they fold into a single session bucket
    // spanning both timestamps.
    let sess = sessions(&home);
    let a = &sess["sessions"]["sess-main"];
    assert_eq!(u(a, "input_tokens"), 3_000_000);
    assert_eq!(a["first_seen"].as_str(), Some("2099-03-15T10:00:00Z"));
    assert_eq!(a["last_seen"].as_str(), Some("2099-03-15T10:05:00Z"));

    fs::remove_dir_all(&home).ok();
}

#[test]
fn timezone_offset_shifts_daily_bucket() {
    // 23:30 UTC + 02:00 = 01:30 the next day -> lands in 03-16, not 03-15.
    let home = enabled("+02:00");
    let line = assistant_line("2099-03-15T23:30:00Z", "s", "/w", "claude-opus-4-7", 10, 0, 0, 0, 0);
    write_transcript(&home, "t1", &[line]);
    run(&home);

    let s = summary(&home);
    assert_eq!(s["timezone"].as_str(), Some("UTC+02:00"));
    assert!(has_bucket(&s, "daily", "2099-03-16"));
    assert!(!has_bucket(&s, "daily", "2099-03-15"));

    fs::remove_dir_all(&home).ok();
}

#[test]
fn incremental_update_does_not_double_count() {
    let home = enabled("UTC");
    let line = assistant_line("2099-03-15T10:00:00Z", "s", "/w", "claude-opus-4-7", 1000, 0, 0, 0, 0);
    let path = write_transcript(&home, "t1", std::slice::from_ref(&line));
    run(&home);
    assert_eq!(u(&summary(&home)["totals"], "input_tokens"), 1000);

    // Advance mtime so the incremental reader picks up the appended bytes,
    // then verify only the new line is folded (the first isn't re-counted).
    std::thread::sleep(std::time::Duration::from_millis(50));
    append_lines(&path, &[line]);
    run(&home);
    assert_eq!(u(&summary(&home)["totals"], "input_tokens"), 2000);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn ignores_non_usage_synthetic_and_malformed_lines() {
    let home = enabled("UTC");
    let lines = vec![
        assistant_line("2099-03-15T10:00:00Z", "s", "/w", "claude-opus-4-7", 1000, 0, 0, 0, 0),
        // user line: no usage block -> skipped
        json!({"type":"user","timestamp":"2099-03-15T10:01:00Z","message":{"role":"user"}}).to_string(),
        // assistant with synthetic model name (starts with '<') -> skipped
        assistant_line("2099-03-15T10:02:00Z", "s", "/w", "<synthetic>", 5000, 0, 0, 0, 0),
        // contains "usage" but is not valid JSON -> parse error, skipped
        r#"{"type":"assistant","message":{"usage": broken}}"#.to_string(),
        // junk with no usage substring -> skipped before parsing
        "not json at all".to_string(),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let s = summary(&home);
    assert_eq!(u(&s["totals"], "input_tokens"), 1000);
    assert!(s["totals"]["by_model"].get("<synthetic>").is_none());

    fs::remove_dir_all(&home).ok();
}

#[test]
fn disabled_tracking_writes_no_files() {
    let home = sandbox_home(json!({ "enabled": false }));
    let line = assistant_line("2099-03-15T10:00:00Z", "s", "/w", "claude-opus-4-7", 1000, 0, 0, 0, 0);
    write_transcript(&home, "t1", &[line]);
    run(&home);

    assert!(!home.join(".claude/usage").exists());

    fs::remove_dir_all(&home).ok();
}

#[test]
fn partial_final_line_is_not_lost_or_double_counted() {
    let home = enabled("UTC");
    let dir = home.join(".claude/projects/proj");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t1.jsonl");

    let line1 = assistant_line("2099-03-15T10:00:00Z", "s", "/w", "claude-opus-4-7", 1000, 0, 0, 0, 0);
    let line2 = assistant_line("2099-03-15T11:00:00Z", "s", "/w", "claude-opus-4-7", 2000, 0, 0, 0, 0);

    // line1 complete; line2 written WITHOUT its trailing newline (a partial flush).
    {
        let mut f = File::create(&path).unwrap();
        write!(f, "{line1}\n{line2}").unwrap();
    }
    run(&home);
    // Only the complete line is folded; the partial line is held back, not lost.
    assert_eq!(u(&summary(&home)["totals"], "input_tokens"), 1000);

    // Flush the newline that completes line2.
    std::thread::sleep(std::time::Duration::from_millis(50));
    {
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f).unwrap();
    }
    run(&home);
    // line2 now folded exactly once -> 3000 (not 1000=lost, not 5000=double).
    assert_eq!(u(&summary(&home)["totals"], "input_tokens"), 3000);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn truncation_does_not_double_count() {
    let home = enabled("UTC");
    let line = assistant_line("2099-03-15T10:00:00Z", "s", "/w", "claude-opus-4-7", 1000, 0, 0, 0, 0);
    let path = write_transcript(&home, "t1", &[line]);
    run(&home);
    assert_eq!(u(&summary(&home)["totals"], "input_tokens"), 1000);

    // Shrink the file below the recorded offset (rotation/rewrite). Already-counted
    // content must not be re-folded.
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(5).unwrap();
    run(&home);
    assert_eq!(u(&summary(&home)["totals"], "input_tokens"), 1000);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn session_first_last_seen_orders_mixed_timestamp_formats() {
    let home = enabled("UTC");
    // Same second, fractional vs whole: "...00Z" is lexically GREATER than
    // "...00.500Z" ('Z' > '.') yet is the EARLIER instant. Ordering must be by
    // parsed instant, not bytes.
    let lines = vec![
        assistant_line("2099-03-15T10:00:00Z", "s", "/w", "claude-opus-4-7", 1, 0, 0, 0, 0),
        assistant_line("2099-03-15T10:00:00.500Z", "s", "/w", "claude-opus-4-7", 1, 0, 0, 0, 0),
        assistant_line("2099-03-15T10:00:01Z", "s", "/w", "claude-opus-4-7", 1, 0, 0, 0, 0),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let sess = sessions(&home);
    let a = &sess["sessions"]["s"];
    assert_eq!(a["first_seen"].as_str(), Some("2099-03-15T10:00:00Z"));
    assert_eq!(a["last_seen"].as_str(), Some("2099-03-15T10:00:01Z"));

    fs::remove_dir_all(&home).ok();
}

#[test]
fn custom_output_path_is_a_directory_holding_all_three_files() {
    let home = std::env::temp_dir().join(format!("sl_usage_out_{}_{}", std::process::id(), unique_id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(home.join(".claude/projects/proj")).unwrap();
    // Directory that does not exist yet.
    let out_dir = home.join("out/usage");
    let config = json!({
        "lines": [],
        "track_usage": {
            "enabled": true,
            "timezone": "UTC",
            "output_path": out_dir.to_str().unwrap()
        }
    });
    fs::write(home.join(".claude/statusline.json"), config.to_string()).unwrap();

    let line = assistant_line("2099-03-15T10:00:00Z", "s", "/w", "claude-opus-4-7", 1000, 0, 0, 0, 0);
    write_transcript(&home, "t1", &[line]);
    run(&home);

    assert!(out_dir.join("usage-summary.json").exists(), "summary inside directory");
    assert!(out_dir.join("usage-sessions.json").exists(), "sessions inside directory");
    assert!(out_dir.join("usage-cache.json").exists(), "cache inside directory");
    // Nothing at the default location.
    assert!(!home.join(".claude/usage").exists(), "default dir untouched with custom path");

    fs::remove_dir_all(&home).ok();
}

#[test]
fn sessions_are_capped_to_most_recent() {
    let home = enabled("UTC");
    // 1001 distinct sessions, oldest first; cap is 1000.
    let mut lines = Vec::new();
    for i in 0..1001u32 {
        let ts = format!("2099-03-15T10:{:02}:{:02}Z", i / 60, i % 60);
        lines.push(assistant_line(&ts, &format!("s{i:04}"), "/w", "claude-opus-4-7", 1, 0, 0, 0, 0));
    }
    write_transcript(&home, "t1", &lines);
    run(&home);

    let sess = sessions(&home);
    let map = sess["sessions"].as_object().unwrap();
    assert_eq!(map.len(), 1000, "capped at MAX_SESSIONS");
    assert!(map.get("s0000").is_none(), "oldest session evicted");
    assert!(map.get("s1000").is_some(), "newest session retained");
    // Pruning detail does not affect the long-term totals.
    assert_eq!(u(&summary(&home)["totals"], "input_tokens"), 1001);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn unknown_model_is_unpriced_then_repriced_after_update() {
    let home = enabled("UTC");
    // 1M tokens per category under a model price_for doesn't know.
    let line = assistant_line(
        "2099-03-15T10:00:00Z", "sess-up", "/work/up",
        "claude-unknown-9", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
    );
    write_transcript(&home, "t1", &[line]);
    run(&home);

    let s = summary(&home);
    approx(cost(&s["totals"]), 0.0);
    approx(cost(&s["totals"]["by_model"]["claude-unknown-9"]), 0.0);
    // All five categories surface in the unpriced rollup (creation counted once).
    assert_eq!(u(&s["totals"], "unpriced_tokens"), 5_000_000);
    assert_eq!(u(bucket(&s, "daily", "2099-03-15"), "unpriced_tokens"), 5_000_000);
    let sess = sessions(&home);
    assert_eq!(u(&sess["sessions"]["sess-up"], "unpriced_tokens"), 5_000_000);

    // Simulate updating the binary's pricing table: the cached history now sits
    // under a model the current price_for knows. Rename it in the cache, as if
    // the old binary had folded fable usage before fable pricing existed.
    let cache_file = home.join(".claude/usage/usage-cache.json");
    let cache = fs::read_to_string(&cache_file).unwrap();
    fs::write(&cache_file, cache.replace("claude-unknown-9", "claude-fable-5")).unwrap();
    run(&home);

    // Fable rates: input 10 + output 50 + cache_5m 12.50 + cache_1h 20 + read 1 = 93.50
    let s = summary(&home);
    approx(cost(&s["totals"]), 93.50);
    approx(cost(&s["totals"]["by_model"]["claude-fable-5"]), 93.50);
    assert!(s["totals"].get("unpriced_tokens").is_none(), "rollup cleared once priced");
    approx(cost(bucket(&s, "daily", "2099-03-15")), 93.50);
    approx(cost(bucket(&s, "monthly", "2099-03")), 93.50);
    let sess = sessions(&home);
    approx(cost(&sess["sessions"]["sess-up"]), 93.50);
    assert!(sess["sessions"]["sess-up"].get("unpriced_tokens").is_none());

    fs::remove_dir_all(&home).ok();
}

#[test]
fn unknown_version_does_not_inherit_family_pricing() {
    let home = enabled("UTC");
    let lines = vec![
        // A future opus version must NOT fall back to claude-opus-4 legacy rates.
        assistant_line("2099-03-15T10:00:00Z", "s1", "/w", "claude-opus-4-9", 1_000_000, 0, 0, 0, 0),
        // Date snapshots and bracket tags of known ids still price normally.
        assistant_line("2099-03-15T10:01:00Z", "s1", "/w", "claude-opus-4-1-20250805", 1_000_000, 0, 0, 0, 0),
        assistant_line("2099-03-15T10:02:00Z", "s1", "/w", "claude-fable-5[1m]", 1_000_000, 0, 0, 0, 0),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let s = summary(&home);
    approx(cost(&s["totals"]["by_model"]["claude-opus-4-9"]), 0.0);
    assert_eq!(u(&s["totals"], "unpriced_tokens"), 1_000_000);
    approx(cost(&s["totals"]["by_model"]["claude-opus-4-1-20250805"]), 15.0);
    approx(cost(&s["totals"]["by_model"]["claude-fable-5[1m]"]), 10.0);
    approx(cost(&s["totals"]), 25.0);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn fable_5_1_uses_quarter_cache_read_rate() {
    let home = enabled("UTC");
    // 1M tokens in each category. Fable/Mythos 5.1 share 5's $10/$50 and
    // cache-write rates, but cache reads are $0.25 (0.025x) not $1 (0.1x).
    // 5.1: 10 + 50 + 12.50 + 20 + 0.25 = 92.75
    // 5:   10 + 50 + 12.50 + 20 + 1.00 = 93.50
    let lines = vec![
        assistant_line(
            "2099-03-15T10:00:00Z", "s1", "/w",
            "claude-fable-5-1", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
        ),
        assistant_line(
            "2099-03-15T10:01:00Z", "s1", "/w",
            "claude-mythos-5-1", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
        ),
        assistant_line(
            "2099-03-15T10:02:00Z", "s1", "/w",
            "claude-fable-5", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
        ),
        // Date snapshot of 5.1 must not fall through to 5's $1 cache-read rate.
        assistant_line(
            "2099-03-15T10:03:00Z", "s1", "/w",
            "claude-fable-5-1-20260901", 0, 0, 0, 0, 1_000_000,
        ),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let s = summary(&home);
    approx(cost(&s["totals"]["by_model"]["claude-fable-5-1"]), 92.75);
    approx(cost(&s["totals"]["by_model"]["claude-mythos-5-1"]), 92.75);
    approx(cost(&s["totals"]["by_model"]["claude-fable-5"]), 93.50);
    approx(cost(&s["totals"]["by_model"]["claude-fable-5-1-20260901"]), 0.25);
    approx(cost(&s["totals"]), 92.75 + 92.75 + 93.50 + 0.25);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn sonnet_5_5_uses_half_sonnet_5_cache_read_rate() {
    let home = enabled("UTC");
    // 1M per category; only cache reads differ.
    // 5.5: 2 + 10 + 2.50 + 4 + 0.10 = 18.60
    // 5:   2 + 10 + 2.50 + 4 + 0.20 = 18.70
    let lines = vec![
        assistant_line(
            "2099-03-15T10:00:00Z", "s1", "/w",
            "claude-sonnet-5-5", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
        ),
        assistant_line(
            "2099-03-15T10:01:00Z", "s1", "/w",
            "claude-sonnet-5-5[1m]", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
        ),
        assistant_line(
            "2099-03-15T10:02:00Z", "s1", "/w",
            "claude-sonnet-5", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
        ),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let s = summary(&home);
    approx(cost(&s["totals"]["by_model"]["claude-sonnet-5-5"]), 18.60);
    approx(cost(&s["totals"]["by_model"]["claude-sonnet-5-5[1m]"]), 18.60);
    approx(cost(&s["totals"]["by_model"]["claude-sonnet-5"]), 18.70);
    assert!(s["totals"].get("unpriced_tokens").is_none());

    fs::remove_dir_all(&home).ok();
}

#[test]
fn haiku_5_5_short_prompt_tier() {
    let home = enabled("UTC");
    // 10k per category: 40k prompt, short tier.
    // 0.001 + 0.005 + 0.00125 + 0.002 + 0.0001 = 0.00935
    let lines = vec![
        assistant_line(
            "2099-03-15T10:00:00Z", "s1", "/w",
            "claude-haiku-5-5", 10_000, 10_000, 10_000, 10_000, 10_000,
        ),
        // [1m] tag and date snapshot match too.
        assistant_line(
            "2099-03-15T10:01:00Z", "s1", "/w",
            "claude-haiku-5-5[1m]", 10_000, 10_000, 10_000, 10_000, 10_000,
        ),
        assistant_line(
            "2099-03-15T10:02:00Z", "s1", "/w",
            "claude-haiku-5-5-20261007", 10_000, 10_000, 10_000, 10_000, 10_000,
        ),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let s = summary(&home);
    approx(cost(&s["totals"]["by_model"]["claude-haiku-5-5"]), 0.00935);
    approx(cost(&s["totals"]["by_model"]["claude-haiku-5-5[1m]"]), 0.00935);
    approx(cost(&s["totals"]["by_model"]["claude-haiku-5-5-20261007"]), 0.00935);
    assert!(s["totals"].get("unpriced_tokens").is_none());

    fs::remove_dir_all(&home).ok();
}

#[test]
fn haiku_5_5_long_prompt_tier_applies_to_whole_request() {
    let home = enabled("UTC");
    // 1M per category: 4M prompt, long tier incl. output.
    // 0.50 + 2.50 + 0.625 + 1.0 + 0.05 = 4.675
    let lines = vec![
        assistant_line(
            "2099-03-15T10:00:00Z", "s1", "/w",
            "claude-haiku-5-5", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
        ),
        assistant_line(
            "2099-03-15T10:01:00Z", "s1", "/w",
            "claude-haiku-5-5[1m]", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
        ),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let s = summary(&home);
    approx(cost(&s["totals"]["by_model"]["claude-haiku-5-5"]), 4.675);
    approx(cost(&s["totals"]["by_model"]["claude-haiku-5-5[1m]"]), 4.675);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn haiku_5_5_tier_boundary_counts_cache_tokens() {
    let home = enabled("UTC");
    // 40k + 20k + 20k + 20k = 100,000 prompt: short.
    //   0.004 + 0.005 + 0.0025 + 0.004 + 0.0002 = 0.0157
    // One more cache-read token: long.
    //   0.02 + 0.025 + 0.0125 + 0.02 + 20_001 * 0.05 / 1M = 0.07850005
    let lines = vec![
        assistant_line(
            "2099-03-15T10:00:00Z", "s1", "/w",
            "claude-haiku-5-5", 40_000, 10_000, 20_000, 20_000, 20_000,
        ),
        assistant_line(
            "2099-03-15T10:01:00Z", "s2", "/w",
            "claude-haiku-5-5", 40_000, 10_000, 20_000, 20_000, 20_001,
        ),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let sess = sessions(&home);
    approx(cost(&sess["sessions"]["s1"]), 0.0157);
    approx(cost(&sess["sessions"]["s2"]), 0.07850005);
    approx(cost(&summary(&home)["totals"]), 0.0157 + 0.07850005);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn haiku_5_5_tier_is_per_message_not_per_session() {
    let home = enabled("UTC");
    // Tier is per message: two 60k prompts stay short (0.012), one 120k goes long (0.06).
    let lines = vec![
        assistant_line("2099-03-15T10:00:00Z", "s1", "/w", "claude-haiku-5-5", 60_000, 0, 0, 0, 0),
        assistant_line("2099-03-15T10:01:00Z", "s1", "/w", "claude-haiku-5-5", 60_000, 0, 0, 0, 0),
        assistant_line("2099-03-15T10:02:00Z", "s2", "/w", "claude-haiku-5-5", 120_000, 0, 0, 0, 0),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let sess = sessions(&home);
    approx(cost(&sess["sessions"]["s1"]), 0.012);
    approx(cost(&sess["sessions"]["s2"]), 0.06);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn haiku_5_5_history_folded_at_zero_reprices_at_short_tier() {
    let home = enabled("UTC");
    // $0 history reprices at the short tier despite totals over 100k.
    let line = assistant_line(
        "2099-03-15T10:00:00Z", "s1", "/w",
        "claude-unknown-9", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
    );
    write_transcript(&home, "t1", &[line]);
    run(&home);
    approx(cost(&summary(&home)["totals"]), 0.0);

    let cache_file = home.join(".claude/usage/usage-cache.json");
    let cache = fs::read_to_string(&cache_file).unwrap();
    fs::write(&cache_file, cache.replace("claude-unknown-9", "claude-haiku-5-5")).unwrap();
    run(&home);

    // Short tier: 0.10 + 0.50 + 0.125 + 0.20 + 0.01 = 0.935
    let s = summary(&home);
    approx(cost(&s["totals"]["by_model"]["claude-haiku-5-5"]), 0.935);
    assert!(s["totals"].get("unpriced_tokens").is_none());

    fs::remove_dir_all(&home).ok();
}

#[test]
fn haiku_5_5_history_reprices_when_new_lines_land_in_same_run() {
    let home = enabled("UTC");
    // $0 history under an unknown model.
    let line = assistant_line(
        "2099-03-15T10:00:00Z", "s1", "/w",
        "claude-unknown-9", 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
    );
    let path = write_transcript(&home, "t1", &[line]);
    run(&home);

    // Model now priced, plus a new line in the same run.
    let cache_file = home.join(".claude/usage/usage-cache.json");
    let cache = fs::read_to_string(&cache_file).unwrap();
    fs::write(&cache_file, cache.replace("claude-unknown-9", "claude-haiku-5-5")).unwrap();
    append_lines(&path, &[assistant_line(
        "2099-03-15T10:05:00Z", "s1", "/w", "claude-haiku-5-5", 10_000, 0, 0, 0, 0,
    )]);
    run(&home);

    // 0.935 repriced history + 10k input * $0.10 = 0.936
    let s = summary(&home);
    approx(cost(&s["totals"]["by_model"]["claude-haiku-5-5"]), 0.936);
    approx(cost(&s["totals"]), 0.936);
    assert!(s["totals"].get("unpriced_tokens").is_none());
    approx(cost(bucket(&s, "daily", "2099-03-15")), 0.936);
    approx(cost(bucket(&s, "monthly", "2099-03")), 0.936);
    approx(cost(&sessions(&home)["sessions"]["s1"]), 0.936);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn haiku_5_5_tier_uses_billed_cache_creation_when_total_missing() {
    let home = enabled("UTC");
    // No creation total, 150k 5m breakdown: 150,010 prompt, long tier.
    //   10 * 0.50 / 1M + 150k * 0.625 / 1M = 0.093755
    let line = json!({
        "type": "assistant", "timestamp": "2099-03-15T10:00:00Z", "sessionId": "s1", "cwd": "/w",
        "message": { "model": "claude-haiku-5-5", "usage": {
            "input_tokens": 10, "output_tokens": 0, "cache_read_input_tokens": 0,
            "cache_creation": { "ephemeral_5m_input_tokens": 150_000, "ephemeral_1h_input_tokens": 0 }
        }}
    })
    .to_string();
    write_transcript(&home, "t1", &[line]);
    run(&home);

    approx(cost(&summary(&home)["totals"]["by_model"]["claude-haiku-5-5"]), 0.093755);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn haiku_5_5_big_output_small_prompt_stays_short() {
    let home = enabled("UTC");
    // Output isn't prompt: short tier.
    //   1k * 0.10 / 1M + 1M * 0.50 / 1M = 0.5001
    let line = assistant_line(
        "2099-03-15T10:00:00Z", "s1", "/w", "claude-haiku-5-5", 1_000, 1_000_000, 0, 0, 0,
    );
    write_transcript(&home, "t1", &[line]);
    run(&home);

    approx(cost(&summary(&home)["totals"]["by_model"]["claude-haiku-5-5"]), 0.5001);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn dated_sonnet_5_5_uses_sonnet_5_5_cache_read_rate() {
    let home = enabled("UTC");
    // Dated 5.5 id gets 5.5's $0.10 cache reads.
    let line = assistant_line(
        "2099-03-15T10:00:00Z", "s1", "/w", "claude-sonnet-5-5-20261007", 0, 0, 0, 0, 1_000_000,
    );
    write_transcript(&home, "t1", &[line]);
    run(&home);

    approx(cost(&summary(&home)["totals"]["by_model"]["claude-sonnet-5-5-20261007"]), 0.10);

    fs::remove_dir_all(&home).ok();
}

/// An assistant line whose usage block has a creation total but a missing or
/// partial 5m/1h breakdown, exercising the bill-remainder-at-5m rule.
fn assistant_line_with_breakdown(
    ts: &str, model: &str, cache_total: u64, breakdown: Option<(u64, u64)>,
) -> String {
    let mut usage = json!({
        "input_tokens": 0, "output_tokens": 0,
        "cache_creation_input_tokens": cache_total, "cache_read_input_tokens": 0
    });
    if let Some((m5, h1)) = breakdown {
        usage["cache_creation"] = json!({
            "ephemeral_5m_input_tokens": m5, "ephemeral_1h_input_tokens": h1
        });
    }
    json!({
        "type": "assistant", "timestamp": ts, "sessionId": "s1", "cwd": "/w",
        "message": { "model": model, "usage": usage }
    })
    .to_string()
}

#[test]
fn missing_or_short_cache_breakdown_bills_remainder_at_5m() {
    let home = enabled("UTC");
    let lines = vec![
        // No breakdown at all: the full 1M creation total bills at the 5m rate.
        assistant_line_with_breakdown("2099-03-15T10:00:00Z", "claude-opus-4-7", 1_000_000, None),
        // Breakdown covers only 800k of 1M: the 200k remainder bills at 5m.
        assistant_line_with_breakdown(
            "2099-03-15T10:01:00Z", "claude-opus-4-7", 1_000_000, Some((400_000, 400_000)),
        ),
    ];
    write_transcript(&home, "t1", &lines);
    run(&home);

    let s = summary(&home);
    let m = &s["totals"]["by_model"]["claude-opus-4-7"];
    // opus-4-7 cache rates: 5m 6.25, 1h 10.
    // Line 1: 1M at 5m = 6.25. Line 2: 0.4*6.25 + 0.4*10 + 0.2*6.25 = 7.75.
    approx(cost(m), 14.0);
    // The split is stored as reported, not inflated by the billed remainder.
    assert_eq!(u(m, "cache_5m_tokens"), 400_000);
    assert_eq!(u(m, "cache_1h_tokens"), 400_000);
    assert_eq!(u(m, "cache_creation_tokens"), 2_000_000);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn legacy_cache_entry_without_split_reprices_remainder_at_5m() {
    let home = enabled("UTC");
    // A cache written by an older binary: no cache_5m/1h split fields, and
    // cost 0 because the model's pricing wasn't known yet. No transcripts.
    let tokens = json!({
        "input_tokens": 1_000_000, "output_tokens": 0,
        "cache_creation_tokens": 1_000_000, "cache_read_tokens": 0, "cost_usd": 0.0
    });
    let bkt = json!({
        "input_tokens": 1_000_000, "output_tokens": 0,
        "cache_creation_tokens": 1_000_000, "cache_read_tokens": 0, "cost_usd": 0.0,
        "by_model": { "claude-fable-5": tokens }
    });
    let cache = json!({
        "files": {}, "totals": bkt.clone(),
        "daily": { "2099-03-15": bkt }, "weekly": {}, "monthly": {}, "sessions": {}
    });
    fs::create_dir_all(home.join(".claude/usage")).unwrap();
    fs::write(home.join(".claude/usage/usage-cache.json"), cache.to_string()).unwrap();
    run(&home);

    // fable: 1M input * $10 + 1M creation billed wholly at the 5m rate $12.50.
    let s = summary(&home);
    approx(cost(&s["totals"]), 22.50);
    approx(cost(&s["totals"]["by_model"]["claude-fable-5"]), 22.50);
    assert!(s["totals"].get("unpriced_tokens").is_none());
    approx(cost(bucket(&s, "daily", "2099-03-15")), 22.50);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn steady_state_run_does_not_rewrite_outputs() {
    let home = enabled("UTC");
    // Unpriced usage is the case most at risk of a rewrite loop: the reprice
    // pass re-derives unpriced_tokens every run and must not report a change.
    let line = assistant_line(
        "2099-03-15T10:00:00Z", "s1", "/w", "claude-unknown-9", 1_000_000, 0, 0, 0, 0,
    );
    write_transcript(&home, "t1", &[line]);
    run(&home);
    assert_eq!(u(&summary(&home)["totals"], "unpriced_tokens"), 1_000_000);

    // Nothing changed since: a second run must not re-fold or rewrite outputs.
    fs::remove_file(home.join(".claude/usage/usage-summary.json")).unwrap();
    run(&home);
    assert!(
        !home.join(".claude/usage/usage-summary.json").exists(),
        "summary rewritten on a no-op run"
    );

    fs::remove_dir_all(&home).ok();
}
