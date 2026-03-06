use crate::colors::{Colors, COLOR_RESET};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Input {
    pub cwd: String,
    pub transcript_path: String,
    pub model: Model,
    context_window: Option<ContextWindow>,
}

#[derive(Deserialize)]
pub struct Model {
    pub display_name: String,
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

pub fn get_token_info(input: &Input, colors: &Colors) -> String {
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

    let filled = ((current * 8) / size) as usize;
    let bar: String = "█".repeat(filled) + &"░".repeat(8 - filled);

    let current_k = current / 1000;
    let size_k = size / 1000;

    format!(
        "{}{} {}k/{}k{}",
        colors.tokens, bar, current_k, size_k, COLOR_RESET
    )
}
