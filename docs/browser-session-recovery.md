# LightCraft browser recovery

Status: Rust implementation under review. No parity score increase is claimed.

Recovery uses Rust futures and web-sys IndexedDB APIs. Controls use egui theme visuals.
There is no handwritten JavaScript, inline JavaScript, npm dependency or DOM stylesheet.
Storage loading never blocks the editor. Every load uses a fresh random writer id so
reloads and duplicated tabs cannot overwrite another page's checkpoint. A reload reads
its previous committed id and a later visit offers the latest checkpoint. Snapshot,
previous checkpoint and discovery pointer commit atomically. Failed saves show Retry.
Restore/import require closing current documents in PhotoCraft and VectorCraft.
LightCraft replaces its loaded catalog on explicit restore. Its original photos remain
in existing content-addressed storage and recovery copies contain references to them.

Checkpoints preserve serializable workspace state and undo/redo history. Capture waits
for inactivity and pointer release. They are bounded to 512 MiB without history truncation.
Hard termination may lose changes since the last completed write. Storage eviction and
clearing site data remove recovery. Full-history capture can be expensive.
The legacy draft's JavaScript database is retained without migration or deletion.
Recovery import/export and persistent-storage requests are explicit user actions.
No fonts, images, external assets, brand changes or launcher code are included.

Validation and outstanding browser/platform checks are recorded in the PR description.
Revision history: 2026-10-12 — replace the noncompliant JavaScript coordinator with Rust.

Recovery operations are exposed through the existing engine command registry as
`session.recovery.capture {ui?}`, `session.recovery.inspect {bytes}` and `session.recovery.restore {bytes}`.
They are available to existing CLI/control/MCP dispatchers. JSON byte-array transport
is capped at 8 MiB. Browser storage uses the same engine serializer through the binary
adapter with a 512 MiB bound. Command restore requires an empty engine session and
returns UI metadata for the calling shell to apply.

## Contribution review and validation

Reviewed the published AGENTS.md rules for Rust-only product code, checked inputs,
command dispatch, layering, clean-room provenance, asset ownership and fonts.
Formatting, WebAssembly compilation, three native engine integration regressions and
all 23 native web-host tests passed. Layering and asset checks passed. Scoped native
Clippy passed for engine/UI/web with --no-deps --all-targets -- -D warnings. The
contribution's test-module ordering and unused helper issues were corrected without
suppressing lints. Contributor credit remains @mmmbeer as requested. No new fonts,
external assets, branding changes, handwritten JavaScript, CSS coordinator or npm test
dependency are included.

Full upstream quality gates remain unverified. Native Clippy with dependencies fails
on unchanged pipeline/src/spots.rs:250 (Rust 1.99 chunks_exact_to_as_chunks lint).
Strict wasm Clippy fails on unchanged codecs/src/jxl_encode.rs imports and preview/src/pool.rs
DROP_WAIT; other existing wasm warnings are in engine/src/devices.rs and UI Lightroom
import code. These files were not changed by this contribution. The full workspace
cargo xtask ci and parity generation were not certified locally. The Rust IndexedDB
probe was compiled but not executed in a real browser. Cross-browser clipboard/storage,
visual UI and large-history performance QA remain required. Private craftrules was
inaccessible (404), so unpublished standards need maintainer review.
