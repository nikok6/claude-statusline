use std::fs;
use std::path::{Path, PathBuf};

/// Subdirectory under a session that holds its subagent (sidechain) transcripts.
pub const SUBAGENTS_DIR: &str = "subagents";

/// The `*.jsonl` files directly inside `dir` (non-recursive). Empty if `dir`
/// is missing or unreadable.
pub fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
    out
}
