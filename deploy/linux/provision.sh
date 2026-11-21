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
#   - unattended-upgrades (security updates, cloudflared, and a reboot at 04:30 UTC when one is needed);
#   - journald size limits;
#   - cloudflared from Cloudflare's apt repository, and the todo-tunnel service once a token is given
#     (the file must be root-only; it is installed as /etc/cloudflared/todo-tunnel.token, root:cloudflared
#     0640);
#   - the systemd units: todo-server, todo-backup@ with its two timers, todo-tunnel.
#
# It never starts todo-server for the first time (a deploy does) but restarts it after a unit change if
# it runs. It never prints the tunnel token. Nothing here is provider-specific and nothing opens an
# inbound port. See docs/hosting-linux.md.
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

token_src=""
swap_size=1G
auto_reboot=1

usage() {
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
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
    [ -f "$token_src" ] && [ ! -L "$token_src" ] || die "$token_src is not a regular file"
    [ "$(stat -c %u "$token_src")" -eq 0 ] || die "$token_src must be owned by root (sudo chown root: $token_src)"
    mode="$(stat -c %a "$token_src")"
    [ $((8#$mode & 8#077)) -eq 0 ] || die "$token_src has mode $mode; make it readable by root only (chmod 600)"
    size="$(stat -c %s "$token_src")"
    [ "$size" -ge 32 ] && [ "$size" -le 4096 ] || die "$token_src doesn't look like a tunnel token ($size bytes)"
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
# Prints the fingerprints of the keys in a keyring file, one per line, and nothing if it isn't one.
key_fingerprints() {
    gpg --homedir "$key_tmp" --batch --quiet --with-colons --show-keys "$1" 2> /dev/null |
        awk -F: '$1 == "fpr" { print $10 }' || true
}
# True if the fingerprint list in $1 holds the pinned one.
has_pinned_key() {
    case "$(printf '\n%s\n' "$1")" in *"
$CLOUDFLARE_KEY_FINGERPRINT
"*) return 0 ;; esac
    return 1
}
installed="$(if [ -f "$CLOUDFLARE_KEYRING" ]; then key_fingerprints "$CLOUDFLARE_KEYRING"; fi)"
if has_pinned_key "$installed"; then
    log "Cloudflare's apt key is already installed"
else
    log "adding Cloudflare's apt key"
    curl -fsSL --proto '=https' --tlsv1.2 -o "$key_tmp/key" "$CLOUDFLARE_KEY_URL"
    # The file may be armored or binary; dearmor only if needed.
    if grep -q 'BEGIN PGP PUBLIC KEY BLOCK' "$key_tmp/key"; then
        gpg --homedir "$key_tmp" --batch --quiet --dearmor -o "$key_tmp/key.gpg" "$key_tmp/key"
    else
        cp "$key_tmp/key" "$key_tmp/key.gpg"
    fi
    fingerprints="$(key_fingerprints "$key_tmp/key.gpg")"
    if ! has_pinned_key "$fingerprints"; then
        die "the key at $CLOUDFLARE_KEY_URL has fingerprint(s) [$(printf '%s' "$fingerprints" | tr '\n' ' ')], but $CLOUDFLARE_KEY_FINGERPRINT was expected"
    fi
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

# ---- swap -----------------------------------------------------------------------------------------

put_file /etc/sysctl.d/90-todo.conf 0644 root:root <<'EOF'
# Written by deploy/linux/provision.sh: swap only under real memory pressure.
vm.swappiness=10
EOF
if [ "$in_container" -eq 1 ]; then
    warn "running in a container: skipping swap and sysctl"
else
    sysctl -q -p /etc/sysctl.d/90-todo.conf || warn "could not apply vm.swappiness"
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
    [ -f "$f" ] && [ ! -L "$f" ] || continue
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
    useradd --system --user-group --home-dir /nonexistent --no-create-home         --shell /usr/sbin/nologin --comment "Cloudflare Tunnel connector" "$TUNNEL_USER"
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

# ---- systemd units --------------------------------------------------------------------------------

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

# The server is enabled for boot but only started by a deploy. A unit change restarts it if it runs.
systemctl enable todo-server.service > /dev/null 2>&1
for u in "${changed_units[@]}"; do
    case "$u" in
        todo-server.service) systemctl try-restart todo-server.service ;;
    esac
done

# Timers run from the start; their service does nothing until /etc/todo/backup.toml and the binary exist.
systemctl enable --now todo-backup-frequent.timer todo-backup-daily.timer > /dev/null 2>&1

if [ -f "$TOKEN_FILE" ]; then
    systemctl enable todo-tunnel.service > /dev/null 2>&1
    tunnel_unit_changed=0
    for u in "${changed_units[@]}"; do [ "$u" = todo-tunnel.service ] && tunnel_unit_changed=1; done
    # The unit is Type=notify: it is active once the connector has registered with Cloudflare. Don't
    # block on that (it never comes without network); wait a while, then report.
    if [ "$token_changed" -eq 1 ] || [ "$tunnel_unit_changed" -eq 1 ]; then
        systemctl restart --no-block todo-tunnel.service
    else
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
