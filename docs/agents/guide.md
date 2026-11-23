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
| read | Read the list and its tasks (`list_lists`, `list_tags`, `query_tasks`, `get_task`, `get_activity`). |
| capture | Also create tasks (`create_task`), append notes (`add_note`) and add web links (`add_link`). |
| plan | Also change, complete, reopen, move, merge and archive tasks; make tasks repeat; add and change steps; put tasks in My Day; tag tasks; add and move reminders (`update_task`, `complete_task`, `move_task`, `merge_tasks`, `archive_task`, `add_step`, `update_step`, `set_my_day`, `tag_task`, `untag_task`, `add_reminder`, `update_reminder`, and `create_task` with `recurrence`). Moving or merging needs plan level on both ends. |

You can never change the structure: collections and lists are the owner's, and so is renaming or deleting
tags. You may create a tag (`create_tag`) if you have plan level on at least one existing list **and** your
default level is at least read, which is also what lets you see every tag. It's the list's own level that
counts: plan on a collection with no lists, or whose lists are all set lower, isn't enough. Otherwise `create_tag` fails with
`forbidden` (`details.needed` says which level is missing), whatever the name: ask the owner to create
the tag. When a tool needs a
higher level than you have, it fails with `forbidden` and `details.needed` / `details.have`. Don't retry
it; tell the owner what you wanted to do instead.

## Where to put new tasks

- **The Inbox, unless you were told otherwise.** `create_task` without `list_id` uses the list named
  "Inbox" that you can capture into. If there's none, you get `inbox_not_found`: ask which list to use.
- Use another list only when the owner named it, or the task obviously belongs to one they pointed you at.
- Keep titles short and actionable (`Renew passport`, not `I think you should maybe renew…`). Put context,
  links and reasoning in `notes`.
- Don't set priority, due dates, repeat rules, reminders or My Day unless asked to or it's clearly implied
  ("by Friday", "every Monday", "remind me at 9").
- Use existing tags (`list_tags`). Create one only when the owner asked for it.

## Avoiding duplicates

1. **Search first.** Before creating a task, call `query_tasks` with a few distinctive words from the
   title, for example `query: "passport"` or `query: "list:inbox passport"`. If a matching open task
   exists, add a note to it (`add_note`) instead of creating another one.
2. **Retry with the same `command_id`.** Every write accepts an optional `command_id` (a UUID). If a call
   times out, fails with `outcome_unknown`, or you're not sure it went through, repeat it with the **same
   `command_id` and the same arguments**. If the command committed, the server returns the original
   result (`replayed: true`) instead of writing twice. Rejected commands are not stored. For example,
   if `move_task` returns `move_source_changed`, retrying with the same ID may succeed if the task
   is back in the expected source list. Never reuse a `command_id` for a different write: if the ID is
   already committed, this fails with `command_id_reused`. A committed write replays even if the
   Inbox was renamed or the day changed in between.
   But a retry is checked like a new call first: if the write itself took away your view of something it
   names, the retry gets `not_found`. For example, with no default level you see a tag only while it's on
   a task you can read, so after `untag_task` takes it off the last such task, the tag is gone for you.
   To confirm such a write, read the item that is still visible to you (here
   `get_task` on the task: the tag is no longer in its `tags`).
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
- `archive_task` is a soft delete, the same as deleting in the app: there's no separate archived state and
  no unarchive tool. The task leaves every list and query (`get_task` with `include_deleted: true` still
  finds it), nothing is erased, and `undo_event` on its event brings it back.

## Workflow and start dates

- `list_lists` returns `features: {workflow, timeline, default_view}` for each list. Prefer to set
  `status: "in_progress"` when starting work on a workflow-enabled list and use `complete_task`
  when finished. Turning a feature off does not erase the fields.
- `create_task` (capture) and `update_task` (plan) accept `status` (`backlog`, `todo`, or
  `in_progress`) and `start_date` (`YYYY-MM-DD`). If you supply both dates, start must be on or
  before due. If you edit only one and the dates would cross, the other moves to match it. Other
  omitted patch fields are left alone; `start_date: null` clears the start date.
- `get_task` and `query_tasks` return the effective status: `done` exactly when `completed_at` is
  set. Use `complete_task` with `completed: false` to reopen and reveal the stored open stage again.
  REST task reads expose the stored open stage instead; missing `status`, `start_date` or list
  `features` mean `todo`, no start date or both features off with the List view, respectively.

## Repeating tasks, due dates and reminders

- A due date is a day, `YYYY-MM-DD`; there's no due time. Use a reminder for a time of day.
- A repeat rule (`recurrence` on `create_task` and `update_task`) needs a due date. It has the shape
  `get_task` shows: `{"rule": {"freq": ..., "interval": 1}, "until": "YYYY-MM-DD"}`, where `freq` is
  `daily`, `weekly` (with `days`, such as `["mon", "thu"]`), `monthly` (with `day`, 1–31) or `yearly`.
  `interval` (1–999) defaults to 1 and `until` (inclusive) is optional. For example, every other Friday:
  `{"rule": {"freq": "weekly", "interval": 2, "days": ["fri"]}}`.
- Completing a repeating task creates the next one, with open status `todo` and due on the rule's
  next date. Its start date, if set, shifts by the same number of days as the due date.
  `recurrence: null` stops a task repeating.
- A bad rule fails with `invalid_recurrence`, and a rule without a due date with `invalid`.
- `add_reminder` and `update_reminder` take `at` as an RFC 3339 time with an offset
  (`2030-01-02T09:00:00+01:00`), or as a local time (`2030-01-02T09:00`) in the owner's time zone. The
  result gives the time in UTC.

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
| `invalid_body`, `invalid_query`, `invalid`, `invalid_recurrence` | Bad arguments | Fix them and retry |
| `duplicate_name` | A tag with that name exists | Use it (`list_tags`) |
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
status:in_progress sort:start    work under way, ordered by start date
status!=done start<=today        open tasks scheduled to start by today
start:any sort:status            tasks with start dates, in board-stage order
my_day                           today's My Day
created_by:agent updated>=-7d    what agents created this week
passport                         free text: titles, notes and steps
```

`status:` and `status!=` use `backlog`, `todo`, `in_progress` and `done`, including for checklist
lists. `sort:status` uses that order. `start:` accepts a date, `any` or `none`; date comparisons
accept the same relative and absolute dates as `due`. `sort:start` puts undated starts last in
ascending order; `sort:-start` reverses the order.
