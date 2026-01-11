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

    let reader = BufReader::new(file);
    let mut file_originals: HashMap<String, String> = HashMap::new();

    for line in reader.lines().flatten() {
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(&line) {
            if let Some(result) = entry.tool_use_result {
                if let Some(file_path) = result.file_path {
                    if result.original_file.is_some() || result.content.is_some() {
                        file_originals
                            .entry(file_path)
                            .or_insert_with(|| result.original_file.unwrap_or_default());
                    }
                }
            }
        }
    }

    let tmp_dir = match std::env::temp_dir().join("statusline").to_str() {
        Some(s) => s.to_string(),
        None => return (0, 0),
    };
    let _ = fs::create_dir_all(&tmp_dir);

    let mut added = 0;
    let mut removed = 0;

    for (file_path, original) in &file_originals {
        if !std::path::Path::new(file_path).exists() {
            removed += original.lines().filter(|l| !l.is_empty()).count();
            continue;
        }

        let tmp_original = format!("{}/original", tmp_dir);
        if fs::write(&tmp_original, original).is_err() {
            continue;
        }

        let (a, r) = compute_diff(&tmp_original, file_path);
        added += a;
        removed += r;
    }

    let _ = fs::remove_dir_all(&tmp_dir);
    (added, removed)
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
