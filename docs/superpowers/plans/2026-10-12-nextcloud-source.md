# Nextcloud photo source — LightCraft integration plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Browse a whole Nextcloud photo archive in LightCraft and develop selected shots without mounting the server or downloading originals the user only browses.

**Architecture:** Catalog paths are `RemotePath` strings (`nextcloud://user@host/dir/x.ARW`), parsed as a volume. `Session::with_fs()` installs routing hooks: remote paths go to `engine::remote` (one `craft_nextcloud::Client` per account, one `OriginalsCache`), everything else to the unchanged fs hooks. Work done off the session (import listing, metadata, finished downloads) comes back as `remote::Event`s that `Session::drain_remote()` applies.

**Tech Stack:** Rust 2024, `craft-nextcloud` (craft-libs, ureq 3 + rustls + `rustls-rustcrypto`), egui.

**Spec:** `docs/superpowers/specs/2026-10-12-nextcloud-source-design.md`. Shared client: `~/GitHub/craft-libs/docs/nextcloud.md` and its plan `docs/superpowers/plans/2026-10-12-craft-nextcloud.md` (exact craft API below).

## Global Constraints

- CLAUDE.md "Never crash": no `unwrap`/`expect`/`panic!`/indexing on input-derived offsets in non-test code; every hostile value (server names, sizes, JSON/settings files) is checked.
- Pure Rust; the TLS provider is `rustls_rustcrypto::provider()` (`rustls-rustcrypto = "0.0.2-alpha"`, as `crates/fetch`).
- `craft-nextcloud` is a **native-only** engine dependency (`[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`); `cargo xtask wasm` must stay green. Code using it is `#[cfg(not(target_arch = "wasm32"))]`.
- Layering: nothing below L5 depends on egui. `lightcraft-catalog` never depends on `craft-nextcloud`.
- Everything is a command (`crates/engine/src/cmd/mod.rs`, `cmd!` macro). Command ids contain no `_`.
- Long-running work holds a `TaskGuard` from `session.activity.start(kind, label, cancel)` for its whole run; tests assert the row.
- The app password never appears in logs, settings, the catalog, errors, `Debug` output or command replies.
- Remote photos keep `content_hash: None` forever (thumbnail, view and smart-preview keys stay `file:{path}:{size}`).
- Refusal copy for local-only actions: `"{action} isn't available for Nextcloud photos"`.
- Gate before every commit: `cargo xtask ci` green (see "Build environment"). One task id per commit: `NC<n>: <summary>`. Check `git status` and `git diff --cached` before committing; stage only this task's files; never stage `.claude/`.
- Defaults: cache budget 20 GB (`20_000` MB); header window 256 KiB, retry window 4 MiB (Task NC5 measures and may raise them); sign-in poll every 2 s; listing workers 8; Memories `info` 8 at a time; reachability answer cached 30 s.

## Build environment (every task)

- Worktree: `/home/martin/GitHub/lightcraft/.claude/worktrees/nextcloud-source`, branch `feat/nextcloud-source`. Work and commit there; no new worktrees.
- `export CARGO_TARGET_DIR=/home/martin/GitHub/lightcraft/target/agent-nextcloud CARGO_PROFILE_DEV_DEBUG=line-tables-only` for every cargo command (the second matches what `xtask ci` uses, so targeted runs share its build cache).
- Iterate with targeted runs (`cargo test -p lightcraft-catalog --lib <filter>`, `cargo test -p lightcraft-engine --lib <filter>`, `cargo clippy -p <crate> --all-targets -- -D warnings`); run the full `cargo xtask ci` once before committing.
- `craft-nextcloud` comes from `../craft-libs/crates/nextcloud` relative to the workspace root, i.e. the read-only snapshot clone `/home/martin/GitHub/lightcraft/.claude/worktrees/craft-libs` (the controller updates it; never edit it). Its `testing` feature gives `FakeServer`.
- Never use control port 7980; pick one in 18000–19999. Never send OS-level synthetic input.

## craft-nextcloud API used (from its plan; check the snapshot's source if a call fails to compile)

- `RemotePath::parse(&str) -> Result<RemotePath, Error>`, `RemotePath::is_remote(&str) -> bool`, `.account_key() -> String`, `.segments() -> &[String]`, `.name() -> Option<&str>`, `.parent()`, `.join(&str) -> Result<RemotePath, Error>`, `Display`.
- `Account { server: String, user: String, trust: Option<Fingerprint> }` (serde), `.key() -> String`, `.root() -> RemotePath`. `Fingerprint`: `Display`/`FromStr` (`AB:CD:…`).
- `Client::new(account: Account, app_password: String, provider: Arc<rustls::crypto::CryptoProvider>) -> Result<Client, Error>` (`Clone`); `list(&RemotePath) -> Result<Vec<Entry>, Error>`, `stat(&RemotePath) -> Result<Entry, Error>`, `preview(&RemotePath, max_edge: u32) -> Result<Option<Vec<u8>>, Error>`, `read_range(&RemotePath, Range<u64>) -> Result<Vec<u8>, Error>`, `download_to(&RemotePath, &mut dyn Write, &AtomicBool, &mut dyn FnMut(u64, Option<u64>)) -> Result<u64, Error>`, `memories() -> Result<Option<Memories<'_>>, Error>`, `with_stall_limit(Duration)`.
- `Entry { name, is_dir, size: u64, etag: String, fileid: Option<u64>, mtime: Option<i64>, content_type: Option<String>, has_preview: bool }`.
- `Memories::days() -> Result<Vec<MemoriesPhoto>, Error>` (`fileid, dayid, w, h, etag, basename, size, epoch: Option<i64>, mimetype`), `Memories::info(fileid) -> Result<RemoteMeta, Error>` (`make, model, lens, exposure_time: Option<f64>, f_number, iso: Option<u32>, focal_length, orientation, captured: Option<i64>, width, height`).
- `SignIn::start(server: &str, provider) -> SignIn`; `.state() -> &SignInState`; `.confirm_trust(bool)`; `.poll()`; `.cancel()`. `SignInState::{NeedsTrust { fingerprint }, Waiting { login_url }, Done { account, app_password }, Failed(Error)}`.
- `OriginalsCache::open(dir: &Path, budget: u64) -> Result<_, Error>` (`Clone`); `cached(&RemotePath, etag: &str) -> Option<PathBuf>`; `fetch(&Client, &RemotePath, &AtomicBool, &mut dyn FnMut(u64, Option<u64>)) -> Result<Fetched { path, stale }, Error>`; `pin`/`unpin(&RemotePath) -> Result<(), Error>`; `hold(&RemotePath) -> Hold` (released on drop); `usage() -> Usage { total, pinned }`; `set_budget(u64)`.
- Feature `keyring`: `store_password(app: &str, &Account, &str)`, `load_password(app, &Account) -> Result<Option<String>, Error>`, `delete_password(app, &Account)`. App name `"LightCraft"`.
- `Error::{Unreachable(String), Unauthorized, NotFound, CertificateChanged, UnknownCertificate { fingerprint }, RateLimited, TooLarge, Malformed(String), Cancelled, Io(io::Error)}`; `Display` is user-facing. `craft_nextcloud::rustls` re-exports rustls.
- Feature `testing`: `FakeServer::start()` (plain HTTP on loopback), `start_tls(provider)`, `url()`, `account()`, `fingerprint()`, `add_dir`, `add_file(path, bytes) -> fileid`, `remove`, `set_preview(path, bytes)`, `approve_login()`, `enable_memories()`, `set_photo(path, dayid, w, h, epoch, exif_json)`, `fail_next(Fault)`, `requests()`, `connections()`; `FakeServer::USER = "alice"`, `FakeServer::PASSWORD`.

## Decisions taken while planning (record each in `docs/nextcloud.md` in NC9)

1. `lightcraft_catalog::is_remote(path)` (prefix `nextcloud://`) is the cfg-free routing check used by the engine and UI; it agrees with `RemotePath::is_remote`. The wasm build never sees `craft-nextcloud`.
2. Hosts opt in with `Session::with_default_nextcloud()`, like `with_default_face_models` (tests stay hermetic). `Session::new()` creates an unconfigured `Remote`; `with_fs()` routes to it.
3. Engine feature `keyring = ["craft-nextcloud/keyring"]`; the desktop app and CLI enable it; `with_default_nextcloud()` installs the keyring store only under that feature.
4. Results of off-session work reach the catalog as `remote::Event`s drained by `Session::drain_remote()`, called at the start of `Session::execute` and once per UI frame. Commands with `wait: true` drain inline until their worker ends.
5. The import reuses `ImportJob` / `commit_prepared` (import presets, import look, path dedup) through a new `ImportJob::prepare_infos`.
6. "Develop" means the Detail view with an edit tool open (`RightPanel::is_edit_tool`) or a percentage zoom. An unedited remote photo shown at Fit/Fill otherwise stays on the server preview (no download). An edited one renders from its original, cached or downloaded (spec said "smart preview if edited"; the smart preview is still used when the server is unreachable).
7. `library.importNextcloud {dirs}` and `nextcloud.browse {dir}` take full `nextcloud://` paths; the account comes from the path (the spec sketched an `account` param).
8. `OriginalsCache::cached` needs the etag: `Remote` remembers etags from listings, stats and fetches; a background lookup with no known etag does one `stat` (skipped while the account is unreachable). `ponytail:` a "newest copy" accessor in craft-nextcloud would remove that `stat`.
9. Downloads run one at a time behind one lock. `ponytail:` per-path in-flight tracking if a waiting loupe shows up in measurements.
10. Merge (HDR/Panorama), Convert to DNG, Duplicate, Edit In, file rename/move, folder operations, Synchronize Folder, sidecar writes and Show in Finder refuse remote photos (they write or act beside the original). Export, denoise and "all metadata" download through the byte hook.
11. End-to-end runs the in-process MCP server with `MemoryCredentials`; the CLI binary can't script sign-in without an OS keyring.
12. Settings live in `<config_dir>/nextcloud.json` (`{accounts, cacheBudgetMb}`); originals in `config::cache_dir()/nextcloud` (new fn: macOS `~/Library/Caches/LightCraft`, Windows `%LOCALAPPDATA%\LightCraft\Cache`, else `$XDG_CACHE_HOME/lightcraft` or `~/.cache/lightcraft`; `$LIGHTCRAFT_CACHE_DIR` overrides, empty disables).

## Review Focus

1. **Server unreachable mid-session** (laptop leaves the LAN): grid scrolling, availability checks and background renders must not each wait for a connect timeout. One failure marks the account unreachable for 30 s and every remote call short-circuits → NC5 test `unreachable_server_fails_fast`.
2. **Awkward names** (spaces, `#`, `%`, non-ASCII like `Datový Trezor/a b#1.jpg`) must survive catalog → `RemotePath::parse` → URL → load → NC6 test `awkward_names_import_and_load`.
3. **Revoked app password**: the account turns `signedOut`, errors say to sign in again, and further calls short-circuit until `nextcloud.signIn` → NC5 test `revoked_password_marks_account_signed_out`.
4. **Catalog references an account this machine doesn't have** (removed, or library copied to another computer): photos are offline, loads fail with a sign-in message, nothing panics → NC3 test `unknown_account_is_offline`.
5. **Batch jobs must not download**: Build Previews, the smart-preview refresh at library open, Build Smart Previews and the face scan run their loads as background work → NC5 test `batch_builders_never_download`.

---

### Task 1 (NC1): `nextcloud://` as a catalog volume

**Files:**
- Modify: `crates/catalog/src/query.rs` (`split_path` ~490, `folder_key` ~538), `crates/catalog/src/folders.rs` (`place` ~128, `folder_label` ~180), `crates/catalog/src/lib.rs` (re-export)
- Test: `crates/catalog/src/tests_folders.rs`, `crates/catalog/src/tests.rs`

**Interfaces — Produces:**
- `pub const REMOTE_SCHEME: &str = "nextcloud://";` and `pub fn is_remote(path: &str) -> bool` (prefix check) in `query.rs`, re-exported as `lightcraft_catalog::{is_remote, REMOTE_SCHEME}`.
- `split_path("nextcloud://root@cloud.dev-null.me/Sony A7R VI/x.ARW", _)` → `prefix == "nextcloud://root@cloud.dev-null.me"`, `parts == ["Sony A7R VI", "x.ARW"]`, `absolute == true`. Backslashes are not rewritten inside remote paths. `..` never climbs above the prefix.
- `folder_key` keeps remote keys' case on every platform (server paths are case-sensitive).
- `place()` puts a remote folder on a volume of its own, keyed by the prefix; its label is `"<user>@<host>"` (the authority). `is_disk_root("nextcloud://root@h/")` is true.

- [ ] **Step 1: Write failing tests** in `tests_folders.rs`:
  - `a_nextcloud_account_is_a_volume_of_its_own`: photos `nextcloud://root@cloud.dev-null.me/Sony A7R VI/2026/a.ARW` and `…/Canon R8/b.CR3` plus a local `/home/u/c.jpg` → `folder_tree()` has one top node for the account (label `root@cloud.dev-null.me`) containing `Sony A7R VI` and `Canon R8`, and the local disk node; `volume_of` of both remote photos is equal and differs from the local one.
  - `remote_paths_keep_their_case_and_stay_inside_the_account`: `folder_key("nextcloud://u@h/A/b/../C")` == `"nextcloud://u@h/A/C"`; `folder_key("nextcloud://u@h/../../x")` == `"nextcloud://u@h/x"`; keys of `…/Dir` and `…/dir` differ.
  - `nextcloud_roots_are_disk_roots`: `is_disk_root("nextcloud://u@h/")` and `!is_disk_root("nextcloud://u@h/Dir")`.
  - `the_tree_and_the_filter_agree_on_remote_folders`: extend the existing `every_row_shows_exactly_as_many_photos_as_it_says` style check to remote photos.
  - In `tests.rs`: `is_remote_is_a_prefix_check` (`nextcloud://a@b/x` true; `/home/x`, `C:\x`, `//h/s`, `nextcloud:/a` false).
- [ ] **Step 2:** `cargo test -p lightcraft-catalog --lib` → the new tests fail.
- [ ] **Step 3:** Implement: in `split_path`, before the backslash rewrite, if `is_remote(path)` take the authority up to the next `/` as the prefix and split the rest; in `folder_key` skip the Windows lower-casing for remote keys; in `place()` map the remote prefix to its own volume.
- [ ] **Step 4:** Tests pass; all existing catalog tests still pass.
- [ ] **Step 5:** `cargo xtask ci`; commit `NC1: nextcloud:// paths as a catalog volume`.

### Task 2 (NC2): Local-only actions refuse remote photos; existence checks use the availability probe

**Files:**
- Modify: `crates/engine/src/cmd/mod.rs` (helper), `sidecar.rs`, `import.rs`, `cmd/library.rs` (`import_params`), `cmd/missing.rs`, `cmd/browse.rs`, `sync.rs`, `cmd/convert.rs`, `merge.rs`, `rename.rs` (via its callers), `cmd/previews.rs:~508`, `segment/mod.rs:~396`, `crates/mcp/src/headless.rs` (`expand_paths` ~179)
- Test: `crates/engine/src/tests_remote_paths.rs` (new, registered in `lib.rs`), `crates/mcp/src/headless.rs` tests

**Interfaces:**
- Consumes: `lightcraft_catalog::is_remote` (NC1).
- Produces: `pub(crate) fn local_only(path: &str, action: &str) -> Result<()>` in `cmd/mod.rs`, returning `EngineError::Other(format!("{action} isn't available for Nextcloud photos"))` for remote paths, `Ok(())` otherwise.

Behaviour per site (each bullet gets one test in `tests_remote_paths.rs`, using a session with an injected availability probe via `s.media.availability.set_probe(...)` and photos added with `Source::File { path: "nextcloud://alice@127.0.0.1/…" }`):
- **Sidecars:** `find_sidecar`/`read_packet` return `None` for remote paths (no fs call); auto-write after an edit skips remote photos silently; `xmp.save` on one returns the refusal (`"Saving XMP"`). Test `sidecars_skip_remote_photos`: edit a remote photo with auto-write on → `Ok`, no file created; `xmp.save` → error text.
- **Import:** `import_params` rejects remote paths in `library.import` with `"use library.importNextcloud for Nextcloud folders"`; `trashed_file_is_gone` asks the availability probe instead of `Path::exists`; `scan_with`/`prepare_files` skip the `fs::metadata` cache check for remote paths. Test `library_import_rejects_remote_paths`.
- **Missing photos:** `missing::missing` (`library.missing`) and `plan_find_missing` use `availability.probe()`; Find Missing skips remote candidates; `photo.relink` refuses a remote target path (`"Relinking"`). Tests `missing_uses_the_availability_probe`, `find_missing_skips_remote_photos`.
- **Browse and folders:** `library.browse`, folder rename/move/create/remove and `photo.moveToFolder` refuse remote paths (`"Browsing"`, `"Renaming folders"`, `"Moving files"`); `Session::plan_rename`/`apply_rename` refuse remote photos (`"Renaming files"`). Test `folder_and_file_operations_refuse_remote_paths`.
- **Synchronize Folder:** `SyncInput::new` on a remote folder refuses (`"Synchronize Folder"`). Test `synchronize_refuses_remote_folders`.
- **Writes beside the original:** `photo.convertToDng` (`"Convert to DNG"`), `photo.duplicate` (`"Duplicate"`), `photo.editExternal` (`"Edit In"`), Photo Merge planning (`"Photo Merge"`) refuse when any target is remote. Test `writes_beside_the_original_refuse_remote_photos`.
- **Existence checks:** `photo.smartPreview`'s `originalOnline` and `segment_job`'s missing-file check use the availability probe. Test `ai_masks_do_not_call_a_remote_photo_missing` (probe says present → no "file is missing" error from the check).
- **MCP/CLI paths:** `expand_paths` leaves remote strings unchanged (no `is_dir`, no `std::path::absolute`). Test in `headless.rs`: `expand_paths(&["nextcloud://a@b/x.jpg".into()]) == ["nextcloud://a@b/x.jpg"]`.

- [ ] **Step 1:** Write the tests above → `cargo test -p lightcraft-engine --lib tests_remote_paths` and `cargo test -p lightcraft-mcp --lib expand` fail.
- [ ] **Step 2:** Implement `local_only` and the per-site checks (a guard at each entry point, not inside low-level fs helpers).
- [ ] **Step 3:** Targeted tests pass; full engine lib tests pass.
- [ ] **Step 4:** `cargo xtask ci`; commit `NC2: local-only actions refuse Nextcloud photos`.

### Task 3 (NC3): craft-nextcloud dependency, `engine::remote` core, account settings

**Files:**
- Modify: root `Cargo.toml` (workspace dep + `[patch]`), `crates/engine/Cargo.toml`, `crates/engine/src/lib.rs` (`remote` field and module), `crates/engine/src/config.rs` (`cache_dir`, `with_default_nextcloud`), `crates/engine/src/cmd/mod.rs` (register module)
- Create: `crates/engine/src/remote/mod.rs`, `crates/engine/src/cmd/nextcloud.rs`, `crates/engine/src/tests_nextcloud.rs`

**Interfaces — Produces:**
- Root `Cargo.toml`: `craft-nextcloud = { git = "https://github.com/storytold/craft-libs", rev = "5c874d51eeb3de70e735c8ce87a59a7dedd47675" }` next to `craft-ui`, and
  ```toml
  # DROP before the upstream PR: local craft-libs until craft-nextcloud is merged and the rev bumped.
  [patch."https://github.com/storytold/craft-libs"]
  craft-nextcloud = { path = "../craft-libs/crates/nextcloud" }
  ```
- Engine `Cargo.toml` (native target section): `craft-nextcloud = { workspace = true }`, `rustls-rustcrypto = "0.0.2-alpha"`; native dev-deps: `craft-nextcloud = { workspace = true, features = ["testing"] }`; `[features] keyring = ["craft-nextcloud/keyring"]`.
- `config::cache_dir() -> Option<PathBuf>` per Decision 12, with `cache_dir_with(platform, env)` unit tests mirroring `config_dir_with`'s.
- `remote/mod.rs` (`#[cfg(not(target_arch = "wasm32"))]`, re-exports `craft_nextcloud::{Account, RemotePath}`):
  - `pub trait CredentialStore: Send + Sync { fn get(&self, a: &Account) -> Result<Option<String>, String>; fn set(&self, a: &Account, password: &str) -> Result<(), String>; fn delete(&self, a: &Account) -> Result<(), String>; }`
  - `#[derive(Default)] pub struct MemoryCredentials` (pub: other crates' tests use it); `#[cfg(feature = "keyring")] pub struct KeyringCredentials` (app name `"LightCraft"`).
  - `pub struct RemoteConfig { pub settings_file: Option<PathBuf>, pub cache_dir: Option<PathBuf>, pub credentials: Option<Arc<dyn CredentialStore>> }`
  - `pub enum AccountState { SignedIn, SignedOut, Unreachable, SigningIn }` (serde camelCase).
  - `pub struct Remote` with `pub fn new(activity: Activity) -> Remote`, `pub fn configure(&self, config: RemoteConfig)` (loads the settings file; a missing/corrupt/oversized (>1 MiB) file means no accounts, never an error), `pub fn accounts(&self) -> Vec<(Account, AccountState)>`, `pub fn client(&self, account_key: &str) -> Result<craft_nextcloud::Client, String>` (cached per account; unknown account → `"No Nextcloud account {key} on this computer; add it with File ▸ Add Nextcloud Account…"`; no password → `"Sign in to Nextcloud ({key}) again: Settings ▸ Nextcloud"`), `pub(crate) fn note_error(&self, account_key: &str, e: &craft_nextcloud::Error)` (Unauthorized → `SignedOut`, Unreachable → `Unreachable` for 30 s), `pub fn remove_account(&self, key: &str) -> Result<(), String>`, `pub fn cache_budget_mb(&self) -> u64`, `pub fn set_cache_budget_mb(&self, mb: u64) -> Result<(), String>`, `pub fn usage(&self) -> craft_nextcloud::Usage` (zero when no cache), `pub(crate) fn provider() -> Arc<CryptoProvider>`.
  - Settings file shape: `{"accounts": [Account…], "cacheBudgetMb": 20000}`, written atomically (follow `cmd/face_models.rs` `write_atomic`).
- `Session` field `#[cfg(not(target_arch = "wasm32"))] pub remote: Arc<remote::Remote>` created in `Session::new()` from `self.activity.clone()`; `pub fn with_default_nextcloud(self) -> Self` (in `config.rs`): configures `config_dir()/nextcloud.json`, `cache_dir()/nextcloud`, and `KeyringCredentials` under the `keyring` feature.
- Commands in `cmd/nextcloud.rs` (registered native-only):
  - `cmd!(query "nextcloud.preferences", "Nextcloud Preferences", [], None, "{cacheBudgetMb?}", always, …)` → `{"accounts": [{"account": key, "server", "user", "root": "nextcloud://…/", "state"}], "cacheBudgetMb", "cachedBytes", "pinnedBytes"}`; `cacheBudgetMb` below 1000 is `bad(…, "the cache budget must be at least 1000 MB")`.
  - `nextcloud.removeAccount {account}` → deletes the password (credential store), the settings entry and the client; photos stay. → `{"removed": key}`.

- [ ] **Step 1: Failing tests** in `tests_nextcloud.rs` (helper `fn nc_session(dir: &Path) -> Session` = `Session::new().with_fs()` + `remote.configure` with a temp settings file, temp cache dir and `MemoryCredentials`):
  - `settings_round_trip`: write accounts via the settings file, reconfigure → `accounts()` lists them; `nextcloud.preferences {cacheBudgetMb: 5000}` persists `5000`.
  - `corrupt_settings_mean_no_accounts`: settings file `b"{nope"` → `accounts()` empty, no error.
  - `remove_account_deletes_password_and_entry`: store a password, remove → `MemoryCredentials` has none, settings have none, `client(key)` errors with the "add it" message.
  - `unknown_account_is_offline` (Review Focus 4): `client("bob@nowhere")` → error naming File ▸ Add Nextcloud Account…; no panic.
  - `preferences_never_show_the_password`: reply JSON doesn't contain the stored password.
  - `config::tests`: `cache_dir_with` for Mac/Windows/Other/override/empty.
- [ ] **Step 2:** Run → fail (`cargo test -p lightcraft-engine --lib tests_nextcloud`).
- [ ] **Step 3:** Implement. Check `cargo tree -p lightcraft-engine -e normal | grep -E "ring |aws-lc|openssl|webpki-root"` prints nothing new beyond `crates/fetch`'s `webpki-roots`.
- [ ] **Step 4:** Tests pass; `cargo xtask wasm` and `cargo xtask layers` pass.
- [ ] **Step 5:** `cargo xtask ci`; commit `NC3: craft-nextcloud dependency and Nextcloud account settings`.

### Task 4 (NC4): Sign-in commands

**Files:** Modify `crates/engine/src/remote/mod.rs`, `crates/engine/src/cmd/nextcloud.rs`, `crates/engine/src/activity.rs` (kind doc list); test `crates/engine/src/tests_nextcloud.rs`.

**Interfaces — Produces:**
- `nextcloud.addAccount {server, trust?: "<fingerprint>", wait?: false}` and `nextcloud.signIn {account, wait?: false}` (menu `[]`; the UI adds menu entries).
  - Unknown issuer and no matching `trust` → `{"state": "needsTrust", "fingerprint": "AB:…"}` (the caller re-runs with `trust`); a `trust` that doesn't match → error `"The certificate's fingerprint changed; check it with the server's admin"`.
  - Waiting → a polling thread (poll every 2 s) holding `activity.start("nextcloud", "Waiting for Nextcloud approval…", Cancel::Flag(..))`; the command returns `{"state": "waiting", "loginUrl": …}`. With `wait: true` the command blocks until done and returns `{"state": "signedIn", "account": key}`.
  - Done → `credentials.set`, account (with trust) added or replaced in settings, cached client dropped, state `SignedIn`. Failed/cancelled → state back to `SignedOut`, row gone.
  - No credential store → error `"This build can't store Nextcloud passwords (no OS keyring support)"`; a store error → its message.
  - `nextcloud.signIn` reuses the account's server and pinned fingerprint.
- `Remote::sign_in(&self, server: &str, trust: Option<Fingerprint>, wait: bool) -> Result<Value, String>`.

- [ ] **Step 1: Failing tests** (`FakeServer`; approve from a helper thread after the row appears):
  - `plain_sign_in_adds_the_account`: `addAccount {server: fake.url()}` → `waiting` + `loginUrl`; activity row `("nextcloud", "Waiting for Nextcloud approval…")`; `approve_login()`; wait for the row to go → `accounts()` has `alice@127.0.0.1:<port>` `SignedIn`; `MemoryCredentials` holds `FakeServer::PASSWORD`; the reply JSON never contains it.
  - `wait_true_blocks_until_signed_in`.
  - `tls_sign_in_asks_for_trust`: `FakeServer::start_tls(provider)` → `needsTrust` with `fake.fingerprint()`; re-run with `trust` → waiting → approve → account has `trust == Some(fp)`.
  - `cancelling_the_row_stops_sign_in`: `activity.cancel(id)` → row gone, account not added.
  - `no_keyring_is_an_actionable_error`: store whose `set` errors → command error contains its message.
- [ ] **Step 2:** fail. **Step 3:** implement. **Step 4:** pass.
- [ ] **Step 5:** `cargo xtask ci`; commit `NC4: Nextcloud sign-in commands`.

### Task 5 (NC5): Routing hooks, downloads and the header probe

**Files:**
- Create: `crates/engine/src/remote/hooks.rs`, `crates/engine/src/remote/originals.rs`
- Modify: `crates/engine/src/files.rs` (`with_fs`, a `pub(crate) fn decode_upright_srgb`), `crates/engine/src/media.rs` (`QuickJob.keep_embedded`, `quick_thumb_job`), `crates/engine/src/cmd/previews.rs` (background loads), the face-scan worker
- Test: `crates/engine/src/tests_nextcloud_hooks.rs` (new)

**Interfaces — Produces:**
- `with_fs()` wraps each hook as `if is_remote(path) { remote… } else { fs… }`: `file_loader`, `file_probe`, `preview_loader`, `denoise.loader`, and now `file_bytes = Some(..)` natively (remote → `local_original` + `std::fs::read`; local → `std::fs::read`), and `availability.set_probe(..)` (remote → `Remote::exists`, local → the current `fs_exists`).
- `Remote::local_original(&self, path: &str) -> Result<PathBuf, String>`: known etag → `cache.cached`; unknown etag → one `stat` (skipped while unreachable) → `cached`. On a miss: under `memory::is_background()` → `Err(NOT_DOWNLOADED)` (`"The original isn't downloaded yet"`), else `OriginalsCache::fetch` behind the download lock with `activity.start("download", &format!("Downloading {name} ({size})"), Cancel::Flag(..))`, `Unit::Bytes` progress. A foreground load while the account is unreachable still calls `fetch`, which returns the newest cached copy (`stale: true`) without downloading. `size` is `"26 MB"` style (decimal MB, integer).
- `NotFound` from `stat`/`fetch` records the path as gone: `exists` answers `false` for it (the photo shows as missing) until a listing contains it again.
- File loader for remote: `local_original` → fs loader on the local path. Background miss: non-raw extensions (`jpg jpeg png tif tiff webp gif bmp heic heif avif jxl psd`) decode `Client::preview(path, max_edge.min(4096))` with `files::load_bytes`; raws return the error (the smart-preview fallback applies).
- Pair loader and byte reader: `local_original` → fs.
- Preview loader: `Client::preview(path, max_edge.min(4096) as u32)` → `decode_upright_srgb(bytes, max_edge) -> Option<Rgba8>` (decode + fit like `quick_image_srgb`, no orientation: server previews are upright).
- Probe: cached original → fs probe on it; else `read_range(0..HEADER_WINDOW)` → `probe_bytes(name, &bytes)`, retried once with `HEADER_WINDOW_RETRY` on error; `file_size` from the listing/stat entry; `content_hash` always `None`.
- `Remote::exists(&self, path: &str) -> bool`: account configured and signed in, reachable (cached 30 s; checked with `stat(root)`), and, when the parent folder was listed this session, the path was in that listing.
- `QuickJob.keep_embedded: bool` set by `quick_thumb_job` for remote paths; `QuickJob::run` then `put`s the embedded image into its first `cached` entry (the thumbnail disk cache).
- `library.buildPreviews`, `library.smartPreviews` (incl. `refresh_stale_smart_previews`) and the face scan run their per-photo loads inside `memory::in_background`.

- [ ] **Step 1: Measure the header window.** Using real files (read-only): `~/GitHub/lightcraft/corpus/` raws if present, and `head -c 8M` copies of one ARW from `~/mnt/nextcloud/Sony A7R VI/…` and one CR3 from `~/mnt/nextcloud/Canon R8/…` into the scratch dir. For windows 64 KiB…8 MiB, compare `probe_bytes(name, &bytes[..w])` with the full-file probe (width, height, captured, camera, lens, shutter, aperture, iso). Set `HEADER_WINDOW` to the smallest window that matches for both, rounded up to a power of two (keep 256 KiB if it already works) and `HEADER_WINDOW_RETRY` to 4 MiB or 2× the largest need. Put the table in the commit message body. Never commit the files.
- [ ] **Step 2: Failing tests** (`FakeServer` with a small real JPEG with EXIF from `lightcraft-scenes`/existing test helpers, and a synthetic `.ARW` byte blob):
  - `fs_paths_are_untouched`: a local temp JPEG loads and probes as before; `fake.requests()` stays 0.
  - `background_loads_never_download`: `memory::in_background(|| loader(raw_path, 512))` → `Err`; cache dir has no file; for a remote JPEG it returns server-preview pixels.
  - `an_interactive_miss_downloads_once`: two loads → one GET (request count delta: stat+GET, then stat only); the second is a cache hit.
  - `downloads_show_and_cancel_in_activity`: `fail_next(Stall)` + client stall limit 2 s → the `("download", "Downloading x.ARW (…)")` row appears; after the load returns the row is gone and no file is cached.
  - `previews_come_from_the_server_upright`: `set_preview` with a 64×32 JPEG → preview loader returns 64×32.
  - `remote_probe_reads_the_header_only`: probe returns dimensions/date; `content_hash == None`; requests are Range reads (no full GET).
  - `remote_thumbnails_land_in_the_disk_cache`: run `quick_thumb_job` twice → second is `QuickSource::Cached`.
  - `stale_copy_is_used_when_the_server_is_down`: download, drop the server → interactive load still decodes the cached copy.
  - `deleted_on_server_shows_missing`: `remove` the file → interactive load errors with the craft `NotFound` text and `exists` turns `false`.
  - `unreachable_server_fails_fast` (Review Focus 1): drop the server; 50 background loads + 50 `exists` calls finish within 3 s total and all report offline.
  - `revoked_password_marks_account_signed_out` (Review Focus 3): wrong password → load error says "sign in again", state `SignedOut`, a second load sends no request.
  - `batch_builders_never_download` (Review Focus 5): `library.buildPreviews {wait: true}` and `library.smartPreviews` over remote raws → no GET of any original.
- [ ] **Step 3:** Run → fail. **Step 4:** Implement. **Step 5:** pass; existing `tests_offline` UI tests still pass.
- [ ] **Step 6:** `cargo xtask ci`; commit `NC5: route Nextcloud paths through the engine's file hooks`.

### Task 6 (NC6): Browse and import from Nextcloud

**Files:**
- Create: `crates/engine/src/remote/import.rs`
- Modify: `crates/engine/src/remote/mod.rs` (`Event`, `take_events`), `crates/engine/src/lib.rs` (`drain_remote`, called first in `execute`), `crates/engine/src/import.rs` (`ImportJob::prepare_infos`, `pub(crate)` on what it needs), `crates/engine/src/cmd/convert.rs` (`fill_missing_meta` → `pub(crate)`), `crates/engine/src/cmd/nextcloud.rs`
- Test: `crates/engine/src/tests_nextcloud_import.rs` (new)

**Interfaces — Produces:**
- `nextcloud.browse {dir: "nextcloud://…/"}` (query) → `{"dir", "folders": [{"name", "path"}], "photos": <count of importable files>}`; sorted by name.
- `library.importNextcloud {dirs: ["nextcloud://…"], wait?: false}` (menu `[]`) → starts a worker and returns `{"started": true}`; with `wait: true` returns `{"added": n, "skipped": n, "metadata": n}` after both phases.
  1. **List**: walk the dirs with 8 worker threads (`Client::list`), keep files whose lower-cased extension is in `import::EXTENSIONS`; remember etags and listed folders in `Remote`; row `("import", "Importing from Nextcloud")` with `detail` = current folder name and progress = files found.
  2. **Memories join** when `client.memories()` is `Some`: `days()` joined by fileid gives `w`, `h`, `epoch` (→ `captured` as `YYYY-MM-DDTHH:MM:SS`, epoch read as local wall time).
  3. **Commit**: batches of `import::batch_size()` as `Event::Found(Vec<(String, ProbeInfo)>)`; the drain runs `ImportJob::new(s, opts)` + `prepare_infos` + `commit_prepared` (mode Add; a path already in the catalog is skipped). `ProbeInfo`: `file_size` = entry size, `kind` = `Raw` unless the extension is a non-raw image, `format` = what the local probe reports for that extension (upper-case extension, `JPG`→`JPEG`, `TIF`→`TIFF`), `content_hash: None`.
  4. **Metadata** (same worker, after listing): for listed photos whose catalog `meta.camera` was empty when the command started, Memories `info(fileid)` 8 at a time, else the NC5 header probe; each result is `Event::Meta { path, info }`, applied with `fill_missing_meta` through `apply_system` (no undo step). Row `("scan", "Reading metadata")`, progress n of m, `Cancel::Flag`. A rescan redoes photos still missing a camera.
- `ImportJob::prepare_infos(&mut self, items: Vec<(String, ProbeInfo)>) -> Prepared` applies the same by-path dedup as `prepare_files` and no file-system calls.
- `Session::drain_remote(&mut self)`: applies queued events; cheap when empty (one lock).

- [ ] **Step 1: Failing tests:**
  - `browse_lists_one_level`: dirs and photo count; hostile names already dropped by the client don't appear.
  - `import_lists_joins_memories_and_reads_metadata`: Memories on, 3 photos with `set_photo` (w/h/epoch, exif Make/Model/LensModel/ExposureTime/FNumber/ISO) → `wait: true` → 3 photos with `Source::File` remote paths, width/height/captured from Memories, camera/lens/exposure filled, `content_hash == None`; no GET of any original.
  - `import_without_memories_probes_headers`: Memories off, a JPEG with EXIF → dims, date and camera from the header probe.
  - `rescan_adds_only_new_files_and_finishes_metadata`: run twice with one file added in between → one new photo; no duplicates.
  - `import_rows_show_in_activity`: `wait: false` → `("import", "Importing from Nextcloud")` row appears, then `("scan", "Reading metadata")`; after the worker ends and `drain_remote` runs, the catalog has all photos and no rows remain.
  - `awkward_names_import_and_load` (Review Focus 2): `Datový Trezor/a b#1%.jpg` imports, renders a thumbnail via the server preview and loads interactively.
  - `import_never_downloads_originals`: request log shows only PROPFIND/preview/Range/Memories requests (use `requests()` deltas and the empty cache dir).
- [ ] **Step 2:** fail. **Step 3:** implement. **Step 4:** pass.
- [ ] **Step 5:** `cargo xtask ci`; commit `NC6: import Nextcloud folders`.

### Task 7 (NC7): Originals in Develop — hold, prefetch, completion after download, offline copies

**Files:** Modify `crates/engine/src/remote/originals.rs`, `crates/engine/src/remote/mod.rs`, `crates/engine/src/lib.rs` (`drain_remote` handles `Downloaded`), `crates/engine/src/cmd/nextcloud.rs`; test `crates/engine/src/tests_nextcloud_originals.rs` (new).

**Interfaces — Produces:**
- `Remote::hold(&self, path: Option<&str>)`: keeps one `OriginalsCache::hold` for the photo open in Develop (replaced or released by the next call).
- `Remote::prefetch(&self, path: Option<&str>)`: one background download at a time, own row `("download", "Prefetching {name}")`, `Cancel::Flag`; a different path or `None` cancels the running one; a cached path does nothing.
- `local_original` pushes `Event::Downloaded { path }` after a download (not after a cache hit); drained → full local probe of the cached file (fs probe, `content_hash` forced `None`) → `content_op`, `fill_missing_meta`, `lens_ops`, `as_shot_wb_op` via `apply_system`; then `library.smartPreviews {ids, background: true}` (check its params string at `cmd/previews.rs:~487`) when the library has a smart-preview folder.
- `photo.makeAvailableOffline {ids?, wait?: false}` (menu `["Photo"]`, label `"Make Available Offline"`, enabled when a target is remote) → pins and fetches each remote target in a worker with row `("download", "Making photos available offline")`, progress n of m → `{"photos": n}`. `photo.removeOfflineCopy {ids?}` (menu `["Photo"]`, label `"Remove Offline Copy"`) → `unpin` → `{"photos": n}`. `nextcloud.preferences` reports `pinnedBytes` (NC3).

- [ ] **Step 1: Failing tests:**
  - `the_open_photo_is_held`: budget smaller than two files; hold A, fetch B → A survives eviction; `hold(None)` → A evictable.
  - `prefetch_downloads_the_next_photo_once`: `prefetch(Some(b))` → row appears then goes, B cached; `prefetch(Some(b))` again → no request.
  - `prefetch_is_cancelled_by_moving_on`: stalled B, `prefetch(Some(c))` → B's row goes, B not cached.
  - `first_download_completes_the_record`: interactive load of a remote JPEG with EXIF whose catalog record lacks camera → after `drain_remote` the record has camera and `content_hash == None`; smart-preview build requested when the library has a folder.
  - `offline_pins_survive_eviction`: `photo.makeAvailableOffline {wait: true}` → `pinnedBytes > 0`; filling the cache doesn't evict them; `photo.removeOfflineCopy` → `pinnedBytes == 0`.
  - `offline_commands_ignore_local_photos`: a local target is skipped; command disabled with only local targets.
- [ ] **Step 2:** fail. **Step 3:** implement. **Step 4:** pass.
- [ ] **Step 5:** `cargo xtask ci`; commit `NC7: originals in Develop, prefetch and offline copies`.

### Task 8 (NC8): User interface

**Files:**
- Modify: `crates/ui-egui/src/state.rs` (`Dialog` variants), `panels/dialogs.rs` (titles, bodies, confirm, keeps-open), `menus.rs` (`UI_COMMANDS`, `run_ui_command`, `ui_enabled` for Show in Finder), `menubar.rs` (`LAYOUT` placement), `panels/settings.rs` (Nextcloud tab), `panels/detail.rs` (Develop policy), `lib.rs` (per-frame `drain_remote`, release hold/prefetch outside Develop), `panels/left.rs` (`missing_count` via availability)
- Create: `crates/ui-egui/src/nextcloud.rs` (dialog logic + folder picker), `crates/ui-egui/src/tests_nextcloud.rs`

**Interfaces:**
- Consumes: NC3–NC7 commands; `crate::links::open`; `tasks::spawn`; `TextField`.
- UI commands: `("file.addNextcloudAccount", "Add Nextcloud Account…", None, "File")`, `("file.importNextcloud", "Import from Nextcloud…", None, "File")`, placed after `file.importLightroom` in `LAYOUT`. `photo.makeAvailableOffline` / `photo.removeOfflineCopy` appear in Photo via their engine menu path.
- `Dialog::NextcloudSignIn { server: String, fingerprint: Option<String>, login_url: Option<String>, error: Option<String> }`: server field (`TextField`, id `field:nextcloudServer`) → Sign In runs `nextcloud.addAccount`; `needsTrust` shows the fingerprint with **Trust This Certificate** / Cancel; `waiting` opens `loginUrl` via `links::open` and shows "Approve LightCraft in your browser…" with Cancel (cancels the activity row); the dialog closes when `nextcloud.preferences` shows the account `signedIn` (checked at most every 2 s).
- `Dialog::NextcloudImport { dir: String, folders: Vec<(String, String)>, chosen: Vec<String>, loading: bool, error: Option<String> }`: account picker (from `nextcloud.preferences`), one level at a time via `nextcloud.browse` on `tasks::spawn`, a checkbox per folder, **Import** runs `library.importNextcloud {dirs: chosen}`. No accounts → the dialog offers Add Nextcloud Account….
- Settings tab `"nextcloud"` (label "Nextcloud"): accounts with state, **Sign In Again** (runs `nextcloud.signIn`, opens its login URL) and **Remove**; cache budget choices 5 / 10 / 20 / 50 / 100 GB; "Cached: X GB · Offline copies: Y GB".
- Detail view: `fn wants_original(app) -> bool` = an edit tool is open or zoom is a percentage. For a remote photo: when `!wants_original` and the photo is unedited (`embedded_thumb_is_final`), don't request the `Slot::Main` loupe job (the quick view is final); when `wants_original`, `remote.hold(Some(path))` and `remote.prefetch(next remote photo in visible order)`. Outside the Detail view (checked once per frame in `lib.rs`): `remote.hold(None)`, `remote.prefetch(None)`. `session.drain_remote()` once per frame next to the import tick.
- Show in Finder is disabled for remote photos with the NC2 refusal as its reason; `missing_count` uses the availability probe.

- [ ] **Step 1: Failing tests** in `tests_nextcloud.rs` (existing kittest harness, `FakeServer`, `MemoryCredentials`):
  - `menus_list_the_nextcloud_items`.
  - `sign_in_dialog_walks_trust_and_waiting`: TLS fake → fingerprint shown → trust → waiting text → `approve_login` → dialog closes, account listed.
  - `import_dialog_browses_and_imports`.
  - `browsing_unedited_remote_photos_downloads_nothing`: Detail view, Info panel, Fit → frames run, no original in the cache; switching to the Edit panel downloads it once and prefetches the next.
  - `settings_tab_shows_accounts_and_sizes`.
  - `show_in_finder_is_disabled_for_remote_photos`.
- [ ] **Step 2:** fail. **Step 3:** implement. **Step 4:** pass.
- [ ] **Step 5: Look at it.** Run `lightcraft` with `--control <port 18000–19999>` against a `FakeServer`-free library (the dialogs' empty states) and take `ui.screenshot` of the sign-in dialog, the import dialog and the settings tab (headless if no display); view the images and fix anything misaligned.
- [ ] **Step 6:** `cargo xtask ci`; commit `NC8: Nextcloud dialogs, menus and Develop policy`.

### Task 9 (NC9): Hosts, end-to-end test, docs

**Files:**
- Modify: `apps/lightcraft/Cargo.toml` + `apps/lightcraft-cli/Cargo.toml` (`lightcraft-engine` with `features = ["keyring"]`), `apps/lightcraft/src/main.rs` (`open_library_session`, `with_default_nextcloud()`), `apps/lightcraft-cli/src/main.rs` (`render`, `merge`, `run` chains), `crates/mcp/src/headless.rs` (`Headless::default`/`demo`)
- Test: `crates/mcp/tests/e2e.rs` (`nextcloud_add_import_render_export`)
- Docs: create `docs/nextcloud.md`; modify `docs/control-protocol.md`, `docs/mcp.md`, `docs/parity-checklist.md`, `docs/gaps.md`, `ROADMAP.md` / `docs/target-app-parity.md` only if a listed number moves; `README.md` features list if it has one for sources.

**Interfaces — Produces:**
- Every host session chain gains `.with_default_nextcloud()` after `.with_fs()`.
- `docs/nextcloud.md`: setup (Add Account, trust prompt, browser approval), what downloads when (grid/loupe: never; Develop: the open photo + the next; export/denoise/merge-refusal), caching (budget, eviction, offline copies, where files live), offline behaviour, certificate pinning, privacy (password only in the OS keyring), limits (no write-back, moved files go missing, no duplicates detection across folders), commands list, Decisions 1–12.
- Parity rows (beyond Lightroom, `LR-CLOUD-SYNC` unchanged): `LR-IMP-NEXTCLOUD` (`cmd:library.importNextcloud`, `cmd:nextcloud.addAccount`, `crates/engine/src/remote/mod.rs`), `MENU-FILE-NEXTCLOUD-ACCOUNT` (`cmd:file.addNextcloudAccount`), `MENU-FILE-NEXTCLOUD-IMPORT` (`cmd:file.importNextcloud`), `MENU-PHOTO-OFFLINE` (`cmd:photo.makeAvailableOffline`, `cmd:photo.removeOfflineCopy`). Bump "Last updated" + revision history in each touched progress doc. `docs/gaps.md`: one unranked limitations line under "Gaps by kind" or the closest section for "Nextcloud: no XMP write-back, no relink by fileid" (not a Lightroom gap; no rank).
- `docs/control-protocol.md`: a `### Nextcloud` subsection listing the commands and the `needsTrust` → `trust` → `waiting` sequence. `docs/mcp.md`: a `## Nextcloud` section (sign-in prints `loginUrl`; agents pass `wait`).

- [ ] **Step 1: Failing e2e test** `nextcloud_add_import_render_export`: `Headless::default()` with `remote.configure` (temp dirs, `MemoryCredentials`), `FakeServer` with Memories and two JPEGs → MCP calls `cmd_nextcloud_addAccount {server}` (approve from a thread), `cmd_library_importNextcloud {dirs, wait: true}`, `set_develop`, `render_photo`, `export` → all `isError == false`, export file exists and decodes.
- [ ] **Step 2:** fail. **Step 3:** implement host wiring and docs. **Step 4:** pass; `cargo xtask parity --write` then `cargo xtask parity`.
- [ ] **Step 5:** `cargo xtask ci`; commit `NC9: Nextcloud in the app, CLI and MCP, with docs`.

### Task 10 (NC10): Manual verification against cloud.dev-null.me (controller, with the user)

- [ ] Release build with the `[patch]`; sign in through the real browser flow (the user approves) and the real keyring.
- [ ] Import `Canon R8/` then `Sony A7R VI/`: time to "every photo listed" and to "Reading metadata" done; grid scroll with no original downloads (cache dir stays empty); open one ARW in the Edit panel (download time), step to the next (prefetched).
- [ ] Control-channel screenshots (port 18000–19999) of the grid, the Develop view during a download, Settings ▸ Nextcloud; disconnect test (block the host) → offline UI.
- [ ] Record the numbers in `docs/nextcloud.md` → Measurements and in the final report.
- [ ] Not doable here (waits on upstream): once craft-nextcloud is merged in storytold/craft-libs, one commit bumps the `rev` and drops the `[patch]`; this spec and plan are dropped before the upstream LightCraft PR.
