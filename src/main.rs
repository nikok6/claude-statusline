use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::process::Command;

// ANSI colors (Catppuccin 256-color)
const COLOR_BRANCH: &str = "\x1b[38;5;111m";
const COLOR_ADDED: &str = "\x1b[38;5;151m";
const COLOR_REMOVED: &str = "\x1b[38;5;211m";
const COLOR_MODEL: &str = "\x1b[38;5;183m";
const COLOR_TOKENS: &str = "\x1b[38;5;216m";
const COLOR_RESET: &str = "\x1b[0m";

#[derive(Deserialize)]
struct Input {
    cwd: String,
    transcript_path: String,
    model: Model,
    context_window: Option<ContextWindow>,
}

#[derive(Deserialize)]
struct Model {
    display_name: String,
}

#[derive(Deserialize)]
struct ContextWindow {
    current_usage: Option<CurrentUsage>,
    context_window_size: Option<u64>,
}

#[derive(Deserialize)]
struct CurrentUsage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct TranscriptEntry {
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<ToolUseResult>,
}

#[derive(Deserialize)]
struct ToolUseResult {
    #[serde(rename = "filePath")]
    file_path: Option<String>,
    #[serde(rename = "originalFile")]
    original_file: Option<String>,
    content: Option<String>,
    #[serde(rename = "oldString")]
    old_string: Option<String>,
    #[serde(rename = "newString")]
    new_string: Option<String>,
}

fn get_git_branch(cwd: &str) -> String {
    Command::new("git")
        .args(["-C", cwd, "branch", "--show-current"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "no-git".to_string())
}

fn calculate_net_diff(transcript_path: &str) -> (usize, usize) {
    let file = match File::open(transcript_path) {
        Ok(f) => f,
        Err(_) => return (0, 0),
    };

    let tmp_dir = match std::env::temp_dir().join("statusline").to_str() {
        Some(s) => s.to_string(),
        None => return (0, 0),
    };
    let _ = fs::create_dir_all(&tmp_dir);

    let reader = BufReader::new(file);
    // Track per-file: (original content at first touch, last content we wrote)
    let mut file_originals: HashMap<String, String> = HashMap::new();
    let mut file_finals: HashMap<String, String> = HashMap::new();
    // Track edit chains: maps current content -> original content
    // When edits chain (edit2.old == edit1.new), we track back to the original
    let mut edit_chains: HashMap<String, String> = HashMap::new();

    for line in reader.lines().flatten() {
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(&line) {
            if let Some(result) = entry.tool_use_result {
                if let Some(ref file_path) = result.file_path {
                    // Write tool: track original and final content
                    if let Some(ref content) = result.content {
                        file_originals
                            .entry(file_path.clone())
                            .or_insert_with(|| result.original_file.clone().unwrap_or_default());
                        file_finals.insert(file_path.clone(), content.clone());
                    }
                    // Edit tool: track chains of edits to compute net change
                    else if let (Some(old_str), Some(new_str)) = (&result.old_string, &result.new_string) {
                        // Check if this edit applies to content we wrote
                        let applied_to_write = if let Some(final_content) = file_finals.get_mut(file_path) {
                            if final_content.contains(old_str.as_str()) {
                                *final_content = final_content.replacen(old_str, new_str, 1);
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        };

                        // If not applied to written content, track in edit chains
                        if !applied_to_write {
                            let original = edit_chains.remove(old_str).unwrap_or_else(|| old_str.clone());
                            edit_chains.insert(new_str.clone(), original);
                        }
                    }
                }
            }
        }
    }

    let mut added = 0;
    let mut removed = 0;

    // Compute diffs for Edit chains (original vs final for each chain)
    for (final_content, original) in &edit_chains {
        let (a, r) = compute_diff_strings(&tmp_dir, original, final_content);
        added += a;
        removed += r;
    }

    // Compute diffs for Write operations (original vs final)
    for (file_path, original) in &file_originals {
        // If file was deleted, skip it (net effect is zero for files we created)
        if !std::path::Path::new(file_path).exists() {
            continue;
        }

        let final_content = match file_finals.get(file_path) {
            Some(content) => content,
            None => continue,
        };

        let (a, r) = compute_diff_strings(&tmp_dir, original, final_content);
        added += a;
        removed += r;
    }

    let _ = fs::remove_dir_all(&tmp_dir);
    (added, removed)
}

fn compute_diff_strings(tmp_dir: &str, old: &str, new: &str) -> (usize, usize) {
    let tmp_old = format!("{}/old", tmp_dir);
    let tmp_new = format!("{}/new", tmp_dir);
    if fs::write(&tmp_old, old).is_err() || fs::write(&tmp_new, new).is_err() {
        return (0, 0);
    }
    compute_diff(&tmp_old, &tmp_new)
}

fn compute_diff(original_file: &str, current_file: &str) -> (usize, usize) {
    let output = Command::new("diff")
        .args([original_file, current_file])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let added = output.lines().filter(|l| l.starts_with('>')).count();
    let removed = output.lines().filter(|l| l.starts_with('<')).count();
    (added, removed)
}

fn get_token_info(input: &Input) -> String {
    let ctx = match &input.context_window {
        Some(c) => c,
        None => return String::new(),
    };

    let size = ctx.context_window_size.unwrap_or(0);
    if size == 0 {
        return String::new();
    }

    let usage = ctx.current_usage.as_ref();
    let current = usage
        .map(|u| {
            u.input_tokens.unwrap_or(0)
                + u.cache_creation_input_tokens.unwrap_or(0)
                + u.cache_read_input_tokens.unwrap_or(0)
        })
        .unwrap_or(0);

    let pct = (current * 100) / size;
    let filled = (pct / 20) as usize;
    let bar: String = "\u{25B0}".repeat(filled) + &"\u{25B1}".repeat(5 - filled);

    let current_k = current / 1000;
    let size_k = size / 1000;

    format!(
        "{}{}  {}k/{}k tokens{}",
        COLOR_TOKENS, bar, current_k, size_k, COLOR_RESET
    )
}

fn main() {
    let input: Input = match serde_json::from_reader(io::stdin()) {
        Ok(i) => i,
        Err(_) => std::process::exit(1),
    };

    let git_branch = get_git_branch(&input.cwd);
    let model_name = &input.model.display_name;
    let (added, removed) = calculate_net_diff(&input.transcript_path);
    let token_info = get_token_info(&input);

    println!(
        "{}{}{} | {}+{}{} {}-{}{} | {}{}{} | {}",
        COLOR_BRANCH, git_branch, COLOR_RESET,
        COLOR_ADDED, added, COLOR_RESET,
        COLOR_REMOVED, removed, COLOR_RESET,
        COLOR_MODEL, model_name, COLOR_RESET,
        token_info
    );
}
