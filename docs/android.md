# Android app

`apps/android` is the Android client: Kotlin and Jetpack Compose on top of the shared Rust core. The core
(`todo-client`, `todo-core`, `todo-design`) is reached through `crates/todo-ffi`, which exposes it with
[UniFFI](https://mozilla.github.io/uniffi-rs/). The Kotlin bindings are generated at build time, and
`cargo-ndk` builds the native library for each ABI.

What the first version does:

- **Pairing.** You type the server address (nothing is compiled in), and approve the phone with your passkey
  on any device. The device token is sealed with a key in the Android Keystore.
- **Lists.** My Day, the smart lists, collections and lists, with the room pills and the drawer. In the
  drawer, collections fold with their chevron, **+ New list** adds a list, and the footer shows the sync
  state ("Synced 10:35", "Offline, 3 waiting") with a settings button (the paired server, Sync now, Unpair).
- **Filter.** The search button in the bar opens a filter under the room pills. It takes the desktop's query
  language (`tag:docs`, `due<=friday`, `priority>=high`), or plain words to find in titles. The rows narrow;
  the sun and the counts still measure the whole room. Switching rooms clears it.
- **Tasks.** Add, complete (with the check animation and undo), rename, add to or remove from My Day, mark
  important (the star, as on the desktop: high priority), and add and tick steps.
- **The finished day.** My Day hands over to tomorrow, with **Plan <weekday>** (it opens Planned) and today's
  Completed section.
- **Offline.** The core keeps a local SQLite cache and an outbox, so everything works offline and syncs
  when the network comes back. A chip in the bar shows how many changes are waiting.
- **Live updates.** While the app is in the foreground it holds the server's event stream open.
- **Layouts.** Folded (compact width) it is one pane, with the detail over the list. Unfolded it shows the
  list and the detail side by side, like the F2 inner-screen mockup. The header collapses while scrolling
  and while the keyboard is open. Light and dark follow the system.

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
