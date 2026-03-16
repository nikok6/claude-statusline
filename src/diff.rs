use crate::cache;
use serde::Deserialize;
use similar::{ChangeTag, TextDiff};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

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

fn parse_transcript(transcript_path: &str) -> (HashMap<String, String>, HashMap<String, String>, HashMap<String, Vec<(String, String)>>) {
    let file = match File::open(transcript_path) {
        Ok(f) => f,
        Err(_) => return (HashMap::new(), HashMap::new(), HashMap::new()),
    };

    let exclude_dirs: Vec<String> = std::env::var("HOME")
        .map(|h| vec![format!("{}/.claude/plans/", h), format!("{}/.claude/projects/", h)])
        .unwrap_or_default();

    let reader = BufReader::new(file);
    let mut file_originals: HashMap<String, String> = HashMap::new();
    let mut file_finals: HashMap<String, String> = HashMap::new();
    let mut edit_chains: HashMap<String, Vec<(String, String)>> = HashMap::new();

    for line in reader.lines().flatten() {
        if !line.contains("\"toolUseResult\"") {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(&line) {
            if let Some(result) = entry.tool_use_result {
                if let Some(ref file_path) = result.file_path {
                    if exclude_dirs.iter().any(|d| file_path.starts_with(d)) {
                        continue;
                    }
                    if let Some(ref content) = result.content {
                        file_originals
                            .entry(file_path.clone())
                            .or_insert_with(|| result.original_file.clone().unwrap_or_default());
                        file_finals.insert(file_path.clone(), content.clone());
                        edit_chains.remove(file_path);
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
                                // Absorb any existing chains whose current is inside this edit's old_string
                                let mut resolved_old = old_str.clone();
                                chains.retain(|(orig, cur)| {
                                    if resolved_old.contains(cur.as_str()) {
                                        resolved_old = resolved_old.replacen(cur, orig, 1);
                                        false
                                    } else {
                                        true
                                    }
                                });
                                chains.push((resolved_old, new_str.clone()));
                            }
                        }
                    }
                }
            }
        }
    }

    (file_originals, file_finals, edit_chains)
}

pub fn calculate_net_diff(transcript_path: &str) -> (usize, usize) {
    let cache_path = cache::get_cache_path(transcript_path);

    // Try cache first
    if let Some(c) = cache::load(&cache_path, transcript_path) {
        return (c.added, c.removed);
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

    // Save cache, preserving existing claude_pid
    let existing_pid = cache::load_raw(&cache_path).and_then(|c| c.claude_pid);
    cache::save(&cache_path, &cache::Cache {
        byte_offset: cache::get_file_size(transcript_path),
        added,
        removed,
        files,
        claude_pid: existing_pid,
    });

    (added, removed)
}

fn ensure_trailing_newline(s: &str) -> Cow<'_, str> {
    if s.is_empty() || s.ends_with('\n') {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(format!("{}\n", s))
    }
}

fn compute_diff(old: &str, new: &str) -> (usize, usize) {
    let old_normalized = ensure_trailing_newline(old);
    let new_normalized = ensure_trailing_newline(new);

    let diff = TextDiff::from_lines(&*old_normalized, &*new_normalized);
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
