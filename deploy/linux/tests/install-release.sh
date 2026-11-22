#!/usr/bin/env bash
# Exercises the real install and recovery phases without systemd or access to a VM. Run as root
# in a Linux container (the installer checks real file owners and modes during a restore).
set -euo pipefail

[[ $(id -u) = 0 ]] || { echo 'run this test as root in a Linux container' >&2; exit 1; }
installer="$(dirname "${BASH_SOURCE[0]}")/../install-release.sh"
root="$(mktemp -d /tmp/todo-install-test.XXXXXX)"
trap 'rm -rf -- "$root"' EXIT
mkdir -p "$root"/{etc/todo,etc/systemd/system,opt/todo,run/systemd/system,usr/local/lib/todo,usr/local/bin,var/lib/todo,stubs}
export TEST_ROOT="$root"
export PATH="$root/stubs:$PATH"
# Relocate only the installer's host paths; the real filesystem and locking tools run unchanged.
sed -E "s@(/opt/todo|/run/(todo-deploy|systemd/system)|/usr/local/(bin|lib/todo)|/etc/(todo|systemd/system)|/var/lib/todo)@$root\1@g" \
    "$installer" > "$root/install-release.sh"
touch "$root/etc/systemd/system/todo-server.service" "$root/var/lib/todo/backup-run.lock"
install -m 0755 /bin/true "$root/usr/local/lib/todo/deploy-guard"

cat > "$root/stubs/id" <<'EOF'
#!/usr/bin/env bash
[[ $* = todo ]] || exec /usr/bin/id "$@"
EOF
cat > "$root/stubs/runuser" <<'EOF'
#!/usr/bin/env bash
[[ $1 = -u && $2 = todo && $3 = -- ]] || exit 1
shift 3
exec "$@"
EOF
cat > "$root/stubs/systemctl" <<'EOF'
#!/usr/bin/env bash
set -eu
case "$1" in
    show)
        case "$3" in
            FragmentPath) echo "$TEST_ROOT/etc/systemd/system/todo-server.service" ;;
            Environment) echo "TODO_SERVER_CONFIG=$TEST_ROOT/etc/todo/server.toml" ;;
            WorkingDirectory) echo "$TEST_ROOT/var/lib/todo" ;;
        esac
        ;;
    show-environment | reset-failed) ;;
    is-active)
        unit="${!#}"
        if [[ $unit = todo-server.service && -f "$TEST_ROOT/running" ]]; then
            [[ $2 = --quiet ]] || echo active
        else
            [[ $2 = --quiet ]] || echo inactive
            exit 3
        fi
        ;;
    stop)
        printf '%s\n' "$*" >> "$TEST_ROOT/stops"
        if [[ $2 = todo-server.service ]]; then rm -f "$TEST_ROOT/running"; fi
        ;;
    *) exit 1 ;;
esac
EOF
cat > "$root/stubs/curl" <<'EOF'
#!/usr/bin/env bash
if [[ -f "$TEST_ROOT/running" ]]; then echo 200; else echo 000; fi
EOF
chmod +x "$root/stubs/"*

stage() {
    local dir="$root/$1" bundle="$2"
    mkdir -p "$dir/web/fonts"
    # Both bundles intentionally have identical binaries. Asset-only changes must get a new ID.
    cat > "$dir/todo-server" <<'EOF'
#!/usr/bin/env bash
set -eu
case "$1" in
    --version) echo 'todo-server 1.0.0' ;;
    database-path)
        incompatible=none
        case "$(cat "${0%/*}/web/index.html" 2>/dev/null || true)" in
            'first web bundle') incompatible=second_only ;;
            'second web bundle') incompatible=first_only ;;
        esac
        if grep -qE "^\[(future|$incompatible)\]$" "$TODO_SERVER_CONFIG"; then
            echo 'unknown config section' >&2
            exit 1
        fi
        sed -n 's/^data_dir = "\(.*\)"$/\1\/todo.sqlite3/p' "$TODO_SERVER_CONFIG"
        ;;
    *) exit 1 ;;
esac
EOF
    cp "$dir/todo-server" "$dir/todo-backup"
    printf '%s\n' "$bundle" > "$dir/web/index.html"
    printf 'test font\n' > "$dir/web/fonts/font.ttf"
    (cd "$dir" && sha256sum todo-server todo-backup web/fonts/font.ttf web/index.html) > "$dir/SHA256SUMS"
}

run() {
    local want="$1" got=0
    shift
    bash "$@" > "$root/output" 2>&1 || got=$?
    if [[ $got != "$want" ]]; then
        printf 'expected exit %s, got %s: %s\n' "$want" "$got" "$*" >&2
        cat "$root/output" >&2
        exit 1
    fi
}

config="$root/etc/todo/server.toml"
current="$root/opt/todo/current"
marker="$root/opt/todo/deploy-in-progress"
recovery="$root/usr/local/lib/todo/install-release"
db="$root/var/lib/todo/todo.sqlite3"
printf 'data_dir = "%s"\n[web]\ndir = "%s"\n' "$(dirname "$db")" "$current/web" > "$config"
cp "$config" "$root/compatible.toml"
printf 'before deploy\n' > "$db"
stage a 'first web bundle'
stage b 'second web bundle'

run 0 "$root/install-release.sh" --no-start "$root/a"
first="$(readlink "$current")"
grep -qx 'first web bundle' "$current/web/index.html"
[[ -f "$current/web/fonts/font.ttf" && $(stat -c %a "$current/web") = 755 && $(stat -c %a "$current/web/index.html") = 644 ]]

# A corrupted web file fails before stopping a running server or changing current.
touch "$root/running"
: > "$root/stops"
printf 'bad transfer\n' > "$root/b/web/index.html"
run 1 "$root/install-release.sh" --no-start "$root/b"
grep -q 'checksums don.t match' "$root/output"
[[ $(readlink "$current") = "$first" && ! -s "$root/stops" && -f "$root/running" ]]
stage b 'second web bundle'

# Incompatible live config refuses an ordinary deploy as well as recovery, without a switch or stop.
printf '\n[future]\n' >> "$config"
run 1 "$root/install-release.sh" --no-start "$root/b"
grep -q 'refusing to switch' "$root/output"
[[ $(readlink "$current") = "$first" && ! -s "$root/stops" && ! -e "$marker" ]]
cp "$root/compatible.toml" "$config"

# Interrupt before switching, then recover with only the files installed on the VM.
TODO_DEPLOY_CRASH_AT=switched-marked run 137 "$root/install-release.sh" --no-start "$root/b"
[[ $(readlink "$current") = "$first" ]]
rm -rf -- "$root/b"
: > "$root/stops"
printf '\n[first_only]\n' >> "$config"
run 3 "$recovery" --recover finish
grep -q 'refusing to switch' "$root/output"
[[ $(readlink "$current") = "$first" && ! -s "$root/stops" && -f "$marker" ]]
cp "$root/compatible.toml" "$config"
TODO_DEPLOY_CRASH_AT=switched run 137 "$recovery" --recover finish
second="$(readlink "$current")"
[[ $second != "$first" ]]
grep -qx 'second web bundle' "$current/web/index.html"

# A rejected rollback doesn't switch current, stop services, or restore the saved databases.
printf 'after switch\n' > "$db"
printf '\n[second_only]\n' >> "$config"
: > "$root/stops"
run 3 "$recovery" --recover rollback
grep -q 'refusing to switch' "$root/output"
[[ $(readlink "$current") = "$second" && ! -s "$root/stops" && -f "$marker" ]]
grep -qx 'phase=switched' "$marker"
grep -qx 'after switch' "$db"
cp "$root/compatible.toml" "$config"

# A rollback onto a release without a web bundle is refused while the config serves current/web: that
# release's server couldn't start.
first_web="$root/opt/todo/releases/$(basename "$first")/web/index.html"
mv "$first_web" "$root/first-index.html"
: > "$root/stops"
run 3 "$recovery" --recover rollback
grep -q 'has no web bundle' "$root/output"
[[ $(readlink "$current") = "$second" && ! -s "$root/stops" && -f "$marker" ]]
grep -qx 'after switch' "$db"
mv "$root/first-index.html" "$first_web"
run 2 "$recovery" --recover rollback
[[ $(readlink "$current") = "$first" && ! -e "$marker" ]]
grep -qx 'first web bundle' "$current/web/index.html"
grep -qx 'before deploy' "$db"
echo 'install-release tests passed'
