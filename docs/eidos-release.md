# Eidos release readiness (independent fork)

Eidos is an independent product from Nostos, forked from upstream LightCraft. It installs,
runs, configures, updates and uninstalls alongside LightCraft without sharing mutable state.
This checklist tracks what is done, what remains, and every deliberate LightCraft reference.

## Legacy-name occurrences and why they remain

All remaining `LightCraft` / `lightcraft` / `LIGHTCRAFT` / `ai.storyteller.lightcraft`
occurrences fall into exactly one of these classes (verified by repository-wide search):

1. **Legally required attribution** — never rename: `LICENSE-MIT` copyright line,
   `NOTICE` copyright + distributor note, `assets/app-icon/LICENSE.txt` provenance,
   `docs/brand/LICENSE-brand.txt` (third-party trademark terms, untouched),
   `contributors/contributors.json` (generated, never hand-edited).
2. **Internal technical identifiers kept for format interop** — read-only: the
   `lcpreset`/`lccurve` extensions and `lightcraft.preset` / `lightcraft.curvePresets`
   format tags (import accepts them; exports write `eidospreset` / `eidoscurve`);
   the `lc.` prefix in `preset_import.rs` FILLER (strips foreign bundle words);
   `.lca` recovery archives, `.lcsp` smart-preview cache (recreatable), `.lcp` in
   `xtask` FORBIDDEN_EXT (Adobe format). None of these identify the Eidos product.
3. **Third-party references** — `lclevy` researcher URLs (raw codec provenance),
   `aws-lc` (library name), Adobe namespaces (`crs:`, `adobe:ns:meta/`, …),
   `modelcontextprotocol.io`, vendor/model URLs, `craftrules` / `craft-fonts` /
   sibling-project paths in contributor docs.
4. **Appropriate upstream history** — `README.md` / `NOTICE` lineage notes ("based on
   LightCraft by the ArtCraft Team"), `docs/whats-new.md` 0.2.0 entry (those binaries
   were LightCraft), the orphaned `de.json` 0.2.0 catalog key, upstream PR/issue links
   in `docs/library-shortcuts.md`, the `assets/app-icon/README.md` upstream source path.
5. **Foreign-backup detection** — `apps/eidos-web` recognises the `LightCraft library
   backup` README marker for the sole purpose of refusing it with a clear error.

Everything else was renamed, including all crate/package/binary names (`eidos-*`,
`eidos`, `eidos-cli`), `EidosApp`, all `EIDOS_*` environment variables, the
`ai.nostos.eidos` application id, all config/cache/log paths, MCP `eidos://` URIs and
server name, web storage keys, the `eid:` XMP namespace, preset/curve formats and ids,
installer identities, and all user-facing strings.

## Official URLs (all TODO — none invented)

`crates/ui-egui/src/links.rs` centralises this: `EIDOS_*` constants (`WEBSITE, SOURCE,
DOCS, DOWNLOADS, ISSUES, COMMUNITY`) are `None`, guarded by a test that fails when one
is filled in without wiring it up. Each needs a file/setting update when published:

| Destination | Files / settings to update |
|---|---|
| Website | `links.rs` EIDOS_WEBSITE (+ About/menu labels), metainfo `homepage`, nfpm `homepage`, nix `meta.homepage`, wxs ARP URL props, README Downloads + header links |
| Repository | workspace `Cargo.toml` `repository`, README clone/quickstart/nix snippets, `links.rs` EIDOS_SOURCE (+ About/menu), metainfo `help`, README craft-apps row |
| Documentation | `links.rs` EIDOS_DOCS (+ Help menu label) |
| Downloads/releases | README Downloads section, `GITHUB_REPOSITORY` default in `packaging/linux/package.sh` (zsync channel), `docs/releasing.md` coordinates |
| Issue tracker | `links.rs` EIDOS_ISSUES (+ feedback label), metainfo `bugtracker` |
| Community/support | `links.rs` EIDOS_COMMUNITY, README community sections, in-app Discord references |
| App id domain | `ai.nostos.eidos` is provisional: bundle id, desktop/metainfo/flatpak ids, icon names, XMP namespace `http://ns.eidos.app/eid/1.0/`, `crates/ui-egui/src/links.rs`, `docs/eidos-release.md` (this file) |
| Vendor/legal entity | nfpm `vendor`, wxs `Manufacturer`, metainfo `developer`, `EIDOS_MAINTAINER` default contact |

Until then, upstream links stay only where they truthfully name upstream destinations.

## Branding assets and screenshots

- Done: DMG background title re-set to Eidos (Inter SemiBold outlines from the repo's own
  OFL font, same metrics/position; `background.tiff` + `dmg-layout.DS_Store` regenerated
  with `packaging/macos/dmg/generate.py`).
- Open (needs a designer, not a placeholder): a distinct Eidos app icon. The lynx artwork
  is original and attributed, and all icon files were renamed to Eidos names, but the mark
  itself is still the upstream one. To replace: new `assets/app-icon/eidos.svg`, then
  `packaging/icons.sh` regenerates every derived file (filenames/ATTRIBUTION already match).
- Screenshots (`docs/images/*.jpg`): verified to contain no product-name text and to depict
  views whose strings did not change — no recapture needed. Recapture via `docs/showcase/`
  if later UI text in those views changes.

## Side-by-side installation and data paths (verified by review + tests)

- Windows: new UpgradeCode + component GUIDs, `Program Files\Eidos`, `eidos.exe` /
  `eidos-cli.exe`, own Start Menu shortcut and App Paths key. MSI upgrade only ever
  replaces Eidos; uninstall removes only MSI-installed files.
- macOS: own bundle id, `Eidos.app`, own executable/icon; settings under
  `~/Library/Application Support/Eidos`.
- Linux: `eidos` deb/rpm (own upgrade path), own Flatpak id, own desktop/icon/metainfo
  names, `/usr/bin/eidos*`, `~/.config/eidos`.
- Data: default library `~/Pictures/Eidos Library` (no LightCraft fallback), default
  exports `~/Pictures/Eidos Exports`, own log files, own env vars, own MCP server name +
  `eidos://` URIs, own web storage keys + IndexedDB name, own XMP namespace, own preset
  formats/ids, own temp-file prefixes. Web restores refuse non-Eidos backups by README
  marker. Tests: `default_library_is_eidos_only`,
  `legacy_lightcraft_preset_files_still_import`, `backup_readme_identifies_the_writer`,
  lock-owner + server-name assertions.
- Residual risk (documented, not mitigated): deliberately opening the *same* library
  folder in both apps interleaves one catalog journal format both can write. Defaults
  make this unlikely; preventing it would require a divergent catalog format.

## Eidos upgrade and uninstall behavior

- Upgrades keep Eidos data: settings, libraries, presets, profiles, logs and caches all
  live under Eidos-owned paths/keys that installers never remove.
- Uninstall removes only installed files (Windows: binaries, shortcut, App Paths key;
  other platforms: package payload). Libraries, settings and backups are user data and
  are kept.
- Not yet executed here: real installer runs on Windows/macOS/Linux (CI covers
  `windows-arm64` install/uninstall and the FreeBSD dry-run; packaging-lint covers
  metadata syntax).

## Licensing and attribution questions

- None open in the implementation: MIT/Apache texts untouched, copyright preserved,
  ArtCraft marks unused by the product, third-party notices intact, no ownership claims.
- Still for Nostos to confirm: the `ai.nostos.eidos` id domain, the `ns.eidos.app`
  namespace domain, the `Nostos` vendor/developer display names, and the Eidos repository
  URL that provenance comments should eventually name.

## Separate technical follow-ups (not branding; do not mix in)

- JPEG debug-overflow: `2 * mw` in `crates/codecs/src/jpeg.rs` overflows `u32` in debug
  builds when decoding full-size JPEGs (`SourceLevel::Full → usize::MAX` edge), caught by
  the never-crash guard as a decode error. Release builds are unaffected (wrapping lands
  on the correct path). Needs saturating arithmetic plus a full-size-JPEG debug
  regression test.
- Flaky GPU equivalence tests: the two heaviest renders fail under full-suite parallel
  load on this machine but pass in isolation on both trees (known GPU fragility).
