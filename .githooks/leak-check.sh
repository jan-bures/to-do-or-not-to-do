#!/bin/sh
# Shared leak check for the pre-commit and pre-push hooks.
# Usage: leak-check.sh <file-with-text-to-scan> <label>
# Patterns come from private/config/denylist.txt (private companion repo, not published).
# If that file is missing (e.g. a fresh public clone), only gitleaks runs.

input="$1"
label="$2"
root="$(git rev-parse --show-toplevel)"
denylist="$root/private/config/denylist.txt"
# Linked worktrees (agent checkouts) have no private/ of their own; fall back to the main checkout's.
if [ ! -f "$denylist" ]; then
  common="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)"
  if [ -n "$common" ] && [ -f "$(dirname "$common")/private/config/denylist.txt" ]; then
    denylist="$(dirname "$common")/private/config/denylist.txt"
  fi
fi
status=0

if [ -f "$denylist" ]; then
  patterns="$(mktemp)"
  grep -v -E '^[[:space:]]*(#|$)' "$denylist" | tr -d '\r' > "$patterns"
  if [ -s "$patterns" ] && grep -n -i -E -f "$patterns" "$input" > /dev/null 2>&1; then
    echo "leak-check: $label contains denylisted text (see private/config/denylist.txt):" >&2
    grep -n -i -E -f "$patterns" "$input" | cut -c1-200 >&2
    status=1
  fi
  rm -f "$patterns"
else
  echo "leak-check: private/config/denylist.txt not found; skipping denylist (gitleaks only)." >&2
fi

exit $status
