use catppuccin::{Color, Flavor, PALETTE};
use std::fs;

pub const COLOR_RESET: &str = "\x1b[0m";

fn ansi(color: Color) -> String {
    format!("\x1b[38;2;{};{};{}m", color.rgb.r, color.rgb.g, color.rgb.b)
}

pub struct Colors {
    pub dir: String,
    pub branch: String,
    pub added: String,
    pub removed: String,
    pub model: String,
    pub tokens: String,
    pub cpu: String,
    pub ram: String,
    pub sep: String,
}

pub fn detect_theme() -> Colors {
    let is_light = std::env::var("HOME")
        .ok()
        .and_then(|home| fs::read_to_string(format!("{}/.claude.json", home)).ok())
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|json| json.get("theme").and_then(|v| v.as_str()).map(String::from))
        .map(|theme| theme.contains("light"))
        .unwrap_or(false);

    let flavor: &Flavor = if is_light { &PALETTE.latte } else { &PALETTE.frappe };

    Colors {
        dir: ansi(flavor.colors.teal),
        branch: ansi(flavor.colors.blue),
        added: ansi(flavor.colors.green),
        removed: ansi(flavor.colors.red),
        model: ansi(flavor.colors.mauve),
        tokens: ansi(flavor.colors.peach),
        cpu: ansi(flavor.colors.subtext0),
        ram: ansi(flavor.colors.subtext0),
        sep: ansi(flavor.colors.text),
    }
}
