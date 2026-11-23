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
| **core done, UI pending** | Shared logic is implemented; this client has not added the view or controls. |
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
| Installer and auto-update | n/a | wired: [Velopack installer and paired updates](desktop.md#windows-installer-and-updates) (`apps/desktop/scripts/package.ps1`, `D/updates.rs`) | n/a (deploys with the server) | n/a (sideloaded APK) |
| Pop-out list windows (always on top) | n/a | wired: **Pop out** from a list or smart list's context menu or room header; pin, resize and reopen per room (`D/ui/popouts.rs`, `D/ui/shell/popout.rs`) | n/a | n/a |
| Home-screen task widget | shares room derivation and the syncing outbox (`F/engine.rs`) | n/a | n/a | wired: resizable Glance widget with a per-widget smart list, list, or collection; task detail, quick add, complete and importance (`A/widget/`) |
| Drop an email (`.eml`) to create a task | client parses, sends `CreateTask` and, when identifiable, `AddLink` (`T/intent/email.rs`) | wired: drop files onto the list, one task per email with Undo (`D/ui/shell/email_drop.rs`) | wired: drop files onto the task list, same parsing and Undo (`W/app/email_drop.rs`) | n/a |
| Share text into the app | `CreateTask` | n/a | n/a | wired: `A/MainActivity.kt:30` (`ACTION_SEND`) pre-fills the composer |
| Reminder notifications | reminders stored (`C/command.rs:340`); no push. What to alert for: `T/intent/reminder.rs` `alarms` | wired: Windows toasts while the app runs, also hidden in the tray, planned again after every change to the data, with a timer for the next one; Open (shows the task), Snooze 10 min (this computer only) and Complete; a reminder more than five minutes stale never fires on its own but goes into one summary toast, and older than a day isn't mentioned; shown once, also across restarts (`D/model/reminders.rs`, `D/ui/shell/reminders.rs`, `D/platform/toast.rs`) | wired: one same-origin tab alerts while the app is open, once per reminder time across reloads; Settings controls browser permission, and the system alert opens the task. If unavailable, the in-app toast offers Complete and Snooze 10 min; web snooze moves the synced reminder (`W/app/reminder_alerts.rs`, `apps/web/static/reminder-browser.js`) | wired: local alarms from the last synced state (no push; a brief sync when one goes off), inexact with an exact one beside it when allowed, rescheduled after every sync change, a reboot and a clock or time-zone change; a notification on the Reminders channel with Complete (the app's own command, which syncs) and Snooze 10 min; missed reminders from the last 24 hours shown once (`A/reminders/ReminderPlan.kt`, `A/reminders/Reminders.kt`, `A/reminders/AndroidAlarms.kt`, `F/engine.rs::reminder_alarms`) |
| Address-bar routes for rooms and tasks | n/a | n/a | wired `W/app.rs:153`, `W/app.rs:161` | n/a |

### Pairing, account and sync

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Sign in or pair | `POST /pair/start`, `/pair/poll` (`S/api/mod.rs:134`), passkey session | wired `D/ui/pairing.rs:498` | wired (passkey session, `/login`) | wired `A/ui/PairingScreen.kt` |
| Sign out, unpair, remove local data | `/auth/logout` | wired: the account menu (`D/ui/shell/menu.rs:410`, asks first) | wired `W/app.rs:862`; the IndexedDB copy is kept on purpose | wired `A/ui/Drawer.kt:201`, `A/ui/Unpair.kt` (asks first, counts unsent changes) |
| Sync status line | client `status()` | shown `D/model/derive.rs:628` (sidebar footer) | shown `W/app.rs:791` | shown `A/ui/Drawer.kt:165`, chip `A/ui/RoomPane.kt:215` |
| Sync now | client `nudge()` | wired: the account menu (`D/ui/shell/menu.rs::sync_now`) | wired: Settings retries immediately and shows **Syncing…** while busy (`W/app.rs::SettingsMenu`, `W/backend.rs::nudge`); also runs on wake | wired: Settings retries immediately and shows **Syncing…** while busy (`A/ui/Drawer.kt::DrawerSettings`); also on the room's sync chip |
| Offline outbox, replay on reconnect | `todo-client` | wired (SQLite) | wired (IndexedDB, `MirrorStorage`) | wired (SQLite) |
| "Couldn't sync" list: see and dismiss | client `failed()`, `dismiss()` and `dismiss_many()`; plain words in `T/failure.rs` | wired: "N couldn't sync" in the sidebar footer opens the refused changes, each with what it was about and why (never the server's text), with Dismiss and Dismiss all (only what the list shows; one refused since stays) (`D/ui/shell/sidebar.rs:259`, `D/model/menus.rs:671`, `D/ui/shell/menu.rs:513`) | wired: Not synced, Dismiss and Dismiss all for the shown IDs (`W/app/room_actions.rs`, `W/backend.rs`) | wired banner and dismiss, in the shared plain words, never the server's text (`A/ui/RoomPane.kt:145`, `F/engine.rs` `failed`) |
| Recover a damaged cache, start fresh | client `salvage_all` | wired `D/ui/failed.rs:105` | wired `W/main.rs:111` (Reset local data) | wired `F/recovery.rs`, `A/ui/PairingScreen.kt:106` |
| Settings: server, admin pages link | admin pages at `/admin` (`T/lib.rs::ADMIN_PATH`, `T/native.rs::admin_url`) | wired: the account menu shows the paired server and opens its admin pages (`D/model/menus.rs::account_menu`, `D/ui/shell/menu.rs::open_admin`) | wired: Settings shows this app's origin and opens its admin pages in a new tab (`W/app.rs::SettingsMenu`) | wired: the drawer's settings menu shows the paired server and opens its admin pages in the browser (`A/ui/Drawer.kt::DrawerSettings`, `F/pairing.rs::admin_url`) |

### Navigation and filtering

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Six built-in smart lists with counts (My Day, Important, Planned, All, Completed, Agent activity) | `C/query/mod.rs:54` | wired `D/ui/shell/sidebar.rs:330` | wired `W/app.rs:716` | wired `A/ui/Drawer.kt:85`, `F/view.rs:57` |
| Collections and lists with open counts | read model | wired `D/ui/shell/sidebar.rs:266`, `:417` | wired `W/app.rs:716` | wired `A/ui/Drawer.kt:94`, `:102` |
| Fold a collection | client-only | wired `D/ui/shell/sidebar.rs:383`; kept in `ui.json` (`D/config.rs:120`) | wired: remembered in `localStorage` (`W/app.rs`, `W/platform.rs`) | wired `A/ui/Drawer.kt:95` |
| Room pills, drawer (phone) | n/a | n/a | wired `W/app.rs:953`, `:888` | wired `A/ui/Pills.kt:65`, `:81`, `A/ui/Header.kt:109` |
| Filter by query or words (Ctrl+K, `/`) | `C/query` | wired `D/ui/shell.rs:271`, clear `:1150` | wired `W/app.rs:899`, `:907` | wired `A/ui/RoomPane.kt:136`, `:182` |
| Keyboard: J/K, arrows, Space, E, Esc | n/a | wired `D/ui/shell.rs:66`–`78`, `:2089` | wired `W/app.rs:492` | n/a |
| Keyboard: D (due), P (priority), Shift+P (priority picker), Alt+Up/Down (reorder) | n/a | wired `D/ui/shell.rs:2251`–`2264` | wired `W/app.rs` (`on_key`) | n/a |
| Keyboard: Delete (delete), M (move to another list), Shift+F10 (the task's menu) | n/a | wired `D/ui/shell.rs` (`list_key`, `MoveTaskTo`, `TaskMenu`) | wired on a task row or in its detail pane (`W/app/task_actions.rs`) | n/a |
| Keyboard: R (repeat), T (tag picker), L (add a link) | n/a | wired `D/ui/shell.rs` (`PickTags`, `AddLinkKey`; `pick_tags` `:1023`, `D/ui/shell/detail.rs:254`) | wired `W/app.rs` (`on_key`) | n/a |
| Keyboard: Ctrl+N or N focuses quick add | n/a | wired `D/ui/shell.rs:2604` | wired `W/app.rs:583` | n/a |

### Lists and collections

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| New top-level list | `CreateList` `C/command.rs:156` | wired `D/ui/shell/sidebar.rs:472` (then renames in place) | wired `W/app.rs` (`NewList`) | wired `A/model/TodoModel.kt` (`createList`) |
| New list in a collection | `CreateList` | wired: the collection menu's "New list here" (`D/ui/shell/menu.rs:407`) | wired: **New list** in a collection or one of its lists (`W/app.rs`) | wired: **New list** in a collection or one of its lists (`A/model/TodoModel.kt`, `F/engine.rs`) |
| Rename a list | `UpdateList` `C/command.rs:172` | wired: double-click, its context menu or the room menu (`D/ui/shell/sidebar.rs:241`, `D/ui/shell/menu.rs:385`) | wired: sidebar and room menus (`W/app/room_actions.rs`) | wired: its drawer menu (long press) or the room menu, then a dialog (`A/ui/MainScreen.kt`, `A/model/Menus.kt`, `F/engine.rs::rename_list`) |
| Reorder lists | `Reorder` `C/command.rs:358` | wired: menu Move up and down (`D/ui/shell/menu.rs:405`) | wired: Move up and down, with undo (`W/app/room_actions.rs`) | wired: Move up and down in its drawer menu or the room menu (`A/model/Menus.kt`, `F/engine.rs::move_room`) |
| Delete a list | `Delete` `C/command.rs:365` | wired from its context menu or the room menu, after a confirmation, with undo (`D/ui/shell/menu.rs:401`, `D/ui/shell/undo.rs:35`) | wired: both menus, with confirmation and undo (`W/app/room_actions.rs`) | wired from its drawer menu or the room menu, after a confirmation checked again on confirm, with undo (`A/ui/Editing.kt` `DeleteListCard`, `F/engine.rs::delete_list`, `undo_deletion`) |
| Move a list to another collection | `MoveList` `C/command.rs:187`; like a task's move, the event records the position too, so undoing it after a reorder in the new collection conflicts | wired: Move to… in its context menu or the room menu opens a collection picker (and No collection), with undo (`D/model/menus.rs:703`, `D/ui/shell/menu.rs:720`, `T/intent/organize.rs:75`) | wired: Move to… with the same choices and Undo (`W/app/room_actions.rs`) | wired: Move to… in either list menu, including No collection and tracked Undo |
| List colour | `UpdateList.color` | wired: Colour… in its context menu or the room menu, the design's twelve swatches (`D/model/menus.rs:516`, `D/ui/shell/menu.rs:629`, `T/intent/organize.rs:103`) | wired: Colour… and twelve swatches (`W/app/room_actions.rs`, `W/app/pickers.rs`) | wired: Colour… in both menus, twelve design swatches |
| List settings and default view | `ListFeatures` on `List`; `UpdateList.features`; `T/intent/workflow.rs` | core done, UI pending | core done, UI pending | core done, UI pending (`F/engine/workflow.rs`) |
| New collection | `CreateCollection` `C/command.rs:128` | wired: **+ New → Collection**, then named in place (`D/ui/shell/sidebar.rs`, `T/intent/organize.rs:19`) | wired: **+ New → Collection** with a name field (`W/app.rs`, `NewRoom`) | wired: **+ New → Collection** in the drawer |
| Rename a collection | `UpdateCollection` `C/command.rs:141` | wired: double-click, its context menu or the room menu (`D/ui/shell/sidebar.rs:384`, `D/ui/shell/menu.rs:386`) | wired: sidebar and room menus (`W/app/room_actions.rs`) | wired: its drawer menu (long press) or the room menu, then a dialog (`F/engine.rs::rename_collection`) |
| Reorder collections | `Reorder` | wired: menu Move up and down (`D/ui/shell/menu.rs:405`) | wired: Move up and down, with undo (`W/app/room_actions.rs`) | wired: Move up and down in its drawer menu or the room menu (`F/engine.rs::move_room`) |
| Delete a collection | `Delete`; its lists and their tasks are hidden with it, not deleted | wired from its context menu or the room menu, after a confirmation that counts the lists and tasks that go with it, with undo (`D/model/menus.rs:437`, `D/ui/shell/menu.rs:622`, `D/ui/shell/undo.rs:29`) | wired: both menus, with confirmation and undo (`W/app/room_actions.rs`) | wired: both menus, rechecked confirmation and tracked Undo |
| Collection colour | `UpdateCollection.color` | wired: Colour… in its context menu or the room menu (`D/model/menus.rs:516`) | wired: same picker as for lists (`W/app/room_actions.rs`) | wired: Colour… in both menus |

### Task list

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Quick add in a room | `CreateTask` `C/command.rs:197` | wired `D/ui/shell.rs:248`, `:940` | wired `W/app.rs:1146` | wired `A/ui/RoomPane.kt:152`, `F/engine.rs:440` |
| The room shapes a new task (My Day date, high priority in Important, due today in Planned) | `NewTask` fields | wired `T/intent/mod.rs` | wired `T/intent/mod.rs` | wired `T/intent/mod.rs` |
| Quick-add syntax: a trailing date (`tomorrow`, `by fri`, `za 2 týdny`, `1 Dec`) and `!!`/`!!!` | `T/intent/quick_add.rs` | wired, with live date and priority chips (`D/ui/shell.rs`) | wired, with live date and priority chips (`W/app/quick_add.rs`) | wired, with live date and priority chips (`A/ui/QuickAdd.kt`, `F/quick_add.rs`) |
| Quick-add field's calendar and flag icons | shared draft, calendar model and priority choices (`T/intent/quick_add.rs`, `T/intent/calendar.rs`) | wired: date presets, keyboard month grid and four priorities with removable chips, including in pop-outs (`D/ui/shell/quick_pickers.rs`) | wired: same calendar and priority pickers, keyboard grid and removable chips (`W/app/quick_add.rs`) | wired: same presets, grid, priorities and removable chips (`A/ui/QuickAdd.kt`, `F/quick_add.rs`) |
| Workflow status and start dates | `UpdateTask.status` / `start_date`; effective `status:` and `start:` queries; REST and MCP | core done, UI pending | core done, UI pending | core done, UI pending (`F/engine/workflow.rs`) |
| Workflow quick-add suffixes and staged start dates | `T/intent/quick_add.rs`; destination-gated draft and submission | core done, UI pending | core done, UI pending | core done, UI pending (`F/quick_add.rs`) |
| Board view and column moves | `T/board.rs`; one `MoveTaskStatus` or `CompleteTask` command | core done, UI pending | core done, UI pending | core done, UI pending (`F/engine/workflow.rs`) |
| Timeline view | `T/timeline.rs`, including start-only bars and Unscheduled | core done, UI pending | core done, UI pending | core done, UI pending (`F/engine/workflow.rs`) |
| Complete or reopen, with the choreography | `CompleteTask` / `UncompleteTask` `C/command.rs:216` | wired `D/ui/shell.rs:1767`, `:992`; Space | wired `W/app.rs:1340`, `:193`; Space | wired `A/ui/Rows.kt:75`, `F/engine.rs:477` |
| Completed section toggle | n/a | wired: the footer and the room menu (`D/ui/shell/undo.rs:320`, `D/ui/shell/menu.rs:397`) | wired `W/app.rs:1235` | wired `A/ui/RoomPane.kt:295` |
| Priority marks on rows | `Priority` | shown | shown | shown `A/ui/Rows.kt:97` |
| Set priority (four levels) | `UpdateTask.priority` | wired: the detail pane's Priority row picks any level, by click or Shift+P (`D/ui/shell/detail.rs:727`, `T/intent/mod.rs`); click the row's mark, or P, cycles (`D/ui/shell.rs:1951`, `:2257`) | wired: the detail pane's Priority field, Shift+P and P (`W/app.rs`) | wired: the detail pane's Priority row picks any level (`A/ui/Detail.kt` `Fields`, `F/engine.rs::set_priority`, `T/intent/mod.rs` `pick_priority`); the star sets high or none |
| Reorder tasks in a list | `Reorder` | wired: Alt+Up/Down, with undo (`D/ui/shell.rs:1028`) | wired: Alt+Up/Down in a list, with undo (`W/app.rs`, `W/model.rs`) | wired: Move up/down past the adjacent visible open task in a manual list, with undo (`A/model/Menus.kt`, `F/engine.rs::reorder_task`) |
| Move a task to another list | `MoveTask` `C/command.rs:206`; the event records the position too, so undoing it after a reorder in the new list conflicts | wired: the task's menu or M opens a list picker, grouped by collection, that filters as you type; with undo (`D/model/menus.rs:613`, `D/ui/shell.rs:1203`, `T/intent/mod.rs:332`) | wired: the task menu or M opens a grouped, filterable list picker; with Undo (`W/app/task_actions.rs`) | wired: grouped, filterable Move to… picker, with undo (`A/ui/TaskActions.kt`, `F/engine.rs::move_task_to`) |
| Delete a task | `Delete` | wired: the task's menu or the Delete key, without asking, with undo (`D/ui/shell.rs:1196`, `T/intent/mod.rs:326`) | wired: the task menu or Delete, without confirmation, with Undo (`W/app/task_actions.rs`) | wired: Delete task without confirmation, with undo (`F/engine.rs::delete_task`, `undo_deletion`) |
| Trash and restore | `Restore` (`C/command.rs`); explicit task, list and collection tombstones from normal sync, ordered and labeled in `T/trash.rs` | wired: sidebar Trash, Restore and tracked Undo (`D/ui/shell/trash.rs`) | wired: sidebar/drawer, `/app/trash`, Restore and tracked Undo (`W/app/trash.rs`) | wired: drawer Trash, Restore and tracked Undo (`A/ui/Trash.kt`, `F/engine/trash.rs`) |
| Task context menu (right-click or detail overflow) | n/a | wired: right-click a row, or Shift+F10: complete or reopen, My Day, Important, Priority…, Move to…, Merge into…, Links…, Delete (`D/ui/shell.rs:2129`, `D/model/menus.rs:915`) | wired: right-click a row, use Shift+F10, or use ⋯ in detail; offers Move to…, Merge into… and Delete task (`W/app/task_actions.rs`) | wired: ⋯ in detail; long-press on a row now selects it (`A/ui/TaskActions.kt`, `A/ui/RoomPane.kt`) |
| Drag and drop (reorder, to a list, to My Day) | `Reorder`, `MoveTask`, `SetMyDay`; `BulkTasks` for a selection; shared eligibility, placement and toast in `T/task_drag.rs` | wired: grip, insertion line, edge scrolling, sidebar targets, Alt+Up/Down and Undo (`D/ui/shell/task_drag.rs`) | wired: grip, insertion line, edge scrolling, sidebar and phone drawer targets, Alt+Up/Down and Undo (`W/app/task_drag.rs`) | wired: select with long press, drag the grip, left edge opens the drawer, tracked Undo (`A/ui/TaskDrag.kt`, `F/engine/task_drag.rs`) |
| Multi-select and bulk actions | `BulkTasks` groups a single explicit task action into one undoable event batch; `T/selection.rs` defines selection order, actions and toast text | wired: Ctrl+click, Shift+click, Shift+Up/Down, Ctrl+A, Esc; selection bar and count summary (`D/ui/shell/bulk.rs`) | wired: same gestures, including Cmd on Mac; responsive selection bar (`W/app/bulk.rs`) | wired: long-press, tap to toggle, Back or Close to clear; contextual bars (`A/ui/BulkSelection.kt`, `F/engine/bulk.rs`) |
| Room header's "view options" (sliders) and "more" (ellipsis) icons | n/a | "more" wired: the room menu, with what applies to the room: rename, colour, move a list to another collection, pop out lists and smart lists, show or hide completed, delete the list or collection (`D/ui/shell.rs:1382`, `D/model/menus.rs:549`, `D/ui/shell/menu.rs:351`). The sliders are hidden until there are sort or view options. | "more" wired for lists and collections: rename, recolour, reorder, move a list to another collection, toggle completed and delete (`W/app/room_actions.rs`); sliders hidden | wired: the ⋯ menu offers rename, colour, move, Show or Hide completed and delete as applicable (`A/model/Menus.kt`); no sliders |
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
| Rename a step | `UpdateStep` `C/command.rs:274` | wired: click its title (`D/ui/shell/detail.rs:403`) | wired: click its title (`W/app.rs`, `DetailStep`) | wired: tap its title, or its menu (`A/ui/Detail.kt` `Steps`, `F/engine.rs::rename_step`) |
| Reorder steps | `Reorder` | wired: hover arrows (`D/ui/shell/detail.rs:459`, `:466`) | wired: up/down buttons (`W/app.rs`, `DetailStep`) | wired: Move up and down in its ⋯ menu (`A/ui/Detail.kt`, `F/engine.rs::move_step`) |
| Delete a step | `Delete` | wired, with undo (`D/ui/shell/detail.rs:476`, `D/ui/shell/undo.rs:34`) | wired: delete button with Undo (`W/app.rs`, `DetailStep`) | wired, with undo (`A/ui/Detail.kt`, `F/engine.rs::delete_step`, `undo_deletion`) |
| Due date | `UpdateTask.due` via `T/intent/mod.rs::set_due`; No date also clears the repeat, as noted in the picker for repeating tasks | wired: click Due or press D for the shared composer calendar (`D/ui/shell/detail.rs`, `D/ui/shell/quick_pickers.rs`) | wired: same calendar, with D or a click (`W/app.rs::DueEditor`, `W/app/quick_add.rs`) | wired: Due opens the shared quick-add calendar (`A/ui/Detail.kt`, `A/ui/QuickAdd.kt`, `F/engine.rs::set_due`) |
| Reminder | `AddReminder` / `UpdateReminder` `C/command.rs:340`, `:350`; `Delete`. Builders: `T/intent/reminder.rs` (`set_reminder`, `remove_reminder`, which reminder the row shows) | wired: the Reminder row is always there (Tab or Shift+R); presets (Later today, Tomorrow morning, Next week), a custom day and time (steps or typed), Remove; with Undo; an edit on a reminder that changed meanwhile writes nothing (`D/model/menus/reminder.rs`, `D/ui/shell/menu.rs`, `T/intent/reminder.rs`) | wired: the Reminder row offers presets and a custom local date and time; add, change or remove with Undo; refuses edits on a reminder that changed meanwhile (`W/app/reminder.rs`) | wired: the Reminder row is always there and opens a date and time picker; set, change or remove, with Undo; an edit on a reminder that changed meanwhile writes nothing (`A/ui/Reminders.kt` `ReminderCard`, `F/engine.rs::set_reminder`, `remove_reminder`, `undo_reminder`) |
| Repeat | `UpdateTask.recurrence` (`C/recurrence.rs`) | wired: always-present Repeat row, Tab or R; presets, custom interval/unit/weekly days and Don’t repeat. An undated task gets its first matching due date on or after today in the same undoable patch (`D/model/menus/repeat.rs`, `T/intent/repeat.rs`). Clearing Due clears Repeat. | wired: presets, custom interval/unit/weekly days and clear; first due date is set with the rule, with Undo (`W/app/repeat.rs`) | wired: presets, custom interval/unit/weekly days, stop, tracked Undo (`A/ui/Organize.kt`, `T/intent/repeat.rs`). Clearing Due clears Repeat. |
| Tags on a task | `AddTag` / `RemoveTag` `C/command.rs:234`, `:241` | wired: the detail pane's Tags row (always there, each tag a dot in its colour) or T opens the tag picker; choosing a tag puts it on or takes it off, and the picker stays open (`D/model/derive.rs:764`, `D/model/menus.rs:1133`, `D/ui/shell/menu.rs:576`, `T/intent/organize.rs:293`) | wired: Tags or T, with the picker staying open and tracked Undo (`W/app/tags.rs`) | wired: Tags picker, including create from typed name |
| Create, rename, recolour or delete a tag | `CreateTag` / `UpdateTag` / `Delete` | wired: type a new name into the tag picker to create it (and put it on the task); a tag's own page (its chevron, or Right) renames it (any tag) in place in the Tags row or recolours it; names stay unique ignoring case, as the core compares them (`to_lowercase`, so composed and decomposed accents are still two names; the search fold would merge them but the core doesn't use it) (`D/ui/shell/menu.rs:733`, `D/model/menus.rs:1187`, `T/intent/organize.rs:250`, `:268`). Delete tag… confirms with the live task count, rechecked on confirm; Undo restores it (`D/model/menus.rs::tag_confirm`, `D/ui/shell/undo.rs`). | wired: create from the filter, rename and recolour from the tag menu, delete after a rechecked impact confirmation, with Undo (`W/app/tags.rs`) | wired: tag options, rechecked delete confirmation and tracked Undo |
| Breadcrumb (collection and list) | n/a | shown | shown | shown |
| Links: open a web or email link | `Link` | wired, http(s) only (`D/ui/shell/detail.rs:600`) | wired `W/app/links.rs` | wired: `SourceCard` opens an HTTP(S) destination through `ACTION_VIEW`; otherwise text only (`A/ui/Detail.kt`, `F/view.rs`) |
| Add or remove a link | `AddLink` `C/command.rs:316`; `Delete` | wired: the detail pane's link field (L) adds a web link, checked by the core's rules (a bare `example.com` gets `https://`); a link's remove button or the task's Links… page removes it; both with undo (`D/ui/shell/detail.rs:228`, `:625`, `D/model/menus.rs:1095`, `T/intent/organize.rs:148`) | wired: link field or L, shared URL validation, and removal with Undo (`W/app/links.rs`) | wired: Add link… and each source’s remove button, with tracked Undo |
| Attachments: download or save | `GET /attachments/{id}` `S/api/mod.rs:149` | wired: a save dialog (`D/ui/shell/detail.rs:664`, `D/session.rs:360`) | wired (`W/app/attachments.rs`, `W/platform.rs::download`) | wired: replica-backed file list; authenticated download to private cache and Save through the system picker (`A/ui/Attachments.kt`, `F/engine/attachments.rs`) |
| Attachments: open | same | wired: documents and images only (`D/ui/shell/detail.rs:656`) | n/a (the browser opens downloads) | wired: documents and images only, through a read-only FileProvider grant |
| Attachments: upload | `POST /tasks/{id}/attachments` (`S/api/attachments.rs`), bearer or owner session with CSRF and matching origin | wired: **Add file** opens the Windows multi-select picker, or drop files from Explorer onto the open task. Uploads are direct, with busy and error feedback (`D/ui/shell/attachments.rs`, `D/ui/shell/detail.rs`, `D/session.rs`) | wired: **Add file** or drop onto task detail (`W/app/attachments.rs`) | wired: **Add file** via system picker, size-limited direct upload |
| Attachments: remove | `Delete` | wired: each file's **Remove** button uses tracked Undo and queues offline (`D/ui/shell/attachments.rs`, `D/ui/shell/undo.rs`) | wired: **Remove** with Undo (`W/app/attachments.rs`) | wired: **Remove** with tracked Undo, including offline |
| Created and edited stamps | stamps | shown `D/model/derive.rs:654` | shown `W/app.rs:1625` | shown `A/ui/Detail.kt:115` |
| Per-task activity (who created, edited, completed it) | stamps and local completion provenance; shared names, actions, relative times and ordering (`T/task_activity.rs`) | shown below attachments (`D/ui/shell/detail.rs`) | shown below attachments (`W/app.rs`, `DetailActivity`) | shown below attachments (`A/ui/Detail.kt`, `F/view.rs`) |
| Merge two tasks | `MergeTasks` (`C/command.rs`); open destinations, title search and confirmation shared in `T/intent/merge.rs` | wired: **Merge into…** from the task's context menu, with confirmation and tracked Undo (`D/ui/shell/menu.rs`, `D/ui/shell/undo.rs`) | wired: row context or detail task menu, with confirmation and tracked Undo (`W/app/task_actions.rs`) | wired: row long-press or detail task menu, with confirmation and tracked Undo (`A/ui/TaskActions.kt`, `F/engine/merge.rs`) |
| Close the detail | n/a | wired `D/ui/shell/detail.rs:752`, Esc | wired `W/app.rs:1699`, Esc | wired `A/ui/Detail.kt:81`, `:99`, Back |

### Undo, agents and activity

| Feature | Core / server | Desktop | Web | Android |
|---|---|---|---|---|
| Undo toast after completing | client-side | wired `D/ui/shell/undo.rs:376`, Ctrl+Z | wired `W/app.rs:1437`, Ctrl+Z | wired `A/ui/RoomPane.kt:509` |
| Exact undo through the server (compensating event, covers a recurring task's next occurrence) | `POST /events/{seq}/undo` (`S/api/mod.rs:142`); `SyncClient::undo_tracked` | wired `D/ui/shell/undo.rs:172`, `D/store.rs:147` | wired: compensates the whole completion, including the next occurrence (`W/backend.rs`) | wired: the toast holds the tracked completion; `F/engine.rs::undo_completion` reverses the whole event batch |
| Undo a reorder | same | wired | wired: tasks, steps, lists and collections (`W/app.rs`, `W/app/room_actions.rs`) | wired for tasks (`F/engine.rs::undo_task_move`) |
| Undo anything else (rename, edits, deletes) | `Restore` or event undo | wired through Activity for confirmed changes (`D/ui/shell/activity.rs`); toasts also undo changing Repeat, setting, moving or removing a reminder, adding or deleting a link, removing an attachment, deleting a task, a step, a tag, a list or a collection, and moving a task to another list or a list to another collection (`D/ui/shell/undo.rs:29`) | wired through Activity for confirmed changes; tracked toasts for repeat, reminders, tag membership, tag rename and delete, link edits, list moves, colours and other tracked deletions | tracked toasts for repeat, links, deletions, moves and reminders; per-event Undo for confirmed changes in Activity |
| Activity feed (all events, newest first, undo per event) | `GET /activity`; shared `SyncClient::activity`, `undo_activity`, `undo_result` and `activity_confirmed`; the shared pages, undos and wording in `T/activity.rs` (`Feed`, `Undos`, `undo_action`) | wired: day groups, older pages read as it scrolls or as the keyboard nears the end, per-event Undo (Enter on the selected change) with Undoing and refusals in plain words, Refresh, Ctrl+H (`D/ui/shell/activity.rs`, `D/model/activity.rs`) | wired: day groups, cursor paging and per-event Undo (`W/app/activity.rs`) | wired: drawer Activity room over the shared pager, batch Undo and Refresh indicator (`A/ui/Activity.kt`, `F/engine/activity.rs`) |
| Group or filter changes by agent | `updated_by:` in the query language; `GET /activity?agent=` for events | wired: sidebar agent rows open Activity filtered to that agent; Clear filter or the room menu's All changes removes it, and the room menu switches agents (`D/ui/shell/sidebar.rs`, `D/model/menus.rs::activity_menu`); also has the Agent activity smart list | wired: sidebar agent links open filtered Activity; also has the Agent activity smart list | wired: drawer agent summaries open Activity with the exact agent ID; Agent activity smart list remains separate |
| Agent marks on rows | `updated.by` | shown | shown | shown `A/ui/Rows.kt:91` |

### Server-side only (admin pages)

The owner manages these on the server's own pages (`S/web/mod.rs`) or through the local admin API
(`S/admin/mod.rs:222`). All three clients link to the admin landing page from Settings.

| Feature | Server page |
|---|---|
| Share links (create, rotate, revoke) | `/admin/shares` |
| Agent tokens and their per-list access | `/admin/agents` |
| Devices (list, revoke) | `/admin/devices` |
| Email ingest settings, audit log, passkeys | `/admin/ingest`, `/admin/audit` |

## What the server supports and no client offers

Every item below exists in `todo-core` and over REST (`POST /api/v1/commands`), and mostly over MCP too
(`S/mcp/tools.rs`). No client offers it.

- **Saved queries.** The read API serves smart lists (`GET /api/v1/smart-lists`) and queries
  (`GET /api/v1/tasks?q=`). Clients evaluate the same built-in queries locally. The core has no
  user-defined saved smart lists, so this is not a client gap.

## Gaps by priority

Sizes: **S** is under half a day, **M** is one to two days, **L** is more than two days.

### 1. Stubs visible on the desktop

| # | Gap | Where | Size |
|---|---|---|---|
| 1.1 | **Done in D1** (ellipsis wired, sliders hidden). The room header's sliders and ellipsis icons do nothing. Wire them, or remove them until they work. Sliders: sort and view options, such as showing completed tasks. Ellipsis: a room menu with rename, delete, pop out and copy a share link. | `D/ui/shell.rs:1349`, `:1379`, `:1380` | S to hide, M to wire |
| 1.2 | **Done in D2 on all three clients.** Quick-add Calendar and Flag set the pending date and priority, with typed/picked reconciliation and removable chips. Task detail's Due row reuses the same calendar on all three clients, opening on the saved date or today. | `T/intent/calendar.rs`, `T/intent/quick_add.rs` | — |
| 1.3 | **Done.** Repeat, Tags and Reminder rows are always present and editable. | `D/model/derive.rs:681`–`737`, `D/ui/shell/detail.rs:669` | M each |
| 1.4 | **Done in D1.** The hand-over rows' checkboxes are drawn but only select the task. Make them complete it, as on Android. | `D/ui/shell.rs:2315` | S |
| 1.5 | **Done.** "N couldn't sync" in the sidebar has no way to see or dismiss what failed. Android has a banner. | `D/model/derive.rs:636` | S–M |
| 1.6 | **Done.** The sidebar's agent rows look like rows but are inert. Make them open the activity feed filtered to that agent (see 2.3). | `D/ui/shell/sidebar.rs` | S, after 2.3 |
| 1.7 | **Done in D1** (the Priority row). You can set the priority only by clicking the `!` mark, and the mark is absent when the priority is none. Add a priority control to the detail pane, or keep the column clickable. | `D/ui/shell.rs:1813` | S |
| 1.8 | **Done in D1.** Deleting a step (no confirmation) and deleting a list (confirmation) can't be undone. | `D/ui/shell/detail.rs:473`, `D/ui/shell/sidebar.rs:695` | S |

### 2. Missing on the desktop (planned or supported by the core)

| # | Gap | Size |
|---|---|---|
| 2.1 | **Done on desktop, Android and web.** Delete a task or move it to another list from Task actions, with Undo. Desktop and web also offer a row context menu, Delete and a keyboard-accessible "Move to…" picker. | M |
| 2.2 | **Done.** Manage collections: new, delete, recolour; move a list between collections; recolour a list. | M |
| 2.3 | **Done.** Activity feed with per-event undo and an agent filter. The desktop, web and Android rooms are wired; all three share the pages, undos and wording in `todo-client` (`T/activity.rs`). | L |
| 2.4 | **Done.** Reminders: edit them and fire Windows toasts in the tray, Android local notifications, and browser notifications or in-app toasts while a web tab is open. | L |
| 2.5 | **Done.** Repeat presets, custom interval/unit/weekly days, clear and tracked undo. | M |
| 2.6 | **Done.** Tags: create, rename, recolour, delete with confirmation and undo, add to and remove from a task. | M–L |
| 2.7 | **Done.** Add and remove web links; upload attachments with the file picker or a drop onto task detail, and remove attachments with Undo. | — |
| 2.8 | **Done.** Pop-out list windows. | — |
| 2.9 | **Done for `.eml` files** on desktop and web. New Outlook works when it supplies a real `.eml` file. Classic Outlook's virtual `.msg` is not supported. | — |
| 2.10 | **Done in D1.** Settings: a link to the admin pages, Sync now. | S |
| 2.11 | **Done.** Velopack installer and auto-update. | — |
| 2.12 | **Done on desktop, web and Android.** Search for another open task across lists, confirm the merge and undo it as one change. | S–M |

### 3. What the web lacks relative to the desktop

| # | Gap | Size |
|---|---|---|
| 3.1 | **Done.** Quick-add previews a date with the shared parser. | S |
| 3.2 | **Done.** Edit or clear the due date from detail or with D. | S–M |
| 3.3 | **Done.** Choose from four priority levels in detail or cycle with P. | S |
| 3.4 | **Done.** Steps can be renamed, reordered and deleted. | S–M |
| 3.5 | **Done.** Rename, reorder and delete lists and collections from their sidebar or room menus. | M |
| 3.6 | **Done.** Reorder tasks in a list with Alt+Up/Down. | S |
| 3.7 | **Done.** Exact undo through `SyncClient::compensate_tracked`, including recurring completion and reorders across tabs. | M |
| 3.8 | **Done.** Hand-over rows select the task and can complete it from the checkbox. | S |
| 3.9 | **Done.** Not synced list with Dismiss and Dismiss all for the shown entries. | S |
| 3.10 | **Done.** Remember folded collections across reloads in `localStorage`. | S |
| 3.11 | **Done.** Upload attachments from the picker or by dropping files onto the task detail. The upload route accepts the owner's session with CSRF and a matching origin; removal offers Undo. | M |
| 3.12 | **Done:** task delete and move (2.1), collections and list organization (2.2), repeat rules (2.5), tags (2.6), web links (2.7), and reminder editing and alerts (2.4); Activity (2.3), attachments and task merges (2.12) are also wired. | — |

### 4. What Android lacks relative to the desktop

| # | Gap | Size |
|---|---|---|
| 4.1 | **Done.** Editable notes, including **Add notes** when empty. | S |
| 4.2 | **Done.** Open HTTP(S) sources; other sources have no arrow. | S |
| 4.3 | **Done.** Attachments: offline metadata, download, open or save, picker upload, remove with Undo. | M |
| 4.4 | **Done.** Exact completion undo through `undo_tracked`, including recurring tasks. | M |
| 4.5 | **Done.** Due date (a date picker), priority (four levels). | M |
| 4.6 | **Done.** Steps: rename, reorder, delete. | S–M |
| 4.7 | **Done.** Lists: rename, delete, reorder; collections: rename, reorder. | M |
| 4.8 | **Done.** A live date chip in quick-add, using the submit parser. | S |
| 4.9 | **Done.** Agent summary in the drawer. | S |
| 4.10 | **Done.** Reminder notifications: local alarms from the synced reminders, with Complete and Snooze 10 min, and a Reminder row that sets, changes and removes a reminder. | L |
| 4.11 | **Done.** Task actions, reminders (4.10), repeat, tags, links, collection management, list moves and colours, and task merges (2.12). The drawer opens paged Activity with per-event Undo, optionally filtered to an agent. | — |

## Shared gesture-to-command code

`todo_client::selection` shares the room-local selection model, the bulk actions and their toast text.
A batch applies one explicit action to at most 1,000 affected tasks, and compensates it as one command.
`todo_client::task_drag` maps a visible gap in an open manual list to a placement relative to a visible
sibling. It preserves the picked tasks' relative order, does not use hidden or completed rows as anchors,
and makes no change when the resulting visible order is unchanged. Smart rooms and collections are not
reorderable. Drops on another list prepend the affected tasks (in room order); My Day adds today's stamp
without changing their list. A target that would change nothing is not highlighted.

`todo_client::intent` now builds commands for all three clients without I/O. It owns the quick-add
parser, task destinations and room defaults, and the builders for lists, tasks and steps. Each client
maps its own room key to the shared task intent. This resolves the creation drift:

- Every new list gets a stable swatch from its ID.
- All three clients can create lists inside collections.
- New tasks are placed first in their list's manual order. A smart list may display its own sort order.
- A task moved to another list is placed first there too (`move_to_list`), and the list picker groups the
  lists as the sidebar does (`list_groups`).
- `intent::organize` builds the rest of the organizing: a new collection gets a swatch from its ID like a
  list, a list moves last into its new collection, colours are only the design's swatches, a typed link
  is checked by the core's own rules (`todo_core::decide::validate_link`), and tag names stay unique
  ignoring case, as the domain requires.
- `intent::reminder` picks the reminder a task's Reminder row shows and edits (the first from the local
  today, else the latest), builds the commands that set, move or remove it, and lists what a client
  should alert for (`alarms`). All three clients use it for editing and alerts. It also provides the web's
  delivery plan, occurrence key and snooze command.
- All three parse the same trailing date and priority, preview both as chips, and use the same draft to
  reconcile typed syntax with choices from the calendar or priority picker.

## Work package: list suggestion

The shared `todo_client::intent::suggest::ListSuggester` learns from live tasks, including completed
ones. Pass the title returned by `quick_add::parse`, not the raw composer text. The model keeps sparse
counts of hashed words, word pairs and character fragments, not training titles. It abstains unless
at least two live lists have three training tasks each, the folded title has a word of at least three
characters, and it recognizes a feature. The default confidence threshold is 0.9.
The model checks the winner against the current replica before suggesting it. UI integration in
My Day, Planned, Important and All is pending on every client; the current destination is unchanged.

To measure it locally, set `TODO_SUGGEST_SNAPSHOT` to a JSON replica snapshot (`State` or a record
with a `state` field) and run `cargo test -p todo-client --lib suggest_eval -- --ignored --nocapture`.
It prints only aggregate metrics for a repeatable five-fold split and a chronological holdout.
