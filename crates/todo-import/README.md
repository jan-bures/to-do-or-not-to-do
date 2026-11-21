# todo-import

One-time importer from Microsoft To Do. It reads a Microsoft Graph export (made with
`spikes/graph-export/export.py`) and a **copy** of the To Do desktop cache, writes a self-contained import
bundle, and applies it to the server through `POST /api/v1/commands`.

The export, the cache and the bundle are personal data. Keep them outside this repository: `bundle` refuses
to write inside a git work tree. On Windows, use `%LOCALAPPDATA%\todo\import\`.

## Commands

```powershell
# 1. Copy the cache (never point at the live file).
$acct = "$env:LOCALAPPDATA\Packages\Microsoft.Todos_8wekyb3d8bbwe\LocalState\AccountsRoot\<account-id>"
$work = "$env:LOCALAPPDATA\todo\import"
New-Item -ItemType Directory -Force "$work\cache" | Out-Null
Copy-Item "$acct\todosqlite.db*" "$work\cache\"

# 2. Build and check the bundle.
todo-import bundle --export <export-dir> --cache "$work\cache\todosqlite.db" --out "$work\bundle.json"
todo-import verify --export <export-dir> --bundle "$work\bundle.json" --cache "$work\cache\todosqlite.db"

# 3. Apply (dry run first), then read everything back.
todo-import apply --bundle "$work\bundle.json" --server https://todo.example.com --dry-run
todo-import apply --bundle "$work\bundle.json" --server https://todo.example.com --token-file <token-file> --attachments
todo-import verify-server --bundle "$work\bundle.json" --server https://todo.example.com --token-file <token-file>
```

- `--time-zone <IANA name>` sets the owner's time zone for due dates (default: this machine's). `bundle`
  cross-checks every due date with the cache and warns if they differ.
- Dates are read strictly. Graph time zones may be `UTC`, IANA names or Windows names (mapped with the CLDR
  `windowsZones` table). If any date can't be read, or a required one is missing, `bundle` fails and names
  it. With `--allow-lossy` it builds anyway and lists each one in the report, with what was used instead
  (the value dropped, or another time). `verify` reads every date again with its own parser. Any date it
  can't read must be one the bundle lists as lossy.
- `verify --cache` also checks the received times of email links, which come from the cache.
- The token comes from `--token-file` or `TODO_IMPORT_TOKEN`, never from the command line. Plain `http://`
  is accepted only for `localhost`.
- Imports are owner-only, so the token must be an owner device token (from pairing, `tdd_…`). Agent tokens
  are refused. The server's static `[[auth.owner_tokens]]` work only with `auth.allow_config_owner_tokens`.
- `verify` and `verify-server` exit with 1 on any mismatch. Reports name lists and tasks by a short hash of
  their Graph ID; `verify --show-names` adds list names.
- `apply` is safe to re-run and resumes after a crash: see the `apply` module documentation. Its journal is
  tied to the server's log epoch, so after a server restore it refuses the old journal. Delete it and run
  again: re-sending is safe.
- `apply --attachments` also uploads each attachment's content, after the commands, through the owner-only
  `PUT /api/v1/attachments/{id}/content`. The server checks the size and SHA-256 against the imported
  reference and the body before it stores anything. Content the server already holds is skipped (a `HEAD`
  of the download route, which fetches nothing), progress goes in the same journal, and a failed attachment
  doesn't stop the run: the summary lists it, `apply` exits with 1, and running the same command again
  retries only what failed. Each upload counts against the server's attachment budget like any upload.
- `verify-server` also reports every attachment whose content the server doesn't hold yet.
- `apply` and `verify-server` wait as long as `Retry-After` asks, up to `--max-wait` seconds in total
  (default 900). Past that they stop with an error, and a later run resumes.

## Mapping

| Microsoft To Do | Imported as |
|---|---|
| List group (cache) | Collection, in cache order |
| List | List, in its collection or at the top level, in cache order; the default list first |
| List theme (cache) | List colour (approximate hex for known theme names) |
| Flagged Emails | An ordinary list; Outlook links become email links (sender and received time from the cache) |
| Task order, step order (cache) | Position keys in display order |
| Importance `high` / `low` / `normal` | Priority high / low / none |
| Categories | Tags (deduplicated case-insensitively) |
| Notes | Notes (line endings normalised; HTML converted to text) |
| Due date | Due date, taken in the owner's time zone |
| Completion, creation, modification times | Kept |
| Reminder (when on) | Reminder |
| Recurrence | The supported RRULE subset; anything else is reported in the bundle and not imported |
| Attachments | Attachment references; the content is uploaded by `apply --attachments` |
| My Day | Not imported (old marks are stale) |
