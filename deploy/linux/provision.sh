#!/usr/bin/env bash
# Provisions a Debian VM (Debian 13, x86_64) for todo-server. Idempotent: run it again after changing
# anything here, or to add the tunnel token later. Run as root from a copy of deploy/linux:
#
#   sudo bash provision.sh [--tunnel-token-file FILE] [--swap-size SIZE] [--no-auto-reboot]
#
# It sets up:
#   - the `todo` system user, /var/lib/todo (0700) and /etc/todo (0750, root:todo), and the
#     `cloudflared` system user for the connector;
#   - a swap file (1G by default; --swap-size 0 skips it) and a low swappiness;
#   - UDP buffer limits large enough for cloudflared's QUIC connections;
#   - unattended-upgrades (security updates, cloudflared, and a reboot at 04:30 UTC when one is needed);
#   - journald size limits;
#   - cloudflared from Cloudflare's apt repository, and the todo-tunnel service once a token is given
#     (the file must be root-only; it is installed as /etc/cloudflared/todo-tunnel.token, root:cloudflared
#     0640);
#   - the systemd units: todo-server, todo-backup@ with its two timers, todo-tunnel; and the deploy
#     guard and install-release.sh in /usr/local/lib/todo (see install-release.sh).
#
# It never enables or starts todo-server or the backup timers: a deploy starts the server, and the
# runbook enables both at cutover, so a staging VM that reboots stays quiet. It restarts the server after
# a unit change if it runs. It never prints the tunnel token. Nothing here is provider-specific and nothing
# opens an inbound port. See docs/hosting-linux.md.
set -euo pipefail
set +x
umask 022

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

TODO_USER=todo
TUNNEL_USER=cloudflared
DATA_DIR=/var/lib/todo
CONFIG_DIR=/etc/todo
UNIT_DIR=/etc/systemd/system
TOKEN_DIR=/etc/cloudflared
TOKEN_FILE="$TOKEN_DIR/todo-tunnel.token"
SWAP_FILE=/swapfile
UNITS=(todo-server.service todo-backup@.service todo-backup-frequent.timer todo-backup-daily.timer todo-tunnel.service)

# Cloudflare's package signing key (https://pkg.cloudflare.com/). The download is checked against this
# fingerprint (the 2025 signing key), so a tampered or swapped key stops provisioning. If Cloudflare
# rotates the key again, check the new fingerprint on pkg.cloudflare.com and pass it in
# CLOUDFLARE_KEY_FINGERPRINT.
CLOUDFLARE_KEY_URL=https://pkg.cloudflare.com/cloudflare-main.gpg
CLOUDFLARE_KEYRING=/usr/share/keyrings/cloudflare-main.gpg
CLOUDFLARE_KEY_FINGERPRINT="${CLOUDFLARE_KEY_FINGERPRINT:-CC94B39C77AE7342A68B89628A682D308D4E5E73}"
# For tests: a local key file to use instead of the download.
CLOUDFLARE_KEY_FILE="${CLOUDFLARE_KEY_FILE:-}"

token_src=""
swap_size=1G
auto_reboot=1

usage() {
    sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'
}

log() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --tunnel-token-file) [ $# -ge 2 ] || die "$1 needs a file"; token_src="$2"; shift 2 ;;
        --swap-size) [ $# -ge 2 ] || die "$1 needs a size such as 1G, or 0"; swap_size="$2"; shift 2 ;;
        --no-auto-reboot) auto_reboot=0; shift ;;
        -h | --help) usage; exit 0 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

[ "$(id -u)" -eq 0 ] || die "run as root (sudo bash $0)"
[ -r /etc/debian_version ] || die "this script is for Debian"
[ "$(uname -m)" = x86_64 ] || warn "this is $(uname -m); the deploy script builds x86_64 binaries"
for u in "${UNITS[@]}"; do
    [ -f "$script_dir/systemd/$u" ] || die "missing $script_dir/systemd/$u; copy the whole deploy/linux directory"
done
for f in deploy-guard.sh install-release.sh; do
    [ -f "$script_dir/$f" ] || die "missing $script_dir/$f; copy the whole deploy/linux directory"
done

# systemd may be absent (a plain container): then files are written but no service is touched.
have_systemd=0
[ -d /run/systemd/system ] && have_systemd=1
in_container=0
if command -v systemd-detect-virt > /dev/null 2>&1 && systemd-detect-virt --container --quiet; then
    in_container=1
elif [ -f /.dockerenv ] || [ -f /run/.containerenv ]; then
    in_container=1
fi

# Checks the tunnel token file before anything changes: root-owned, a regular file, no group or other
# access, and plausibly a token. Its contents are never printed.
if [ -n "$token_src" ]; then
    if [ ! -f "$token_src" ] || [ -L "$token_src" ]; then die "$token_src is not a regular file"; fi
    [ "$(stat -c %u "$token_src")" -eq 0 ] || die "$token_src must be owned by root (sudo chown root: $token_src)"
    mode="$(stat -c %a "$token_src")"
    [ $((8#$mode & 8#077)) -eq 0 ] || die "$token_src has mode $mode; make it readable by root only (chmod 600)"
    size="$(stat -c %s "$token_src")"
    if [ "$size" -lt 32 ] || [ "$size" -gt 4096 ]; then die "$token_src doesn't look like a tunnel token ($size bytes)"; fi
    [ "$(tr -d '\r\n' < "$token_src" | LC_ALL=C tr -d 'A-Za-z0-9+/=_-' | wc -c)" -eq 0 ] ||
        die "$token_src doesn't look like a tunnel token (unexpected characters)"
fi

# Writes stdin to $1 with mode $2 and owner $3 if the content or metadata differ. Sets $changed to 1 or 0.
changed=0
put_file() {
    local dest="$1" mode="$2" owner="$3" tmp
    tmp="$(mktemp)"
    cat > "$tmp"
    # stat prints the mode without leading zeros.
    if [ -f "$dest" ] && cmp -s "$tmp" "$dest" &&
        [ "$(stat -c '%a %U:%G' "$dest")" = "$(printf '%o' "$((8#$mode))") $owner" ]; then
        rm -f "$tmp"
        changed=0
        return
    fi
    install -m "$mode" -o "${owner%%:*}" -g "${owner##*:}" "$tmp" "$dest"
    rm -f "$tmp"
    changed=1
}

# ---- packages -------------------------------------------------------------------------------------

export DEBIAN_FRONTEND=noninteractive
log "installing base packages"
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
    ca-certificates curl gpg tzdata sqlite3 util-linux unattended-upgrades > /dev/null

# ---- cloudflared from Cloudflare's apt repository -------------------------------------------------

# gpg only inspects keys here; a throwaway home keeps it from creating /root/.gnupg.
key_tmp="$(mktemp -d)"
trap 'rm -rf "$key_tmp"' EXIT
# Prints the fingerprints of the primary keys in a keyring file (not their subkeys), one per line, and
# nothing if it isn't one.
primary_fingerprints() {
    gpg --homedir "$key_tmp" --batch --quiet --with-colons --show-keys "$1" 2> /dev/null |
        awk -F: '$1 == "pub" { want = 1; next } $1 == "fpr" && want { print $10 } { want = 0 }' || true
}
# apt trusts every key in a signed-by keyring, so the keyring must hold the pinned key and nothing else.
installed="$(if [ -f "$CLOUDFLARE_KEYRING" ]; then primary_fingerprints "$CLOUDFLARE_KEYRING"; fi)"
if [ "$installed" = "$CLOUDFLARE_KEY_FINGERPRINT" ]; then
    log "Cloudflare's apt key is already installed"
else
    log "adding Cloudflare's apt key"
    if [ -n "$CLOUDFLARE_KEY_FILE" ]; then
        cp -- "$CLOUDFLARE_KEY_FILE" "$key_tmp/key"
    else
        curl -fsSL --proto '=https' --tlsv1.2 -o "$key_tmp/key" "$CLOUDFLARE_KEY_URL"
    fi
    # Exactly the pinned key: a download with another primary key in it is refused, since apt would
    # trust that key to sign packages too.
    fingerprints="$(primary_fingerprints "$key_tmp/key")"
    if [ "$fingerprints" != "$CLOUDFLARE_KEY_FINGERPRINT" ]; then
        die "the key file at $CLOUDFLARE_KEY_URL holds the primary key(s) [$(printf '%s' "$fingerprints" | tr '\n' ' ')], but exactly $CLOUDFLARE_KEY_FINGERPRINT was expected"
    fi
    # Import it into a throwaway home and export it again by fingerprint: the keyring then holds only that
    # key (with its subkeys), as a binary keyring, whatever the download's format.
    gpg --homedir "$key_tmp" --batch --quiet --no-autostart --import "$key_tmp/key" 2> /dev/null ||
        die "gpg could not import the key from $CLOUDFLARE_KEY_URL"
    gpg --homedir "$key_tmp" --batch --quiet --no-autostart --export -o "$key_tmp/key.gpg" "$CLOUDFLARE_KEY_FINGERPRINT"
    exported="$(primary_fingerprints "$key_tmp/key.gpg")"
    [ "$exported" = "$CLOUDFLARE_KEY_FINGERPRINT" ] ||
        die "the exported keyring holds [$(printf '%s' "$exported" | tr '\n' ' ')], not exactly $CLOUDFLARE_KEY_FINGERPRINT"
    install -d -m 0755 /usr/share/keyrings
    put_file "$CLOUDFLARE_KEYRING" 0644 root:root < "$key_tmp/key.gpg"
fi
put_file /etc/apt/sources.list.d/cloudflared.list 0644 root:root <<EOF
deb [signed-by=$CLOUDFLARE_KEYRING] https://pkg.cloudflare.com/cloudflared any main
EOF
apt-get update -qq
apt-get install -y -qq --no-install-recommends cloudflared > /dev/null
log "cloudflared $(cloudflared --version 2>/dev/null | head -n 1 | awk '{ print $3 }')"

# ---- unattended upgrades --------------------------------------------------------------------------

log "configuring unattended-upgrades"
put_file /etc/apt/apt.conf.d/20auto-upgrades 0644 root:root <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT::Periodic::AutocleanInterval "7";
EOF
reboot_conf='Unattended-Upgrade::Automatic-Reboot "false";'
if [ "$auto_reboot" -eq 1 ]; then
    reboot_conf='Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-WithUsers "true";
Unattended-Upgrade::Automatic-Reboot-Time "04:30";'
fi
put_file /etc/apt/apt.conf.d/52todo-unattended-upgrades 0644 root:root <<EOF
// Written by deploy/linux/provision.sh. Debian's security updates are on by default; this adds
// cloudflared, so the tunnel connector stays current too.
Unattended-Upgrade::Origins-Pattern {
        "site=pkg.cloudflare.com";
};
Unattended-Upgrade::Remove-Unused-Kernel-Packages "true";
Unattended-Upgrade::Remove-Unused-Dependencies "true";
$reboot_conf
EOF

# ---- journald -------------------------------------------------------------------------------------

log "limiting the journal"
install -d -m 0755 /etc/systemd/journald.conf.d
put_file /etc/systemd/journald.conf.d/90-todo.conf 0644 root:root <<'EOF'
# Written by deploy/linux/provision.sh: keep the journal small on a 30 GB disk.
[Journal]
Storage=persistent
SystemMaxUse=200M
SystemKeepFree=2G
SystemMaxFileSize=20M
MaxRetentionSec=1month
EOF
if [ "$changed" -eq 1 ] && [ "$have_systemd" -eq 1 ]; then
    systemctl restart systemd-journald
fi

# ---- sysctl and swap ------------------------------------------------------------------------------

put_file /etc/sysctl.d/90-todo.conf 0644 root:root <<'EOF'
# Written by deploy/linux/provision.sh: swap only under real memory pressure.
vm.swappiness=10
EOF
# cloudflared's QUIC library asks for 7 MiB socket buffers and logs "failed to sufficiently increase
# receive buffer size" when the kernel caps it lower (Debian's default is about 200 KiB). These are the
# limits quic-go recommends. They are applied here, before the tunnel starts below: a running connector
# keeps the buffers it got until it restarts.
put_file /etc/sysctl.d/91-todo-quic.conf 0644 root:root <<'EOF'
# Written by deploy/linux/provision.sh: UDP buffers large enough for cloudflared's QUIC connections.
net.core.rmem_max=7500000
net.core.wmem_max=7500000
EOF
quic_sysctl_changed="$changed"
if [ "$in_container" -eq 1 ]; then
    warn "running in a container: skipping swap and sysctl"
else
    sysctl -q -p /etc/sysctl.d/90-todo.conf || warn "could not apply vm.swappiness"
    sysctl -q -p /etc/sysctl.d/91-todo-quic.conf || warn "could not raise the UDP buffer limits"
    if [ "$swap_size" != 0 ]; then
        active_swap="$(swapon --show=NAME --noheadings)"
        if printf '%s\n' "$active_swap" | grep -qx "$SWAP_FILE"; then
            log "swap: $SWAP_FILE is already on"
        else
            log "swap: creating $SWAP_FILE ($swap_size)"
            if [ ! -f "$SWAP_FILE" ]; then
                fallocate -l "$swap_size" "$SWAP_FILE" ||
                    dd if=/dev/zero of="$SWAP_FILE" bs=1M count="$(numfmt --from=iec "$swap_size" | awk '{ print int($1 / 1048576) }')" status=none
                chmod 600 "$SWAP_FILE"
                mkswap -q "$SWAP_FILE" > /dev/null
            fi
            chmod 600 "$SWAP_FILE"
            swapon "$SWAP_FILE"
        fi
        grep -qE "^${SWAP_FILE}[[:space:]]" /etc/fstab || printf '%s none swap sw 0 0\n' "$SWAP_FILE" >> /etc/fstab
    fi
fi

# ---- user and directories -------------------------------------------------------------------------

if ! id "$TODO_USER" > /dev/null 2>&1; then
    log "creating the $TODO_USER system user"
    useradd --system --user-group --home-dir "$DATA_DIR" --no-create-home \
        --shell /usr/sbin/nologin --comment "todo-server" "$TODO_USER"
fi
install -d -m 0700 -o "$TODO_USER" -g "$TODO_USER" "$DATA_DIR"
chown "$TODO_USER:$TODO_USER" "$DATA_DIR"
chmod 0700 "$DATA_DIR"
install -d -m 0750 -o root -g "$TODO_USER" "$CONFIG_DIR"
chown "root:$TODO_USER" "$CONFIG_DIR"
chmod 0750 "$CONFIG_DIR"

# Config files are root:todo 0640: the services read them and can't change them. Secrets that the server
# reads with its owner-only check (`*.secret`, for example the email ingest HMAC secret) must be owned by
# the service account with mode 0600, or the server refuses them.
shopt -s nullglob
for f in "$CONFIG_DIR"/*; do
    if [ ! -f "$f" ] || [ -L "$f" ]; then continue; fi
    case "$f" in
        *.example) want_owner="root:root"; want_mode=644 ;;
        *.secret) want_owner="$TODO_USER:$TODO_USER"; want_mode=600 ;;
        *) want_owner="root:$TODO_USER"; want_mode=640 ;;
    esac
    have="$(stat -c '%a %U:%G' "$f")"
    if [ "$have" != "$want_mode $want_owner" ]; then
        chown "$want_owner" "$f"
        chmod "$want_mode" "$f"
        log "fixed $f: was $have, now $want_mode $want_owner"
    fi
done
shopt -u nullglob
for example in server.toml.example backup.toml.example; do
    [ -f "$script_dir/$example" ] && put_file "$CONFIG_DIR/$example" 0644 root:root < "$script_dir/$example"
done

# ---- the tunnel token -----------------------------------------------------------------------------

# The connector runs as its own system user, which may read the token but not change it.
if ! id "$TUNNEL_USER" > /dev/null 2>&1; then
    log "creating the $TUNNEL_USER system user"
    useradd --system --user-group --home-dir /nonexistent --no-create-home \
        --shell /usr/sbin/nologin --comment "Cloudflare Tunnel connector" "$TUNNEL_USER"
fi
install -d -m 0750 -o root -g "$TUNNEL_USER" "$TOKEN_DIR"
chown "root:$TUNNEL_USER" "$TOKEN_DIR"
chmod 0750 "$TOKEN_DIR"
token_changed=0
if [ -n "$token_src" ]; then
    # Copied byte for byte with install(1); never read into a variable or a command line.
    if [ -f "$TOKEN_FILE" ] && cmp -s "$token_src" "$TOKEN_FILE"; then
        log "tunnel token: unchanged"
    else
        install -m 0640 -o root -g "$TUNNEL_USER" "$token_src" "$TOKEN_FILE"
        token_changed=1
        log "tunnel token: installed at $TOKEN_FILE"
    fi
fi
if [ -f "$TOKEN_FILE" ]; then
    chown "root:$TUNNEL_USER" "$TOKEN_FILE"
    chmod 0640 "$TOKEN_FILE"
fi

# ---- deploy helpers and systemd units -------------------------------------------------------------

# The deploy guard runs before the services start (see deploy-guard.sh). install-release.sh is kept here
# for --recover, which runs after the deploy's upload is gone; each deploy refreshes it.
install -d -m 0755 /usr/local/lib/todo
put_file /usr/local/lib/todo/deploy-guard 0755 root:root < "$script_dir/deploy-guard.sh"
put_file /usr/local/lib/todo/install-release 0755 root:root < "$script_dir/install-release.sh"

log "installing systemd units"
changed_units=()
for u in "${UNITS[@]}"; do
    put_file "$UNIT_DIR/$u" 0644 root:root < "$script_dir/systemd/$u"
    [ "$changed" -eq 1 ] && changed_units+=("$u")
done

if [ "$have_systemd" -eq 0 ]; then
    warn "systemd is not running: units were written but nothing was enabled or started"
    exit 0
fi

systemctl daemon-reload
systemctl enable unattended-upgrades.service > /dev/null 2>&1 || true

# A connector set up earlier with `cloudflared service install` would be a second replica of the tunnel.
if systemctl list-unit-files cloudflared.service > /dev/null 2>&1 &&
    systemctl is-enabled --quiet cloudflared.service 2> /dev/null; then
    warn "cloudflared.service (from 'cloudflared service install') is enabled. This setup uses todo-tunnel.service;"
    warn "disable the other one: sudo cloudflared service uninstall"
fi

# The server and the backup timers are neither enabled nor started here: a deploy starts the server, and
# the runbook enables both once the data is in (docs/hosting-linux.md, "Cutover"). A unit change
# restarts the server if it runs.
for u in "${changed_units[@]}"; do
    case "$u" in
        todo-server.service) systemctl try-restart todo-server.service ;;
    esac
done

if [ -f "$TOKEN_FILE" ]; then
    systemctl enable todo-tunnel.service > /dev/null 2>&1
    tunnel_unit_changed=0
    for u in "${changed_units[@]}"; do [ "$u" = todo-tunnel.service ] && tunnel_unit_changed=1; done
    # The unit is Type=notify: it is active once the connector has registered with Cloudflare. Don't
    # block on that (it never comes without network); wait a while, then report.
    if [ "$token_changed" -eq 1 ] || [ "$tunnel_unit_changed" -eq 1 ]; then
        systemctl restart --no-block todo-tunnel.service
    else
        if [ "$quic_sysctl_changed" -eq 1 ] && systemctl is-active --quiet todo-tunnel.service; then
            # Not restarted here: that takes the hostname offline for about 30 s.
            warn "the UDP buffer limits changed; todo-tunnel uses them after its next restart"
            warn "(sudo systemctl restart todo-tunnel; the hostname is offline for about 30 s)"
        fi
        systemctl start --no-block todo-tunnel.service
    fi
    for _ in $(seq 30); do
        systemctl is-active --quiet todo-tunnel.service && break
        sleep 1
    done
    state="$(systemctl is-active todo-tunnel.service || true)"
    log "tunnel: $state"
    [ "$state" = active ] || warn "the connector isn't connected yet; see: sudo journalctl -u todo-tunnel"
else
    log "tunnel: no token yet; run again with --tunnel-token-file to start the connector"
fi

log "done"
if [ ! -f "$CONFIG_DIR/server.toml" ]; then
    log "next: write $CONFIG_DIR/server.toml (see server.toml.example), then deploy with deploy-linux.ps1"
fi
