#!/usr/bin/env bash
# Installs a release of todo-server and todo-backup on a provisioned VM, and rolls back if the new
# server doesn't come up healthy. deploy-linux.ps1 uploads it with the binaries and runs it:
#
#   sudo bash install-release.sh [--health-timeout S] [--keep N] [--no-start] [--allow-empty-database] STAGING_DIR
#   sudo bash install-release.sh --recover finish|rollback [--health-timeout S]
#   sudo bash install-release.sh --status
#
# STAGING_DIR holds todo-server, todo-backup and SHA256SUMS. A deploy:
#   1. Checks the checksums and copies the binaries into their own directory,
#      /opt/todo/releases/<version>-<hash>/, which is synced to disk before it gets its final name. It asks
#      the new server for the database path its config resolves to (`todo-server database-path`, which
#      also checks the config); a config it refuses stops the deploy before anything changes.
#   2. Writes a durable marker, /opt/todo/deploy-in-progress, with the phase it is in. While the marker
#      exists, todo-server refuses to start (except when this script starts it), so a power cut or a
#      killed deploy never leaves a half-installed server running after a reboot.
#   3. Stops the backup timers and runs and todo-server, and checks that they are inactive and that
#      nothing answers the health URL any more.
#   4. Copies the databases (todo.sqlite3 and attachments.sqlite3 with their -wal and -shm files) to
#      /var/lib/todo-deploy/pre-<time>/ and syncs them. The server is stopped, so the copy is consistent.
#      Without a database where one is expected (a release is already installed), it stops instead,
#      unless --allow-empty-database is given.
#   5. Switches /opt/todo/current to the new release: a new symlink renamed over the old one, then the
#      directory synced. Migrations need no step of their own: the server runs them before it listens.
#   6. Starts todo-server and waits until it is active and /health answers 200.
#   7. If it doesn't: switches back to the previous release and starts again. If that fails too (the new
#      release ran a migration the old one doesn't know), restores the copy from step 4 (each file to a
#      temporary name, synced, renamed over, then the directory synced) and tries once more.
#   8. Removes the marker, and starts again the backup timers that were running.
#
# --no-start stops after step 5: the release is current and the server stays stopped. --keep N (3) is how
# many older database copies (pre-*), replaced files (failed-*) and older releases to keep.
#
# After an interrupted deploy (a reboot or a kill), the marker stays and the server refuses to start.
# --recover finish completes the deploy from where it stopped; --recover rollback returns to the
# previous release and, if the new one may have touched the databases, to the copy from step 4. Neither
# needs the staging directory. --status shows the marker.
#
# Exit codes: 0 deployed (or recovered forward); 1 nothing changed; 2 rolled back, the previous release
# runs; 3 the server is down and the marker stays.
set -euo pipefail
set +x
umask 077

OPT=/opt/todo
RELEASES="$OPT/releases"
CURRENT="$OPT/current"
MARKER="$OPT/deploy-in-progress"
ALLOW=/run/todo-deploy-allow-start
BIN_DIR=/usr/local/bin
CONFIG=/etc/todo/server.toml
DEPLOY_DIR=/var/lib/todo-deploy
SERVICE=todo-server.service
TIMERS=(todo-backup-frequent.timer todo-backup-daily.timer)
BINARIES=(todo-server todo-backup)
DB_BASES=(todo.sqlite3 attachments.sqlite3)
DB_SUFFIXES=("" -wal -shm)

health_timeout=60
keep=3
no_start=0
allow_empty=0
staging=""
recover=""
status_only=0

log() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --health-timeout) [ $# -ge 2 ] || fail "$1 needs seconds"; health_timeout="$2"; shift 2 ;;
        --keep) [ $# -ge 2 ] || fail "$1 needs a count"; keep="$2"; shift 2 ;;
        --no-start) no_start=1; shift ;;
        --allow-empty-database) allow_empty=1; shift ;;
        --recover) [ $# -ge 2 ] || fail "$1 needs finish or rollback"; recover="$2"; shift 2 ;;
        --status) status_only=1; shift ;;
        -h | --help) awk 'NR > 1 && !/^#/ { exit } NR > 1' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) fail "unknown option: $1" ;;
        *) [ -z "$staging" ] || fail "only one staging directory"; staging="$1"; shift ;;
    esac
done
case "$health_timeout$keep" in *[!0-9]*) fail "--health-timeout and --keep take whole numbers" ;; esac
[ "$keep" -ge 1 ] || fail "--keep must be at least 1"
case "$recover" in "" | finish | rollback) ;; *) fail "--recover takes finish or rollback" ;; esac

[ "$(id -u)" -eq 0 ] || fail "run as root"
[ -d /run/systemd/system ] || fail "systemd is not running"
id todo > /dev/null 2>&1 || fail "no todo user: run provision.sh first"
[ -f "/etc/systemd/system/$SERVICE" ] || fail "no $SERVICE: run provision.sh first"
[ -x /usr/local/lib/todo/deploy-guard ] || fail "no deploy guard: run provision.sh again"

# Test hook: TODO_DEPLOY_CRASH_AT=<point> kills this script there, as a power cut would stop it. The
# points are marker-written, stopped, snapshotted, switched, started and restoring.
crash_point() {
    if [ "${TODO_DEPLOY_CRASH_AT:-}" = "$1" ]; then
        printf 'test: crashing at %s\n' "$1" >&2
        kill -KILL $$
    fi
}

# ---- durable files --------------------------------------------------------------------------------

# fsync a file or a directory (GNU sync opens it and calls fsync).
fsync() { sync -- "$@"; }

marker_get() { sed -n "s/^$1=//p" "$MARKER" 2> /dev/null | head -n 1; }

# Writes the marker from the current variables, durably: a temporary file, synced, renamed over.
write_marker() {
    local phase="$1"
    {
        printf 'phase=%s\n' "$phase"
        printf 'new=%s\n' "$new_id"
        printf 'previous=%s\n' "$prev_id"
        printf 'database=%s\n' "$db_path"
        printf 'snapshot=%s\n' "$snapshot"
        printf 'timers=%s\n' "$active_timers"
        printf 'new_ran=%s\n' "$new_ran"
        printf 'started=%s\n' "$stamp"
    } > "$MARKER.tmp"
    fsync "$MARKER.tmp"
    mv -f -- "$MARKER.tmp" "$MARKER"
    fsync "$OPT"
}

remove_marker() {
    rm -f -- "$MARKER" "$MARKER.tmp"
    fsync "$OPT"
}

# Points $1 at $2 atomically: a new symlink renamed over the old one, then the directory synced.
switch_link() {
    local link="$1" target="$2" tmp
    tmp="$(dirname "$link")/.$(basename "$link").new"
    rm -f -- "$tmp"
    ln -s -- "$target" "$tmp"
    mv -T -f -- "$tmp" "$link"
    fsync "$(dirname "$link")"
}

current_id() {
    local t
    t="$(readlink "$CURRENT" 2> /dev/null || true)"
    printf '%s' "${t##*/}"
}

switch_release() {
    [ -d "$RELEASES/$1" ] || fail "release $1 is missing from $RELEASES"
    switch_link "$CURRENT" "releases/$1"
    # Stable names for the operator; they always follow `current`.
    for b in "${BINARIES[@]}"; do
        if [ "$(readlink "$BIN_DIR/$b" 2> /dev/null || true)" != "$CURRENT/$b" ]; then
            switch_link "$BIN_DIR/$b" "$CURRENT/$b"
        fi
    done
    log "current release: $1"
}

# ---- health ---------------------------------------------------------------------------------------

health_url=""
set_health_url() {
    local bind host port
    # bind is only read for the health check; the database path comes from the server itself.
    bind="$(awk '/^[[:space:]]*\[/ { exit } match($0, /^[[:space:]]*bind[[:space:]]*=[[:space:]]*["\047][^"\047]*["\047]/) {
        v = substr($0, RSTART, RLENGTH); sub(/^[^"\047]*["\047]/, "", v); sub(/["\047]$/, "", v); print v; exit }' "$CONFIG" 2> /dev/null || true)"
    bind="${bind:-127.0.0.1:47320}"
    port="${bind##*:}"
    host="${bind%:*}"
    case "$host" in 0.0.0.0 | "") host=127.0.0.1 ;; "[::]") host="[::1]" ;; esac
    health_url="http://$host:$port/health"
}

http_status() { curl -s -o /dev/null -w '%{http_code}' --max-time 3 "$health_url" 2> /dev/null || true; }

healthy() { systemctl is-active --quiet "$SERVICE" && [ "$(http_status)" = 200 ]; }

wait_healthy() {
    local deadline=$((SECONDS + health_timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
        healthy && return 0
        sleep 1
    done
    return 1
}

# Starts the server while the marker exists: the deploy guard lets it through while this script runs.
start_server() {
    printf '%s\n' "$$" > "$ALLOW"
    systemctl reset-failed "$SERVICE" 2> /dev/null || true
    systemctl start "$SERVICE" || true
    wait_healthy
}

stop_all() {
    systemctl stop "${TIMERS[@]}" 2> /dev/null || true
    # Interrupting a backup run is safe: nothing is recorded until an upload completes.
    systemctl stop 'todo-backup@*.service' 2> /dev/null || true
    systemctl stop "$SERVICE" 2> /dev/null || true
    local u
    for u in "$SERVICE" "${TIMERS[@]}" todo-backup@frequent.service todo-backup@daily.service; do
        if systemctl is-active --quiet "$u"; then
            printf 'error: %s is still active after stopping it\n' "$u" >&2
            return 1
        fi
    done
    local code
    code="$(http_status)"
    if [ "$code" != 000 ]; then
        printf 'error: something still answers %s (HTTP %s) after stopping %s\n' "$health_url" "$code" "$SERVICE" >&2
        return 1
    fi
}

restart_timers() {
    local t
    for t in ${active_timers//,/ }; do
        systemctl start "$t" 2> /dev/null || printf 'warning: could not start %s\n' "$t" >&2
    done
}

# Removes the marker and the start permission and starts the timers again: the end of every outcome in
# which the server runs.
finish_ok() {
    remove_marker
    rm -f -- "$ALLOW"
    restart_timers
}

# ---- databases ------------------------------------------------------------------------------------

db_files() { # the database files present in directory $1
    local base suffix
    for base in "${DB_BASES[@]}"; do
        for suffix in "${DB_SUFFIXES[@]}"; do
            [ -e "$1/$base$suffix" ] && printf '%s\n' "$base$suffix"
        done
    done
    return 0
}

take_snapshot() {
    local data_dir f
    data_dir="$(dirname "$db_path")"
    if [ ! -f "$db_path" ]; then
        log "no database at $db_path yet; nothing to copy"
        snapshot=""
        return
    fi
    rm -rf -- "$snapshot"
    mkdir -m 0700 -- "$snapshot" || return 1
    while read -r f; do
        cp -a -- "$data_dir/$f" "$snapshot/$f" || return 1
        cmp -s -- "$data_dir/$f" "$snapshot/$f" || { printf 'error: the copy of %s differs\n' "$f" >&2; return 1; }
        fsync "$snapshot/$f"
    done < <(db_files "$data_dir")
    fsync "$snapshot" "$DEPLOY_DIR"
    log "copied the databases to $snapshot"
}

# Puts the snapshot back: every file to a temporary name next to the live one, synced, then the live
# files moved aside and the copies renamed over, then the directory synced. The snapshot itself is only
# read, so this can run again after an interruption.
restore_snapshot() {
    local data_dir failed f base suffix
    data_dir="$(dirname "$db_path")"
    if [ -z "$snapshot" ] || [ ! -f "$snapshot/todo.sqlite3" ]; then
        printf 'error: no database copy to restore\n' >&2
        return 1
    fi
    failed="$DEPLOY_DIR/failed-$(date -u +%Y%m%dT%H%M%SZ)"
    mkdir -m 0700 -- "$failed" || return 1
    while read -r f; do
        cp -a -- "$snapshot/$f" "$data_dir/.$f.restore" || return 1
        fsync "$data_dir/.$f.restore" || return 1
    done < <(db_files "$snapshot")
    crash_point restoring
    for base in "${DB_BASES[@]}"; do
        [ -e "$snapshot/$base" ] || continue
        # The live WAL must go too: replayed onto the restored file, it would corrupt it.
        for suffix in "${DB_SUFFIXES[@]}"; do
            if [ -e "$data_dir/$base$suffix" ]; then mv -f -- "$data_dir/$base$suffix" "$failed/" || return 1; fi
        done
        for suffix in "${DB_SUFFIXES[@]}"; do
            if [ -e "$data_dir/.$base$suffix.restore" ]; then
                mv -f -- "$data_dir/.$base$suffix.restore" "$data_dir/$base$suffix" || return 1
            fi
        done
    done
    fsync "$data_dir" "$failed"
    log "restored the databases from $snapshot (the replaced files are in $failed)"
}

prune() {
    local kind old r
    # Copies from before a deploy (pre-*) and the files a restore replaced (failed-*): the newest $keep.
    for kind in pre failed; do
        find "$DEPLOY_DIR" -mindepth 1 -maxdepth 1 -type d -name "$kind-*" -printf '%f\n' | sort -r |
            tail -n +"$((keep + 1))" | while read -r old; do rm -rf -- "${DEPLOY_DIR:?}/$old"; done
    done
    # Keep the new and the current release and the newest $keep others.
    find "$RELEASES" -mindepth 1 -maxdepth 1 -type d ! -name '.*' -printf '%T@ %f\n' | sort -rn |
        awk -v a="$new_id" -v b="$prev_id" '$2 != a && $2 != b { print $2 }' |
        tail -n +"$((keep + 1))" | while read -r r; do
        rm -rf -- "${RELEASES:?}/$r"
    done
}

# ---- the steps from a phase onwards ---------------------------------------------------------------

# Rolls back from $1, the phase reached: to the previous release, and to the snapshot if the new release
# ran (it may have migrated the databases). Exits.
rollback_from() {
    local phase="$1"
    write_marker rolling-back
    stop_all || true
    if [ -n "$prev_id" ]; then
        switch_release "$prev_id"
    fi
    if [ "$new_ran" = 1 ]; then
        # The previous release may still open what the new one left; try that first, unless a restore
        # was already under way.
        if [ "$phase" != restoring ] && [ -n "$prev_id" ] && start_server; then
            finish_ok
            log "rolled back: $prev_id runs"
            exit 2
        fi
        stop_all || true
        if [ -n "$snapshot" ] && [ -d "$snapshot" ]; then
            write_marker restoring
            if ! restore_snapshot; then
                rm -f -- "$ALLOW"
                printf 'error: the restore failed; %s is down and the marker stays. See: sudo journalctl -u %s\n' "$SERVICE" "$SERVICE" >&2
                exit 3
            fi
        else
            log "there was no database before the deploy; nothing to restore"
        fi
    fi
    if [ -z "$prev_id" ]; then
        # A first install that failed: there is nothing to go back to.
        finish_ok
        log "no previous release: $SERVICE stays stopped"
        exit 3
    fi
    if start_server; then
        finish_ok
        log "rolled back: $prev_id runs"
        exit 2
    fi
    stop_all || true
    rm -f -- "$ALLOW"
    printf 'error: the rollback failed; %s is down and the marker stays. See: sudo journalctl -u %s\n' "$SERVICE" "$SERVICE" >&2
    exit 3
}

# Runs the deploy from phase $1 onwards. Exits.
deploy_from() {
    local phase="$1"
    case "$phase" in
        stopping)
            log "stopping backups and $SERVICE"
            if ! stop_all; then
                if [ -z "$recover" ]; then
                    # Nothing has changed yet: undo the stop and the marker.
                    if [ "$server_was_active" -eq 1 ]; then start_server || true; fi
                    finish_ok
                    fail "nothing changed"
                fi
                exit 3
            fi
            crash_point stopped
            if ! take_snapshot; then
                # Still nothing changed but the stop: start the current release again.
                printf 'error: could not copy the databases to %s\n' "$snapshot" >&2
                if [ "$server_was_active" -eq 1 ]; then start_server || true; fi
                finish_ok
                fail "nothing changed (is the disk full?)"
            fi
            write_marker snapshotted
            crash_point snapshotted
            ;&
        snapshotted)
            switch_release "$new_id"
            write_marker switched
            crash_point switched
            ;&
        switched | starting)
            if [ "$no_start" -eq 1 ]; then
                finish_ok
                log "--no-start: $new_id is current and $SERVICE stays stopped"
                exit 0
            fi
            new_ran=1
            write_marker starting
            log "starting $SERVICE and waiting up to ${health_timeout}s for HTTP 200 from $health_url"
            if start_server; then
                crash_point started
                finish_ok
                log "deployed: $("$CURRENT/todo-server" --version)"
                exit 0
            fi
            printf 'error: the new release did not become healthy. Its last log lines:\n' >&2
            journalctl -u "$SERVICE" -n 30 --no-pager -o cat >&2 || true
            rollback_from starting
            ;;
        *)
            fail "can't finish from phase '$phase': run --recover rollback"
            ;;
    esac
}

# ---- entry points ---------------------------------------------------------------------------------

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
install -d -m 0755 -o root -g root "$OPT" "$RELEASES"
install -d -m 0700 -o root -g root "$DEPLOY_DIR"
set_health_url
trap 'rm -f -- "$ALLOW"' EXIT
# A dropped SSH connection must not stop a deploy halfway; if it does stop, the marker covers it.
trap '' HUP

if [ "$status_only" -eq 1 ]; then
    if [ -f "$MARKER" ]; then
        printf 'an unfinished deploy:\n'
        sed 's/^/  /' "$MARKER"
    else
        printf 'no deploy in progress; current release: %s\n' "$(current_id)"
    fi
    exit 0
fi

if [ -n "$recover" ]; then
    [ -f "$MARKER" ] || fail "no deploy in progress (no $MARKER)"
    phase="$(marker_get phase)"
    new_id="$(marker_get new)"
    prev_id="$(marker_get previous)"
    db_path="$(marker_get database)"
    snapshot="$(marker_get snapshot)"
    active_timers="$(marker_get timers)"
    new_ran="$(marker_get new_ran)"
    stamp="$(marker_get started)"
    server_was_active=0
    log "recovering ($recover) a deploy of $new_id interrupted in phase '$phase'"
    if [ "$recover" = rollback ]; then
        rollback_from "$phase"
    fi
    [ -d "$RELEASES/$new_id" ] || fail "release $new_id is missing; run --recover rollback"
    deploy_from "$phase"
fi

if [ -f "$MARKER" ]; then
    printf 'error: an earlier deploy was interrupted (%s exists). Run this script with --recover finish or\n' "$MARKER" >&2
    printf '%s\n' '--recover rollback first (see docs/hosting-linux.md, "An interrupted deploy"):' >&2
    sed 's/^/  /' "$MARKER" >&2
    exit 1
fi
[ -n "$staging" ] || fail "usage: install-release.sh [options] STAGING_DIR (see --help)"
[ -f "$CONFIG" ] || fail "$CONFIG doesn't exist yet: write it first (see server.toml.example)"
for f in "${BINARIES[@]}" SHA256SUMS; do
    [ -f "$staging/$f" ] || fail "$staging/$f is missing"
done

# ---- 1. stage and check ---------------------------------------------------------------------------

# Leftovers of a stage that was interrupted; never a finished release.
find "$RELEASES" -mindepth 1 -maxdepth 1 -name '.stage-*' -exec rm -rf -- {} +
work="$RELEASES/.stage-$stamp"
mkdir -m 0755 "$work"
for f in "${BINARIES[@]}" SHA256SUMS; do
    cp -- "$staging/$f" "$work/$f"
done
(cd "$work" && sha256sum --check --strict --quiet SHA256SUMS) || fail "checksums don't match; nothing changed"
for b in "${BINARIES[@]}"; do
    grep -qE "^[0-9a-f]{64} [ *]$b\$" "$work/SHA256SUMS" || fail "SHA256SUMS doesn't cover $b"
    chmod 0755 "$work/$b"
    version="$("$work/$b" --version 2>&1)" || fail "$b doesn't run on this host: $version"
    log "new $version"
done
chmod 0644 "$work/SHA256SUMS"
version="$("$work/todo-server" --version | awk '{ print $2 }')"
new_id="$version-$(sha256sum "$work/todo-server" | cut -c 1-12)"
case "$new_id" in *[!A-Za-z0-9._+-]*) fail "unexpected version string: $new_id" ;; esac
if [ -d "$RELEASES/$new_id" ]; then
    cmp -s "$work/SHA256SUMS" "$RELEASES/$new_id/SHA256SUMS" || fail "$RELEASES/$new_id exists with other binaries"
    rm -rf -- "$work"
    log "release $new_id is already installed; reusing it"
else
    fsync "$work"/* "$work"
    mv -T -- "$work" "$RELEASES/$new_id"
    fsync "$RELEASES"
    log "installed release $new_id"
fi

# The database the new server will open, by its own rules: a relative data_dir is relative to the unit's
# working directory. A config the server refuses stops the deploy here.
workdir="$(systemctl show -p WorkingDirectory --value "$SERVICE")"
workdir="${workdir:-/}"
db_path="$(cd "$workdir" && runuser -u todo -- env -i PATH=/usr/bin:/bin TODO_SERVER_CONFIG="$CONFIG" \
    "$RELEASES/$new_id/todo-server" database-path)" || fail "the new server refuses $CONFIG; nothing changed"
case "$db_path" in /*) ;; *) fail "the server printed an unexpected database path: $db_path" ;; esac
prev_id="$(current_id)"
if [ -n "$prev_id" ] && [ ! -f "$db_path" ] && [ "$allow_empty" -eq 0 ]; then
    fail "release $prev_id is installed but there is no database at $db_path to copy before the deploy. Nothing changed. If that is expected (a fresh or wiped install), pass --allow-empty-database (-AllowEmptyDatabase)."
fi
log "database: $db_path"

# ---- 2. onwards -----------------------------------------------------------------------------------

active_timers=""
for t in "${TIMERS[@]}"; do
    systemctl is-active --quiet "$t" && active_timers="${active_timers:+$active_timers,}$t"
done
server_was_active=0
systemctl is-active --quiet "$SERVICE" && server_was_active=1
snapshot="$DEPLOY_DIR/pre-$stamp"
new_ran=0
# Keep this script where --recover can find it after the upload is gone.
self="$(readlink -f -- "$0")"
if [ "$self" != /usr/local/lib/todo/install-release ]; then
    install -m 0755 -o root -g root -- "$self" /usr/local/lib/todo/.install-release.new
    fsync /usr/local/lib/todo/.install-release.new
    mv -f -- /usr/local/lib/todo/.install-release.new /usr/local/lib/todo/install-release
fi
write_marker stopping
crash_point marker-written
# Prune only once the marker is written, so an interrupted run never loses the copies it may need.
prune
deploy_from stopping
