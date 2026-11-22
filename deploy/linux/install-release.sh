#!/usr/bin/env bash
# Installs a release of todo-server and todo-backup on a provisioned VM, and stops it again if the new
# server doesn't come up healthy. deploy-linux.ps1 uploads it with the binaries and runs it:
#
#   sudo bash install-release.sh [--health-timeout S] [--keep N] [--no-start] [--allow-empty-database] STAGING_DIR
#   sudo bash install-release.sh --recover finish|rollback [--discard-writes] [--health-timeout S]
#   sudo bash install-release.sh --status
#
# STAGING_DIR holds todo-server, todo-backup and SHA256SUMS. Every run that changes anything holds
# /opt/todo/deploy.lock, so only one deploy or recovery runs at a time. Before anything stops, a deploy
# checks the checksums, installs the binaries as /opt/todo/releases/<version>-<hash>/, asks the new
# server for its database path (`todo-server database-path`, which also checks the config) and refuses
# a unit with drop-ins or environment overrides. Nothing has changed if any of this fails.
#
# Then it writes a durable marker, /opt/todo/deploy-in-progress. While it exists, todo-server and the
# backups refuse to start, except for one start at a time that this script authorizes (deploy-guard.sh).
# The marker's phase says what is durably true:
#
#   prepared   `current` is the previous release; the copy may be incomplete; the databases are as the
#              previous release left them. Forward: stop everything, take the backup lock, copy the
#              databases with a MANIFEST of exactly the files that existed (/var/lib/todo-deploy/pre-*).
#   switched   The copy is complete. `current` should be the new release (the rename may not have
#              happened yet), and the new release may have run. Forward: point `current` at it, check,
#              start, wait for /health 200. The marker's `served` says whether it was ever started.
#   restoring  A rollback after the new release may have run; everything was stopped before this was
#              written. The databases may be half restored. Forward: restore the copy in full, every time.
#   restored   The databases are exactly the copy again, durably; the previous release may have run on
#              them since. Forward: point `current` at the previous release, check, start. Never restores.
#
# Every phase begins by stopping everything and re-applying its own state, so each one can run again
# after an interruption. The last step of every outcome starts the backup timers that were running,
# then removes the marker; if a timer won't start, the marker stays and a recovery tries again.
#
# Any start of the new release counts as serving: it may accept writes as soon as it listens, before a
# health check sees it. So served=yes is made durable before the start is authorized (in the same marker
# write as the start's nonce; if that write fails, nothing starts). From then on nothing restores the
# copy on its own: if the new release doesn't become healthy, the deploy and --recover finish stop it,
# check that it stopped, keep the marker and exit 3. Only --recover rollback --discard-writes then
# restores the copy. A rollback needs no flag only while the new release was never started.
#
# --no-start leaves the server stopped; the policy is kept in the marker, so a recovery honours it too.
# --keep N (3) is how many older copies (pre-*), replaced files (failed-*) and older releases to keep.
#
# After an interruption (a power cut, a reboot, a kill), the marker stays and nothing starts.
# --recover finish carries the deploy on from "prepared" or "switched". --recover rollback returns to
# the previous release from any phase; from "switched" it restores the copy first (with served=yes,
# only with --discard-writes). Neither needs the staging directory. --status only reads.
#
# Exit codes: 0 deployed (or finished); 1 nothing changed; 2 rolled back, the previous release is
# current; 3 unfinished: see --status.
set -euo pipefail
set +x
umask 077

OPT=/opt/todo
RELEASES="$OPT/releases"
CURRENT="$OPT/current"
MARKER="$OPT/deploy-in-progress"
LOCK="$OPT/deploy.lock"
ALLOW_DIR=/run/todo-deploy
ALLOW="$ALLOW_DIR/allow-start"
SELF_COPY=/usr/local/lib/todo/install-release
BIN_DIR=/usr/local/bin
CONFIG=/etc/todo/server.toml
BACKUP_LOCK=/var/lib/todo/backup-run.lock
DEPLOY_DIR=/var/lib/todo-deploy
SERVICE=todo-server.service
UNIT_FILE="/etc/systemd/system/$SERVICE"
TIMERS=(todo-backup-frequent.timer todo-backup-daily.timer)
BACKUP_UNITS=(todo-backup@frequent.service todo-backup@daily.service)
BINARIES=(todo-server todo-backup)
DB_BASES=(todo.sqlite3 attachments.sqlite3)
DB_SUFFIXES=("" -wal -shm)
# How long to wait for each unit to become inactive (overridable for tests).
STOP_TIMEOUT="${TODO_DEPLOY_STOP_TIMEOUT:-60}"

health_timeout=60
keep=3
no_start=0
allow_empty=0
staging=""
recover=""
discard_writes=0
status_only=0

log() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
# For failures after the marker: it stays, and nothing starts until a recovery.
die3() {
    printf 'error: %s\n' "$*" >&2
    printf 'The deploy is unfinished and the marker stays. See: sudo %s --status\n' "$SELF_COPY" >&2
    exit 3
}

while [ $# -gt 0 ]; do
    case "$1" in
        --health-timeout) [ $# -ge 2 ] || fail "$1 needs seconds"; health_timeout="$2"; shift 2 ;;
        --keep) [ $# -ge 2 ] || fail "$1 needs a count"; keep="$2"; shift 2 ;;
        --no-start) no_start=1; shift ;;
        --allow-empty-database) allow_empty=1; shift ;;
        --recover) [ $# -ge 2 ] || fail "$1 needs finish or rollback"; recover="$2"; shift 2 ;;
        --discard-writes) discard_writes=1; shift ;;
        --status) status_only=1; shift ;;
        -h | --help) awk 'NR > 1 && !/^#/ { exit } NR > 1' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) fail "unknown option: $1" ;;
        *) [ -z "$staging" ] || fail "only one staging directory"; staging="$1"; shift ;;
    esac
done
case "$health_timeout$keep" in *[!0-9]*) fail "--health-timeout and --keep take whole numbers" ;; esac
[ "$keep" -ge 1 ] || fail "--keep must be at least 1"
case "$recover" in "" | finish | rollback) ;; *) fail "--recover takes finish or rollback" ;; esac
if [ -n "$recover" ] && { [ "$no_start" -eq 1 ] || [ -n "$staging" ]; }; then
    fail "--recover takes no staging directory and no --no-start: the marker holds the deploy's settings"
fi
if [ "$discard_writes" -eq 1 ] && [ "$recover" != rollback ]; then fail "--discard-writes goes with --recover rollback"; fi

[ "$(id -u)" -eq 0 ] || fail "run as root"

marker_get() { sed -n "s/^$1=//p" "$MARKER" 2> /dev/null | head -n 1; }
current_id() {
    local t
    t="$(readlink "$CURRENT" 2> /dev/null || true)"
    printf '%s' "${t##*/}"
}

# ---- --status: reads only -------------------------------------------------------------------------

if [ "$status_only" -eq 1 ]; then
    if [ -e "$LOCK" ] && ! flock --nonblock --shared "$LOCK" true 2> /dev/null; then
        printf 'a deploy or recovery is running right now\n'
    fi
    if [ -f "$MARKER" ]; then
        printf 'an unfinished deploy (current release: %s):\n' "$(current_id)"
        grep -v '^nonce=' "$MARKER" | sed 's/^/  /'
    else
        printf 'no deploy in progress; current release: %s\n' "$(current_id)"
    fi
    exit 0
fi

[ -d /run/systemd/system ] || fail "systemd is not running"
id todo > /dev/null 2>&1 || fail "no todo user: run provision.sh first"
[ -f "$UNIT_FILE" ] || fail "no $UNIT_FILE: run provision.sh first"
[ -x /usr/local/lib/todo/deploy-guard ] || fail "no deploy guard: run provision.sh again"

# ---- one at a time --------------------------------------------------------------------------------

install -d -m 0755 -o root -g root "$OPT"
exec 9> "$LOCK"
flock --nonblock 9 || fail "another deploy or recovery is running (it holds $LOCK)"
install -d -m 0755 -o root -g root "$RELEASES"
install -d -m 0700 -o root -g root "$DEPLOY_DIR"
# A start authorization a killed run left behind is void.
rm -f -- "$ALLOW" "$ALLOW.tmp"
trap 'rm -f -- "$ALLOW" "$ALLOW.tmp"' EXIT
# A dropped SSH connection must not stop a deploy halfway; if it does stop, the marker covers it.
trap '' HUP

# Test hook: TODO_DEPLOY_CRASH_AT=<point> kills this script there, as a power cut would stop it.
crash_point() {
    if [ "${TODO_DEPLOY_CRASH_AT:-}" = "$1" ]; then
        printf 'test: crashing at %s\n' "$1" >&2
        kill -KILL $$
    fi
}

# ---- durable files --------------------------------------------------------------------------------

# fsync files or directories (GNU sync opens each one and calls fsync).
fsync() { sync -- "$@"; }

# Writes the marker from the current variables: a temporary file, synced, renamed over, the directory
# synced. Exits on any failure: without a durable marker nothing else may happen.
write_marker() {
    phase="$1"
    if ! {
        printf 'phase=%s\n' "$phase"
        printf 'new=%s\n' "$new_id"
        printf 'previous=%s\n' "$prev_id"
        printf 'data_dir=%s\n' "$data_dir"
        printf 'snapshot=%s\n' "$snapshot"
        printf 'start=%s\n' "$start_policy"
        printf 'served=%s\n' "$served"
        printf 'timers=%s\n' "$active_timers"
        printf 'nonce=%s\n' "$nonce"
        printf 'started=%s\n' "$stamp"
    } > "$MARKER.tmp" || ! fsync "$MARKER.tmp" || ! mv -f -- "$MARKER.tmp" "$MARKER" || ! fsync "$OPT"; then
        die3 "could not write $MARKER durably"
    fi
}

remove_marker() {
    if ! rm -f -- "$MARKER" "$MARKER.tmp" || ! fsync "$OPT"; then die3 "could not remove $MARKER durably"; fi
}

# Points $1 at $2 atomically: a new symlink renamed over the old one, then the directory synced.
switch_link() {
    local link="$1" target="$2" tmp
    tmp="$(dirname "$link")/.$(basename "$link").new"
    rm -f -- "$tmp" && ln -s -- "$target" "$tmp" && mv -T -f -- "$tmp" "$link" && fsync "$(dirname "$link")"
}

# Makes release $1 current (or, if $1 is empty, no release), whatever `current` points at now, and checks
# the result. Idempotent: every phase that depends on `current` runs it before it starts anything.
ensure_current() {
    local id="$1" b
    if [ -z "$id" ]; then
        if ! rm -f -- "$CURRENT" "$BIN_DIR/todo-server" "$BIN_DIR/todo-backup" || ! fsync "$OPT" "$BIN_DIR"; then
            die3 "could not remove $CURRENT"
        fi
        if [ -e "$CURRENT" ] || [ -L "$CURRENT" ]; then die3 "$CURRENT still exists"; fi
        log "no release is current (there was none before this deploy)"
        return 0
    fi
    [ -x "$RELEASES/$id/todo-server" ] || die3 "release $id is missing from $RELEASES"
    switch_link "$CURRENT" "releases/$id" || die3 "could not switch $CURRENT to $id"
    [ "$(readlink "$CURRENT" 2> /dev/null || true)" = "releases/$id" ] ||
        die3 "$CURRENT points at '$(readlink "$CURRENT" 2> /dev/null || true)', not releases/$id"
    # Stable names for the operator; they always follow `current`.
    for b in "${BINARIES[@]}"; do
        if [ "$(readlink "$BIN_DIR/$b" 2> /dev/null || true)" != "$CURRENT/$b" ]; then
            switch_link "$BIN_DIR/$b" "$CURRENT/$b" || die3 "could not link $BIN_DIR/$b"
        fi
    done
    log "current release: $id"
}

# ---- services -------------------------------------------------------------------------------------

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

# Starts the server while the marker exists. The guard lets exactly one start through: it needs a file
# in /run (gone after a reboot) that holds the same random nonce as the marker, and deletes it.
start_server() {
    nonce="$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
    [ ${#nonce} -eq 32 ] || return 1
    write_marker "$phase"
    install -d -m 0700 -o root -g root "$ALLOW_DIR" || return 1
    printf '%s\n' "$nonce" > "$ALLOW.tmp" && mv -f -- "$ALLOW.tmp" "$ALLOW" || return 1
    systemctl reset-failed "$SERVICE" 2> /dev/null || true
    systemctl start "$SERVICE" || true
    rm -f -- "$ALLOW"
    wait_healthy
}

# Stops the timers, any backup run and the server (which also cancels a queued Restart= attempt), and
# waits until each is inactive or failed. Every stop is attempted even if an earlier one failed, and every
# failure is reported; still_active lists what is still running. Returns non-zero on any failure: nothing
# may touch `current` or the databases while one of them still runs.
still_active=""
stop_all() {
    local u state deadline code failed=0
    still_active=""
    systemctl stop "${TIMERS[@]}" || { printf 'error: could not stop the backup timers\n' >&2; failed=1; }
    systemctl stop 'todo-backup@*.service' || { printf 'error: could not stop the backup runs\n' >&2; failed=1; }
    systemctl stop "$SERVICE" || { printf 'error: could not stop %s\n' "$SERVICE" >&2; failed=1; }
    deadline=$((SECONDS + STOP_TIMEOUT))
    for u in "$SERVICE" "${TIMERS[@]}" "${BACKUP_UNITS[@]}"; do
        while :; do
            state="$(systemctl is-active "$u" 2> /dev/null || true)"
            case "$state" in inactive | failed) break ;; esac
            if [ "$SECONDS" -ge "$deadline" ]; then
                printf 'error: %s is still %s after stopping it\n' "$u" "${state:-unknown}" >&2
                still_active="${still_active:+$still_active, }$u"
                failed=1
                break
            fi
            sleep 1
        done
    done
    systemctl reset-failed "$SERVICE" 2> /dev/null || true
    code="$(http_status)"
    if [ "$code" != 000 ]; then
        printf 'error: something still answers %s (HTTP %s)\n' "$health_url" "$code" >&2
        still_active="${still_active:+$still_active, }a listener on $health_url"
        failed=1
    fi
    return "$failed"
}

# The last step of every outcome: the timers that were running start again (harmless while the marker
# exists, because the guard refuses backup runs), and then the marker goes, as the very last step. A
# timer that won't start keeps the marker, so a recovery runs this again.
finalize() {
    local t
    for t in ${active_timers//,/ }; do
        # A timer that hit its start limit (deploys in quick succession) would otherwise stay off.
        systemctl reset-failed "$t" 2> /dev/null || true
        if ! systemctl start "$t" || ! systemctl is-active --quiet "$t"; then
            die3 "could not start $t, which ran before the deploy; the backups stay off until it does. Fix it, then run --recover finish (or rollback) to finish again"
        fi
    done
    crash_point finalizing
    remove_marker
}

# Starts the release that `current` names, by the start policy, and ends the run with exit code $1.
# $2 says which release that is, for the messages.
start_and_finish() {
    local code="$1" what="$2" id
    id="$(current_id)"
    if [ "$start_policy" = no ] || [ -z "$id" ]; then
        finalize
        if [ -z "$id" ]; then log "no release is current: $SERVICE stays stopped"; else
            log "the $what release $id is current; $SERVICE stays stopped (--no-start)"; fi
        exit "$code"
    fi
    log "starting $SERVICE ($what release $id), waiting up to ${health_timeout}s for HTTP 200 from $health_url"
    # Once it starts, the new release may accept writes. start_server makes served=yes durable (with the
    # nonce) before it authorizes the start, and exits 3 without starting anything if it can't.
    if [ "$what" = new ]; then served=yes; fi
    if start_server; then
        crash_point "started-$what"
        finalize
        log "the $what release runs: $("$CURRENT/todo-server" --version)"
        exit "$code"
    fi
    return 1
}

# ---- databases ------------------------------------------------------------------------------------

# Holds the lock every backup run takes, until this script exits, so no run reads or writes a database
# file while it is copied or replaced.
backup_locked=0
take_backup_lock() {
    [ "$backup_locked" -eq 0 ] || return 0
    if [ ! -e "$BACKUP_LOCK" ]; then
        install -m 0600 -o todo -g todo /dev/null "$BACKUP_LOCK" || return 1
    fi
    exec 8< "$BACKUP_LOCK" || return 1
    flock --nonblock 8 || { printf 'error: a backup run holds %s\n' "$BACKUP_LOCK" >&2; return 1; }
    backup_locked=1
}

db_names() { # every database and sidecar name this script handles
    local base suffix
    for base in "${DB_BASES[@]}"; do
        for suffix in "${DB_SUFFIXES[@]}"; do printf '%s\n' "$base$suffix"; done
    done
}

# True if the files in directory $1 match the MANIFEST (an empty MANIFEST matches: nothing existed).
manifest_matches() { [ ! -s "$snapshot/MANIFEST" ] || (cd "$1" && sha256sum --check --quiet --strict "$snapshot/MANIFEST"); }

in_manifest() { awk -v n="$1" '$2 == n { found = 1 } END { exit !found }' "$snapshot/MANIFEST"; }

# The data directory is writable by the service user: refuse a symlink where a directory or database
# should be, rather than follow it.
check_data_dir() {
    if [ -L "$data_dir" ] || { [ -e "$data_dir" ] && [ ! -d "$data_dir" ]; }; then
        printf 'error: %s is not a plain directory\n' "$data_dir" >&2
        return 1
    fi
}

# Copies the databases and sidecars that exist to $snapshot, then writes MANIFEST (a sha256sum list of
# exactly those files) last: a copy without a MANIFEST is incomplete. Every step is checked.
take_snapshot() {
    local f
    check_data_dir || return 1
    rm -rf -- "$snapshot" || return 1
    mkdir -m 0700 -- "$snapshot" || return 1
    : > "$snapshot/MANIFEST.tmp" || return 1
    while read -r f; do
        if [ -L "$data_dir/$f" ] || { [ -e "$data_dir/$f" ] && [ ! -f "$data_dir/$f" ]; }; then
            printf 'error: %s is not a regular file\n' "$data_dir/$f" >&2
            return 1
        fi
        [ -e "$data_dir/$f" ] || continue
        cp -a -- "$data_dir/$f" "$snapshot/$f" || return 1
        cmp -s -- "$data_dir/$f" "$snapshot/$f" || { printf 'error: the copy of %s differs\n' "$f" >&2; return 1; }
        fsync "$snapshot/$f" || return 1
        (cd "$snapshot" && sha256sum -- "$f") >> "$snapshot/MANIFEST.tmp" || return 1
        crash_point snapshot-partial
    done < <(db_names)
    fsync "$snapshot/MANIFEST.tmp" || return 1
    mv -f -- "$snapshot/MANIFEST.tmp" "$snapshot/MANIFEST" || return 1
    fsync "$snapshot" "$DEPLOY_DIR" || return 1
    if [ -s "$snapshot/MANIFEST" ]; then
        log "copied the databases to $snapshot: $(awk '{ printf "%s ", $2 }' "$snapshot/MANIFEST")"
    else
        # The empty MANIFEST is the record a rollback needs: there was nothing to restore.
        log "no database yet, so nothing to copy ($snapshot records that with an empty MANIFEST)"
    fi
}

# Puts the data directory back exactly as the MANIFEST lists it, without following anything the service
# user could have planted there:
#   1. The listed files are copied into a fresh root-only directory inside the data directory (the same
#      file system, so renames are atomic), given the owner and mode, and synced.
#   2. For each database, every live entry with one of its names (a file, a symlink, anything) is moved
#      aside into a failed-* directory, and the copies are renamed in. A rename replaces a name; it never
#      follows a symlink.
#   3. Everything is synced and checked: exactly the listed names exist, as regular files with the listed
#      contents.
# The copy is only read, so this can run again after an interruption at any point.
restore_snapshot() {
    local failed stage name base suffix owner p
    [ -f "$snapshot/MANIFEST" ] || { printf 'error: %s has no MANIFEST\n' "$snapshot" >&2; return 1; }
    manifest_matches "$snapshot" || { printf 'error: the copy in %s is damaged\n' "$snapshot" >&2; return 1; }
    check_data_dir || return 1
    if [ ! -d "$data_dir" ]; then
        # Never created: right only if there was nothing to copy either.
        [ ! -s "$snapshot/MANIFEST" ] || { printf 'error: %s is missing\n' "$data_dir" >&2; return 1; }
        log "there was no database before the deploy and there is none now"
        return 0
    fi
    owner="$(stat -c '%U:%G' -- "$data_dir")" || return 1
    find "$data_dir" -mindepth 1 -maxdepth 1 -name '.restore-*' -exec rm -rf -- {} + || return 1
    stage="$(mktemp -d "$data_dir/.restore-XXXXXX")" || return 1
    if [ -L "$stage" ] || [ "$(stat -c '%u %a' -- "$stage")" != "0 700" ]; then
        printf 'error: %s is not a root-only directory\n' "$stage" >&2
        return 1
    fi
    while read -r name; do
        in_manifest "$name" || continue
        if [ -L "$snapshot/$name" ] || [ ! -f "$snapshot/$name" ]; then
            printf 'error: %s is not a file\n' "$snapshot/$name" >&2
            return 1
        fi
        cp --no-dereference --preserve=timestamps -- "$snapshot/$name" "$stage/$name" || return 1
        chown -- "$owner" "$stage/$name" || return 1
        chmod 0600 -- "$stage/$name" || return 1
        fsync "$stage/$name" || return 1
    done < <(db_names)
    fsync "$stage" || return 1
    crash_point restore-copied
    failed="$(mktemp -d "$DEPLOY_DIR/failed-$(date -u +%Y%m%dT%H%M%SZ)-XXXX")" || return 1
    for base in "${DB_BASES[@]}"; do
        for suffix in "${DB_SUFFIXES[@]}"; do
            p="$data_dir/$base$suffix"
            if [ -e "$p" ] || [ -L "$p" ]; then
                if [ -L "$p" ] || [ ! -f "$p" ]; then
                    printf 'warning: %s is not a regular file; moving it aside unread\n' "$p" >&2
                fi
                mv -T -- "$p" "$failed/$base$suffix" || return 1
            fi
        done
        for suffix in "${DB_SUFFIXES[@]}"; do
            if [ -e "$stage/$base$suffix" ]; then mv -T -- "$stage/$base$suffix" "$data_dir/$base$suffix" || return 1; fi
        done
        crash_point "restore-swapped-$base"
    done
    rmdir -- "$stage" || return 1
    fsync "$data_dir" "$failed" "$DEPLOY_DIR" || return 1
    while read -r name; do
        p="$data_dir/$name"
        if in_manifest "$name"; then
            if [ -L "$p" ] || [ ! -f "$p" ]; then printf 'error: %s was not restored\n' "$p" >&2; return 1; fi
        elif [ -e "$p" ] || [ -L "$p" ]; then
            printf 'error: %s exists, but the copy has no such file\n' "$p" >&2
            return 1
        fi
    done < <(db_names)
    manifest_matches "$data_dir" || { printf 'error: the restored files differ from the copy\n' >&2; return 1; }
    log "restored the databases from $snapshot (the replaced files are in $failed)"
}

prune() {
    local kind old r
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

# ---- the phases -----------------------------------------------------------------------------------

# Rolls back from phase $1 and ends the run with exit code $2 (default 2).
rollback_from() {
    local code="${2:-2}"
    case "$1" in
        prepared)
            # The new release never ran: the databases are as the previous release left them.
            stop_all || die3 "could not stop everything; nothing was rolled back"
            ensure_current "$prev_id"
            start_and_finish "$code" previous ||
                die3 "the previous release did not become healthy. See: sudo journalctl -u $SERVICE"
            ;;
        switched | restoring)
            # The new release may have run, and a restore may be half done: restore in full, every time.
            if [ "$1" = switched ] && [ "$served" = yes ]; then
                printf 'The new release %s was started (served=yes), so it may have accepted writes since\n' "$new_id" >&2
                printf 'the deploy started (%s). A rollback restores the copy taken before the deploy, %s;\n' "$stamp" "$snapshot" >&2
                printf 'the live databases move to %s/failed-<time>/ and stay there, and everything written since is lost\n' "$DEPLOY_DIR" >&2
                printf 'from the running service.\n' >&2
                if [ "$discard_writes" -ne 1 ]; then
                    printf 'error: nothing was changed. Run --recover finish to keep those writes, or\n' >&2
                    printf '  sudo %s --recover rollback --discard-writes\nto discard them.\n' "$SELF_COPY" >&2
                    exit 1
                fi
                log "--discard-writes: going ahead"
            fi
            stop_all || die3 "could not stop everything; the databases were not touched"
            take_backup_lock || die3 "could not take the backup lock; the databases were not touched"
            # Only now, with everything stopped and nothing able to start, does the restore begin.
            write_marker restoring
            crash_point rollback-marked
            restore_snapshot || die3 "the restore did not complete; run --recover rollback again"
            write_marker restored
            crash_point restored
            ;&
        restored)
            # The databases are the copy. The previous release may have run on them since: never restore again.
            stop_all || die3 "could not stop everything"
            ensure_current "$prev_id"
            start_and_finish 2 previous ||
                die3 "the previous release did not become healthy on the restored databases. See: sudo journalctl -u $SERVICE"
            ;;
        *) die3 "unknown phase '$1' in $MARKER" ;;
    esac
}

# Runs the deploy forward from phase $1. Exits.
deploy_from() {
    case "$1" in
        prepared)
            log "stopping backups and $SERVICE"
            if ! stop_all || ! take_backup_lock; then
                printf 'error: could not stop everything and take the backup lock; rolling back\n' >&2
                rollback_from prepared 1
            fi
            crash_point stopped
            if ! take_snapshot; then
                printf 'error: could not copy the databases to %s; rolling back\n' "$snapshot" >&2
                rollback_from prepared 1
            fi
            write_marker switched
            crash_point switched-marked
            ;&
        switched)
            stop_all || die3 "could not stop everything"
            ensure_current "$new_id"
            crash_point switched
            if ! start_and_finish 0 new; then
                printf 'error: the new release did not become healthy. Its last log lines:\n' >&2
                journalctl -u "$SERVICE" -n 30 --no-pager -o cat >&2 || true
                # It was started, so it may have accepted writes: never restore on our own. Stop it,
                # keep the marker, and leave the choice to the operator.
                if stop_all; then
                    die3 "the new release $new_id was started (served=yes) and may have accepted writes, so the copy is not restored automatically. It is stopped, and so are the backups. Fix the cause and run --recover finish, or discard its writes with --recover rollback --discard-writes"
                fi
                if [ -n "$still_active" ]; then
                    die3 "the new release $new_id did not become healthy, and it could not be stopped completely: still active: $still_active. Stop it by hand (sudo systemctl stop $SERVICE), then run --recover finish, or --recover rollback --discard-writes"
                fi
                die3 "the new release $new_id did not become healthy. Nothing is still active, but a stop command failed (see the errors above). Check with systemctl status, then run --recover finish, or --recover rollback --discard-writes"
            fi
            ;;
        restoring | restored) fail "a rollback is under way (phase $1): run --recover rollback" ;;
        *) die3 "unknown phase '$1' in $MARKER" ;;
    esac
}

set_health_url

# ---- recovery -------------------------------------------------------------------------------------

if [ -n "$recover" ]; then
    [ -f "$MARKER" ] || fail "no deploy in progress (no $MARKER)"
    phase="$(marker_get phase)"
    new_id="$(marker_get new)"
    prev_id="$(marker_get previous)"
    data_dir="$(marker_get data_dir)"
    snapshot="$(marker_get snapshot)"
    start_policy="$(marker_get start)"
    served="$(marker_get served)"
    active_timers="$(marker_get timers)"
    nonce="$(marker_get nonce)"
    stamp="$(marker_get started)"
    case "$start_policy" in yes | no) ;; *) die3 "the marker has no start policy" ;; esac
    case "$served" in yes | no) ;; *) die3 "the marker doesn't say whether the new release served (served=)" ;; esac
    if [ -z "$data_dir" ] || [ -z "$snapshot" ] || [ -z "$new_id" ]; then die3 "the marker is incomplete"; fi
    log "recovering ($recover) a deploy of $new_id interrupted in phase '$phase'"
    if [ "$recover" = rollback ]; then rollback_from "$phase"; fi
    deploy_from "$phase"
fi

# ---- a new deploy ---------------------------------------------------------------------------------

if [ -f "$MARKER" ]; then
    printf 'error: an earlier deploy is unfinished (%s exists):\n' "$MARKER" >&2
    grep -v '^nonce=' "$MARKER" | sed 's/^/  /' >&2
    printf 'Run %s --recover finish or --recover rollback first (docs/hosting-linux.md, "An interrupted deploy").\n' "$SELF_COPY" >&2
    exit 1
fi
[ -n "$staging" ] || fail "usage: install-release.sh [options] STAGING_DIR (see --help)"
[ -f "$CONFIG" ] || fail "$CONFIG doesn't exist yet: write it first (see server.toml.example)"
for f in "${BINARIES[@]}" SHA256SUMS; do
    [ -f "$staging/$f" ] || fail "$staging/$f is missing"
done

# The config file is the one source of the server's settings. The unit is ours alone: refuse drop-ins,
# variables passed in or unset from the manager, an EnvironmentFile, a TODO_SERVER_* variable other than
# TODO_SERVER_CONFIG=$CONFIG, and TODO_SERVER_* in the manager's own environment.
unit_prop() { systemctl show -p "$1" --value "$SERVICE"; }
[ "$(unit_prop FragmentPath)" = "$UNIT_FILE" ] || fail "$SERVICE is not loaded from $UNIT_FILE; run provision.sh again. Nothing changed."
for p in DropInPaths PassEnvironment UnsetEnvironment EnvironmentFiles; do
    v="$(unit_prop "$p")"
    [ -z "$v" ] || fail "$SERVICE has $p set ($v). The unit must not have drop-ins; config lives in $CONFIG. Nothing changed."
done
if systemctl show-environment | grep -q '^TODO_SERVER_'; then
    fail "the systemd manager's environment sets TODO_SERVER_* (systemctl show-environment); config lives in $CONFIG. Nothing changed."
fi
set -f
config_seen=0
for kv in $(unit_prop Environment); do
    case "$kv" in
        "TODO_SERVER_CONFIG=$CONFIG") config_seen=1 ;;
        TODO_SERVER_*) fail "$SERVICE sets ${kv%%=*} in its environment; config lives in $CONFIG. Nothing changed." ;;
    esac
done
set +f
[ "$config_seen" -eq 1 ] || fail "$SERVICE doesn't set TODO_SERVER_CONFIG=$CONFIG; run provision.sh again. Nothing changed."

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
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
    fsync "$work"/* "$work" || fail "could not sync $work"
    mv -T -- "$work" "$RELEASES/$new_id"
    fsync "$RELEASES" || fail "could not sync $RELEASES"
    log "installed release $new_id"
fi

# The database the new server will open, by its own rules: run as the service user, in the unit's working
# directory, with the config the unit passes and nothing else. A config it refuses stops the deploy here.
workdir="$(unit_prop WorkingDirectory)"
db_path="$(cd "${workdir:-/}" && runuser -u todo -- env -i PATH=/usr/bin:/bin TODO_SERVER_CONFIG="$CONFIG" \
    "$RELEASES/$new_id/todo-server" database-path)" || fail "the new server refuses $CONFIG; nothing changed"
case "$db_path" in /*) ;; *) fail "the server printed an unexpected database path: $db_path" ;; esac
data_dir="$(dirname "$db_path")"
prev_id="$(current_id)"
if [ -n "$prev_id" ] && [ ! -f "$db_path" ] && [ "$allow_empty" -eq 0 ]; then
    fail "release $prev_id is installed but there is no database at $db_path to copy before the deploy. Nothing changed. If that is expected (a fresh or wiped install, or a VM that has not been cut over yet), pass --allow-empty-database (-AllowEmptyDatabase)."
fi
log "database: $db_path"

active_timers=""
for t in "${TIMERS[@]}"; do
    if systemctl is-active --quiet "$t"; then active_timers="${active_timers:+$active_timers,}$t"; fi
done
snapshot="$DEPLOY_DIR/pre-$stamp"
start_policy=yes
[ "$no_start" -eq 0 ] || start_policy=no
served=no
nonce=""

# Keep this script where --recover can find it after the upload is gone.
self="$(readlink -f -- "$0")"
if [ "$self" != "$SELF_COPY" ]; then
    if ! install -m 0755 -o root -g root -- "$self" "$SELF_COPY.new" || ! fsync "$SELF_COPY.new" ||
        ! mv -f -- "$SELF_COPY.new" "$SELF_COPY"; then
        fail "could not install $SELF_COPY"
    fi
fi

crash_point staged
write_marker prepared
crash_point marker-written
# Prune only once the marker is written, so an interrupted run never loses the copies it may need.
prune
deploy_from prepared
