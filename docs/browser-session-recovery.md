# Browser editing-session recovery and clipboard

This contribution ports the existing *craft browser fixes. Desktop recovery and native OS clipboard readers are unchanged. The browser checkpoint contains open documents/catalog, tab or photo selection, both history stacks and serializable view/preferences state. GPU resources and unfinished gestures are recreated or excluded.

Refresh restores the latest committed checkpoint. Returning later asks whether to restore. Start fresh keeps the old recovery slot. Separate tabs use independent writers protected by Web Locks. Without locks a fresh identity is allocated. IndexedDB commits checkpoint bytes and discovery metadata atomically and retains the preceding checkpoint. Quota errors preserve the last committed save and expose Retry. Recovery import/download and persistent-storage requests are user initiated. No document bytes are uploaded. Clearing browser data or eviction removes recovery. Pending writes can still be lost on hard termination.

Checkpoints are bounded to 512 MiB without truncating undo/redo. Full-history serialization may be expensive for large projects. PDF checkpoints can include passwords necessary to reopen encrypted files. LightCraft recovery contains original-photo references rather than bundling original photos; its explicit library backup remains available.

The browser coordinator and minimal storage controls use JavaScript for IndexedDB, lifecycle and locks. Engine serialization and clipboard ownership stay in Rust. This is an explicit architecture proposal for maintainer review against the project's Rust-only preference.

Clipboard reads are explicit paste actions. Browser image/text routes reject changed destinations and report unsupported APIs or denied permissions. PhotoCraft and VectorCraft publish PNG with alpha; VectorCraft also publishes editable SVG text. PDF pages and LightCraft edit settings retain their internal clipboard semantics. Focused text fields retain their own events.

Validation: `npm install --ignore-scripts && npm run test:sessions` in the web app directory checks storage with fake IndexedDB/DOM. `cargo test -p lightcraft-engine --test craft_session` exercises native history round trips. `cargo check --target wasm32-unknown-unknown -p lightcraft-web` checks the actual browser host. Real Chromium, Firefox and WebKit clipboard permissions, paired/split-frame gestures, text focus, stale asynchronous destinations, refresh/return, tab duplication, quota failures and large-history performance require separate browser review.
