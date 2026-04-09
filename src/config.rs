use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default = "default_lines")]
    pub lines: Vec<LineConfig>,
    #[serde(default)]
    pub colors: HashMap<String, String>,
}

#[derive(Serialize, Deserialize)]
pub struct LineConfig {
    pub fields: Vec<String>,
    pub separator: String,
}

impl Config {
    pub fn has_field(&self, name: &str) -> bool {
        self.lines.iter().any(|l| l.fields.iter().any(|f| f == name))
    }
}

fn default_lines() -> Vec<LineConfig> {
    vec![
        LineConfig {
            fields: vec!["dir", "branch", "diff", "model", "tokens"]
                .into_iter().map(String::from).collect(),
            separator: "|".to_string(),
        },
        LineConfig {
            fields: vec!["rate-5h", "rate-7d"]
                .into_iter().map(String::from).collect(),
            separator: "\u{2014}".to_string(),
        },
    ]
}

pub fn default_config() -> Config {
    Config {
        lines: default_lines(),
        colors: [
            ("dir", "teal"), ("branch", "blue"), ("added", "green"),
            ("removed", "red"), ("model", "mauve"), ("tokens", "peach"),
            ("tokens-percent", "peach"), ("cost", "green"),
            ("cpu", "subtext0"), ("ram", "subtext0"),
            ("rate-5h", "subtext0"), ("rate-7d", "subtext0"),
            ("separator", "text"),
        ].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

fn config_path() -> Option<String> {
    std::env::var("HOME").ok().map(|h| format!("{}/.claude/statusline.json", h))
}

pub fn load_config() -> Config {
    let path = match config_path() {
        Some(p) => p,
        None => return default_config(),
    };
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            eprintln!("statusline: invalid config {}: {}", path, e);
            default_config()
        }),
        Err(_) => default_config(),
    }
}

pub fn write_default_config() {
    let path = match config_path() {
        Some(p) => p,
        None => {
            eprintln!("statusline: $HOME not set");
            std::process::exit(1);
        }
    };
    if std::path::Path::new(&path).exists() {
        eprintln!("statusline: {} already exists", path);
        std::process::exit(1);
    }
    let dir = std::path::Path::new(&path).parent().unwrap();
    let _ = fs::create_dir_all(dir);
    let content = serde_json::to_string_pretty(&default_config()).unwrap();
    fs::write(&path, &content).unwrap_or_else(|e| {
        eprintln!("statusline: failed to write {}: {}", path, e);
        std::process::exit(1);
    });
    println!("Created {}", path);
}

pub fn print_fields() {
    println!("Fields:");
    println!("  dir              project directory (link-wrapped if remote URL exists)");
    println!("  branch           git branch name");
    println!("  diff             lines added/removed (uses 'added' and 'removed' colors)");
    println!("  model            Claude model name");
    println!("  tokens           token usage bar (50k/200k)");
    println!("  tokens-percent   token usage bar (15% · 200k)");
    println!("  cpu              Claude CPU usage");
    println!("  ram              Claude RAM usage");
    println!("  cost             session cost in USD");
    println!("  rate-5h          5-hour rate limit remaining + reset time");
    println!("  rate-7d          7-day rate limit remaining + reset time");
    println!();
    println!("Colors (catppuccin):");
    println!("  rosewater, flamingo, pink, mauve, red, maroon, peach, yellow,");
    println!("  green, teal, sky, sapphire, blue, lavender, text, subtext1,");
    println!("  subtext0, overlay2, overlay1, overlay0, surface2, surface1,");
    println!("  surface0, base, mantle, crust");
}
