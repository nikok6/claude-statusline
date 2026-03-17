mod cache;
mod colors;
mod config;
mod diff;
mod git;
mod process;
mod tokens;

use colors::{COLOR_RESET, detect_flavor, resolve_colors};
use std::io;

fn get_dir_name(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn link_wrap(text: &str, url: &Option<String>) -> String {
    match url {
        Some(url) => format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", url, text),
        None => text.to_string(),
    }
}

fn render_field(
    field: &str,
    colors: &colors::Colors,
    dir_name: &str,
    remote_url: &Option<String>,
    git_branch: &str,
    model_name: &str,
    added: usize,
    removed: usize,
    token_info: &Option<String>,
    stats: &Option<process::ClaudeStats>,
) -> Option<String> {
    match field {
        "dir" => Some(format!("{}{}{}", colors.dir, link_wrap(dir_name, remote_url), COLOR_RESET)),
        "branch" => Some(format!("{}{}{}", colors.branch, git_branch, COLOR_RESET)),
        "diff" => Some(format!("{}+{}{} {}-{}{}", colors.added, added, COLOR_RESET, colors.removed, removed, COLOR_RESET)),
        "model" => Some(format!("{}{}{}", colors.model, model_name, COLOR_RESET)),
        "tokens" => token_info.as_ref().map(|t| format!("{}{}{}", colors.tokens, t, COLOR_RESET)),
        "cpu" => stats.as_ref().map(|s| format!("{}CPU {}{}", colors.cpu, s.cpu, COLOR_RESET)),
        "ram" => stats.as_ref().map(|s| format!("{}RAM {}{}", colors.ram, s.ram, COLOR_RESET)),
        _ => None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--fields") {
        config::print_fields();
        return;
    }
    if args.iter().any(|a| a == "--init") {
        config::write_default_config();
        return;
    }

    let input: tokens::Input = match serde_json::from_reader(io::stdin()) {
        Ok(i) => i,
        Err(_) => std::process::exit(1),
    };

    let cfg = config::load_config();
    let flavor = detect_flavor();
    let colors = resolve_colors(flavor, &cfg.colors);

    let dir_name = get_dir_name(&input.cwd);
    let (git_branch, remote_url) = git::get_git_info(&input.cwd);
    let model_name = input.model.display_name.split('(').next().unwrap_or(&input.model.display_name).trim().to_string();
    let (added, removed) = diff::calculate_net_diff(&input.transcript_path);
    let token_info = tokens::get_token_info(&input);
    let stats = process::get_claude_stats(&input.transcript_path);

    for line_cfg in &cfg.lines {
        let segments: Vec<String> = line_cfg.fields.iter().filter_map(|field| {
            render_field(field, &colors, &dir_name, &remote_url, &git_branch,
                         &model_name, added, removed, &token_info, &stats)
        }).collect();

        if !segments.is_empty() {
            let sep = format!(" {}{}{} ", colors.sep, line_cfg.separator, COLOR_RESET);
            println!("{}", segments.join(&sep));
        }
    }
}
