use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, SecondsFormat, TimeDelta};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::SystemTime;

/// Scratch cache for incremental parsing, kept in the output directory so that
/// a custom `output_path` (e.g. a bind-mounted volume) keeps the cache and
/// outputs together — and so concurrent writers sharing one output location
/// share one lock. Scoped per output location, so distinct configs/homes don't
/// corrupt each other's aggregates. Falls back to a fixed /tmp path if no
/// home/path.
fn cache_path(custom_path: Option<&str>) -> PathBuf {
    output_dir(custom_path)
        .map(|d| d.join("usage-cache.json"))
        .unwrap_or_else(|| PathBuf::from("/tmp/statusline_usage_cache.json"))
}

#[derive(Deserialize)]
struct TranscriptLine {
    #[serde(rename = "type")]
    line_type: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
}

#[derive(Deserialize)]
struct CacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Tokens {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    // 5m/1h split of cache_creation_tokens, kept so an entry folded while its
    // model had no known pricing can be costed exactly once a price is added.
    #[serde(default, skip_serializing_if = "is_zero")]
    cache_5m_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    cache_1h_tokens: u64,
    cost_usd: f64,
}

impl Tokens {
    fn add(&mut self, other: &Tokens) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_5m_tokens += other.cache_5m_tokens;
        self.cache_1h_tokens += other.cache_1h_tokens;
        self.cost_usd += other.cost_usd;
    }

    fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }

    fn cost_with(&self, p: &Pricing) -> f64 {
        // Creation tokens the recorded split doesn't cover (entries cached
        // before the split existed, a missing breakdown, or a reported total
        // exceeding 5m+1h) are billed at the 5m rate, so no creation tokens
        // are silently costed at zero.
        let m5 = self.cache_5m_tokens
            + self.cache_creation_tokens.saturating_sub(self.cache_5m_tokens + self.cache_1h_tokens);
        (self.input_tokens as f64) * p.input / 1_000_000.0
            + (self.output_tokens as f64) * p.output / 1_000_000.0
            + (m5 as f64) * p.cache_5m / 1_000_000.0
            + (self.cache_1h_tokens as f64) * p.cache_1h / 1_000_000.0
            + (self.cache_read_tokens as f64) * p.cache_read / 1_000_000.0
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Bucket {
    #[serde(flatten)]
    totals: Tokens,
    // Tokens counted under models price_for doesn't know (costed at $0 so far).
    #[serde(default, skip_serializing_if = "is_zero")]
    unpriced_tokens: u64,
    by_model: BTreeMap<String, Tokens>,
}

impl Bucket {
    fn add(&mut self, model: &str, t: &Tokens) {
        self.totals.add(t);
        self.by_model.entry(model.to_string()).or_default().add(t);
    }
}

#[derive(Serialize, Deserialize, Default)]
struct FileState {
    offset: u64,
    mtime_ms: u128,
}

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    files: HashMap<String, FileState>,
    totals: Bucket,
    daily: BTreeMap<String, Bucket>,
    weekly: BTreeMap<String, Bucket>,
    monthly: BTreeMap<String, Bucket>,
    #[serde(default)]
    sessions: BTreeMap<String, SessionBucket>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct SessionBucket {
    #[serde(flatten)]
    bucket: Bucket,
    first_seen: String,
    last_seen: String,
    cwd: Option<String>,
}

#[derive(Serialize)]
struct SummaryOutput<'a> {
    generated_at: String,
    timezone: &'a str,
    totals: &'a Bucket,
    daily: Vec<KeyedBucket<'a>>,
    weekly: Vec<KeyedBucket<'a>>,
    monthly: Vec<KeyedBucket<'a>>,
}

#[derive(Serialize)]
struct SessionsOutput<'a> {
    generated_at: String,
    timezone: &'a str,
    sessions: &'a BTreeMap<String, SessionBucket>,
}

#[derive(Serialize)]
struct KeyedBucket<'a> {
    key: &'a str,
    #[serde(flatten)]
    bucket: &'a Bucket,
}

struct Pricing {
    input: f64,
    output: f64,
    cache_5m: f64,
    cache_1h: f64,
    cache_read: f64,
}

impl Pricing {
    const fn zero() -> Self {
        Pricing { input: 0.0, output: 0.0, cache_5m: 0.0, cache_1h: 0.0, cache_read: 0.0 }
    }
}

/// Matches a transcript model string against a known id: exact, or followed by
/// a date snapshot (`claude-opus-4-1-20250805`) and/or a bracket tag
/// (`claude-fable-5[1m]`). Bare version prefixes deliberately do NOT match, so
/// a future `claude-opus-4-9` stays unpriced (and is repriced once listed)
/// instead of silently inheriting another version's rates.
fn is_model(model: &str, id: &str) -> bool {
    let model = model.split('[').next().unwrap_or(model);
    let Some(rest) = model.strip_prefix(id) else { return false };
    rest.is_empty()
        || (rest.starts_with("-2") && rest.len() >= 5 && rest[1..].bytes().all(|b| b.is_ascii_digit()))
}

fn price_for(model: &str) -> Pricing {
    // Source: https://platform.claude.com/docs/en/about-claude/pricing
    // Each version is listed explicitly — see is_model. Opus 4.5+ uses the new
    // lower pricing; Opus 4.0/4.1 keep the legacy rate.
    let m = |id: &str| is_model(model, id);
    if m("claude-fable-5") || m("claude-mythos-5") {
        Pricing { input: 10.0, output: 50.0, cache_5m: 12.50, cache_1h: 20.0, cache_read: 1.0 }
    } else if m("claude-opus-4-5") || m("claude-opus-4-6") || m("claude-opus-4-7") || m("claude-opus-4-8") {
        Pricing { input: 5.0, output: 25.0, cache_5m: 6.25, cache_1h: 10.0, cache_read: 0.50 }
    } else if m("claude-opus-4") || m("claude-opus-4-1") {
        Pricing { input: 15.0, output: 75.0, cache_5m: 18.75, cache_1h: 30.0, cache_read: 1.50 }
    } else if m("claude-sonnet-5") {
        // Introductory pricing through 2026-08-31; reverts to $3/$15 (matching
        // Sonnet 4.x below) on 2026-09-01 — update these five rates then.
        Pricing { input: 2.0, output: 10.0, cache_5m: 2.50, cache_1h: 4.0, cache_read: 0.20 }
    } else if m("claude-sonnet-4") || m("claude-sonnet-4-5") || m("claude-sonnet-4-6") {
        Pricing { input: 3.0, output: 15.0, cache_5m: 3.75, cache_1h: 6.0, cache_read: 0.30 }
    } else if m("claude-haiku-4-5") {
        Pricing { input: 1.0, output: 5.0, cache_5m: 1.25, cache_1h: 2.0, cache_read: 0.10 }
    } else if m("claude-haiku-3-5") || m("claude-3-5-haiku") {
        Pricing { input: 0.80, output: 4.0, cache_5m: 1.00, cache_1h: 1.60, cache_read: 0.08 }
    } else {
        Pricing::zero()
    }
}

fn compute_tokens(model: &str, u: &Usage) -> Tokens {
    let input = u.input_tokens.unwrap_or(0);
    let output = u.output_tokens.unwrap_or(0);
    let cache_read = u.cache_read_input_tokens.unwrap_or(0);
    let cache_total = u.cache_creation_input_tokens.unwrap_or(0);
    // Store the breakdown as reported; cost_with bills any creation tokens it
    // doesn't cover (missing/short breakdown) at the 5m rate.
    let (cache_5m, cache_1h) = match &u.cache_creation {
        Some(c) => (c.ephemeral_5m_input_tokens.unwrap_or(0), c.ephemeral_1h_input_tokens.unwrap_or(0)),
        None => (0, 0),
    };

    let mut t = Tokens {
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: cache_total,
        cache_read_tokens: cache_read,
        cache_5m_tokens: cache_5m,
        cache_1h_tokens: cache_1h,
        cost_usd: 0.0,
    };
    t.cost_usd = t.cost_with(&price_for(model));
    t
}

/// Re-derives cost for model entries that were folded while their model had no
/// known pricing (cost 0 despite tokens), so adding the model to `price_for`
/// retroactively prices the cached history on the next run. Also refreshes the
/// bucket's `unpriced_tokens` rollup. Returns true if anything changed.
fn reprice_bucket(b: &mut Bucket) -> bool {
    let mut changed = false;
    let mut unpriced = 0u64;
    for (model, t) in b.by_model.iter_mut() {
        if t.cost_usd == 0.0 && t.total_tokens() > 0 {
            let cost = t.cost_with(&price_for(model));
            if cost > 0.0 {
                t.cost_usd = cost;
                b.totals.cost_usd += cost;
                changed = true;
            } else {
                unpriced += t.total_tokens();
            }
        }
    }
    if b.unpriced_tokens != unpriced {
        b.unpriced_tokens = unpriced;
        changed = true;
    }
    changed
}

fn reprice_cache(cache: &mut Cache) -> bool {
    let mut changed = reprice_bucket(&mut cache.totals);
    for b in cache
        .daily
        .values_mut()
        .chain(cache.weekly.values_mut())
        .chain(cache.monthly.values_mut())
    {
        changed |= reprice_bucket(b);
    }
    for s in cache.sessions.values_mut() {
        changed |= reprice_bucket(&mut s.bucket);
    }
    changed
}

/// Resolves a transcript timestamp to the calendar date its usage should be
/// bucketed under, in the configured timezone. Claude timestamps are UTC, so we
/// parse the leading `YYYY-MM-DDTHH:MM:SS` (ignoring fractional seconds and the
/// `Z`/zone designator) as naive UTC, then shift by the display offset.
fn bucket_date(ts: &str, offset_min: i32) -> Option<NaiveDate> {
    let naive = NaiveDateTime::parse_from_str(ts.get(0..19)?, "%Y-%m-%dT%H:%M:%S").ok()?;
    naive
        .checked_add_signed(TimeDelta::minutes(offset_min as i64))
        .map(|dt| dt.date())
}

fn daily_key(d: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

fn monthly_key(d: NaiveDate) -> String {
    format!("{:04}-{:02}", d.year(), d.month())
}

fn weekly_key(d: NaiveDate) -> String {
    let iso = d.iso_week();
    format!("{:04}-W{:02}", iso.year(), iso.week())
}

/// Chronological "a is before b" for transcript timestamps. Parses as RFC3339 so
/// mixed formats (fractional vs whole seconds, `Z` vs numeric offset) order by
/// actual instant rather than by bytes; falls back to lexical if either fails.
fn ts_before(a: &str, b: &str) -> bool {
    match (DateTime::parse_from_rfc3339(a), DateTime::parse_from_rfc3339(b)) {
        (Ok(x), Ok(y)) => x < y,
        _ => a < b,
    }
}

fn write_cache(lock_file: &mut File, cache: &Cache) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(cache).unwrap_or_default();
    lock_file.set_len(0)?;
    lock_file.seek(SeekFrom::Start(0))?;
    lock_file.write_all(&bytes)?;
    lock_file.flush()?;
    Ok(())
}

fn projects_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".claude/projects"))
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    PathBuf::from(p)
}

/// Directory holding the three output files. `output_path` always names a
/// directory (created on demand); default is `~/.claude/usage`.
fn output_dir(custom: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = custom {
        return Some(expand_tilde(p));
    }
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".claude/usage"))
}

fn summary_path(custom: Option<&str>) -> Option<PathBuf> {
    output_dir(custom).map(|d| d.join("usage-summary.json"))
}

fn sessions_path(custom: Option<&str>) -> Option<PathBuf> {
    output_dir(custom).map(|d| d.join("usage-sessions.json"))
}

fn list_transcripts() -> Vec<PathBuf> {
    let Some(root) = projects_dir() else { return Vec::new() };
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else { return out };
    for e in entries.flatten() {
        let project = e.path();
        let Ok(sub) = fs::read_dir(&project) else { continue };
        for f in sub.flatten() {
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                out.push(p);
            } else if p.is_dir() {
                // Subagent transcripts live one level deeper, under
                // `<session>/subagents/*.jsonl`. Their assistant turns carry
                // real (billed) usage that would otherwise go uncounted. They
                // share the parent's sessionId, so they fold into the same
                // session bucket; the usage entries are unique to this file,
                // so scanning it adds no double counting.
                out.extend(crate::fsutil::jsonl_files(&p.join(crate::fsutil::SUBAGENTS_DIR)));
            }
        }
    }
    out
}

fn metadata_mtime_ms(m: &fs::Metadata) -> u128 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Folds new complete lines of `path` starting at `from_offset`, returning the
/// new offset (bytes actually consumed) and how many usage entries were folded.
/// Only newline-terminated lines are consumed, so a partially-written final line
/// is left for the next pass rather than skipped-and-lost or double-counted.
fn process_new_lines(
    path: &std::path::Path,
    from_offset: u64,
    size: u64,
    cache: &mut Cache,
    tz_offset_min: i32,
) -> Option<(u64, u32)> {
    let mut file = File::open(path).ok()?;
    // Resume from where we left off, clamped to the current size. A file smaller
    // than our offset was truncated/rotated; we deliberately do NOT re-read from
    // zero (aggregates already include its earlier content, so that would
    // double-count) and instead resume from the new EOF.
    let start = from_offset.min(size);
    file.seek(SeekFrom::Start(start)).ok()?;

    let mut reader = BufReader::new(&mut file);
    let mut pos = start;
    let mut folded = 0u32;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            break; // EOF
        }
        if !line.ends_with('\n') {
            // Partially-written final line: leave it unconsumed (pos not advanced)
            // so it is re-read in full once the writer flushes the newline.
            break;
        }
        pos += n as u64;

        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<TranscriptLine>(&line) else { continue };
        if entry.line_type.as_deref() != Some("assistant") {
            continue;
        }
        let Some(ts) = entry.timestamp.as_deref() else { continue };
        let Some(date) = bucket_date(ts, tz_offset_min) else { continue };
        let Some(msg) = entry.message else { continue };
        let Some(usage) = msg.usage else { continue };
        let model = msg.model.unwrap_or_else(|| "unknown".to_string());
        if model.starts_with('<') {
            continue;
        }

        let tokens = compute_tokens(&model, &usage);

        cache.totals.add(&model, &tokens);
        cache.daily.entry(daily_key(date)).or_default().add(&model, &tokens);
        cache.weekly.entry(weekly_key(date)).or_default().add(&model, &tokens);
        cache.monthly.entry(monthly_key(date)).or_default().add(&model, &tokens);

        if let Some(sid) = entry.session_id {
            let s = cache.sessions.entry(sid).or_default();
            s.bucket.add(&model, &tokens);
            if s.first_seen.is_empty() || ts_before(ts, &s.first_seen) {
                s.first_seen = ts.to_string();
            }
            if s.last_seen.is_empty() || ts_before(&s.last_seen, ts) {
                s.last_seen = ts.to_string();
            }
            if s.cwd.is_none() {
                s.cwd = entry.cwd;
            }
        }
        folded += 1;
    }

    Some((pos, folded))
}

fn now_iso(offset_min: i32) -> String {
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let offset = FixedOffset::east_opt(offset_min * 60).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    let dt = DateTime::from_timestamp(secs, 0).unwrap_or_default().with_timezone(&offset);
    // `use_z = true` renders a UTC offset as `Z` and any other offset as `+HH:MM`.
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Returns (offset_minutes, canonical_label).
fn resolve_timezone(spec: Option<&str>) -> (i32, String) {
    let s = spec.map(|x| x.trim()).unwrap_or("");
    if s.is_empty() || s.eq_ignore_ascii_case("local") {
        if let Some(m) = local_offset_minutes() {
            return (m, format_offset_label(m));
        }
        return (0, "UTC".to_string());
    }
    if s.eq_ignore_ascii_case("utc") || s.eq_ignore_ascii_case("z") {
        return (0, "UTC".to_string());
    }
    let stripped = s.strip_prefix("UTC").or_else(|| s.strip_prefix("utc")).unwrap_or(s);
    if let Some(m) = parse_offset(stripped) {
        return (m, format_offset_label(m));
    }
    (0, "UTC".to_string())
}

fn format_offset_label(m: i32) -> String {
    if m == 0 { return "UTC".to_string(); }
    let sign = if m >= 0 { '+' } else { '-' };
    let abs = m.unsigned_abs();
    format!("UTC{}{:02}:{:02}", sign, abs / 60, abs % 60)
}

/// Parses `+HH:MM`, `+HHMM`, `+HH`, `+H`, with optional sign (default +).
fn parse_offset(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.is_empty() { return None; }
    let (sign, rest) = match s.as_bytes()[0] {
        b'+' => (1i32, &s[1..]),
        b'-' => (-1i32, &s[1..]),
        _ => (1i32, s),
    };
    let (h, m) = if let Some((h, m)) = rest.split_once(':') {
        (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?)
    } else if rest.len() == 4 && rest.chars().all(|c| c.is_ascii_digit()) {
        (rest[0..2].parse().ok()?, rest[2..4].parse().ok()?)
    } else {
        (rest.parse::<i32>().ok()?, 0)
    };
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) { return None; }
    Some(sign * (h * 60 + m))
}

fn local_offset_minutes() -> Option<i32> {
    for cmd in ["date", "/bin/date", "/usr/bin/date"] {
        if let Ok(out) = std::process::Command::new(cmd).arg("+%z").output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Some(off) = parse_offset(&s) {
                return Some(off);
            }
        }
    }
    None
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    // Per-PID temp name so two processes sharing one output_path (e.g. host +
    // devcontainer on a bind mount, which hold different cache locks) don't
    // clobber each other's temp file mid-write.
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn write_sessions(cache: &Cache, custom_path: Option<&str>, tz_offset: i32, tz_label: &str) -> std::io::Result<()> {
    let Some(path) = sessions_path(custom_path) else { return Ok(()) };
    let out = SessionsOutput {
        generated_at: now_iso(tz_offset),
        timezone: tz_label,
        sessions: &cache.sessions,
    };
    let bytes = serde_json::to_vec_pretty(&out).unwrap_or_default();
    atomic_write(&path, &bytes)
}

fn write_summary(cache: &Cache, custom_path: Option<&str>, tz_offset: i32, tz_label: &str) -> std::io::Result<()> {
    let Some(path) = summary_path(custom_path) else { return Ok(()) };
    fn to_vec(b: &BTreeMap<String, Bucket>) -> Vec<KeyedBucket<'_>> {
        b.iter().rev().map(|(k, v)| KeyedBucket { key: k, bucket: v }).collect()
    }
    let out = SummaryOutput {
        generated_at: now_iso(tz_offset),
        timezone: tz_label,
        totals: &cache.totals,
        daily: to_vec(&cache.daily),
        weekly: to_vec(&cache.weekly),
        monthly: to_vec(&cache.monthly),
    };
    let bytes = serde_json::to_vec_pretty(&out).unwrap_or_default();
    atomic_write(&path, &bytes)
}

/// Upper bound on per-session detail retained in the cache and usage-sessions
/// file. Long-term aggregates (totals/daily/weekly/monthly) are unbounded by
/// design; only the per-session breakdown is capped to keep the file and the
/// per-render (de)serialization cost from growing without limit.
const MAX_SESSIONS: usize = 1000;

/// Keeps the most recently active sessions, dropping the oldest by `last_seen`.
/// Ordered by parsed instant (consistent with `ts_before`); unparseable
/// timestamps sort first and are evicted first.
fn prune_sessions(cache: &mut Cache) {
    if cache.sessions.len() <= MAX_SESSIONS {
        return;
    }
    let mut by_recency: Vec<(Option<DateTime<FixedOffset>>, String)> = cache
        .sessions
        .iter()
        .map(|(id, s)| (DateTime::parse_from_rfc3339(&s.last_seen).ok(), id.clone()))
        .collect();
    by_recency.sort(); // ascending by parsed last_seen (None first), then id
    let excess = cache.sessions.len() - MAX_SESSIONS;
    for (_, id) in by_recency.into_iter().take(excess) {
        cache.sessions.remove(&id);
    }
}

pub fn update(custom_path: Option<&str>, tz_spec: Option<&str>) {
    let _ = try_update(custom_path, tz_spec);
}

fn try_update(custom_path: Option<&str>, tz_spec: Option<&str>) -> std::io::Result<()> {
    let (tz_offset, tz_label) = resolve_timezone(tz_spec);
    let path = cache_path(custom_path);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;

    match lock_file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(()),
        Err(TryLockError::Error(e)) => return Err(e),
    }

    let mut buf = String::new();
    lock_file.read_to_string(&mut buf).ok();
    let mut cache: Cache = if buf.is_empty() {
        Cache::default()
    } else {
        serde_json::from_str(&buf).unwrap_or_default()
    };

    let mut cache_dirty = false;
    let mut summary_dirty = false;

    let transcripts = list_transcripts();
    let mut alive: HashMap<String, FileState> = HashMap::new();

    for path in transcripts {
        let key = path.to_string_lossy().into_owned();
        let meta = fs::metadata(&path).ok();
        let mt = meta.as_ref().map(metadata_mtime_ms).unwrap_or(0);
        let sz = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let prev = cache.files.get(&key);
        let prev_offset = prev.map(|p| p.offset).unwrap_or(0);
        let prev_mtime = prev.map(|p| p.mtime_ms).unwrap_or(0);

        // Skip only when truly unchanged: same mtime AND already folded to EOF.
        // Comparing the size guards against same-millisecond appends that an
        // mtime-only check would miss.
        if prev.is_some() && prev_mtime == mt && prev_offset >= sz {
            alive.insert(key, FileState { offset: prev_offset, mtime_ms: mt });
            continue;
        }

        if let Some((new_offset, folded)) = process_new_lines(&path, prev_offset, sz, &mut cache, tz_offset) {
            cache_dirty = true;
            if folded > 0 {
                summary_dirty = true;
            }
            alive.insert(key, FileState { offset: new_offset, mtime_ms: mt });
        }
    }

    if alive.len() != cache.files.len() {
        cache_dirty = true;
    }
    cache.files = alive;

    // Prices entries that were unpriced when folded (e.g. a model released
    // after the binary was built) once an updated binary knows their rate.
    if reprice_cache(&mut cache) {
        cache_dirty = true;
        summary_dirty = true;
    }

    if summary_dirty {
        prune_sessions(&mut cache);
    }

    // Write outputs BEFORE persisting advanced offsets: if a summary/sessions
    // write fails, the cache offsets stay put so the unwritten lines are re-folded
    // next run rather than being lost from the summary forever.
    if summary_dirty {
        write_summary(&cache, custom_path, tz_offset, &tz_label)?;
        write_sessions(&cache, custom_path, tz_offset, &tz_label)?;
    }
    if cache_dirty {
        write_cache(&mut lock_file, &cache)?;
    }

    Ok(())
}
