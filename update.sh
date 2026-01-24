#!/bin/bash
(
  INSTALLED=$(~/.claude/statusline --version 2>/dev/null || echo "0.0.0")
  LATEST=$(curl -fsSL "https://api.github.com/repos/nikok6/claude-statusline/releases/latest" | grep '"tag_name"' | cut -d'"' -f4 | tr -d 'v')
  [ -z "$LATEST" ] && exit 0
  [ "$INSTALLED" != "$LATEST" ] && curl -fsSL https://raw.githubusercontent.com/nikok6/claude-statusline/main/install.sh | bash
) &>/dev/null &
