# Detail processing

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-11 · **Change:** major (V3 noise-aware luminance reduction) · **Target:** useful RAW detail, not measured Adobe equivalence

Process V1 sharpens from the same Gaussian log-luminance band as Texture, with sigma
0.0018 of the output's full-frame long edge. On a 5568 px image that is about 10 px.
Its Radius and Detail controls were stored but never used by either renderer. At the
RAW colour-NR default (Amount 25, Detail 50), its blend retains 81.25% of the original
chromaticity noise before accounting for what remains in the blurred reference.

V2 uses an independent fine log-luminance band. Radius is relative to a 6000 px reference
frame, with a minimum sigma of 0.6 output pixels; Texture retains its original scale.
Detail controls a soft threshold on the fine residual: low Detail avoids boosting small
variations; high Detail retains them. Masking further restricts sharpening by edge strength.
The sharpening gain is bounded, acts on luminance, and leaves flat fields unchanged.
The CPU and WGSL implementations use the same parameter definitions. The GPU packs the
fine plane after the Texture plane, without adding a storage-buffer binding.

Colour NR uses the same spatial filter and preserves luminance. Its Amount response is
`(1 - exp(-8 * amount)) / (1 - exp(-8))`, where amount is 0–1, before the Detail retention
factor. This makes the default Amount useful while preserving zero and full-strength
endpoints. This is our own retune, not a reproduction of Adobe's noise reduction.

Existing photos retain their saved process. Update to Current Process changes selected photos in one
undoable step. New imports and Reset use V3. Reset restores RAW capture sharpening, luminance/colour NR and
embedded lens corrections instead of clearing them to the rendered-image defaults. Presets continue to set their ordinary
controls and preserve the receiving photo's process. Process is part of the noise-reduction
stage cache key; Radius is part of the sharpening-plane cache key.

## V3 luminance noise reduction

V3 retains V2 sharpening and colour reduction, and replaces the fixed-epsilon log-luminance
filter with an original local variance filter on square-root scene luminance. It estimates
noise from the lower fifth of local median absolute residuals on a bounded 16×12 source grid.
The estimate uses the current white balance, before exposure, and is shared by CPU/GPU and
all windows. It is not estimated from whichever patch happens to be visible at 100%.

The local filter subtracts the estimated noise variance from observed variance, retains
stronger structure, smooths its coefficients, and scales RGB by the reconstructed luminance.
Amount controls the noise variance removed; Detail retains more variation. Zero Amount and
turning Detail off bypass it. New RAW imports use Amount 25, while rendered images start at
zero. V1/V2 preserve both their old algorithms and zero-luminance-NR import baseline. Reset
chooses current defaults; a recorded import preset retains its own settings. Presets and edits
remain ordinary adjustable controls rather than processing baked into the decoded source.

Noise estimation is heuristic: it can mistake texture for noise when a scene has no smooth
regions, or underestimate noise when large flat/clipped areas dominate. Downsampling scales
the estimate by the output/source ratio; this approximates resampling's noise reduction and
is not an exact model of every demosaic or lens warp. Colour/tone calibration is unchanged.

## Evidence and limits

Synthetic regressions check effective Radius and Detail, unchanged flat fields, reduced
broad edge halos, preserved legacy rendering, cache invalidation, and colour-speckle
reduction without altering luminance. GPU equivalence cases cover all three processes, Radius,
Detail, Masking, and simultaneous Texture. A real-file diagnostic uses the actual loader:

```
cargo run -p lightcraft-engine --example raw_quality -- INPUT.NEF OUTPUT_DIR
cargo run -p lightcraft-engine --example raw_quality -- INPUT.NEF OUTPUT_DIR full
```

A private Nikon Z50 II HE file (ISO 2500, 5568×3712 developed crop) provided the motivating
case. All 20,876,800 sensor samples match the independent reference decoder exactly; its
V1 full-resolution render remains pixel-identical to the build before this change. The
V2 result reduces colour speckling and changes sharpening, but the Nikon camera JPEG still
has a different, stronger tone rendering. No full-resolution Lightroom export was available
for a numerical detail comparison. This does not establish Nikon-wide or Adobe rendering
parity. No private image is committed.

V3 signal tests use known clean edges and repeating fine texture with added Gaussian noise;
they require substantially reduced flat-region error, improved textured-region error, retained
texture amplitude and bounded edge bias. Separate tests exercise estimate scaling, old process
defaults, Reset, and CPU/GPU agreement. Real Nikon renders are inspected at fit and native size.
The full GPU equivalence suite passes, including adaptive NR at preview/native sizes, changed
white balance and render windows. On the private ISO 2500 and ISO 100 Z50 II files, CPU/GPU
differences are at most one 8-bit level at 1400 px and two levels at native size. The 21 MP
ISO 2500 pipeline took 2.28 s on two CPU workers and 0.26 s on the local GPU (render only,
excluding decode and export encoding). These are local measurements, not portable budgets.
A headless app check imports a private copy, applies a built-in preset and resets a legacy
edit; Reset restores V3/Amount 25 without a preview-only notice. The V1 and V2 full-resolution
outputs remain byte-identical to the earlier diagnostic renders of the same photo.

Luminance NR remains a separate control. Its Contrast control is still
unimplemented. The Gaussian colour filter is not an edge-aware or learned denoiser, and
the camera colour/tone fit is unchanged. These remain quality limitations.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | major | Add V3 adaptive luminance reduction, current RAW defaults and limitations |
| 2026-10-11 | major | Describe V2 detail processing, legacy preservation, validation method and unresolved quality gaps |
