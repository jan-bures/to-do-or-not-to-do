# Windows pilot

These scripts run `todo-server` and the `cloudflared` connector on a Windows PC for the current user, with no
admin rights. A per-user scheduled task starts a small PowerShell supervisor at logon. It runs headless, so no
window appears.

| Script | What it does |
|---|---|
| `install-pilot.ps1` | Copies the supervisor to `<data>\bin` and registers the logon task `\todo\pilot`. `-Start` also starts it now. |
| `run-pilot.ps1` | The supervisor. It starts the server, waits for `/health`, then starts the tunnel. It restarts either one if it exits, with backoff, and logs to `<data>\logs`. |
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
| `logs\` | Supervisor and process logs, one file per day, pruned after 14 days. |

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
  next time it starts.
- **Starting by hand.** `Start-ScheduledTask` can run an interactive task in another, disconnected session of
  the same user, where it fails with `0xC0000142`. `install-pilot.ps1 -Start` launches the supervisor in the
  current session instead. The logon trigger always uses the session that's logging on.
