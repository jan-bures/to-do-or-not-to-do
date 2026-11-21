#!/bin/sh
# Installed by provision.sh as /usr/local/lib/todo/deploy-guard and run by todo-server.service and
# todo-backup@.service before they start (ExecStartPre, as root). While a deploy is unfinished
# (/opt/todo/deploy-in-progress exists), it refuses the start, so that after a power cut or a killed
# deploy neither a half-installed server nor a backup of half-restored databases runs. The deploy
# script itself starts the server through it: it writes its PID to /run/todo-deploy-allow-start, and
# the start is allowed while that process is alive. Backups are always refused while the marker exists.
set -u
MARKER=/opt/todo/deploy-in-progress
ALLOW=/run/todo-deploy-allow-start
UNIT="${1:-this unit}"

[ -e "$MARKER" ] || exit 0

if [ "$UNIT" = todo-server ] && [ -f "$ALLOW" ]; then
    pid="$(head -c 16 "$ALLOW" | tr -cd 0-9)"
    if [ -n "$pid" ] && grep -q install-release "/proc/$pid/cmdline" 2> /dev/null; then
        exit 0
    fi
fi

phase="$(sed -n 's/^phase=//p' "$MARKER" 2> /dev/null | head -n 1)"
echo "refusing to start $UNIT: a deploy was interrupted (phase '${phase:-unknown}', $MARKER exists)." >&2
echo "Finish it or roll it back: sudo /usr/local/lib/todo/install-release --recover finish (or rollback)." >&2
echo "See docs/hosting-linux.md, \"An interrupted deploy\"." >&2
exit 1
