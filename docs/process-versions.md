# Process versions

Every photo's develop settings record the **rendering process** they are interpreted by
(`DevelopSettings::process`, a `ProcessVersion` in `crates/develop/src/settings.rs`). Lightroom calls
this the process version. It lets the look of LightCraft improve (a new default tone curve, issue #146;
stronger Light sliders, issue #196) without changing a single photo someone has already edited.

| Process | What it changes |
|---|---|
| **V1** | LightCraft's rendering from before process versions existed. Frozen. |
| **V2** | Highlights and Shadows keep fine detail (issue #632): their base layer (`local::tone_base`) is the edge-aware filter of a separable median of log luminance (radius 0.15 % of the long edge, at most 24 px, `local::BASE_DETAIL`) instead of log luminance itself, so twigs, lines and texture take their surroundings' shift and keep their contrast; region edges stay sharp. CPU (`local::prepare`) and GPU prepare branch on `local::tone_base_detail` (0 = no median under V1); the base plane is cached per process (`local::BaseKey`). Renders without Highlights or Shadows (global or local) are the same as V1's. Tone map unchanged. |
| **V3** (the latest) | **Colour noise reduction** removes colour noise at its default (25) and keeps colour detail (`local::nr_params`, `local::nr_color_guided`, `chroma_noise`; GPU `nrx_*` kernels): chromaticity goes through a guided filter steered by the cube root of luminance (`local::nr_guide`), which smooths flat colour and keeps colour edges that are luminance edges, at full strength from 25 (V1/V2 mixed in `amount × (1 − detail/2)` of a plain blur: a fifth at the default), with the pixel's luminance restored exactly. What the filter takes away is put back where it is colour detail rather than noise: where its local energy (a 1.5 px Gaussian of `|c − q|²`) is above `keep` times the render's colour noise at that brightness (`local::keep_weight`, `1 − bar/energy`). The noise is measured per render on 16 patches of the whole frame at the output scale (the median local energy per band of the guide, never rising with brightness), so a zoomed window and an export measure the same; drafts of a white balance or noise reduction drag reuse the last measurement of their framing. Without it the filter greyed low-ISO hair and fabric. Amount and Smoothness widen the filter (σ = (4 + 16·amount)·(0.5 + smoothness) px at full size), Detail tightens its edge test and lowers the bar (`NR_COLOR_EPS` and `NR_COLOR_KEEP` ×4 at 0, ÷4 at 100), Amount above 25 raises the bar. **Sharpening and Masking** (`sharpen`, `local::sharpen_band_plane`, `local::sharpen_edge_plane`, `finish::SharpenParams`; GPU `sqrt_lum_k`, `conv_h` / `conv_v`, `axpy_k`, `grad_k`, `sharpen_view_k`, `sharpen_detail_k`, `finish.wgsl`). V1/V2 sharpened the Texture band (a Gaussian of 0.0018 × the long edge: ~17 px on a 61 MP frame), so it acted as local contrast and Radius and Detail did nothing. **The model:** all three channels scaled by one factor (luminance only); the change is a filter of the square root of display luminance whose lift of each detail size is `exp(c·(G_s1 − G_s2))`: an unsharp mask's at the default (2.7× at the finest detail), rising ever more steeply with Amount (about 10× at 100, 23× at 150) and Detail (5.5× at 100), Radius moving it to coarser detail (the finest lifted only 1.3–1.5× at Radius 3); no halo limit but a soft cap on the change of √luminance (`L·tanh(Δ/L)`, 0.017 at Detail 0 to 0.069 at 100, growing with Amount and Radius); deep shadows left alone (full strength from 4 stops below white, none at 9, straight in √luminance); Masking keeps `E² / (E² + t²)` of a pixel's sharpening, `E` the gradient of √luminance after a 1.5 px Gaussian, `t` 0.0029 at Masking 25 rising ×e^(0.0385·(Masking − 25)). **How it renders:** the filter is designed per Amount, Radius, Detail and output scale (Radius is in the photo's own pixels: `SourceInfo::native_long` scales it for previews rendered from a smaller level; the kernel of the response by inverse DFT, cut to the smallest reach within 1.5 % of it, split into at most four separable passes by its eigenvectors, a centre term keeping flat areas exact), run on √ of scene luminance before exposure (planes survive an exposure drag), and turned into a change of display √luminance by the tone curve's slope after the tone map; local Sharpness scales the band. **Previews** (Option-drag, from every process): Masking shows Masking's weight at the photo's tones, Radius / Detail the change of display √luminance the render makes before Masking, mid-grey for none and white or black at the whole cap (`finish::sharpen_detail_at`), Amount the photo in grayscale. Renders without colour noise reduction or sharpening are the same as V2's. |

## The rule

**A change that alters how existing settings render ships as a new process. V1 is frozen.**

- A deliberate change of look (tone model, slider response, default curve, how a stage interprets a
  value) goes into a new process. Photos stay on the process they have until someone runs Update to
  Current Process (or Reset).
- Bug fixes that make output correct where it was broken (a crash, NaN or black output, a decoder
  error, CPU and GPU disagreeing) are not looks and can land in place, as before.
- Changes to how a file is decoded or which camera colour it gets change the source, not how settings
  are interpreted; the process doesn't reach that stage today.

## Adding a process

1. Add a variant to `Process` (`V2`), append it to `Process::ALL` (oldest first), give it its number in
   `Process::version` and make it `Process::LATEST`.
2. The compiler then points at every `match` on `Process`. The base tone map is chosen in one place,
   `lightcraft_pipeline::finish::base_tone` (both the CPU and the GPU renderer take it from there): give
   V2 its own arm and leave V1's alone. A process that changes another stage branches there the same
   way, on `s.process.process()`; if that stage is cached (`Plan::lin_key`, the planes in `local.rs`),
   add the process to its key.
3. Keep `settings_saved_before_process_versions_load_as_v1` (`crates/develop/src/tests_process.rs`)
   passing: settings saved without the field must stay V1, not become the new latest. That is the
   field's `#[serde(default = "ProcessVersion::legacy")]`; the struct-level `Default` gives new
   settings the latest instead.
4. Render a few procedural scenes on both processes and compare, and update `docs/parity-checklist.md`.

## What gets which process

| | Process |
|---|---|
| A newly imported, merged or Lightroom-imported photo | the latest |
| Settings saved without the field (catalogs and their snapshots, versions and history, `lc:settings` in XMP sidecars, browser libraries: everything from before process versions), or with a damaged value | V1 |
| Reset (`develop.reset`) | the latest |
| Reset of a section or a slider | unchanged |
| Update to Current Process (`develop.updateProcess`) | the latest, for photos on an older one; one undo step. The menu item follows the selection; a call with `ids` / `id` acts on those photos whatever is selected |
| Virtual copy, duplicate | the source's |
| Copy / paste / sync / Auto Sync / presets | unchanged (see below) |
| `crs:` edits (XMP from other raw developers, Lightroom catalogs, imported presets) | unchanged; a new photo has the latest |
| Restoring a version or history step | the one it had |

**Copy, paste, sync and presets** never carry the process: it is in no settings group, so the photos
they change keep their own and interpret the pasted values with it. (Lightroom offers the process
version as a separate checkbox when syncing; LightCraft's groups are coarser and keep it out.) Partial
settings JSON that names `process` itself (`develop.merge`, a hand-written preset) sets it.

**Lightroom edits** are re-interpreted by LightCraft's mapper (`crates/engine/src/crs.rs`), which is
written against the current rendering. `crs:ProcessVersion` numbers Adobe's renderer, not ours, and is
ignored; a photo imported with Lightroom edits is on LightCraft's latest process, and an existing
LightCraft photo that receives them keeps its own.

**A number this build doesn't know** (a library or sidecar written by a newer LightCraft) loads, is
kept when saved, and renders with the newest process this build has. Update to Current Process never
moves a photo to an older process; Reset does move it to this build's latest.

**A damaged value** (anything but a whole number from 0 to 2^32 - 1; `2.0` counts as 2) reads as V1,
like a missing field, and costs nothing else: the rest of the settings load from the catalog, a
snapshot, the op log or an `lc:settings` sidecar as usual. The damaged value isn't kept: the photo is
saved as V1.

## Storage

The process is saved as `"process": <number>` in the settings JSON, except V1, which is left out.
Settings made before process versions existed are therefore saved exactly as before, and their
preview-cache keys (`DevelopSettings::hash64`) don't change. `develop.get` (MCP `get_develop`) always
states the process.
