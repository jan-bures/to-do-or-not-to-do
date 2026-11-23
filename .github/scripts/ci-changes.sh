#!/usr/bin/env bash
# Classify a NUL-delimited `git diff --no-renames --name-only -z` from stdin.
set -euo pipefail

areas=(rust runbooks shared android web desktop desktop-package linux-release email-worker deploy-scripts brand-assets)
declare -A changed=()

mark() {
  local area
  for area in "$@"; do changed[$area]=true; done
}

if [[ ${1:-} == --all ]]; then
  mark "${areas[@]}"
else
  while IFS= read -r -d '' path; do
    case "$path" in
      .github/* | .cargo/* | Cargo.toml | Cargo.lock | rust-toolchain* | \
      rustfmt.toml | .rustfmt.toml | clippy.toml | .clippy.toml | .gitattributes | build.rs)
        mark "${areas[@]}" ;;
    esac
    case "$path" in
      crates/* | assets/* | docs/ingest-email-signature.json) mark rust ;;
      docs/hosting-linux.md | deploy/windows/README.md | deploy/check-doc-powershell.ps1) mark runbooks ;;
    esac
    case "$path" in
      crates/todo-core/* | crates/todo-client/* | crates/todo-design/* | crates/todo-tools/*)
        mark shared android web desktop desktop-package linux-release ;;
      crates/todo-fs/*) mark android web desktop desktop-package linux-release ;;
      crates/todo-server/*) mark web desktop linux-release ;; # Desktop integration tests use the server.
      crates/todo-backup/*) mark linux-release ;;
      crates/todo-ffi/* | apps/android/*) mark android ;;
      apps/web/*) mark web linux-release ;;
      apps/desktop/*) mark desktop desktop-package ;;
      assets/*) mark android web desktop desktop-package linux-release ;;
      deploy/*) mark linux-release deploy-scripts ;;
      scripts/github-release.ps1 | scripts/test-github-release.ps1 | scripts/release_privacy.py)
        mark android desktop-package ;;
    esac
    case "$path" in
      workers/* | docs/ingest-email-signature.json) mark email-worker ;;
      assets/brand/*) mark brand-assets ;;
    esac
  done
fi

for area in "${areas[@]}"; do printf '%s=%s\n' "$area" "${changed[$area]:-false}"; done
