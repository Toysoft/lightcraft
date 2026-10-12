# Nextcloud as a photo source: design

- **Date:** 2026-10-12
- **Status:** design approved section by section in chat; this document awaits review
- **Branches:**
  - **LightCraft:** `feat/nextcloud-source` (from `upstream/main` 701f78ea). This spec is dropped before the upstream PR.
  - **craft-libs:** `feat/nextcloud` for the shared `craft-nextcloud` crate (§4.3). It lands first, because LightCraft
    pins craft-libs by git rev. Until then LightCraft builds against a local clone through Cargo `[patch]`.
- **Audience:** upstream-able (generic Nextcloud, shared by every Craft app), tuned first for the maintainer's own server

## 1. Goal

Browse a whole Nextcloud photo archive in LightCraft and develop selected shots, without mounting the server
and without downloading every original.

The motivating archive is on `cloud.dev-null.me`, a Nextcloud 35 server on the LAN:

| Folder | Files | Size |
|---|---|---|
| `Sony A7R VI/` | 37,863 | 1.38 TiB |
| `Canon R8/` | 10,571 | 72 GiB |

Measured 2026-10-12, the rclone mount plus a normal import read and hashed every byte (`files.rs` probe does
`std::fs::read` + `hash_bytes`). That is ~1.45 TiB at ~39 MB/s, about 11 hours. Thumbnails then re-read whole files
whenever they fall out of rclone's 20 GB cache.

**Success means:**
- The first import of both folders lists every photo with its date, size and dimensions within about a minute.
  Camera, lens and exposure fill in within ~10 minutes.
- Library browsing (grid and loupe) downloads no originals.
- Opening a photo in Develop downloads only that original (~1 s on the LAN), and the next one is prefetched.
- Everything keeps working, degraded, when the server is unreachable.

## 2. Decisions (confirmed in chat)

1. **Approach B:** a native Nextcloud source, not "cheap reads on a mount" (A) or an rclone hybrid (C).
2. **Use cases:**
   - Must have: browse the whole archive, develop selected shots.
   - Nice to have, not in v1: write edits back to Nextcloud.
3. **Generic:** the core uses standard WebDAV plus Nextcloud's core preview endpoint. The Memories app is an optional
   accelerator, detected at run time.
4. **Credentials:** an app password from Login Flow v2, stored in the **OS keyring**. No plaintext fallback.
5. **HTTP stack:**
   - The Nextcloud client lives in **craft-libs as `craft-nextcloud`**. It is built on **`ureq` 3 + rustls**,
     the stack `craft-launcher`, `filmcraft` and `pdfcraft` already use.
   - It uses OS certificate verification, not bundled webpki-roots (the other apps avoid those as CDLA-licensed).
   - Connection reuse comes from ureq's `Agent`.
   - LightCraft's `crates/fetch` is **not changed**. It was measured fast enough, but it has no connection reuse and
     is LightCraft-only.
6. **Metadata:** Memories bulk timeline plus per-file info when available; a header Range read otherwise.
7. **v1 includes:** prefetching the next original in Develop, and Make Available Offline (pinning).
8. **TLS:** self-signed or private-CA servers via **trust on first use** (pinned leaf fingerprint).
9. **Where the work lives:**
   - LightCraft: worktree `.claude/worktrees/nextcloud-source`, branch `feat/nextcloud-source`.
   - The shared client: craft-libs, branch `feat/nextcloud`.

## 3. Evidence (measured 2026-10-12, against `cloud.dev-null.me`)

- **Network:** the server resolves to 192.168.1.1. TCP connect ~1 ms, TLS ~10 ms, preview PHP ~25–30 ms.
- **`fetch` vs curl** (throwaway example, since deleted):

  | Request | `fetch` | curl |
  |---|---|---|
  | 256 px preview, new connection each | 35 ms | 48 ms (includes process spawn) |
  | Full 26 MB ARW | 23–35 MB/s | 22–29 MB/s |

  The server is the bottleneck, not `fetch`.
- **`ureq` 3.4.2** (`rustls-no-provider` + `platform-verifier`, rustls-rustcrypto provider; throwaway project, since
  deleted):

  | Request | `ureq` |
  |---|---|
  | 256 px preview, new connection each | 41 ms |
  | **256 px preview, reused connection** | **24 ms** |
  | PROPFIND of a day folder (115 entries, 46 KB) | 32 ms |
  | Full 26 MB ARW | 29 MB/s |

  The dependency tree has no `openssl-sys`, `ring`, `aws-lc` or `cc`. The only match is `openssl-probe`, a pure-Rust
  crate that locates the system's certificate files.
- **Core previews:** `GET /index.php/core/preview?fileId=694550&x=&y=&a=1` for an ARW returned 256×171 (1.4 KB),
  1024×683 (8 KB) and, when asked for 2048, the pre-rendered 4096×2731 (96 KB). Each took 50–180 ms. The server runs
  camerarawpreviews (embedded-JPEG extraction) and previewgenerator.
- **PROPFIND:** returns `oc:fileid`, size, etag, mtime, content type and `nc:has-preview`. Nextcloud's own
  `nc:metadata-photos-*` properties are **empty for ARW**.
- **Memories `GET /apps/memories/api/days`** (header `OCS-APIRequest: true`; without it the server answers 412
  "CSRF check failed"):
  - one call, 0.74 s: 1,472 days and 52,414 photos;
  - per photo: `fileid`, `dayid`, `w`, `h`, `etag`, `basename`, `size`, `epoch`, `mimetype`;
  - `/days/<dayid>` is ~45 ms.
- **Memories `GET /apps/memories/api/image/info/<fileid>`:** ~60 ms. Returns `w`, `h`, `datetaken` and an `exif`
  object with exiftool tag names (`Make`, `Model`, `ISO`, `FNumber`, `DateTimeOriginal`, `OffsetTimeOriginal`, …).

## 4. Architecture

### 4.1 Remote paths

A Nextcloud photo's `Source::File { path }` is `nextcloud://<user>@<host>[:port]/<path below the user's files root>`.
For example: `nextcloud://root@cloud.dev-null.me/Sony A7R VI/2026/06/28/_DSC1748.ARW`.

- `catalog::query::split_path` learns one more prefix form, `nextcloud://<authority>`, next to the UNC `//server/share`
  and drive-letter forms. The path is then absolute, so the folder tree, folder keys and `volume_of` /
  `is_disk_root` work unchanged. The account appears as one volume in the Folders panel.
- Names are stored decoded, as the user sees them. They are percent-encoded only when building URLs.
- UNC-style `//host/…` paths are rejected as a format: on Windows, `std::fs` would try SMB against the server.
- `std::fs` on a `nextcloud://` path is harmless (relative or invalid), but every hook routes these paths before any
  file system call (§4.4).

### 4.2 Where the code lives

| Part | Repository | Why |
|---|---|---|
| Nextcloud client: login flow, WebDAV, previews, Range/download, Memories, TOFU pin, keyring store | craft-libs → `craft-nextcloud` | Useful to every Craft app (photos, PDFs, PSDs, video); depends on no app |
| `nextcloud://` paths, engine routing, originals cache, prefetch, offline pins, commands, UI | LightCraft | Tied to LightCraft's catalog, media hooks and command registry |

`crates/fetch` is not changed. To LightCraft's layer check (`xtask/src/layers.rs`), `craft-nextcloud` is an ordinary
external crate. Only the UI crates in `UI_CRATES` (which includes `craft-ui`) are restricted. So the engine (L4) may
depend on it directly.

### 4.3 `craft-nextcloud` (craft-libs `crates/nextcloud`; no UI; native only)

Follows craft-libs `AGENTS.md`:
- the never-crash `#![deny(...)]` attributes in `lib.rs`;
- no `unsafe`;
- workspace package fields and lints;
- listed in `[workspace.dependencies]`;
- a README entry;
- gates: `cargo fmt --check`, `clippy --workspace --all-targets -D warnings`, `test --workspace`.

**HTTP:** `ureq` 3 with `default-features = false` and `rustls-no-provider` + `platform-verifier`. The app supplies the
rustls `CryptoProvider` (LightCraft: `rustls-rustcrypto`, pure Rust; others may pass `ring`). One `ureq::Agent` per
account gives connection reuse, with `allow_non_standard_methods` for PROPFIND. Blocking API, like the rest of the
suite.

**API:**
- **`Client::new(account, password, provider)`.** `Account` is `{ server, user, trust: Option<Fingerprint> }`. The
  password lives only inside the `Client`; it is never logged or `Debug`-printed.
- **`login_flow::start(server)`** → `{ login_url, poll_endpoint, token }`.
  **`login_flow::poll(...)`** → `Pending` or `Done { server, login_name, app_password }`.
- **`Client::list(dir)`** → `Vec<Entry { name, is_dir, size, etag, fileid, mtime, content_type, has_preview }>`.
  One PROPFIND `Depth: 1`; Nextcloud forbids `infinity` by default.
- **`Client::preview(path, max_edge)`** → JPEG bytes. Uses `/index.php/core/preview.png?file=<path>&x=&y=&a=1`;
  `404` means "no preview". Addressing by path means no fileid has to be stored. Verified 2026-10-12: same bytes as
  the fileid form, 36–39 ms.
- **`Client::read_range(path, range)`** → bytes. Requires a `206` with a matching `Content-Range`. On a `200`, it reads
  only the needed prefix and drops the response; ureq doesn't reuse a half-read connection.
- **`Client::download(path, dest_tmp, cancel, progress)`** → size. Streams to a file and checks `Content-Length`.
- **`memories::{detect, days, info}`**, mapped to a neutral `RemoteMeta`. Sends `OCS-APIRequest: true`.
- **`keyring` feature:** `store_password` / `load_password` / `delete_password` over `keyring` 4.2 with
  `default-features = false` and native stores only:
  - Apple Keychain;
  - Windows Credential Manager;
  - `zbus-secret-service-keyring-store` with `crypto-rust`, so no OpenSSL.

  The feature is optional, so headless tools can skip it.

**Trust on first use:**
- On an unknown issuer, the error carries the leaf certificate's SHA-256 fingerprint so the app can ask for
  confirmation.
- Once confirmed, the client pins that certificate. ureq 3 exposes no custom rustls verifier, so the first attempt is
  `RootCerts::Specific([pinned leaf])`.
- If rustls-webpki won't accept a self-signed leaf as its own anchor, the fallback is a custom connector through
  ureq's `unversioned::transport::Connector` (pinned to the ureq minor version).
- This gets prototyped before the rest of the crate (§10).

**Limits:**
- XML/JSON response caps: e.g. 64 MiB per PROPFIND, 256 MiB for the Memories timeline.
- Element depth cap, entry-count cap.
- Names rejected if, after decoding, they contain `/`, `\`, NUL or control characters, or are `.`/`..`.

### 4.4 Engine routing (`crates/engine`)

`Session::with_fs()` installs routing wrappers in place of the plain fs hooks:

- `FileLoader`, `FileProbe`, `PreviewLoader`, `PairLoader`, `ByteReader` (`file_bytes` becomes `Some`), and the
  `Availability` probe (`set_probe`).
- A path starting with `nextcloud://` goes to `engine::remote`; every other path goes to the existing fs function,
  unchanged.
- Precedent: the web build's synthetic `web/<hash>/<name>` paths and its byte hooks (`apps/lightcraft-web/src/store.rs`).

**`engine::remote` holds:**
- the account registry: accounts from settings, passwords via the credential hook (§4.6);
- one `craft_nextcloud::Client` per account (its `ureq::Agent` reuses connections);
- the originals cache (§5.4).

Previews, Range reads and downloads all address files by path, so nothing remote-specific needs persisting beyond the
catalog path. fileids are only used inside the import job, to join with Memories.

The direct `std::fs` uses that bypass hooks (sidecar find/write, import revalidation and copy/move, relink, Find
Missing, browse/expand, export-original fallback, Show in file manager) check `remote::is_remote(path)` first. They
then skip the step or report "not available for Nextcloud photos". They never error-spam and never panic. Each gets
a test.

### 4.5 Commands (everything is a command)

| Command | What it does |
|---|---|
| `nextcloud.addAccount {server}` | Login Flow v2 plus the TOFU prompt; stores the password and account. CLI/MCP print the login URL. |
| `nextcloud.signIn {account}` | Re-run the login flow after a 401. |
| `nextcloud.removeAccount {account}` | Delete the keyring entry and settings. Photos stay in the catalog, offline. |
| `nextcloud.browse {account, dir}` | List one level, for the folder picker. |
| `library.importNextcloud {account, dirs[]}` | The import job (§5.2). Re-running it is a rescan. |
| `photo.makeAvailableOffline` / `photo.removeOfflineCopy` | On the selection. |

**Setting:** `nextcloud.cacheBudget` (default 20 GB). Menus: File ▸ Add Nextcloud Account…,
File ▸ Import from Nextcloud…, and Photo ▸ Make Available Offline.

### 4.6 Credentials

- **Engine:** gets a `CredentialStore` hook (`get/set/delete(service, account) -> Result<…, String>`). Tests install
  an in-memory store.
- **App (`apps/lightcraft`, L6) and CLI:** install a keyring-backed store using `craft-nextcloud`'s `keyring`
  feature (§4.3).
  - Check: `cargo tree -i openssl-sys` is empty.
  - `zbus` 5.19 is already in LightCraft's tree.
- **No keyring available:** `nextcloud.addAccount` fails with "No system keyring (Secret Service) is available…".
  No plaintext fallback.
- The password never appears in logs, the catalog, settings, crash logs or MCP/control replies.

## 5. Data flow

### 5.1 Adding an account

1. `addAccount` calls `POST <server>/index.php/login/v2` and opens `login_url` in the browser.
2. It polls every 2 s, holding a `TaskGuard` ("Waiting for Nextcloud approval…", `Cancel::Flag`), and gives up after
   20 minutes.
3. On success, the app password goes into the keyring and `{server, user, trust}` into settings.
4. If TLS fails with an unknown issuer, the UI shows the presented SHA-256 fingerprint. On confirmation it retries
   with the pin.
5. A later fingerprint mismatch is a hard error: "Nextcloud's certificate changed".

### 5.2 Import (`library.importNextcloud`)

1. **List:** parallel PROPFIND walk of the chosen folders (8 workers, bounded depth and entry counts). Keeps the
   extensions a local import accepts.
2. **Memories join** (if detected): the `days` list, plus one `/days/<dayid>` detail call per day (1,472 for this
   archive, ~45 ms each, ≈10 s with 8 in parallel). Joined by fileid, it gives date, dimensions and size.
3. **Commit:** records are written in batches (`mode: add`, no copy) with
   `Photo { source: File { path: nextcloud://… }, file_size, width, height, captured, kind, format, content_hash: None }`.
   They appear in the grid within about a minute.
4. **Background metadata:** a "Reading metadata" activity row fills camera, lens and exposure.
   - With Memories: per-file `image/info`, 8 at a time, using the fileids this job listed. A run interrupted by a
     quit resumes on the next rescan, which lists again.
   - Without Memories, or for files it hasn't indexed: a header Range read (a 256 KiB window to start; the right size
     is measured on ARW/CR3 during implementation) fed to the existing probe. If the parser needs bytes past the
     window, the probe fetches only those bytes.
5. **Dedup and rescan:** duplicates are detected by path only (no content hash). Files already in the catalog by path
   are skipped, so re-running the import adds new files.
6. **Cache keys:** thumbnail keys use the existing `file:{path}:{size}` fallback (`content_key`, media.rs:1062).

### 5.3 Viewing (Library): no original downloads

- **Grid:** quick stand-ins and final thumbnails for unedited photos come from `preview(path, 256|512)`, run on the
  background workers and stored in the existing thumbnail disk cache.
- **Edited photos:** render from their smart preview.
- **Library loupe:** shows the largest server preview (up to 4096 px). Edited photos use the smart preview.
- **Orientation:** server previews are already upright. The loader reports them as orientation-normalized so they are
  not rotated twice.
- **Rule:** a hook running under `memory::is_background()` never starts an original download. It returns the preview,
  the smart preview or an error.

### 5.4 Originals: Develop, 1:1, Export, Merge/HDR/Denoise

- **Cache:** `<cache>/nextcloud/<user@host>/<sha256(path)>-<etag>.<ext>`. Names are built from hex only; the
  extension is allow-listed.
- **Hit:** a `PROPFIND Depth: 0` (~30 ms) confirms the etag is still current, then the file is decoded through the
  existing fs loader. When the server is unreachable, the cached copy is used as is.
- **Miss** on an interactive load: download to `*.part` with a `TaskGuard` ("Downloading _DSC1748.ARW (26 MB)",
  cancellable, progress), check the size, rename, then decode. The loupe keeps showing the server preview meanwhile.
- **After the first download:**
  - the full local probe updates the record (as-shot WB, embedded lens data, exact dimensions);
  - a smart preview is built in the background.
- **Eviction:**
  - LRU by last use above `nextcloud.cacheBudget`;
  - files open in Develop and pinned files are never evicted;
  - a changed etag makes the old file stale, so it is deleted.
- **Prefetch:** in Develop, the next photo in filmstrip order downloads in the background. One at a time, its own
  activity row, cancelled when leaving Develop or moving on. It doesn't decode.
- **Make Available Offline:** downloads the selection with an "N of M" activity row and pins it. Pinned bytes are
  shown separately in Settings.

### 5.5 Offline and server changes

- **Availability** for a remote path is answered from the account's reachability, cached and checked on a background
  worker, plus the last listing.
- **Server unreachable:** the existing "Original offline · editing the smart preview" UI. The grid keeps its cached
  thumbnails.
- **File deleted on the server:** it shows as missing.
- **File moved or renamed on the server:** the record goes missing. Relinking by stable fileid is a follow-up (§8).

## 6. Error handling (never crash)

- **Network errors** → `Err(String)` with a next step:
  - unreachable: `"Can't reach cloud.dev-null.me: …"`;
  - 401 → offer `nextcloud.signIn`;
  - 404 → missing;
  - 429/503 → back off and retry, max 3;
  - stall → ureq connect and receive timeouts, set per request kind (short for previews, generous for downloads).
- **Response checks:** responses are size-capped, XML/JSON depth-capped, and every number from the server is
  checked/saturating.
- **Downloads:** a full disk, cancel or truncation deletes the `.part` file and reports.
- **Hooks never block the UI thread:** they already run on render or import workers.
- **Crash fixes:** each lands with a synthetic regression test.

## 7. Testing

- **Fake Nextcloud:** an in-process server for tests (`std::net::TcpListener`, plain HTTP, scripted per test). It
  serves login flow, PROPFIND, previews, Range, downloads and Memories. Failure injection: 401, 429, truncation,
  stall, dropped connection, server ignoring Range, oversized or malformed XML/JSON, hostile names (`..`, NUL,
  `%2F`).
- **`craft-nextcloud`** (tests in craft-libs):
  - parsers on synthetic fixtures;
  - login-flow state machine;
  - Range semantics, including a server that ignores Range;
  - name validation;
  - connection reuse through the `Agent` (request count per socket);
  - TOFU accept, mismatch and no-pin;
  - keyring feature compiled, with the real keyring tested by hand.

  The fake server is exported behind a `testing` feature, so LightCraft's engine and end-to-end tests drive the same
  fake.
- **`catalog`:** `nextcloud://` in `split_path`, `folder_key`, the folder tree, `volume_of` and `is_disk_root`
  (Unix and Windows).
- **Engine:**
  - routing (fs paths untouched);
  - background loads never download;
  - interactive miss downloads once;
  - eviction honours pins and the open photo;
  - etag change;
  - prefetch;
  - offline;
  - every job's activity row is asserted;
  - the bypassed `std::fs` paths behave as in §4.4.
- **End-to-end:** CLI and MCP against the fake server: add account (scripted login flow) → import → render → export.
- **Manual, against `cloud.dev-null.me`:**
  - first-import time, grid scrolling, Develop open and prefetch;
  - headless/control-channel screenshots;
  - timings recorded in `plan/STATUS.md` → Metrics;
  - the real keyring.
- **Gates:** `cargo xtask ci` green before every LightCraft commit; craft-libs' `fmt --check` / `clippy -D warnings` /
  `test --workspace` before every craft-libs commit.

## 8. Not in v1 (follow-ups)

- Writing ratings and edits back as XMP sidecars (PUT next to the raw).
- Relinking moved files by fileid; detecting changed files during a rescan beyond etag.
- Server-change notifications (`notify_push`) instead of a manual rescan.
- Other WebDAV servers without Nextcloud's preview endpoint (they would use header and embedded-JPEG reads).
- The web build: `craft-nextcloud` is native-only, and the browser would also need CORS on the server.

## 9. Docs and repository rules

- New `docs/nextcloud.md` (setup, what downloads when, caching, offline, TOFU, privacy).
- craft-libs: `crates/nextcloud/README.md` and the root README's library list. `ATTRIBUTION.md` only if an asset is
  added (none planned).
- New parity rows for the Nextcloud source as a beyond-Lightroom feature. `LR-CLOUD-SYNC` stays out of scope.
- `docs/gaps.md` entry; `ROADMAP.md` / `docs/target-app-parity.md` numbers if a listed gap closes (bump their
  *Last updated* lines).
- `docs/control-protocol.md` and `docs/mcp.md` for the new commands.
- `cargo xtask parity` passes. Icons, if any, are drawn in code and recorded in `assets/ATTRIBUTION.md`.
- One task id per commit. Commit small and often.

## 10. Risks and open questions

- **Header window size** for ARW (A7R VI) and CR3 (R8): measure; the fallback is a second Range read.
- **Memories' API is not a documented stable API.** It is detected at run time and the header path is the fallback,
  so a breaking Memories update degrades speed, not function.
- **Blocking downloads:** an interactive original download blocks one render worker for ~1 s on the LAN, longer on
  WAN. Acceptable for v1; watch the loupe and Develop responsiveness budgets.
- **Server load:** ~48k preview requests on the first full scroll. previewgenerator has pre-rendered them, and
  8 parallel requests on reused connections (24 ms each) cost ~2.5 min.
- **TOFU with ureq:** it has no custom-verifier hook. `RootCerts::Specific` with a self-signed leaf may be rejected by
  rustls-webpki, in which case the fallback is a custom connector on ureq's `unversioned` API, which can change
  between minor versions. Prototype first; pin the ureq minor version if the connector is needed.
- **Two repositories:** LightCraft's PR waits for the craft-libs PR to merge and for the rev to be bumped. The
  personal build uses a `[patch]` to the local craft-libs branch meanwhile.
- **No content hash:** cross-folder duplicates are not detected, and a same-size in-place replacement keeps a stale
  thumbnail until the etag-based cache invalidation (§5.4) notices it.
