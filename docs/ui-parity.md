# UI parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-11 · **Change:** minor (mask workspace layout and component targeting) · **Target:** Adobe Lightroom Classic 15.5 / Lightroom 9.6

How LightCraft's interaction compares with Lightroom's: views, panels, tools and handles, modifier
keys, shortcuts, zoom, context menus and feel. Rows for individual menu items and shortcuts live in
[parity-checklist.md](parity-checklist.md) (sections Y and Z, Classic keys); this page is the
assessment and the notes that don't fit a row. Visual reference for agents: `plan/lightroom/`
(local, gitignored; `10-observed-ui.md` and screenshots).

## Measured counts (2026-10-10)

- 226 engine commands (`crates/engine/src/cmd/`, 30 modules) and 143 UI commands (`UI_COMMANDS` in
  `crates/ui-egui/src/menus.rs`, 73 with a shortcut) plus 10 language commands; 23 secondary
  shortcut bindings (`ALIASES`, `crates/ui-egui/src/shortcuts.rs`); a keymap editor.
- 122 develop controls plus 10 indexed templates (`crates/develop/src/controls.rs`).
- Checklist: menus 95.3% (86 in-scope rows), desktop shortcuts 94.4% (81 rows), Classic extras
  48.2% (82 rows), measured by recounting [parity-checklist.md](parity-checklist.md).
- 31 panel modules (`crates/ui-egui/src/panels/`), 26 context-menu sites.

## Assessment by area

| Area | Target | LightCraft | Ready | Main gaps |
|---|---|---|---:|---|
| Layout and views | Lightroom: grid / detail / edit; Classic: Library, Develop, Map, Book, Slideshow, Print, Web modules, filmstrip, panels | Lightroom-style single window: Photo Grid, Square Grid, Detail, Compare, Survey (≤ 48), Before / After, People, quick slideshow, second window, filmstrip | 75% | no Classic module switcher (by design) or output modules; dark theme only |
| Develop panels and sliders | Light, Color, Effects, Detail, Optics, Geometry, Lens Blur, Calibration | every global slider; scrub, double-click reset, Alt-drag previews where implemented | 80% | Lens Blur, Calibration panel depth; slider feel (acceleration, fine Shift-drag) unmeasured |
| Crop, Upright, transform handles | handles, aspect lock, angle, overlays (O / ⇧O), guided Upright | ✅ all of them; overlay cycle on ⇧O | 85% | crop opacity of cut-out area (15.5) |
| Masking tools | brush (A / B, flow, density, auto mask, pressure), linear / radial, range masks, AI masks, pins | brush, linear, radial, colour / luminance range, add / subtract / intersect / invert, masks panel, pins | 60% | brush live preview (#517 / #518), no pen pressure, no depth range, AI masks, Feather / Edge controls (15.5) |
| Remove / heal | content-aware remove, heal, clone, visualize spots | heal, clone, auto source, visualize spots, red / pet eye | 65% | content-aware (patch synthesis) remove, generative remove, detect distractions |
| Zoom and navigation | Fit / Fill / 1:1 / 2:1 … 11:1, Navigator, spacebar pan | Fit to 800%, pinch zoom, pan, Navigator-like | 60% | **original-pixel detail at 100% (preview capped at 2,560 px; P0 gap LR-VIEW-ZOOM)** |
| Library culling and metadata | ratings, flags, labels with keys, painter, keywording, metadata panel, filter bar | ratings / flags / labels and advance, keyword list and sets, keywording box, metadata fields, smart albums, filter bar | 85% | painter tool, quick collection, some Classic keys |
| Shortcuts | Lightroom desktop and Classic keymaps | Lightroom-desktop keymap with Classic additions, editable | 85% | deliberate differences below; no Classic keymap layer |
| Context menus | everywhere | 26 sites (grid, filmstrip, albums, folders, keywords, masks, presets…) | 70% | unaudited against Lightroom's menus item by item |
| Text fields | native editing behaviour | custom text field (LR-BEHAV-TEXTFIELD) | 70% | IME and selection edge cases |
| Accessibility | VoiceOver / Narrator, keyboard navigation | AccessKit: sliders, buttons, dropdowns, section headers announce | 30% | canvas tools pointer-only; not audited with a screen reader |
| Feel and responsiveness | GPU-interactive sliders (15.3) | ≤ 16 ms slider budget on GPU; background renders | 75% | colour NR at half resolution, 100k-photo grid test |
| **UI/UX overall** | | | **~70%** | estimated, weighted by use (develop and library 60%, tools 25%, rest 15%) |

Remaining: **40–70 h** (zoom detail 8–15 h, brush and masking feel 8–12 h, accessibility audit 10–20
h, Classic keymap layer 4–8 h, context-menu audit 4–8 h, text-field and IME polish 4–8 h). Output
modules and AI tools are counted with their features in [target-app-parity.md](target-app-parity.md).

## Masking workspace (#450)

The mask/component list is embedded above the right-side settings by default. Its panel icon
floats it beside those settings; drag the title to move it, resize its corner, and use the panel
icon or **Dock to panel** to embed it again. Placement and overlay preferences are saved. With no
masks, creation choices remain directly visible in the right panel; subsequent creation uses +.

Selecting a component highlights its row and shows its tool controls on the right. Backspace/Delete
removes only that component; selecting the parent mask row makes deletion remove the whole mask.
Deleting the last component removes its empty mask, and each deletion can be undone. Component
menus also offer Move Up/Down, disabled at the list boundaries. Moving changes the actual
composition order while preserving the component’s contents and selection, including undo/redo. Brush strokes,
Object clicks and colour sampling target that component; brush Size, Feather, Flow, Density,
Add/Erase and Auto Mask apply to subsequent strokes. Add/Subtract occupy equal halves of the
selected mask’s action row, with larger hit areas and spacing above. Component settings lead
into the local adjustment sliders under a small **Mask Adjustments** heading, without a
duplicate mask-name/invert row. Add, Subtract and Intersect with offer the same ten types as
new-mask creation, with localized labels. Both mask and component right-click/ellipsis menus
offer Intersect with and Subtract; choosing a type targets the menu’s mask, including an
unselected mask, and selects its new component. Opening the menu preserves selection and edits.
An intersection brush keeps only painted overlap;
Erase removes paint from that brush component. Color Range samples the selected component
when clicking the photo in the Masking panel; Shift-click adds a sample. Subtract and Intersect
indicators use separate rounded-square badges before the component name, with descriptive hover text.
Renaming a mask or component replaces only its name with an input. Mask thumbnails and
visibility/menu buttons, component icons and operation badges, and row heights stay in place.
The list footer contains the overlay checkbox and colour/options popups (custom colour, opacity,
visualization mode and pins). Mode choices and the custom colour picker are child menus, so
opening or using them preserves the settings popup; Escape or an outside click dismisses it.
These display/layout preferences do not add photo undo steps.

Mask rows show the actual composite coverage as white on black, with grey feathered edges,
fitted to the edited photo’s aspect ratio. Background renders share the canvas mask pipeline
(add/subtract/intersect, inversion, range and AI masks), use a separate thumbnail cache, and
refresh after edits and undo/redo. Hidden masks retain their range preview, independent of
overlay colour/opacity. Component rows keep their original type icons. Six-dot grips to the right
of each ellipsis reorder masks or components within their mask, with an insertion line and edge
scrolling. A drop is one undo step and preserves selection/tool targeting; Escape, outside drops
and stale drags do not reorder. A/B brushes, pressure, live preview and
broader gesture parity remain open.

## Shortcuts

Compared `plan/lightroom/06-shortcuts.md` (desktop part) with our bindings: command specs in `crates/engine/src/cmd/`,
`UI_COMMANDS` in `crates/ui-egui/src/menus.rs` and secondary bindings (`ALIASES`) in `crates/ui-egui/src/shortcuts.rs`.
`no_conflicting_bindings` (same file) fails when one key fires two actions.

**Fixed (M16.1):** added Lightroom-desktop keys as secondary bindings for existing commands — ⌘D Select None, ⇧E
Export dialog, Space Toggle zoom, ⇧M Create Version, ⇧X Reject + advance, ⇧U Unflag + advance. `⇧Y` fired both
Before/After Split and the History panel; History lost the binding. `W` and `⇧⌘I` also fired the engine command
under the UI command that wraps it (a no-op error); the UI command now wins.

**Deliberate differences (our key → Lightroom desktop key)** — each is a conflict with another binding we have:

| Action | Ours | Lightroom desktop | Why |
|---|---|---|---|
| Pick flag | P | Z | Z = toggle zoom (Classic convention); P is the Classic pick key |
| Photos panel (left) | ⌘⇧L | P | P = pick |
| Expand/collapse edit sections | ⌘⌥1–5 | ⌘1–6 | ⌘0 = Zoom to Fit, ⌘1 = Zoom 100 %; Geometry lives in the Crop panel |
| Histogram | ⌘⇧H | ⌘0 | ⌘0 = Zoom to Fit |
| Square Grid | ⇧G | (none; ⇧G = Guided Upright) | Guided Upright is a button in the Crop panel |
| Export dialog | ⌘⇧E (+ ⇧E) | ⇧E | ⌘⇧E is "Edit in Photoshop" there; no external-editor command yet |
| Crop overlay cycle | ⇧O | O | O = mask overlay; ⇧O (mask colour / overlay orientation) unused otherwise |
| Create Version | ⌘⇧S (+ ⇧M) | ⇧M (Windows: Ctrl+⇧S) | — |
| Select None | ⌘⇧A (+ ⌘D) | ⌘D | — |

**Library culling (KEYC-RATING):** `0–5`, `6–9` and `P/X/U` reach egui on macOS even when displayed in the native menu (adapted from PR #261; issue #283). `Shift+0–9` applies and advances once; `Shift+P` picks and advances in Photo Grid / Square Grid and retains Presets elsewhere. See [library shortcuts](library-shortcuts.md) for the verified Classic mapping and regression coverage. Remaining Classic gaps: rating `[` / `]`, flag cycling and filter-bar keys.

**Still missing / broken:**
- No command yet: F1 help (verify the rest of the old list: full screen, settings, stacks,
  visualize spots and merges have commands now).
- `H` opens Remove; Lightroom also uses it (Classic) to hide pins — pins toggle from View → Show Mask Pins.
- ⌘M / ⌘H / ⌘Q / ⌘W rely on the platform window defaults (unverified).


## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | minor | #450: parent mask menus expose Intersect with/Subtract and target the clicked mask |
| 2026-10-11 | minor | #450: mask/component drag handles, insertion feedback, edge scrolling and one-step undo |
| 2026-10-11 | minor | #450: fix overlay mode/custom-colour submenus closing the settings popup; verify both launchers and layouts |
| 2026-10-11 | minor | #450: embedded/floating list, selected-component settings, overlay footer and remaining masking gaps |
| 2026-10-10 | major | Created: measured command and shortcut counts, assessment by area, estimates; moved the shortcut conflicts section here from the checklist |
