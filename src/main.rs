use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::collections::HashMap;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::process::Command;

// ANSI colors - Light mode (Catppuccin Latte 256-color)
const COLOR_BRANCH_LIGHT: &str = "\x1b[38;5;32m";
const COLOR_ADDED_LIGHT: &str = "\x1b[38;5;71m";
const COLOR_REMOVED_LIGHT: &str = "\x1b[38;5;131m";
const COLOR_MODEL_LIGHT: &str = "\x1b[38;5;97m";
const COLOR_TOKENS_LIGHT: &str = "\x1b[38;5;173m";

// ANSI colors - Dark mode (Catppuccin Mocha 256-color)
const COLOR_BRANCH_DARK: &str = "\x1b[38;5;117m";
const COLOR_ADDED_DARK: &str = "\x1b[38;5;114m";
const COLOR_REMOVED_DARK: &str = "\x1b[38;5;210m";
const COLOR_MODEL_DARK: &str = "\x1b[38;5;183m";
const COLOR_TOKENS_DARK: &str = "\x1b[38;5;215m";

const COLOR_RESET: &str = "\x1b[0m";

struct Colors {
    branch: &'static str,
    added: &'static str,
    removed: &'static str,
    model: &'static str,
    tokens: &'static str,
}

fn detect_theme() -> Colors {
    let is_light = std::env::var("HOME")
        .ok()
        .and_then(|home| fs::read_to_string(format!("{}/.claude.json", home)).ok())
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|json| json.get("theme").and_then(|v| v.as_str()).map(String::from))
        .map(|theme| theme.contains("light"))
        .unwrap_or(false);

    if is_light {
        Colors {
            branch: COLOR_BRANCH_LIGHT,
            added: COLOR_ADDED_LIGHT,
            removed: COLOR_REMOVED_LIGHT,
            model: COLOR_MODEL_LIGHT,
            tokens: COLOR_TOKENS_LIGHT,
        }
    } else {
        Colors {
            branch: COLOR_BRANCH_DARK,
            added: COLOR_ADDED_DARK,
            removed: COLOR_REMOVED_DARK,
            model: COLOR_MODEL_DARK,
            tokens: COLOR_TOKENS_DARK,
        }
    }
}

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

#[derive(Serialize, Deserialize)]
struct DiffCache {
    byte_offset: u64,
    added: usize,
    removed: usize,
    files: Vec<String>,
}

fn get_cache_path(transcript_path: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    transcript_path.hash(&mut hasher);
    format!("/tmp/statusline_cache_{:x}.json", hasher.finish())
}

fn has_new_file_ops(transcript_path: &str, byte_offset: u64) -> bool {
    let mut file = match File::open(transcript_path) {
        Ok(f) => f,
        Err(_) => return true,
    };

    // Seek to last known position
    if file.seek(SeekFrom::Start(byte_offset)).is_err() {
        return true;
    }

    // Read new content and check for filePath
    let mut new_content = String::new();
    if file.read_to_string(&mut new_content).is_err() {
        return true;
    }

    // Fast string check - if "filePath" appears in new content, we have new file ops
    new_content.contains("\"filePath\"")
}

fn get_file_size(path: &str) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn load_cache(cache_path: &str, transcript_path: &str) -> Option<DiffCache> {
    let content = fs::read_to_string(cache_path).ok()?;
    let cache: DiffCache = serde_json::from_str(&content).ok()?;

    // Check if any tracked file was deleted
    if !cache.files.iter().all(|f| std::path::Path::new(f).exists()) {
        return None;
    }

    // Check if there are new file operations since last cache
    if has_new_file_ops(transcript_path, cache.byte_offset) {
        return None;
    }

    Some(cache)
}

fn save_cache(cache_path: &str, cache: &DiffCache) {
    if let Ok(content) = serde_json::to_string(cache) {
        let _ = fs::write(cache_path, content);
    }
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

fn parse_transcript(transcript_path: &str) -> (HashMap<String, String>, HashMap<String, String>, HashMap<String, Vec<(String, String)>>) {
    let file = match File::open(transcript_path) {
        Ok(f) => f,
        Err(_) => return (HashMap::new(), HashMap::new(), HashMap::new()),
    };

    let reader = BufReader::new(file);
    let mut file_originals: HashMap<String, String> = HashMap::new();
    let mut file_finals: HashMap<String, String> = HashMap::new();
    let mut edit_chains: HashMap<String, Vec<(String, String)>> = HashMap::new();

    for line in reader.lines().flatten() {
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(&line) {
            if let Some(result) = entry.tool_use_result {
                if let Some(ref file_path) = result.file_path {
                    if let Some(ref content) = result.content {
                        file_originals
                            .entry(file_path.clone())
                            .or_insert_with(|| result.original_file.clone().unwrap_or_default());
                        file_finals.insert(file_path.clone(), content.clone());
                    } else if let (Some(old_str), Some(new_str)) = (&result.old_string, &result.new_string) {
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

                        if !applied_to_write {
                            let chains = edit_chains.entry(file_path.clone()).or_default();
                            let mut found = false;

                            for (_original, current) in chains.iter_mut() {
                                if current.contains(old_str.as_str()) {
                                    *current = current.replacen(old_str, new_str, 1);
                                    found = true;
                                    break;
                                }
                            }

                            if !found {
                                chains.push((old_str.clone(), new_str.clone()));
                            }
                        }
                    }
                }
            }
        }
    }

    (file_originals, file_finals, edit_chains)
}

fn calculate_net_diff(transcript_path: &str) -> (usize, usize) {
    let cache_path = get_cache_path(transcript_path);

    // Try cache first
    if let Some(cache) = load_cache(&cache_path, transcript_path) {
        return (cache.added, cache.removed);
    }

    // Cache miss: parse and compute
    let (file_originals, file_finals, edit_chains) = parse_transcript(transcript_path);

    let mut added = 0;
    let mut removed = 0;
    let mut files = Vec::new();

    for (file_path, chains) in &edit_chains {
        if !std::path::Path::new(file_path).exists() {
            continue;
        }
        files.push(file_path.clone());
        for (original, final_content) in chains {
            let (a, r) = compute_diff(original, final_content);
            added += a;
            removed += r;
        }
    }

    for (file_path, original) in &file_originals {
        if !std::path::Path::new(file_path).exists() {
            continue;
        }
        files.push(file_path.clone());

        let final_content = match file_finals.get(file_path) {
            Some(content) => content,
            None => continue,
        };

        let (a, r) = compute_diff(original, final_content);
        added += a;
        removed += r;
    }

    // Save cache with current file size as byte offset
    save_cache(&cache_path, &DiffCache {
        byte_offset: get_file_size(transcript_path),
        added,
        removed,
        files,
    });

    (added, removed)
}

fn compute_diff(old: &str, new: &str) -> (usize, usize) {
    // Normalize trailing newlines to avoid spurious diffs
    let old_normalized = if old.is_empty() || old.ends_with('\n') {
        old.to_string()
    } else {
        format!("{}\n", old)
    };
    let new_normalized = if new.is_empty() || new.ends_with('\n') {
        new.to_string()
    } else {
        format!("{}\n", new)
    };

    let diff = TextDiff::from_lines(&old_normalized, &new_normalized);
    let mut added = 0;
    let mut removed = 0;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

fn get_token_info(input: &Input, colors: &Colors) -> String {
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
        "{}{} {}k/{}k tokens{}",
        colors.tokens, bar, current_k, size_k, COLOR_RESET
    )
}

fn main() {
    let input: Input = match serde_json::from_reader(io::stdin()) {
        Ok(i) => i,
        Err(_) => std::process::exit(1),
    };

    let colors = detect_theme();
    let git_branch = get_git_branch(&input.cwd);
    let model_name = &input.model.display_name;
    let (added, removed) = calculate_net_diff(&input.transcript_path);
    let token_info = get_token_info(&input, &colors);

    println!(
        "{}{}{} | {}+{}{} {}-{}{} | {}{}{} | {}",
        colors.branch, git_branch, COLOR_RESET,
        colors.added, added, COLOR_RESET,
        colors.removed, removed, COLOR_RESET,
        colors.model, model_name, COLOR_RESET,
        token_info
    );
}
