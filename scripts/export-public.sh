#!/usr/bin/env bash
# Produce a shareable copy of this repo with NO history: one commit of the
# current tree, minus anything git-ignored (ledger CSVs, invoices, clients.json,
# your invoice template). The private repo's history still contains personal
# data from before the scrub, so never publish it directly; publish this.
#
#   scripts/export-public.sh /path/to/stint-public
#   cd /path/to/stint-public && git remote add origin <new empty repo> && git push -u origin main
set -euo pipefail
dest="${1:?usage: export-public.sh <dest-dir>}"
src="$(git rev-parse --show-toplevel)"
[[ -e "$dest" ]] && { echo "refusing to overwrite $dest" >&2; exit 1; }
mkdir -p "$dest"
# git archive honours .gitignore via the index: only tracked files are exported.
git -C "$src" archive --format=tar HEAD | tar -x -C "$dest"
cd "$dest"
# Belt and braces: fail if anything that looks personal slipped into tracked files.
if grep -rIl -E '@(gmail|yahoo|outlook)\.|wsl\.localhost\Ubuntu\home\[a-z]+' . >/dev/null 2>&1; then
  echo "export contains what looks like personal data; aborting:" >&2
  grep -rIl -E '@(gmail|yahoo|outlook)\.|wsl\.localhost\Ubuntu\home\[a-z]+' . >&2
  exit 1
fi
git init -q -b main
git add -A
git -c user.name="${GIT_AUTHOR_NAME:-stint}" -c user.email="${GIT_AUTHOR_EMAIL:-stint@users.noreply.github.com}" \
  commit -q -m "stint: time tracker + invoices (CLI, desktop GUI, MCP server)"
echo "exported $(git rev-list --count HEAD) commit to $dest"
