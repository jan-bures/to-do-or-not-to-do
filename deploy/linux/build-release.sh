#!/usr/bin/env bash
# Builds release binaries of todo-server and todo-backup for x86_64-unknown-linux-gnu, and writes them
# with a SHA256SUMS file to OUT_DIR. deploy-linux.ps1 runs it inside WSL; it works on any Linux host.
#
#   bash deploy/linux/build-release.sh OUT_DIR
#
# It uses the toolchain pinned in rust-toolchain.toml, installing rustup (into ~/.cargo, for this user
# only) and that toolchain if they are missing. The build directory defaults to
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
    echo "==> installing rustup for $(id -un) (into ~/.cargo)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --no-modify-path --profile minimal --default-toolchain none
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
fi
if ! command -v cc > /dev/null 2>&1; then
    echo "no C compiler (SQLite, zstd and ring need one): sudo apt-get install build-essential" >&2
    exit 1
fi

echo "==> toolchain $channel"
rustup toolchain install "$channel" --profile minimal --no-self-update > /dev/null
rustup target add --toolchain "$channel" "$TARGET" > /dev/null

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/to-do-or-not-to-do/target-linux}"
echo "==> building in $CARGO_TARGET_DIR"
build_args=()
for b in "${BINARIES[@]}"; do build_args+=(-p "$b"); done
cargo "+$channel" build --release --locked --target "$TARGET" \
    --manifest-path "$repo/Cargo.toml" "${build_args[@]}"

mkdir -p "$out"
for b in "${BINARIES[@]}"; do
    install -m 0755 "$CARGO_TARGET_DIR/$TARGET/release/$b" "$out/$b"
done
(cd "$out" && sha256sum "${BINARIES[@]}" > SHA256SUMS)

if command -v objdump > /dev/null 2>&1; then
    glibc="$(for b in "${BINARIES[@]}"; do objdump -T "$out/$b"; done |
        grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n 1)"
    echo "==> needs ${glibc:-an unknown glibc} or newer on the server"
fi
echo "==> wrote ${BINARIES[*]} and SHA256SUMS to $out"
