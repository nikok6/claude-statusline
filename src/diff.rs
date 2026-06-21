use crate::cache;
use crate::fsutil;
use serde::Deserialize;
use similar::{ChangeTag, TextDiff};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct LineEntry {
    uuid: Option<String>,
    #[serde(rename = "parentUuid")]
    parent_uuid: Option<String>,
    timestamp: Option<String>,
    // Error results are plain strings, so parse lazily from a Value.
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<serde_json::Value>,
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
    #[serde(rename = "structuredPatch")]
    structured_patch: Option<Vec<Hunk>>,
}

#[derive(Deserialize)]
struct Hunk {
    #[serde(rename = "oldStart")]
    old_start: usize,
    #[serde(rename = "newStart")]
    new_start: usize,
    lines: Vec<String>,
}

/// A surviving file-edit record: (file path, timestamp, tool result).
type Record = (String, Option<String>, ToolUseResult);

/// Edit records from a single transcript, in file order, restricted to the
/// active conversation branch.
fn parse_records(transcript_path: &str) -> Vec<Record> {
    let file = match File::open(transcript_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let exclude_dirs: Vec<String> = std::env::var("HOME")
        .map(|h| vec![format!("{}/.claude/plans/", h), format!("{}/.claude/projects/", h)])
        .unwrap_or_default();

    let reader = BufReader::new(file);
    let mut parents: HashMap<String, Option<String>> = HashMap::new();
    let mut last_uuid: Option<String> = None;
    let mut records: Vec<(String, Option<String>, Option<String>, ToolUseResult)> = Vec::new();

    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("\"uuid\"") && !line.contains("\"toolUseResult\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<LineEntry>(&line) else { continue };
        if let Some(u) = &entry.uuid {
            parents.insert(u.clone(), entry.parent_uuid.clone());
            last_uuid = Some(u.clone());
        }
        if let Some(v) = entry.tool_use_result
            && v.is_object()
            && let Ok(result) = serde_json::from_value::<ToolUseResult>(v)
                && let Some(file_path) = result.file_path.clone() {
                    if exclude_dirs.iter().any(|d| file_path.starts_with(d)) {
                        continue;
                    }
                    records.push((file_path, entry.uuid.clone(), entry.timestamp.clone(), result));
                }
    }

    // The transcript is a tree: rewinding a session abandons a branch whose
    // edits were rolled back. The active path is the parent chain of the last
    // uuid-bearing entry. An entry is abandoned only if it hangs off the
    // active path without being on it; entries in disconnected components
    // (sidechains, pre-compact history) still count.
    let mut active: HashSet<String> = HashSet::new();
    let mut cur = last_uuid;
    while let Some(u) = cur {
        if !active.insert(u.clone()) {
            break;
        }
        cur = parents.get(&u).cloned().flatten();
    }

    let mut memo: HashMap<String, bool> = HashMap::new();
    let mut surviving: Vec<Record> = Vec::new();
    for (file_path, uuid, ts, result) in records {
        if let Some(u) = &uuid
            && is_abandoned(u, &parents, &active, &mut memo) {
                continue;
            }
        surviving.push((file_path, ts, result));
    }

    surviving
}

/// Subagent transcripts for a session live at `<dir>/<stem>/subagents/*.jsonl`
/// alongside the main `<dir>/<stem>.jsonl`. Their edits target real files and
/// belong in the session's diff, so they're folded in with the main records.
fn subagent_transcripts(transcript_path: &str) -> Vec<PathBuf> {
    let p = Path::new(transcript_path);
    let (Some(stem), Some(parent)) = (p.file_stem().and_then(|s| s.to_str()), p.parent()) else {
        return Vec::new();
    };
    fsutil::jsonl_files(&parent.join(stem).join(fsutil::SUBAGENTS_DIR))
}

/// Combined byte size of the subagent transcripts — the cache freshness signal
/// (a new subagent file or a grown one changes this; see `cache::load`).
fn subagent_signature(paths: &[PathBuf]) -> u64 {
    paths.iter().map(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0)).sum()
}

/// Sort key for ordering records across the main and subagent transcripts.
/// Second precision is enough and is lexically ordered for RFC3339; finer
/// ordering within a second falls back to the stable sort's original order.
fn ts_key(ts: &Option<String>) -> &str {
    ts.as_deref().map(|s| s.get(0..19).unwrap_or(s)).unwrap_or("")
}

/// Merges the main transcript's records with every subagent transcript's,
/// ordered chronologically so a file edited by both reconstructs correctly.
fn collect_records(transcript_path: &str, subagents: &[PathBuf]) -> (Vec<String>, HashMap<String, Vec<ToolUseResult>>) {
    let mut records = parse_records(transcript_path);
    if !subagents.is_empty() {
        for sub in subagents {
            records.extend(parse_records(&sub.to_string_lossy()));
        }
        // Merge the transcripts chronologically. Stable, so records with equal
        // (or missing) timestamps keep their original per-file order. Skipped
        // entirely when there are no subagents: the single-transcript path then
        // keeps its append-only file order, the reliable signal it always used,
        // without depending on every record carrying a timestamp.
        records.sort_by(|a, b| ts_key(&a.1).cmp(ts_key(&b.1)));
    }

    let mut order: Vec<String> = Vec::new();
    let mut by_file: HashMap<String, Vec<ToolUseResult>> = HashMap::new();
    for (file_path, _ts, result) in records {
        if !by_file.contains_key(&file_path) {
            order.push(file_path.clone());
        }
        by_file.entry(file_path).or_default().push(result);
    }
    (order, by_file)
}

fn is_abandoned(
    uuid: &str,
    parents: &HashMap<String, Option<String>>,
    active: &HashSet<String>,
    memo: &mut HashMap<String, bool>,
) -> bool {
    if active.contains(uuid) {
        return false;
    }
    if let Some(&v) = memo.get(uuid) {
        return v;
    }
    let mut chain = vec![uuid.to_string()];
    let mut seen: HashSet<String> = chain.iter().cloned().collect();
    let mut cur = parents.get(uuid).cloned().flatten();
    let mut abandoned = false;
    while let Some(p) = cur {
        if active.contains(&p) {
            abandoned = true;
            break;
        }
        if let Some(&v) = memo.get(&p) {
            abandoned = v;
            break;
        }
        if !seen.insert(p.clone()) {
            break;
        }
        chain.push(p.clone());
        cur = parents.get(&p).cloned().flatten();
    }
    for c in chain {
        memo.insert(c, abandoned);
    }
    abandoned
}

pub fn calculate_net_diff(transcript_path: &str) -> (usize, usize) {
    let cache_path = cache::get_cache_path(transcript_path);
    let subagents = subagent_transcripts(transcript_path);
    let subagent_sig = subagent_signature(&subagents);

    // Try cache first
    if let Some(c) = cache::load(&cache_path, transcript_path, subagent_sig) {
        return (c.added, c.removed);
    }

    // Cache miss: parse and compute
    let (order, mut by_file) = collect_records(transcript_path, &subagents);

    let mut added = 0;
    let mut removed = 0;
    let mut files = Vec::new();

    for file_path in order {
        let Some(records) = by_file.remove(&file_path) else { continue };
        if !std::path::Path::new(&file_path).exists() {
            continue;
        }
        files.push(file_path);
        let (a, r) = diff_for_file(&records);
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
        subagent_sig,
        claude_pid: existing_pid,
    });

    (added, removed)
}

fn diff_for_file(records: &[ToolUseResult]) -> (usize, usize) {
    // structuredPatch is authoritative: originalFile is sometimes empty or
    // missing even when the file had content, which would turn rewrites into
    // pure additions. Reconstruct before/after states from the patches and
    // fall back to oldString/newString chaining when that isn't possible.
    if records.iter().all(|r| r.structured_patch.is_some())
        && let Some((before, after)) = patch_reconstruct(records) {
            return compute_diff(&join_normalized(&before), &join_normalized(&after));
        }
    legacy_diff(records)
}

fn split_lines(s: &str) -> Vec<String> {
    s.lines().map(str::to_string).collect()
}

/// A Write that created a new file: it has content, an empty structuredPatch
/// (Claude Code emits no diff for a fresh file), and no non-empty originalFile.
fn is_creation_write(r: &ToolUseResult) -> bool {
    r.content.is_some()
        && r.structured_patch.as_deref().is_some_and(|p| p.is_empty())
        && r.original_file.as_deref().is_none_or(str::is_empty)
}

/// structuredPatch lines store tabs as two spaces; file snapshots keep real tabs.
fn norm_ws(s: &str) -> Cow<'_, str> {
    if s.contains('\t') { Cow::Owned(s.replace('\t', "  ")) } else { Cow::Borrowed(s) }
}

fn join_normalized(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for l in lines {
        out.push_str(&norm_ws(l));
        out.push('\n');
    }
    out
}

/// Rebuild the file's content before the first record and after the last one.
/// Anchored at the latest full snapshot (a non-empty originalFile holds the
/// content just before its record; a Write's content holds the content just
/// after); earlier patches are un-applied backward, later ones applied forward.
fn patch_reconstruct(records: &[ToolUseResult]) -> Option<(Vec<String>, Vec<String>)> {
    let orig_anchor = records
        .iter()
        .rposition(|r| r.original_file.as_deref().is_some_and(|s| !s.is_empty()));
    let content_anchor = records.iter().rposition(|r| r.content.is_some());

    // (records to un-apply backward, anchor snapshot, first record to apply forward)
    let (backward, anchor, forward) = match (orig_anchor, content_anchor) {
        (Some(i), Some(j)) if j >= i => (j + 1, split_lines(records[j].content.as_ref()?), j + 1),
        (Some(i), _) => (i, split_lines(records[i].original_file.as_ref()?), i),
        (None, Some(j)) => (j + 1, split_lines(records[j].content.as_ref()?), j + 1),
        (None, None) => return None,
    };

    // A Write that creates a new file carries content but an empty patch and an
    // empty/missing originalFile — an empty patch over non-empty content is only
    // consistent with the file not having existed before. The empty patch holds
    // no diff to un-apply, so the backward walk would wrongly leave `before`
    // equal to the created content. The true prior state is empty.
    if records.first().is_some_and(is_creation_write) {
        let mut after = anchor;
        for r in &records[forward..] {
            after = apply_patch(&after, r.structured_patch.as_ref()?, false)?;
        }
        return Some((Vec::new(), after));
    }

    let mut before = anchor.clone();
    for r in records[..backward].iter().rev() {
        before = apply_patch(&before, r.structured_patch.as_ref()?, true)?;
    }
    let mut after = anchor;
    for r in &records[forward..] {
        after = apply_patch(&after, r.structured_patch.as_ref()?, false)?;
    }
    Some((before, after))
}

fn apply_patch(lines: &[String], hunks: &[Hunk], reverse: bool) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut pos = 0usize;
    for hunk in hunks {
        let start = (if reverse { hunk.new_start } else { hunk.old_start }).saturating_sub(1);
        if start < pos || start > lines.len() {
            return None;
        }
        out.extend_from_slice(&lines[pos..start]);
        pos = start;
        for raw in &hunk.lines {
            let tag = raw.chars().next().unwrap_or(' ');
            let text = raw.get(1..).unwrap_or("");
            let tag = match (reverse, tag) {
                (true, '+') => '-',
                (true, '-') => '+',
                (_, t) => t,
            };
            match tag {
                ' ' => {
                    if pos >= lines.len() || norm_ws(&lines[pos]) != norm_ws(text) {
                        return None;
                    }
                    out.push(lines[pos].clone());
                    pos += 1;
                }
                '-' => {
                    if pos >= lines.len() || norm_ws(&lines[pos]) != norm_ws(text) {
                        return None;
                    }
                    pos += 1;
                }
                '+' => out.push(text.to_string()),
                '\\' => {} // "\ No newline at end of file"
                _ => return None,
            }
        }
    }
    out.extend_from_slice(&lines[pos..]);
    Some(out)
}

fn legacy_diff(records: &[ToolUseResult]) -> (usize, usize) {
    let mut file_original: Option<String> = None;
    let mut file_final: Option<String> = None;
    let mut chains: Vec<(String, String)> = Vec::new();

    for result in records {
        if let Some(content) = &result.content {
            if file_original.is_none() {
                file_original = Some(result.original_file.clone().unwrap_or_default());
            }
            file_final = Some(content.clone());
            chains.clear();
        } else if let (Some(old_str), Some(new_str)) = (&result.old_string, &result.new_string) {
            let applied_to_write = if let Some(final_content) = file_final.as_mut() {
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

    let mut added = 0;
    let mut removed = 0;
    for (original, final_content) in &chains {
        let (a, r) = compute_diff(original, final_content);
        added += a;
        removed += r;
    }
    if let (Some(original), Some(final_content)) = (&file_original, &file_final) {
        let (a, r) = compute_diff(original, final_content);
        added += a;
        removed += r;
    }
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
