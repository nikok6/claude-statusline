use std::fs;

pub fn get_git_branch(cwd: &str) -> String {
    read_git_branch(cwd).unwrap_or_else(|| "no-git".to_string())
}

fn read_git_branch(cwd: &str) -> Option<String> {
    let git_path = std::path::Path::new(cwd).join(".git");

    // .git can be a file (worktrees/submodules) with "gitdir: <path>"
    let git_dir = match fs::read_to_string(&git_path) {
        Ok(content) if content.starts_with("gitdir: ") => {
            let gitdir = content.strip_prefix("gitdir: ").unwrap().trim();
            std::path::Path::new(cwd).join(gitdir)
        }
        _ => git_path,
    };

    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        return Some("rebasing".to_string());
    }

    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => {
            let branch = branch.trim().to_string();
            if branch.is_empty() { None } else { Some(branch) }
        }
        None => Some("detached".to_string()),
    }
}
