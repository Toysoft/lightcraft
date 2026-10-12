# Nextcloud as a photo source in LightCraft: integration design

- **Date:** 2026-10-12
- **Status:** design approved section by section in chat; this document awaits review. It is dropped before the
  upstream PR.
- **Branches:**
  - **LightCraft:** `feat/nextcloud-source` (from `upstream/main` 701f78ea).
  - **craft-libs:** `feat/nextcloud`, in `~/GitHub/craft-libs` (from `main` f58bc1a).
- **The shared client** is designed in craft-libs `docs/nextcloud.md`. It covers `craft-nextcloud`: sign-in and the
  certificate pin, keyring, the `RemotePath` format, WebDAV/preview/Range/download, Memories, the originals cache, the
  `FakeServer` for tests, and the measurements. This document covers only what LightCraft adds on top. It does not
  repeat the shared design.

## 1. Goal

Browse a whole Nextcloud photo archive in LightCraft and develop selected shots, without mounting the server and
without downloading every original.

The motivating archive (`cloud.dev-null.me`, on the LAN) has `Sony A7R VI/` (37,863 files, 1.38 TiB) and `Canon R8/`
(10,571 files, 72 GiB). Through an rclone mount, a normal import read and hashed every byte (the `files.rs` probe does
`std::fs::read` + `hash_bytes`): ~1.45 TiB at ~39 MB/s, about 11 hours.

**Success means:**
- The first import lists every photo with date, size and dimensions within about a minute. Camera, lens and exposure
  fill in within ~10 minutes.
- Library browsing (grid and loupe) downloads no originals.
- Develop downloads only the open photo (~1 s on the LAN) and prefetches the next one.
- Everything keeps working, degraded, when the server is unreachable.

## 2. Decisions (confirmed in chat)

1. **Native source** (approach B), not "cheap reads on a mount" or an rclone hybrid.
2. **Use cases:** browse the whole archive and develop selected shots are must-haves. Writing back to Nextcloud is a
   nice-to-have, not in v1.
3. **Generic Nextcloud core**, with the Memories app as an optional accelerator.
4. **Credentials:** app password from Login Flow v2, stored in the OS keyring, never in plaintext.
5. **Shared client:** the client, the remote-path format, the sign-in and certificate-pin sequence, and the originals
   cache are **shared in craft-libs** as `craft-nextcloud` on `ureq` 3 + rustls. LightCraft's `crates/fetch` is not
   changed.
6. **Metadata:** from Memories when available, otherwise a header Range read parsed by LightCraft's own probe.
7. **v1 includes** prefetching the next original in Develop, and Make Available Offline.
8. **TLS:** self-signed servers via trust on first use.

## 3. Architecture

### 3.1 What lives where

| Part | Where |
|---|---|
| Client, `RemotePath`, `SignIn` state machine, certificate pin, keyring, originals cache, Memories, `FakeServer` | craft-libs `craft-nextcloud` |
| `nextcloud://` as a volume in the catalog's folder parsing | LightCraft `crates/catalog` |
| Hook routing, remote preview/probe/loader, download policy, prefetch, smart previews, offline-pin commands | LightCraft `crates/engine` |
| Sign-in dialog, folder picker, menus, Settings (cache budget, pinned size) | LightCraft `crates/ui-egui` |
| Keyring-backed credential store and the RustCrypto provider | `apps/lightcraft`, `apps/lightcraft-cli` |

### 3.2 Depending on `craft-nextcloud`

- Root `Cargo.toml`: `craft-nextcloud = { git = "https://github.com/storytold/craft-libs", rev = "<rev>" }`, next to
  `craft-ui`.
- During development, a `[patch."https://github.com/storytold/craft-libs"]` points both crates at
  `../craft-libs` (the local clone). The patch never lands in an upstream PR.
- Features:
  - the engine uses `craft-nextcloud` with no optional features;
  - the app and CLI enable `keyring`;
  - dev-dependencies enable `testing`.
- To the layer check (`xtask/src/layers.rs`) it is an ordinary external crate. Only `UI_CRATES` such as `craft-ui`
  are restricted, so the engine (L4) may depend on it.
- The rustls `CryptoProvider` is `rustls-rustcrypto`, already used by `crates/fetch`, which keeps the product pure
  Rust.

### 3.3 Catalog paths

A remote photo's `Source::File { path }` is the `RemotePath` string, e.g.
`nextcloud://root@cloud.dev-null.me/Sony A7R VI/2026/06/28/_DSC1748.ARW`.

`catalog::query::split_path` learns the `nextcloud://<authority>` prefix next to the UNC and drive-letter forms. The
path then counts as absolute, so `folder_key`, the folder tree, `volume_of` and `is_disk_root` work unchanged, and
the account shows as one volume in Folders.

### 3.4 Engine routing

`Session::with_fs()` installs routing wrappers in place of the plain fs hooks:
- `FileLoader`, `FileProbe`, `PreviewLoader`, `PairLoader`, `ByteReader` (`file_bytes` becomes `Some`), and the
  `Availability` probe.
- `RemotePath::is_remote(path)` sends a path to `engine::remote`; everything else goes to the unchanged fs functions.
- Precedent: the web build's synthetic paths and byte hooks (`apps/lightcraft-web/src/store.rs`).

`engine::remote` holds the account registry (accounts from settings, passwords via the credential hook), one
`craft_nextcloud::Client` per account, and one `OriginalsCache`.

The direct `std::fs` uses that bypass the hooks check `is_remote` first, then skip the step or report "not available
for Nextcloud photos":
- sidecar find/write;
- import revalidation and copy/move;
- relink and Find Missing;
- browse/expand;
- the export-original fallback;
- Show in file manager.

Each gets a test.

### 3.5 Commands (everything is a command)

| Command | What it does |
|---|---|
| `nextcloud.addAccount {server}` | Drives `SignIn`, including the certificate prompt. CLI/MCP print the login URL. |
| `nextcloud.signIn {account}` | Re-run sign-in after `Unauthorized`. |
| `nextcloud.removeAccount {account}` | Delete the keyring entry and settings. Photos stay in the catalog, offline. |
| `nextcloud.browse {account, dir}` | One level, for the folder picker. |
| `library.importNextcloud {account, dirs[]}` | §4.2. Re-running it is a rescan. |
| `photo.makeAvailableOffline` / `photo.removeOfflineCopy` | `OriginalsCache::pin` / `unpin` on the selection. |

**Setting:** `nextcloud.cacheBudget` (default 20 GB). Menus: File ▸ Add Nextcloud Account…,
File ▸ Import from Nextcloud…, Photo ▸ Make Available Offline.

### 3.6 Credentials

The engine gets a `CredentialStore` hook (`get/set/delete(account) -> Result<…, String>`), and tests install an
in-memory one. The app and CLI back it with `craft-nextcloud`'s `keyring` feature. With no keyring,
`nextcloud.addAccount` fails with a message saying so. The password never appears in logs, the catalog, settings,
crash logs or MCP/control replies.

## 4. Data flow (LightCraft policy)

### 4.1 Adding an account

`nextcloud.addAccount` runs `SignIn`:
- `NeedsTrust`: the dialog shows the fingerprint.
- `Waiting`: opens `login_url`, then polls every 2 s while holding a `TaskGuard` ("Waiting for Nextcloud approval…",
  `Cancel::Flag`).
- `Done`: the password goes to the keyring, the `Account` to settings.

### 4.2 Import (`library.importNextcloud`)

1. **List:** a parallel `Client::list` walk of the chosen folders (8 workers), keeping the extensions a local import
   accepts.
2. **Memories join** (if available): `days()` plus per-day detail (1,472 days ≈ 10 s), joined by fileid, gives date,
   dimensions and size.
3. **Commit** in batches (`mode: add`, no copy):
   `Photo { source: File { path: nextcloud://… }, file_size, width, height, captured, kind, format, content_hash: None }`.
4. **Background "Reading metadata" row** fills camera, lens and exposure:
   - with Memories: `info(fileid)`, 8 at a time, using the fileids this job listed;
   - otherwise, or for unindexed files: `read_range` of the header (a 256 KiB window to start; the size is measured on
     ARW/CR3) into LightCraft's existing probe, which asks for further windows if needed;
   - a run interrupted by a quit resumes on the next rescan.
5. **Dedup and rescan:** by path only, since there's no content hash. Thumbnail keys use the existing
   `file:{path}:{size}` fallback (`content_key`, media.rs:1062).

### 4.3 Viewing (Library): no original downloads

- **Grid:** thumbnails for unedited photos are `Client::preview(path, 256|512)`, fetched on the background workers
  and kept in the existing thumbnail disk cache.
- **Edited photos** render from their smart preview.
- **Loupe:** the largest server preview (up to 4096 px), or the smart preview if edited.
- **Orientation:** server previews are already upright and are marked so, so they aren't rotated twice.
- **Rule:** under `memory::is_background()` the remote hooks never call `OriginalsCache::fetch`.

### 4.4 Originals: Develop, 1:1, Export, Merge/HDR/Denoise

- **Interactive loads** call `OriginalsCache::fetch` under a `TaskGuard` ("Downloading _DSC1748.ARW (26 MB)",
  cancellable, with progress), then decode the local file with the existing fs loader. The loupe keeps the server
  preview up meanwhile.
- **The photo open in Develop** is `hold`-protected from eviction.
- **After the first download:**
  - the full local probe completes the record (as-shot WB, embedded lens data);
  - a smart preview is built in the background.
- **Prefetch:** in Develop, the next photo in filmstrip order is fetched in the background. One at a time, its own
  activity row, cancelled when leaving Develop or moving on.
- **Make Available Offline:** pins the selection, with an "N of M" activity row. Settings shows the pinned size
  separately.

### 4.5 Offline

- **Availability** for remote paths comes from the account's reachability, checked on a background worker, plus the
  last listing.
- **Server unreachable:** the existing "Original offline · editing the smart preview" UI. `fetch` falls back to a
  stale cached copy, and the grid keeps its cached thumbnails.
- **Deleted on the server:** the photo shows as missing.
- **Moved on the server:** the photo goes missing. Relinking by fileid is a follow-up.

## 5. Errors (never crash)

- `craft_nextcloud::Error` values become user messages with a next step:
  - `Unauthorized` → offer `nextcloud.signIn`;
  - `Unreachable` → offline state;
  - `NotFound` → missing.
- Hooks already run on render and import workers, so nothing blocks a frame.
- Every crash fix lands with a synthetic regression test.

## 6. Testing (LightCraft side; the client's own tests are in craft-libs)

All of the following run against `craft_nextcloud::testing::FakeServer`:
- **Catalog:** `nextcloud://` in `split_path`, `folder_key`, the folder tree, `volume_of` and `is_disk_root`
  (Unix and Windows).
- **Engine:**
  - routing leaves fs paths untouched;
  - background loads never download;
  - an interactive miss downloads once;
  - the open photo is held;
  - prefetch;
  - offline pins;
  - offline state;
  - every job's activity row is asserted;
  - the bypassed `std::fs` paths behave as in §3.4.
- **End-to-end:** CLI and MCP: add account (scripted sign-in) → import → render → export.
- **Manual, against `cloud.dev-null.me`:**
  - first-import time, grid scrolling, Develop open and prefetch;
  - control-channel screenshots;
  - timings in `plan/STATUS.md` → Metrics;
  - the real keyring.
- **Gate:** `cargo xtask ci` green before every commit.

## 7. Not in v1

- Writing ratings and edits back as XMP (needs PUT in `craft-nextcloud`).
- Relinking moved files by fileid.
- `notify_push` change notifications.
- WebDAV servers without the preview endpoint.
- The web build.

## 8. Docs and repository rules

- `docs/nextcloud.md` (user docs: setup, what downloads when, caching, offline, certificate pinning, privacy).
- Parity rows for the source as a beyond-Lightroom feature (`LR-CLOUD-SYNC` stays out of scope), a `docs/gaps.md`
  entry, and ROADMAP / target-app-parity numbers if a listed gap closes.
- `docs/control-protocol.md` and `docs/mcp.md` for the new commands. `cargo xtask parity` passes.
- One task id per commit. The craft-libs rev bump is its own commit.

## 9. Risks

- **Header window size** for A7R VI ARW and R8 CR3: measure it; the fallback is a second Range read.
- **Blocking downloads:** an interactive original download blocks one render worker for ~1 s on the LAN, longer on
  WAN. Watch the loupe and Develop budgets.
- **No content hash:** cross-folder duplicates are not detected.
- **Two repositories:** the upstream LightCraft PR waits for the craft-libs PR and a rev bump. The personal build uses
  the `[patch]` meanwhile.
- **Client-side risks** (certificate pinning on ureq, Memories API stability) are tracked in craft-libs
  `docs/nextcloud.md`.
