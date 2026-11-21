# Hosting on Linux

This runbook moves `todo-server` from the Windows pilot (`deploy/windows/`) to a small Debian VM. The scripts
work on any Debian 13 x86_64 VM. Only the sections marked **Google Cloud** are specific to the free e2-micro
it was written for (1 GB of RAM, a 30 GB standard disk).

- **No inbound ports.** Clients reach the server only through the existing Cloudflare Tunnel. The `cloudflared`
  connector on the VM dials out to Cloudflare, and the server listens on `127.0.0.1`.
- **Admin access** is SSH through the provider's tunnel (IAP on Google Cloud), never a public port 22.
- **Lean traffic.** Backups upload only when something changed, and nothing polls.

Every name below is a placeholder: `<project-id>`, `<zone>`, `<instance>`, `todo.example.com`.

| Script | Runs on | What it does |
|---|---|---|
| `deploy/linux/provision.sh` | the VM, as root | Users, directories, swap, unattended upgrades, journald limits, `cloudflared`, the systemd units. Idempotent. |
| `deploy/deploy-linux.ps1` | the Windows PC | Builds the Linux binaries in WSL, copies them over, installs them on the VM, checks health, rolls back. |
| `deploy/linux/install-release.sh` | the VM, as root | The VM half of a deploy. `deploy-linux.ps1` uploads and runs it. |
| `deploy/linux/build-release.sh` | WSL or any Linux | Builds `todo-server` and `todo-backup` for `x86_64-unknown-linux-gnu`. |
| `deploy/linux/server.toml.example`, `backup.toml.example` | | Config templates, placeholders only. |
| `deploy/probe.ps1`, `deploy/probe.sh` | anywhere | Probe the deployment from outside (see `deploy/windows/README.md`). |

## Layout on the VM

| Path | Owner, mode | Contents |
|---|---|---|
| `/usr/local/bin/todo-server`, `todo-backup` | root, 0755 | The binaries. The previous release stays next to them as `*.prev`. |
| `/etc/todo/` | root:todo, 0750 | Config and secrets (see [Secrets](#secrets)). |
| `/var/lib/todo/` | todo:todo, 0700 | `todo.sqlite3` with its WAL, `attachments.sqlite3`, `attachments.key`, `blobs/` (local store only), `backup-state/`. |
| `/var/lib/todo-deploy/` | root, 0700 | Database copies taken before each deploy (`pre-<time>/`, the newest three), and the files a rollback replaced (`failed-<time>/`). They are plaintext: keep the mode. |
| `/etc/cloudflared/` | root:cloudflared, 0750 | `todo-tunnel.token`. |
| `/swapfile` | root, 0600 | 1 GB of swap. |

| Unit | What it runs |
|---|---|
| `todo-server.service` | `todo-server serve` as `todo`, with `TODO_SERVER_CONFIG=/etc/todo/server.toml`. It stops on SIGINT, so in-flight requests drain. |
| `todo-backup@frequent.service`, `todo-backup@daily.service` | One `todo-backup run --kind <kind>` as `todo`, with `TODO_BACKUP_CONFIG=/etc/todo/backup.toml`. Skipped until that file and the binary exist. |
| `todo-backup-frequent.timer` | Every 15 minutes (`:00`, `:15`, `:30`, `:45`). |
| `todo-backup-daily.timer` | Once a day at 03:30 UTC, with up to 15 minutes of random delay. A day missed while the VM was off runs at boot. |
| `todo-tunnel.service` | `cloudflared tunnel run --token-file /etc/cloudflared/todo-tunnel.token` as the `cloudflared` user. |

Every service runs sandboxed. The settings include `ProtectSystem=strict` (only `/var/lib/todo` is writable),
`ProtectHome`, `PrivateTmp`, `PrivateDevices`, `NoNewPrivileges`, an empty `CapabilityBoundingSet`, a
system-call filter and `RestrictAddressFamilies`, plus memory caps sized for 1 GB: `MemoryMax=512M` for the
server, 256M for the backup and the connector. `systemd-analyze security` rates each unit about 1.1 out of 10,
which it calls "OK".

The two backup kinds never run at once. Each run takes a `flock` on `/var/lib/todo/backup-run.lock`, so a
daily run that lands on a frequent one waits for it instead of failing on the tool's state lock.

## Google Cloud: create the VM

Pick one of the free-tier regions (`us-west1`, `us-central1` or `us-east1`) and a zone in it.

```powershell
$gcloud = "$env:LOCALAPPDATA\google-cloud-sdk\bin\gcloud.cmd"
& $gcloud compute instances create <instance> --project <project-id> --zone <zone> `
    --machine-type e2-micro --image-family debian-13 --image-project debian-cloud `
    --boot-disk-size 30GB --boot-disk-type pd-standard --network-tier STANDARD `
    --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring `
    --metadata enable-oslogin=TRUE --no-service-account --no-scopes
```

- **`--no-service-account`.** The VM never calls Google APIs, so it gets no credentials to leak.
- **External IP.** The VM keeps its ephemeral external IP for outbound traffic: apt, R2 and the tunnel.
  Without one it would need Cloud NAT, which costs money. The firewall rules below let nothing in.

Close every inbound port. Allow SSH only from Google's IAP range, which is where `--tunnel-through-iap`
connections come from:

```powershell
& $gcloud compute firewall-rules create allow-ssh-from-iap --project <project-id> --network default `
    --direction INGRESS --allow tcp:22 --source-ranges 35.235.240.0/20
& $gcloud compute firewall-rules delete default-allow-ssh default-allow-rdp default-allow-icmp --project <project-id>
```

`gcloud compute ssh <instance> --project <project-id> --zone <zone> --tunnel-through-iap` now works, and
nothing else reaches the VM. Your account needs the IAP-secured Tunnel User role, which project owners have.

## Provision

Copy `deploy/linux` to the VM and run the script. From the repository root (keep the path relative: gcloud and
scp read `C:\...` as a host named `C`):

```powershell
$g = @('--project', '<project-id>', '--zone', '<zone>', '--tunnel-through-iap')
& $gcloud compute scp @g --recurse deploy/linux <instance>:
& $gcloud compute ssh <instance> @g --command 'sudo bash linux/provision.sh'
```

It installs base packages and adds Cloudflare's apt repository. The signing key is checked against a pinned
fingerprint; if Cloudflare rotates it, the script stops and says how to pass the new one. Then it installs
`cloudflared` and sets up:

- **Users.** `todo` and `cloudflared`, both system accounts without a login shell.
- **Directories.** `/var/lib/todo` and `/etc/todo`.
- **Memory.** 1 GB of swap (`--swap-size`) and `vm.swappiness=10`.
- **Upgrades.** Unattended upgrades for Debian security updates and for `cloudflared`. When an update needs
  one, the VM reboots at 04:30 UTC; `--no-auto-reboot` turns that off. Services come back by themselves.
- **The journal.** At most 200 MB, kept for a month.
- **The units.** Installed and enabled. The server isn't started: a deploy does that.

Run it again whenever this directory changes. It only touches what differs. It restarts `todo-server` only if
its unit changed, and only if it is running.

Without `--tunnel-token-file`, the tunnel connector is installed but not started. That is the right state
until the [move](#moving-from-the-windows-pilot): a connector on the VM joins the same tunnel as the pilot's
and gets a share of the traffic straight away.

## Configure

```sh
sudo install -m 0640 -o root -g todo linux/server.toml.example /etc/todo/server.toml
sudoedit /etc/todo/server.toml
```

Keep `bind` on `127.0.0.1` and the port the tunnel's public hostname points at (`http://127.0.0.1:47320` by
default), `data_dir = "/var/lib/todo"`, the same `public_origin` as the pilot, and
`trust_cloudflare_headers = true`. Put each secret in place with the owner and mode from [Secrets](#secrets),
for example:

```sh
sudo install -m 0640 -o root -g todo r2-attachments.json /etc/todo/
sudo install -m 0600 -o todo -g todo ingest-email-hmac.secret /etc/todo/
shred -u r2-attachments.json ingest-email-hmac.secret
```

`provision.sh` also corrects the modes in `/etc/todo` when you run it: `*.secret` files become todo:todo 0600,
everything else root:todo 0640.

## Deploy

From the Windows PC:

```powershell
./deploy/deploy-linux.ps1 -Project <project-id> -Zone <zone> -Instance <instance>
```

1. **Build.** WSL (Ubuntu) builds release binaries with the toolchain pinned in `rust-toolchain.toml`. If
   rustup is missing in WSL, the script installs it for your WSL user, with the pinned toolchain. The build
   directory is `~/.cache/to-do-or-not-to-do/target-linux` inside WSL. The output is `target\linux-release\`:
   `todo-server`, `todo-backup` and `SHA256SUMS`. The binaries link against WSL's glibc: Ubuntu 22.04 has
   2.35, so they need 2.34 or newer, and Debian 13 has 2.41. A first build takes a few minutes, later ones
   less.
2. **Copy.** The binaries and `install-release.sh` go to `~/todo-deploy-<time>/` on the VM. That directory is
   removed again afterwards, whatever happens.
3. **Install.** `sudo install-release.sh` takes over:
   1. It checks the checksums, and that both binaries run on the VM.
   2. It stops the backup timers and the server.
   3. It copies the databases to `/var/lib/todo-deploy/pre-<time>/`.
   4. It swaps each binary with an atomic rename, keeping the old one as `*.prev`.
   5. It starts the server and waits up to 60 s (`-HealthTimeoutSeconds`) for `/health` on the VM. The server
      runs pending migrations before it listens.
4. **Roll back.** If `/health` doesn't answer, the script puts the `.prev` binaries back. If the old release
   doesn't come up either, typically because the new one ran a migration it doesn't know, it moves the
   databases to `failed-<time>/`, restores the copy and tries once more. The backup timers are started again
   in every case.

Exit codes: 0 deployed; 1 nothing changed (a build, copy or checksum failure, or the server wouldn't stop);
2 rolled back, the previous release runs; 3 the server is down; 4 deployed, but `-PublicHealthUrl` didn't
answer.

| Option | Effect |
|---|---|
| `-SkipBuild` | Deploys what is already in `-ArtifactDir`. |
| `-NoStart` | Installs the binaries and leaves the server stopped. |
| `-PublicHealthUrl https://todo.example.com/health` | Also checks the server from outside after the deploy. |
| `-GcloudPath` | Another gcloud. By default the script uses the portable install in `%LOCALAPPDATA%\google-cloud-sdk`, then `PATH`. |
| `-Transport ssh -SshTarget user@host` | Uses plain OpenSSH `ssh` and `scp` instead of gcloud, for any other VM. `-SshOption 'IdentityFile=...'` adds `-o` options. |
| `-WslDistro` | Builds in another WSL distribution. |

The first time `todo-server` starts on an empty database, it writes a one-time setup code to
`/var/lib/todo/setup-code.txt` (`sudo cat` it). That doesn't happen in the move below, which brings the
existing database and its passkeys along.

**Logs and status:**

```sh
systemctl status todo-server todo-tunnel
sudo journalctl -u todo-server -f
systemctl list-timers 'todo-backup-*'
sudo journalctl -u 'todo-backup@*' --since today
```

## Moving from the Windows pilot

The pilot's database, its attachment key and its secrets move to the VM. The tunnel stays the same: the VM
runs a connector with the same token, and the PC's goes away. Clients notice a few minutes of downtime and
nothing else. Passkeys, sessions, device and agent tokens, and share links all keep working, because the
public origin doesn't change. Forwarded emails wait in the Email Worker's retry queue meanwhile.

Plan for 30 minutes. The pilot's data directory is `$data = "$env:LOCALAPPDATA\todo"`.

### Before the day

1. **Get the VM ready.** Create, provision and configure it as above, without `--tunnel-token-file`.
2. **Deploy without starting.** Run `./deploy/deploy-linux.ps1 ... -NoStart`.
3. **Optional: rehearse.** Deploy without `-NoStart` once, check `curl -fsS http://127.0.0.1:47320/health` on
   the VM, then wipe the rehearsal data:

   ```sh
   sudo systemctl stop todo-server
   sudo find /var/lib/todo -mindepth 1 -delete
   ```

   The tunnel isn't running on the VM, so nobody could reach the rehearsal server.
4. **Port.** If the pilot's `server.toml` uses another port than 47320, use that port on the VM too, so the
   tunnel's public hostname still matches.

### Cut over

1. **Stop the pilot for good.** On the PC:

   ```powershell
   ./deploy/windows/uninstall-pilot.ps1
   ```

   This stops the tunnel and then the server, and removes the logon task. Without that last step, the next
   logon would start a second server writing to the old database. Data, logs and secrets stay.
2. **Take the final snapshot.** The server is stopped, so this is the complete log:

   ```powershell
   $env:TODO_BACKUP_CONFIG = "$data\backup.toml"
   & "$data\bin\todo-backup.exe" run --kind daily --force
   ```

   Note the uploaded key it prints (`daily/<time>-<seq>.sqlite3.zst.age`).
3. **Restore it on the PC.** Paste the backup identity from the password manager into a temporary file inside
   `$data`; that directory's ACL is yours only. Then:

   ```powershell
   New-Item -ItemType Directory "$data\move" | Out-Null
   & "$data\bin\todo-backup.exe" restore --object <key> --identity "$data\identity.tmp" --out "$data\move\todo.sqlite3"
   Remove-Item "$data\identity.tmp"
   ```

   The restore checks integrity and prints `max(seq)` and the event count. Keep them for step 7. This also
   proves that the last backup restores. Without R2 you can copy `todo.sqlite3` and its `-wal` file instead,
   both together: the server is stopped, so they are consistent.
4. **Collect the rest in `$data\move`:**
   - `attachments.key`, and `attachments.sqlite3` with its `-wal` if there is one. Without the key no
     attachment can be decrypted.
   - `blobs\`, if the pilot uses the local attachment store.
   - The secret files that the pilot's `server.toml` and `backup.toml` point to: the R2 credentials for
     attachments and for backups, and the ingest HMAC secret.
   - `cloudflared-token`.
   - Optionally `backup-state\budget.json`, which carries this month's operation count over.
5. **Copy and remove.** From inside `$data\move`, with relative names:

   ```powershell
   Push-Location "$data\move"
   & $gcloud compute ssh <instance> @g --command 'mkdir -m 700 move'
   & $gcloud compute scp @g --recurse * <instance>:move/
   Pop-Location
   Remove-Item -Recurse "$data\move"
   ```

6. **Put everything in place.** On the VM. Adjust the secret names to the ones `server.toml` uses:

   ```sh
   cd ~/move
   sudo install -m 0600 -o todo -g todo todo.sqlite3 attachments.key attachments.sqlite3* /var/lib/todo/
   # Local attachment store only:
   sudo cp -r blobs /var/lib/todo/ && sudo chown -R todo:todo /var/lib/todo/blobs && sudo chmod -R go= /var/lib/todo/blobs
   sudo install -m 0640 -o root -g todo r2-attachments.json r2-backups.json /etc/todo/
   sudo install -m 0600 -o todo -g todo ingest-email-hmac.secret /etc/todo/
   sudo install -m 0600 -o root -g root cloudflared-token /root/todo-tunnel.token
   # Optional: keep this month's backup operation count.
   sudo install -d -m 0700 -o todo -g todo /var/lib/todo/backup-state
   sudo install -m 0600 -o todo -g todo budget.json /var/lib/todo/backup-state/
   cd && shred -u move/* 2> /dev/null; rm -rf move
   ```

   Write `/etc/todo/backup.toml` from `backup.toml.example`, with the pilot's `recipient`.
7. **Start the server, but not the tunnel yet.**

   ```sh
   sudo -u todo sqlite3 -readonly /var/lib/todo/todo.sqlite3 'select count(*), max(seq) from events'
   sudo systemctl start todo-server
   curl -fsS http://127.0.0.1:47320/health && echo ok
   sudo journalctl -u todo-server -n 20 --no-pager
   ```

   The count and `max(seq)` must match step 3. Run `sqlite3` as `todo`: as root it could create WAL files that
   the server can't open. Rotating the log epoch isn't needed, because this is the complete log.
8. **Point the tunnel at the VM.**

   ```sh
   sudo bash linux/provision.sh --tunnel-token-file /root/todo-tunnel.token
   sudo shred -u /root/todo-tunnel.token
   ```

   In the Cloudflare dashboard, the tunnel now lists one connector, the VM's. The PC's went away in step 1;
   if it is still listed, stop `cloudflared` on the PC.

### Verify

- `./deploy/probe.ps1 -BaseUrl https://todo.example.com` passes, from any machine.
- Sign in with a passkey in the browser. The desktop app syncs, and a change made on one device shows up on
  another.
- An agent's MCP call works.
- A forwarded email becomes a task within a minute.
- Back up once by hand:

  ```sh
  sudo systemctl start todo-backup@daily
  sudo journalctl -u todo-backup@daily -n 5 --no-pager
  sudo -u todo todo-backup --config /etc/todo/backup.toml status
  ```

- After a day: `systemctl list-timers 'todo-backup-*'` shows recent runs, and `systemctl --failed` is empty.
- After a week of quiet, delete the pilot's data directory. Keep the final backup.

### Roll back

**Before the VM took any writes,** for example because step 7 or 8 failed:

```sh
sudo systemctl disable --now todo-tunnel
sudo systemctl stop todo-server
```

```powershell
./deploy/windows/install-pilot.ps1 -Start
```

The pilot's database is exactly as it was when you stopped it.

**After the VM took writes,** bring its database back so they aren't lost:

1. **Stop the VM side.** Disable and stop `todo-tunnel`, then stop `todo-server`. Its clean stop checkpoints
   the WAL.
2. **Copy the database out.** Copy `/var/lib/todo/todo.sqlite3` to your home with `sudo install -m 0600 -o
   $USER`, then fetch it with `gcloud compute scp <instance>:todo.sqlite3 .`, and shred the copy on the VM.
3. **Swap it in on the PC.** Move the pilot's `todo.sqlite3*` files aside and put this one in their place.
4. **Start the pilot.** Run `install-pilot.ps1 -Start`.

If you start the pilot on its old database anyway, run `todo-server.exe rotate-log-epoch` on it first. Clients
that saw the VM's newer events then resync instead of mixing the two histories.

## Backups on Linux

Backups work as on the pilot: a `VACUUM INTO` snapshot, an integrity check, zstd, then age to the public
recipient, then R2. See `deploy/windows/README.md` for the format, the budget and the bucket's lifecycle
rules.

The timers run `todo-backup run --kind frequent` every 15 minutes and `--kind daily` once a day, never with
`--force`. A run uploads nothing when `max(seq)` and the snapshot's SHA-256 are unchanged since the last
upload of its kind, unless that upload is older than the `[dedupe]` limit. An idle database therefore costs no
requests and no egress.

A run that hits the monthly operations budget exits with 3 and shows as failed in `systemctl --failed`.

### Restore drill

Run one once a month. The identity never stays on the VM: it lives in RAM (`/run`) for the length of the
drill.

```sh
sudo install -d -m 0700 -o todo -g todo /run/todo-drill
sudo -u todo sh -c 'umask 077; cat > /run/todo-drill/identity'   # paste the identity, then Ctrl+D
sudo -u todo todo-backup --config /etc/todo/backup.toml drill --identity /run/todo-drill/identity
sudo rm -rf /run/todo-drill
```

The drill restores the newest backup into `backup-state/scratch` and checks:

- `integrity_check`;
- the SHA-256 against the upload record;
- the event count and `max(seq)`.

It exits with 0 if they all match, 4 on a mismatch, and 1 if the backup couldn't be fetched or decrypted.

### Putting a backup into service

```sh
sudo systemctl stop todo-tunnel todo-backup-frequent.timer todo-backup-daily.timer todo-server
sudo install -d -m 0700 -o todo -g todo /var/lib/todo/restore /run/todo-drill
sudo -u todo sh -c 'umask 077; cat > /run/todo-drill/identity'   # paste, Ctrl+D
sudo -u todo todo-backup --config /etc/todo/backup.toml restore --latest --kind frequent \
    --identity /run/todo-drill/identity --out /var/lib/todo/restore/todo.sqlite3
sudo rm -rf /run/todo-drill
# /var/lib/todo is readable only by todo, so the moves run as todo.
sudo -u todo sh -c 'umask 077; cd /var/lib/todo && old=replaced-$(date -u +%Y%m%dT%H%M%SZ) && mkdir "$old" &&
    mv todo.sqlite3* "$old"/ && mv restore/todo.sqlite3 . && rmdir restore'
sudo -u todo todo-server --config /etc/todo/server.toml rotate-log-epoch
sudo systemctl start todo-server todo-tunnel todo-backup-frequent.timer todo-backup-daily.timer
```

`rotate-log-epoch` makes clients that followed the replaced log resync from scratch. They keep their unsent
changes.

## The admin API

The admin listener stays on `127.0.0.1:47321`. Reach it through an SSH port forward over IAP:

```powershell
& $gcloud compute ssh <instance> @g -- -N -L 47321:127.0.0.1:47321
```

Then call `http://127.0.0.1:47321/api/v1/admin/...` on the PC (see `admin-api.md`).

- **Make the token on the PC.** Run `todo-server.exe admin-token --out <file>` there, so the token never
  lives on the VM, and put only the printed `token_sha256` line in `/etc/todo/server.toml`.
- **Never route the tunnel to this port.**

## Secrets

Only paths and modes are listed here; no value belongs in this repository.

| Secret | Where | Owner, mode | Read by |
|---|---|---|---|
| Server config (holds the admin token's hash) | `/etc/todo/server.toml` | root:todo 0640 | `todo-server` |
| Backup config (holds only the public recipient) | `/etc/todo/backup.toml` | root:todo 0640 | `todo-backup` |
| R2 credentials for attachments | `/etc/todo/r2-attachments.json` | root:todo 0640 | `todo-server` |
| R2 credentials for backups | `/etc/todo/r2-backups.json` | root:todo 0640 | `todo-backup` |
| Email ingest HMAC secret | `/etc/todo/ingest-email-hmac.secret` | todo:todo 0600 | `todo-server`, which refuses it with any other owner or mode |
| Attachment master key | `/var/lib/todo/attachments.key` | todo:todo 0600 | `todo-server`, which creates it on the first start. Keep a copy in the password manager. |
| Setup code (first start only) | `/var/lib/todo/setup-code.txt` | todo:todo 0600 | you, once; the server removes it after setup |
| Tunnel token | `/etc/cloudflared/todo-tunnel.token` | root:cloudflared 0640 | `todo-tunnel`. The file you pass to `provision.sh` must be root-only. |
| Admin token | not on the VM | your PC, owner-only | you |
| Backup identity (age secret key) | not on the VM | the password manager; `/run/todo-drill/identity` during a drill | `todo-backup restore` and `drill` |
| Pre-deploy database copies | `/var/lib/todo-deploy/` | root 0700 | rollbacks |

Neither the tunnel token nor any other secret appears on a command line, in the environment or in a unit file.
`provision.sh` copies the token file byte for byte and never prints it.

## Cost (Google Cloud)

The always-free tier covers:

- **The VM.** One e2-micro in `us-west1`, `us-central1` or `us-east1`, for every hour of the month. One VM
  only: a second e2-micro shares the same free hours.
- **Disk.** 30 GB-months of standard persistent disk. Keep `pd-standard` at 30 GB; balanced or SSD disks
  aren't free.
- **Egress.** 1 GB a month of outbound traffic from North America, on the Premium tier. The VM above uses the
  **Standard network tier** (`--network-tier STANDARD`), which comes with its own free monthly allowance of
  internet egress: 200 GB at the time of writing. Check the current terms on the Network Service Tiers
  pricing page.

Things that can cost money, so check them:

- **An in-use external IPv4 address.** Google has charged for these; whether the free tier covers the one on a
  free e2-micro is worth checking on the VPC pricing page for your account.
- **Snapshots, a second disk, Cloud NAT or a static IP.** None of them is needed here.

All client traffic leaves the VM as egress to Cloudflare. A month of normal use stays far below the free
allowance:

- an SSE heartbeat every 25 s on each open stream, a few MB a month per stream;
- sync and API calls of a few KB each;
- uploads to R2 only when the database changed.

Attachment downloads are the one thing that can add up: the server decrypts each one and streams it out
through the tunnel. Downloads from R2 to the VM are ingress, which is free, and R2 itself charges no egress.

**Set a budget alert** so any charge is noticed at once:

```powershell
& $gcloud billing budgets create --billing-account <billing-account-id> --display-name todo-vm `
    --budget-amount 1USD --threshold-rule percent=0.01 --threshold-rule percent=0.5 --threshold-rule percent=1.0
```

Or use Billing → Budgets & alerts in the console. The alert emails the billing admins. It doesn't stop
anything.
