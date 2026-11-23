#!/usr/bin/env bash
set -euo pipefail

classifier=$(dirname "$0")/ci-changes.sh
check() {
  local want=$1 actual
  shift
  actual=$(printf '%s\0' "$@" | bash "$classifier" | awk -F= '$2 == "true" { print $1 }' | paste -sd ' ' -)
  if [[ $actual != "$want" ]]; then
    printf 'Paths: %q\nExpected: %s\nActual: %s\n' "$*" "$want" "$actual" >&2
    return 1
  fi
}

all='rust runbooks shared android web desktop desktop-package linux-release email-worker deploy-scripts brand-assets'
check '' AGENTS.md docs/design-language.md 'docs/a space.md'
check android apps/android/app/build.gradle.kts
check 'rust android' crates/todo-ffi/src/lib.rs
check 'web linux-release' $'apps/web/line\nbreak'
check 'desktop desktop-package' apps/desktop/Cargo.lock
check 'rust web desktop linux-release' crates/todo-server/src/main.rs
check 'rust linux-release' crates/todo-backup/src/main.rs
check 'rust shared android web desktop desktop-package linux-release' crates/todo-client/src/lib.rs
check 'rust android web desktop desktop-package linux-release brand-assets' assets/brand/generate.mjs
check 'rust email-worker' docs/ingest-email-signature.json
check 'runbooks linux-release deploy-scripts' deploy/windows/README.md
check 'android email-worker' apps/android/README.md workers/email-ingest/package.json
check "$all" .github/workflows/ci.yml
check "$all" Cargo.lock
check "$all" rust-toolchain.toml
actual=$(bash "$classifier" --all | awk -F= '$2 == "true" { print $1 }' | paste -sd ' ' -)
[[ $actual == "$all" ]]
echo 'CI path selection tests passed.'
