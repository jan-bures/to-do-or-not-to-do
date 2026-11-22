# Android app

`apps/android` is the Android client: Kotlin and Jetpack Compose on top of the shared Rust core. The core
(`todo-client`, `todo-core`, `todo-design`) is reached through `crates/todo-ffi`, which exposes it with
[UniFFI](https://mozilla.github.io/uniffi-rs/). The Kotlin bindings are generated at build time, and
`cargo-ndk` builds the native library for each ABI.

What the first version does:

- **Pairing.** You type the server address (nothing is compiled in), and approve the phone with your passkey
  on any device. The device token is sealed with a key in the Android Keystore.
- **Lists.** My Day, the smart lists, collections and lists, with the room pills and the drawer. In the
  drawer, collections fold with their chevron. **+ New list** adds a list with a swatch and opens it. It
  creates a sibling from a list, a child from a collection, or a top-level list from a smart room. The
  footer shows the sync state ("Synced 10:35", "Offline, 3 waiting") with a settings button (the paired
  server, Sync now, Unpair).
- **Managing lists.** A long press on a list in the drawer opens its menu: **Rename**, **Move up**, **Move
  down** and **Delete list…**; a collection's offers **Rename**, **Move up** and **Move down**. A move
  that would go past either end is greyed out. The room's **⋯** button offers what applies to the room:
  in a list, Rename list, the moves, Show or Hide completed and Delete list…; in a collection, Rename
  collection, the moves and the Completed toggle; in My Day, Important, Planned and All, the Completed
  toggle. Completed and Agent activity have no menu, so no button. Renaming asks for the name in a dialog
  (a blank one can't be saved). Deleting asks first, in full, as the desktop does ("Delete the list
  “Errands” and its 4 tasks? Undo brings them back."), with **Cancel** first and focused. Confirming checks
  the question again (`TodoEngine::delete_list` takes the question the owner saw): if the list was renamed,
  or tasks synced into it or out of it while the question was open, nothing is deleted and the new
  question is asked, saying it changed. Deleting the open list goes to My Day, and the drawer closes so the
  toast's **Undo** is in view; it brings the list and its tasks back either way.
- **Agents.** Under the lists, the drawer shows the agents seen in the last seven days (at most four), each
  with its letter mark and what it did today ("2 added, 1 edited", "3 edited", "idle"), worded as the
  desktop's sidebar words it (`view::agents` in `todo-ffi`). The rows only tell; they aren't controls.
- **Filter.** The search button in the bar opens a filter under the room pills. It takes the desktop's query
  language (`tag:docs`, `due<=friday`, `priority>=high`), or plain words to find in titles. The rows narrow;
  the sun and the counts still measure the whole room. Done tasks that match stay under Completed. Opening a
  room, even the same one, clears the filter and closes its field.
- **Tasks.** Add, complete (with the completion choreography and undo, see *Motion*), rename, add to or remove from My Day, mark
  important (the star, as on the desktop: high priority), and add and tick steps.
- **Task actions.** Long-press a task row or tap **⋯** in its detail header. The menu offers Complete or
  Reopen, Add to or Remove from My Day, Mark as important or Remove importance, Priority…, Move to… and
  Delete task. **Move to…** shows lists grouped by collection, with the current list ticked. Type words
  from a list or collection name to filter; case and accents don't matter. The task moves to the front
  of the chosen list. In a manual list, **Move up** and **Move down** move an open task past its visible
  neighbour. **Delete task** doesn't ask first and closes that task's detail. All three changes offer
  **Undo**. If another device reorders the moved task before Undo, the change stays and the failed-change
  banner explains: “Undo moving a task: It was changed again since.”
- **Due date and priority.** In the detail pane, the **Due** row opens the date picker: the Material date
  picker in the room's colours and the tokens' type, on the overlay surface. It opens on the task's date
  (or today's month), takes a tap on a day or a typed date, and keeps it with **Save**. **Clear** takes
  the date away, and with it a repeat, which needs a date (`intent::set_due`); when the task repeats, a
  note under the picker says so. In a short window (the cover screen on its side) the picker drops its
  headline and mode toggle, and the calendar scrolls under the buttons, which stay in view. The **Priority** row opens a picker with all four levels, highest first
  (**High** `!!!`, **Normal** `!!`, **Low** `!`, **None**), the current one ticked. The star still sets
  high priority or none, as on the desktop.
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
  submit. A calendar chip previews a recognized date (for example, **Sun 22 Nov**) without changing the text. A trailing date can be
  `tomorrow`, `zítra`, `+3d`, a current or future ISO date, or a weekday (`Sunday`, `by fri`).
  Whole phrases work too: `in 2 weeks`, `za 3 dny`, `next week`, `příští pátek`, `this weekend`,
  `konec měsíce`, or a day and month such as `1 Dec` or `1.12.`. Past ISO dates stay literal. A separate `!!` or `!!!` sets priority. `See you \tomorrow`
  keeps `See you tomorrow` as the title. My Day adds today's My Day stamp, Important defaults to high
  priority and Planned defaults to due today. New tasks go first in their list's manual order. If no
  destination list exists, quick-add creates **Tasks**, inside the current collection when applicable.
- **The finished day.** My Day hands over to tomorrow, with **Plan <weekday>** (it opens Planned) and today's
  Completed section. With nothing due tomorrow there is no hand-over, but **Plan <weekday>** is still there.
- **Offline.** The core keeps a local SQLite cache and an outbox, so everything works offline and syncs
  when the network comes back. A chip in the bar shows how many changes are waiting.
- **Live updates.** While the app is in the foreground it holds the server's event stream open.
- **Layouts.** Folded (compact width) it is one pane, with the detail over the list. Unfolded it shows the
  list and the detail side by side, like the F2 inner-screen mockup. The header collapses while scrolling
  and while the keyboard is open. Light and dark follow the system.

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
  the open rows (entering an open Completed section, `ROW_ENTER`), and the counts (header, drawer, pills)
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
cards in place over the screen, at rest: the date picker (`due_date_picker`, drawn with today's ring off,
since the picker reads the device's clock for it), the priority picker, a step being renamed and a step's
menu (`step_actions`, with the toast after deleting one), a list's drawer menu, the room menu and the
rename dialog (`list_menu`), the delete question, also asked again (`delete_confirmation`), the task menu, Move picker and deleted task with Undo (`task_actions`), and the agent
summary; the date picker also on the cover screen's side (`due_date_picker_sideways_light`). The gallery
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
model, on the cover screen (412 × 920 dp) and the inner screen (900 × 1000 dp), light and dark. It
includes the collapsed header, a short window and 1.3× text. Look at it after every UI change; the
images are in `app/src/test/snapshots/images/`.

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
   variant's network security config says the same for the platform's network stack.
2. The app shows a code and a link. Open the link on a device where you can sign in with your passkey (or
   tap *Open link* to use the phone's browser), check that the code matches, and approve it.
3. The app moves on by itself. **Unpair this phone**, under the settings button in the drawer's footer,
   forgets the token, the Keystore key and the local cache.

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
