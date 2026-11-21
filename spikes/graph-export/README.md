# Graph export spike

Dumps all Microsoft To Do data (personal Microsoft account) through Microsoft Graph v1.0 to raw JSON, then
reconciles it against a copy of the local To Do SQLite cache. Read-only: only GET requests, scope `Tasks.Read`.

## 1. Client ID

No app registration is needed. Microsoft's own "Microsoft Graph Command Line Tools" public client
`14d82eec-204b-4c2f-b7e8-296a70dab67e` works with a personal Microsoft account, device-code sign-in and
`Tasks.Read`. This was verified on 2026-11-21 with a full export. The script therefore never calls `/me`,
which would need `User.Read`.

If that ever stops working, register your own app instead. Since June 2024 that needs a directory:
https://entra.microsoft.com, then App registrations, then New registration. Choose "Personal Microsoft accounts
only", turn on "Allow public client flows", add the delegated `Tasks.Read` permission and copy the client ID.

## 2. Export

```powershell
cd spikes\graph-export
uv sync
$env:GRAPH_CLIENT_ID = "14d82eec-204b-4c2f-b7e8-296a70dab67e"
uv run python export.py            # prints URL + code; sign in with the personal account
```

Output goes to `out/` (gitignored, personal data): `lists.json`, `summary.json`,
`lists/NN_<hash>/{list,tasks,task_children}.json`, `lists/NN_<hash>/attachments/...` (binary files).
Token cache: `.token-cache.json` (gitignored). Re-runs sign in silently. Options: `--no-attachment-content`,
`--max-lists N`, `--out DIR`.

## 3. Reconcile with the SQLite cache

Copy the cache first; never point at the live file (close To Do first if possible):

```powershell
# <account-id> is the single 32-hex-character folder under AccountsRoot
$d = "$env:LOCALAPPDATA\Packages\Microsoft.Todos_8wekyb3d8bbwe\LocalState\AccountsRoot\<account-id>"
New-Item -ItemType Directory -Force $env:TEMP\todo-copy | Out-Null
Copy-Item "$d\todosqlite.db*" $env:TEMP\todo-copy\
uv run python reconcile.py --db $env:TEMP\todo-copy\todosqlite.db
```

`--db-only` prints cache counts without an export. Exit code 1 if any list differs.

## Known caveats

- Graph does not expose list groups, My Day, list/task order, list theme, or step order. Use the cache for those.
- Some users report `/me/todo/lists` returning fewer lists than the app shows. Reconcile flags lists that exist
  only in the cache.
- Attachment content comes from `contentBytes` (base64) per attachment.
