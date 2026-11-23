# Android app

`apps/android` is the Android client: Kotlin and Jetpack Compose on top of the shared Rust core. The core
(`todo-client`, `todo-core`, `todo-design`) is reached through `crates/todo-ffi`, which exposes it with
[UniFFI](https://mozilla.github.io/uniffi-rs/). The Kotlin bindings are generated at build time, and
`cargo-ndk` builds the native library for each ABI.

What the first version does:

- **Pairing.** You type the server address (nothing is compiled in), and approve the phone with your passkey
  on any device. The QR and status sit above the bottom actions. Address and invite fields scroll above
  the keyboard. The device token is sealed with a key in the Android Keystore.
- **Lists.** The bottom room pills show all siblings in a sideways-scrolling strip. Tap **Rooms** or
  swipe up on the bottom bar to open the room switcher. Its pinned **Find a room…** field searches
  names without regard to case or accents, including lists in folded collections. The six smart rooms
  sit above the filter; collections, loose lists, Activity, agents and Trash share the scrolling area.
  Collections fold with their chevron. **+ New → List** adds a list with a swatch and opens it.
  It creates a sibling from a list, a child from a collection, or a top-level list from a smart room.
  The pinned footer's sync status retries sync on tap. The settings gear opens a sheet with the paired
  server and a **Thumb-zone hand** choice. Left-handed mode mirrors the cover room bar and switcher
  footer, plus the cover detail action bar; the unfolded list stays on the left. This choice is stored on the phone, not synced. **Open admin pages** opens that server's `/admin` in the
  browser, where you can sign in with your passkey if asked. **Sync now** retries without waiting for the
  next automatic attempt; it shows **Syncing…** and is disabled while the engine is busy. The menu also
  offers **Devices…** and **Unpair this phone**.
- **Managing lists.** A long press on a list in the room switcher opens its bottom action sheet: **Rename**, **Move up**, **Move
  down**, **Move to…**, **Colour…** and **Delete list…**. The move picker lists the collections and **No collection**.
  A collection offers **Rename**, **Move up**, **Move down**, **Colour…** and **Delete collection…**. The switcher's
  **+ New → Collection** opens the collection after creating it. Both colour pickers use the twelve design swatches. A move
  that would go past either end is greyed out. The room's **⋯** button offers what applies to the room:
  in a list or collection, Rename, Colour…, the moves, Show or Hide completed and Delete…; in My Day, Important, Planned and All, the Completed
  toggle. Completed and Agent activity have no menu, so no button. The actions open in a bottom sheet;
  colour and move pickers replace its contents. Renaming asks for the name in a bottom-aligned dialog
  (a blank one can't be saved). Deleting asks first, in full, as the desktop does ("Delete the list
  “Errands” and its 4 tasks? Undo brings them back."), with **Cancel** first and focused. Confirming checks
  the question again (`TodoEngine::delete_list` takes the question the owner saw): if the list was renamed,
  or tasks synced into it or out of it while the question was open, nothing is deleted and the new
  question is asked, saying it changed. Deleting the open list goes to My Day, and the room switcher closes so the
  toast's **Undo** is in view; it brings the list and its tasks back either way.
- **Agents.** The switcher's **Activity** row sits below the lists and collections. Under it, indented, are the
  agents active today (at most four), each with its letter mark and what it did ("2 added, 1 edited today",
  "3 edited today"). The selection and wording are shared with desktop (`todo_client::activity`), and
  the phone supplies the actual bounds of its local day, including on days the clocks change.
  Tap one to open Activity filtered to its ID. Sections keep their shared order within each zone; Android
  groups the smart rooms at the bottom to keep them within reach.
- **Activity.** The switcher's Activity room reads the confirmed event log, newest first. Events are grouped
  by local calendar day and show the actor and relative time. Scrolling reads older pages. New events mark
  **Refresh activity •** without moving the reader. Switching agent or log epoch starts a new feed. If a
  change is too large, **Skip this change** continues at the server’s cursor. Undo names the event by
  sequence **and** event ID; “Undoing…” disables Undo for its whole batch. A refusal allows a retry and
  uses the shared plain wording, never the server message. The bottom bar opens Rooms, filters by agent
  and refreshes the feed; the sky remains read-only. `todo_client::activity::Feed` owns each room’s pages;
  the engine keeps `Undos` for the pairing, so switching agents or leaving Activity does not unblock a pending
  batch. Compose draws the day groups and row actions returned by `todo-ffi`. The Agent activity smart list is still a list of tasks.
- **Trash.** Open it in the switcher below Activity and its agents. Directly deleted tasks, lists and collections
  are ordered newest first. **Restore** returns an item to its original place and offers tracked **Undo**.
  A deleted list or collection must be restored first before a separately deleted child can return. Restoring
  a parent also reveals children hidden with it that were not deleted themselves. The bottom bar opens
  Rooms and shows the deletion count and any sync issue. Undo a merge in Activity
  to recover a merge source. Trash reads the local synced replica, has no automatic expiry or permanent-delete
  control, and does not change the server's separate attachment blob-retention policy.
- **Filter.** The search button in the bottom bar swaps the composer for a filter field right above the
  bar or keyboard. With a query active and the keyboard closed, **+** restores the composer without clearing
  the query. The filter takes the desktop's query
  language (`tag:docs`, `due<=friday`, `priority>=high`), or plain words to find in titles. The rows narrow;
  the sun and the counts still measure the whole room. Done tasks that match stay under Completed. Opening a
  room, even the same one, clears the filter and closes its field.
- **Tasks.** Add, complete (with the completion choreography and undo, see *Motion*), rename, add to or remove from My Day, mark
  important (the star, as on the desktop: high priority), and add and tick steps.
- **Select several tasks.** Long-press a task row, then tap other rows to add or remove them. The app
  bottom dock shows the count; **Clear selection** or system **Back** clears it. Changing rooms or filtering a row out
  drops its selection. With two or more tasks, the contextual controls offer Complete or Reopen,
  importance, My Day, Move to…, Set due… and Delete. Mixed selections offer the positive action and
  skip tasks already in that state. Move and Due open in bottom sheets; moved tasks keep their
  room order at the front of the destination. The unfolded detail pane shows the selected count.
  Each successful change clears selection and offers one **Undo** for all affected tasks, including
  any next occurrences created by completing repeats. The `multi_selection` gallery shows both themes.
- **Drag tasks.** Long-press a row to select it, then drag a selected row by its six-dot grip. All
  selected tasks lift together, with a count on the preview. In a manual list, drop on the insertion
  line to reorder the open tasks. A filter still allows reorder relative to visible rows; hidden
  tasks keep their relative order. Smart lists and collections don't show an insertion line. As tasks
  lift, the dock shows sibling room pills and **Rooms** as drop targets. Drop on a valid pill, or drag
  onto **Rooms** to open the switcher and drop on another list to prepend the tasks or on **My Day**
  to add them to today's plan. The list and switcher scroll near their edges. Only valid destinations
  highlight; letting go elsewhere cancels. A drop has one **Undo**. The detail menu still offers
  **Move up**, **Move down**, **Move to…**, and the My Day action without a drag. The `task_drag`
  gallery captures the lifted selection, insertion line and switcher highlight.
- **Swipe tasks.** Swipe a row at least 72 dp right to Complete or Reopen, or left to toggle My Day.
  A tick marks crossing the threshold; releasing short of it cancels. Both actions offer Undo. Swipes
  ignore touches that start within 24 dp of either outer edge and are off during bulk selection and drag.
  The row checkbox, TalkBack actions and detail controls work without a swipe.
- **Task actions.** The breadcrumb is read-only. The detail bottom bar holds Back or Close, Complete,
  My Day, Important and **⋯**, and hides while the keyboard is open. On the inner screen, detail sheets
  stay in the right pane. The actions sheet offers Complete or
  Reopen, Add to or Remove from My Day, Mark as important or Remove importance, Priority…, Move to…,
  Merge into… and Delete task. **Move to…** shows lists grouped by collection, with the current list ticked.
  In the bottom filter field, type words from a list or collection name; case and accents don't matter. The task moves to the front
  of the chosen list. In a manual list, **Move up** and **Move down** move an open task past its visible
  neighbour. **Delete task** doesn't ask first and closes that task's detail. All three changes offer
  **Undo**. If another device reorders the moved task before Undo, the change stays and the failed-change
  banner explains: “Undo moving a task: Couldn't undo: the task was changed again.”
- **Merge into…** finds other open tasks across lists. Filter by title in the bottom field (case and accents don't matter),
  then pick a destination and confirm. Its list, title, priority and schedule stay as they were; the
  source's notes, live steps, links, attachments and reminders move over, and tags are combined.
  If either task changes name or list while the question is open, the app rechecks it on confirm.
  **Undo** reverses the whole merge. The `merge_tasks` Paparazzi gallery shows the picker and confirmation.
- **Due date and priority.** Tap **Due** to open the shared quick-add calendar as a bottom sheet.
  It opens on the saved date or today. Choose **Today**, **Tomorrow**, **Next week** (next Monday), or
  any day in the Monday-first grid to save it immediately. **No date** removes the due date and any
  repeat rule (`intent::set_due`). For a repeating task, a note below the grid warns that **No date**
  also stops the repeat. Back or **Esc** closes without saving. On a hardware keyboard,
  arrows move by day or week, **Page Up/Down** move by month and **Enter** chooses. The **Priority** row
  opens a picker with all four levels, highest first
  (**High** `!!!`, **Normal** `!!`, **Low** `!`, **None**), the current one ticked. The star still sets
  high priority or none, as on the desktop.
- **Repeat.** Tap Repeat for Daily, Weekdays, Weekly, Monthly, Yearly, Custom… or Don’t repeat. Custom
  takes an interval, a unit and (for weeks) the days. `intent::repeat::set_repeat` gives an undated task
  its first matching due date in the same undoable edit. The row uses `describe_repeat`.
- **Tags and links.** Tags is always present. Pick a tag to add or remove it, or type a name in the
  sheet's bottom field to create and apply it. The chevron beside a tag offers Rename, Colour… and Delete tag…. Add link… accepts a web
  address through `intent::add_link`; a source opens in the browser, and its remove button offers Undo.
- **Attachments.** Task detail lists file names, type icons and sizes from the replica, even offline.
  **Add file** opens the system document picker. An upload goes directly to the paired server, not into
  the outbox. The app checks the 25 MiB ceiling while reading, even if the picker reports no size;
  a server with a smaller limit reports its own limit. **Save** downloads the complete file to private
  cache before copying to a destination you choose in the system file picker. Documents and images also
  have **Open**, which grants the viewer read-only access to that file through a `FileProvider`. Other
  extensions are save-only. Neither action requires a storage permission. Opening a file replaces its
  previous cached copy. Unpairing and process startup clear that cache: a viewer that tries to reopen
  a file after this app’s process has restarted may need you to open it from task detail again.
  Transfers stay with their task across rotation and navigation, but stop when the app goes into the
  background or is unpaired. If a Save fails or is cancelled, the app asks the document provider to
  delete the destination only if the provider reports it is empty. A completed Save is kept. Add file
  waits for a new task to sync; once the server accepts an upload, cancelling the sync wait does not
  cancel the upload. **Remove** works offline and offers **Undo**.
- **Task activity.** Below Attachments, **Activity** shows who created, last edited or completed the
  task, newest first. `todo-client` supplies the same labels and relative times as desktop and web
  through UniFFI; Compose uses the switcher's agent marks. A later edit doesn't replace the completer,
  and unknown historical completers aren't guessed. Unlike the switcher's Activity feed, this summary
  works offline and offers no Undo.
- **Delete confirmations.** For tags and collections the app checks the displayed name and impact again
  behind pending writes. If they changed, it asks again and queues nothing. Undo restores the exact deletion.
- **Reminders.** The **Reminder** row is always in the detail pane: "Today, 18:00", or "–" with none.
  The model words its time in the phone's time zone, at the offset of the reminder's own instant
  (`localReminder`), as the toast and the notification do: a reminder after the clocks change reads as
  set, not an hour off as the phone's offset today would put it. It
  opens the reminder sheet: the Material date picker and a 24-hour input just above the bottom actions,
  in the room's colours and the tokens' type. It opens on the reminder's own day and
  time; with none, on 09:00 of a later due date, else on the next whole hour. **Save** keeps a whole
  time still to come: not while an hour or minute is half typed, and not one that has passed (the engine
  refuses that too). **Remove** takes the reminder away, and the toast offers **Undo** for either,
  through the tracked undo, naming the time saved. A task can carry more than one reminder (an agent can
  add them); the row shows and edits one, the first from today (else the latest), as
  `todo_client::intent::reminder` picks it. In a short window the time comes first and the calendar
  scrolls under the buttons. See *Reminder notifications* below.
  - **An edit names its reminder.** Saving or removing carries the reminder the picker opened on, and the
    time it showed (`ReminderRef`). If that reminder moved or went while the picker was open (another
    device, an agent), or a task that had none got one, nothing is written, and the toast says so: "The
    reminder changed while the picker was open. Check it, then try again."
  - **When the clocks change.** The time is the phone's zone's wall clock on the day picked. A time the
    clocks skip (02:30 when they go forward at 02:00) doesn't exist, so the one the gap moves it to is
    saved, and a note under the picker says which ("Saves 03:30."). A time that happens twice (when they
    go back) saves the first, and the note says so. The toast names the time saved in the zone of the
    reminder's own day.
- **Steps.** Tap a step's title to rename it in place (Done or leaving the field keeps a changed title; a
  blank one is dropped). Its **⋯** button opens its menu: **Rename**, **Move up**, **Move down** and
  **Delete step**. A moved step and the one it swaps with glide to their places. Deleting a step shows the
  toast with **Undo**, which brings it back exactly as it was (its title, its tick and its place). On the
  cover screen the detail pane shows the toast strip itself, and its content scrolls clear of it. A new
  step the core refuses (over 500 characters, say) keeps its draft, and the reason shows under it in the
  detail pane. A step draft lives in the app's model: it survives a rotation (even with the step on its way, which is then
  added once), but not the process being killed, when a half-typed step is gone.
- **Notes and sources.** Tap the notes, or **Add notes** on an empty task. Enter inserts a new line;
  **Save notes** keeps the plain text (clear the field to remove it). Back, opening another task, or
  switching rooms also saves the edit. **Cancel** discards it. If saving fails after you leave, the app
  tells you; reopen the task to correct the draft.
  A source with a web address opens in the browser. Sources without an HTTP or HTTPS destination are
  text only, with no arrow.
- **Quick add.** The composer uses the [shared syntax](desktop.md#quick-add-syntax) while typing and on
  submit. Removable chips preview date (for example, **Sun 22 Nov**) and priority without changing the text. A trailing date can be
  `tomorrow`, `zítra`, `+3d`, a current or future ISO date, or a weekday (`Sunday`, `by fri`).
  Whole phrases work too: `in 2 weeks`, `za 3 dny`, `next week`, `příští pátek`, `this weekend`,
  `konec měsíce`, or a day and month such as `1 Dec` or `1.12.`. Past ISO dates stay literal. A separate `!!` or `!!!` sets priority. `See you \tomorrow`
  keeps `See you tomorrow` as the title. My Day adds today's My Day stamp, Important defaults to high
  priority and Planned defaults to due today. New tasks go first in their list's manual order. If no
  destination list exists, quick-add creates **Tasks**, inside the current collection when applicable.
  The Calendar button opens upward with a Monday-first month grid and, nearest the button, **Today**,
  **Tomorrow**, **Next week** (next Monday), and **No date**. The Flag button opens the same four priorities as the detail Priority row.
  A choice takes out the matching typed suffix and leaves everything else alone; removing a chip, or
  choosing **No date** or **None**, clears even a room default. The `quick_add_pickers` gallery shows
  both chips and both popovers in light and dark; task detail presents the same calendar as a sheet.
- **The finished day.** My Day hands over to tomorrow, with **Plan <weekday>** (it opens Planned) and today's
  Completed section. With nothing due tomorrow there is no hand-over, but **Plan <weekday>** is still there.
- **Offline.** Task and organization edits use a local SQLite cache and outbox, and sync
  when the network comes back. Activity needs a connection to read pages. The sky and switcher footer
  show sync trouble or waiting changes. Failed edits and sign-out actions sit right above the dock.
- **Live updates.** While the app is in the foreground it holds the server's event stream open.
- **Layouts.** Folded (compact width) it is one pane, with detail over the list. Unfolded it puts the list
  and its sheets in the left pane, clear of a separating hinge, with detail to the right. The read-only sky
  collapses to a 36 dp strip while scrolling. On the cover, pull down from the top of the list to bring
  tasks closer; release after a long pull to leave them there, or tap the stretched sky to reset. While
  typing, the room bar hides and the header collapses to leave more space for rows. Light and dark follow
  the system. See [the thumb-zone specification](android-thumb-zone.md) for the postures and Fold7 gallery.

## Home-screen widget

Long-press an empty spot on the home screen, open **Widgets**, and add **To-do list**. Pick **My Day**,
**Important**, **Planned**, **All**, a list, or a collection (a group of lists). Each widget remembers its
own selection. Tap the name or chevron to pick again; on wider widgets **Widget settings** does the same.
A deleted list or collection offers to pick another. A signed-out widget says **Open the app to sign in**.
If the server signs the phone out, widgets hide task and list names and refuse task edits, even after an
offline restart. A 401 from sync, Activity, an upload or a download latches immediately. The native
engine writes an empty `todo.sqlite.revoked` marker beside that pairing's cache; the app also honors
its existing preference marker, and keeps the pairing blocked while retrying a failed write to
preferences. The outbox stays intact until the owner confirms **Pair again**.

The widget shows up to 100 open tasks in the app's order. Collection rows name their source list.
Scroll the rows without moving the header, or resize the widget to see more rows at once.
Tap the title to open its task, the round checkbox to complete it, or the star to change importance
(filled with the room's strong accent when on). **+** opens the app's composer in the selected room,
with that room's usual defaults. Widget edits use the same offline outbox as the app. Glance hands the
completed row straight back to the launcher without the in-app exit animation.

**Refresh** redraws from the local cache and requests a short sync. The widget also follows engine changes,
including syncs triggered by reminders. While widgets are installed, the launcher requests a refresh about
once an hour (Android may defer it); a single network-constrained WorkManager job catches up for at most
four seconds once the engine is ready. It can finish earlier once that connection has received the
server's backlog and the outbox is empty. No push or continuously running background service is added.

The rounded card follows the system's light/dark theme and the room's `todo-design` palette, not wallpaper
colours. Glance renders text in the system's corresponding font families at the token sizes. The launcher
receives task titles and row metadata for the scrollable list. The app stores only the selected room key
per widget, not a copy of the tasks or pairing credentials. `WidgetGalleryTest` renders the real Glance RemoteViews to Paparazzi
snapshots (`app/src/test/snapshots/images/app.todoornot_WidgetGalleryTest_*.png`).

## Reminder notifications

The phone alerts for the synced reminders with local alarms. Nothing comes from the server: whatever
device sets a reminder, the phone sees it after a sync and sets its alarm.

**The alarms come from the last synced state.** There is no push. The app syncs in the foreground;
a widget can also request a brief background sync. A reminder added, moved or removed elsewhere (or its task completed there) while the app is in
the background is seen only at the next sync: until then an alarm can be missing, at an old time, or
for a reminder that has gone. To narrow that, when an alarm goes off with the app in the background, the
app first syncs for up to four seconds (until it's caught up, or the network is found to be down), shows
nothing meanwhile, and then shows only what is still due: a reminder removed or moved elsewhere, or a
task completed there, isn't shown. A reminder added elsewhere still waits for the app to sync.

- **What gets an alarm.** `TodoEngine.reminderAlarms` lists the live reminders of open tasks
  (`intent::reminder::alarms`). `reminders/ReminderPlan.kt` decides, purely, which alarms to set or cancel
  and which notifications to show or hide, and `reminders/Reminders.kt` applies it and remembers what it
  did (reminder IDs and times only, never task content) in the app's private preferences. Every change the
  engine reports runs it again, so a reminder removed, or a task completed or deleted, here or on another
  device, loses its alarm and its notification, and a moved reminder's alarm moves.
- **Exact or not.** Every reminder has an inexact alarm (`setAndAllowWhileIdle`), which Android may
  deliver a few minutes late. With *Alarms & reminders* allowed it also has an exact one
  (`setExactAndAllowWhileIdle`). Taking the permission away cancels the exact alarms without a word to
  the app, so the inexact one still goes off; whichever goes off first shows the reminder, and the other
  finds it shown.
- **Set again.** After a reboot or an app update, a clock or time-zone change, or the exact-alarm
  permission being granted, `ReminderReceiver` sets every alarm again; so does the first pass after the
  process starts, as a reboot clears them, and any pass that finds the permission granted or taken away
  since the last (each pass checks `canScheduleExactAlarms`). A pass runs after every change the engine
  reports and whenever the app comes to the foreground.
- **The notification** is on the *Reminders* channel: the task's title, when it was for, and two
  actions. **Complete** completes the task through the app's own command
  (`TodoModel.completeFromNotification`, the checkbox's `setDone`), so it syncs. The notification goes
  only once the completion is in the outbox, on disk; if the cache can't be opened, it stays and nothing
  else happens. With the app in the background it then syncs to send it. A broadcast's work (waiting for
  the engine, the write, the sync) has one deadline of eight seconds, inside Android's limit for a
  receiver. **Snooze 10 min** puts the
  notification away and shows it again ten minutes later. A snooze is this phone's own: it is never synced
  and doesn't move the reminder (a repeating task's next reminders stay where they were). Tapping the
  notification opens the task.
- **Missed reminders.** A reminder whose time has come is shown once for that time, whether the alarm
  went off or the app found it due when it started or synced. One missed while the phone was off, or that
  synced in late, is shown once if its time is within the last 24 hours, and never if it is older. A
  reminder counts as shown only when it could be: if notifications are off for the app (or the
  *Reminders* channel is), it waits, and shows once they are allowed, still within the 24 hours. What was
  shown is written down, by occurrence (the reminder and the time), before the notification goes up, and
  kept for 24 hours whatever becomes of the reminder: one removed and brought back by Undo never shows the
  same time again. After the 24 hours the app forgets it, and takes its notification away if it is still
  up, as it could no longer take it away when the task is completed.
- **Permissions, asked when they matter.** Never at launch: right after the owner sets a reminder, the app
  says why it needs notifications (Android 13 and later), then Android asks. A later reminder asks for
  exact alarms the same way (Android 12 and later), opening the system's page. Each is asked once; **Not
  now** (or Back) is kept.
- **Unpairing** cancels every alarm, and every notification on the *Reminders* channel.
- Log lines about reminders are counts, at debug level; never task content.

`RemindersTest` checks the policy against the fixture engine with a fake `AlarmSink` and a clock the test
sets: which alarms are set, moved or cancelled, what is shown once, snoozes, missed reminders, a restart,
exact alarms taken away, notifications off, the catch-up sync, Undo after removing, unpairing, and the
picker's times in a zone whose clocks change (Europe/Prague).

## Motion

The motion follows `docs/design-language.md` (Motion, moments 1 and 2) and moves as the desktop's does. Its
timing and state live in a pure model, `model/Motion.kt` (the same model as the desktop's
`src/model/motion.rs`), driven by a monotonic clock and unit-tested in `MotionTest`, with a randomized
test that any sequence of taps, changes from elsewhere, deletions, ghosts and room switches stays
consistent and comes to rest. `model/MotionHost.kt` holds it for the screen: the composables sample it at the frame's
time through `LocalMotion`, and it asks for frames only while something moves. Every duration, curve and
cue comes from the generated `Tokens.Motion` (`motion.durationsMs`, `motion.easings` and `motion.cues` in
`tokens.json`).

- **Completing a task.** The command is written at once (the toast offers Undo). The disc pops in
  (`CHECK_FILL`, `POP`), the tick and the title's strike-through draw from the left as the title dims
  (`CHECK_TICK` after `CHECK_TICK_DELAY`), the row holds still (`COMPLETE_HOLD`), then fades and collapses
  (`ROW_COLLAPSE`, `EXIT`) while the rows below rise to close the gap. Only then is it released: it leaves
  the open rows (entering an open Completed section, `ROW_ENTER`), and the counts (header and switcher)
  change and the sun travels to the new count (`SUN_TRAVEL`, `SUN`). Until the release the Rust view
  model counts it as open: each read passes the rows in their choreography (`RowPlacement`) to
  `TodoEngine.roomPlaced` and `homePlaced`.
- **Reopening** a completed task mirrors it: the disc clears (`OVERLAY_OUT`), the row collapses out of its
  completed place (it counts as done until then) and enters the open rows. Undo plays the reopening
  motion but reverses the exact completion through `SyncClient::undo_tracked`. If it can cancel the
  unsent command it does; otherwise the outbox sends a checked inverse, including removal of a recurring
  task's generated next occurrence. A conflicting edit is not overwritten: the failed-change banner
  explains why Undo could not be applied.
  The Completed and Agent activity lists show done tasks among their rows: there a row plays its choreography
  where it is, and enters there too. The placement only ever keeps a task's done state, by exact completion
  time (`TaskRow.completedAt`); rows carry no device or actor stamp for it (the agent badge still names the
  agent that last touched a task).
- **Ghosts.** A read can drop a row while it plays, or while it settles, for a reason other than its own
  done state: a toggle makes the owner the task's last editor, so Agent activity, or a filter such as
  `updated_by:agent`, no longer lists it. The app then keeps a ghost (`Motion.ghost`): the row's last
  `TaskRow`, painted at its last place (under the nearest row it was painted under that is still drawn, so
  ghosts that come and go never reorder the rows), which plays the rest of its choreography from there. A
  row that drops out while it settles because the data changed (an agent completed it, and the filter no
  longer matches) becomes a ghost too, rather than snapping out. A completing or
  reopening ghost collapses out as the row would have and is gone at its release (it enters nowhere); a
  reversed one settles, then collapses out (`ROW_COLLAPSE`, `EXIT`; a `REDUCED_FADE` fade with reduced
  motion) and is gone. Ghosts are paint only: a tap on one does nothing (the Undo toast and the detail
  pane still reverse it), accessibility skips them, and they never count towards the header, the counts or
  the sun. The empty line waits until the last ghost has gone. If a fresh read lists the task again (an
  agent changed it once more), that read lists it as it is, not placed, so the ghost goes at once and the
  live row shows its true state, at rest. Switching rooms ends every ghost.
- **Refused changes** show in the banner in the app's own words (`todo_client::failure`, the desktop's
  wording): what the change was and why it was refused, from the rejection's code. The server's message,
  which can be technical or quote task content, never reaches the screen.
- **Undo for deletions.** Deleting a task, a step or a list goes through the same tracked path as a completion
  (`SyncClient::submit_tracked`, then `undo_tracked` from the toast): an unsent delete is cancelled, a sent
  one gets a checked inverse. A refused undo shows in the banner in plain words, naming what it reverses
  ("Undo deleting a step: Couldn't undo: the step was changed again."), never the server's text. If what
  the item belongs to was deleted elsewhere meanwhile, the undo still brings back exactly the item, and the
  toast says why it stays hidden ("Step restored, but its task was deleted elsewhere.").
- **Menus and dialogs** (moment 12). A menu opens under its button or row (over it, when there's no room
  below), fading in and growing from `OVERLAY_FROM_SCALE` over `OVERLAY_IN` (`ENTER`) as it moves its last
  `OVERLAY_SHIFT` away from the anchor, and fades out to `OVERLAY_TO_SCALE` over `OVERLAY_OUT` (`EXIT`).
  The chosen action runs at once; the menu only leaves after, and takes no more taps while it does (a
  double tap on Delete step deletes once). A dialog's scrim is the token's (`elevation.scrim`, in the
  room's hue, never blurred), not the window's dim: it fades with the card, which grows from
  `OVERLAY_FROM_SCALE` and rises `DIALOG_RISE`, and drops `DIALOG_DROP` as it leaves; its buttons are
  one-shot too. Menus and the date picker wear the one overlay shadow (`elevation.overlayShadow`, with
  a `line-strong` border in dark mode). A reorder (a step, a list or a collection moved from its menu)
  glides the two rows it swaps over `ROW_ENTER` (`STANDARD`); a change from elsewhere moves nothing. These
  are Compose animations, so the animator scale stretches them, and at 0 they snap.
- **Interrupting.** A second tap part way reverses the row from where it is, in place, and nothing is
  released. Until the engine has the reversal written, the reads still place the task as the tap asked, even
  if the write lands after the row has settled, so a read in between never moves the row or changes the
  counts (the hold goes when the write lands or is refused, when the task is gone, or on switching rooms;
  a read that finds the task gone lets go only of the toggle it saw, never of a newer one); a read taken
  with a placement the motion has since changed is never published. When a reversed row has settled and its
  write has landed (in either order), the next read decides: where the room still lists the row, it simply
  comes to rest; where it doesn't, it becomes a ghost and collapses out. It never snaps out. A change from
  elsewhere (another device, an agent, a refused command) settles a choreography the same way; a deleted
  task stops at once, and so does a row in a section that isn't shown (a closed Completed); switching rooms
  (or creating a list, which opens it) puts everything at rest. A row that has gone (or hasn't arrived)
  takes no taps.
- **Room generations.** Every way into a room starts a new generation (`UiState.generation`), and so does a
  read where the engine resolves the room to another (a sync deleted the open list: My Day). A row tap and
  the detail pane's checkbox carry the generation they were read in; Undo uses the one on screen. A toggle
  from the room before, in the moment before the new room's rows arrive, is written (with its toast), but it
  starts no choreography and places, holds or paints nothing in the new room.
- **The sun** eases from where it is to wherever the count puts it, whatever changed it (a sync too),
  retargeting mid-travel. A new room places it without travel.
- **Dusk.** Once My Day's last row has left, the header grows into the dusk scene over `DUSK` (`SUN`): the
  dusk sky and scene fade in over the strip (whose sun travels on from where it is to the end of its arc), the text switches to the night's ink and the moon and stars
  come out (in batches, `STAGGER` apart) from `DUSK_NIGHT_AT`, and tomorrow's hand-over enters from
  `DUSK_HANDOVER_AT`, its first items `STAGGER` apart. A hand-over item takes taps only once it has begun
  to appear. Reopening or adding a task lifts the dusk again over `ROOM_HUE`.
- **Reduced motion and the animation scale.** The system's animator duration scale
  (`Settings.Global.ANIMATOR_DURATION_SCALE`, read whenever a transition starts) stretches or shrinks every
  animation, as it does everywhere else on the phone; the hold and the undo window (and its countdown
  line) keep their length. At 0 (*Remove
  animations*) the reduced variants apply (`motion::reduced`): the checkbox and the strike switch at once,
  the title dims and the row fades in `REDUCED_FADE` but never slides, the sun jumps, and dusk crossfades
  while the header takes its new height at once.
- **Idle is idle.** At rest no frame callback is pending: the driver waits for the next change; a row
  holding still waits on one timer, which asks for a frame when it runs out. The driver decides from the
  time the last frame sampled, not the clock, so the last frame drawn is always the end (a frame that
  samples just before dusk ends is followed by one more). The landscape's geometry is built once per size and scene, never per
  frame.

The gallery draws everything at rest, and freezes the motion at chosen moments (`GalleryTest.motion_frames`:
the fill, the hold, the collapse half way, the sun half way, dusk half way and the hand-over entering).
Popups and dialogs are windows of their own, which Paparazzi can't draw, so the overlay shots draw their
cards in place over the screen, at rest: the shared calendar (`detail_due_calendar`, on a repeating task
and an undated task in both themes), the priority picker, a step being renamed and a step's
menu (`step_actions`, with the toast after deleting one), the rename dialog (`list_menu`), the delete
question, also asked again (`delete_confirmation`), the task menu, Move picker and deleted task with Undo (`task_actions`), the
reminder picker, set, new and sideways (`reminder_picker`, drawn in UTC+1 with today's ring off), the
Reminder row set and empty (`reminder_row`) and the permission rationale (`reminder_permission`). The
inline sheets are drawn directly, including the switcher with Activity and its agents (`agent_summary`),
the room and list actions (`list_menu`), bulk pickers (`multi_selection`), and Settings with idle and
busy sync and both hand choices (`settings_menu`). The `organize` gallery covers the repeat and tag rows, pickers, confirmations, links, collection actions, undo surfaces and Activity in both themes. The gallery
is drawn in US English (`en-rUS`) whatever the machine's locale, as the date picker words the month.

Every size, colour, type style and duration comes from `todo-design`. Gradle generates `Tokens.kt`, the icon
paths, the font resources and the window colours from it, so nothing is copied by hand.

The launcher icon is Daybreak, from `assets/brand/` (see its README). Gradle converts the adaptive
background and foreground SVGs, and the flattened monochrome one (for themed icons), into VectorDrawables.
minSdk is 29, so every launcher takes the adaptive icon and there are no legacy bitmaps. The sun and the tick
stay inside the 66 dp safe circle, so no launcher mask (circle, squircle, rounded square) cuts them; a unit
test checks that against the SVG, and the gallery draws the icon under each mask.

## Building

You need:

- JDK 21 (CI pins Temurin 21.0.11+10);
- the Android SDK with platform `android-37.2` and an NDK (set `ANDROID_HOME`, or `sdk.dir` in
  `apps/android/local.properties`, which git ignores). CI pins NDK 30.0.16248370 and points cargo-ndk at
  it with `ANDROID_NDK_HOME`; do the same if you have several NDKs installed;
- the Rust targets and `cargo-ndk`:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
```

Then:

```sh
cd apps/android
./gradlew :app:assembleDebug                 # app/build/outputs/apk/debug/app-debug.apk
./gradlew :app:testDebugUnitTest             # unit tests, including the Rust core through JNA
./gradlew :app:verifyPaparazziDebug          # the screenshot gallery against the checked-in images
./gradlew :app:recordPaparazziDebug          # re-record it after a deliberate UI change
```

`-Ptodo.abis=x86_64` builds only one ABI, which is quicker while working with the emulator. The default
builds `arm64-v8a` and `x86_64`.

The unit tests load a host build of `todo_ffi` from `target/debug`, which Gradle builds first. The
gallery draws the real screens from invented fixture data (`TodoEngine.fixture`) through the real Rust view
model. The thumb-zone gallery uses Fold7 cover (412 × 960 dp) and inner (876 × 970 dp) configurations;
older fixtures also use the F2 sizes (412 × 920 and 900 × 1000 dp). It includes reachability, the
collapsed header, filtered and mirrored switchers, both themes, a short window and 1.3× text. Look at it
after every UI change; the
images are in `app/src/test/snapshots/images/`.

Paparazzi does not run the system keyboard. On a Fold7 or equivalent emulator, check that the room and
detail action bars hide while typing, and that **Rooms → New → List** and **Tags** keep their focused
fields above the keyboard in portrait and landscape. Press **Done** in the list-name field to confirm
the new list opens. Check predictive Back at both outer edges with rows on screen, and open a detail
sheet unfolded to check it stays clear of the hinge.

CI builds the debug APK and runs the unit and screenshot tests. It uses no signing secrets: the debug APK
is signed with the SDK's debug key, and the APK is uploaded as a build artifact.

## Sideloading

1. Turn on developer options and USB debugging on the phone.
2. `adb install -r apps/android/app/build/outputs/apk/debug/app-debug.apk`

`-r` keeps the app's data, so the pairing survives an update. Uninstalling the app removes the pairing,
because the Keystore key goes with it.

## Pairing

1. Open the app and type your server's address, for example `https://todo.example.com`. A release build
   talks HTTPS only. A debug build also accepts plain `http://` to the device itself (`localhost`,
   `127.0.0.1`), which is what the emulator setup below uses. The Rust core enforces this (the build
   variant passes the allowance in, because the core is always compiled in release mode), and each
   variant's network security config says the same for the platform's network stack. That config
   has one exception: the CRL and OCSP responder hosts of the CAs that issue Cloudflare's edge
   certificates (Let's Encrypt, Google Trust Services, SSL.com, and Sectigo as the backup). Android fetches OCSP responses and CRLs over plain http when it
   checks the server certificate, and if that fetch is blocked, the verifier reports the certificate as
   revoked.
2. The app shows a code, a QR code and a link. Scan the QR from an already paired phone, type the
   code into **Approve a device** on a signed-in app, or open the link on a device where you can sign
   in with your passkey (or tap *Open link* to use the phone's browser). Check that the code matches,
   then approve it.
3. The app moves on by itself. **Unpair this phone**, under the settings button in the switcher footer,
   forgets the token, the Keystore key and the local cache.

Alternatively [use an invitation](device-pairing.md): scan its QR code on the signed-out screen or
enter its code with the server address. An already paired phone can also issue invites and approve
requests from **Rooms → Settings → Devices…**. Camera access is optional and requested only when scanning.

**Unpairing always asks first.** The question says how many changes haven't reached the server; with any,
the button reads **Unpair and discard**, and Cancel is the default. The same question guards **Pair again**
(shown when the server has signed this phone out) and the unpair button on the can't-open screen. The
count is checked again when you confirm, after any edit still being saved; if it has changed, the question
says so and asks again.

Each pairing keeps its cache in its own directory, `pairings/<pairing ID>/`, so a new pairing never opens
an old cache. Unpairing first records `pairings/<pairing ID>.signed-out`, then forgets the token and its
key, then removes the directory, the record last. Whatever a crash or a file still in use leaves behind is
removed at the next start, before any pairing opens a cache. If `pairings/` or a pairing's directory is
ever a link or not a directory, the app refuses to open, write or remove anything through it. A cache left
directly in the app's directory by a pre-release build (`todo.sqlite`, from before pairing directories) is
not migrated: the app leaves it alone and syncs into a new cache.

If the local cache can't be opened, the app never unpairs by itself. A damaged cache is set aside as
`todo.damaged-<ms>.sqlite` in the pairing's directory, and its unsent changes are carried into a new one, as on the desktop. If even
that can't finish (a full disk, say), the app keeps the pairing and shows **Try again**, with unpairing
offered only as the owner's choice.

## Testing on an emulator

This runs the whole loop (pairing, sync, live updates, offline and reconnect) against a **throwaway local
server**, never a real one. Nothing in it needs a test-only path in the server: the owner side is a real
browser with a virtual passkey.

### The emulator

Create a foldable AVD from the Pixel Fold profile once:

```sh
sdkmanager "system-images;android-36.1;google_apis_playstore;x86_64"
avdmanager create avd -n Foldable_API_36.1 -d pixel_fold \
  -k "system-images;android-36.1;google_apis_playstore;x86_64"
```

Run it headless, so it stays out of the way, and stop it when you're done:

```sh
emulator -avd Foldable_API_36.1 -no-window -no-audio -no-boot-anim
adb wait-for-device
adb emu fold          # cover screen
adb emu unfold        # inner screen
adb emu kill
```

After folding or unfolding, wake the screen and bring the app back if it went to the lock screen:
`adb shell input keyevent 224`, `adb shell wm dismiss-keyguard`, then start the app again.

### A throwaway server

Give it its own data directory in a temporary folder and a config like this one:

```toml
bind = "127.0.0.1:47390"
data_dir = "<temp>/e2e/data"
time_zone = "Europe/Prague"
public_origin = "http://localhost:47390"
```

```sh
cargo run -p todo-server -- --config <temp>/e2e/server.toml serve
adb reverse tcp:47390 tcp:47390
```

`adb reverse` makes the emulator's `localhost:47390` reach the server on the host, so the app pairs with
`http://localhost:47390`. This needs the debug build. (The emulator also reaches the host at `10.0.2.2`,
but even a debug build refuses plain http to anything but the device itself.)

### The owner side

The server is claimed and pairings are approved with a passkey, so a script drives a browser instead:
[Playwright](https://playwright.dev/) with Chrome and a WebAuthn **virtual authenticator** from the Chrome
DevTools Protocol (`WebAuthn.enable`, then `WebAuthn.addVirtualAuthenticator` with `ctap2`, `internal`,
resident keys and user verification). With it, the script:

1. opens `/setup` and enters the one-time setup code the server wrote to its data directory, which
   registers the virtual passkey as the owner's;
2. for each pairing code the app shows, opens `/pair?code=<code>`, signs in when asked, and approves.

To make changes from "another client" (the live-update test), pair a second device the same way through
`POST /api/v1/pair/start` and `/api/v1/pair/poll`, and use its token with the REST API. Keep the token in
the temporary folder.

### What to check

- Pairing with the code; the lists appear.
- Adding a task in the app; it reaches the server.
- Completing a task: the check, the toast with undo, the row leaving.
- A change through REST shows up in the app within a moment (the event stream).
- Offline: `adb reverse --remove-all`, complete and add tasks, see the waiting count in the bar; restore
  the reverse and see them reach the server.
- Renaming in the detail (Enter or the keyboard's Done commits).
- With the keyboard open, folded and unfolded: the header is collapsed to the bar and at least four rows
  stay visible above the composer.

Screenshots from a specific screen of the foldable need its display ID:

```sh
adb shell dumpsys display | grep uniqueId     # local:<inner-id>, local:<cover-id>
adb exec-out screencap -p -d <display-id> > shot.png
adb shell cmd uimode night yes                # dark; "no" for light
```

Afterwards: stop the server, `adb reverse --remove-all`, `adb emu kill`, and delete the temporary folder.
