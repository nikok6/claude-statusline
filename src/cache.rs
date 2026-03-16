use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};

#[derive(Serialize, Deserialize, Default)]
pub struct Cache {
    pub byte_offset: u64,
    pub added: usize,
    pub removed: usize,
    pub files: Vec<String>,
    #[serde(default)]
    pub claude_pid: Option<u32>,
}

pub fn get_cache_path(transcript_path: &str) -> String {
    let name = std::path::Path::new(transcript_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    format!("/tmp/statusline_cache_{}.json", name)
}

pub fn load(cache_path: &str, transcript_path: &str) -> Option<Cache> {
    let content = fs::read_to_string(cache_path).ok()?;
    let cache: Cache = serde_json::from_str(&content).ok()?;

    if !cache.files.iter().all(|f| std::path::Path::new(f).exists()) {
        return None;
    }

    if has_new_file_ops(transcript_path, cache.byte_offset) {
        return None;
    }

    Some(cache)
}

pub fn load_raw(cache_path: &str) -> Option<Cache> {
    let content = fs::read_to_string(cache_path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save(cache_path: &str, cache: &Cache) {
    if let Ok(content) = serde_json::to_string(cache) {
        let _ = fs::write(cache_path, content);
    }
}

pub fn get_file_size(path: &str) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
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
