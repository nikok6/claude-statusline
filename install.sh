#!/bin/bash

set -e

echo "Installing Claude Code statusline..."

# Create .claude directory
mkdir -p ~/.claude

# Detect platform and copy correct statusline executable
OS="$(uname -s)"
ARCH="$(uname -m)"

if [ "$OS" = "Darwin" ] && [ "$ARCH" = "arm64" ]; then
  BINARY="statusline-darwin-arm64"
elif [ "$OS" = "Linux" ] && [ "$ARCH" = "x86_64" ]; then
  BINARY="statusline-linux-x64"
elif [ "$OS" = "Linux" ] && [ "$ARCH" = "aarch64" ]; then
  BINARY="statusline-linux-arm64"
else
  echo "Error: Unsupported platform: $OS $ARCH"
  exit 1
fi

VERSION=$(curl -fsSL "https://api.github.com/repos/nikok6/claude-statusline/releases/latest" | grep '"tag_name"' | cut -d'"' -f4)
curl -fsSL "https://github.com/nikok6/claude-statusline/releases/latest/download/$BINARY" -o ~/.claude/statusline
chmod +x ~/.claude/statusline

# Sign binary to avoid Gatekeeper issues on macOS
if [ "$OS" = "Darwin" ]; then
  codesign --sign - --force ~/.claude/statusline 2>/dev/null || true
fi

STATUSLINE_CMD="~/.claude/statusline"

# Download update script
curl -fsSL "https://raw.githubusercontent.com/nikok6/claude-statusline/main/update.sh" -o ~/.claude/statusline-update.sh
chmod +x ~/.claude/statusline-update.sh
UPDATE_CMD="~/.claude/statusline-update.sh"

# Update settings.json with statusLine and hooks config
if [ -f ~/.claude/settings.json ] && [ -s ~/.claude/settings.json ]; then
  # Merge into existing settings (file exists and is non-empty)
  if command -v jq &> /dev/null; then
    jq --arg cmd "$STATUSLINE_CMD" --arg update "$UPDATE_CMD" '
      .statusLine = {"type": "command", "command": $cmd} |
      .hooks.SessionStart = ((.hooks.SessionStart // []) + [{"hooks": [{"type": "command", "command": $update}]}] | unique_by(.hooks[0].command))
    ' ~/.claude/settings.json > /tmp/claude-settings.json && mv /tmp/claude-settings.json ~/.claude/settings.json
  else
    echo "Warning: jq not found, cannot merge settings. Please add config manually."
  fi
else
  # Create new settings.json
  cat > ~/.claude/settings.json << EOF
{
  "statusLine": {
    "type": "command",
    "command": "$STATUSLINE_CMD"
  },
  "hooks": {
    "SessionStart": [{
      "hooks": [{
        "type": "command",
        "command": "$UPDATE_CMD"
      }]
    }]
  }
}
EOF
fi

echo "Done! Installed $BINARY ($VERSION)"
