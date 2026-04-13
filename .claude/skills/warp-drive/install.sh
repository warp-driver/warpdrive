#!/usr/bin/env bash
set -euo pipefail
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
git clone --depth=1 --filter=blob:none --no-checkout \
  https://github.com/warp-driver/warpdrive.git "$TMP/warpdrive" 2>/dev/null
cd "$TMP/warpdrive"
git sparse-checkout init --cone
git sparse-checkout set .claude/skills/warp-drive
git checkout
mkdir -p ~/.claude/skills
cp -r .claude/skills/warp-drive ~/.claude/skills/warp-drive
echo "WarpDrive skill installed to ~/.claude/skills/warp-drive"
echo "Restart Claude Code to pick up the skill."
echo ""
echo "Next: register warpdrive-mcp with Claude Code."
echo "Run from any project directory:"
echo "  npx @warpdrive/mcp@latest"
echo ""
echo "This interactive wizard installs warpdrive-mcp, writes ~/.claude.json,"
echo "and writes ~/.wavs/wavs.toml so chain-write tools work from any project."
echo ""
echo "WarpDrive repo users: 'just setup-claude-mcp [/path/to/project]' does the same."
