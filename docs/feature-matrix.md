# Feature matrix

This audit compares what the three clients do with what the server and the domain support. It reflects the
code at `origin/main` 13ea299, read file by file. The desktop gallery and the Android Paparazzi images were
checked against it. The desktop cells changed by work package D1 (every visible desktop control does
something or is hidden) cite the code after that change.

Use it to plan work. When a client gains a feature, update its cell.

## Legend

| Value | Meaning |
|---|---|
| **wired** | It works: it sends a `todo_core` command through `todo-client` (or the FFI engine, or the web backend) to the server, or performs the platform action. The cell cites the handler. |
| **shown** | It displays synced data, but nothing in this client can change it. |
| **stub** | It looks like a control (an icon, a hover, an arrow), but a click does nothing. |
| **missing** | Another client or the plan has it, but this client doesn't render it. |
| **n/a** | It doesn't apply to this platform. |

Path prefixes:

| Prefix | Path |
|---|---|
| `C/` | `crates/todo-core/src/` |
| `T/` | `crates/todo-client/src/` |
| `S/` | `crates/todo-server/src/` |
| `F/` | `crates/todo-ffi/src/` (the Android engine) |
| `D/` | `apps/desktop/src/` |
| `W/` | `apps/web/src/` |
| `A/` | `apps/android/app/src/main/kotlin/app/todoornot/` |

Every write goes through `POST /api/v1/commands` (`S/api/mod.rs:140`), carrying a `Command` from
`C/command.rs`. The core column names the command and its line. All three clients submit through
`todo_client::SyncClient`:

- desktop: `D/ui/shell.rs:947` `submit`;
- web: `W/app.rs:112` `Ctx::submit`, then `W/backend.rs`;
- Android: `F/engine.rs`, behind `A/model/TodoModel.kt`.

## Matrix

### Shell and platform

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Tray icon and menu (show, quick add, start with Windows, quit) | n/a | wired `D/platform/tray.rs:74`, `D/app.rs:339` | n/a | n/a |
| Closing the window hides it to the tray | n/a | wired `D/app.rs:289` | n/a | n/a |
| One running copy | n/a | wired `D/platform/single_instance.rs` | n/a | n/a |
| Global quick-add hotkey (Ctrl+Alt+Space, adds to My Day) | n/a | wired `D/platform/hotkey.rs:21`, `D/ui/quick_add.rs:69`, `D/ui/root.rs:457` | n/a | n/a |
| Start with Windows | n/a | wired `D/platform/autostart.rs:75` | n/a | n/a |
| Installer and auto-update | n/a | missing (planned) | n/a (deploys with the server) | n/a (sideloaded APK) |
| Pop-out list windows (always on top) | n/a | missing (planned) | n/a | n/a |
| Drop an email (`.eml`) to create a task | client parses, sends `CreateTask` and `AddLink` | missing (planned) | missing | n/a |
| Share text into the app | `CreateTask` | n/a | n/a | wired: `A/MainActivity.kt:30` (`ACTION_SEND`) pre-fills the composer |
| Reminder notifications | reminders stored (`C/command.rs:340`); no push | missing (planned: Windows toasts while in the tray) | missing | missing |
| Address-bar routes for rooms and tasks | n/a | n/a | wired `W/app.rs:153`, `W/app.rs:161` | n/a |

### Pairing, account and sync

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Sign in or pair | `POST /pair/start`, `/pair/poll` (`S/api/mod.rs:134`), passkey session | wired `D/ui/pairing.rs:498` | wired (passkey session, `/login`) | wired `A/ui/PairingScreen.kt` |
| Sign out, unpair, remove local data | `/auth/logout` | wired: the account menu (`D/ui/shell/menu.rs:410`, asks first) | wired `W/app.rs:862`; the IndexedDB copy is kept on purpose | wired `A/ui/Drawer.kt:201`, `A/ui/Unpair.kt` (asks first, counts unsent changes) |
| Sync status line | client `status()` | shown `D/model/derive.rs:628` (sidebar footer) | shown `W/app.rs:791` | shown `A/ui/Drawer.kt:165`, chip `A/ui/RoomPane.kt:215` |
| Sync now | client `nudge()` | wired: the account menu (`D/ui/shell/menu.rs:452`) | missing (runs automatically on wake, `W/app.rs:424`) | wired `A/ui/Drawer.kt:196`; the chip, `A/ui/RoomPane.kt:231` |
| Offline outbox, replay on reconnect | `todo-client` | wired (SQLite) | wired (IndexedDB, `MirrorStorage`) | wired (SQLite) |
| "Couldn't sync" list: see and dismiss | client `failed()` and `dismiss()` | shown as a count only (`D/model/derive.rs:636`); missing list and dismiss | shown as a count only (`W/app.rs:672`); missing list and dismiss | wired banner and dismiss `A/ui/RoomPane.kt:145`, `F/engine.rs:649` |
| Recover a damaged cache, start fresh | client `salvage_all` | wired `D/ui/failed.rs:105` | wired `W/main.rs:111` (Reset local data) | wired `F/recovery.rs`, `A/ui/PairingScreen.kt:106` |
| Settings: server, admin pages link | admin pages at `/admin` | wired: the account menu shows the server and opens `<server>/admin`, built from the paired address (`D/model/menus.rs:379`, `:463`, `D/ui/shell/menu.rs:407`) | missing | partial: the drawer menu shows the server (`A/ui/Drawer.kt:186`); missing the admin link |

### Navigation and filtering

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Six built-in smart lists with counts (My Day, Important, Planned, All, Completed, Agent activity) | `C/query/mod.rs:54` | wired `D/ui/shell/sidebar.rs:330` | wired `W/app.rs:716` | wired `A/ui/Drawer.kt:85`, `F/view.rs:57` |
| Collections and lists with open counts | read model | wired `D/ui/shell/sidebar.rs:266`, `:417` | wired `W/app.rs:716` | wired `A/ui/Drawer.kt:94`, `:102` |
| Fold a collection | client-only | wired `D/ui/shell/sidebar.rs:383`; kept in `ui.json` (`D/config.rs:120`) | wired `W/app.rs:757`; forgotten on reload | wired `A/ui/Drawer.kt:95` |
| Room pills, drawer (phone) | n/a | n/a | wired `W/app.rs:953`, `:888` | wired `A/ui/Pills.kt:65`, `:81`, `A/ui/Header.kt:109` |
| Filter by query or words (Ctrl+K, `/`) | `C/query` | wired `D/ui/shell.rs:271`, clear `:1150` | wired `W/app.rs:899`, `:907` | wired `A/ui/RoomPane.kt:136`, `:182` |
| Keyboard: J/K, arrows, Space, E, Esc | n/a | wired `D/ui/shell.rs:66`–`78`, `:2089` | wired `W/app.rs:492` | n/a |
| Keyboard: D (due), P (priority), Shift+P (priority picker), Alt+Up/Down (reorder) | n/a | wired `D/ui/shell.rs:2251`–`2264` | wired `W/app.rs` (`on_key`) | n/a |
| Keyboard: Ctrl+N or N focuses quick add | n/a | wired `D/ui/shell.rs:2604` | wired `W/app.rs:583` | n/a |

### Lists and collections

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| New top-level list | `CreateList` `C/command.rs:156` | wired `D/ui/shell/sidebar.rs:472` (then renames in place) | wired `W/app.rs` (`NewList`) | wired `A/model/TodoModel.kt` (`createList`) |
| New list in a collection | `CreateList` | wired: the collection menu's "New list here" (`D/ui/shell/menu.rs:407`) | wired: **New list** in a collection or one of its lists (`W/app.rs`) | wired: **New list** in a collection or one of its lists (`A/model/TodoModel.kt`, `F/engine.rs`) |
| Rename a list | `UpdateList` `C/command.rs:172` | wired: double-click, its context menu or the room menu (`D/ui/shell/sidebar.rs:241`, `D/ui/shell/menu.rs:385`) | missing | missing |
| Reorder lists | `Reorder` `C/command.rs:358` | wired: menu Move up and down (`D/ui/shell/menu.rs:405`) | missing | missing |
| Delete a list | `Delete` `C/command.rs:365` | wired from its context menu or the room menu, after a confirmation, with undo (`D/ui/shell/menu.rs:401`, `D/ui/shell/undo.rs:35`) | missing | missing |
| Move a list to another collection | `MoveList` `C/command.rs:187` | missing | missing | missing |
| List colour | `UpdateList.color` | missing (a swatch is set at creation only) | missing | missing |
| New collection | `CreateCollection` `C/command.rs:128` | missing | missing | missing |
| Rename a collection | `UpdateCollection` `C/command.rs:141` | wired: double-click, its context menu or the room menu (`D/ui/shell/sidebar.rs:384`, `D/ui/shell/menu.rs:386`) | missing | missing |
| Reorder collections | `Reorder` | wired: menu Move up and down (`D/ui/shell/menu.rs:405`) | missing | missing |
| Delete a collection | `Delete` | missing | missing | missing |
| Collection colour | `UpdateCollection.color` | missing | missing | missing |

### Task list

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Quick add in a room | `CreateTask` `C/command.rs:197` | wired `D/ui/shell.rs:248`, `:940` | wired `W/app.rs:1146` | wired `A/ui/RoomPane.kt:152`, `F/engine.rs:440` |
| The room shapes a new task (My Day date, high priority in Important, due today in Planned) | `NewTask` fields | wired `T/intent/mod.rs` | wired `T/intent/mod.rs` | wired `T/intent/mod.rs` |
| Quick-add syntax: a trailing date (`tomorrow`, `fri`, `+3d`, `zítra`) and `!`/`!!`/`!!!` | `T/intent/quick_add.rs` | wired, with live date chip (`D/ui/shell.rs`) | wired, with live date chip (`W/app.rs`, `QuickAdd`) | wired, with live date chip (`A/ui/RoomPane.kt`, `F/engine.rs::quick_add_date`) |
| Quick-add field's calendar and flag icons | n/a | hidden until the date picker (D2); the date and priority are typed (`D/ui/shell.rs:1564`) | missing | missing |
| Complete or reopen, with the choreography | `CompleteTask` / `UncompleteTask` `C/command.rs:216` | wired `D/ui/shell.rs:1767`, `:992`; Space | wired `W/app.rs:1340`, `:193`; Space | wired `A/ui/Rows.kt:75`, `F/engine.rs:477` |
| Completed section toggle | n/a | wired: the footer and the room menu (`D/ui/shell/undo.rs:320`, `D/ui/shell/menu.rs:397`) | wired `W/app.rs:1235` | wired `A/ui/RoomPane.kt:295` |
| Priority marks on rows | `Priority` | shown | shown | shown `A/ui/Rows.kt:97` |
| Set priority (four levels) | `UpdateTask.priority` | wired: the detail pane's Priority row picks any level, by click or Shift+P (`D/ui/shell/detail.rs:727`, `T/intent/mod.rs`); click the row's mark, or P, cycles (`D/ui/shell.rs:1951`, `:2257`) | wired: the detail pane's Priority field, Shift+P and P (`W/app.rs`) | missing (the star only) |
| Reorder tasks in a list | `Reorder` | wired: Alt+Up/Down, with undo (`D/ui/shell.rs:1028`) | wired: Alt+Up/Down in a list (`W/app.rs`, `W/model.rs`) | missing |
| Move a task to another list | `MoveTask` `C/command.rs:206` | missing (undo already handles it: `D/ui/shell/undo.rs:31`) | missing | missing |
| Delete a task | `Delete` | missing (undo already handles it: `D/ui/shell/undo.rs:28`) | missing | missing (a fixture-only hook: `F/engine.rs:499`) |
| Trash and restore | `Restore` `C/command.rs:370` | missing | missing | missing |
| Task context menu (right-click or long-press) | n/a | missing | missing | missing |
| Drag and drop (reorder, to a list, to My Day) | `Reorder`, `MoveTask`, `SetMyDay` | missing | missing | missing |
| Multi-select and bulk actions | n/a | missing | missing | missing |
| Room header's "view options" (sliders) and "more" (ellipsis) icons | n/a | "more" wired: the room menu, with what applies to the room: rename, show or hide completed, delete list (`D/ui/shell.rs:1382`, `D/model/menus.rs:245`, `D/ui/shell/menu.rs:351`). The sliders are hidden until there are sort or view options. | missing | missing |
| Finished My Day: hand-over to tomorrow | read-only | wired: a click selects (`D/ui/shell.rs:2442`, `:2476`) | wired: click or J/K selects (`W/app.rs`, `Handover`) | wired: a tap selects, the box completes (`A/ui/RoomPane.kt:460`, `:465`) |
| Hand-over row checkbox | `CompleteTask` | wired: completes the task with the row choreography, once reachable (`D/ui/shell.rs:2578`) | wired: completes in place, with the usual completion undo (`W/app.rs`, `HandoverCheck`) | wired `A/ui/RoomPane.kt:465` |
| Plan <weekday> (opens Planned) | n/a | wired `D/ui/shell.rs:2160` | wired `W/app.rs:1538` | wired `A/ui/RoomPane.kt:398`, `:438` |

### Task detail

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Edit the title | `UpdateTask.title` `C/command.rs:199` | wired `D/ui/shell/detail.rs:144`; E | wired `W/app.rs:1949`, `:2017`; E | wired `A/ui/Detail.kt:173`, `F/engine.rs:521` |
| Edit notes | `UpdateTask.notes` | wired `D/ui/shell/detail.rs:160` (multi-line) | wired `W/app.rs:2039` | wired: multi-line edit, save and clear (`A/ui/Detail.kt`, `F/engine.rs::set_notes`) |
| Complete from the detail pane | `CompleteTask` | wired `D/ui/shell/detail.rs:781` | wired `W/app.rs:1730` | wired `A/ui/Detail.kt:110` |
| My Day toggle | `SetMyDay` `C/command.rs:226` | wired `D/ui/shell/detail.rs:735` | wired `W/app.rs:1683` | wired `A/ui/Detail.kt:88`, `F/engine.rs:547` |
| Important toggle (high priority or none) | `UpdateTask.priority` | wired `D/ui/shell/detail.rs:744` | wired `W/app.rs:1694` | wired `A/ui/Detail.kt:94`, `F/engine.rs:558` |
| Add a step | `CreateStep` `C/command.rs:262` | wired `D/ui/shell/detail.rs:83`, `:184` | wired `W/app.rs:2063` | wired `A/ui/Detail.kt:223`, `F/engine.rs:581` |
| Complete a step | `CompleteStep` / `UncompleteStep` `C/command.rs:281` | wired `D/ui/shell/detail.rs:434` | wired `W/app.rs:1764` | wired `A/ui/Detail.kt:135`, `F/engine.rs:598` |
| Rename a step | `UpdateStep` `C/command.rs:274` | wired: click its title (`D/ui/shell/detail.rs:403`) | wired: click its title (`W/app.rs`, `DetailStep`) | missing |
| Reorder steps | `Reorder` | wired: hover arrows (`D/ui/shell/detail.rs:459`, `:466`) | wired: up/down buttons (`W/app.rs`, `DetailStep`) | missing |
| Delete a step | `Delete` | wired, with undo (`D/ui/shell/detail.rs:476`, `D/ui/shell/undo.rs:34`) | wired: delete button (`W/app.rs`, `DetailStep`), no undo yet | missing |
| Due date | `UpdateTask.due` | wired: typed, with D or a click (`D/ui/shell/detail.rs:706`, `:203`, `:251`) | wired: typed, with D or a click (`W/app.rs`, `DueEditor`) | shown (`A/ui/Detail.kt:154`) |
| Reminder | `AddReminder` / `UpdateReminder` `C/command.rs:340`, `:350`; `Delete` | shown only when set, with a "not editable" tooltip and no edit affordance (`D/model/derive.rs:724`, `D/ui/shell/detail.rs:740`) | shown "read only" | shown |
| Repeat | `UpdateTask.recurrence` (`C/recurrence.rs`) | shown only when set, with a "not editable" tooltip (`D/model/derive.rs:733`). Clearing the due date also clears the repeat (`T/intent/mod.rs`, `set_due`). | shown "read only" | shown |
| Tags on a task | `AddTag` / `RemoveTag` `C/command.rs:234`, `:241` | shown only when set, with a "not editable" tooltip (`D/model/derive.rs:749`) | shown "read only" | shown |
| Create, rename or recolour a tag | `CreateTag` / `UpdateTag` `C/command.rs:291`, `:301` | missing | missing | missing |
| Breadcrumb (collection and list) | n/a | shown | shown | shown |
| Links: open a web or email link | `Link` | wired, http(s) only (`D/ui/shell/detail.rs:600`) | wired `W/app.rs:1906` | wired: `SourceCard` opens an HTTP(S) destination through `ACTION_VIEW`; otherwise text only (`A/ui/Detail.kt`, `F/view.rs`) |
| Add or remove a link | `AddLink` `C/command.rs:316`; `Delete` | missing | missing | missing |
| Attachments: download or save | `GET /attachments/{id}` `S/api/mod.rs:149` | wired: a save dialog (`D/ui/shell/detail.rs:664`, `D/session.rs:360`) | wired `W/app.rs:1842`, `W/platform.rs:255` | missing (not in `TaskDetail`, `F/view.rs:402`) |
| Attachments: open | same | wired: documents and images only (`D/ui/shell/detail.rs:656`) | n/a (the browser opens downloads) | missing |
| Attachments: upload | `POST /tasks/{id}/attachments` (`S/api/attachments.rs:315`), bearer only | missing | missing (the route refuses the session cookie) | missing |
| Attachments: remove | `Delete` | missing | missing | missing |
| Created and edited stamps | stamps | shown `D/model/derive.rs:654` | shown `W/app.rs:1625` | shown `A/ui/Detail.kt:115` |
| Per-task activity (who created, edited, completed it) | stamps | missing (the subtitle only) | shown `W/app.rs:1863` (from stamps, not the event log) | missing (the subtitle only) |
| Merge two tasks | `MergeTasks` `C/command.rs:248` | missing | missing | missing |
| Close the detail | n/a | wired `D/ui/shell/detail.rs:752`, Esc | wired `W/app.rs:1699`, Esc | wired `A/ui/Detail.kt:81`, `:99`, Back |

### Undo, agents and activity

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Undo toast after completing | client-side | wired `D/ui/shell/undo.rs:376`, Ctrl+Z | wired `W/app.rs:1437`, Ctrl+Z | wired `A/ui/RoomPane.kt:509` |
| Exact undo through the server (compensating event, covers a recurring task's next occurrence) | `POST /events/{seq}/undo` (`S/api/mod.rs:142`); `SyncClient::undo_tracked` | wired `D/ui/shell/undo.rs:172`, `D/store.rs:147` | missing: reopens instead, and offers no Undo for a recurring task (`W/app.rs:243`) | wired: the toast holds the tracked completion; `F/engine.rs::undo_completion` reverses the whole event batch |
| Undo a reorder | same | wired | missing | n/a |
| Undo anything else (rename, edits, deletes) | same | partial: deleting a list or a step can be undone (`D/ui/shell/undo.rs:29`); renames and edits can't | missing | missing |
| Activity feed (all events, newest first, undo per event) | `GET /activity` (`S/api/mod.rs:143`) | missing (planned) | missing | missing |
| Group or filter changes by agent | `updated_by:` in the query language | partial: Agent activity smart list; sidebar summary per agent (`D/model/derive.rs:607`, `D/ui/shell/sidebar.rs:474`), not clickable | partial: smart list; sidebar summary (`W/app.rs:778`) | partial: smart list only; missing the agent summary |
| Agent marks on rows | `updated.by` | shown | shown | shown `A/ui/Rows.kt:91` |

### Server-side only (admin pages)

The owner manages these on the server's own pages (`S/web/mod.rs`) or through the local admin API
(`S/admin/mod.rs:222`). No client links to them yet.

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Share links (create, rotate, revoke) | `/admin/shares` | missing (no link) | missing (no link) | missing (no link) |
| Agent tokens and their per-list access | `/admin/agents` | missing (no link) | missing (no link) | missing (no link) |
| Devices (list, revoke) | `/admin/devices` | missing (no link) | missing (no link) | missing (no link) |
| Email ingest settings, audit log, passkeys | `/admin/ingest`, `/admin/audit` | missing (no link) | missing (no link) | missing (no link) |

## What the server supports and no client offers

Every item below exists in `todo-core` and over REST (`POST /api/v1/commands`), and mostly over MCP too
(`S/mcp/tools.rs`). No client offers it.

- **Tasks.** Delete and restore, `Delete` and `Restore`. Move to another list, `MoveTask` (MCP
  `move_task`). Merge, `MergeTasks` (MCP `merge_tasks`).
- **Collections.** Create (`CreateCollection`), delete and recolour.
- **Lists.** Move into another collection (`MoveList`) and recolour.
- **Tags.** Create, rename and recolour (`CreateTag`, `UpdateTag`; MCP `create_tag`). Attach and detach
  them on a task (`AddTag`, `RemoveTag`; MCP `tag_task`, `untag_task`).
- **Reminders.** Add, change and delete (`AddReminder`, `UpdateReminder`; MCP `add_reminder`,
  `update_reminder`).
- **Repeat rules.** `UpdateTask.recurrence` (MCP `update_task`).
- **Links.** Add and delete (`AddLink`; MCP `add_link`).
- **Attachments.** Upload (`POST /api/v1/tasks/{id}/attachments`) and delete.
- **Activity.** The feed (`GET /api/v1/activity`; MCP `get_activity`) and undoing any event in it,
  including an agent's.
- **Saved queries.** The read API serves smart lists (`GET /api/v1/smart-lists`) and queries
  (`GET /api/v1/tasks?q=`). Clients evaluate the same built-in queries locally. The core has no
  user-defined saved smart lists, so this is not a client gap.

## Gaps by priority

Sizes: **S** is under half a day, **M** is one to two days, **L** is more than two days.

### 1. Stubs visible on the desktop

| # | Gap | Where | Size |
|---|---|---|---|
| 1.1 | **Done in D1** (ellipsis wired, sliders hidden). The room header's sliders and ellipsis icons do nothing. Wire them, or remove them until they work. Sliders: sort and view options, such as showing completed tasks. Ellipsis: a room menu with rename, delete, pop out and copy a share link. | `D/ui/shell.rs:1349`, `:1379`, `:1380` | S to hide, M to wire |
| 1.2 | **Hidden in D1** until the date picker (D2). The quick-add field's calendar and flag icons do nothing. Make them pick a due date and a priority for the task being typed (a small popover, or insert the syntax). | `D/ui/shell.rs:1474`, `:1475` | M |
| 1.3 | **Tooltip in D1**; editing is D3 and D6. Reminder, Repeat and Tags rows look like the editable Due row but are inert, and they appear only when set, so nothing can be added. Make each always present and editable. | `D/model/derive.rs:681`–`737`, `D/ui/shell/detail.rs:669` | M each (tags L, with tag creation) |
| 1.4 | **Done in D1.** The hand-over rows' checkboxes are drawn but only select the task. Make them complete it, as on Android. | `D/ui/shell.rs:2315` | S |
| 1.5 | "N couldn't sync" in the sidebar has no way to see or dismiss what failed. Android has a banner. | `D/model/derive.rs:636` | S–M |
| 1.6 | The sidebar's agent rows look like rows but are inert. Make them open the activity feed filtered to that agent (see 2.3). | `D/ui/shell/sidebar.rs:474` | S, after 2.3 |
| 1.7 | **Done in D1** (the Priority row). You can set the priority only by clicking the `!` mark, and the mark is absent when the priority is none. Add a priority control to the detail pane, or keep the column clickable. | `D/ui/shell.rs:1813` | S |
| 1.8 | **Done in D1.** Deleting a step (no confirmation) and deleting a list (confirmation) can't be undone. | `D/ui/shell/detail.rs:473`, `D/ui/shell/sidebar.rs:695` | S |

### 2. Missing on the desktop (planned or supported by the core)

| # | Gap | Size |
|---|---|---|
| 2.1 | Delete a task, and move it to another list. The undo kinds exist already (`D/ui/shell/undo.rs:28`, `:31`). Add a task context menu, the Delete key and "Move to…". | M |
| 2.2 | Manage collections: new, delete, recolour; move a list between collections; recolour a list. | M |
| 2.3 | Activity feed with per-event undo, grouped by agent. Needs a `todo-client` call for `GET /activity`. | L |
| 2.4 | Reminders: edit them, and fire Windows toasts while the app runs in the tray. | L |
| 2.5 | Repeat editor: presets plus interval. | M |
| 2.6 | Tags: create, rename, recolour, add to and remove from a task. | M–L |
| 2.7 | Links: add and remove a web link. Attachments: upload by file picker or drop, and remove. Upload needs a `todo-client` call. | M |
| 2.8 | Pop-out list windows. | L |
| 2.9 | Drop a `.eml` from Outlook to create a task. | M (New Outlook), L (classic Outlook) |
| 2.10 | **Done in D1.** Settings: a link to the admin pages, Sync now. | S |
| 2.11 | Installer and auto-update. | L |
| 2.12 | Merge tasks. | S–M |

### 3. What the web lacks relative to the desktop

| # | Gap | Size |
|---|---|---|
| 3.1 | **Done.** Quick-add previews a date with the shared parser. | S |
| 3.2 | **Done.** Edit or clear the due date from detail or with D. | S–M |
| 3.3 | **Done.** Choose from four priority levels in detail or cycle with P. | S |
| 3.4 | **Done.** Steps can be renamed, reordered and deleted. | S–M |
| 3.5 | Lists and collections: rename, reorder, delete, with a context menu or a room menu. | M |
| 3.6 | **Done.** Reorder tasks in a list with Alt+Up/Down. | S |
| 3.7 | Exact undo through `SyncClient::undo_tracked`, including undo for a recurring task and for reorders. | M |
| 3.8 | **Done.** Hand-over rows select the task and can complete it from the checkbox. | S |
| 3.9 | "Not synced" list with dismiss. | S |
| 3.10 | Remember folded collections across reloads (in `localStorage`, say). | S |
| 3.11 | Upload attachments. The server must also accept the session (cookie and CSRF) on the upload route, which is bearer-only today. | M |
| 3.12 | Everything in section 2 that isn't desktop-only (2.1–2.7, 2.12), once the desktop has it. | — |

### 4. What Android lacks relative to the desktop

| # | Gap | Size |
|---|---|---|
| 4.1 | **Done.** Editable notes, including **Add notes** when empty. | S |
| 4.2 | **Done.** Open HTTP(S) sources; other sources have no arrow. | S |
| 4.3 | Attachments: list, download, open (`TaskDetail` has none). | M |
| 4.4 | **Done.** Exact completion undo through `undo_tracked`, including recurring tasks. | M |
| 4.5 | Due date (a date picker), priority (four levels). | M |
| 4.6 | Steps: rename, reorder, delete. | S–M |
| 4.7 | Lists: rename, delete, reorder; collections: rename, reorder. | M |
| 4.8 | **Done.** A live date chip in quick-add, using the submit parser. | S |
| 4.9 | Agent summary in the drawer. | S |
| 4.10 | Reminder notifications (local alarms from synced reminders). | L |
| 4.11 | Task delete and move, tags, reminders, repeat, once the shared command helpers exist. | — |

## Shared gesture-to-command code

`todo_client::intent` now builds commands for all three clients without I/O. It owns the quick-add
parser, task destinations and room defaults, and the builders for lists, tasks and steps. Each client
maps its own room key to the shared task intent. This resolves the creation drift:

- Every new list gets a stable swatch from its ID.
- All three clients can create lists inside collections.
- New tasks are placed first in their list's manual order. A smart list may display its own sort order.
- Web and Android parse the same trailing date and priority as desktop. Desktop and Android preview the parsed date while typing.
