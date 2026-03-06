mod colors;
mod diff;
mod git;
mod tokens;

use colors::{COLOR_RESET, detect_theme};
use std::io;

fn get_dir_name(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn main() {
    // Handle --version flag
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let input: tokens::Input = match serde_json::from_reader(io::stdin()) {
        Ok(i) => i,
        Err(_) => std::process::exit(1),
    };

    let colors = detect_theme();
    let dir_name = get_dir_name(&input.cwd);
    let git_branch = git::get_git_branch(&input.cwd);
    let model_name = &input.model.display_name;
    let (added, removed) = diff::calculate_net_diff(&input.transcript_path);
    let token_info = tokens::get_token_info(&input, &colors);

    println!(
        "{}{}{} {}|{} {}{}{} {}|{} {}+{}{} {}-{}{} {}|{} {}{}{} {}|{} {}",
        colors.dir, dir_name, COLOR_RESET,
        colors.sep, COLOR_RESET,
        colors.branch, git_branch, COLOR_RESET,
        colors.sep, COLOR_RESET,
        colors.added, added, COLOR_RESET,
        colors.removed, removed, COLOR_RESET,
        colors.sep, COLOR_RESET,
        colors.model, model_name, COLOR_RESET,
        colors.sep, COLOR_RESET,
        token_info
    );
}
