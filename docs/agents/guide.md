# Working with the to-do list: a guide for agents

You are connected to one person's to-do list (the *owner*). These rules hold whether you reach it over
MCP (`/mcp`) or REST (`/api/v1`). Paste this file, or the short version the server sends when you connect,
into your instructions.

## What you can do: levels

The owner gives your token a level for each list (a list can also inherit the level of its collection, or
your default):

| Level | You may |
|---|---|
| none | Nothing. The list and its tasks don't exist for you: they never appear in results, and reading or changing them by ID gets `not_found`, exactly as for an ID that doesn't exist. |
| read | Read the list and its tasks (`list_lists`, `query_tasks`, `get_task`, `get_activity`). |
| capture | Also create tasks (`create_task`) and append notes (`add_note`). |
| plan | Also change, complete, reopen, move, merge and archive tasks; add and change steps; put tasks in My Day (`update_task`, `complete_task`, `move_task`, `merge_tasks`, `archive_task`, `add_step`, `update_step`, `set_my_day`). Moving or merging needs plan level on both ends. |

You can never change the structure: collections, lists and tags are the owner's. When a tool needs a
higher level than you have, it fails with `forbidden` and `details.needed` / `details.have`. Don't retry
it; tell the owner what you wanted to do instead.

## Where to put new tasks

- **The Inbox, unless you were told otherwise.** `create_task` without `list_id` uses the list named
  "Inbox" that you can capture into. If there's none, you get `inbox_not_found`: ask which list to use.
- Use another list only when the owner named it, or the task obviously belongs to one they pointed you at.
- Keep titles short and actionable (`Renew passport`, not `I think you should maybe renew…`). Put context,
  links and reasoning in `notes`.
- Don't set priority, due dates or My Day unless asked to or it's clearly implied ("by Friday").

## Avoiding duplicates

1. **Search first.** Before creating a task, call `query_tasks` with a few distinctive words from the
   title, for example `query: "passport"` or `query: "list:inbox passport"`. If a matching open task
   exists, add a note to it (`add_note`) instead of creating another one.
2. **Retry with the same `command_id`.** Every write accepts an optional `command_id` (a UUID). If a call
   times out, fails with `outcome_unknown`, or you're not sure it went through, repeat it with the **same
   `command_id` and the same arguments**. The server returns the original result (`replayed: true`)
   instead of writing twice. Never reuse a `command_id` for a different write: that fails with
   `command_id_reused`. A retry replays even if the Inbox was renamed or the day changed in between.
3. **Merge duplicates you find**, if you have plan level: `merge_tasks` with the duplicate as `source`
   and the task to keep as `target`.

## Your changes are attributed and can be undone

- Every write is recorded under your agent name, shows in the owner's activity feed, and can be undone
  by the owner at any time.
- Each write returns `events` (`[{id, seq}]`), a one-line `summary` and the log's `epoch`. To take back
  one of **your own** changes, call `undo_event` with the `seq` and `id` of any of its events (they're in
  the write result and in `get_activity`), and the `epoch` if you have it. You can't undo other people's
  changes (`not_own_event`).
- If the list was restored from a backup since you saw the event, the undo is refused with
  `event_mismatch` or `epoch_mismatch` instead of undoing something else. Look the change up again with
  `get_activity`.
- If later changes touched the same fields, undo fails with `undo_conflict` and lists them. Only pass
  `force: true` once you've checked that overwriting them is what the owner wants.
- `archive_task` hides a task from every list and query, but nothing is lost: `undo_event` brings it back.

## Limits

- `update_step` makes one change per call: retitle, or tick off / reopen, not both at once.
- Your token has a **write quota** per rolling hour (`quota_exceeded`, with `retry_after` seconds). Batch
  your work, and don't loop over writes you don't need.
- Requests are rate-limited per token, and only a few may be in flight at once
  (`too_many_mcp_requests`). Wait `retry_after` seconds and retry.
- Lists are paginated: when a result has `next_cursor`, pass it back as `cursor` (with the same query)
  for the next page. `get_activity` pages with `next_before` → `before`.

## Errors

A failed tool call is a tool error whose content is
`{"error": {"code": "...", "message": "...", "details": {...}, "retry_after": 1}}`, with the same codes as
the REST API. The ones you'll meet:

| Code | Meaning | What to do |
|---|---|---|
| `forbidden` | Your level is too low | Don't retry; tell the owner |
| `not_found` | No such item, or not visible to you | Check the ID with `list_lists` / `query_tasks` |
| `invalid_body`, `invalid_query`, `invalid` | Bad arguments | Fix them and retry |
| `undo_conflict` | Later changes touched the same fields | Inspect, then maybe `force: true` |
| `event_mismatch`, `epoch_mismatch` | The list was restored since you saw the event | Find the change again with `get_activity` |
| `quota_exceeded`, `rate_limited`, `too_many_mcp_requests` | Slow down | Wait `retry_after` seconds |
| `timeout`, `outcome_unknown`, `overloaded` | The server was busy | Retry with the same `command_id` |

## The query language, briefly

`query_tasks` takes the same queries as the app's smart lists:

```text
list:inbox and not done          open tasks in the Inbox
due<=today and not done sort:due overdue and due today
priority>=high tag:work          important work tasks
my_day                           today's My Day
created_by:agent updated>=-7d    what agents created this week
passport                         free text: titles, notes and steps
```
