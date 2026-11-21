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
| `/opt/todo/releases/<version>-<hash>/` | root, 0755 | One directory per release: `todo-server`, `todo-backup`, `SHA256SUMS`. The current and previous release and three older ones are kept. |
| `/opt/todo/current` | root | A symlink to the running release. A deploy switches it with one atomic rename. The units run the binaries through it. |
| `/usr/local/bin/todo-server`, `todo-backup` | root | Symlinks to `/opt/todo/current/`, for running the tools by hand. |
| `/opt/todo/deploy-in-progress` | root, 0600 | Exists only while a deploy is unfinished. See [An interrupted deploy](#an-interrupted-deploy). |
| `/usr/local/lib/todo/` | root, 0755 | `deploy-guard`, which the units run before they start, and `install-release`, a copy of the deploy script for `--recover`. |
| `/etc/todo/` | root:todo, 0750 | Config and secrets (see [Secrets](#secrets)). |
| `/var/lib/todo/` | todo:todo, 0700 | `todo.sqlite3` with its WAL, `attachments.sqlite3`, `attachments.key`, `blobs/` (local store only), `backup-state/`. |
| `/var/lib/todo-deploy/` | root, 0700 | Database copies taken before each deploy (`pre-<time>/`), and the files a rollback replaced (`failed-<time>/`), the newest three of each. They are plaintext: keep the mode. |
| `/etc/cloudflared/` | root:cloudflared, 0750 | `todo-tunnel.token`. |
| `/swapfile` | root, 0600 | 1 GB of swap. |

| Unit | What it runs |
|---|---|
| `todo-server.service` | `todo-server serve` as `todo`, with `TODO_SERVER_CONFIG=/etc/todo/server.toml`. It stops on SIGINT, so in-flight requests drain. Disabled until the [cutover](#cut-over). |
| `todo-backup@frequent.service`, `todo-backup@daily.service` | One `todo-backup run --kind <kind>` as `todo`, with `TODO_BACKUP_CONFIG=/etc/todo/backup.toml`. Skipped until that file and the binary exist. |
| `todo-backup-frequent.timer` | Every 15 minutes (`:00`, `:15`, `:30`, `:45`). Both timers are disabled until the cutover. |
| `todo-backup-daily.timer` | Once a day at 03:30 UTC, with up to 15 minutes of random delay. A day missed while the VM was off runs at boot. |
| `todo-tunnel.service` | `cloudflared tunnel run --token-file /etc/cloudflared/todo-tunnel.token` as the `cloudflared` user. |

Every service runs sandboxed. The settings include `ProtectSystem=strict` (only `/var/lib/todo` is writable),
`ProtectHome`, `PrivateTmp`, `PrivateDevices`, `NoNewPrivileges`, an empty `CapabilityBoundingSet`, a
system-call filter and `RestrictAddressFamilies`, plus memory caps sized for 1 GB: `MemoryMax=512M` for the
server, 256M for the backup and the connector. `systemd-analyze security` rates each unit about 1.1 out of 10,
which it calls "OK".

The two backup kinds never run at once. Each run takes a `flock` on `/var/lib/todo/backup-run.lock`, so a
daily run that lands on a frequent one waits for it instead of failing on the tool's state lock. A restore
takes the same lock (see [Putting a backup into service](#putting-a-backup-into-service)).

The server and the backups refuse to start while a deploy is unfinished: `deploy-guard` runs first
(`ExecStartPre`) and fails while `/opt/todo/deploy-in-progress` exists. The one exception is the deploy script
itself starting the server.

## Google Cloud: create the VM

Pick one of the free-tier regions (`us-west1`, `us-central1` or `us-east1`) and a zone in it (`<region>` and
`<zone>` below).

The VM gets a network of its own. Leave the project's `default` network alone: don't attach the VM to it and
don't delete it, since other things in the project may rely on it. A new custom-mode network has no firewall
rules, so nothing gets in; one rule then lets SSH in from Google's IAP range only, which is where
`--tunnel-through-iap` connections come from. Networks, subnets and firewall rules cost nothing.

```powershell
$gcloud = "$env:LOCALAPPDATA\google-cloud-sdk\bin\gcloud.cmd"
& $gcloud compute networks create todo-net --project <project-id> --subnet-mode custom
& $gcloud compute networks subnets create todo-subnet --project <project-id> --network todo-net `
    --region <region> --range 10.10.0.0/24
& $gcloud compute firewall-rules create todo-net-allow-ssh-from-iap --project <project-id> --network todo-net `
    --direction INGRESS --action ALLOW --rules tcp:22 --source-ranges 35.235.240.0/20
```

Check that this is the network's only rule:

```powershell
& $gcloud compute firewall-rules list --project <project-id> --filter 'network~/todo-net$'
```

Then create the VM in it:

```powershell
& $gcloud compute instances create <instance> --project <project-id> --zone <zone> `
    --machine-type e2-micro --image-family debian-13 --image-project debian-cloud `
    --boot-disk-size 30GB --boot-disk-type pd-standard `
    --network todo-net --subnet todo-subnet --network-tier STANDARD `
    --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring `
    --metadata enable-oslogin=TRUE --no-service-account --no-scopes
```

- **`--no-service-account`.** The VM never calls Google APIs, so it gets no credentials to leak.
- **External IP.** The VM keeps its ephemeral external IP for outbound traffic: apt, R2 and the tunnel.
  Without one it would need Cloud NAT, which costs money. Outbound traffic is allowed by default; the
  network lets nothing in but SSH from IAP.

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

It installs base packages and adds Cloudflare's apt repository. apt trusts every key in the repository's
keyring, so the keyring holds exactly one key: the download must contain the pinned fingerprint and no other
primary key, and only that key, exported again by fingerprint, is installed. If Cloudflare rotates the key, the
script stops; check the new fingerprint on pkg.cloudflare.com and pass it in `CLOUDFLARE_KEY_FINGERPRINT`.
Then it installs `cloudflared` and sets up:

- **Users.** `todo` and `cloudflared`, both system accounts without a login shell.
- **Directories.** `/var/lib/todo` and `/etc/todo`.
- **Memory.** 1 GB of swap (`--swap-size`) and `vm.swappiness=10`.
- **Upgrades.** Unattended upgrades for Debian security updates and for `cloudflared`. When an update needs
  one, the VM reboots at 04:30 UTC; `--no-auto-reboot` turns that off. Services come back by themselves.
- **The journal.** At most 200 MB, kept for a month.
- **The units.** Installed, with `deploy-guard` and `install-release` in `/usr/local/lib/todo`. The server
  and the backup timers are **not enabled**: a deploy starts the server, and the cutover enables both once the
  real data is in. Until then a reboot leaves the VM quiet, and no backup of rehearsal data can run.

Run it again whenever this directory changes. It only touches what differs, and never enables or disables a
service. It restarts `todo-server` only if its unit changed, and only if it is running.

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

### The build host

The binaries are built in WSL (Ubuntu). Set it up once, by hand; the build script installs nothing but the
pinned toolchain, through rustup:

```sh
sudo apt-get install build-essential curl
# rustup, for your WSL user only. Read https://rustup.rs first, or use your distribution's rustup package.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
```

Open a new WSL shell afterwards, so `~/.cargo/bin` is on `PATH`.

### Deploying

From the Windows PC:

```powershell
./deploy/deploy-linux.ps1 -Project <project-id> -Zone <zone> -Instance <instance>
```

1. **Build.** WSL builds release binaries with the toolchain pinned in `rust-toolchain.toml`, under `nice`.
   The build directory is `~/.cache/to-do-or-not-to-do/target-linux` inside WSL. The output is
   `target\linux-release\`: `todo-server`, `todo-backup` and `SHA256SUMS`. The binaries link against WSL's
   glibc: Ubuntu 22.04 has 2.35, so they need 2.34 or newer, and Debian 13 has 2.41. A first build takes a
   few minutes, later ones less.
2. **Copy.** The binaries and `install-release.sh` go to `~/todo-deploy-<time>/` on the VM. That directory is
   removed again afterwards, whatever happens.
3. **Install.** `sudo install-release.sh` takes over. Nothing is stopped until the first three steps pass:
   1. It checks the checksums and that both binaries run on the VM, and installs them as a new release
      directory, `/opt/todo/releases/<version>-<hash>/`, synced to disk before it gets its name.
   2. It asks the new server which database its config points at (`todo-server database-path`, with the
      unit's working directory), so a relative `data_dir` resolves exactly as the server resolves it. A config
      the new server refuses stops the deploy.
   3. If a release is installed already but there is no database at that path, it stops: the copy in step 5
      would silently be skipped. After a deliberate wipe, pass `-AllowEmptyDatabase`.
   4. It writes the deploy marker, `/opt/todo/deploy-in-progress`, synced to disk. Then it stops the backup
      timers, any backup run and the server, and checks that all of them are inactive and that nothing
      answers `/health`.
   5. It copies the databases with their WAL files to `/var/lib/todo-deploy/pre-<time>/` and syncs them.
   6. It points `/opt/todo/current` at the new release: a new symlink, renamed over the old one.
   7. It starts the server and waits up to 60 s (`-HealthTimeoutSeconds`) until the unit is active and
      `/health` answers 200. The server runs pending migrations before it listens.
   8. It removes the marker and starts the backup timers that were running before. Timers that were off stay
      off.
4. **Roll back.** If the new release doesn't become healthy, the script points `current` back at the previous
   release and starts it. If that doesn't come up either, typically because the new one ran a migration it
   doesn't know, it restores the copy from step 5 and tries once more. Each file is written to a temporary
   name next to the live one and synced, the live files (with their WAL) go to `failed-<time>/`, the copies
   are renamed into place, and the directory is synced. The live database is never overwritten in place.

Exit codes: 0 deployed; 1 nothing changed (a build, copy, checksum or config failure, a missing database, or
the server wouldn't stop); 2 rolled back, the previous release runs; 3 the server is down; 4 deployed, but
`-PublicHealthUrl` didn't answer.

| Option | Effect |
|---|---|
| `-SkipBuild` | Deploys what is already in `-ArtifactDir`. |
| `-NoStart` | Installs the release and leaves the server stopped. |
| `-AllowEmptyDatabase` | Deploys although a release is installed and no database exists, after a deliberate wipe. |
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
sudo /usr/local/lib/todo/install-release --status
```

### An interrupted deploy

A deploy that stops halfway (the VM loses power or reboots, the script is killed, or `deploy-linux.ps1`
reports an unexpected exit code) leaves the marker behind. While it exists:

- `todo-server` refuses to start, at boot and by hand. The journal says why:
  `refusing to start todo-server: a deploy was interrupted (phase '...')`.
- Backup runs refuse to start too, so nothing backs up half-restored databases.

The databases are safe in every phase. The copy is taken while the server is stopped, the release switch is a
single rename, and a restore never writes over the live file. To see where the deploy stopped:

```sh
sudo /usr/local/lib/todo/install-release --status
```

It prints the phase, the new and previous release, the database path, the copy (`snapshot=`) and whether the
new release had already started (`new_ran=1`). Then choose one:

- **Finish.** `sudo /usr/local/lib/todo/install-release --recover finish` carries on from that phase: it
  stops what runs, copies the databases if that hadn't happened yet, switches to the new release and starts
  it. If the new release doesn't become healthy, it rolls back as a normal deploy would.
- **Roll back.** `sudo /usr/local/lib/todo/install-release --recover rollback` points `current` back at the
  previous release and starts it. If the new release had already started and the previous one won't run on
  what it left, the databases are restored from the copy, as above. Anything written while the new release
  ran is then only in `failed-<time>/`.

Both need neither the PC nor the upload: the script keeps a copy of itself in `/usr/local/lib/todo`. Both
exit with the deploy's codes, and on 0 or 2 the marker is gone and the timers run again. Exit code 3 leaves
the marker in place; read `sudo journalctl -u todo-server`, fix the cause, and run `--recover` again. A
recovery that is itself interrupted can be run again.

Don't delete the marker by hand. If you must, stop the server first, and check that `/opt/todo/current` points
at a release that can open the databases.

## Moving from the Windows pilot

The pilot's database, its attachment key and its secrets move to the VM. The tunnel stays the same: the VM
runs a connector with the same token, and the PC's goes away. Clients notice a few minutes of downtime and
nothing else. Passkeys, sessions, device and agent tokens, and share links all keep working, because the
public origin doesn't change. Forwarded emails wait in the Email Worker's retry queue meanwhile.

Plan for 30 minutes. The pilot's data directory is `$data = "$env:LOCALAPPDATA\todo"`.

### Before the day

1. **Get the VM ready.** Create, provision and configure it as above, without `--tunnel-token-file`.
2. **Optional: rehearse.** Deploy once without `-NoStart`, check `curl -fsS http://127.0.0.1:47320/health` on
   the VM, then stop the server and wipe the rehearsal data:

   ```sh
   sudo systemctl stop todo-server
   sudo find /var/lib/todo -mindepth 1 -delete
   ```

   The tunnel isn't running on the VM, so nobody could reach the rehearsal server. The backup timers are
   disabled, so no rehearsal data was backed up.
3. **Deploy without starting.** Run `./deploy/deploy-linux.ps1 ... -NoStart`, adding `-AllowEmptyDatabase` if
   you rehearsed: a release is installed and the database is gone, on purpose.
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

6. **Put everything in place.** On the VM. First check that nothing on the VM runs or would start by itself:

   ```sh
   systemctl is-enabled todo-server todo-backup-frequent.timer todo-backup-daily.timer
   systemctl is-active todo-server todo-backup-frequent.timer todo-backup-daily.timer \
       todo-backup@frequent todo-backup@daily
   ```

   Every line of the first must say `disabled`, and of the second `inactive` (or `failed`). If one doesn't,
   `sudo systemctl disable --now` it and check again: a server or a backup running now would write to or read a
   half-copied database. Then, adjusting the secret names to the ones `server.toml` uses:

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
9. **Enable the server and the backups,** once the checks in [Verify](#verify) up to the manual backup have
   passed:

   ```sh
   sudo systemctl enable todo-server
   sudo systemctl enable --now todo-backup-frequent.timer todo-backup-daily.timer
   systemctl list-timers 'todo-backup-*'
   ```

   From now on the server comes back after a reboot, and backups run on their own. Deploys keep the timers
   running.

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

**After the VM took writes,** bring its data back so they aren't lost. That is both databases (tasks and the
attachment registry), and the attachment key with them. With the R2 attachment store (`store = "r2"`), the
attachments themselves are already in R2 and there are no local blobs to bring; with the local store, the
VM's `blobs/` comes along too.

1. **Stop everything on the VM, and check.**

   ```sh
   sudo systemctl disable --now todo-tunnel todo-backup-frequent.timer todo-backup-daily.timer
   sudo systemctl stop 'todo-backup@*' todo-server
   sudo systemctl disable todo-server
   systemctl is-active todo-tunnel todo-server todo-backup-frequent.timer todo-backup-daily.timer \
       todo-backup@frequent todo-backup@daily
   curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:47320/health
   ```

   Every `is-active` line must say `inactive` (or `failed`), and curl must print `000`. Don't go on until they
   do: a server or a backup still running could change the files while you copy them.
2. **Take consistent copies.** `VACUUM INTO` writes a complete, self-contained copy of each database, WAL
   included; never copy the raw files. As `todo`, so no file in the data directory changes owner:

   ```sh
   sudo -u todo sh -c 'set -e; umask 077; cd /var/lib/todo && mkdir rollback
       for db in todo attachments; do
           sqlite3 "$db.sqlite3" "VACUUM INTO '"'"'rollback/$db.sqlite3'"'"'"
           sqlite3 -readonly "rollback/$db.sqlite3" "PRAGMA integrity_check"
       done
       cp attachments.key rollback/
       sqlite3 -readonly rollback/todo.sqlite3 "select count(*), max(seq) from events"
       cd rollback && sha256sum todo.sqlite3 attachments.sqlite3 attachments.key > SHA256SUMS'
   # Local attachment store only:
   sudo -u todo cp -r /var/lib/todo/blobs /var/lib/todo/rollback/
   ```

   Each `integrity_check` must print `ok`. Note the event count and `max(seq)`. Instead of the `VACUUM INTO`
   copies you can also take a backup (`sudo systemctl start todo-backup@daily`, then `todo-backup restore` on
   the PC as in step 3 of the cutover), but only for `todo.sqlite3`: the attachment registry isn't backed up.
3. **Fetch them.** Move the copies to your home on the VM, fetch them into a folder of yours on the PC
   (relative names again), then remove them from the VM:

   ```sh
   sudo mv /var/lib/todo/rollback ~/rollback && sudo chown -R "$USER": ~/rollback
   ```

   ```powershell
   & $gcloud compute scp @g --recurse <instance>:rollback .
   ```

   ```sh
   find ~/rollback -type f -exec shred -u {} + && rm -rf ~/rollback
   ```

4. **Check them on the PC.** In the folder you fetched to, compare each file with `SHA256SUMS`:

   ```powershell
   Get-Content rollback\SHA256SUMS | ForEach-Object {
       $hash, $name = $_ -split '\s+\*?', 2
       if ((Get-FileHash "rollback\$name" -Algorithm SHA256).Hash -ne $hash) { throw "$name differs" }
   }
   (Get-FileHash rollback\attachments.key).Hash -eq (Get-FileHash "$data\attachments.key").Hash
   ```

   The last line must print `True`: the VM ran with the pilot's attachment key. If it prints `False`, the VM
   made a key of its own, and attachments uploaded on the VM need the VM's key. Stop here and sort that out
   before anything replaces the pilot's key.
5. **Swap them in.** Move the pilot's databases aside, then put the copies in their place:

   ```powershell
   $aside = New-Item -ItemType Directory "$data\before-rollback-$(Get-Date -Format yyyyMMddTHHmmss)"
   Move-Item "$data\todo.sqlite3*", "$data\attachments.sqlite3*" $aside
   Copy-Item rollback\todo.sqlite3, rollback\attachments.sqlite3 $data
   # Local attachment store only:
   Copy-Item -Recurse -Force rollback\blobs\* "$data\blobs\"
   ```

6. **Verify without the tunnel.** Start only the server, check it, then stop it:

   ```powershell
   ./deploy/windows/install-pilot.ps1 -NoRegister -NoTunnel -Start
   Invoke-RestMethod http://127.0.0.1:47320/health   # the pilot's port, if it uses another
   ./deploy/windows/stop-pilot.ps1
   ```

   Check its log in `$data\logs\` for a clean start. If the PC has `sqlite3`, compare the event count and
   `max(seq)` of `$data\todo.sqlite3` with step 2, with the server stopped.
7. **Reconnect the pilot.** `./deploy/windows/install-pilot.ps1 -Start` registers the logon task again and
   starts the server with the tunnel. `./deploy/probe.ps1 -BaseUrl https://todo.example.com` must pass, and
   the Cloudflare dashboard must list only the PC's connector.
8. **Clean up.** Remove `rollback\` once the pilot has run for a while. It holds plaintext data and the
   attachment key.

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

First stop the tunnel, the timers, any backup run that is under way, and the server, and check:

```sh
sudo systemctl stop todo-tunnel todo-backup-frequent.timer todo-backup-daily.timer
sudo systemctl stop 'todo-backup@*' todo-server
systemctl is-active todo-tunnel todo-server todo-backup-frequent.timer todo-backup-daily.timer \
    todo-backup@frequent todo-backup@daily
```

Every line must say `inactive` (or `failed`). Then restore, swap the files and rotate the epoch in one go,
holding the lock that every backup run takes (`/var/lib/todo/backup-run.lock`), so that no run can start in
between, even one started by hand:

```sh
sudo install -d -m 0700 -o todo -g todo /run/todo-drill
sudo -u todo sh -c 'umask 077; cat > /run/todo-drill/identity'   # paste, Ctrl+D
# /var/lib/todo is readable only by todo, so everything runs as todo.
sudo -u todo flock --nonblock /var/lib/todo/backup-run.lock sh -c '
    set -e; umask 077; cd /var/lib/todo
    mkdir restore
    todo-backup --config /etc/todo/backup.toml restore --latest --kind frequent \
        --identity /run/todo-drill/identity --out restore/todo.sqlite3
    old=replaced-$(date -u +%Y%m%dT%H%M%SZ)
    mkdir "$old"
    mv todo.sqlite3* "$old"/
    mv restore/todo.sqlite3 .
    rmdir restore
    todo-server --config /etc/todo/server.toml rotate-log-epoch
    echo restored'
sudo rm -rf /run/todo-drill
```

It must end with `restored`. If `flock` says the lock is held, a backup run is still going: wait for it (`sudo
journalctl -u 'todo-backup@*' -f`) and run the block again. If the restore itself fails, nothing has been moved
yet; remove `/var/lib/todo/restore` and start over. Then start everything again:

```sh
sudo systemctl start todo-server todo-tunnel todo-backup-frequent.timer todo-backup-daily.timer
```

`rotate-log-epoch` makes clients that followed the replaced log resync from scratch. They keep their unsent
changes. The paths assume `data_dir = "/var/lib/todo"`. To see where the database really is:

```sh
sudo -u todo sh -c 'cd /var/lib/todo && todo-server --config /etc/todo/server.toml database-path'
```

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
