use catppuccin::{Color, Flavor, PALETTE};
use std::collections::HashMap;
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
    pub tokens_percent: String,
    pub cost: String,
    pub sep: String,
}

pub fn detect_flavor() -> &'static Flavor {
    let is_light = std::env::var("HOME")
        .ok()
        .and_then(|home| fs::read_to_string(format!("{}/.claude.json", home)).ok())
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|json| json.get("theme").and_then(|v| v.as_str()).map(String::from))
        .map(|theme| theme.contains("light"))
        .unwrap_or(false);

    if is_light { &PALETTE.latte } else { &PALETTE.frappe }
}

fn parse_hex(hex: &str) -> Option<String> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(format!("\x1b[38;2;{};{};{}m", r, g, b))
}

fn resolve_color(flavor: &Flavor, name: &str) -> Option<String> {
    if name.starts_with('#') {
        return parse_hex(name);
    }
    let color = match name {
        "rosewater" => flavor.colors.rosewater,
        "flamingo" => flavor.colors.flamingo,
        "pink" => flavor.colors.pink,
        "mauve" => flavor.colors.mauve,
        "red" => flavor.colors.red,
        "maroon" => flavor.colors.maroon,
        "peach" => flavor.colors.peach,
        "yellow" => flavor.colors.yellow,
        "green" => flavor.colors.green,
        "teal" => flavor.colors.teal,
        "sky" => flavor.colors.sky,
        "sapphire" => flavor.colors.sapphire,
        "blue" => flavor.colors.blue,
        "lavender" => flavor.colors.lavender,
        "text" => flavor.colors.text,
        "subtext1" => flavor.colors.subtext1,
        "subtext0" => flavor.colors.subtext0,
        "overlay2" => flavor.colors.overlay2,
        "overlay1" => flavor.colors.overlay1,
        "overlay0" => flavor.colors.overlay0,
        "surface2" => flavor.colors.surface2,
        "surface1" => flavor.colors.surface1,
        "surface0" => flavor.colors.surface0,
        "base" => flavor.colors.base,
        "mantle" => flavor.colors.mantle,
        "crust" => flavor.colors.crust,
        _ => return None,
    };
    Some(ansi(color))
}

fn lookup(flavor: &Flavor, color_map: &HashMap<String, String>, key: &str, default: Color) -> String {
    color_map.get(key)
        .and_then(|name| resolve_color(flavor, name))
        .unwrap_or_else(|| ansi(default))
}

pub fn resolve_colors(flavor: &Flavor, color_map: &HashMap<String, String>) -> Colors {
    Colors {
        dir: lookup(flavor, color_map, "dir", flavor.colors.teal),
        branch: lookup(flavor, color_map, "branch", flavor.colors.blue),
        added: lookup(flavor, color_map, "added", flavor.colors.green),
        removed: lookup(flavor, color_map, "removed", flavor.colors.red),
        model: lookup(flavor, color_map, "model", flavor.colors.mauve),
        tokens: lookup(flavor, color_map, "tokens", flavor.colors.peach),
        cpu: lookup(flavor, color_map, "cpu", flavor.colors.subtext0),
        ram: lookup(flavor, color_map, "ram", flavor.colors.subtext0),
        tokens_percent: lookup(flavor, color_map, "tokens-percent", flavor.colors.peach),
        cost: lookup(flavor, color_map, "cost", flavor.colors.green),
        sep: lookup(flavor, color_map, "separator", flavor.colors.text),
    }
}
