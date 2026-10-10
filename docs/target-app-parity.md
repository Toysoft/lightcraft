# Target-app parity: LightCraft vs Adobe Lightroom

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (full re-measure against Lightroom Classic 15.5 and the installed Lightroom 9.6; merged ROADMAP's parity estimate and honest assessment into this file) · **Target:** Adobe Lightroom Classic 15.5 (August 2026) and Lightroom 9.6 (desktop)

The authoritative assessment of how close LightCraft is to Lightroom. The row-by-row checklist is
[parity-checklist.md](parity-checklist.md); the ranked work list is [gaps.md](gaps.md); deep areas
have their own documents: [raw-parity.md](raw-parity.md), [file-format-parity.md](file-format-parity.md),
[ui-parity.md](ui-parity.md), [hardware-parity.md](hardware-parity.md),
[localization-parity.md](localization-parity.md).

## Headline

| Number | Value | Kind |
|---|---:|---|
| **Feature breadth**, checklist | **81.1%** of 517 in-scope rows (P0 98.0%, P1 95.1%, P2 46.9%) | **measured**: recount of [parity-checklist.md](parity-checklist.md) rows (✅ 1, 🟡 ½, 🚫 left out), the same rule `cargo xtask parity` uses |
| **Feature breadth**, weighted by use | **~76%** | estimated: feature-area table below |
| **Ready for real work** | **~55%** (range 50–60%) | estimated: feature areas ~57% weighted, pulled down by stability, platforms, ecosystem |
| Remaining to **beta** | **~300–520 h** | estimated; ~60% parallelizes |
| Remaining to **full parity** | **~1,000–1,800 h** | estimated; ~70% parallelizes |
| **Stage** | **alpha** | see [Stage](#stage) |

## The target and how it was measured

- **Target:** Lightroom Classic 15.5 (August 2026; latest per Adobe's release notes) for the
  library, modules and depth; Lightroom 9.6 (the cloud-library desktop app), installed on the
  owner's Mac at `/Applications/Adobe Lightroom CC`, for the interface LightCraft's layout follows.
  Both share the Camera Raw engine.
- **Installed bundle, inspected by listing only** (the clean-room rule forbids reading contents):
  `Info.plist` document types (31 raw extensions, jpg, tif, heic, xmp, catalog bundles); 16
  `.lproj` localizations; `CameraProfiles` (1,500 Adobe Standard and 395 Camera Matching model
  names); `LensProfiles` (3,635 files, 56 makers); `ModelZoo` (37 AI model names). The app was not
  launched.
- **LightCraft side:** origin/main at `b70ae174` (2026-10-10): code counts (226 engine commands, 143
  UI commands, 122 + 10 develop controls, 2,561 tests, ≈ 202,000 Rust lines in 24 crates + 3 apps),
  the checklist rows, raw corpus manifests, locale catalogs, CI workflows, docs.
- **Behaviour and output** were not compared against a running Lightroom in this pass; the
  measured ΔE figures in [raw-parity.md](raw-parity.md) come from earlier work.

**Presence is not parity.** ✅ rows are set by whoever lands a feature and have not been checked
systematically against Lightroom's behaviour; bugs keep turning up in ✅ areas (CR2 colour-filter
phase #85, duplicate Local entries #22, black GPU exports on an Intel iGPU #78).

## By dimension

| Dimension | Breadth | Ready for real work | Remaining | Doc |
|---|---:|---:|---:|---|
| Features (weighted areas below) | ~76% (checklist 81.1%, measured) | ~57% | 600–1,100 h | [Features](#features) |
| ↳ RAW cameras, colour and lens profiles (part of features) | ~72% | ~50% | 240–450 h | [raw-parity.md](raw-parity.md) |
| UI/UX fidelity | ~85% | ~70% | 40–70 h | [ui-parity.md](ui-parity.md) |
| File formats (non-raw: images, sidecars, presets, catalogs) | ~85% | ~70% | 30–55 h | [file-format-parity.md](file-format-parity.md) |
| Hardware | ~50% | ~40% | 40–80 h (+ tethering and printing, counted under Classic modules) | [hardware-parity.md](hardware-parity.md) |
| Localization (12 key languages) | 7 of 12 present | ~52% (mean of the 12, measured) | 130–210 h for the target's set; +145–255 h for Hindi, Arabic, Indonesian, Vietnamese, Korean | [localization-parity.md](localization-parity.md) |
| Performance | ~80% | ~70% | 30–50 h | 24 MP GPU slider 3.9 ms, export 0.3 s (measured, [gpu-pipeline.md](gpu-pipeline.md)); 85k library opens in 1.7–4.7 s; 100k test pending |
| Stability | — | ~60% | 30–60 h | never-crash lints and guards workspace-wide, crash-tested journal, 2,561 tests; but CI runs tests only on FreeBSD, user reports keep finding bugs in ✅ areas |
| Platforms | macOS, Windows (x64 / x86 / ARM64), Linux (AppImage, deb, rpm, Flatpak, Nix), FreeBSD, web | ~60% | 25–45 h | macOS is where it is tested; Windows installer UI unverified on Windows (#79); web experimental (CPU only) |
| Ecosystem / plug-ins | ~25% | ~15% | 40–80 h + owner decision | no Lightroom SDK equivalent (export, publish, metadata plug-ins); beyond the target: JSON-lines control channel, MCP server with 28 tools + one per command, CLI |
| AI features | ~30% | ~20% | 120–220 h + model decisions | [AI](#ai) (overlaps the masking and Enhance rows; not added twice) |

## Features

Weights are a judgement of what Lightroom users spend their time on (library and develop dominate;
Classic output modules and video are used by few). Breadth and readiness per area are estimated
from the checklist sections and the deep-dive documents.

| Area | Weight | Breadth | Ready | Remaining | Notes |
|---|---:|---:|---:|---:|---|
| Import and library (import, folders, albums, smart albums, keywords, metadata, search, faces) | 15% | 90% | 80% | 25–45 h | checklist A–E 83–92%; robust journal; 85k-photo libraries |
| RAW decoding and camera coverage | 15% | 72% | 50% | 120–220 h | [raw-parity.md](raw-parity.md): CR3 variants, compressed ORF, HE NEF |
| Colour, camera profiles, render fidelity | 10% | 65% | 40% | 80–150 h | estimated colour, no fidelity suite |
| Global develop adjustments (light, colour, curves, grading, effects, detail) | 15% | 93% | 70% | 30–60 h | checklist F 91.7%; character tuned by eye |
| Optics, lens profiles, crop and geometry | 6% | 80% | 50% | 40–80 h | no lens database; crop / Upright complete |
| Masking | 8% | 67% | 45% | 60–110 h | checklist K 67.4%; AI masks heuristic or SAM 3 without mirror |
| Remove, heal, Enhance (denoise, Raw Details, Super Resolution, Lens Blur) | 7% | 45% | 30% | 70–130 h | checklist I 75%, P 0% |
| Presets, profiles, versions, history, sync | 5% | 95% | 85% | 8–15 h | |
| Export | 6% | 89% | 75% | 15–30 h | checklist S 88.9% |
| Merge and HDR | 3% | 85% | 60% | 20–35 h | merge 100%, HDR 70% (no HDR display) |
| Views, compare, zoom, culling | 5% | 90% | 70% | 15–30 h | zoom detail P0 gap |
| Classic output modules (Map, Book, Slideshow, Print, Web, publish, tethering) | 4% | 15% | 10% | 90–150 h | Classic extras 48% counts many small keys; the modules themselves are mostly ⬜ |
| Video | 1% | 0% | 0% | 25–45 h | catalogued only |
| **Weighted** | 100% | **~76%** | **~57%** | **600–1,100 h** | |

## AI

Lightroom 9.6 ships 37 on-device models (names listed from its `ModelZoo`), covering subject, sky,
portrait, skin and object selection with refinement, depth estimation, denoise, dust detection,
reflections removal, distraction / people detection, culling scores (eyes open, blur, aesthetics) and
search embeddings; generative remove runs in Adobe's cloud.

| Feature | LightCraft | Status |
|---|---|---|
| Subject / Sky / Background masks | classical heuristics; the file's own DNG semantic mattes | 🟡 |
| People (faces, body, hair, skin…) and Landscape masks | — | ⬜ |
| Object / Describe masks | SAM 3 in pure Rust (Metal / CPU); opt-in 3.4 GB download, no mirror configured | 🟡 |
| Depth range mask, Lens Blur | — | ⬜ |
| AI Denoise | Bayer only, pure-Rust CPU / GPU, user-installed weights | 🟡 |
| Raw Details, Super Resolution | — | ⬜ |
| Generative remove, distractions, reflections | — (generative is out of scope: cloud) | ⬜ |
| Dust detection | ✅ | ✅ |
| Faces / People | YuNet detection, SFace / AuraFace recognition (opt-in downloads), People view | ✅ (MVP, [faces.md](faces.md)) |
| Assisted culling | focus, bursts | 🟡 (no eyes-open / aesthetics scores) |
| Auto (tone), adaptive presets | Auto calibrated on Lightroom's Auto results (#628); no adaptive presets | 🟡 |
| Natural-language search | — | ⬜ |

**Blocked on a model strategy** (maintainer decision): licensable weights or our own training;
pure-Rust inference is proven (faces, SAM 3, denoise).

## Stage

**Alpha.** Core workflows exist end to end (import, cull, develop, mask, export, catalog migration),
but ready-for-real-work is ~55%, below the ~75% beta bar, and the main file formats have blocking
gaps: Canon CR3 decodes from sensor data on three verified bodies only, compressed ORF and HE NEF
open as previews, and colour on every non-DNG raw is estimated rather than measured.

**Alpha gate:** all six core workflows (import, cull, develop, save and reopen, batch looks, export)
work end to end on macOS; see the table in [roadmap.md](roadmap.md#alpha-gate). The partial rows (CR3
variants, compressed ORF, HE NEF as previews; capped 100% zoom) degrade but don't block a workflow.

**To beta:** ~20 points of readiness, **~300–520 h**: the ten `B` gaps in [gaps.md](gaps.md)
(~250–460 h) plus CI on the main platforms and stability work around them. Beta needs: CR3, ORF and
HE NEF decoding from sensor data; measured colour for the popular models and a fidelity suite with
scores; full-resolution zoom; Subject / Sky / People masks; a lens profile database for the
popular lenses; tests in CI on macOS, Windows and Linux.

## How hours are calibrated

Hours are Opus 5.5 agent wall-clock hours, one session working sequentially, including tests and
verification. Calibrated on this repository's history (git log and GitHub PR open-to-merge times):

- **Project pace:** first commit 2026-09-30; by 2026-10-10, 1,066 non-merge commits and 325 merged
  PRs, commits in 159 distinct clock hours, with 4–6 parallel agents plus community contributors.
  On 2026-10-02 the ROADMAP recorded ≈ 105–145 agent-hours spent; M0–M13 took ≈ 25 active hours with
  parallel agents against an up-front estimate of 110–170 h.
- **UI and library rows:** a lead agent closed ≈ 45 checklist rows in ≈ 5 h (2026-10-01) and ≈ 60 in
  ≈ 12 h (2026-10-02): 6–12 minutes per small row.
- **Raw decoders:** RW2 in every format 6.5 h (#215), compressed RAF 8.6 h (#282), CR3 CRX 10 h
  (#279), NEF lossy after split 4.7 h (#633), ORF 12.8 bpp 1.7 h (#651), HEIC 7 h (#609).
- **Large features:** AI RAW denoise 8.3 h (#384, 12.8k lines), SAM 3 masks 14.9 h (#162),
  HDR editing and export 2.3 h (#646, on existing groundwork), contact-sheet PDF (#563), German UI
  13.8 h (#265).
- PR open time includes queueing and review, so these are upper bounds per feature; the ranges add
  a long-tail factor of 1.3–1.5× for verification against real files.

### Why the estimate went up

The previous estimate (2026-10-02, kept below) was ≈ 270–505 h to full parity. It counted checklist
rows only. New evidence since: the target's own lists (≈ 1,000 dedicated camera models, 3,635 lens
profiles, 37 AI models, 16 languages) measured from the installed bundle; per-maker verification
counts; that CI tests only FreeBSD; and the standard's added dimensions (localization, stability,
platforms, ecosystem). The raw-codec package it estimated at 40–80 h took roughly that and closed
most of its list; what remains is the data-heavy part (colour, lenses, models) it did not count.

## History of the numbers

| Date | Checklist (all / P0 / P1 / P2) | Ready for real work | Remaining to full parity | Source |
|---|---|---|---|---|
| 2026-10-10 | 81.1% / 98.0% / 95.1% / 46.9% (517 rows) | ~55% | 1,000–1,800 h | this re-measure |
| 2026-10-08 | 79.7% / 98.2% / 95.6% / 41.6% (509 rows) | 60–70% | — | ROADMAP *Where we stand* |
| 2026-10-02 | 75.6% | — | 270–505 h (cloud parity without AI / Classic: 130–255 h) | ROADMAP *Parity estimate* |

The 2026-10-08 assessment by dimension, kept for evidence: feature checklist 79.7%; RAW coverage
~55%; colour and image quality ~55–65%; AI and computational ~15–20%; workflow and library ~85%
(single machine); Classic modules ~30%; HDR and video ~25%; platform and robustness ~70%. By kind of
user: JPEG / DNG shooter ~85%; Nikon / Sony / Panasonic / older-Canon ~65%; Fujifilm ~65%; Canon
CR3 / Olympus compressed ~45%; Classic power user ~45%; relies on AI ~25%. Today's numbers per user:
JPEG / DNG ~75%; Sony, Panasonic, Fujifilm ~60%; Nikon ~55% (HE NEF); Canon CR3 ~40%; OM / Olympus
~30%; Classic power user ~40%; relies on AI ~20%.

The 2026-10-02 effort table, kept for calibration: remaining P0/P1 UI 5–10 h; raw codecs 40–80 h;
lens database 15–30 h; video 20–40 h; AI 80–150 h; HDR 15–25 h; Classic modules 60–100 h; smaller
P2 items 12–25 h; look tuning, performance, packaging, hardening 25–45 h.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | Stage re-checked against the core-workflow alpha gate: passes, stays alpha |
| 2026-10-10 | major | Created per the progress-docs standard: full re-measure (checklist recount, installed Lightroom 9.6 bundle listing, code counts, catalogs, CI), feature areas with weights, dimensions, AI, stage, calibration; merged ROADMAP's *Where we stand* and *Parity estimate* |
