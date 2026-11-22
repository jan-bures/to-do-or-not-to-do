#!/usr/bin/env bash
# Builds todo-server and todo-backup for x86_64-unknown-linux-gnu and the production web app for
# wasm32-unknown-unknown. Writes a complete release with SHA256SUMS to OUT_DIR.
# deploy-linux.ps1 runs it inside WSL; it works on any Linux host.
#
#   bash deploy/linux/build-release.sh OUT_DIR
#
# It needs rustup, a C compiler and Node.js on the build host. It installs the pinned Rust toolchain
# and, if needed, the matching wasm-bindgen CLI. See docs/hosting-linux.md, "The build host".
# The builds run under nice. The build directory defaults to
# ~/.cache/to-do-or-not-to-do/target-linux, on the Linux file system: building under /mnt/c is slow and
# would mix with the Windows build. Set CARGO_TARGET_DIR to use another one.
#
# The binaries link against the build host's glibc, so build on a distribution whose glibc is no newer
# than the server's (Ubuntu 22.04 has 2.35, Debian 13 has 2.41). The script prints what they need.
set -euo pipefail

TARGET=x86_64-unknown-linux-gnu
BINARIES=(todo-server todo-backup)

[ $# -eq 1 ] || { echo "usage: $0 OUT_DIR" >&2; exit 2; }
out="$1"
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

channel="$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$repo/rust-toolchain.toml")"
[ -n "$channel" ] || { echo "no channel in $repo/rust-toolchain.toml" >&2; exit 1; }

if ! command -v rustup > /dev/null 2>&1 && [ -f "$HOME/.cargo/env" ]; then
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
fi
if ! command -v rustup > /dev/null 2>&1; then
    echo "rustup is not installed for $(id -un). Set it up once (docs/hosting-linux.md, \"The build host\")," >&2
    echo "then run this again." >&2
    exit 1
fi
if ! command -v cc > /dev/null 2>&1; then
    echo "no C compiler (SQLite, zstd and ring need one): sudo apt-get install build-essential" >&2
    exit 1
fi

if ! command -v node > /dev/null 2>&1; then
    echo 'Node.js is missing from PATH; install Node.js 24 LTS (docs/hosting-linux.md, "The build host").' >&2
    exit 1
fi

echo "==> toolchain $channel"
rustup toolchain install "$channel" --profile minimal --no-self-update > /dev/null
rustup target add --toolchain "$channel" "$TARGET" wasm32-unknown-unknown > /dev/null

cache="$HOME/.cache/to-do-or-not-to-do"
export CARGO_TARGET_DIR
CARGO_TARGET_DIR="$(realpath -m -- "${CARGO_TARGET_DIR:-$cache/target-linux}")"
echo "==> building in $CARGO_TARGET_DIR"
build_args=()
for b in "${BINARIES[@]}"; do build_args+=(-p "$b"); done
nice -n 10 cargo "+$channel" build --release --locked --target "$TARGET" \
    --manifest-path "$repo/Cargo.toml" "${build_args[@]}"

bindgen_version="$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/"/, "", $3); print $3; exit }' "$repo/apps/web/Cargo.lock")"
[ -n "$bindgen_version" ] || { echo 'no wasm-bindgen version in apps/web/Cargo.lock' >&2; exit 1; }
if [ "$(wasm-bindgen --version 2> /dev/null || true)" != "wasm-bindgen $bindgen_version" ]; then
    # Do not replace a CLI the operator installed for another workspace.
    bindgen_root="$cache/wasm-bindgen-cli/$bindgen_version"
    if [ "$("$bindgen_root/bin/wasm-bindgen" --version 2> /dev/null || true)" != "wasm-bindgen $bindgen_version" ]; then
        nice -n 10 cargo "+$channel" install wasm-bindgen-cli --version "$bindgen_version" --locked --root "$bindgen_root"
    fi
    export PATH="$bindgen_root/bin:$PATH"
fi
RUSTUP_TOOLCHAIN="$channel" nice -n 10 node "$repo/apps/web/scripts/build.mjs"

mkdir -p "$out"
for b in "${BINARIES[@]}"; do
    install -m 0755 "$CARGO_TARGET_DIR/$TARGET/release/$b" "$out/$b"
done
rm -rf -- "$out/web"
cp -R -- "$repo/apps/web/dist" "$out/web"
# The installer regenerates the same manifest from what it copied before running either binary.
(cd "$out" && sha256sum "${BINARIES[@]}" && find web -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum) \
    > "$out/SHA256SUMS"

if command -v objdump > /dev/null 2>&1; then
    glibc="$(for b in "${BINARIES[@]}"; do objdump -T "$out/$b"; done |
        grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n 1)"
    echo "==> needs ${glibc:-an unknown glibc} or newer on the server"
fi
echo "==> wrote ${BINARIES[*]}, web/ and SHA256SUMS to $out"
