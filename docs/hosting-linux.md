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
| `deploy/linux/provision.sh` | the VM, as root | Users, directories, swap, UDP buffer limits, unattended upgrades, journald limits, `cloudflared`, the systemd units. Idempotent. |
| `deploy/deploy-linux.ps1` | the Windows PC | Builds the Linux binaries and web app in WSL, deploys them to the VM, checks health, and stops a release that fails it. |
| `deploy/linux/install-release.sh` | the VM, as root | The VM half of a deploy. `deploy-linux.ps1` uploads and runs it. |
| `deploy/linux/build-release.sh` | WSL or any Linux | Builds the `x86_64-unknown-linux-gnu` binaries and the `wasm32-unknown-unknown` web app into one release. |
| `deploy/linux/server.toml.example`, `backup.toml.example` | | Config templates, placeholders only. |
| `deploy/probe.ps1`, `deploy/probe.sh` | anywhere | Probe the deployment from outside (see `deploy/windows/README.md`). |

## Layout on the VM

| Path | Owner, mode | Contents |
|---|---|---|
| `/opt/todo/releases/<version>-<hash>/` | root, 0755 | One directory per release: `todo-server`, `todo-backup`, `web/`, `SHA256SUMS`. The hash in the name covers both binaries and the web files. The current and previous release and three older ones are kept. |
| `/opt/todo/current` | root | A symlink to the running release. A deploy switches it with one atomic rename. The units run the binaries through it and the server reads `/opt/todo/current/web` through it. |
| `/opt/todo/desktop-releases/` | root, 0755 | Optional legacy Windows desktop feed for the [GitHub Releases transition](desktop.md#transition-from-the-authenticated-server-feed). |
| `/usr/local/bin/todo-server`, `todo-backup` | root | Symlinks to `/opt/todo/current/`, for running the tools by hand. |
| `/opt/todo/deploy-in-progress` | root, 0600 | Exists only while a deploy is unfinished. See [An interrupted deploy](#an-interrupted-deploy). |
| `/opt/todo/deploy.lock` | root | Held by the deploy or recovery that is running, so only one runs at a time. |
| `/run/todo-deploy/allow-start` | root, 0600 | Exists only for a moment: the deploy's permission for one start of the server. In RAM, gone after a reboot. |
| `/usr/local/lib/todo/` | root, 0755 | `deploy-guard`, which the units run before they start, and `install-release`, a copy of the deploy script for `--recover`. |
| `/etc/todo/` | root:todo, 0750 | Config and secrets (see [Secrets](#secrets)). |
| `/var/lib/todo/` | todo:todo, 0700 | `todo.sqlite3` with its WAL, `attachments.sqlite3`, `attachments.key`, `blobs/` (local store only), `backup-state/`. |
| `/var/lib/todo-deploy/` | root, 0700 | Database copies taken before each deploy (`pre-<time>/`), and the files a rollback replaced (`failed-<time>/`), the newest three of each. They are plaintext: keep the mode. A deploy with no database yet leaves a `pre-<time>/` with only an empty `MANIFEST`: that records, for a rollback, that there was nothing to copy. |
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
takes the same lock (see [Putting a backup into service](#putting-a-backup-into-service)), and so does a
deploy before it copies or replaces a database.

The server and the backups refuse to start while a deploy is unfinished: `deploy-guard` runs first
(`ExecStartPre`) and fails while `/opt/todo/deploy-in-progress` exists. The one exception is a start of the
server that the deploy script authorizes. Each authorization is a random nonce, written both to the marker and
to `/run/todo-deploy/allow-start` (root, 0600, in RAM, so it can't outlive a reboot). The guard allows a start
only if the two match and it has deleted the file (if the file can't be removed, the start is refused), so
each nonce allows at most one start. A backup is never let through.

## Google Cloud: create the VM

Pick one of the free-tier regions (`us-west1`, `us-central1` or `us-east1`) and a zone in it. Every PowerShell
block in this runbook uses these variables, so set them once in the session, replacing the placeholders:

```powershell
$projectId = '<project-id>'
$region = '<region>'      # for example us-west1
$zone = '<zone>'          # a zone in that region, for example us-west1-b
$instance = '<instance>'  # the VM's name
$gcloud = "$env:LOCALAPPDATA\google-cloud-sdk\bin\gcloud.cmd"
$g = @('--project', $projectId, '--zone', $zone, '--tunnel-through-iap')
```

**Run the VM's shell blocks in an interactive session,** `& $gcloud compute ssh $instance @g`, or copy a
script over and run that. Use `--command` only for one line without pipes. `gcloud.cmd` passes the command
through `cmd.exe`, which splits a multi-line command into several (a stray redirection can even leave a file
named `NUL` in your home on the VM), and a `\|`, for example in a `grep` pattern, breaks it too.

The VM gets a network of its own. Leave the project's `default` network alone: don't attach the VM to it and
don't delete it, since other things in the project may rely on it. A new custom-mode network has no firewall
rules, so nothing gets in; one rule then lets SSH in from Google's IAP range only, which is where
`--tunnel-through-iap` connections come from. Networks, subnets and firewall rules cost nothing.

```powershell
& $gcloud compute networks create todo-net --project $projectId --subnet-mode custom
& $gcloud compute networks subnets create todo-subnet --project $projectId --network todo-net `
    --region $region --range 10.10.0.0/24
& $gcloud compute firewall-rules create todo-net-allow-ssh-from-iap --project $projectId --network todo-net `
    --direction INGRESS --action ALLOW --rules tcp:22 --source-ranges 35.235.240.0/20
```

Check that this is the network's only rule:

```powershell
& $gcloud compute firewall-rules list --project $projectId --filter 'network~/todo-net$'
```

Then create the VM in it:

```powershell
& $gcloud compute instances create $instance --project $projectId --zone $zone `
    --machine-type e2-micro --image-family debian-13 --image-project debian-cloud `
    --boot-disk-size 30GB --boot-disk-type pd-standard `
    --network todo-net --subnet todo-subnet --network-tier STANDARD `
    --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring `
    --metadata enable-oslogin=TRUE --no-service-account --no-scopes
```

- **`--no-service-account`.** The base deployment needs no Google API credentials. Attach a dedicated
  account only if you choose metadata authentication for [Android push](#android-reminder-wake-ups).
- **External IP.** The VM keeps its ephemeral external IP for outbound traffic: apt, R2 and the tunnel.
  Without one it would need Cloud NAT, which costs money. Outbound traffic is allowed by default; the
  network lets nothing in but SSH from IAP.

`gcloud compute ssh <instance> --project <project-id> --zone <zone> --tunnel-through-iap` now works, and
nothing else reaches the VM. Your account needs the IAP-secured Tunnel User role, which project owners have.

## Provision

Copy `deploy/linux` to the VM and run the script. From the repository root (keep the path relative: gcloud and
scp read `C:\...` as a host named `C`):

```powershell
& $gcloud compute scp @g --recurse deploy/linux "${instance}:"
& $gcloud compute ssh $instance @g --command 'sudo bash linux/provision.sh'
```

It installs base packages and adds Cloudflare's apt repository. apt trusts every key in the repository's
keyring, so the keyring holds exactly one key: the download must contain the pinned fingerprint and no other
primary key, and only that key, exported again by fingerprint, is installed. If Cloudflare rotates the key, the
script stops; check the new fingerprint on pkg.cloudflare.com and pass it in `CLOUDFLARE_KEY_FINGERPRINT`.
Then it installs `cloudflared` and sets up:

- **Users.** `todo` and `cloudflared`, both system accounts without a login shell.
- **Directories.** `/var/lib/todo` and `/etc/todo`.
- **Memory.** 1 GB of swap (`--swap-size`) and `vm.swappiness=10`.
- **UDP buffers.** `/etc/sysctl.d/91-todo-quic.conf` raises `net.core.rmem_max` and `net.core.wmem_max` to
  7500000, as quic-go advises, and is applied before the tunnel starts. Without it `cloudflared` logs
  `failed to sufficiently increase receive buffer size` and runs its QUIC connections with small buffers.
  A connector that already runs keeps its old buffers: if the script warns that the limits changed,
  restart `todo-tunnel` when a short outage suits you (about 30 s: `cloudflared` waits out a grace period
  before it exits).
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
`trust_cloudflare_headers = true`. Keep the template's `[web] dir = "/opt/todo/current/web"` to serve
`/app`; that path first resolves when a bundled release becomes current.

This file is the only source of the server's settings. **The unit must not have drop-ins; config lives in
the TOML.** A deploy must know which database the server opens, so it refuses to run if `todo-server.service`
has any drop-in (`systemctl show -p DropInPaths todo-server` is empty when it has none), uses
`PassEnvironment`, `UnsetEnvironment` or an `EnvironmentFile`, sets a `TODO_SERVER_*` variable other than
`TODO_SERVER_CONFIG`, or if the systemd manager's own environment sets one (`systemctl show-environment`).

Put each secret in place with the owner and mode from [Secrets](#secrets), for example:

```sh
sudo install -m 0640 -o root -g todo r2-attachments.json /etc/todo/
sudo install -m 0600 -o todo -g todo ingest-email-hmac.secret /etc/todo/
shred -u r2-attachments.json ingest-email-hmac.secret
```

`provision.sh` also corrects the modes in `/etc/todo` when you run it: `*.secret` files become todo:todo 0600,
everything else root:todo 0640.

### Android reminder wake-ups

Optional. Omit `[push]` to keep FCM off; Android still runs its periodic sync. FCM itself is available on
Firebase's free Spark plan. No Firebase configuration or credentials go in the APK, source, tunnel or CI.

1. In Firebase, register the Android app with package name `app.todoornot.android`. From its project and
   Android app settings (or the downloaded Android configuration), copy the project ID (`project_id`),
   Firebase app ID (`mobilesdk_app_id`), Web API key (`current_key`), and project number (`project_number`).
   Use the key for that Android app and ensure its API restrictions permit Firebase Installations and
   Cloud Messaging. If you restrict it by Android package and certificate, include the certificate of the
   APK you actually install. Do not add `google-services.json` to the app.
2. Enable the **Firebase Cloud Messaging API (HTTP v1)** in Google Cloud for the sending project. Create
   a dedicated service account and grant it **Firebase Cloud Messaging API Admin**
   (`roles/firebasecloudmessaging.admin`) in the target Firebase project, or an equivalent role that
   grants `cloudmessaging.messages.create`.
3. For a Compute Engine VM, prefer attaching that account to the VM and granting the
   `https://www.googleapis.com/auth/cloud-platform` access scope. This consumes short-lived metadata
   credentials and needs no key on disk. Set `gce_metadata = true`. If not using Compute Engine, use a
   service-account JSON key, install it outside the repo with mode `0640` and owner `root:todo` (or the
   equivalent owner/service-only ACL on Windows), and set `service_account_file` instead. Grant no other
   roles to the sender; rotate a file-based key if exposed.
4. Add this section to the private server config, using **one** credential option:

   ```toml
   [push]
   project_id = "<firebase-project-id>"
   application_id = "<firebase-android-app-id>"
   api_key = "<firebase-android-api-key>"
   gcm_sender_id = "<firebase-project-number>"
   gce_metadata = true
   # Or omit gce_metadata and use:
   # service_account_file = "/etc/todo/fcm-service-account.json"
   ```

   Restart `todo-server`. Give Android a sync cycle or reopen the app once to register, and check
   **Rooms → Settings → Push: On**. From another client, set a test reminder a few minutes ahead while
   Android is in the background. No inbound port is needed; the server makes outgoing requests to
   Google's OAuth and FCM endpoints. Google receives installation tokens and sequence hints, not task
   content. Treat the database and backups as private: they contain the registration tokens.

### Web Push reminders

Optional, independent of Android FCM. Deploy a release with Web Push and its bundled web app **before**
adding the new config section. An older binary rejects the unknown section. On the VM:

1. As the service account, generate a persistent VAPID key outside the repo. The command creates a new
   owner-only file (mode 0600) and won't overwrite one. If the file already exists, keep using it.

   ```sh
   sudo -u todo /opt/todo/current/todo-server web-push-key --out /var/lib/todo/web-push.secret
   sudoedit /etc/todo/server.toml
   ```

2. Keep `[web] dir = "/opt/todo/current/web"` and the HTTPS `public_origin`. Add:

   ```toml
   [web_push]
   vapid_key_file = "/var/lib/todo/web-push.secret"
   ```

   Save, then run:

   ```sh
   sudo systemctl restart todo-server
   sudo systemctl is-active todo-server
   curl -fsS http://127.0.0.1:47320/health
   ```

3. Sign in at `https://todo.example.com/app`. In **Settings**, choose **Turn on background reminders** and
   allow notifications. Choose **Send test notification**. Finally, schedule a reminder a few minutes
   ahead and close the app tab without signing out. On iOS/iPadOS, install the web app from Safari with
   **Add to Home Screen** and opt in from the installed app.

Allow outbound HTTPS to the supported browser push services (Google FCM, Mozilla autopush, Apple Web Push,
Windows WNS); no inbound port, cloud account or Firebase credentials are needed. The push body is encrypted
and contains only a task ID and reminder time. Session expiry or sign-out ends delivery rather than keeping
sessions alive in the background. With the default limits, even an opted-in browser stops receiving push after
one hour idle or 12 hours since sign-in; sign in and turn on background reminders again to resume.

Back up the VAPID file separately in a password manager or other private secret store; the normal database
backup does not include it. Treat database copies as private: they also hold push endpoints and subscription
keys. Changing the VAPID key requires users to opt in again. To disable push, remove `[web_push]` and restart.
For a rollback to a pre-Web-Push binary, also restore the pre-deploy database using the normal recovery
procedure: the old binary does not recognize the new migration.

## Deploy

### The build host

The release is built in WSL (Ubuntu). Install a C toolchain, rustup and **Node.js 24 LTS** (CI uses
24.15.0) in that distribution, not just on Windows. Both `rustup` and `node` must be on the WSL login
shell's `PATH`; `node` must be the Linux executable. Set up the C toolchain and rustup once:

```sh
sudo apt-get install build-essential curl
# rustup, for your WSL user only. Read https://rustup.rs first, or use your distribution's rustup package.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
```

Node.js doesn't need sudo either. This installs the version CI uses under `~/.local/node`, after checking
the tarball against the release's published checksums:

```sh
v=24.15.0 && cd /tmp
curl -fsSLO "https://nodejs.org/dist/v$v/node-v$v-linux-x64.tar.xz"
curl -fsSL "https://nodejs.org/dist/v$v/SHASUMS256.txt" | grep " node-v$v-linux-x64.tar.xz\$" | sha256sum -c -
mkdir -p ~/.local/node && tar -xJf "node-v$v-linux-x64.tar.xz" -C ~/.local/node --strip-components=1
echo 'export PATH="$HOME/.local/node/bin:$PATH"' >> ~/.profile
```

Open a new WSL shell afterwards, so `~/.cargo/bin` and `~/.local/node/bin` are on `PATH` (`node --version`
prints `v24.15.0`). The release script installs the
pinned toolchain and the Linux and wasm32 targets. It uses `wasm-bindgen-cli` at the exact version in
`apps/web/Cargo.lock`; if the CLI on `PATH` doesn't match, it installs the pinned version under the
build cache without replacing the operator's CLI. The production Node build needs no npm install.

CI's `linux-release` job builds and checks the complete release but **does not publish a deployable
artifact**. Build on the PC (or supply a full release you built on a compatible Linux host).

### Existing VMs: enable bundled web assets once

On a VM with an unbundled release already installed, leave the current config unchanged for the first
bundled deploy. If it uses `[web] dir = "/opt/todo/web"` with a manual symlink, leave both in place so
the previous release can still use them if you have to roll back. After the deploy succeeds and the
marker is gone, use `sudoedit /etc/todo/server.toml` to change `dir` to `"/opt/todo/current/web"`, or add
that `[web]` section from the template if it is missing. Before the restart, confirm the bundle is there with
`test -f /opt/todo/current/web/index.html && echo ok`: the server won't start if that directory is
missing, and `install-release.sh` refuses to switch to a release without a bundle while the config points
there. Restart `todo-server` and verify `/health`
and `/app` (sign in to check that the app loads). The server reads the assets at start-up. Future
bundled deploys and recoveries need no edit. Keep the manual symlink and assets until you no longer
need the unbundled release. If you must return to that release, first restore its old `[web]` settings
(or remove that section if the old binary predates web support); the old release has no `current/web`.

### Deploying

From the Windows PC:

```powershell
./deploy/deploy-linux.ps1 -Project $projectId -Zone $zone -Instance $instance
```

1. **Build.** WSL builds the Linux binaries and the production web app under `nice`, with the toolchain
   pinned in `rust-toolchain.toml`. The build cache is `~/.cache/to-do-or-not-to-do/target-linux` in WSL.
   The output is `target\linux-release\`: `todo-server`, `todo-backup`, `web/` (a copy of `apps/web/dist`)
   and `SHA256SUMS`. The binaries link against WSL's glibc: Ubuntu 22.04 has 2.35 and Debian 13 has 2.41.
   A first build takes longer, especially if the wasm-bindgen CLI needs to be built.
2. **Copy.** The release and `install-release.sh` go to `~/todo-deploy-<time>/` on the VM. That directory
   is removed again afterwards, whatever happens.
3. **Install.** `sudo install-release.sh` takes over. It holds `/opt/todo/deploy.lock` throughout, so a second
   deploy or recovery started meanwhile is refused at once. Nothing is stopped until the first three steps
   pass:
   1. It checks the binaries and all web file checksums, checks that both binaries run on the VM, and
      installs the release at `/opt/todo/releases/<version>-<hash>/`, synced before it gets its name.
   2. It runs the target binary's `todo-server database-path` as `todo`, in the unit's working directory.
      This reads and validates the live config just as `serve` does, without starting the server or
      touching the database. It also resolves a relative `data_dir` exactly as the server would. A
      config the binary refuses stops the deploy, as does a unit that overrides the settings (see
      [Configure](#configure)). Don't edit the config or unit during a deploy or recovery.
   3. If a release is installed already but there is no database at that path, it stops: there would be
      nothing to roll back to. After a deliberate wipe, or on a VM that isn't cut over yet, pass
      `-AllowEmptyDatabase`.
   4. **prepared.** It writes the deploy marker, `/opt/todo/deploy-in-progress`, synced to disk. Then it
      stops the backup timers, any backup run and the server, checks each `systemctl stop`, and waits until
      every one of them is inactive and nothing answers `/health`. It takes the backup lock, copies the
      databases and their `-wal` and `-shm` files to `/var/lib/todo-deploy/pre-<time>/`, and writes a
      `MANIFEST` (a `sha256sum` list) of exactly the files that existed. A database name that is a symlink or
      not a regular file stops the deploy. Every file is synced, and every sync is checked; any failure
      stops the deploy before it switches.
   5. **switched.** It points `/opt/todo/current` at the new release (a new symlink, renamed over the old
      one), checks that it does, starts the server and waits up to 60 s (`-HealthTimeoutSeconds`) until the
      unit is active and `/health` answers 200. The server runs pending migrations before it listens. Before
      it authorizes the start, it records `served=yes` durably in the marker: a started release may accept
      writes as soon as it listens. If that write fails, nothing starts and the deploy exits 3.
   6. It starts the backup timers that were running before (timers that were off stay off), and then removes
      the marker, as the very last step. Until then the guard refuses any backup run the timers start. If a
      timer won't start, the marker stays and the deploy exits 3; `--recover finish` tries again.
4. **A failed start.** If the new release doesn't become healthy, the script doesn't roll back on its own:
   the release was started and may have accepted writes. It stops the server and the backups, checks that
   they stopped, keeps the marker and exits 3. The message says either that everything is stopped or what
   is still running. The service stays down until you decide, from
   [An interrupted deploy](#an-interrupted-deploy): fix the cause and finish, or roll back with
   `--discard-writes`. A rollback then works like this:
   1. **restoring.** It stops everything and checks that it stopped, and only then writes `restoring` and
      restores the copy, in full, before anything starts. The files in the
      `MANIFEST` are copied into a fresh root-only directory inside the data directory, given the service
      user and mode 0600, and synced. For each database, every live entry with one of its names (file,
      sidecar, or anything the service user put there, such as a symlink) is moved aside, unread, into
      `failed-<time>/`, and the copies are renamed into place: a rename replaces a name and never follows a
      link. Then the directory is synced and the result checked against the `MANIFEST`.
   2. **restored.** Once the restore is durable, the marker says so. `current` points at the previous release
      again, is checked, and the previous release starts. From here on nothing restores again, so writes the
      previous release accepts are never undone.

   The live database is never overwritten in place.

Exit codes: 0 deployed; 1 nothing changed (a build, copy, checksum or config failure, a missing database, or
another deploy running); 2 rolled back, the previous release is current (from a recovery); 3 unfinished,
including a new release that didn't become healthy (see the next section); 4 deployed, but
`-PublicHealthUrl` didn't answer.

| Option | Effect |
|---|---|
| `-SkipBuild` | Deploys a complete release already in `-ArtifactDir`; older binary-only directories don't qualify. |
| `-NoStart` | Installs the release and leaves the server stopped. The marker records it, so a recovery keeps it stopped too. |
| `-AllowEmptyDatabase` | Deploys although a release is installed and no database exists: after a deliberate wipe, and for every deploy after the first to a VM that isn't cut over yet. |
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

### Recovering owner access

If the first passkey is lost or compromised, [reset owner sign-ins](device-pairing.md#owner-recovery)
without restoring a backup or rolling back tasks. For the standard systemd deployment:

```sh
sudo systemctl stop todo-server
sudo -u todo sh -c 'cd /var/lib/todo && exec /usr/local/bin/todo-server --config /etc/todo/server.toml reset-owner --yes'
sudo systemctl start todo-server
sudo cat /var/lib/todo/setup-code.txt
```

After a compromise, add `--revoke-agents --revoke-shares` to the reset command. This revokes agent
tokens and share links in the same audited transaction. Without the flags, the reset leaves them
unchanged and reports how many remain active. Check the canonical database path printed by the
reset before starting the service again.

Open `/setup` within 15 minutes of startup. A code generated offline expires 24 hours after reset;
if that deadline has passed at startup, the server writes a fresh code to the same file. Register
a new passkey, pair your devices again, and review any remaining agent tokens and share links.
Don't copy the setup code into a log or support message.

### An interrupted deploy

A deploy that stops halfway (the VM loses power or reboots, the script is killed, or `deploy-linux.ps1`
reports an unexpected exit code) leaves the marker behind. While it exists:

- `todo-server` refuses to start, at boot and by hand. The journal says why:
  `refusing to start todo-server: a deploy was interrupted (phase '...')`.
- Backup runs refuse to start too, so nothing backs up half-restored databases.

The databases are safe in every phase. The copy is taken while everything is stopped, the release switch is a
single rename, and a restore never writes over the live file. To see where the deploy stopped:

```sh
sudo /usr/local/lib/todo/install-release --status
```

It only reads. It prints the phase, the new and previous release, the data directory, the copy (`snapshot=`),
the start policy (`start=no` after `-NoStart`) and whether the new release ever answered its health check
(`served=yes`), and says so if a deploy or recovery is running right now. The phases:

| Phase | What is true |
|---|---|
| `prepared` | The previous release is current, and the new one never ran: the databases are as the previous release left it. The copy may be incomplete. |
| `switched` | The copy is complete. `current` may point at either release (the rename may not have happened). With `served=no` the new release was never started; with `served=yes` it was, and it may have accepted writes. |
| `restoring` | Everything was stopped, then a rollback restore was started; the databases may be a mix of old and new until it completes. Nothing runs. |
| `restored` | The databases are exactly the copy again. The previous release may have run on them since. |

Any phase can also end with the server and the timers running and only the marker left: the last step,
removing it, didn't happen. The recovery handles that like any other point in the phase.

Then choose one:

- **Finish.** `sudo /usr/local/lib/todo/install-release --recover finish` carries on from `prepared` or
  `switched`: it stops what runs, takes the copy again if the phase is `prepared`, points `current` at the
  new release and checks it, and starts it. If it doesn't become healthy, it never restores: it stops the
  server, checks that it stopped, keeps the marker and exits 3, so writes are never thrown away without you
  deciding to. Fix the cause and run `--recover finish` again. From `restoring` and `restored` only a
  rollback is possible.
- **Roll back.** `sudo /usr/local/lib/todo/install-release --recover rollback` returns to the previous
  release. From `prepared` it just points `current` back and starts it. From `switched` or `restoring` it first
  restores the copy in full, as above, every time it runs, so the server never starts on a mixed pair of
  databases; anything the new release wrote is then only in `failed-<time>/`. From `restored` it never
  restores again: it only points `current` back and starts the previous release.
- **Roll back and discard writes.** From `switched` with `served=yes` (the new release was started), a plain
  rollback changes nothing: it prints what would be lost (everything written since the deploy started) and
  exits 1. To go ahead, run
  `sudo /usr/local/lib/todo/install-release --recover rollback --discard-writes`. The live databases are moved
  to `failed-<time>/`, where they stay, and the copy taken before the deploy is restored.

Before stopping anything or restoring a database, recovery runs the target release's `database-path`
against the live config. If the binary can't check or accept the config (for example, an older binary
rejects a new section), or if it resolves to a different data directory than the marker records, the
script refuses to switch and exits 3 with the marker intact. Correct the config for the release you
intend to run and retry. The check validates the config, not external assets, credentials or database
compatibility. Then recovery stops the server and timers and points `current` at the chosen release;
`current/web` follows that same switch with no separate web restore.

Both honour the deploy's start policy, so after `-NoStart` the server stays stopped. Both need neither the PC
nor the upload: the script keeps a copy of itself in `/usr/local/lib/todo`. They exit with the deploy's codes,
and on 0 or 2 the marker is gone and the timers run again. Exit code 3 leaves the marker in place: read the
message and `sudo journalctl -u todo-server`, fix the cause, and run `--recover` again. A recovery that is
itself interrupted can always be run again.

Don't delete the marker by hand. If you must, stop the server first, and check that `/opt/todo/current` points
at a release that can open the databases.

### SSH lands without a home directory

If SSH prints `Could not chdir to home directory /home/<user>: Permission denied` and `deploy-linux.ps1`
fails to create its remote directory, OS Login has given your username a new POSIX UID: Google can issue a
second one for the same account, and the VM then uses it while your home directory still belongs to the old
one. Check, and if the home directory is the only thing owned by the old UID, give it to the new one:

```sh
id -u; sudo stat -c '%u %n' "/home/$USER"     # differ when this is the cause
sudo find / -xdev -uid "$(sudo stat -c %u "/home/$USER")" -not -path "/home/$USER/*" -print -quit   # prints nothing
sudo chown -R "$(id -u):$(id -g)" "/home/$USER"
```

### Known limits

These remain by design:

- **A failed start means downtime until you act.** A new release that starts but doesn't become healthy is
  never rolled back automatically, because it may have accepted writes. The service stays down until you
  run `--recover finish` (after a fix) or `--recover rollback --discard-writes`.
- **`--discard-writes` loses data from the service.** Everything written since the deploy started, including
  any writes the new release accepted, stops being served. The replaced databases stay in
  `/var/lib/todo-deploy/failed-<time>/` (the newest three), but nothing merges them back.
- **No way back after a finished deploy.** Once the marker is gone, the script can't undo the deploy. A
  release that becomes healthy but turns out broken stays current, and deploying the older release again
  only works if it can open the migrated databases. The `pre-<time>/` copies (the newest three) are there for
  a manual restore, which also loses the writes made since.
- **Only `/health` is checked.** A release that answers `/health` but is broken elsewhere counts as deployed.
- **A leftover start permission.** If the script is killed (not by a reboot) in the moment between writing
  `/run/todo-deploy/allow-start` and the server's start, the permission stays. Until the next deploy or
  recovery removes it, or a reboot clears it, it allows one start of the release that phase was about to
  start anyway.
- **A daily backup can be skipped.** If a deploy is still finishing when the daily run is due (03:30 UTC),
  the guard refuses that run, because the marker still exists. That day's daily backup is then missing.
  The 15-minute backups go on as usual.
- **The restore relies on the service being stopped.** Planted symlinks and files are moved aside rather
  than followed, but the restore assumes nothing runs as `todo` while it works. The script checks that
  every unit is stopped; a process started as `todo` outside these units could still race it.

## Moving from the Windows pilot

The pilot's database, its attachment key and its secrets move to the VM. The tunnel stays the same: the VM
runs a connector with the same token, and the PC's goes away. Clients notice about a minute of downtime
(a few minutes if you restore the final backup instead of copying the files) and nothing else. Passkeys,
sessions, device and agent tokens, and share links all keep working, because the public origin doesn't
change. Forwarded emails wait in the Email Worker's retry queue meanwhile.

Plan for 30 minutes. Besides the variables from [Google Cloud: create the VM](#google-cloud-create-the-vm),
the blocks below use two of the pilot's directories and its port:

```powershell
$data = "$env:LOCALAPPDATA\todo"   # the pilot's directory: server.toml, backup.toml, secrets, bin\, backup-state\
$db = $data                        # the pilot's data_dir: the databases and attachments.key
$port = 47320                      # the port in the pilot's bind
```

`$db` must be the `data_dir` from `$data\server.toml`, as an absolute path. It needn't be `$data` itself: a
pilot whose `data_dir` is a `data` subdirectory keeps its databases and `attachments.key` in `$data\data`,
and `$data` then holds no database at all. `$port` is the port in its `bind`, which the tunnel's public
hostname points at. Check both before the day. The VM's shell blocks below use 47320; put the pilot's port
there if it differs.

The copies below are taken with SQLite's `VACUUM INTO`, which writes one complete, self-contained file, WAL
included. **Never copy the raw `*.sqlite3*` files:** a pilot stopped from outside may leave changes in its
`-wal` file, and the `-shm` file must not travel at all. The PC needs Python 3 (`python` on `PATH`) for these
helpers; paste them into the session:

```powershell
# Prints the integrity check of an SQLite database, opened read-only. It must print 'ok'.
function Test-Database([string]$Path) {
    python -c "import sqlite3,sys,pathlib; print(sqlite3.connect(pathlib.Path(sys.argv[1]).resolve().as_uri()+'?mode=ro', uri=True).execute('pragma integrity_check').fetchone()[0])" $Path
}
# Writes a consistent copy of an SQLite database with VACUUM INTO. The source is opened read-only, so it
# stays exactly as it was. Prints the copy's integrity check, which must be 'ok'.
function Copy-Database([string]$From, [string]$To) {
    python -c "import sqlite3,sys,pathlib; c=sqlite3.connect(pathlib.Path(sys.argv[1]).resolve().as_uri()+'?mode=ro', uri=True); c.execute('VACUUM INTO ?', (sys.argv[2],)); c.close()" $From $To
    if ($LASTEXITCODE) { throw "could not copy $From" }
    Test-Database $To
}
# Prints the event count and max(seq) of a task database.
function Get-EventCount([string]$Path) {
    python -c "import sqlite3,sys,pathlib; print(sqlite3.connect(pathlib.Path(sys.argv[1]).resolve().as_uri()+'?mode=ro', uri=True).execute('select count(*), max(seq) from events').fetchone())" $Path
}
# Writes SHA256SUMS, in sha256sum's format, for every file below a directory.
function Write-Sha256Sums([string]$Dir) {
    $root = (Resolve-Path -LiteralPath $Dir).Path.TrimEnd('\') + '\'
    $lines = Get-ChildItem -LiteralPath $root -Recurse -File | Where-Object Name -ne 'SHA256SUMS' | ForEach-Object {
        '{0}  {1}' -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.FullName.Substring($root.Length).Replace('\', '/')
    }
    [IO.File]::WriteAllText("${root}SHA256SUMS", (($lines -join "`n") + "`n"))
}
# Checks every file that a directory's SHA256SUMS lists. Throws if one is missing or differs.
function Test-Sha256Sums([string]$Dir) {
    Get-Content -LiteralPath (Join-Path $Dir 'SHA256SUMS') | ForEach-Object {
        $hash, $name = $_ -split '\s+\*?', 2
        $file = Join-Path $Dir $name
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "$name is missing" }
        if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $hash) { throw "$name differs" }
    }
    'checksums ok'
}
```

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
3. **Deploy without starting.** Run `./deploy/deploy-linux.ps1 ... -NoStart`. The first deploy to the VM
   finds no database to copy: it says so and leaves a `pre-<time>/` with an empty `MANIFEST`, which is
   expected. **Every later deploy before the cutover needs `-AllowEmptyDatabase` too,** whether you rehearsed
   or are bringing a newer release along: a release is installed and there is no database yet, on purpose.
4. **Port.** If the pilot's `server.toml` uses another port than 47320, use that port on the VM too, so the
   tunnel's public hostname still matches.
5. **Optional: stage what doesn't change.** The secrets, `backup.toml` and the tunnel token can go to the VM
   before the day, as in steps 4 to 6 below: the token to `/root/todo-tunnel.token` (root, 0600) and nowhere
   else, so the connector stays off. The cutover then moves only the databases, the attachment key and the
   budget file, which keeps the downtime to about a minute. Do it after any rehearsal: the wipe above would
   delete what is in `/var/lib/todo`.

### Cut over

1. **Stop the pilot, but keep it installed.** On the PC:

   ```powershell
   Disable-ScheduledTask -TaskPath '\todo\' -TaskName pilot | Out-Null
   ./deploy/windows/stop-pilot.ps1 -DataDir $data
   # What must be gone: the supervisor (PowerShell running bin\run-pilot.ps1), anything run from the pilot's
   # directory (todo-server, todo-backup), its connector, and whatever listens on its port. Paths are compared
   # literally, without wildcards.
   $prefix = (Resolve-Path -LiteralPath $data).Path.TrimEnd('\') + '\'
   $runner = $prefix + 'bin\run-pilot.ps1'
   $tokenArg = '--token-file\s+"?' + [regex]::Escape($prefix + 'cloudflared-token') + '("|\s|$)'
   $left = @(Get-CimInstance Win32_Process | Where-Object {
       $cmd = [string]$_.CommandLine
       $file = if ($cmd -match '(?i)\s-File\s+(?:"([^"]*)"|(\S+))') { "$($Matches[1])$($Matches[2])" } else { '' }
       ($_.ExecutablePath -and ([string]$_.ExecutablePath).StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) -or
       ($_.Name -in 'powershell.exe', 'pwsh.exe' -and [string]::Equals($file, $runner, [StringComparison]::OrdinalIgnoreCase)) -or
       ($_.Name -eq 'cloudflared.exe' -and $cmd -match $tokenArg)
   })
   $left += @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue |
       ForEach-Object { Get-CimInstance Win32_Process -Filter "ProcessId = $($_.OwningProcess)" })
   if ($left) {
       $left | Format-Table ProcessId, Name, CommandLine -AutoSize | Out-Host
       throw 'THE PILOT IS STILL RUNNING. Stop these processes, then run this block again.'
   } else { 'stopped' }
   ```

   It must print `stopped`. If it lists processes instead, don't go on: the copies in step 3 would miss what
   the pilot still writes, and its connector would keep serving next to the VM's. Stop them
   (`Stop-Process -Id <pid>`) and run the block again.

   The disabled logon task can't start a second server on the old database at the next logon, and it stays in
   place for a [rollback](#roll-back). None of this needs admin rights. Remove the task with
   `uninstall-pilot.ps1` once the VM has run for a while (see [Verify](#verify)).

   **Downtime starts.** Until the VM's connector registers in step 8, the hostname answers with a Cloudflare
   tunnel error. Expect 502 rather than 530 at first: stopping the processes doesn't unregister the pilot's
   connector, and the edge keeps trying it for a while. That is the old connector, not a second one.
2. **Take the final backup.** The server is stopped, so this is the complete log:

   ```powershell
   $env:TODO_BACKUP_CONFIG = "$data\backup.toml"
   & "$data\bin\todo-backup.exe" run --kind daily --force
   ```

   Note the uploaded key it prints (`daily/<time>-<seq>.sqlite3.zst.age`). The upload needs no identity: it
   is the off-site copy of the final state, whichever way step 3 goes.
3. **Copy the databases.** The pilot is stopped, so a `VACUUM INTO` copy of each file is complete:

   ```powershell
   New-Item -ItemType Directory -Path "$data\move" | Out-Null
   Copy-Database "$db\attachments.sqlite3" "$data\move\attachments.sqlite3"
   Copy-Database "$db\todo.sqlite3" "$data\move\todo.sqlite3"
   Get-EventCount "$data\move\todo.sqlite3"
   ```

   Both copies must print `ok`. Keep the event count and `max(seq)` for step 7.

   **Or restore the final backup** in place of the `todo.sqlite3` copy. It proves that the last backup
   restores, but it needs the backup identity and adds a couple of minutes of downtime. Paste the identity from
   the password manager into a temporary file inside `$data` (that directory's ACL is yours only), then:

   ```powershell
   $objectKey = '<key>'   # the key from step 2
   & "$data\bin\todo-backup.exe" restore --object $objectKey --identity "$data\identity.tmp" --out "$data\move\todo.sqlite3"
   Remove-Item -LiteralPath "$data\identity.tmp"
   ```

   The restore checks integrity and prints `max(seq)` and the event count. The attachment registry isn't
   backed up, so `attachments.sqlite3` is always the copy. If you copied, run the
   [restore drill](#restore-drill) on the VM later instead.
4. **Collect the rest in `$data\move`:**
   - `$db\attachments.key`. Without it no attachment can be decrypted.
   - `$db\blobs\`, if the pilot uses the local attachment store.
   - Unless you staged them: the secret files that the pilot's `server.toml` and `backup.toml` point to (the R2
     credentials for attachments and for backups, and the ingest HMAC secret), and `cloudflared-token`.
   - Optionally `backup-state\budget.json`, which carries this month's operation count over.

   For example, with the secrets staged:

   ```powershell
   Copy-Item -LiteralPath "$db\attachments.key", "$data\backup-state\budget.json" -Destination "$data\move\"
   ```

   Leave `attachments-spool\`: it holds only leftovers of interrupted uploads, which the server deletes when
   it starts.
5. **Copy to the VM.** Record the checksums, then copy from inside `$data\move`, with relative names:

   ```powershell
   Write-Sha256Sums "$data\move"
   Push-Location -LiteralPath "$data\move"
   & $gcloud compute ssh $instance @g --command 'mkdir -p -m 700 move'
   if ($LASTEXITCODE) { Pop-Location; throw "ssh failed (exit code $LASTEXITCODE)" }
   & $gcloud compute scp @g --recurse @(Get-ChildItem -Name) "${instance}:move/"
   if ($LASTEXITCODE) { Pop-Location; throw "scp failed (exit code $LASTEXITCODE): run this block again" }
   Pop-Location
   ```

   Keep `$data\move` until step 7 has passed: it is the fallback if the copy on the VM turns out damaged.
6. **Check and put everything in place.** On the VM, in an interactive session. First check what arrived:

   ```sh
   cd ~/move
   sha256sum --check --strict SHA256SUMS
   for db in todo attachments; do sqlite3 -readonly "$db.sqlite3" 'PRAGMA integrity_check'; done
   sqlite3 -readonly todo.sqlite3 'select count(*), max(seq) from events'
   cd
   ```

   Every file must say `OK`, both checks `ok`, and the count and `max(seq)` must match step 3. If not, copy
   again (step 5). Then check that nothing on the VM runs or would start by itself:

   ```sh
   systemctl is-enabled todo-server todo-tunnel todo-backup-frequent.timer todo-backup-daily.timer
   systemctl is-active todo-server todo-tunnel todo-backup-frequent.timer todo-backup-daily.timer \
       todo-backup@frequent todo-backup@daily
   ```

   Every line of the first must say `disabled`, and of the second `inactive` (or `failed`). If one doesn't,
   `sudo systemctl disable --now` it and check again: a server or a backup running now would write to or read a
   half-copied database. Then install what arrived. The block stops at the first command that fails, and
   prints `INSTALLED` only if every one succeeded. Whatever wasn't collected (staged secrets, blobs with the R2
   store, the budget file) is skipped. The secrets keep their names here: if `server.toml` uses other names,
   rename them in `~/move` first.

   ```sh
   (
       set -e
       cd ~/move
       sudo install -m 0600 -o todo -g todo todo.sqlite3 attachments.sqlite3 attachments.key /var/lib/todo/
       if [ -d blobs ]; then   # the local attachment store
           sudo cp -rT blobs /var/lib/todo/blobs
           sudo chown -R todo:todo /var/lib/todo/blobs
           sudo chmod -R go= /var/lib/todo/blobs
       fi
       for f in r2-attachments.json r2-backups.json; do
           if [ -e "$f" ]; then sudo install -m 0640 -o root -g todo "$f" /etc/todo/; fi
       done
       if [ -e ingest-email-hmac.secret ]; then
           sudo install -m 0600 -o todo -g todo ingest-email-hmac.secret /etc/todo/
       fi
       if [ -e cloudflared-token ]; then
           sudo install -m 0600 -o root -g root cloudflared-token /root/todo-tunnel.token
       fi
       if [ -e budget.json ]; then   # this month's backup operation count
           sudo install -d -m 0700 -o todo -g todo /var/lib/todo/backup-state
           sudo install -m 0600 -o todo -g todo budget.json /var/lib/todo/backup-state/
       fi
       echo INSTALLED
   )
   ```

   If it doesn't print `INSTALLED`, fix the cause (a full disk, for example) and run it again. Then check every
   installed file against the checksums from the PC, blobs included:

   ```sh
   sed -E -e 's#  (todo\.sqlite3|attachments\.sqlite3|attachments\.key|blobs/.+)$#  /var/lib/todo/\1#' \
       -e 's#  budget\.json$#  /var/lib/todo/backup-state/budget.json#' \
       -e 's#  cloudflared-token$#  /root/todo-tunnel.token#' \
       -e 's#  ([^/]+\.(json|secret))$#  /etc/todo/\1#' ~/move/SHA256SUMS | sudo sha256sum --check --strict -
   ```

   Every line must say `OK`. Only then remove the copy from your home:

   ```sh
   find ~/move -type f -exec shred -u {} + && rm -rf ~/move
   ```

   Unless you staged it, write `/etc/todo/backup.toml` from `backup.toml.example`, with the pilot's `recipient`.
7. **Start the server, but not the tunnel yet.**

   ```sh
   sudo -u todo sqlite3 -readonly /var/lib/todo/todo.sqlite3 'select count(*), max(seq) from events'
   sudo systemctl start todo-server
   curl -fsS http://127.0.0.1:47320/health && echo ok
   sudo journalctl -u todo-server -n 20 --no-pager
   ```

   The count and `max(seq)` must match step 3. Run `sqlite3` as `todo`: as root it could create WAL files that
   the server can't open. Rotating the log epoch isn't needed, because this is the complete log. Once the
   server is healthy on that count, remove the staging copy on the PC, which holds plaintext data and secrets:
   `Remove-Item -LiteralPath "$data\move" -Recurse`.
8. **Point the tunnel at the VM.**

   ```sh
   sudo bash linux/provision.sh --tunnel-token-file /root/todo-tunnel.token
   sudo shred -u /root/todo-tunnel.token
   ```

   It installs the token, raises the UDP buffer limits if they aren't yet, and enables and starts
   `todo-tunnel`; it must end with `tunnel: active`. **Downtime ends** as soon as the public `/health` answers
   200, usually within seconds. In the Cloudflare dashboard, the tunnel now lists one connector, the VM's. The
   PC's went away in step 1; if it is still listed after a minute, check that no `cloudflared` for this tunnel
   still runs on the PC.
9. **Enable the server and the backups,** once the checks in [Verify](#verify) up to the manual backup have
   passed:

   ```sh
   sudo systemctl enable todo-server
   sudo systemctl enable --now todo-backup-frequent.timer todo-backup-daily.timer
   systemctl list-timers 'todo-backup-*'
   ```

   `todo-tunnel` isn't in the list because step 8 already enabled it. From now on the server comes back after a
   reboot, and backups run on their own. Deploys keep the timers running.

### Verify

- `./deploy/probe.ps1 -BaseUrl https://todo.example.com` passes, from any machine.
- Sign in with a passkey in the browser. The desktop app syncs, and a change made on one device shows up on
  another.
- An agent's MCP call works.
- A forwarded email becomes a task within a minute.
- Back up once by hand. The first run on the VM uploads even if nothing changed, because the VM has no upload
  record yet:

  ```sh
  sudo systemctl start todo-backup@daily
  sudo journalctl -u todo-backup@daily -n 5 --no-pager
  sudo -u todo todo-backup --config /etc/todo/backup.toml status
  ```

- After a day: `systemctl list-timers 'todo-backup-*'` shows recent runs, and `systemctl --failed` is empty.
- After a week of quiet, retire the pilot: `./deploy/windows/uninstall-pilot.ps1 -DataDir $data` removes its logon task, and
  then you can delete its data directory. Keep the final backup.

### Roll back

The VM's connector must never start again next to the pilot's. `provision.sh` enables and starts the tunnel
whenever `/etc/cloudflared/todo-tunnel.token` exists, and replaying step 8 would install the staged
`/root/todo-tunnel.token` again, so a rollback removes both copies of the token from the VM.

**Before the VM took any writes,** for example because step 7 or 8 failed:

```sh
sudo systemctl disable --now todo-tunnel
sudo systemctl stop todo-server
for f in /etc/cloudflared/todo-tunnel.token /root/todo-tunnel.token; do
    if sudo test -e "$f"; then sudo shred -u "$f"; fi
    if sudo test -e "$f"; then echo "STILL THERE: $f"; else echo "gone: $f"; fi
done
systemctl is-active todo-tunnel todo-server
```

Both files must say `gone`, and both units `inactive` (or `failed`). Only then, on the PC:

```powershell
./deploy/windows/install-pilot.ps1 -DataDir $data -Start -HealthUrl "http://127.0.0.1:$port/health"
```

This registers the logon task again, enabled, and starts the supervisor now. Pass `-HealthUrl` with the
pilot's port: the supervisor starts the connector only once `/health` answers 200 at that URL, which
otherwise is port 47320, so a pilot on another port would serve locally with no tunnel. The task's action
gets the same URL, so the next logon starts the tunnel too. The pilot's database
is exactly as it was when you stopped it. For the next attempt, stage the token at `/root/todo-tunnel.token`
again.

**After the VM took writes,** bring its data back so they aren't lost. That is both databases (tasks and the
attachment registry), and the attachment key with them. With the R2 attachment store (`store = "r2"`), the
attachments themselves are already in R2 and there are no local blobs to bring; with the local store, the
VM's `blobs/` comes along too.

1. **Stop everything on the VM, remove the token, and check.**

   ```sh
   sudo systemctl disable --now todo-tunnel todo-backup-frequent.timer todo-backup-daily.timer
   sudo systemctl stop 'todo-backup@*' todo-server
   sudo systemctl disable todo-server
   for f in /etc/cloudflared/todo-tunnel.token /root/todo-tunnel.token; do
       if sudo test -e "$f"; then sudo shred -u "$f"; fi
       if sudo test -e "$f"; then echo "STILL THERE: $f"; else echo "gone: $f"; fi
   done
   systemctl is-active todo-tunnel todo-server todo-backup-frequent.timer todo-backup-daily.timer \
       todo-backup@frequent todo-backup@daily
   curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:47320/health
   ```

   Both token files must say `gone`, every `is-active` line `inactive` (or `failed`), and curl must print
   `000`. Don't go on until they do: a server or a backup still running could change the files while you copy
   them.
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
   sudo -u todo sh -c 'set -e; umask 077; cd /var/lib/todo/rollback && cp -r ../blobs . &&
       find blobs -type f -exec sha256sum {} + >> SHA256SUMS'
   ```

   Each `integrity_check` must print `ok`. Note the event count and `max(seq)`. Instead of the `VACUUM INTO`
   copies you can also take a backup (`sudo systemctl start todo-backup@daily`, then `todo-backup restore` on
   the PC as in step 3 of the cutover), but only for `todo.sqlite3`: the attachment registry isn't backed up.
3. **Fetch them.** Move the copies to your home on the VM, then fetch them into a folder of yours on the PC
   (relative names again):

   ```sh
   sudo mv /var/lib/todo/rollback ~/rollback && sudo chown -R "$USER": ~/rollback
   ```

   ```powershell
   & $gcloud compute scp @g --recurse "${instance}:rollback" .
   if ($LASTEXITCODE) { throw "scp failed (exit code $LASTEXITCODE): run it again" }
   ```

   Leave the copy on the VM until the next step has passed.
4. **Check them on the PC,** in the folder you fetched to:

   ```powershell
   Test-Sha256Sums rollback
   Test-Database rollback\todo.sqlite3
   Test-Database rollback\attachments.sqlite3
   Get-EventCount rollback\todo.sqlite3
   (Get-FileHash rollback\attachments.key).Hash -eq (Get-FileHash -LiteralPath "$db\attachments.key").Hash
   ```

   It must print `checksums ok`, `ok` twice, the event count and `max(seq)` from step 2, and `True`. If a file
   is missing or differs, fetch it again (step 3). `True` means the VM ran with the pilot's attachment key. If
   it prints `False`, the VM made a key of its own, and attachments uploaded on the VM need the VM's key. Stop
   here and sort that out before anything replaces the pilot's key.

   Only once everything passed, remove the copy from the VM:

   ```sh
   find ~/rollback -type f -exec shred -u {} + && rm -rf ~/rollback
   ```

5. **Swap them in.** Move the pilot's databases aside, then put the copies in their place, in the pilot's
   `data_dir`:

   ```powershell
   $aside = New-Item -ItemType Directory -Path "$data\before-rollback-$(Get-Date -Format yyyyMMddTHHmmss)"
   Get-ChildItem -LiteralPath $db -File |
       Where-Object { $_.Name -like 'todo.sqlite3*' -or $_.Name -like 'attachments.sqlite3*' } |
       Move-Item -Destination $aside.FullName
   Copy-Item rollback\todo.sqlite3, rollback\attachments.sqlite3 $db
   # Local attachment store only:
   Copy-Item -Recurse -Force rollback\blobs\* "$db\blobs\"
   ```

6. **Verify without the tunnel.** Start only the server, check it, then stop it:

   ```powershell
   ./deploy/windows/install-pilot.ps1 -DataDir $data -NoRegister -NoTunnel -Start
   Invoke-RestMethod "http://127.0.0.1:$port/health"
   ./deploy/windows/stop-pilot.ps1 -DataDir $data
   ```

   `-NoRegister` leaves the logon task disabled meanwhile. Check the log in `$data\logs\` for a clean start.
   With the server stopped, `Get-EventCount "$db\todo.sqlite3"` must match step 2.
7. **Reconnect the pilot.** Register the logon task again and start the server with the tunnel, with the
   pilot's health URL as in the rollback before writes:

   ```powershell
   ./deploy/windows/install-pilot.ps1 -DataDir $data -Start -HealthUrl "http://127.0.0.1:$port/health"
   ./deploy/probe.ps1 -BaseUrl https://todo.example.com
   ```

   The probe must pass, and the Cloudflare dashboard must list only the PC's connector.
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
& $gcloud compute ssh $instance @g -- -N -L 47321:127.0.0.1:47321
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
| Setup code (pending setup or recovery) | `/var/lib/todo/setup-code.txt` | todo:todo 0600 | you, once; the server removes it after setup |
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

- **The external IPv4 address.** The VPC pricing page lists an hourly charge for in-use external IPv4
  addresses. When this runbook was written, the SKU for a VM's address in the Cloud Billing catalog (Billing →
  Pricing in the console, "External IP Charge on a Standard VM") had a free tier of 720 hours a month per
  billing account; the pricing page itself doesn't mention it. Look the SKU up for your account. If that tier
  still applies, a 30-day month costs nothing for the address and a 31-day month about 24 hours' worth: a
  small amount, but enough to trip a budget alert at 1% (see below).
- **IPv6 instead of IPv4.** The VPC pricing page lists no charge for external IPv6 addresses, but a VM gets
  one only on the **Premium tier** (`gcloud compute instances network-interfaces update --ipv6-network-tier`
  accepts only `PREMIUM`), whose free internet egress is 1 GB a month instead of the Standard tier's 200 GB. Backups and
  attachment downloads can pass 1 GB in a busy month. Even trying it costs: with a dual-stack interface, Linux
  prefers IPv6 wherever the other end has it (apt, R2 and Cloudflare's edge all do), so egress moves to the
  Premium tier as soon as the address is added. Keep the VM on IPv4 and the Standard tier, and accept the
  small charge in 31-day months.
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
$billingAccount = '<billing-account-id>'
& $gcloud billing budgets create --billing-account $billingAccount --display-name todo-vm `
    --budget-amount 1USD --threshold-rule percent=0.01 --threshold-rule percent=0.5 --threshold-rule percent=1.0
```

Or use Billing → Budgets & alerts in the console. The alert emails the billing admins. It doesn't stop
anything. If the IPv4 address has the free hours described above, expect the 1% alert in every 31-day month.
