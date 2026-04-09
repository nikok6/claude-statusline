# claude-statusline

A custom statusline for [Claude Code](https://claude.ai/code) showing git info, session diff, model, token usage, and rate limits.

```
my-project | main | +249 -9 | Opus 4.6 | ██▃▁▁▁▁▁ 79k/1000k
5h: 93% → 2h41m | 7d: 57% → 4d12h
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

- **Directory name** — hyperlinked to remote URL if available
- **Git branch** — detached HEAD and rebasing states
- **Session diff** — net lines added/removed (excludes plan mode files)
- **Model name**
- **Token usage** — progress bar with current/total
- **Rate limits** — 5-hour and 7-day usage with reset countdown
- **CPU/RAM usage** — Claude process stats (configurable)
- **Theme-aware colors** — auto-detects light/dark mode
- **Performance caching** — avoids re-parsing transcript and process tree

Uses [Catppuccin](https://catppuccin.com/) color theme (Latte for light mode, Frappé for dark mode).

## Auto-update

If installed via `install.sh`, a Claude Code session start hook is added that checks for new releases on each session start and updates the binary automatically.

## Configuration

Optionally create `~/.claude/statusline.json` to customize fields, layout, and colors:

```bash
~/.claude/statusline --init    # generate default config
~/.claude/statusline --fields  # list available fields and colors
```

### Example config

```json
{
  "lines": [
    { "fields": ["dir", "branch", "diff", "model", "tokens"], "separator": "|" },
    { "fields": ["rate-5h", "rate-7d"], "separator": "|" }
  ],
  "colors": {
    "dir": "teal",
    "branch": "blue",
    "added": "green",
    "removed": "red",
    "model": "mauve",
    "tokens": "peach",
    "cpu": "subtext0",
    "ram": "subtext0",
    "rate-5h": "subtext0",
    "rate-7d": "subtext0",
    "separator": "text"
  }
}
```

All fields are optional — missing fields use defaults. Colors can be [Catppuccin](https://catppuccin.com/) names or hex values (`#ff6b6b`).

### Available fields

| Field | Description |
|-------|-------------|
| `dir` | Project directory name (hyperlinked if remote URL exists) |
| `branch` | Git branch name |
| `diff` | Lines added/removed (`+N -N`) |
| `model` | Claude model name |
| `tokens` | Token usage progress bar |
| `tokens-percent` | Token usage bar with percentage |
| `cpu` | Claude CPU usage |
| `ram` | Claude RAM usage |
| `cost` | Session cost in USD |
| `rate-5h` | 5-hour rate limit remaining + reset countdown |
| `rate-7d` | 7-day rate limit remaining + reset countdown |

### Available colors

Any [Catppuccin](https://catppuccin.com/palette) color name or hex value (`#rrggbb`). Run `~/.claude/statusline --fields` to see the full list.
