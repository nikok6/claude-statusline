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

struct RenderContext<'a> {
    colors: &'a colors::Colors,
    dir_name: &'a str,
    remote_url: &'a Option<String>,
    git_branch: &'a str,
    model_name: &'a str,
    added: usize,
    removed: usize,
    token_info: &'a Option<String>,
    token_percent_info: &'a Option<String>,
    stats: &'a Option<process::ClaudeStats>,
    cost: Option<f64>,
    rate_5h: &'a Option<tokens::RateLimitInfo>,
    rate_7d: &'a Option<tokens::RateLimitInfo>,
}

fn render_field(field: &str, ctx: &RenderContext) -> Option<String> {
    let c = ctx.colors;
    match field {
        "dir" => Some(format!("{}{}{}", c.dir, link_wrap(ctx.dir_name, ctx.remote_url), COLOR_RESET)),
        "branch" => Some(format!("{}{}{}", c.branch, ctx.git_branch, COLOR_RESET)),
        "diff" => Some(format!("{}+{}{} {}-{}{}", c.added, ctx.added, COLOR_RESET, c.removed, ctx.removed, COLOR_RESET)),
        "model" => Some(format!("{}{}{}", c.model, ctx.model_name, COLOR_RESET)),
        "tokens" => ctx.token_info.as_ref().map(|t| format!("{}{}{}", c.tokens, t, COLOR_RESET)),
        "tokens-percent" => ctx.token_percent_info.as_ref().map(|t| format!("{}{}{}", c.tokens_percent, t, COLOR_RESET)),
        "cpu" => ctx.stats.as_ref().map(|s| format!("{}CPU {}{}", c.cpu, s.cpu, COLOR_RESET)),
        "ram" => ctx.stats.as_ref().map(|s| format!("{}RAM {}{}", c.ram, s.ram, COLOR_RESET)),
        "cost" => ctx.cost.map(|v| format!("{}${:.2}{}", c.cost, v, COLOR_RESET)),
        "rate-5h" => ctx.rate_5h.as_ref().map(|r| format!("{}5h: {:.0}% → {}{}", c.rate_5h, 100.0 - r.used_pct, r.resets_in, COLOR_RESET)),
        "rate-7d" => ctx.rate_7d.as_ref().map(|r| format!("{}7d: {:.0}% → {}{}", c.rate_7d, 100.0 - r.used_pct, r.resets_in, COLOR_RESET)),
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

    let needs_git = cfg.has_field("dir") || cfg.has_field("branch");
    let needs_diff = cfg.has_field("diff");
    let needs_stats = cfg.has_field("cpu") || cfg.has_field("ram");

    let dir_name = get_dir_name(&input.cwd);
    let (git_branch, remote_url) = if needs_git { git::get_git_info(&input.cwd) } else { (String::new(), None) };
    let model_name = input.model.display_name.split('(').next().unwrap_or(&input.model.display_name).trim().to_string();
    let (added, removed) = if needs_diff { diff::calculate_net_diff(&input.transcript_path) } else { (0, 0) };
    let token_info = if cfg.has_field("tokens") { tokens::get_token_info(&input) } else { None };
    let token_percent_info = if cfg.has_field("tokens-percent") { tokens::get_token_percent_info(&input) } else { None };
    let stats = if needs_stats { process::get_claude_stats(&input.transcript_path) } else { None };
    let cost = if cfg.has_field("cost") { tokens::get_cost(&input) } else { None };
    let rate_5h = if cfg.has_field("rate-5h") { tokens::get_rate_limit_5h(&input) } else { None };
    let rate_7d = if cfg.has_field("rate-7d") { tokens::get_rate_limit_7d(&input) } else { None };

    let ctx = RenderContext {
        colors: &colors,
        dir_name: &dir_name,
        remote_url: &remote_url,
        git_branch: &git_branch,
        model_name: &model_name,
        added,
        removed,
        token_info: &token_info,
        token_percent_info: &token_percent_info,
        stats: &stats,
        cost,
        rate_5h: &rate_5h,
        rate_7d: &rate_7d,
    };

    for line_cfg in &cfg.lines {
        let segments: Vec<String> = line_cfg.fields.iter().filter_map(|field| {
            render_field(field, &ctx)
        }).collect();

        if !segments.is_empty() {
            let sep = format!(" {}{}{} ", colors.sep, line_cfg.separator, COLOR_RESET);
            println!("{}", segments.join(&sep));
        }
    }
}
