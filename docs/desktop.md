# Desktop app

`apps/desktop` is the GPUI desktop client. It is Windows-only for now: on other platforms the binary builds
and prints that it isn't supported yet. It pairs with the owner's server, syncs through `todo-client`, keeps
a local cache so it starts with data on screen, and shows and edits the real lists and tasks in the F2
"Playful" layout. Around that sit the tray, the window lifecycle, the global quick-add hotkey and
autostart.

## Building and running

```sh
cd apps/desktop
cargo run --release                    # the app; --background starts with the main window in the tray
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
pwsh scripts/gallery.ps1               # screenshot gallery, see below
```

### Its own workspace

`apps/desktop` is a separate Cargo workspace with its own `Cargo.lock`. The root workspace excludes it.

- **GPUI is pinned to a Zed git revision** (`2f808b3c`, `gpui` 0.2.2), and it brings a large dependency
  tree. Keeping it out of the root lock means the server and the shared crates never resolve against it.
- **It patches crates**: a vendored `gpui_windows` (below) and `async-task` (a waker identity fix,
  smol-rs/async-task#108, that GPUI needs). Patches apply to a whole workspace, and the server shouldn't
  carry them.
- **The Linux CI jobs stay as they were**: `cargo … --workspace` at the root doesn't see the desktop app. A
  separate Windows job builds, lints and tests it.

It depends on `todo-design` (with `embed-fonts`), `todo-core` and `todo-client` (with `sqlite` and `assistant`) by path. The
shared crates stay portable, so that works from this workspace too. Its tests also use `todo-server` by
path, as an in-process server.

The toolchain comes from the root `rust-toolchain.toml` (1.98.1), which is pinned to what GPUI needs.

## Windows installer and updates

### Build an unsigned release

On a Windows x64 build machine, install PowerShell 7, the MSVC x64 build tools, the pinned Rust toolchain,
Python 3 and a .NET SDK/runtime compatible with the pinned `vpk` tool. From a clean checkout with full history:

```powershell
git fetch origin
pwsh -NoProfile -File apps/desktop/scripts/package.ps1
```

The script builds the locked desktop workspace and installs the matching `vpk` .NET tool under
`apps/desktop/target/desktop-tools/` without admin privileges. It packages the app with the Daybreak icon.
The result in `apps/desktop/target/desktop-release/` contains `todo-desktop-win-Setup.exe`,
`releases.win.json` and the full `.nupkg`. To make a local delta, seed `-ReleaseDir` with the previous
feed and its full package. The GitHub publishing script does that automatically. Keep the full package
even when a delta exists: installs that cannot apply a delta fall back to it. No command here runs Setup.exe.
Packaging requires `HEAD` to be on the history of `origin/main`. For local test builds and pull-request CI,
`-AllowUnmerged` skips that check. Use a separate `-ReleaseDir` for test builds and never publish them.
CI builds and scans the unsigned installer and packages without uploading them.

The Velopack version is **`<desktop Cargo.toml version>-build.<Git first-parent commit count>`**. For
example, `0.1.0-build.10` follows `0.1.0-build.9`: the final component is numeric, not a string or SemVer
build metadata. Publish only reviewed commits on `main`, advance it by fast-forward only, and never lower
the crate's `major.minor.patch`. Every new main commit then has a greater version. Rebuilding the same
commit is not a new release. Never replace the bytes of a published package; to roll back a client change,
revert it on `main` and publish that as a new version. Use the filename or `releases.win.json` for the
packaged version; the executable's `--version` still reports the Cargo version.

### Publish on GitHub Releases

With Python 3, OpenSSL 3 and an authenticated `gh` CLI, run the publisher first without `-Publish`:

```powershell
pwsh apps/desktop/scripts/publish-github.ps1 -OutputDir apps/desktop/target/release-dry-run
```

The publisher finds the highest previous **desktop** release (even if Latest is Android-only), verifies
its signature and full package, and uses it to build the delta. The first desktop release has no delta.
For a real release, use a new output directory and pass `-Publish -Notes '<public release notes>'`.
Publishing requires a clean worktree at the live tip of `origin/main`, an origin matching the root
`Cargo.toml`, and the offline signing key. Desktop and Android must agree on the version. Both publish to
`v<version>-build.<count>` without creating local tags or commits. The first platform creates the release
at that commit; the second adds its assets to that same release. Desktop uploads Setup.exe, the full
package, the delta if available, `desktop-update.sig.json` and `releases.win.json`. Both publishers upload
packages (and the desktop signature) before the manifest. Neither overwrites assets. If a publish fails
partway, inspect GitHub before retrying.

Prefer publishing both platforms from the same commit. Either order works. Clients check up to five
recent build tags and skip releases that don't have their platform's manifest. If five or more newer
releases lack one platform's manifest, that platform's clients report that they're up to date, so never
publish five single-platform releases in a row. Keep old tagged assets
available for clients that have already fetched a manifest. Serialize publishing, and don't enable
immutable releases if the second platform still needs to add its assets.

**These binaries are public.** Never build in a deployment hostname, private identity, credentials or
local paths. For packaging only, the Rust build remaps the workspace, home, Cargo, Rustup/sysroot, temp
and known native toolchain paths. Only the executable is staged from Cargo; no PDB is shipped. The
packager also remaps upstream CI build paths in Velopack's pinned, unsigned x64 templates before vpk builds
and hashes the packages. Review the exact template hashes when updating Velopack. The release scan reads
the notes, manifest, proof, raw package and installer bytes, decompressed entries, and nested ZIPs. It
checks for local build and native toolchain paths, user-directory prefixes, local username, Git identity,
hostname, and extra private values. Failures report categories, not values. The publisher discards the
portable ZIP and legacy vpk indexes; it uploads only the named release assets. Set `TODO_DESKTOP_DENYLIST`
to a private UTF-8 file with additional values to reject, one per line. Blank lines and `#` comments are
ignored. Publishing requires at least one active pattern. Add any one- or two-character local identity
that needs scanning to this file. Review assets and release notes even if the scan passes.

### Desktop update signing key

Use OpenSSL 3 to create a PKCS#8 Ed25519 key on a trusted signing machine, in an owner-only directory
outside the checkout. Never put the private PEM or a copy of it in the repo, GitHub, CI, or a release.
Back it up securely offline. These commands create a new key for a fork or rotation, not a replacement
for the current production key:

```powershell
openssl genpkey -algorithm ED25519 -out '<offline-dir>/desktop-feed-private.pem'
openssl pkey -in '<offline-dir>/desktop-feed-private.pem' -pubout -out '<offline-dir>/desktop-feed-public.pem'
python -B apps/desktop/scripts/sign-update-feed.py export-public '<offline-dir>/desktop-feed-public.pem'
```

The last command prints **only** the public key, as 64 hex digits. Add it on a new line in
[`apps/desktop/resources/update-keys.txt`](../apps/desktop/resources/update-keys.txt). The app compiles in
that file's non-comment lines. Forks can set `TODO_DESKTOP_UPDATE_KEYS` at build time to a different
public key list in the same format. `-Publish` uses the checked-in list, never the build override.
On the signing machine, verify the reviewed source commit and build tools independently before running
any repository script with access to the key. Set `TODO_DESKTOP_FEED_KEY` in the publishing session to
the **path** of the offline private PEM. The publisher signs only with a key that matches a bundled pin. With the variable set, a dry run also signs
and verifies the proof; without it the dry run just builds and scans. A dry run never uploads assets.

To keep the key on a separate offline machine, run the publisher without `-Publish` and without
`TODO_DESKTOP_FEED_KEY` on the build machine, with the private denylist configured. Transfer only the
scanned installer, feed, and packages over a trusted channel. Verify the reviewed commit and check the
installer and packages against the build's trusted hashes before signing. Use a trusted copy of this
repository's signer and key list on the offline machine:

```powershell
python -B apps/desktop/scripts/sign-update-feed.py sign '<release-tag>' releases.win.json `
    desktop-update.sig.json --setup todo-desktop-win-Setup.exe
```

Return only the proof to the publishing machine. Recheck the proof, installer, package hashes, and
privacy scan (including the release notes) before uploading. For a manual upload, follow the same
live `origin/main`, tag, and no-overwrite rules as the combined publisher; upload the manifest last
when adding to an existing release. Never copy the private key back to the build machine.

For rotation, first publish a bridge build that includes **both** public keys, signing the bridge with
the old private key. Wait for installed clients to take the bridge before signing later releases with the
new key. After that, remove the old key in a release signed by the new key. A client that misses the bridge
cannot take a new-key update: share the trusted bridge or a verified installer out of band. Do not rotate
through a compromised signing key. Recover with a separately verified installer in that case.

### Transition from the authenticated server feed

Publish the first GitHub-enabled Windows build, **with the verifier and production public key already
compiled in**, on both GitHub and the existing paired server's `/releases` feed. Older installed clients
keep using the trusted server until they install that build; the new build switches to the independently
signed GitHub feed without changing their pairing. Keep the existing `[releases]` section and
restricted write access on the legacy release directory. Transfer the scanned packages, manifest and
Setup.exe to the server over your trusted SSH transport. Use
[`deploy/linux/publish-desktop-release.py`](../deploy/linux/publish-desktop-release.py) to install them
into the configured directory; pass `--setup-sha256` with the hash from the trusted build output. Preserve
any previous packages still referenced by the legacy manifest. If the transition manifest references an older
GitHub package not already on the server, include that exact package in the transfer. Do not cache `/releases/*`
on a proxy or CDN.
Once all clients have migrated, the server feed and configuration can be removed in a later release.

### First install and day-to-day updates

From the same GitHub release, download `todo-desktop-win-Setup.exe`, `desktop-update.sig.json`, and
`releases.win.json`. Before running Setup, use the verifier and public key list from an independently
trusted checkout (not a script or key downloaded from that release):

```powershell
python -B apps/desktop/scripts/sign-update-feed.py verify-setup '<download-dir>/todo-desktop-win-Setup.exe' `
    '<download-dir>/desktop-update.sig.json' '<download-dir>/releases.win.json'
```

Check that the signed tag matches the release you intended to install. The installer has no Windows
Authenticode signature, so Windows may still warn or a managed policy may block it. Do not bypass
policy or run an unverified installer. Quit any older unpackaged copy first. Setup installs per user
under `%LOCALAPPDATA%` without elevation and adds Start menu and desktop shortcuts. Pair once in the app
using your server's HTTPS address (for example `https://todo.example.com`). An existing local pairing from
the unpackaged app is reused. If you use **Start with Windows**, enable it from the installed app's tray menu
to point Windows at the installed copy. Uninstall removes the app's Start with Windows entry.

An installed app checks for updates about one minute after launch, then every six hours on a Windows
background-priority thread, even when it is not paired. After consecutive failures it retries after
five minutes, thirty minutes, then six hours. Downloads do not interrupt normal use.

The build reads the root `Cargo.toml` repository URL by default. Forks can set the build-time
`TODO_DESKTOP_UPDATE_FEED` to their public HTTPS repository base, or to an empty value to disable updates.
The public publisher always uses the repository default. The updater reads `/releases.atom` (at most
1 MiB), then tries up to five valid release tags in descending build order. It skips a tag only if
`/releases/download/<tag>/releases.win.json` returns HTTP 404. It fetches the proof and package under the
same tag. If no Windows manifest is found, the app reports no update. These public URLs need no GitHub API
calls or an access token. Windows verifies HTTPS certificates. The source follows at most five HTTPS
redirects on port 443, only to the configured feed host, `github.com`, `objects.githubusercontent.com`,
or `release-assets.githubusercontent.com`. No request uses the paired server's address or credentials.

The v2 proof signs `todo-desktop-update-v2`, the tag, the lowercase installer SHA-256, and the decimal
installer size, each followed by a newline, then the exact bytes of `releases.win.json`. This binds the
installer as well as the versions, hashes, and sizes of every listed full and delta package.
The app verifies the proof against its pinned keys and only accepts a newer version. It checks the size
and SHA-256 before staging, then checks the saved proof and package again before asking Velopack to apply
it. Current clients download the signed full package: Velopack's native delta reconstruction does not
produce a byte-identical full nupkg. The publisher still produces and signs the delta metadata.
A signature, release identity, or checksum failure stages nothing and shows **Update couldn’t be verified**.
A missing or invalid signature or a release identity mismatch always stops the update before download.
A download that ends early uses the normal network retry schedule. For an oversized package or a
checksum mismatch at the expected size, the app remembers the exact signed offer across restarts.
Automatic checks can retry that offer after six hours, then after 18 hours if it fails again, and at
most once every 24 hours for further mismatches. **Check now** can retry during a cooldown, but it never
bypasses verification.

Anyone controlling the release assets can withhold updates or serve an older signed release that is
still newer than the installed version. They cannot authorize changed bytes or downgrade an installed
version. We accept this freshness limit so signatures don't expire during legitimate periods without
releases.

After pairing, **Settings → Updates…** shows the installed package version (or the Cargo version on a development build),
the update status, and how long ago it last checked successfully or failed. **Check now** checks immediately
when no check, download or staged update is pending. It does not move the next automatic check or change its backoff.
Development builds do not check for updates. Errors are shown without the feed address or credentials.

When a package is ready, a quiet **Update ready** control above the sidebar footer opens Updates. Dismissing
it hides the indicator for that version without removing the update from Settings or the tray. Select
**Restart to update** in Settings or **Restart to update (<version>)** in the tray to exit normally and have
Velopack apply the package and relaunch. Alternatively, choose **Quit** from the tray (closing the window
usually just hides it), then open the app again: the next primary launch still applies the package before
opening the window. Both paths keep local data and pairing. If Velopack relaunches the app with a newer
package still pending, the app logs a warning and skips that version. A later release can still be staged.
Once staged, an update belongs to the installation, not to a particular pairing.

## Vendored `gpui_windows`

`vendor/gpui_windows` is upstream `gpui_windows` at the pinned revision plus a few patches, each marked
`TO-DO PATCH`. `vendor/gpui_windows/VENDORED.md` lists them and explains how to upgrade:

- **WARP fallback.** If no hardware adapter can create a Direct3D 11 device (CI runners, VMs, remote
  desktop), the renderer uses the WARP software adapter instead of failing. `TODO_FORCE_WARP=1` forces it.
- **Browser navigation input.** The client area already receives GPUI navigation buttons. The patch
  also forwards non-client side-button releases and Windows `WM_APPCOMMAND` Back/Forward to GPUI.
- **Frame capture.** A window can request a readback of its next frame, which is taken from its own
  render target before it's presented. The screenshot mode uses it.
- **Fixed popups.** Quick add's frameless HWND is created at its requested bounds and tracks the monitor's
  DPI. Its native show path doesn't need to activate through GPUI to apply those bounds. Normal windows
  keep the upstream placement path.
- **Hidden capture windows.** With the screenshot scale override set, the renderer sizes hidden normal
  windows with `SWP_NOACTIVATE`, skips activation and keeps the pointer outside the fixture. It requests
  lower DXGI device scheduling priority and paints on demand instead of at every vsync.
- **Present timestamps.** The time of each window's first and latest present, for the benchmarks.
- **Scale factor override.** Lets the screenshot mode render at 150 % on a 100 % monitor. While it's set,
  the window keeps the bounds it asked for even when they don't fit on the display, and
  `WM_GETMINMAXINFO` lifts the maximum track size, so a 2160 × 1350 capture works on a 1024 × 768 CI
  screen.

## gpui-component: not adopted

[gpui-component](https://github.com/longbridge/gpui-component) has a lot of ready-made widgets (inputs,
lists, menus, dock), so it was the first thing evaluated. The decision is to **build our own small widgets
on top of GPUI** and revisit the question later.

- **It doesn't build at our GPUI pin.** v0.10.0 depends on `gpui-pre =0.3.12` from crates.io, which is a
  snapshot of a different Zed revision. Patching `gpui-pre` to our git revision doesn't work: Cargo
  accepts the patch but never uses it because the versions differ (0.2.2 against 0.3.12). That leaves two
  copies of GPUI in the tree, and their types don't mix.
- **Adopting it means re-pinning GPUI to its snapshot**, then following its weekly snapshot bumps. Our pin
  was chosen in the GPUI spike together with the vendored Windows fixes and the toolchain, so moving it is
  a separate decision.
- **It's heavy.** At its own pin it builds about 500 crates and takes about 2.5 minutes for a clean release
  build.
- **F2 needs its own look anyway.** The design uses round checkboxes, pills only for list chips, Geist
  room titles and the painted header landscape. We'd restyle most components rather than use them as they
  are.

What we use instead: the text input from GPUI's `input` example (`src/ui/input.rs`, IME, selection and
clipboard included), a multi-line version of it for notes (`src/ui/text_area.rs`, wrapping with GPUI's
text system), GPUI's own `uniform_list` for the virtualized task list, `anchored` overlays for menus, and
plain `div`s in `src/ui/kit.rs`. **Revisit** when the app needs a date picker or richer menus. If
gpui-component has moved onto a Zed revision that we can pin to (or onto the same snapshot crates),
adopting it then is cheap.

## Pairing, config and sync

**Pairing.** On first run (no saved pairing) the window shows the pairing screen (`src/ui/pairing.rs`). The
owner types the server's address, for example `todo.example.com`; `https://` is added, and plain `http://`
is refused except for `localhost`, so the token never crosses a network unencrypted. The app calls
`POST /api/v1/pair/start` with the device name ("To-do on <computer name>", read at run time), shows the
user code and the approval QR, offers to open the approval page in the browser and polls `POST /api/v1/pair/poll` at the interval the
server asks for. The address is checked with the transport's own parser (`todo_client::native::server_origin`):
https, or plain http only for this computer, and no user name, query or fragment. The approval page is built
from that address (`<server>/pair?code=…`); the `verification_uri` the server sends is never opened. The
transport follows no redirects and gives every finite request 30 s. Rate limits wait for `Retry-After`; a run of network failures, a denial or an expired code
go back to the address with a message. Nothing spins while it waits. **The server address is never built
into the app**: it is learnt here and saved in the user's config. Alternatively, paste an
[invite link or code](device-pairing.md) from a signed-in app. A typed code also needs the server address.

**Where things are kept** (`src/config.rs`):

| File | Where | Contents |
|---|---|---|
| `config.json` | `%APPDATA%\to-do-or-not-to-do\` | the server URL, the device ID and a random pairing ID |
| `device-token.bin` | same | the device token, sealed with DPAPI (`CryptProtectData`, current user, with app-specific entropy) |
| `pairings\<pairing ID>\cache.sqlite` | `%LOCALAPPDATA%\to-do-or-not-to-do\` | the sync engine's replica and outbox |
| `pairings\<pairing ID>\cache.sqlite.revoked` | same | empty marker: the server replied 401 to an authenticated request |
| `pairings\<pairing ID>\ui.json` | same | collapsed collections, preferred pane widths, explicit Done rail choices and Timeline zoom, rail and tray preferences (never synced) |
| `pairings\<pairing ID>\popouts.json` | same | list-window placement, pin and open state by room ID (never synced) |
| `pairings\<pairing ID>\assistant-profile.bin` | same | owner-only, separately DPAPI-sealed AI server URL, model, optional API key, capability test result, enable switch, and list access |
| `pairings\<pairing ID>\reminders.json` | same | the reminders shown and snoozed on this computer: reminder and task IDs and times, never task content (never synced) |
| `pairings\<pairing ID>\attachments\<random>\` | same | an attachment downloaded to be opened: a new owner-only directory for every open |
| `pairings\<pairing ID>\damaged\<ms>\` | same | damaged data set aside by **Start fresh**, kept until sign-out |
| `pairings\<pairing ID>.signed-out` | same | the record that the pairing beside it was signed out and its data is to be removed |

`TODO_DESKTOP_HOME` puts everything in one directory instead (tests, benchmarks, a second profile). The token
is never written in plain text and never logged. DPAPI's keys belong to the user's profile: they protect the
token from other accounts and from a copy taken off the machine, not from programs running as the same user,
and with a roaming profile they travel with it. Each pairing's data lives under its own pairing ID, so a new
pairing never picks up an old cache and cleaning up an old one can't touch the current one.

**The account menu** (the gear in the sidebar footer) names the server the app is paired with, opens its
admin pages (`<server>/admin`, built from the paired address by the same `todo-client` helper as Android,
using the transport's address policy), and has **Sync now**, which retries at once instead of waiting out the backoff
(`SyncClient::nudge`). **Devices…** opens the pairing dialog with **Pair a new device** and
**Approve a device**.

**Sign out** (the account menu) runs in order: syncing stops and attachment transfers in flight are
cancelled and waited for; the record `pairings\<pairing ID>.signed-out` is written beside the directory and
flushed to disk (if it can't be, nothing is removed: the app says so and stays signed in, syncing again, and
signing out can be tried again); the token and the config are removed (if that fails, the record is
withdrawn, and the app says so and stays signed in the same way); the shell lets go of the cache, and the directory (cache, sidebar preferences and downloads) is removed; the
record goes last. Only then is pairing shown. A directory still in use is removed at the next start, which
only ever removes directories that have a record, named exactly by a pairing ID, and never the current one.

Removal never leaves our own data. The app directory and `pairings` must be plain directories (a junction or
symlink there stops all removal); a link at a pairing's directory, or anywhere inside it, is removed as a
link and never followed. The server has no revoke endpoint for a device yet; revoke it there too if the
computer is lost.

**Sync** (`src/session.rs`). The app runs `todo_client::SyncClient` with the native HTTP transport and the
SQLite storage on a small tokio runtime of its own (two worker threads). Initial load, catch-up, the SSE
event stream, the offline outbox and reconnecting with backoff are the client's; the UI thread only calls
its synchronous `view`, `submit` and `status` and awaits its change signal, coalesced to one frame. A
command applies to the view at once and is queued for the server; one the domain refuses shows a message in
the status bar. Attachments upload and download on the runtime too. A 401 from sync, Activity, a download, an upload or
a sync request marks the pairing as signed out. It stays signed out when reopened offline; the
marker is separate from the database, so recovery doesn't clear it or discard any unsent edits.

**Startup.** A loader thread reads the config, unseals the token and opens the cache while GPUI sets up
Direct3D and DirectWrite, so a warm start draws real data in its first frame. Opening never deletes anything:

- **A damaged cache** (SQLite reports it malformed or not a database, or stored data doesn't decode) is
  recovered in steps, each recorded in `recovery.json` before the next one starts: the cache and its
  write-ahead log are copied aside as `cache.damaged-<ms>.sqlite` and flushed, then the original is removed.
  The copy's outbox and "couldn't sync" list are read back (`SqliteStorage::salvage_all`, which fails on any
  read error rather than come back short) into a new cache built as `cache.recovering.sqlite`, which then
  replaces `cache.sqlite`; the record goes last. A copy that can't be read stops the recovery behind the
  "couldn't be opened" screen, keeping the record and the copy for the next try. A crash or a
  failure at any step leaves the record, and the next start finishes the recovery from there, so an unsent
  edit is never lost. The confirmed state is loaded from the server again. The status bar says how many
  unsent changes were kept.
- **A recovery that can't finish** (the copy can never be read, say) doesn't lock the app: while
  `recovery.json` is there, the screen also offers **Start fresh (keep the damaged copy)**, after asking,
  since it gives up on the unsent changes. It first records the destination, a new `damaged\<ms>\` folder in
  the pairing's directory, in `start-fresh.json`, then moves each damaged database with its log files, then
  `recovery.json`, then `start-fresh.json` last (nothing is deleted), and opens an empty cache that loads
  from the server. It refuses, moving nothing, if the pairing's directory or any of those files is a link or
  not a plain file. While `start-fresh.json` is there, nothing opens or salvages the half-moved files: the
  screen offers only **Finish starting fresh**, which continues into the same folder. The status bar says the unsent changes
  weren't recovered, with **Open folder** for the kept copy. Kept copies are removed with the pairing's data
  at sign-out.
- **A cache that can't be opened now** (another copy of the app holds it, no permission, disk full) is left
  as it is. A screen says what happened, with **Try again**.
- **A saved sign-in that can't be used** goes back to pairing with the reason. The old data stays.

## The views

The views draw a `model::Shell`, which `model::derive` computes from a `todo_core::State`, the window's UI
state (room, selection, filter, Completed open, tasks being completed), the sync status and the clock. It is
pure and unit-tested; the running app and the screenshot fixtures go through the same code.

| Part | What it does |
|---|---|
| Sidebar | Smart lists (My Day, Important, Planned, All, Completed, Agent activity) with counts from the query language's built-in lists, collections and lists in server order with open counts. A collection's chevron or context menu hides or shows its lists without changing the room. A collapsed header shows the selected look when it hides the current list; navigating to that list reveals it. List colours snap to the design's swatches; glyphs come from the list's name (English and Czech keywords). Double-click or right-click renames; a list's context menu recolours it (**Colour…**: the design's twelve swatches, the current one ticked; the arrows walk them as a grid of two rows), moves it to another collection or out of any (**Move to…**: a picker of the collections, as the sidebar orders them, then No collection; the list goes last in its new collection, which unfolds, and Undo moves it back), moves it up or down, or deletes it (after asking). A collection's context menu folds it, adds a list to it, renames, recolours, moves it up or down, or deletes it: the confirmation counts what goes with it ("Delete the collection “Work” with its 4 lists and their 58 tasks? Undo brings them all back."). The domain tombstones only the collection; its lists and their tasks are hidden with it, not deleted, so Undo brings all of them back. **Activity** (**Ctrl+H**) follows Agent activity among the smart lists, and under it, indented, up to four agents active today, each saying what it did ("3 added today"; the tooltip repeats the name and status in full when the row is too narrow): clicking an agent's row opens Activity filtered to that agent. The order is the one every client keeps (`docs/design-language.md`, *Sidebar order*). Choosing **List** or **Collection** from the pinned **+ New** menu adds one, opens it and scrolls its inline name editor into view. Tabbing onto the footer doesn't scroll the rooms. While something couldn't sync, the sync status is a button (and a Tab stop): it opens the refused changes, each saying what it was, what it was about and why in plain words (`todo_client::failure`, never the server's own text), with **Dismiss** and **Dismiss all**. The list shows the changes refused when it opened; one refused while it's open shows the next time, and Dismiss all takes only what was shown (`SyncClient::dismiss_many`, one write). |
| Task list | Open tasks in server order (by list, then position, in a collection), virtualized; Completed below, newest first, behind a toggle. Checkbox completes or reopens; completing plays the choreography (see *Motion*). The priority mark cycles on click. **J/K** or arrows move, **Space** completes (a held key's repeats are ignored, so holding it never flips a task back and forth), **E** edits the title, **D** the due date, **Alt+Up/Down** moves a task in a list, **Delete** deletes it (without asking: Undo brings it back; like Space, a held key's repeats do nothing, so holding it never deletes the rows that take the selection), **M** moves it to another list, **T** opens the tag picker, **L** puts the keyboard in the link field and **Shift+F10** opens its menu. Right-clicking a row opens the task's menu at the pointer without selecting it: Complete or Reopen, My Day, Important, Priority…, Move to…, Links… and Delete task. Move to… (and M) is a list picker: the lists grouped by collection as in the sidebar, the task's own ticked; typing narrows them by list or collection name (Backspace takes a letter back, Escape clears it first). A moved task goes first in its new list. A deleted task's row (or its line in the finished day's hand-over), or one moved to a list the room doesn't show, leaves with the completion's exit, and the selection goes to the row that takes its place. A menu's keyboard highlight is kept by item, not by place: a list added above it by a sync leaves it on the same list, and Enter on an item that has gone does nothing. A menu whose task or list is deleted elsewhere closes, and the keyboard goes back to the list. The keyboard starts on the list, and a click that no field takes leaves it there. The title-bar field filters (**Ctrl+K**): a query (`tag:docs`, `due:today`) or plain words. A row's metadata line shows the list chip (at most `desktop::CHIP_MAX` wide, ending in an ellipsis), steps, the mail mark and then the tags. Each item is measured against the title column (widths rounded up per element, as GPUI lays them out), and tags that don't fit whole are counted as `+N`, never cut. In a narrower column the mail mark, the step meter and the step count give way in turn, and the chip narrows last, down to its dot and an ellipsis; the line is never wider than its column. Names from the data (lists, collections, tags, titles, steps, links, attachments) are drawn on one line: `labels::one_line` turns line breaks and other control characters into spaces. The data keeps them: a rename editor starts with the one-line name and saves only a real change, and Save and Open use an attachment's stored file name (the save dialog offers it made safe for Windows). Single-line inputs keep whatever is set, typed or pasted on one line too. |
| Activity | Every confirmed change, from the owner, the agents and email, newest first and grouped by local day ("Today", "Yesterday", "Thu 19 Nov", each with its count): what happened (the server's plain description), who did it with their mark (an agent's letter, a neutral `Y` for the owner, a mail icon for email), how long ago and the local time. It opens from the sidebar's Activity row or **Ctrl+H** (never while typing: in a text field the shortcut does nothing), and an agent's row opens it filtered to that agent: the eyebrow says "Agent: Claude", and **Clear filter** in the toolbar (or the room menu's **All changes**) takes the filter off. The room menu also switches between the agents and has **Refresh activity**. The toolbar takes the quick-add's place: the order, Clear filter while filtered, and **Refresh activity**, which reads the newest changes again without moving the feed; a dot on it says the replica has moved on since the feed was read. Older changes are read twenty at a time as the feed scrolls near its end, or as the keyboard nears the last one read; at the end, **Load older activity**, or after a failed read the reason and **Try again** (**Skip this change** past one too large to show). Each change the server can undo has **Undo**: it queues the checked, non-forced undo of the change's whole command (`SyncClient::undo_activity`, idempotent), shows **Undoing** until the server answers, and **Undone** after; a refused undo says why in plain words under the change and in the status bar (`failure::undo_message`, never the server's text), and can be tried again. **Up** and **Down** (or J and K) move through the changes, **Enter** (or **U**) undoes the selected one, **R** refreshes and **Esc** clears the selection; these act only while the list itself has the keyboard, so Enter on **Load older activity** or **Try again** presses that button and never undoes a change. The undos asked for are kept by the store for the whole pairing, not by the room: while one is on its way, leaving Activity or changing its filter never lets another change of the same command queue a second undo. A late page read for an earlier filter (or before a replaced log) is dropped, since every read has a number no other feed reuses. Tab landing on the button at the end of the feed scrolls it into view. Ctrl+N goes to My Day's quick-add. The status bar counts the changes shown and today's among them. There is no detail pane in this room. The pages, the undos asked for, what each row offers and all the wording are shared with the web app (`todo_client::activity`: `Feed`, `Undos`, `undo_action`); the desktop adds the selection, the rows as drawn and their fades (`model::activity`, `ui/shell/activity.rs`). |
| Quick add | In the room (**Ctrl+N**) and global (**Ctrl+Alt+Space**, into My Day). The room field shows removable date and priority chips and offers both pickers next to the [quick-add syntax](#quick-add-syntax). The shared `todo_client::intent` parser and builders apply My Day, Important and Planned defaults and put the new task first in its list's manual order. On the finished My Day the field stays, quietly drawn, so the hand-over to tomorrow leads. |
| Detail | Title and notes edited in place (notes are multi-line plain text), steps added, completed, renamed, moved and deleted (Undo brings a deleted step back), My Day and Important toggles, the due date picked from the shared calendar (click **Due**, Tab then Enter, or press **D** from the list), and the priority picked from its four levels on the Priority row (click it, or **Shift+P** from the list; **P** cycles them). The Tags row is always there, each tag a dot in its colour and its name; clicking it, or **T** from the list, opens the tag picker under it: every tag by name, those on the task ticked. Choosing one puts it on or takes it off, and the picker stays open for the next. Typing narrows the tags (ignoring case and accents), and a name no tag has yet offers **Create tag “…”** first, so Enter makes it (with a swatch from its ID) and puts it on the task. A tag's chevron, shown on the row under the pointer or the keyboard (or **Right**), opens its own page: put it on or take it off, **Rename tag** (in place in the Tags row, for any tag, whether or not the open task has it; a name
another tag has, in any case, or one longer than the domain's 200 characters, is refused with a reason), **Colour…**, **Delete tag…** and **All tags** (or **Left**). Deleting shows how many live tasks carry the tag; they stay, and Undo restores the tag on them. If the name or count changes before confirmation, the picker asks again. If another live tag has since taken the deleted tag’s name, Undo leaves both tags as they are and explains the conflict. The Links section is always there too: under the links, a field (**L** from the list) adds a web link, typed or pasted; `https://` goes in front only of an address that doesn't start with a scheme of its own
(`example.com/page`, while `example.com/?next=https://…` stays as typed after it), and the result must
parse as an `http` or `https` URL with a host (`url`), then pass the core's own check; anything else
(`javascript:`, `mailto:`, a bare `https://`, spaces) says why in the status bar and stays to be
corrected. Adding a link can be undone like removing one. A link's remove button shows on hover; the task's menu has **Links…** too (Add a link…, then each link to remove), so the keyboard reaches both. Undo brings a removed link back. The **Reminder** row is always there and editable (click, Tab then Enter, or **Shift+R** from the list); it shows the reminder the shared `intent::reminder::shown` picks (the first from today, else the latest) or *No reminder*. Its picker offers **Later today** (two hours on, rounded up to the hour, while that is still today), **Tomorrow morning** (09:00) and **Next week** (Monday 09:00), each with its time beside it, then **Pick a day and time…** and, when there is one, **Remove reminder**. The current choice is ticked and the keyboard starts on it. The custom page steps the day (−7, −1, +1, +7, never before today) and the time (−1h, −15, +15, +1h); typing digits sets the time (`9` is 09:00, `930` 09:30, `1430` 14:30, Backspace takes one back). It says when it will remind before **Set reminder**: a time that has passed, or typed digits that aren't a time, can't be set; a time the clocks skip saves the one the gap moves it to, and one that happens twice saves the first, and the page says so. Escape discards the draft. An edit is made on the reminder the row showed when the picker opened (`intent::reminder::Seen`): if it changed meanwhile (another device, an agent) and that change has already synced to this computer, nothing is written, the status bar says so and the picker shows it as it is now. A change made elsewhere that hasn't arrived yet (this computer offline) can't be seen: the later write wins, as for every other field. A preset saves exactly the time its row showed, from a click or from the keyboard, even if the hour or time zone changed since opening the picker (its preset instants are resolved on opening); **Later today** never lands less than two hours away, also when the clocks go back. Setting, moving and removing a reminder show the undo toast (**Set reminder for …**, **Removed reminder for …**), and Undo is the exact inverse through the same tracked compensation as every other change. The **Repeat** row is always editable (click, Tab then Enter, or **R** from the list). Pick Daily, Weekdays, Weekly on the due weekday, Monthly on the due day, Yearly, or **Don’t repeat**. **Custom…** lets you type an interval from 1 to 999 or use −/+, choose a unit, and toggle weekdays for weeks. The preview describes the schedule before **Apply repeat**. Escape discards the draft. An undated task starts today if the rule includes today, otherwise on the next matching day (for example Monday for Weekdays chosen on Saturday). Custom shows the first due date before Apply. Monthly keeps a rule’s original day even when a shorter month clamped Due to its last day. Edits preserve an existing end date. Clearing Due on a repeating task says **Cleared due date for …**; Undo restores both Due and Repeat. Stopping a repeat keeps Due; Undo reverses the entire schedule change, including a due date it added. A title or notes save that fails keeps the text in the editor, and the task stays selected until it saves. Only `http` and `https` links open, in the browser; anything else (`file:`, UNC paths, other protocols) is shown as text. **Add file** opens the native multi-select picker; dropping files from Explorer on the task detail does the same. A picked or dropped filesystem link uploads its target, not the link itself. Files upload directly, up to 25 MiB each (a server can set a lower cap), and show in the list when sync confirms them. If the outcome is uncertain, retrying the same path on the same task in the current session reuses the original upload ID, or uses sync's confirmation if it has arrived. Uploads are unavailable offline and for a task that has not synced yet. Tab reaches Add file and each file’s Save, Open (if supported) and Remove controls. **Remove** queues a regular edit even offline and offers **Undo**. An attachment is saved where the owner chooses (a save dialog in Downloads): it is written to a temporary file beside the destination, marked, and then moved over it, so a failed save leaves an existing file as it was. Only documents and images (pdf, png, jpg, jpeg, gif, webp, txt, md, csv, docx, xlsx, pptx, eml, msg, decided on the cleaned file name) also offer **Open**, from a new owner-only directory for each open. Every downloaded file gets the Mark of the Web (`Zone.Identifier`, zone 3), so Office uses Protected View and SmartScreen checks it. A drive that can't hold it (FAT, exFAT) still gets a saved file, with a warning that Windows won't warn when it's opened; a file to open is refused without it. |
| Header | The sky, hills and sun from real counts: *done of total* and open. No estimate (tasks don't have one). The "more" button opens the room's menu with only what applies there (`model::menus`): in a list, Rename list (in place in the sidebar), Colour…, Move to… (where there is somewhere to go), Show or Hide completed and Delete list (after asking, with Undo); in a collection, Rename collection, Colour…, the Completed toggle and Delete collection (after asking, with Undo); in My Day, Important, Planned and All, the Completed toggle. All six smart lists and user lists also offer **Pop out**, in the header and on right-click in the sidebar. There is no view options button until there are sort or view options. In a narrow window the counts go on a line of their own under the title, which keeps its full size. My Day all done turns the sky to dusk (once the last row has left) and hands over to tomorrow. Its tasks open in the detail pane, by click or with **J/K**, and their boxes (or Space) complete them with the row's choreography, in place (the first one's card too: it dims, is struck through, fades and closes), and the hand-over scrolls when it's taller than the pane; with none selected, the pane is left out rather than shown empty. With nothing due tomorrow there is no hand-over, but planning is still offered: "Nothing is due on Sunday yet." and **Plan Sunday**, which opens Planned (as on Android). While a filter narrows the finished day, neither shows, and the room says nothing matches. |
| Narrow window | Below `desktop::THREE_PANE_MIN` (1200 px; `model::layout`), the list keeps its full width and the detail pane opens over its right part only while a task is open, with a scrim over the rest. Clicking the scrim or pressing **Esc** closes it and leaves the keyboard on the list. The pane slides in 24 px and fades in over `OVERLAY_IN`, and fades and slides out over `OVERLAY_OUT`; with reduced motion (Windows' *Animation effects* off, read with `SPI_GETCLIENTAREAANIMATION` whenever the overlay appears or closes) it only fades, in `REDUCED_FADE`. A window narrowed past the breakpoint with a task already open shows the overlay at rest, without the slide. A pane fading out is inert: nothing in it takes a click, and the scrim still keeps clicks off the rows. The screenshot fixtures draw it at rest. |
| Status bar | Open and *done today* counts (*done today* is what was completed since local midnight; **Completed n** holds every finished task), the keys that work where the keyboard is now (the list's letters only while the list has focus and a task is selected, Space saying *reopen* for a completed one; Enter and Esc while typing), and notices. In a narrow window the hints give way (`hints::fit`): Shift R, R, T, L, Shift P, Del and M first, which have another way in, then the other letters; Space, Esc and Ctrl Z stay. |

### Back and Forward

Use the mouse's Back/Forward side buttons, a keyboard's Browser Back/Forward keys, or **Alt+Left** /
**Alt+Right**. The status bar shows the shortcut when a destination is available. Side buttons act on
release. Navigation stays in the focused window, and does not run while editing a text field. Back
cancels a confirmation or closes Devices when Escape would close it; otherwise an open menu or
settings dialog keeps the current location. There are no new title-bar buttons.

A room visit, a List/Board/Timeline switch, or explicitly opening or closing task detail is a history
entry. A keyboard cursor move (**J/K**, arrows) may preview another task, but does not change the
recorded location. Selection, filters, scroll, menus and dialogs are not restored. A new location
replaces the forward branch; consecutive duplicates do not add an entry. Back and Forward skip
rooms or tasks no longer available, or views that have been disabled. Each window keeps at most
100 entries in memory for the current session. A pop-out stays in its fixed List room, so its
selection does not add entries; **Open in main window** navigates in the main window's history.

### Ask AI

**Ask AI** (**Ctrl+J**) opens a right-hand panel. In wide windows, drag its left divider, or use
**←/→** (with **Shift** for larger steps) once the divider has focus. **Home** resets its width.
In narrower windows it opens over the list. Space for Detail is shared at 1600 px and wider; in the
middle range opening a task from chat brings Detail over the room, beside chat. **Enter** sends;
**Shift+Enter** inserts a line break. Suggestions fill the composer without sending. Chats are in
memory only. **New chat**, a profile change, a change to list access, or sign-out clears chat history
and any unapproved proposal.

1. Open **AI assistant…** in Settings. Enter your OpenAI-compatible server's base URL (for example
   `https://mac.tailnet.example/v1`) and its optional API key. This is not your To-do key or password.
   An HTTPS address outside loopback or Tailscale is blocked until you confirm that it is your own
   server, keeps data private, uses local weights, and has no per-token billing. This confirmation
   applies only to that URL. Editing the URL clears the confirmation, key, and list access.
2. Choose **Test connection** to fetch the models. Pick a listed model or enter its ID. **Test model**
   sends an invented prompt to check streaming and tool calls; without this test, or if only text
   works, the model has no task tools.
3. Choose which lists the assistant can read. A collection header sets every list in it and the
   default for lists added or moved into it later. **Mixed** means its lists have different access.
   Individual choices override the collection and follow the list if it moves. Setting the collection
   again replaces the individual choices for its current lists. Without either choice a list is
   **Hidden**, including when it moves out of a shared collection. Inheritance can reach **All models**
   only if you explicitly set it on the collection. **Private only** and **All models** both allow
   the configured private provider. Select **Enable assistant on this device**, then **Done** to
   save. No tests or inference run in the background. **Forget server** clears the key and removes the protected
   profile and any orphaned files from an interrupted settings save.

The panel shows the configured destination: **This device**, **Tailscale**, **Your server** (confirmed
HTTPS), or **Not private** (blocked). HTTPS uses normal certificate verification. Plain HTTP on
loopback supports a model running on the same PC. Direct Tailscale HTTP is unencrypted by the app and
relies on the tunnel. On every such request, Windows must report an up interface with a Tailscale
address; otherwise no request is sent. The interface check does not prove the route or peer identity.
Prefer HTTPS when you cannot verify the intended route. No redirects, HTTP proxies or alternate
providers are used. A missing or rejected key, missing model, server unavailability (including sleep),
or usage limit needs your attention; nothing is sent again until you choose **Retry** or send a new
message. The app waits at most five minutes for a server's `Retry-After` hint. See
[provider architecture](ai-assistant-architecture.md#3-provider-choices).

Prompts and allowed task summaries go directly to the server you entered, not through the To-do
server. Task notes and attachments are not shared. A model can prepare new tasks, title/due/priority
edits, and non-repeating completion changes, but cannot apply them. Destructive tools aren't offered.
Review the exact changes, optionally pick a subset of a batch, then **Approve**. Approval waits until
sync has caught up; an outdated review needs a new proposal. Confirmed changes display the `A`
mark and appear in **Activity** under **In-app assistant**. **Undo** on the chat receipt or in Activity
reverses the whole approved group. Chat drafts and API keys never enter the To-do sync log.

#### ChatGPT account (Windows)

No owner-supplied client ID, API key, or client secret is needed for this
[open-source, locally hosted flow](https://developers.openai.com/siwc/token-sharing-open-source/sign-in).
Choose **ChatGPT account** in AI settings, then **Continue with ChatGPT**. OpenAI registers the app
as an agent for the account and workspace that the owner selects in the system browser. The app
saves the issued client ID before exchanging the code and reuses it on later sign-ins. The stable
host ID for this pairing is sent as `urn:uuid:<uuid>`.
This does not sign the owner into To-do or give To-do access to ChatGPT conversations.

Choose a model from the signed-in account's catalog and explicitly enable the assistant in settings.
The first send asks for permission and names the **All models** lists. The choice is saved for that
account registration. **Hidden** and **Private only** lists, task notes, and attachments are never
included. Text you type and the permitted task summaries the assistant reads go directly to OpenAI.
Responses use `store: false`; this is not a zero-retention promise. The app does not send chat data
or credentials through the To-do server, and never switches providers automatically.

ChatGPT plan usage is available only to eligible accounts. Requests can use plan allowance or
available credits, not a To-do subscription or API key. Use **Manage usage** to review limits;
turn off credit use in ChatGPT settings if you want included-only usage. The app cannot enforce
that setting. An OpenAI usage failure stops the reply; the app will not retry it automatically.

The `assistant-chatgpt.bin` record is separate from the private server profile. Both are owner-only,
DPAPI-protected files in the pairing directory. The app renews a token only as needed, saves rotating
credentials together, and stops using them at sign-out. **Sign out** attempts remote revocation and
then clears local tokens but retains the account and client mapping for reuse. Unpairing removes the
local record. If remote revocation is not confirmed, disconnect the app in ChatGPT settings.

The **Continue with ChatGPT** button uses the official logo published in OpenAI's
[sign-in button guidelines](https://developers.openai.com/siwc/website). The mark is not the To-do
application identity and does not imply OpenAI endorsement.

### Workflow lists, Board and Timeline

Open **List settings…** in a list's header or sidebar menu. **Workflow & Board** adds task status
and Board view; **Timeline & Start dates** adds Starts and Timeline view. Both are off on new lists.
Turning either off hides its controls without clearing task data. **Default view** syncs with the list,
while a switch made in the header applies only to the current visit. **V** cycles available views;
**1**, **2** and **3** choose List, Board and Timeline while the room has focus. Pop-outs stay in List.

The workflow glyph appears beside a task's metadata, after the list chip in cross-list rooms. Those
rooms only show Backlog and In progress marks. Click the mark or press **S** for Status, or **Shift+S**
to cycle it. **Shift+D** opens Starts, above Due in Detail. Setting Starts after Due moves Due to match
in the same undoable edit. In the composer, pick Status or use `status:backlog`, `status:todo`,
`status:wip` or `stav:rozpracovano`. On a list with start dates, choose **Starts** or **Due** in the
composer calendar, or type a range such as `today..+4d`. Date and non-default status chips can be
cleared separately.

The Board has three open columns, each with its own **+ Add** footer, and a Done column. Done shows
at most 20 completions from today and the previous six local dates; **View all** opens this list's
Completed section. Done automatically folds into a drop rail beside docked Detail. An explicit rail
choice is remembered locally. The rail and the columns beside it ease to their new widths. Drag a
card above or below another to place it there, or drop into an empty column. Sibling cards settle
into their new positions after a local move; with reduced motion the rail and siblings snap. Drop on Done to complete, or drag out of Done to reopen in a particular stage. One Undo
reverses both the stage and the placement. Completing a repeat still creates the next occurrence in
To do.

Use **H/L** or **←/→** to move between columns, and **J/K** or **↓/↑** to move between cards.
**Ctrl+Alt+←/→** moves a card to the neighboring stage; **Alt+↑/↓** reorders it within the current stage.
**Enter** opens the focused card (or expands the focused Done rail), **E** edits its title, **Space**
completes or reopens, and **N** opens the active column's composer. **Ctrl+click** and **Shift+click**
select several cards, with a **Status** action in the selection strip. Choosing an open status there
leaves completed cards alone.

**Timeline** shows a sticky, collapsible task rail beside an inclusive calendar grid. Choose **Week**,
**2W** (default), **Month** or **Quarter**. Month labels ISO weeks; Quarter has Monday-first week
columns, but edits still snap to calendar days. Zoom keeps the centre date. The arrows page the grid;
**Today** or **0** returns to the Today line. Horizontal scrolling and **Shift+wheel** pan dates while
the rail stays put. **Show completed on Timeline** reveals completed tasks that have dates.

Drag a bar to move its dates together, or an edge to change one date. Due-only tasks show a one-day
flag or overdue diamond; pulling their left edge out adds Starts. A start-only bar has a dashed
right edge; pulling it out adds Due. **Unscheduled** opens a tray for open tasks without dates: drop a
task on a day to set Due, or use its **Schedule** calendar. The tray, rail and zoom are remembered on
this desktop, not synced. The date readout follows a drag; the final change has a single Undo in the
reserved strip (**Ctrl+Z**). A drag won't overwrite dates that changed on another device mid-gesture.

Use **↑/↓** or **K/J** to select lanes and reveal their dates. **Ctrl+Alt+←/→** shifts both dates one day
(**Shift+Alt+←/→** one week). **[** and **]** shrink or extend the end of the span. **D** and **Shift+D**
open Due and Starts; **Enter** opens Detail. In the tray, use the same selection keys and **D** to
schedule a task without dragging. **Ctrl+N** switches to List and focuses its composer.

**Planned** and collections with a Timeline-enabled child list also offer Timeline. Their bars use
the task's own list colour. Planned shows every dated open task, including start-only tasks, and has
no Unscheduled tray. Its List view keeps the existing due-date membership.

### Task activity

Below Attachments, **Activity** shows the task's creation, latest edit and current completion, newest
first. It uses the shared `todo_client::task_activity` summary and the same letter marks as the Activity
room. An owner device is shown as **You**, not by its device ID. Editing a completed task doesn't change
who completed it; when the replica doesn't know the completer (for example a historical import), the row
says **Unknown actor**. This section works offline and doesn't show every change or offer per-event Undo.

### Select and change several tasks

Use **Ctrl+click** to toggle rows, **Shift+click** to select the range from the anchor, and **Shift+Up/Down**
to extend or shrink it from the keyboard cursor. **Ctrl+A** selects all visible task rows in the room,
including the filtered results and any expanded Completed rows, but not rows already leaving.
**Esc** or the selection bar's Close button clears the selection. Changing rooms also clears it.

With two or more selected, the strip above Undo offers **Complete** or **Reopen**, importance, My Day,
**Move to…**, **Set due…**, and **Delete**. Tab reaches each control, including the icons in narrower
columns. Mixed selections offer the positive action and leave tasks already in that state alone.
Move to… reuses the searchable list picker and puts moved tasks at the front in selection order. The
Due editor accepts the task editor's usual date syntax; Enter saves, a blank clears dates and repeats,
and Esc cancels. The detail pane shows a count instead of a task editor. Each change clears the selection
and offers one Undo (also **Ctrl+Z**), including any next occurrences created by completing repeats.

### Drag tasks

Grab a row by the six-dot grip. A picked row lifts and follows the pointer; dragging a selected row
lifts the whole selection, in visible order. The insertion line appears only among open tasks in a
manual list. In a filtered list it means before the next remaining visible task, or after the last
one; hidden rows keep their order relative to each other. A smart list or collection doesn't reorder.
Drag near the top or bottom of the list or sidebar to scroll it. Drop on a sidebar list to move the
picked tasks to its front, or on **My Day** to add them to today's plan without moving them. Only a
valid destination highlights. Release away from a target or press **Esc** to cancel.

**Alt+Up/Down** moves the cursor task or the selected block past the open visible row just outside it.
A pop-out supports reorder in manual lists but has no destination sidebar. **M** still opens the list
picker. Every drop or nudge is one tracked change with one **Undo** (also **Ctrl+Z**). Task drags are
separate from dragging email or attachment files from Explorer.

### Merge two tasks

Right-click a task row (or select it and press **Shift+F10**) and choose **Merge into…**. The picker
lists other open tasks across lists, sorted by title with the list and collection beneath each title.
Type title words (case and accents don't matter) to narrow the list. Use the arrows and Enter to pick
one, or click it. The confirmation names both tasks and starts on **Cancel**. The destination keeps its
own title, list, priority and schedule; source notes are appended and its live steps, links,
attachments and reminders move over. Tags are combined. **Undo** (or **Ctrl+Z**) reverses the whole merge.
If a task changes name or list before you confirm, the app checks the question again instead of merging
on the old one. A destination that's no longer open cannot be used.

### Trash

Open **Trash** below Activity and its agents. Directly deleted tasks, lists and collections appear newest
first with their deletion age and original list or collection, where applicable. **Restore** puts an item
back where it was; **Undo** (or **Ctrl+Z**) reverses the exact restore. Use **J/K** or the arrows to select an
available item and **Enter** or **Space** to restore it. If the original list or collection is deleted, restore
that parent first. Restoring a parent reveals its children that were only hidden, not the ones deleted
separately. Tasks merged into another task are not individually restorable from Trash; undo the merge instead.
Trash uses the local synced replica and has no automatic expiry or permanent-delete control. An attachment's
file may have expired under the separate blob-retention policy.

## Drop an email onto the task list

Drop saved `.eml` files from Explorer, or from a mail client that supplies them as real files, onto the
task list below quick-add. A light wash and a card with the mail icon show where the tasks will go.
A drop creates one task per message in the same destination as quick-add:
the current list, the first list in the current collection, or the default list in a smart room, with
that room's defaults. Activity and Trash do not accept email drops. Classic Outlook's virtual `.msg`
files are not supported.

The subject becomes the title, without reply and forward prefixes. The notes include the sender and
up to 4 KiB of the body as plain text. Unlike [forwarding mail](ingest-email.md), the drop itself asks to
keep that excerpt. Agents with access to the destination list can read it; public share pages never
show notes. An identifiable message
gets the same Email source as forwarded mail, with the sender and `Message-ID`; no browser link is
inferred. The app never renders the HTML, fetches its resources or uploads the original file and MIME
attachments. To keep the original file, drop it onto an open task's **detail pane** instead.

Drop up to 20 email files at a time, each at most 10 MiB. Other file types dropped on the list are
ignored. The toast reports skipped or unreadable messages and offers one Undo for the imported tasks
and their links. Undo leaves any automatically created destination list in place. The task and link
commands sync separately and each Undo checks for conflicts, so later edits can prevent part of the Undo.

## Quick-add syntax

After at least one title word, all clients accept one trailing date and one priority, in either order:

- Dates: `today` / `dnes`, `tomorrow` / `zítra`, nonnegative `+Nd` (for example `+3d`), or an ISO date
  such as `2026-12-01` that is today or later. `yesterday`, negative offsets and past ISO dates stay in the title.
- Weekdays use today or the next occurrence. English and Czech full names work on their own (`Sunday`,
  `pátek`); abbreviations need `on` or `by` (`on sun`, `by fri`, `by pá`). Both words are removed from
  the title. Dates are case-insensitive, and Czech words also work without accents.
- Relative dates: `in 2 days`, `in 3 weeks`, `za 2 dny`, `za 5 dní`, `za 2 týdny`, `za 5 týdnů`.
- `next week` / `příští týden` means next Monday. `next Sunday` / `příští neděli` means Sunday of
  the **next** Monday–Sunday week, even when this week's Sunday hasn't happened yet.
- `this weekend` / `o víkendu` means Saturday, or today on Saturday or Sunday. `end of month` /
  `konec měsíce` means the last day of the current month. If `o víkendu` means “about the weekend”
  rather than a due date, type `o \víkendu` to keep the words in the title.
- `1 Dec`, `1. 12.`, `1.12.` and `1. prosince` mean the next December 1, including today. The year
  rolls forward if needed. Day-month and end-of-month dates can start with `on`, `by` or `do`:
  `Pay by 1 Dec`, `Pay by end of month`, `Zaplatit do 1. prosince`, `Zaplatit do konce měsíce`.
- Compact `D.M.` dates stay literal immediately after `v`, `ve`, `at`, `version`, `verze`, `section`,
  `sekce`, `chapter`, `kapitola`, `release`, `part` and `část`: `Release 1.12.` and `Sraz v 10.10.`
  have no due date. Elsewhere type `\1.12.` to keep the number. Incomplete phrases like `Meeting in 2`
  and bare numbers stay in the title.
- A separate `!!` sets normal priority; `!!!` sets high priority. `Wow !` and `Wow!` keep their punctuation.
- Put `\` before the final title token to keep it literal: `Enjoy the \sun` becomes `Enjoy the sun`,
  and `See you \tomorrow !!` becomes `See you tomorrow` with normal priority and no parsed date.
  The backslash stops suffix parsing at that token and is removed.

The room composer, including in pop-out windows, previews both fields as removable chips. Tab reaches
**Choose due date** and **Choose priority**. The calendar offers **Today**, **Tomorrow**, **Next week**
(next Monday), **No date**, and a Monday-first month grid. The arrow keys move by days or weeks,
**Page Up/Down** move by months, **Enter** chooses and **Esc** returns focus to the opener without a change.
The Flag offers the same four levels as the task Priority row. A pick replaces only its own typed suffix;
its chip's remove button clears the field, including the Planned or Important default. The fixed global
quick-add window still takes typed syntax only. Task detail's **Due** row opens the same calendar on the
saved date or today. Choose a preset or any day, including past dates, to save it right away. **No date**
clears the due date and any repeat rule. For a repeating task the note below the grid explains this
before you choose. **Esc** returns to the Due row without saving.

## The shell

| Feature | Where | Notes |
|---|---|---|
| Tray icon and menu | `src/platform/tray.rs` | Show or hide, quick add, "Start with Windows" (checkbox), "Restart to update" (while an update is staged), quit. A left click toggles the window. The icon is the app icon from the exe's icon resource, loaded at the small-icon size of the notification area's DPI (16 px at 100 %, 20 at 125 %, 24 at 150 %, 32 at 200 %). A hidden watcher window (`platform/icon.rs`) reloads it when the DPI, the display, the system settings or Explorer change. Without the resource it falls back to the embedded 32 px render. The main window's icons follow its monitor's DPI the same way (see `vendor/gpui_windows/VENDORED.md`). |
| Close hides to tray | `src/app.rs` | Closing the main window hides it without closing any list windows. "Quit" and "Restart to update" exit the running process. If the tray icon can't be created, the main window is shown even with `--background`, and closing it quits, so the process is never left running invisibly. |
| Single instance | `src/platform/single_instance.rs` | A named mutex marks the running instance, and a named event activates it. A second launch signals the event, allows the first to take the foreground and exits. The names carry the user's SID and live in the session's `Local\` namespace, so each user gets their own instance, and both objects have a DACL that admits only that user. The event is created before the mutex is claimed, so a launch that races the first one's startup is never lost. If the check can't be set up (an elevated copy created the objects, say), the app logs it and runs standalone: visible, and not listening for second launches. |
| Global quick add | `src/platform/hotkey.rs`, `src/ui/quick_add.rs` | **Ctrl+Alt+Space** or the tray's Quick add item opens an always-on-top window kept hidden between uses. It prefers 640 × 196 logical pixels at the monitor's DPI, with no resize or maximize controls. It opens horizontally centered, a fifth of the way down the cursor monitor's work area, shifting or shrinking to fit if needed. If the cursor's monitor is unavailable, it tries the foreground window's monitor, then the primary monitor. Enter adds the task to My Day (before pairing, it shows the pairing window instead). Esc or clicking away dismisses it. If another app owns the shortcut, the tray item says so and the quick-add window drops its shortcut hint; the tray item still opens it. |
| Reminder toasts | `src/model/reminders.rs`, `src/ui/shell/reminders.rs`, `src/platform/toast.rs` | See [Reminder toasts](#reminder-toasts). |
| Start with Windows | `src/platform/autostart.rs` | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `To-do or not to do`, `"<exe>" --background`. Per user, so no admin rights are needed. It's toggled from the tray. |
| Main window | `src/ui/root.rs`, `src/ui/shell.rs` | Pairing until paired, then the custom title bar with the filter, sidebar, list, detail pane and status bar, as in F2. The panes show quiet scrollbars when they overflow. Drag the sidebar or docked detail divider to resize it, or Tab to a divider and use the arrows. Double-click, Home or Enter resets the divider. The caption buttons keep Windows' native hit testing for Snap Layouts. |
| Header landscape | `src/ui/landscape.rs` | The sky with two flat hills and the sun on its arc at *done / total*, or the tall dusk scene once My Day is done. The geometry is `todo_design::landscape`; the paths are built once per size and progress, and are cached. |

### Pop-out list windows

Right-click a list or a built-in smart list in the sidebar, or open its **…** room menu, and choose
**Pop out**. Choose it again to bring the same window forward. The compact window keeps the room's hue,
progress landscape, quick-add, task rows, Completed toggle where applicable, and Undo. It has no sidebar,
detail pane, or My Day hand-over. The Windows caption controls still support resize, minimize and Snap
Layouts. The pin beside the caption buttons starts filled and keeps the window above ordinary windows.
Choose **Don't keep on top** to unpin it, or **Keep on top** to pin it again.

The main window and all list windows share one sync session and outbox, even offline. The window where a
completion is made plays the normal row choreography; other windows show the optimistic change on their
next refresh. **J/K** or the arrows select a task, **Space** completes or reopens it, **Ctrl+N** focuses
quick-add. Multi-selection uses the same shortcuts and bulk strip as the main window. **Esc** first
closes an open menu or clears the selection; otherwise it closes the window. Double-click a task,
press **Enter** on the selected task, or choose **Open in main window** from its menu to open its details
in the main window, bringing that window back from the tray if needed.

Closing a list window forgets that it was open, but keeps its pin and its normal (not maximized or
minimized) placement for the next **Pop out**. Quit leaves the open list windows marked for restore at
next launch: they return without taking keyboard focus, including on a `--background` launch, which hides
only the main window. If a display is gone, the window fits itself on an available work area. Removed lists
close once the replica has caught up. Sign-out closes all the list windows and removes their local settings
with that pairing. If sign-out fails and resumes the same pairing, it restores its open list windows.

Implementation: `src/ui/popouts.rs` opens ordinary non-modal GPUI windows, not `WindowKind::PopUp` (which
is always topmost in the Windows backend). `src/platform/win.rs` places the hidden HWND before showing it
and uses `SetWindowPos` with `HWND_TOPMOST` / `HWND_NOTOPMOST` without stealing focus. No vendored change
is needed. `src/ui/shell/popout.rs` draws the compact chrome around the same virtual list and motion code
used by the main shell.

### Reminder toasts

While the app runs, in the window or hidden in the tray, a reminder shows as a Windows toast (WinRT
`ToastNotification`, the *reminder* scenario, so it stays up until acted on): the task's title, when and
its list, with **Open** (shows the window with the task open, in the room the window shows if it's
there, else in its list), **Snooze 10 min** and **Complete**. Snooze is this computer's own, as on
Android: nothing is synced. Complete always submits `CompleteTask` (never a toggle, so a task
completed elsewhere meanwhile is never reopened), and only if the reminder is still as the toast
showed it (live, at the same instant, still on the original open task), checked under the sync client's
lock as the command is queued (`SyncClient::submit_tracked_if`): a sync that moved or removed the reminder, or
completed the task, between the click and the queue wins. It is an ordinary completion (Undo and
Ctrl+Z work), and the row plays it only when this click queued it.

- **What decides.** `model::reminders` is pure and unit-tested, with the toasts behind a `ToastSink`
  trait (the tests use a fake; nothing in the tests shows a real toast). It plans from
  `intent::reminder::alarms` (live reminders of open tasks) and what was shown before.
- **When.** The shell plans again after every change to the data (a sync, an edit, an agent's change,
  the half-minute tick), when the clock is changed and when the computer wakes up (`WM_TIMECHANGE` and
  the resume `WM_POWERBROADCAST`s, received by a hidden window, `platform::clock`). One wake loop
  waits for the next reminder or toast retry (wall-clock instants), or for a schedule save retry
  or queued click expiry (monotonic), in timers of at most a minute and at least a quarter of a second, so a timer
  replaced by a new deadline never lingers, a sleep never makes one more than a minute late, and a
  clock that jumped past a deadline can't make it spin. A reminder removed, rescheduled or moved to
  another task loses its toast, as does one whose task was completed or deleted.
- **Buttons.** A toast's buttons name the occurrence they were shown for: the reminder, its time and a
  token of this session, new for every toast. A button acts only while its toast is up, only on the
  reminder as it was shown (still there, at the same instant, still on the original open task), and
  Snooze and Complete only once. So a click queued behind a sync that moved, removed or completed the reminder
  does nothing, and Open from a toast no longer up brings the window forward without selecting
  anything.
  During catch-up, Snooze and Complete wait up to ten seconds, then expire. A Complete that can't
  be saved leaves the reminder pending so Windows can show it again.
- **Missed.** A reminder found due at most five minutes late (`GRACE_MS`) shows as itself. Staler ones
  (the app was closed, the computer asleep, a reminder synced in late) never fire one by one: they
  collapse into one summary toast ("3 missed reminders", with the first titles; a single one opens its
  task). The summary follows what it covers: completing or removing one of them rebuilds it quietly
  (straight to the action center), and it goes once nothing is left. Older than a day, they're not
  mentioned. Right after starting (and after a full resync), a reminder that is due waits until the
  replica has caught up with the server's head as it was reached (`todo_client::Readiness`, read
  together with the state; `Online` alone is reported before the backlog is in), so a task completed
  elsewhere meanwhile doesn't ring. If the first attempt fails, delivery still waits for catch-up.
  A partial or empty replica does not discard local snoozes.
- **Not shown.** If Windows can't show a toast (no notifier, notifications turned off for the app), or
  reports later that a toast it took failed (its `Failed` event, which names the occurrence), it isn't
  counted as shown: it's tried again after 30 seconds, then 2 and 10 minutes. Each toast backs off
  independently, and the warning is logged at most every ten minutes. If a summary fails, only its
  newly announced reminders go back to pending and can pop up again, even after a restart. Its
  previously announced members stay shown. Each toast keeps the notifier that showed it,
  so it can still be taken away after a later failure.
- **Once.** Each occurrence (a reminder at its time, or at its snooze) shows once, also across restarts:
  `reminders.json` keeps the reminder and task IDs and times only. A change that couldn't be saved is
  saved again on its own back-off (30 seconds, then 2 and 10 minutes), never on every refresh: a
  keystroke in the filter never writes it.
- **Quitting and signing out** (the tray's Quit, or closing the window when there is no tray) take the
  reminder toasts away (no one would answer their buttons; a toast
  clicked after the app has quit does nothing). The app only logs counts, at debug level, never a title.

**The app's identity (AUMID).** Windows only shows toasts for an unpackaged app that has an
AppUserModelID it can name. What we found: `VelopackApp::run` already sets the process's AUMID on an
installed copy, to `velopack.<packId>` (`velopack.todo-desktop`), but Velopack's Setup writes that AUMID
onto its Start menu and desktop shortcuts only when the package names one (`vpk pack --aumid`);
without it the shortcuts carry none. So `scripts/package.ps1` passes `--aumid velopack.todo-desktop`:
the shortcuts, the installed process and the toasts all agree, and Velopack updates existing shortcuts
to it on the next update. The app also sets the same AUMID itself at startup
(`SetCurrentProcessExplicitAppUserModelID`, `platform::toast::set_identity`), for a copy run from the
repo with no shortcut, and registers it per user under `HKCU\Software\Classes\AppUserModelId\velopack.todo-desktop`
(`DisplayName` and an `IconUri` written to `%LOCALAPPDATA%\to-do-or-not-to-do\toast-icon.png`), which is
what Windows shows on a toast when no shortcut names the app. Nothing needs admin rights; uninstalling
removes the registry key. GPUI has system notifications of its own at our pin, but they are silent, have
no reminder scenario and can't be taken away by tag across runs, so the app uses WinRT directly through
the same `windows` crate release as the vendored `gpui_windows`.

### Motion

The motion follows `docs/design-language.md` (completion, dusk, room and detail navigation, sidebar and scrollbars). Its timing and state live in a pure
model, `src/model/motion.rs`, driven by a monotonic clock the view passes in and unit- and property-tested
there; the view only samples it each frame. Every duration, curve and cue comes from `todo_design::motion`.

- **Completing a task.** The command applies at once. The disc pops in (`CHECK_FILL`, `POP`), the tick and the
  title's strike-through draw from the left as the title dims (`CHECK_TICK` after `CHECK_TICK_DELAY`), the
  row holds still (`COMPLETE_HOLD`), then fades and collapses (`ROW_COLLAPSE`, `EXIT`) while the rows below
  rise to close the gap. Only then is it released: it leaves the open rows (entering an open Completed
  section, `ROW_ENTER`), and the counts (header, status bar, sidebar) change and the sun travels to the new
  count (`SUN_TRAVEL`, `SUN`). Until the release the task counts as open (`UiState::holding`).
- **Reopening** a completed task mirrors it: the disc clears (`OVERLAY_OUT`), the row collapses out of its
  completed place (it counts as done until then, `UiState::reopening`) and enters the open rows.
- **Deleting and moving away.** The command applies at once; a row that leaves the room (deleted, or moved
  to a list the room doesn't show) keeps its place (`UiState::leaving`, the task as it was) and fades and
  collapses from wherever it is (`ROW_COLLAPSE`, `EXIT`; with reduced motion it only fades, in
  `REDUCED_FADE`), and only then is it released (`Release::Left`); the counts and the sun follow. It is
  paint-only meanwhile: no click, J, K or Space reaches it. Undo during the exit brings it back from where
  it is; afterwards it enters (`ROW_ENTER`). A completion (or reopening) the delete cut short picks up
  again from there: the task is held in the open rows (or its completed place) until it has played.
- **Interrupting.** Toggling a row again part way (a second click, Space, or the detail pane's checkbox)
  reverses it from where it is, in place: a row in its hold or half collapsed comes back without a jump, and
  nothing is released. A change from elsewhere (another device, an agent, a refused command) settles a
  choreography the same way; switching rooms releases the old room's held rows.
- **The list.** Its rows keep `uniform_list`'s fixed slots until a row is released: a leaving row collapses
  inside its slot and the rows below rise by what the rows above them gave up. Rows rising into view from
  below the last rendered slot are drawn (and take clicks) with it. In a list scrolled to its end, the scroll
  comes back as rows collapse, so the rows above close the gap and nothing jumps when the slot goes. A row the
  list stops showing (deleted, filtered out) stops moving at once.
- **Navigation follows what shows.** While dusk falls, J, K and clicks reach a hand-over task only once it has
  begun to enter.
- **The sun** eases from where it is to wherever the count puts it, whatever changed it, retargeting
  mid-travel. Switching between populated rooms carries it along the arc; at an empty endpoint it fades in or out.
- **Room switches.** The palette follows the shortest hue arc over `ROOM_HUE`. The title and eyebrow overlap
  briefly as they trade places; the list crossfades and resets to the top. Outgoing layers are paint-only.
  Switching again keeps the stronger title and list layers independently. The weaker layers are dropped
  immediately (which can be visible near the midpoint of a crossfade), so repeated switches never stack
  old rooms. Outgoing rows keep the collapse, opacity and lift of their last painted frame.
- **Wide detail.** The placeholder fades into the selected task, which enters from the right by 8 px. The
  column stays at full width until its exit finishes, including when dusk removes the empty column. Switching
  between tasks fades the body in place; rapid J/K selection switches immediately. Resize across the narrow
  breakpoint settles the pane instead of replaying the entrance.
- **Sidebar.** The selection ring and raised fill crossfade. A collection clips and fades its child rows while its
  chevron turns; clicking again reverses from the displayed height. Reduced motion snaps the disclosure.
- **Scrollbars.** Scrolling or hovering reveals the thumb. When neither hovered nor dragged, it holds for
  `SCROLLBAR_HOLD` without requesting frames, then fades out. A room or detail scroll reset doesn't
  reveal the thumb. Windows’ *Always show scrollbars* keeps
  overflowing thumbs visible. At rest, gallery fixtures show them regardless of OS preference.
- **Menus.** The context menus (a list's, a collection's, a task's), the room's menu, the account menu,
  the priority, reminder and repeat pickers, the list picker, the collection picker, the colour swatches, the tag picker and
  the "couldn't sync" list are data (`model::menus`), drawn by `ui/shell/menu.rs`. A menu turns into
  its next page in place (`menus::Page`, `TaskPage`, `TagPage`): a task's into the priority picker, the
  list picker or its links; a list's, a collection's or the room's into the delete confirmation, the
  collection picker or the colour swatches; the tag picker into a tag's own page, its swatches or its delete confirmation; the repeat picker into the custom editor; the reminder picker into its custom day and time. The
  menu stays where it is and the new page fades in over `CONTENT_SWAP` (`ENTER`; `REDUCED_FADE` with
  reduced motion, `motion::Overlay::swap_content`). A picker starts with the keyboard on the current
  choice. The swatches sit in rows of six
  (`menus::SWATCH_COLUMNS`): Left and Right walk them in order, Up and Down a row at a time; the
  current one is ticked, and hovering one tints it in its own colour. A menu
  longer than twelve rows scrolls, and the keyboard scrolls it to its highlight; the list picker's filter
  line stays above the lists. A menu enters over `OVERLAY_IN` (`ENTER`) while it moves
  its last 4 px away from its button, and fades out in place over `OVERLAY_OUT` (`EXIT`), paint-only:
  a fading menu neither blocks nor takes clicks. With reduced motion it only fades
  (`model::motion::Overlay`). The press that closes a menu is remembered, so the click it ends on the
  menu's own button leaves it closed; a later click opens it, even while it still fades. Presses are
  counted by a window-level capture listener (`menu::PressGuard`), so a dismissal on a pane that
  occludes the shell (the narrow detail overlay) never swallows the next click. It takes the
  keyboard: **Up** and **Down** (**Left** and **Right** between a confirmation's buttons) walk the
  items, **Enter** or **Space** choose, **Escape** or **Tab** close it and give the keyboard back to its
  button. An item chosen from the keyboard does the same unless it opens something of its own (an
  editor, another room, sign-out; `MenuAction::moves_focus`). Anything else that takes the keyboard
  (**Ctrl+K**, **Ctrl+N**, a click into a field, another window) closes the menu. Deleting a list or a
  collection and signing out ask first: the menu becomes a confirmation (`desktop::CONFIRM` wide) that says in full what
  will happen, wrapped, above two short buttons (**Delete list**, **Delete collection** or **Sign out**, and **Cancel**); from
  the keyboard it starts on Cancel. A delete confirmation keeps the question as it was asked
  (`menus::Asked`): if the names or counts in it change while it's open (a sync, another device, an
  agent), confirming deletes nothing and asks the new question instead, with a line in the danger ink
  saying the list or collection changed, the keyboard on Cancel again (`menus::on_confirm`). A
  confirmation is modal: a scrim dims everything below the title bar
  (which keeps the window's own controls) and fades with it. While it's open the scrim takes every click
  and hover meant for what's under it, so a click outside only dismisses the confirmation and never also
  completes a task; once it's closing, the scrim is paint-only. **Shift+P** opens the priority picker from the list, on the current
  level, and the keyboard comes back to the list once one is chosen. **Tab** and **Shift+Tab** go round
  the stops in `hints::TAB_ORDER` that are there, from one shell-level handler: the sidebar divider, the
  list, Undo (while the toast is up), the room's "more" button, in Activity its Clear filter (while
  filtered), Refresh activity and the button at the end of the feed (while there is one), the detail
  divider (while the docked detail shows it), the detail pane's Reminder and Repeat rows (while a
  task is open), the staged update indicator and its dismiss button (while shown), **+ New**, the
  sync status (while something couldn't sync) and the account gear.
  The footer stays in view below the sidebar's scrolled rooms. A stop inside a scrolled pane (a detail row below the fold or the button at
  the end of the feed) is scrolled into view when Tab lands on it, as the link field is when **L** puts the keyboard
  there (`layout::reveal_offset`). Held or modified Tab, Tab in a text field, and Tab in an open
  menu (which closes it) are left alone. A focused button shows the focus ring, and **Enter** opens its
  menu.
- **Undo.** Completing a task, moving it up/down in a list or to another list, adding or deleting a
  link, deleting a task, a step, a list or a collection, or moving a list to another collection shows
  one toast in the reserved footer ("Deleted “…”", "Moved “…” to Errands", "Moved “Office” out of
  Work", "Deleted collection “Work”", "Added link “…”"; undoing an addition takes the link away),
  to the right of Completed and outside the rows' viewport and narrow detail overlay. It enters over
  `TOAST_IN`, swaps its message in place over `CONTENT_SWAP`, and leaves over `TOAST_OUT`. The
  `UNDO_WINDOW` countdown pauses while the toast is hovered or its Undo button is focused. The Undo button
  is the next **Tab** stop after the task list. **Ctrl+Z** outside text fields (on the list, Undo, a
  splitter, a menu or a menu's button) can still undo the last change after the toast closes, with
  feedback in the status bar; from an open menu it closes the menu first. Leaving the room or editing that
  task clears the old target. Undo cancels the latest unsent command when nothing follows it in the
  outbox; otherwise it queues a non-forced undo tied to that command's exact server event. The
  shared view shows the undo before sync when it knows the whole original batch, including the next
  occurrence spawned by completing a repeat: the pending command's own batch, or the server's once
  every event of it has arrived (events carry `command_len`, or an older server's receipt lists them,
  and the sync engine applies only whole batches). An undo the server would refuse is not shown. Conflicts appear in the status bar; Undo
  does not overwrite another device's changes. Undo during a held completion reverses the row in
  place; other restored rows enter over `ROW_ENTER`. Undoing a reorder does not replay row entrance. Undo is
  the exact inverse: it brings back the deleted task, step or list, never what it belongs to. When that
  was deleted meanwhile (on another device), the status bar says so in one short line without names,
  such as "Step restored, but its task was deleted elsewhere."
  Narrow detail content can scroll its last row clear of the toast strip.
  Deleting the list or collection the window shows (or the collection its list is in) takes the window
  to My Day, and its undo goes along.
- **Activity.** The rows a page brings in fade in together and rise the last 4 px into place over
  `ROW_ENTER` (`ENTER`), never one after another; rows already shown stay put when a refresh reads
  newer ones. The selection's wash and bar crossfade between changes over `HOVER` (`STANDARD`), and a
  refusal's line fades in beside its change over `TOAST_IN`. With reduced motion nothing rises and the
  fades are at most `REDUCED_FADE` (`model::activity::ActivityMotion`).
- **Dusk.** Once My Day's last row has left, the header grows to the dusk scene over `DUSK` (`SUN`).
  The sun travels from where it is over `SUN_TRAVEL` in both scenes. The scene fades in over the strip,
  the text switches to the dark theme's ink and the moon and stars fade in at `DUSK_NIGHT_AT`, and
  tomorrow's hand-over enters from `DUSK_HANDOVER_AT`, its first items `STAGGER` apart.
  Reopening or adding a task turns it back over `ROOM_HUE`.
- **Reduced motion** (Windows' *Animation effects* off, `SPI_GETCLIENTAREAANIMATION`, read whenever a
  transition starts): `motion::reduced` everywhere. The checkbox and the strike switch at once, the title dims
  and the row fades in `REDUCED_FADE` but never slides, the hold keeps its full length, the sun jumps, and
  dusk crossfades while the header takes its new height at once. Toasts fade without sliding, and
  their countdown keeps its full duration.
- **Idle is idle.** The view asks for the next frame only while something moves; a row holding still waits
  on a single one-shot timer; at rest there are no frames and no timers. The landscape's paths are rebuilt
  per frame only while the sun travels or dusk falls.

The screenshot fixtures draw everything at rest (`ShellView::still`). The motion fixtures freeze a
transition at a moment instead (`ShellView::freeze`, a fixed clock), so its frames can be reviewed.

### Design tokens

Every visual value comes from `todo-design` (see `docs/design-language.md`): palettes per theme and room
hue, list and agent colours, the dusk scene, text styles, spacing, radii and the desktop geometry.
`src/theme.rs` is a thin adapter that turns token colours into GPUI colours and applies text styles; the
views never hard-code a colour or a size. GPUI has no letter spacing at the pinned revision, so tracking is
only faked where it matters (the priority marks). A checked box on a selected row uses `check_selected`.

Icons are the shared Lucide subset and list glyphs (`todo_design::Icon` and `Glyph`), served to GPUI by
`src/assets.rs`.

### Fonts

The fonts are the shared static instances in `assets/fonts`, embedded in the binary through
`todo_design::fonts::FACES` (the `embed-fonts` feature) and registered at startup. The app never uses system
fonts. The families are Geist (titles, eyebrows and interface text) and Geist Mono (counts, times and other
metadata), both under the OFL.

## Screenshots

```sh
todo-desktop --screenshot-all <out-dir> [--overwrite]  # all fixtures × both themes × 100 % and 150 %
todo-desktop --screenshot <out.png> --fixture <name> [--theme light|dark] [--scale 1|1.25|1.5|2] [--overwrite]
```

Both modes use the same below-normal-priority fixture runner. It never enters interactive startup: no
SID-scoped instance mutex or activation event, profile/cache, network, tray, global hotkey or autostart.
It cannot activate an existing instance or receive a handoff. The batch loads GPUI and the fonts once,
then opens and closes **one never-shown HWND per shot**. If the batch includes quick add, it first checks
that the hidden production popup has the right native size at real monitor DPI, before and after positioning
it on the cursor's monitor. The quick-add capture then uses the same window options at the fixture scale.
GPUI's Windows headless platform has no real
text system or Direct3D window renderer. Instead the runner sizes a hidden window without activating it,
requests paint messages and reads its own render target. No visible window, taskbar entry, sound or input.
A capture never samples the live pointer: unlike the old visible gallery, it cannot accidentally show a
hovered row. To render a subset in one process, add `--fixture my-day,quick-add --theme dark --scale 1,1.5`
to `--screenshot-all`. For visual checks, use these modes, not the interactive app.

A fixture is a `todo_core::State` built from invented sample data by running commands through the domain
(as the owner and as agents, at fixed times), plus UI state, a fake sync status and a fixed clock. The
window is the real shell on a local store (`src/store.rs`). `pairing` and `pairing-code` show the pairing
screen before and after asking for a code. `open-failed` and `open-failed-confirm` show the "couldn't be
opened" screen after a recovery that can't finish, before and while confirming **Start fresh**.
`long-sidebar` and `long-task-list` show the scrollbar thumbs (the latter also overflows the detail pane).
`short-sidebar` is the long sidebar at its narrowest in the smallest window (960 × 600): the rooms scroll
while the footer stays pinned. `short-sidebar-new-list` and `short-sidebar-new-collection` show the
new room's focused editor scrolled into view. `sidebar-footer` and `sidebar-footer-min` show the footer
at default and minimum widths; `sidebar-new-menu` opens the menu.
`list-status`, `list-settings`, `board` and `board-detail` show the same workflow list with a staged
composer, the settings page, the full board, and the board with the collapsed Done rail beside Detail.
`room-menu`, `account-menu` and `priority-menu` show the menus open, at rest: a list room's menu, the account
menu and the detail pane's priority picker. `updates-up-to-date`, `updates-checking`, `updates-downloading`,
`updates-ready`, `updates-failed`, `updates-unverified` and `updates-disabled` show the Updates page without checking a real feed; `updates-indicator`
shows the ready control in the sidebar with Settings closed. `quick-add-chips`, `quick-add-calendar` and `quick-add-priority`
show the two composer chips and both pickers from the same draft. `detail-due-calendar` shows the shared
calendar and repeat warning anchored to the repeating task's Due row. `task-context-menu` is a task's menu on the row below the selected
one, as a right-click leaves it; `move-to-picker` is the selected task's list picker as M opens it, grouped by
collection with the keyboard on the task's own list; `sync-failures` is "3 couldn't sync" opened, with three
refused changes in plain words. `merge-task-picker` shows the destination search and list locations;
`merge-task-confirm` shows the merge confirmation with Cancel selected. `multi-select` shows three
selected tasks, the bulk strip and the count in the detail pane. `drag-reorder` shows a lifted pair
and its insertion line, and `drag-destination` shows the highlighted My Day target. Activity: `activity` is the feed over the sample data,
`activity-agent` the same filtered to Claude (opened from the sidebar's agent row), `activity-empty`
the feed with nothing in it, `activity-pending` a change being undone from the keyboard (selected,
Enter, waiting to be sent) and `activity-refused` an undo the server refused, its reason under the
change. `activity-menu` is the room's menu, on All changes. `delete-list-confirm` is a list with a long name being deleted
from its room's menu, its confirmation wrapped in full with the keyboard on Cancel. Organizing:
`new-collection` is a collection just added and being named in place; `tab-new` is Tab landing on
**+ New** in a long sidebar; `collection-menu` and
`list-context-menu` are the Work collection's and the Errands list's context menus at their sidebar
rows, as a right-click leaves them; `delete-collection-confirm` is the Work collection being deleted
from its room's menu; `move-list-picker` and `list-colour` are a list's room menu turned into the
collection picker and the colour swatches, the keyboard on the current choice. Tags: `tag-picker` is
the tag picker on the detail pane's Tags row, `tag-create` the same with a new name typed (Create
first), `tag-edit` and `tag-colour` a tag's own page and its swatches, and `tag-rename` a tag being
renamed in the row. `delete-tag-confirm` shows the task count before deleting a tag. Reminders: `reminder-menu` is the picker on the open task's reminder (Today 17:00, so Pick a day and time… is ticked, and Remove is offered), `reminder-custom` its custom page with tomorrow and 08:30 typed, `reminder-none` the row of a task without a reminder, and `reminder-focus` the row's keyboard focus ring. `repeat-menu` shows the undated-task notice; `repeat-custom` shows a fortnightly Mon/Thu draft. `repeat-none`, `repeat-weekly` and `repeat-monthly` show the row at rest; `repeat-focus` shows its keyboard focus ring. Links: `task-links` is a task's menu turned into its links, and `add-link` the
link field with an address being typed. `email-drop` shows the list's drop highlight and destination card, without opening any file.
`caption-hover` shows the maximize hover tint; `caption-close-hover` shows the red close hover and restore glyph.
`popout-pinned` and `popout-unpinned` show the same list and selected row in a 480 × 528 list window,
including a long Czech title and the two resting pin states. On each of these hidden windows, the runner
also checks that the Windows topmost style follows both pin changes without showing the window.
`sidebar-wide` and `detail-narrow` show the resize extremes, and `resize-handle-hover` shows the sidebar
separator at the end of its hover fade.
`long-names-narrow` and `narrow-detail` are 960 px wide (the narrowest window): the first with nothing open,
the second with the detail pane over the list. `finished-nothing-tomorrow` is the finished day with nothing due
tomorrow. The motion fixtures freeze a transition part way (see *Motion*): `completing-fill`, `completing-hold`,
`completing-collapse-50`, `sun-travel-50`, `reopening-collapse-50`, `long-list-collapse-80`,
`long-list-end-collapse-50`, `handover-complete-50` (tomorrow's first task completed from the finished day's
hand-over, half way through its collapse), `dusk-50`, `dusk-75`, `dusk-one-task-50`, `room-switch-50`, `room-hue-50`,
`detail-open-50`, `detail-dusk-50`, `collection-collapse-50`, `scrollbar-fade-50`, `undo-toast`,
`narrow-undo-toast` and `toast-in-50`.
`trash` shows a task with Restore, a deleted collection, and items blocked by an original parent;
`trash-empty` shows the empty state. `task-activity` shows a task created by Codex, completed by Pi and
then edited by the owner. These states are set by the fixture, not by moving the pointer or
maximizing the capture window.

Before rendering quick add, the runner checks the hidden native popup at startup and moves it to a
known work-area anchor, checking its origin and size at the destination DPI without moving the pointer.
The runner requests five settling frames before reading back the next frame. It doesn't capture the
screen. It refuses to replace an existing file unless `--overwrite` is given. It checks that each image
is the window's logical size times the scale. Exit code 0 means all PNGs were written and flushed to disk,
3 means a font family is missing, 4 means the output exists, and 1 is any other failure (including a
wrong image size).
If a shot fails mid-batch, the runner exits 1 and leaves the partial gallery on disk; earlier PNGs are
not rolled back.

`scripts/gallery.ps1` renders every fixture in both themes at 100 % and 150 % into
`apps/desktop/target/gallery/` (`<fixture>-<theme>-<pct>.png`, git ignores it), passing `--overwrite`. It
samples the foreground owner during the run and fails if it sees the renderer take focus (the owner can still
switch between their own apps). It prints the render and validation time, excluding the build. It also
checks that each PNG is complete (the signature and the final `IEND` chunk), and fails if its size doesn't
match the fixture's logical size times the scale (1440 × 900 for the reference main window, 640 × 196 for
quick add and 480 × 528 for list windows). CI runs it and uploads the gallery as the
`desktop-gallery` artifact. Compare the images with the F2 mockups before calling a UI change done.

## Benchmarks

```sh
todo-desktop --bench startup --metrics <file>   # process start to the main window's first presented frame
todo-desktop --bench hotkey --metrics <file>    # hotkey handler to the quick-add window's presented frame, 20 samples
```

Both write `key = value` lines. `startup` measures from the process creation time that the kernel records
(`GetProcessTimes`), so the loader and runtime startup are included. `hotkey` runs exactly what the
`WM_HOTKEY` handler runs, against windows that have been hidden in the tray. It measures up to the present
call; DWM shows the frame at the next vblank, which adds up to one refresh interval.

To measure with real data, build a paired home with a warm cache of 600 tasks (mostly completed) from an
in-process server, then start the app on it (the server is gone, so it starts offline from the cache):

```sh
TODO_BENCH_HOME=<dir> cargo test --release make_a_bench_home -- --ignored
TODO_DESKTOP_HOME=<dir> todo-desktop --bench startup --metrics <file>
```

`data_loaded_ms` is when the loader thread's config, token and cache are ready, before the first window
opens.

Measured on a desktop with a discrete GPU, two 1080p monitors at 100 %, a release build, warm file caches
and the 600-task cache:

| Metric | Target | Measured |
|---|---|---|
| Cold start to first frame, with data | < 500 ms | 365–451 ms over 6 runs (median ≈ 401 ms). About 300–350 ms of it is GPUI's own platform setup (Direct3D device, DirectWrite); the cache is loaded in parallel and ready about 3 ms after it. |
| Hotkey to quick-add presented | < 100 ms | median 2.1 ms, p95 3.4 ms, max 6.2 ms (20 samples), plus at most one vblank |
