# Localization parity

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-14 · **Change:** major (Arabic ships: full catalog, RTL display order, Arabic face via craft-fonts) · **Target:** Adobe Lightroom Classic 15.5 / Lightroom 9.6

How far each interface language is from `full`. How to add or maintain a language is in
[localization.md](localization.md); per-language notes are in `localization-<code>.md`
([zh-hans](localization-zh-hans.md), [zh-hant](localization-zh-hant.md), [ja](localization-ja.md),
[pt-br](localization-pt-br.md), [es](localization-es.md), [de](localization-de.md),
[ru](localization-ru.md), [ar](localization-ar.md)).

## The target

Lightroom 9.6 (installed on the owner's Mac) ships 16 interface languages, listed from its bundle's
`.lproj` folder names: English, German, Spanish, French, Italian, Japanese, Korean, Norwegian Bokmål,
Dutch, Polish, Portuguese (Brazil), Russian, Swedish, Thai, Simplified Chinese and Traditional
Chinese. It does **not** ship Hindi, Arabic, Indonesian or Vietnamese, so for those LightCraft would
go beyond the target.

## How the numbers are measured

**Measured** (a Python count over `crates/ui-egui/locales/*.json`, 2026-10-10):

- The denominator is every message any catalog knows: the union of the plain catalogs' keys (2,161
  English source strings: menus, commands, controls, panels, dialogs, tooltips, presets, What's New
  lines) plus the 249 messages of the formats catalogs (`*-formats.json`: text with runtime values and
  date patterns), 2,410 messages in all.
- A message counts as translated when the catalog has a non-empty value that differs from the English
  key. Some identical values are correct (`ISO`, `OK` in German), so the figure slightly understates
  coverage; on the other hand, strings that never reach a catalog (technical errors raised in lower
  crates, some `format!` text outside `tr_format!`) are not in the denominator at all. That is why no
  language is `full` even at 99%.
- The UI has 563 distinct `tr("…")` literals and 198 `tr_format!` messages in `crates/ui-egui` and
  `apps/`; command, control, menu and preset labels reach the catalogs through `display_label`.

Script support is checked against the code: `language_table!` in `crates/ui-egui/src/i18n.rs` lists
scripts `Latn`, `Cyrl`, `Hans`, `Hant`, `Jpan`, `Arab`. Arabic ships: catalogs store logical order and
`src/bidi.rs` puts them (and `tr_format!` results, and user text at paint sites) into display order
with `unicode-bidi`; joining comes from the shaper and glyphs from `Noto Sans Arabic` in craft-fonts.
The chrome itself is not mirrored (panels and the menu bar keep their left-to-right arrangement).
Devanagari (conjuncts, reordering) still cannot render correctly today.

Native-speaker review: no catalog has had a formal review. Several catalogs have edits from community
contributors whose commits only touch that language (ja, zh-hans, zh-hant, fr, de, ru, uk, pt-br);
that is promising but is not recorded as review, so the column says `no (community edits)`.

## The twelve key languages

| # | Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | To `full` |
|---|---|---|---:|---|---|---|---|---:|
| 1 | English | `en` | 2,410 / 2,410 (100%, source) | all; Help ▸ What's New and the shortcut sheet; no full user manual | Latin (Inter) | n/a | **full** | 0 h |
| 2 | Simplified Chinese | `zh-hans` | 2,223 / 2,410 (92%) | menus, panels, dialogs, presets, profiles, dates; lower-layer errors English | Hans via craft-fonts (Noto Sans CJK SC); IME through winit; no vertical text in UI (not in target either) | no (community edits) | partial | 6–10 h |
| 3 | Spanish | `es` | 2,209 / 2,410 (92%) | as zh-hans | Latin | no (community edits) | partial | 6–10 h |
| 4 | Hindi | `hi` | 0 (0%) | none | **Devanagari shaping missing** (egui has no complex shaping) | no | none (target: not shipped) | 50–90 h (shaping 25–50 h, shared with Arabic) |
| 5 | Arabic | `ar` | 2,398 / 2,410 (100%) | as zh-hans | Arab via craft-fonts (Noto Sans Arabic); display-order bidi; chrome not mirrored | no | partial | 0 h (catalog done; mirroring unscoped) |
| 6 | French | `fr` | 2,113 / 2,410 (88%) | as zh-hans | Latin | no (community edits) | partial | 5–9 h |
| 7 | Portuguese (Brazil) | `pt-br` | 2,106 / 2,410 (87%) | as zh-hans | Latin | no (community edits) | partial | 6–10 h |
| 8 | Indonesian | `id` | 0 (0%) | none | Latin (Inter covers it) | no | none (target: not shipped) | 10–16 h |
| 9 | Japanese | `ja` | 2,097 / 2,410 (87%) | as zh-hans; vertical export watermarks | Jpan via craft-fonts (BIZ UDPGothic / UDMincho); IME through winit | no (community edits) | partial | 6–10 h |
| 10 | German | `de` | 2,210 / 2,410 (92%) | as zh-hans; What's New translated line by line | Latin | no (community edits) | partial | 4–8 h |
| 11 | Korean | `ko` | 0 (0%) | none | **no Korean (Hangul) face**: needs a non-Adobe OFL face in craft-fonts (Noto CJK is allowed only for Chinese) | no | none | 14–22 h |
| 12 | Vietnamese | `vi` | 0 (0%) | none | Latin with stacked diacritics (Inter covers it; check combining marks) | no | none (target: not shipped) | 10–16 h |

## Other languages LightCraft ships (3)

| Language | Code | UI strings translated | Script | Native review | Status | To `full` |
|---|---|---:|---|---|---|---:|
| Traditional Chinese (Taiwan) | `zh-hant` | 2,095 / 2,410 (87%) | Hant; uses the Simplified Chinese face until craft-fonts has a TC face | no (community edits) | partial | 8–14 h (incl. the font) |
| Russian | `ru` | 2,373 / 2,410 (98%) | Cyrillic (Inter) | no (community edits) | partial | 8–12 h |
| Ukrainian | `uk` | 2,379 / 2,410 (99%) | Cyrillic (Inter) | no (community edits) | partial (lower-layer errors English) | 2–4 h |

Target languages LightCraft lacks: Italian, Korean, Dutch, Polish, Swedish, Norwegian Bokmål, Thai
(Thai needs shaping and line breaking without spaces). Each Latin-script one is ~10–16 h; Thai
~20–35 h on top of the shaping work.

## Shared work before any language is `full` (counted once, ~20–35 h)

- Route lower-layer error messages (catalog, raw, codecs, engine) through translatable message ids
  instead of English `format!` strings (~12–20 h). Translating them must not change what the app does:
  since #1046 the interface decides on an error's kind (`lightcraft_engine::ErrorKind`, a download's or
  Apple Photos import's `cancelled` flag), not its words; new decisions on an error go through a kind too.
- A check in `cargo xtask ci` that every user-visible literal reaches a catalog (~4–8 h).
- A user manual / help pages beyond What's New and the shortcut sheet (Lightroom links to online
  help; ~4–8 h for a translatable in-app help index).
- Native-speaker review passes (human; cannot be done by agents).

## Totals

- Bring the 10 shipped non-English catalogs to `full`: ~50–85 h (incl. the shared work).
- Add the 4 missing key languages: ~85–145 h, most of it Devanagari shaping in egui.
- Reach the target's 16 languages: + ~60–100 h (Italian, Dutch, Polish, Swedish, Norwegian, Thai).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-14 | major | Arabic shipped (`ar`): full 2,410-message catalog, RTL display order via `src/bidi.rs` + `unicode-bidi`, Noto Sans Arabic face via craft-fonts; union re-measured (2,304 → 2,410), all rows re-counted |
| 2026-10-11 | minor | Shared work: the interface decides on error kinds, not English error text (#1046) |
| 2026-10-10 | major | Created: per-language counts measured from the catalogs, the twelve key languages, script support, estimates |
