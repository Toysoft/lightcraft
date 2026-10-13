# Nextcloud photos

LightCraft can work on photos kept on a Nextcloud server without mounting it and without downloading every original:
browse the whole archive in the grid and the loupe, then develop the shots you choose. Parity rows `LR-IMP-NEXTCLOUD`,
`MENU-FILE-NEXTCLOUD-ACCOUNT`, `MENU-FILE-NEXTCLOUD-IMPORT` and `MENU-PHOTO-OFFLINE` (beyond Lightroom; Adobe's own
cloud sync, `LR-CLOUD-SYNC`, stays out of scope). The desktop app, `lightcraft-cli` and the MCP server have it; the
web build doesn't.

The client (sign-in, certificate pin, WebDAV, previews, Memories, the originals cache) is the shared `craft-nextcloud`
crate from [craft-libs](https://github.com/storytold/craft-libs) (its `docs/nextcloud.md`). This page covers what
LightCraft does with it. Code: `crates/engine/src/remote/` (accounts, routing, import, downloads),
`crates/engine/src/cmd/nextcloud.rs` (commands), `crates/ui-egui/src/nextcloud.rs` (dialogs).

## Setting up

1. **File ▸ Add Nextcloud Account…** and type the server's address (`cloud.example.com`; `https://` is assumed).
2. **Certificate.** A server whose certificate your computer already trusts goes straight on. One it doesn't trust
   (self-signed, a private CA, expired) shows its SHA-256 fingerprint. Check it with the server's admin (or on the
   server: `openssl x509 -fingerprint -sha256`), then **Trust This Certificate**. LightCraft pins exactly that
   certificate for this account (see [Certificates](#certificates)).
3. **Approve in the browser.** LightCraft opens the server's login page (Login Flow v2). Sign in there and grant
   access to "LightCraft". The dialog waits ("Approve LightCraft in your browser…", with the link in case no browser
   opened); the activity stack shows a cancellable "Waiting for Nextcloud approval…" row. Nothing is added until you
   approve; after 20 minutes it gives up.
4. **Import.** **File ▸ Import from Nextcloud…** lists the account's folders one level at a time (› opens a folder,
   ‹ goes up). Tick the folders to import (their subfolders come along) and **Import**. The dialog closes; the import
   runs in the activity stack, and its problems arrive as messages.

Importing again is a rescan: photos already in the library are skipped, new files are added and missing metadata is
filled in.

**Capture times** are the camera's local time, as for photos on this computer. The Memories timeline gives the moment
in UTC, so a photo listed from it is first off by its time zone; the metadata phase then sets the local time from
Memories' `datetaken` or the file's EXIF. The first download sets the file's own only on a photo still on the
listing's estimate (one the metadata phase didn't read), so a time you changed with Edit Capture Time stays.

**Settings ▸ Nextcloud** lists the accounts on this computer with their state (Signed In, Signed Out, Unreachable,
Signing In…), **Sign In Again** and **Remove**, the cache size (5, 10, 20, 50 or 100 GB) and how much it holds
("Cached: … · Offline copies: …").

## What is downloaded when

The rule: **browsing never downloads an original; developing downloads the photo you work on, and the next one.**

| You are… | What LightCraft reads from the server |
|---|---|
| Importing | Listings, the Memories app's timeline and metadata when the server has it, else the first 64 KiB of each file (1 MiB when that has no camera data). Never a whole original |
| Looking at the grid | The server's preview of each photo, at thumbnail size, kept in the thumbnail disk cache; for a raw the server has none of, the raw's own embedded preview from the file's first MiB (a Range read). An edited photo's thumbnail is rendered from its cached original or smart preview, or, for a non-raw photo with neither, from the server's preview. Never an original |
| In Detail at Fit or Fill, Compare, the second window, **unedited photo** | The *quick view*: the server's preview (or a raw's embedded one, as in the grid), asked for again at a larger size as the view grows (up to 2,560 px). The original is downloaded only when there is neither, by the view's own render, which moving on stops |
| Looking at an **edited** photo in Detail, Compare or the second window | Its original, from the cache or downloaded once: an edit is rendered from the real file, never from the server's JPEG |
| **Developing** | The open photo's original, held in the cache while it is open, and, once it is here, the next photo's (filmstrip order), prefetched |
| Exporting, denoising | The original, from the cache or downloaded once |
| Reading all metadata | The original when it is on this computer, else the file's first MiB (where its tags are) |
| Building previews or smart previews, the face scan, Find Similar, Assisted Culling | Never an original: they use cached originals and the server's previews, and skip photos that would need a download (Build Previews counts them as `skipped`) |

**Develop** means the Detail view with an edit tool open (Edit, Profiles, Crop, Remove, Masking, Red Eye) or zoomed
to a percentage (1:1 and the like). Opening a photo's Edit panel downloads it; arrowing through photos at Fit with only
the Info panel open doesn't.

- **The download** shows as a cancellable "Downloading _DSC1748.ARW (61 MB)" row with progress, and the loupe keeps
  the server's preview up meanwhile. Downloads run one at a time; the photo you are waiting for goes before a
  prefetch ("Prefetching …"), and the next photo's prefetch starts only once the open photo's original is here.
- **The window never waits for a download.** What runs in the app's window (menu items, shortcuts, buttons, the
  control channel) never downloads an original. A command that needs one that isn't here (Auto, the white-balance
  picker, Auto Straighten, …) says "The original isn't downloaded yet; open it in Develop to download it"; work at
  thumbnail size uses a JPEG's server preview instead. Renders, exports, denoising and the CLI and MCP server, which
  run elsewhere, download as usual.
- **Moving on stops it.** Moving to another photo, leaving Detail or closing the view cancels the download of the photo
  you were developing (between chunks; a server that stopped answering is noticed at the 30 s stall limit). The
  prefetch of the next photo is cancelled by the next prefetch, and the open photo's own prefetch keeps going while you
  stay on it.
- **After the first download** the photo's record is completed from the whole file (dimensions, the capture time
  when only the listing gave one, as-shot white balance, lens data) and a smart preview is built, so the photo can
  still be edited when the server is out of reach. (While another preview build runs, that smart preview is skipped;
  File ▸ Previews ▸ Build Smart Previews makes it later.)
- **Server previews stand in at thumbnail size only** (the grid, and quick looks such as Find Similar). Once the
  original is downloaded they are dropped (a bigger source decoded from the original stays), so Auto, the
  white-balance picker and spot sampling measure the real file.
- **Make Available Offline** (Photo menu) downloads the selected photos' originals and pins them, one at a time, behind
  a "Making photos available offline" row (N of M). **Remove Offline Copy** unpins them; the cache then evicts them
  like any other download when it needs the space.
- **Not for Nextcloud photos:** Photo Merge (HDR, Panorama), Convert to DNG, Duplicate, Edit In, renaming and moving
  files, folder operations, Synchronize Folder, Relink and Find Missing, writing XMP sidecars, and Show in Finder /
  Explorer. They act on files beside the original or write to the server, and say "… isn't available for Nextcloud
  photos". `library.import` refuses `nextcloud://` paths too (use `library.importNextcloud`).

## Caching

- **Originals** downloaded for Develop, export or offline use live in `<cache>/nextcloud/`, where `<cache>` is
  `~/Library/Caches/LightCraft` on macOS, `%LOCALAPPDATA%\LightCraft\Cache` on Windows, else
  `$XDG_CACHE_HOME/lightcraft` or `~/.cache/lightcraft`. `LIGHTCRAFT_CACHE_DIR` overrides it (the complete directory);
  an empty value turns the cache off, and then nothing can be downloaded. File names are hashes, never the server's
  names.
- **Budget:** 20 GB by default (Settings ▸ Nextcloud, or `nextcloud.preferences {cacheBudgetMb}`, at least 1,000 MB).
  Above it, the least recently used originals are deleted, except offline copies (pinned) and the photo open in
  Develop (held).
- **A file changed on the server** (new etag) is downloaded again when next needed; the old copy goes.
- **`etags.json`** in the same folder remembers which version of each file a copy is, so copies are found while the
  server can't be asked, after a restart too. A damaged or deleted one only means that copies are found once the
  server answers again.
- **Thumbnails** made from server previews go to the usual thumbnail disk cache, keyed by path and size (Nextcloud
  photos have no content hash).
- **Accounts** are in `<settings folder>/nextcloud.json` (`{accounts: [{server, user, trust}], cacheBudgetMb}`); no
  password is ever written there.
- The desktop app, `lightcraft-cli` and the MCP server share both folders, but **one process at a time uses the
  download cache**: the first to need it keeps it (a lock on `<cache>/nextcloud/.lock`) until it quits. Meanwhile the
  others can't download ("Another LightCraft window or lightcraft-cli is using the Nextcloud download cache; close it
  and try again") and show no cache sizes; browsing with server previews still works there.
- A desktop-app window started with `--memory` or `LIGHTCRAFT_NO_PREFS` (tests, scripts) leaves this computer's
  Nextcloud settings, cache and keyring alone: it can't sign in ("Nextcloud sign-in isn't available in this window").

## When the server can't be reached

- **One failed connection marks the account unreachable for 30 s.** Meanwhile background work (thumbnails, prefetch,
  offline copies, metadata reads) gives up at once instead of each waiting for a timeout, and photos count as offline
  unless their original is on this computer.
- **Editing goes on:** an edited photo renders from its cached original, even an older version of it, or from its
  smart preview ("Original offline · editing the smart preview"). The grid keeps its cached thumbnails.
- **Opening a photo in Develop** doesn't wait for it either: it opens the copy of the original on this computer
  (downloaded or made available offline, before a restart too; it may be out of date), else says at once that the
  server can't be reached. Only the request that found the server gone waits (up to the 10 s connect timeout, or the
  30 s stall limit for a server that accepts but doesn't answer).
- **A photo whose original is on this computer counts as there,** so it is edited from the original, not from its
  smart preview.
- **A photo deleted on the server** shows as missing. One that was **moved** shows as missing too: LightCraft doesn't
  follow it (see [Limits](#limits)).
- **A revoked app password** (or a deleted device in Nextcloud's security settings) turns the account Signed Out:
  requests stop, and messages say "Sign in to Nextcloud (…) again: Settings ▸ Nextcloud". **Sign In Again** runs the
  browser approval once more.
- **An account this computer doesn't have** (the library was copied to another machine, or the account was removed):
  its photos are offline, and opening one says to add the account. Nothing is lost; add the account and they come
  back.

## Certificates

Trust on first use, with no silent changes:
- A certificate the operating system trusts needs no pin; the OS checks it on every connection.
- A pinned certificate (one you trusted at sign-in) must match its SHA-256 fingerprint exactly; its name and expiry
  aren't checked.
- **For a pinned account, a different certificate is a hard error**: the account turns Signed Out. Signing in again
  shows the new fingerprint and says it changed since you trusted it (`nextcloud.signIn` answers `{state:
  "needsTrust", fingerprint, changed: true}`). Only trusting it again (`trust`) replaces the pin. A renewed self-signed
  certificate looks exactly like an attack, so check the fingerprint with the server's admin.

## Privacy

- **The app password lives only in the OS keyring:** Keychain on macOS, Credential Manager on Windows, Secret Service
  (GNOME Keyring, KWallet) elsewhere, under the service "LightCraft" and the account `user@host`. It is never written
  to the settings file, the library, logs, crash reports, error messages or command replies. Without a keyring (a
  headless Linux box with no Secret Service) adding an account fails and says so; there is no plaintext fallback.
- **Logs:** the HTTP library LightCraft uses (`ureq`, `ureq_proto`) would log raw requests at the TRACE level,
  password included. LightCraft never logs those two targets at TRACE, whatever `RUST_LOG` says (the CLI shows only
  warnings from other crates), so the password stays out of the log file and standard error.
- **Remove** forgets the account on this computer (password and settings); its photos stay in the library, offline.
  The app password stays valid on the server until you revoke it there (Personal settings ▸ Security ▸ Devices &
  sessions, listed as "LightCraft").
- The password is sent only to the account's own server; `http://` is accepted only for a server on this computer.
  Environment proxies are ignored.

## Limits

- **No write-back:** ratings, keywords and edits stay in the library; nothing is written to the server (no XMP sidecars
  there).
- **Moved files go missing:** a photo moved or renamed on the server is a different path; relinking by file id is not
  done yet. Import the new folder again (the old record stays missing).
- **No duplicate detection across folders:** Nextcloud photos have no content hash, so the same picture in two folders
  is imported twice. Within one path, re-imports are skipped.
- **Per-camera import presets don't apply** to Nextcloud raws: the camera is known only after the import added the
  photo, so they get the generic raw default.
- **Without the Memories app** (or for a photo it hasn't indexed), camera, lens, exposure, capture time and the size
  are read from the start of the file. The size is the one its EXIF records (raws such as CR3 and ARW have it, many
  JPEGs don't); without one it is unknown until the original is first downloaded, and the grid tile may have the
  wrong shape until then. The first download can correct it by a few pixels (an ARW's default crop).
- **Raw thumbnails depend on the server's preview providers** (Nextcloud's own, or apps such as camerarawpreviews,
  which see only the formats the server's file types map to raw). For a raw the server has no preview of, LightCraft
  shows the raw's embedded preview when it lies in the file's first MiB (it does in Canon CR3s); otherwise the photo
  has no thumbnail until it is downloaded.
- **An edited raw that was never downloaded has no thumbnail** (edited in the Library, e.g. with Paste Settings or a
  preset): an edit is rendered from the original, which the grid never downloads. Opening it in Develop once makes
  one.
- **The quick view is capped at 2,560 px** (the preview size), not the 4,096 px first planned: an unedited photo at
  Fit on a screen wider than that is a little soft outside Develop.
- **Offline, LightCraft can't tell whether a copy on disk is the latest version:** it is used as it is, and a newer
  version is downloaded once the server answers again (a copy older than a version a listing showed this session isn't
  used). An original downloaded by an older LightCraft (before `etags.json`) is found offline only after the server
  has answered once.
- **Labels in the Nextcloud dialogs and Settings tab are English only** so far.
- One account per import; a login name that differs from the user id (e-mail or LDAP logins) isn't supported by the
  client.
- Not in the web build.

## Commands

All are in `engine.commands`; agents call them through the control channel, `lightcraft-cli run` or MCP
(`cmd_nextcloud_addAccount`, …). Folders are full `nextcloud://user@host/path` strings; the account comes from the path.

| Command | Params | Reply |
|---|---|---|
| `nextcloud.addAccount` | `{server, trust?: "AB:CD:…", wait?: false}` | `{state: "needsTrust", fingerprint}` (run again with `trust`), `{state: "waiting", loginUrl, task}` (open `loginUrl`; `task` is the activity row), or with `wait: true` `{state: "signedIn", account}` |
| `nextcloud.signIn` | `{account: "user@host", trust?, wait?: false}` | As `addAccount`, for an account that was signed out; a certificate other than the pinned one answers `{state: "needsTrust", fingerprint, changed: true}` |
| `nextcloud.removeAccount` | `{account}` | `{removed}` |
| `nextcloud.preferences` | `{cacheBudgetMb?}` | `{accounts: [{account, server, user, root, state}], cacheBudgetMb, cachedBytes, pinnedBytes, signIns, imports}` |
| `nextcloud.browse` | `{dir}` | `{dir, folders: [{name, path}], photos}`: one level, hidden entries left out |
| `library.importNextcloud` | `{dirs: [...], wait?: false}` | `{started: true, task}`, or with `wait: true` `{added, skipped, metadata, failedFolders}` |
| `photo.makeAvailableOffline` | `{ids?, wait?: false}` | `{started: true, task, photos}`, or with `wait: true` `{photos, failed}`; disabled without a Nextcloud photo among the targets |
| `photo.removeOfflineCopy` | `{ids?}` | `{photos}` |
| `file.addNextcloudAccount`, `file.importNextcloud` | — | The two dialogs (desktop app) |

- `state` of an account: `signedIn`, `signedOut`, `unreachable` or `signingIn`.
- **`signIns`** (the last 8, oldest first): `{task, state: waiting | signedIn | failed | cancelled | timedOut,
  account?, error?}`. This is how a `wait: false` sign-in ends: poll until its `task` leaves `waiting`. `error` is
  user-facing (a keyring problem, "The sign-in wasn't approved in time; start it again", …).
- **`imports`** (the last 8): `{task, state: running | done | failed | cancelled, added, failedFolders, error?}`.
  `failedFolders` counts subfolders that couldn't be listed (import again to retry); `failed` carries the reason that
  stopped it (a folder asked for is missing, the account signed out or unreachable). Wait for `state` to leave
  `running` rather than for the activity row to go.
- **Activity rows** (`activity.list`): `nextcloud` (waiting for approval), `import` ("Importing from Nextcloud"),
  `scan` ("Reading metadata"), `download` ("Downloading …", "Prefetching …", "Making photos available offline").
  All can be cancelled.
- An import adds its photos as one undo step (split where you change something while it runs). Opening another
  library cancels running imports.

Errors are plain sentences with a next step, e.g. "No Nextcloud account bob@cloud.example on this computer; add it with
File ▸ Add Nextcloud Account…", "Nextcloud sign-in isn't available in this window", "That isn't a
Nextcloud address LightCraft can use (…); check what you typed".

Agents in an MCP session: [mcp.md](mcp.md#nextcloud); the desktop app's control channel:
[control-protocol.md](control-protocol.md#nextcloud). From a shell, sign in with `wait=true`: while it waits, the
CLI prints the page to open on standard error ("Approve LightCraft in your browser: https://…") and it returns
once you approved. (With `wait: false` the process would end before you approve.) The CLI, the MCP server and the
desktop app share accounts (same settings folder, same keyring):

```sh
lightcraft-cli run nextcloud.addAccount server=cloud.example.com wait=true
# a self-signed server answers needsTrust first: check the fingerprint, then add trust=AB:CD:…
lightcraft-cli run nextcloud.preferences
lightcraft-cli run nextcloud.browse dir=nextcloud://bob@cloud.example.com/Photos
lightcraft-cli run --library ~/lc-lib library.importNextcloud 'dirs=["nextcloud://bob@cloud.example.com/Photos/2026"]' wait=true
```

## Measurements

Against a Nextcloud 35 server on the LAN with Memories (2026-10-13; 52,414 photos; Sony A7R VI ARW and Canon R8
CR3), from the engine with a release build. The sign-in and the OS keyring were not part of this run.

| What | Result |
|---|---|
| Memories timeline (52,414 photos) | 1.9 s |
| Listing `Sony A7R VI/` (37,314 photos, with date, size and dimensions) | 3.2 s |
| Importing `Canon R8/` (5,973 photos), listing then camera, lens and exposure for all | 2.2 s, then 32.9 s in all; no original downloaded |
| Server preview, 512 px (50 photos) | 50 ms median, 76 ms p90 |
| Opening in Develop: CR3 (16 MB) / ARW (58 MB), download plus decode | 1.4 s / 2.1 s (the download runs at ~35 MB/s) |
| The next photo after its prefetch: CR3 / ARW | 0.50 s / 0.41 s (decode only) |
| Camera, lens, exposure, time and size from the file's first 64 KiB | ~30 ms per file |

What the run changed: Memories' timeline time is a UTC instant, so capture times now come from the photo's own local
time; this server makes no previews of CR3 files, so LightCraft now reads the preview embedded in the file's first
MiB; CR3s that Memories hasn't indexed get their size from the file's header.

## Design decisions

1. **One routing check.** A catalog path starting `nextcloud://` is remote (`lightcraft_catalog::is_remote`, the same
   test as craft's `RemotePath::is_remote`). It has no platform conditions, so the catalog, engine and UI share it, and
   the web build never links the Nextcloud client.
2. **Hosts opt in.** `Session::with_default_nextcloud()` points a session at the settings file, the cache folder and
   the keyring, like `with_default_face_models`. A bare `Session` has an unconfigured Nextcloud part, so tests stay
   hermetic and never touch the user's keyring.
3. **The keyring is a feature.** The engine's `keyring` feature turns on craft's OS keyring; the desktop app and the
   CLI enable it. `with_default_nextcloud()` installs the keyring store only then; tests use an in-memory store.
4. **Work off the session comes back as events.** Import listings and metadata and finished downloads are queued as
   `remote::Event`s and applied by `Session::drain_remote()`: at the start of every command and once per UI frame.
   Commands with `wait: true` apply them as they arrive until their worker ends. The catalog is only ever changed on
   the session's thread.
5. **The import is the local import.** `library.importNextcloud` feeds listed photos through the same `ImportJob`
   commit (import look, defaults, duplicate check by path) via `ImportJob::prepare_infos`, so remote and local photos
   are the same kind of record.
6. **"Develop" is an edit tool or a percentage zoom.** That is when an original is needed. An unedited photo at
   Fit/Fill stays on the server's preview; an edited one renders from its original (cached or downloaded), with the
   smart preview as the fallback when the server is unreachable.
7. **Folders are full paths.** `library.importNextcloud {dirs}` and `nextcloud.browse {dir}` take `nextcloud://` paths
   and the account comes from the path, so there is no separate `account` param to get out of step.
8. **The cache is keyed by etag.** LightCraft remembers etags from listings and downloads, and each copy's in
   `etags.json` (so copies are found offline); a lookup with no known etag costs one `stat` (skipped while the
   account is unreachable).
9. **One download at a time,** behind one lock, interactive loads first. Per-path downloads can come if measurements
   show a waiting loupe.
10. **Local-only actions refuse.** Merge, Convert to DNG, Duplicate, Edit In, file renames and moves, folder
    operations, Synchronize Folder, sidecar writes and Show in Finder would write beside the original or act on a
    local file, so they refuse Nextcloud photos. Export and denoise download the original instead, on their own
    threads; "all metadata" reads the file's first MiB when the original isn't on this computer (the app's window
    never downloads).
11. **End to end runs in process.** The MCP test drives the in-process server with an in-memory password store
    (`crates/mcp/tests/e2e.rs`, `nextcloud_add_import_render_export`): the CLI binary can't script a sign-in without
    an OS keyring.
12. **Two folders.** Accounts live in `<settings folder>/nextcloud.json` with the other settings; originals in the
    platform cache folder (`LIGHTCRAFT_CACHE_DIR`), which the OS and backup tools treat as disposable.
