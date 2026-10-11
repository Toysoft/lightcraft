Browser Copy/Paste Edit Settings can be delivered both as keys and logical clipboard events. This ports the existing *craft gesture deduplication, including split frames and focused-text isolation. Cut does not delete photos.

Browser checkpoints preserve the catalog and editing history, selected photos, source/filter/sort state, clipboard, mask/spot selections, preferences and views. Session-specific catalog journals protect separate tabs; original-photo storage and explicit library backup remain available. Recovery exports retain original-photo references rather than bundling originals.

No directly matching open clipboard/session report was found. Native GPU crashes, raw highlight recovery and MCP library locking are not addressed. The rebase retains upstream's library-path validation and numeric-field/docking tests.

Validation so far: nine standalone storage tests passed for this app at the root URL; the common harness extracting the actual rebased production clipboard routers/PNG decoder passed eight tests. All four rebased applications passed local WebAssembly checks. Native engine history round-trip tests are still running and are not yet claimed passed. No GitHub Actions build was requested.

This is a draft port of existing code, rebased onto upstream main as captured by the fork. Launcher, shared fonts, branding removals and hosting changes are excluded. See docs/browser-session-recovery.md for storage limits, migration and testing details. The JavaScript IndexedDB/lifecycle coordinator is an explicit architecture proposal for review against the Rust-only repository preference.

Pending before merge: repository-wide quality checks and real Chromium/Firefox/WebKit clipboard permission, menu/keyboard, text focus, stale destination, refresh/return, duplicated-tab and large-history review. Compilation and fake-DOM storage tests do not establish complete browser behavior. AI assistance was used to port and review the changes.
