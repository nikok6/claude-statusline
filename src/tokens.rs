use serde::Deserialize;

#[derive(Deserialize)]
pub struct Input {
    pub cwd: String,
    pub transcript_path: String,
    pub model: Model,
    context_window: Option<ContextWindow>,
    cost: Option<Cost>,
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
struct CurrentUsage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

fn make_bar(filled: usize) -> String {
    "█".repeat(filled) + &"░".repeat(8 - filled)
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

    let filled = ((current * 8) / size) as usize;
    Some(format!("{} {}k/{}k", make_bar(filled), current / 1000, size / 1000))
}

pub fn get_token_percent_info(input: &Input) -> Option<String> {
    let ctx = input.context_window.as_ref()?;
    let p = ctx.used_percentage?;
    let size_k = ctx.context_window_size.unwrap_or(0) / 1000;
    let filled = (p as usize * 8) / 100;
    Some(format!("{} {}% · {}k", make_bar(filled), p, size_k))
}

pub fn get_cost(input: &Input) -> Option<f64> {
    input.cost.as_ref()?.total_cost_usd
}
