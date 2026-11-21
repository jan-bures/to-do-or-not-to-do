# Windows pilot

These scripts run `todo-server` and the `cloudflared` connector on a Windows PC for the current user, with no
admin rights. A per-user scheduled task starts a small PowerShell supervisor at logon. It runs headless, so no
window appears.

| Script | What it does |
|---|---|
| `install-pilot.ps1` | Copies the supervisor to `<data>\bin` and registers the logon task `\todo\pilot`. `-Start` also starts it now. |
| `run-pilot.ps1` | The supervisor. It starts the server, waits for `/health`, then starts the tunnel. It restarts either one if it exits, with backoff, and logs to `<data>\logs`. When backups are set up, it schedules them too (see [Backups](#backups)). |
| `stop-pilot.ps1` | Stops the tunnel first, then the server, then the supervisor. The task stays registered. |
| `uninstall-pilot.ps1` | Stops everything and removes the task. Data, logs and secrets stay. |

## Layout

The data directory defaults to `%LOCALAPPDATA%\todo`. Restrict its ACL to your own user, because it holds
secrets.

| Path | Contents |
|---|---|
| `bin\todo-server.exe` | The server binary. Copy it here. |
| `bin\run-pilot.ps1` | Copied by the installer. |
| `server.toml` | Server config, passed in as `TODO_SERVER_CONFIG`. Keep `bind` on `127.0.0.1`. |
| `cloudflared-token` | The tunnel token. `cloudflared` reads it with `--token-file`, so it never appears on a command line. |
| `todo.sqlite3*` | The database and its WAL and lock files, under the `data_dir` set in the config. |
| `logs\` | Supervisor and process logs, one file per day, pruned after 14 days. Backup runs go to `backup-<date>.log`. |
| `bin\todo-backup.exe` | Optional: the backup tool. If it and `backup.toml` exist, backups run on a schedule. |
| `backup.toml` | Backup config, passed to `todo-backup --config`. |
| `r2-backups.json` | R2 credentials scoped to the backup bucket. Any name works; `backup.toml` points to it. |
| `backup-state\` | The tool's state: monthly operation counts, the record of past uploads, a lock file, and a private `scratch\` directory. It's created readable only by you. |
| `backup-schedule.json` | When the supervisor last started and last completed each kind of backup. |

`cloudflared` is found on `PATH`. A per-user install, for example with scoop, needs no admin rights either.
Without a token file, or with `-NoTunnel`, only the server runs.

## Install

```powershell
# Once: put the binary and config in place.
New-Item -ItemType Directory -Force "$env:LOCALAPPDATA\todo\bin" | Out-Null
Copy-Item target\release\todo-server.exe "$env:LOCALAPPDATA\todo\bin\"
# Write server.toml (see crates/todo-server for the options) and the tunnel token file.

./deploy/windows/install-pilot.ps1 -Start
```

## Notes

- **Restarts.** If a process keeps exiting within a minute, the restart delay doubles from 5 s up to 5 minutes.
  A process that ran for longer than a minute resets it.
- **Stopping is abrupt.** The server is ended with `Stop-Process`, not a console Ctrl+C. SQLite in WAL mode is
  crash-safe, and an interrupted command is retried with its command ID, so nothing is lost.
- **Leftover processes.** If the supervisor itself is killed, it cleans up the processes it left behind the
  next time it starts. A backup worker is recorded with its start time as well as its PID, so it's only
  killed if it is still the same process, never one that happens to reuse the PID.
- **Starting by hand.** `Start-ScheduledTask` can run an interactive task in another, disconnected session of
  the same user, where it fails with `0xC0000142`. `install-pilot.ps1 -Start` launches the supervisor in the
  current session instead. The logon trigger always uses the session that's logging on.

## Backups

`todo-backup` takes a consistent snapshot of the live database with `VACUUM INTO`, which is safe while the
server writes. Then it:

1. runs `PRAGMA integrity_check` on the snapshot;
2. compresses it with zstd;
3. encrypts it with [age](https://age-encryption.org) to a public key;
4. uploads it to R2 as `<kind>/<UTC timestamp>-<max seq>.sqlite3.zst.age`.

The PC only ever holds the public key. A leaked R2 credential or a stolen disk exposes no readable backup.

### Set up

```powershell
$data = "$env:LOCALAPPDATA\todo"
Copy-Item target\release\todo-backup.exe "$data\bin\"

# Make a key pair. It prints the public recipient (age1...), which is all the config needs.
& "$data\bin\todo-backup.exe" keygen --out "$data\backup-identity.txt"
```

`keygen` creates the identity file readable only by you, before it writes the key. On Windows it gets a
protected ACL with a single entry for your account; on Unix it gets mode 0600. If writing fails, the file is
removed.

1. Copy the file's contents into your password manager.
2. Delete the file: `Remove-Item "$data\backup-identity.txt"`.

Backups never need the identity on this PC. Only restores and drills do, and you can paste it into a
temporary file for those. Don't write the identity into the Desktop, Documents or any other folder that
OneDrive or another service syncs.

Write `backup.toml` in the data directory. Relative paths are resolved against the config file's directory.

```toml
db_path = 'todo.sqlite3'
state_dir = 'backup-state'
recipient = 'age1...'                 # from keygen
credentials_file = 'r2-backups.json'

# Optional. These are the defaults. Caps above the R2 free tier are rejected.
[budget]
class_a_monthly = 20000
class_b_monthly = 200000

# Optional. How long an unchanged database may go without a fresh upload (see Schedule).
# Each must stay at least a day below the bucket's retention: 144 for frequent, 2136 for daily.
[dedupe]
frequent_max_age_hours = 72    # 3 days, against 7 days of retention
daily_max_age_hours = 720      # 30 days, against 90 days of retention
```

Then write `r2-backups.json` with an R2 API token that is limited to the backup bucket:

```json
{
  "endpoint": "https://<account-id>.r2.cloudflarestorage.com",
  "bucket": "<bucket>",
  "access_key_id": "<access-key-id>",
  "secret_access_key": "<secret-access-key>"
}
```

Run it once by hand, then restart the supervisor with `stop-pilot.ps1` and `install-pilot.ps1 -Start`:

```powershell
$env:TODO_BACKUP_CONFIG = "$data\backup.toml"
& "$data\bin\todo-backup.exe" run --kind daily
& "$data\bin\todo-backup.exe" status
```

### Schedule

When `bin\todo-backup.exe` and `backup.toml` both exist, the supervisor:

- runs `run --kind frequent` every 6 hours (`-FrequentBackupMinutes`) and `run --kind daily` once per calendar
  day. The first check comes 5 minutes after the supervisor starts (`-BackupStartDelayMinutes`).
- runs one backup at a time, as a separate process. The supervisor loop only polls it and never waits on
  it, so supervising the server never stalls. A run that takes longer than an hour is killed through its
  process handle (`-BackupTimeoutMinutes`). It stays tracked until it has really exited, so no second
  backup starts next to it, and it's never counted as a success. If the supervisor stops while a worker
  won't exit, it leaves that worker's PID and start time in `pilot.pids.json`. The next start, or
  `stop-pilot.ps1`, stops the worker only if both still match, and the next start runs no backup until the
  worker has gone.
- records start and completion times in `backup-schedule.json`, so restarts and reboots don't back up again
  straight away. A failed run is retried after 30 minutes (`-BackupRetryMinutes`).
- appends each run's output to `logs\backup-<date>.log`.

A run sends no request at all if nothing changed since the last backup of the same kind, as long as that
backup is recent. "Nothing changed" means the same `max(seq)` and the same snapshot SHA-256. "Recent" means
younger than 3 days for `frequent` and 30 days for `daily` (`[dedupe]`). After that, an unchanged database
is uploaded again, so an idle period never lets every copy of a kind expire.

The bucket's lifecycle rules do the pruning: `frequent/` expires after 7 days and `daily/` after 90, and
incomplete multipart uploads are aborted after a day. The tool never deletes anything, and normal runs never
list.

### Budget

The tool counts every request it sends, per calendar month (UTC), in `backup-state\budget.json`. It charges
each request before sending it, so a failed request still counts. Once a cap would be exceeded, it refuses to
run and exits with code 3.

- **Durable.** Each charge is on disk before its request goes out. The counter is written to a temporary
  file, which is flushed and then renamed over `budget.json`. On Windows the rename uses `MoveFileExW` with
  `MOVEFILE_WRITE_THROUGH`, which returns only once the rename is on disk. On Unix it's `rename` followed by
  an fsync of the directory. A crash or power cut therefore never undercounts. At worst, a charged request
  was never sent.
- **The right month.** The month is resolved again right before each request, so a run that crosses midnight
  at the end of a month charges each request to the month it was actually sent in.
- **No redirects.** Redirects are never followed. A 3xx response is an error, so every request on the wire
  has been paid for.

- **Class A:** PUT, LIST and the multipart calls.
- **Class B:** GET and HEAD.

| | Class A a month | Class B a month |
|---|---|---|
| Default schedule (at most 4 frequent and 1 daily a day) | about 155 | 0 |
| Each restore drill | 0, or 1 LIST per kind without a local record | 1 |
| Each `restore --latest` | 1 LIST | 1 |
| Default cap | 20,000 | 200,000 |
| R2 free tier, for the whole account | 1,000,000 | 10,000,000 |

Storage stays small. At most 28 frequent and 90 daily objects exist at a time. A database of a few MB
compresses to well under 1 MB per object, far below the 10 GB free tier.

### Restore

```powershell
# A specific object. Its key is in the logs and in `todo-backup status`.
todo-backup restore --object daily/20261121T060000Z-42.sqlite3.zst.age --identity <file> --out D:\restore\todo.sqlite3
# The newest of one kind. Costs one LIST.
todo-backup restore --latest --kind daily --identity <file> --out D:\restore\todo.sqlite3
```

Restore downloads, decrypts and decompresses the backup, and checks its integrity, before the file appears at
`--out`. It never overwrites an existing file.

The plaintext is staged in a hidden `.todo-restore-...` directory next to `--out`, so the final no-clobber
rename stays on one volume. That directory is readable only by you, and it's recorded in `backup-state`
before it's created. If a restore is killed half-way, the next `restore` or `drill` finds that directory and
deletes it. If that directory can't be checked, or something else is at its path, it stays recorded, and a
later run looks again.

`--out`, like `backup-state` and the `keygen` output, must be on a volume that stores permissions (NTFS or
ReFS). FAT and exFAT, which many USB sticks use, can't keep a file owner-only, so the tool refuses to write
there.

### Temporary files

Snapshots and drill copies live in `backup-state\scratch\`, which is readable only by you. Each run gets its
own subdirectory, and each one, like each restore staging directory, holds a marker file that the tool
wrote. Cleanup deletes only directories that carry that exact marker. It never follows symlinks or
junctions, not even if `scratch` itself has been replaced by one, and it deletes nothing else, wherever it
is.

To put a backup back in place:

1. Stop the pilot.
2. Move the old `todo.sqlite3*` files aside.
3. Restore to `todo.sqlite3`.
4. Start the pilot again.

### Restore drill

`todo-backup drill --identity <file>` restores the newest backup into a scratch directory and checks:

- `integrity_check`;
- the SHA-256 of the restored file;
- the `events` row count and `max(seq)`, against what was recorded at upload;
- that the counters are sane.

It prints a short report. It exits with 1 if the object can't be downloaded, decrypted or checked, and with 4
if its contents don't match. The drill needs the identity, which deliberately isn't kept on the PC, so run it
by hand, for example once a month.
