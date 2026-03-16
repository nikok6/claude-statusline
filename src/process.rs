use crate::cache;
use std::collections::HashMap;
use std::process::Command;

pub struct ClaudeStats {
    pub cpu: String,
    pub ram: String,
}

fn is_claude_comm(comm: &str) -> bool {
    comm == "claude" || comm.ends_with("/claude")
}

fn find_claude_pid() -> Option<u32> {
    let output = Command::new("ps")
        .args(["-eo", "pid=,ppid=,comm="])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);

    let mut parent_of: HashMap<u32, u32> = HashMap::new();
    let mut claude_pids: Vec<u32> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        let Some((pid_str, rest)) = line.split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        let Some((ppid_str, comm)) = rest.split_once(char::is_whitespace) else { continue };
        let Ok(pid) = pid_str.parse::<u32>() else { continue };
        let Ok(ppid) = ppid_str.trim().parse::<u32>() else { continue };
        parent_of.insert(pid, ppid);
        if is_claude_comm(comm.trim()) {
            claude_pids.push(pid);
        }
    }

    let mut pid = std::process::id();
    for _ in 0..64 {
        if claude_pids.contains(&pid) {
            return Some(pid);
        }
        let ppid = *parent_of.get(&pid)?;
        if ppid == 1 || ppid == pid {
            return None;
        }
        pid = ppid;
    }
    None
}

fn get_stats_for_pid(pid: u32) -> Option<ClaudeStats> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm=,%cpu=,rss="])
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&output.stdout);
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let (comm, rest) = line.split_once(char::is_whitespace)?;
    if !is_claude_comm(comm.trim()) {
        return None;
    }
    let rest = rest.trim();
    let (cpu_str, rss_str) = rest.split_once(char::is_whitespace)?;
    let rss_kb: u64 = rss_str.trim().parse().ok()?;
    let ram_mb = rss_kb / 1024;
    Some(ClaudeStats {
        cpu: format!("{}%", cpu_str.trim()),
        ram: format!("{}MB", ram_mb),
    })
}

pub fn get_claude_stats(transcript_path: &str) -> Option<ClaudeStats> {
    let cache_path = cache::get_cache_path(transcript_path);
    let cached = cache::load_raw(&cache_path);

    // Try cached PID first — single ps call validates + gets stats
    if let Some(ref c) = cached {
        if let Some(pid) = c.claude_pid {
            if let Some(stats) = get_stats_for_pid(pid) {
                return Some(stats);
            }
        }
    }

    // Cache miss or stale PID: walk the process tree
    let pid = find_claude_pid()?;

    // Update cache if readable, otherwise just skip caching
    if let Some(mut c) = cache::load_raw(&cache_path) {
        c.claude_pid = Some(pid);
        cache::save(&cache_path, &c);
    }

    get_stats_for_pid(pid)
}
