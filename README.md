# claude-statusline

A custom statusline for [Claude Code](https://claude.ai/code) showing git branch, session diff, model, and token usage.

```
main | +249 -9 | Opus 4.5 | ████░░░░ 79k/200k tokens
```

## Installation

```bash
curl -fsSL https://raw.githubusercontent.com/nikok6/claude-statusline/main/install.sh | bash
```

Or download the binary manually from [Releases](https://github.com/nikok6/claude-statusline/releases).

## Manual setup

1. Download the binary for your platform and save to `~/.claude/statusline`
2. Make it executable: `chmod +x ~/.claude/statusline`
3. Add to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.claude/statusline"
  }
}
```

## Building from source

Requires [Rust](https://rustup.rs/).

```bash
cargo build --release
cp target/release/statusline ~/.claude/statusline
```

## Testing

```bash
cargo test
```

## Features

- Git branch name
- Session net diff (lines added/removed since session start)
- Model name
- Token usage with progress bar
- Theme-aware colors (auto-detects light/dark mode from Claude settings)
- Plan mode file exclusion (changes in `~/.claude/plans` are ignored)
- Automatic updates (checks for new releases on session start)
- Performance caching (avoids re-parsing entire transcript)

Uses Catppuccin color theme (Latte for light mode, Mocha for dark mode).
