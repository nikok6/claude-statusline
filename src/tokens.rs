use serde::Deserialize;

#[derive(Deserialize)]
pub struct Input {
    pub cwd: String,
    pub transcript_path: String,
    pub model: Model,
    context_window: Option<ContextWindow>,
    cost: Option<Cost>,
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
pub struct Model {
    pub display_name: String,
}

#[derive(Deserialize)]
struct ContextWindow {
    current_usage: Option<CurrentUsage>,
    context_window_size: Option<u64>,
    used_percentage: Option<u64>,
}

#[derive(Deserialize)]
struct Cost {
    total_cost_usd: Option<f64>,
}

#[derive(Deserialize)]
struct RateLimits {
    five_hour: Option<RateLimit>,
    seven_day: Option<RateLimit>,
}

#[derive(Deserialize)]
struct RateLimit {
    used_percentage: Option<f64>,
    resets_at: Option<u64>,
}

#[derive(Deserialize)]
struct CurrentUsage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

const BLOCKS: [char; 7] = ['▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn make_bar(total: u64, used: u64, width: usize) -> String {
    let ratio = (used as f64 / total as f64).clamp(0.0, 1.0);
    let levels = BLOCKS.len() as f64;
    let filled_exact = ratio * width as f64 * levels;
    let full = ((filled_exact / levels) as usize).min(width);
    let partial = (filled_exact % levels) as usize;

    let mut bar = String::new();
    for i in 0..width {
        if i < full {
            bar.push('█');
        } else if i == full && partial > 0 {
            bar.push(BLOCKS[partial - 1]);
        } else {
            bar.push('▁');
        }
    }
    bar
}

pub fn get_token_info(input: &Input) -> Option<String> {
    let ctx = input.context_window.as_ref()?;
    let size = ctx.context_window_size.filter(|&s| s > 0)?;

    let usage = ctx.current_usage.as_ref();
    let current = usage
        .map(|u| {
            u.input_tokens.unwrap_or(0)
                + u.cache_creation_input_tokens.unwrap_or(0)
                + u.cache_read_input_tokens.unwrap_or(0)
        })
        .unwrap_or(0);

    Some(format!("{} {}k/{}k", make_bar(size, current, 8), current / 1000, size / 1000))
}

pub fn get_token_percent_info(input: &Input) -> Option<String> {
    let ctx = input.context_window.as_ref()?;
    let p = ctx.used_percentage?;
    let size = ctx.context_window_size.filter(|&s| s > 0)?;
    let used = size * p / 100;
    Some(format!("{} {}% · {}k", make_bar(size, used, 8), p, size / 1000))
}

pub fn get_cost(input: &Input) -> Option<f64> {
    input.cost.as_ref()?.total_cost_usd
}

fn format_reset_time(resets_at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if resets_at <= now {
        return "now".to_string();
    }
    let secs = resets_at - now;
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 24 {
        format!("{}d{}h", hours / 24, hours % 24)
    } else if hours > 0 {
        format!("{}h{}m", hours, mins)
    } else if mins > 0 {
        format!("{}m", mins)
    } else {
        "<1m".to_string()
    }
}

pub struct RateLimitInfo {
    pub used_pct: f64,
    pub resets_in: String,
}

fn parse_rate_limit(rl: Option<&RateLimit>) -> Option<RateLimitInfo> {
    let rl = rl?;
    let pct = rl.used_percentage?;
    let resets_in = rl.resets_at.map(format_reset_time).unwrap_or_default();
    Some(RateLimitInfo { used_pct: pct, resets_in })
}

pub fn get_rate_limit_5h(input: &Input) -> Option<RateLimitInfo> {
    parse_rate_limit(input.rate_limits.as_ref()?.five_hour.as_ref())
}

pub fn get_rate_limit_7d(input: &Input) -> Option<RateLimitInfo> {
    parse_rate_limit(input.rate_limits.as_ref()?.seven_day.as_ref())
}
