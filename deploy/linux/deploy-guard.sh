#!/bin/sh
# Installed by provision.sh as /usr/local/lib/todo/deploy-guard and run by todo-server.service and
# todo-backup@.service before they start (ExecStartPre, as root). While a deploy is unfinished
# (/opt/todo/deploy-in-progress exists), it refuses the start, so that after a power cut or a killed
# deploy neither a half-installed server nor a backup of half-restored databases runs.
#
# The one exception is a start of todo-server that install-release.sh authorizes: it writes a random
# nonce to the marker and to /run/todo-deploy/allow-start (root, 0600, on tmpfs, so it can't outlive a
# reboot). This guard allows the start only if the two match and it has deleted the file: if the file
# can't be removed, the start is refused, so each nonce authorizes at most one start. Backups are never
# let through while the marker exists.
set -u
MARKER=/opt/todo/deploy-in-progress
ALLOW=/run/todo-deploy/allow-start
UNIT="${1:-this unit}"

[ -e "$MARKER" ] || exit 0

if [ "$UNIT" = todo-server ] && [ -f "$ALLOW" ] && [ ! -L "$ALLOW" ]; then
    owner_mode="$(stat -c '%u %a' "$ALLOW" 2> /dev/null)"
    have="$(head -n 1 "$ALLOW" 2> /dev/null)"
    if rm -f "$ALLOW" 2> /dev/null && [ ! -e "$ALLOW" ] && [ ! -L "$ALLOW" ]; then
        want="$(sed -n 's/^nonce=//p' "$MARKER" 2> /dev/null | head -n 1)"
        if [ "$owner_mode" = "0 600" ] && [ ${#want} -eq 32 ] && [ "$have" = "$want" ]; then
            exit 0
        fi
    else
        echo "$ALLOW could not be removed, so it can't authorize a start." >&2
    fi
fi

phase="$(sed -n 's/^phase=//p' "$MARKER" 2> /dev/null | head -n 1)"
echo "refusing to start $UNIT: a deploy was interrupted (phase '${phase:-unknown}', $MARKER exists)." >&2
echo "Finish it or roll it back: sudo /usr/local/lib/todo/install-release --recover finish (or rollback)." >&2
echo "See docs/hosting-linux.md, \"An interrupted deploy\"." >&2
exit 1
