#!/usr/bin/env bash
# Installs a release of todo-server and todo-backup on a provisioned VM, and rolls back if the new
# server doesn't come up healthy. deploy-linux.ps1 uploads it with the binaries and runs it:
#
#   sudo bash install-release.sh [--health-timeout SECONDS] [--keep N] [--no-start] STAGING_DIR
#
# STAGING_DIR holds todo-server, todo-backup and SHA256SUMS. The steps:
#   1. Copy them to a root-only directory, check the checksums, and check that both run here.
#   2. Stop the backup timers (and any run in progress) and todo-server, and check that nothing answers
#      the health URL any more.
#   3. Copy the databases (todo.sqlite3 and attachments.sqlite3 with their -wal and -shm files) to
#      /var/lib/todo-deploy/pre-<time>/, keeping the newest --keep copies. The server is stopped, so the
#      copy is consistent.
#   4. Swap each binary atomically (a rename over /usr/local/bin/<name>), keeping the old one as
#      <name>.prev. Migrations need no step of their own: the server runs them before it listens.
#   5. Start todo-server and wait for /health on the address from `bind` in /etc/todo/server.toml.
#   6. If it isn't healthy in time: put the .prev binaries back and start again. If that fails too
#      (the new release ran a migration the old one doesn't know), move the databases to
#      /var/lib/todo-deploy/failed-<time>/, restore the copy from step 3 and try once more.
#   7. Start the backup timers again, whatever happened.
#
# --no-start stops at step 4: the binaries are installed and the server is left stopped.
#
# Exit codes: 0 deployed; 1 nothing changed; 2 rolled back, the previous release runs; 3 the server is down.
set -euo pipefail
set +x
umask 077

BIN_DIR=/usr/local/bin
CONFIG=/etc/todo/server.toml
DEPLOY_DIR=/var/lib/todo-deploy
SERVICE=todo-server.service
TIMERS=(todo-backup-frequent.timer todo-backup-daily.timer)
BINARIES=(todo-server todo-backup)
DATABASES=(todo.sqlite3 todo.sqlite3-wal todo.sqlite3-shm attachments.sqlite3 attachments.sqlite3-wal attachments.sqlite3-shm)

health_timeout=60
keep=3
no_start=0
staging=""

log() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --health-timeout) [ $# -ge 2 ] || fail "$1 needs seconds"; health_timeout="$2"; shift 2 ;;
        --keep) [ $# -ge 2 ] || fail "$1 needs a count"; keep="$2"; shift 2 ;;
        --no-start) no_start=1; shift ;;
        -h | --help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) fail "unknown option: $1" ;;
        *) [ -z "$staging" ] || fail "only one staging directory"; staging="$1"; shift ;;
    esac
done
[ -n "$staging" ] || fail "usage: install-release.sh [options] STAGING_DIR (see --help)"
case "$health_timeout$keep" in *[!0-9]*) fail "--health-timeout and --keep take whole numbers" ;; esac
[ "$keep" -ge 1 ] || fail "--keep must be at least 1"

[ "$(id -u)" -eq 0 ] || fail "run as root"
[ -d /run/systemd/system ] || fail "systemd is not running"
id todo > /dev/null 2>&1 || fail "no todo user: run provision.sh first"
[ -f "/etc/systemd/system/$SERVICE" ] || fail "no $SERVICE: run provision.sh first"
[ -f "$CONFIG" ] || fail "$CONFIG doesn't exist yet: write it first (see server.toml.example)"
for f in "${BINARIES[@]}" SHA256SUMS; do
    [ -f "$staging/$f" ] || fail "$staging/$f is missing"
done

# The value of a top-level string or bare key in server.toml (before the first [table]).
toml_top() {
    awk -v key="$1" '
        /^[[:space:]]*\[/ { exit }
        {
            line = $0
            if (match(line, "^[[:space:]]*" key "[[:space:]]*=[[:space:]]*")) {
                v = substr(line, RLENGTH + 1)
                if (v ~ /^"/) { v = substr(v, 2); sub(/".*$/, "", v) }
                else if (v ~ /^\047/) { v = substr(v, 2); sub(/\047.*$/, "", v) }
                else { sub(/[[:space:]]*(#.*)?$/, "", v) }
                print v
                exit
            }
        }' "$CONFIG"
}

bind="$(toml_top bind)"
bind="${bind:-127.0.0.1:47320}"
port="${bind##*:}"
host="${bind%:*}"
case "$host" in 0.0.0.0 | "") host=127.0.0.1 ;; "[::]") host="[::1]" ;; esac
health_url="http://$host:$port/health"
data_dir="$(toml_top data_dir)"
case "$data_dir" in
    /*) ;;
    *) printf 'warning: data_dir in %s is not an absolute path; the databases will not be copied\n' "$CONFIG" >&2
       data_dir="" ;;
esac

healthy() { curl -fsS -o /dev/null --max-time 3 "$health_url" 2> /dev/null; }

wait_healthy() {
    local deadline=$((SECONDS + health_timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
        healthy && return 0
        sleep 1
    done
    return 1
}

start_server() {
    systemctl reset-failed "$SERVICE" 2> /dev/null || true
    systemctl start "$SERVICE" || true
    wait_healthy
}

stop_server() {
    systemctl stop "$SERVICE" || true
}

# Called from the EXIT trap.
# shellcheck disable=SC2329
restart_timers() {
    systemctl start "${TIMERS[@]}" 2> /dev/null || printf 'warning: could not start the backup timers\n' >&2
}

# ---- 1. stage and check ---------------------------------------------------------------------------

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
install -d -m 0700 -o root -g root "$DEPLOY_DIR"
work="$DEPLOY_DIR/release-$stamp"
mkdir -m 0700 "$work"
trap 'rm -rf "$work"' EXIT
for f in "${BINARIES[@]}" SHA256SUMS; do
    cp -- "$staging/$f" "$work/$f"
done
(cd "$work" && sha256sum --check --strict --quiet SHA256SUMS) || fail "checksums don't match; nothing changed"
for b in "${BINARIES[@]}"; do
    grep -qE "^[0-9a-f]{64} [ *]$b$" "$work/SHA256SUMS" || fail "SHA256SUMS doesn't cover $b"
    chmod 0755 "$work/$b"
    version="$("$work/$b" --version 2>&1)" || fail "$b doesn't run on this host: $version"
    log "new $version"
done

# ---- 2. stop --------------------------------------------------------------------------------------

log "stopping backups and $SERVICE"
systemctl stop "${TIMERS[@]}" 2> /dev/null || true
trap 'rm -rf "$work"; restart_timers' EXIT
# Interrupting a run is safe: nothing is recorded until an upload completes.
systemctl stop 'todo-backup@*.service' 2> /dev/null || true
was_active=0
systemctl is-active --quiet "$SERVICE" && was_active=1
stop_server
if healthy; then
    printf 'error: something still answers %s after stopping %s; nothing changed\n' "$health_url" "$SERVICE" >&2
    [ "$was_active" -eq 1 ] && systemctl start "$SERVICE"
    exit 1
fi

# ---- 3. copy the databases ------------------------------------------------------------------------

pre=""
if [ -n "$data_dir" ] && [ -f "$data_dir/todo.sqlite3" ]; then
    pre="$DEPLOY_DIR/pre-$stamp"
    mkdir -m 0700 "$pre"
    for f in "${DATABASES[@]}"; do
        [ -e "$data_dir/$f" ] && cp -a -- "$data_dir/$f" "$pre/$f"
    done
    log "copied the databases to $pre"
    # Keep the newest $keep copies.
    find "$DEPLOY_DIR" -mindepth 1 -maxdepth 1 -type d -name 'pre-*' -printf '%f\n' | sort -r |
        tail -n +"$((keep + 1))" | while read -r old; do rm -rf -- "${DEPLOY_DIR:?}/$old"; done
fi

# ---- 4. swap --------------------------------------------------------------------------------------

had_previous=1
for b in "${BINARIES[@]}"; do
    [ -f "$BIN_DIR/$b" ] || had_previous=0
done
for b in "${BINARIES[@]}"; do
    if [ -f "$BIN_DIR/$b" ]; then
        ln -f -- "$BIN_DIR/$b" "$BIN_DIR/$b.prev"
    fi
    install -m 0755 -o root -g root -- "$work/$b" "$BIN_DIR/.$b.new"
    mv -f -- "$BIN_DIR/.$b.new" "$BIN_DIR/$b"
done
log "installed the new binaries"

if [ "$no_start" -eq 1 ]; then
    log "--no-start: $SERVICE stays stopped"
    exit 0
fi

# ---- 5. start -------------------------------------------------------------------------------------

log "starting $SERVICE and waiting up to ${health_timeout}s for $health_url"
if start_server; then
    log "deployed: $("$BIN_DIR/todo-server" --version)"
    exit 0
fi

# ---- 6. roll back ---------------------------------------------------------------------------------

printf 'error: the new release did not become healthy. Its last log lines:\n' >&2
journalctl -u "$SERVICE" -n 30 --no-pager -o cat >&2 || true
stop_server
if [ "$had_previous" -eq 0 ]; then
    printf 'error: there is no previous release to roll back to; %s is down\n' "$SERVICE" >&2
    exit 3
fi
for b in "${BINARIES[@]}"; do
    ln -f -- "$BIN_DIR/$b" "$BIN_DIR/$b.failed"
    install -m 0755 -o root -g root -- "$BIN_DIR/$b.prev" "$BIN_DIR/.$b.new"
    mv -f -- "$BIN_DIR/.$b.new" "$BIN_DIR/$b"
done
log "rolled the binaries back (the failed ones are kept as $BIN_DIR/<name>.failed)"
if start_server; then
    log "rolled back: the previous release runs"
    exit 2
fi
stop_server
if [ -n "$pre" ]; then
    failed="$DEPLOY_DIR/failed-$stamp"
    mkdir -m 0700 "$failed"
    for f in "${DATABASES[@]}"; do
        [ -e "$data_dir/$f" ] && mv -- "$data_dir/$f" "$failed/$f"
    done
    for f in "${DATABASES[@]}"; do
        [ -e "$pre/$f" ] && cp -a -- "$pre/$f" "$data_dir/$f"
    done
    log "restored the databases from $pre (the replaced files are in $failed)"
    if start_server; then
        log "rolled back: the previous release runs on the restored databases"
        exit 2
    fi
fi
printf 'error: the rollback failed; %s is down. See: sudo journalctl -u %s\n' "$SERVICE" "$SERVICE" >&2
stop_server
exit 3
