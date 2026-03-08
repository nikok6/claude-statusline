use std::fs;

pub fn get_git_info(cwd: &str) -> (String, Option<String>) {
    match find_git_dir(cwd) {
        Some(git_dir) => (
            read_git_branch(&git_dir).unwrap_or_else(|| "no-git".to_string()),
            read_remote_url(&git_dir),
        ),
        None => ("no-git".to_string(), None),
    }
}

fn read_remote_url(git_dir: &std::path::Path) -> Option<String> {
    let config = std::fs::read_to_string(git_dir.join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed == "[remote \"origin\"]" {
            in_origin = true;
        } else if trimmed.starts_with('[') {
            in_origin = false;
        } else if in_origin {
            if let Some(url) = trimmed.strip_prefix("url = ") {
                return Some(normalize_remote_url(url));
            }
        }
    }
    None
}

fn normalize_remote_url(url: &str) -> String {
    // git@github.com:user/repo.git -> https://github.com/user/repo
    if let Some(rest) = url.strip_prefix("git@") {
        let rest = rest.replace(':', "/");
        let rest = rest.strip_suffix(".git").unwrap_or(&rest);
        return format!("https://{}", rest);
    }
    // https://github.com/user/repo.git -> https://github.com/user/repo
    url.strip_suffix(".git").unwrap_or(url).to_string()
}

fn find_git_dir(cwd: &str) -> Option<std::path::PathBuf> {
    let mut dir = std::path::Path::new(cwd);
    let git_path = loop {
        let candidate = dir.join(".git");
        if candidate.exists() {
            break candidate;
        }
        dir = dir.parent()?;
    };

    let git_dir = match fs::read_to_string(&git_path) {
        Ok(content) if content.starts_with("gitdir: ") => {
            let gitdir = content.strip_prefix("gitdir: ").unwrap().trim();
            git_path.parent().unwrap().join(gitdir)
        }
        _ => git_path,
    };

    Some(git_dir)
}

fn read_git_branch(git_dir: &std::path::Path) -> Option<String> {
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
