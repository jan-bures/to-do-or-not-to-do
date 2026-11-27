#!/usr/bin/env bash
set -euo pipefail

root=$(git rev-parse --show-toplevel)
scratch_parent=$(cd "${TMPDIR:-/tmp}" && pwd -P)
scratch=$(mktemp -d "$scratch_parent/todo-pre-push.XXXXXX")
scratch=$(cd "$scratch" && pwd -P)
[[ $(dirname "$scratch") == "$scratch_parent" && $(basename "$scratch") == todo-pre-push.* ]]
trap 'rm -rf -- "$scratch"' EXIT

git init -q --bare "$scratch/published.git"
git init -q --bare "$scratch/empty.git"
git init -q -b main "$scratch/work"
cd "$scratch/work"
git config user.name "Test owner"
git config user.email you@example.com
git config core.autocrlf false
git -c user.name='Legacy author marker' commit -q --allow-empty -m 'Published baseline'
git remote add origin "$scratch/published.git"
git push -q origin main

mkdir -p .githooks private/config
cp "$root/.githooks/pre-push" "$root/.githooks/leak-check.sh" .githooks/
chmod +x .githooks/pre-push
git config core.hooksPath .githooks
printf '%s\n' 'Legacy author marker' 'blocked content marker' > private/config/denylist.txt

git switch -q -c clean
printf '%s\n' 'A safe change' > example.txt
git add example.txt
git commit -q -m 'New safe commit'
git push -q origin clean
git ls-remote --exit-code --heads origin refs/heads/clean >/dev/null

assert_blocked() {
  local output
  if output=$(git push "$@" 2>&1); then
    echo 'Expected the pre-push hook to reject the push.' >&2
    return 1
  fi
  grep -Fq 'contains denylisted text' <<<"$output"
  grep -Fq 'pre-push: push blocked.' <<<"$output"
}

# The same inherited identity still gets checked when publishing to a destination without it.
git remote set-url --push origin "$scratch/empty.git"
assert_blocked origin clean
git remote set-url --push origin "$scratch/published.git"

# Excluding the published history must still catch a new change containing a denylisted string.
git switch -q -c rejected
printf '%s\n' 'blocked content marker' > example.txt
git add example.txt
git commit -q -m 'Add example'
assert_blocked origin rejected

echo 'Pre-push published-history tests passed.'
