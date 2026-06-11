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
- **Usage tracking** — optional, writes daily/weekly/monthly token + cost summaries to JSON

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

## Usage tracking (optional)

Opt-in: as a side effect of rendering, the statusline folds your transcripts' `usage` blocks into a JSON summary of tokens and **API-equivalent cost** — what the usage would cost at [API rates](https://platform.claude.com/docs/en/about-claude/pricing), handy for gauging the value of a flat-rate Pro/Max plan.

Enable in `~/.claude/statusline.json`:

```json
{
  "track_usage": {
    "enabled": true,
    "timezone": "local"
  }
}
```

| Field | Default | Description |
|---|---|---|
| `enabled` | `false` | Turn tracking on/off. |
| `output_path` | `~/.claude/usage` | Directory (`~/...` or absolute, created on demand) holding the summary, sessions, and cache files — point it at a bind mount to share across a devcontainer/host. |
| `timezone` | `local` | `local` (via `date +%z`), `UTC`, or an offset like `+07:00` / `-0800`. Affects how new data is bucketed; delete the cache to re-bucket history. |

It walks `~/.claude/projects/**/*.jsonl` incrementally and writes two files into the output directory:

- **`usage-summary.json`** — totals plus daily / weekly (ISO) / monthly buckets, each with a per-model breakdown.
- **`usage-sessions.json`** — per-session detail (most recent 1000).

```json
{
  "generated_at": "2026-05-19T18:08:11+08:00",
  "timezone": "UTC+08:00",
  "totals": {
    "input_tokens": 138957,
    "output_tokens": 8232357,
    "cache_creation_tokens": 87475426,
    "cache_read_tokens": 2236010119,
    "cost_usd": 1976.72,
    "by_model": {
      "claude-opus-4-7":   { "input_tokens": 95522, "output_tokens": 6699853, "cost_usd": 1633.18, ... },
      "claude-sonnet-4-6": { "input_tokens": 43435, "output_tokens": 1532504, "cost_usd": 337.39, ... }
    }
  },
  "daily":   [ { "key": "2026-05-19", "cost_usd": 29.53, ... }, ... ],
  "weekly":  [ { "key": "2026-W21",   "cost_usd": 49.49, ... }, ... ],
  "monthly": [ { "key": "2026-05",    "cost_usd": 371.05, ... }, ... ]
}
```

Aggregates survive transcript rotation. To reset, delete the output directory:

```bash
rm -r ~/.claude/usage
```
