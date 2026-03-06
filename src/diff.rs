use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

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

    if file.seek(SeekFrom::Start(byte_offset)).is_err() {
        return true;
    }

    let mut new_content = String::new();
    if file.read_to_string(&mut new_content).is_err() {
        return true;
    }

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
