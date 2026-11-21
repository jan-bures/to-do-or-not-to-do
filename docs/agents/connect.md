# Connecting an agent over MCP

The server speaks MCP at `/mcp` over the **streamable HTTP** transport, with a bearer token. Every
snippet below uses placeholders: replace `https://todo.example.com` with your server's address.

1. **Create a token.** Sign in to the admin pages, open **Agents**, create a token for the agent, and set
   its level per list (see [the guide](guide.md#what-you-can-do-levels)). The token is shown once. An owner
   device token from pairing works too, but gives the agent the owner's full access: prefer an agent
   token.
2. **Put the token in an environment variable**, not in a config file you might share or commit:

   ```sh
   export TODO_MCP_TOKEN='<token>'           # bash, zsh
   ```

   ```powershell
   [Environment]::SetEnvironmentVariable('TODO_MCP_TOKEN', '<token>', 'User')   # Windows, persistent
   ```

   Restart the agent (and its terminal) afterwards so it sees the variable.
3. **Add the server** to the agent with one of the snippets below.
4. **Give the agent [the guide](guide.md)**, or rely on the short version the server sends when the agent
   connects.

## Claude Code

Project-wide, in `.mcp.json` at the project root. Claude Code expands `${TODO_MCP_TOKEN}` when it loads
the file, so the token itself never lands in the file:

```json
{
  "mcpServers": {
    "todo": {
      "type": "http",
      "url": "https://todo.example.com/mcp",
      "headers": {
        "Authorization": "Bearer ${TODO_MCP_TOKEN}"
      }
    }
  }
}
```

Or let the CLI write that file. The single quotes keep the shell from expanding the variable, so the
literal `${TODO_MCP_TOKEN}` is stored:

```sh
claude mcp add --transport http --scope project todo https://todo.example.com/mcp \
  --header 'Authorization: Bearer ${TODO_MCP_TOKEN}'
```

`claude mcp list` shows the server and warns if the variable isn't set.

`--scope user` makes the server available in every project, but stores it in `~/.claude.json`. Claude Code
documents `${VAR}` expansion for `.mcp.json`, not for that file, so with user scope check
`claude mcp get todo` after connecting. If the variable isn't expanded there, keep the server in a
project's `.mcp.json`.

## Codex

In `~/.codex/config.toml`. Codex reads the token from the named variable and sends it as
`Authorization: Bearer ...`:

```toml
[mcp_servers.todo]
url = "https://todo.example.com/mcp"
bearer_token_env_var = "TODO_MCP_TOKEN"
```

Or with the CLI, which writes the same entry:

```sh
codex mcp add todo --url https://todo.example.com/mcp --bearer-token-env-var TODO_MCP_TOKEN
```

## Any other streamable-HTTP client

| Setting | Value |
|---|---|
| URL | `https://todo.example.com/mcp` |
| Transport | Streamable HTTP. JSON-RPC over `POST`, answered with `application/json` |
| Auth | `Authorization: Bearer <token>` on every request. There's no OAuth flow |
| Sessions | None. The server is stateless and sends no `Mcp-Session-Id`. `GET` and `DELETE` get 405 |
| Browsers | Requests with an `Origin` header are refused (403) |

To check a token by hand:

```sh
curl -sS https://todo.example.com/mcp \
  -H "Authorization: Bearer $TODO_MCP_TOKEN" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_lists","arguments":{}}}'
```

A JSON-RPC `result` means it works. `401` means the token is missing, wrong, revoked or expired.
`429` means slow down: see `Retry-After`.

## Tools

| Tool | Level needed | What it does |
|---|---|---|
| `list_collections` | read | Collections you can see |
| `list_lists` | read | Lists you can see, with their IDs |
| `list_tags` | read | Tags you can see, with their IDs |
| `get_task` | read | One task with steps, links, attachments, reminders and tags |
| `query_tasks` | read | Tasks matching a smart-list query |
| `get_activity` | read | Recent changes in lists you can see, newest first |
| `create_task` | capture (plan with `recurrence`) | New task (in the Inbox unless `list_id` is given), optionally with a due date, priority and repeat rule |
| `add_note` | capture | Append to a task's notes |
| `add_link` | capture | Add a web link to a task |
| `update_task` | plan | Title, notes, due date, priority, repeat rule |
| `complete_task` | plan | Complete or reopen (`completed: false`) |
| `move_task` | plan (both lists) | Move to another list |
| `add_step` | plan | New checklist step |
| `update_step` | plan | Retitle, tick off or reopen a step (one change per call) |
| `set_my_day` | plan | Add to or remove from My Day |
| `tag_task` / `untag_task` | plan | Put a tag on a task, or take it off |
| `create_tag` | plan on at least one list | New tag |
| `add_reminder` | plan | Remind about a task at a time |
| `update_reminder` | plan | Move a reminder to another time |
| `merge_tasks` | plan (both tasks) | Merge a duplicate into another task |
| `archive_task` | plan | Soft-delete a task. Nothing is erased, and `undo_event` brings it back |
| `undo_event` | that of the change | Undo one of your own changes, by an event's `seq` and `id` |

Lists you have no level on are invisible: they never appear in results, and reading or writing them by
ID gets `not_found` (`event_not_found` for an undo), exactly as if they didn't exist.
