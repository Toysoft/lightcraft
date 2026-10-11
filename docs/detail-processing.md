# Detail processing

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-11 · **Change:** major (V2 detail processing) · **Target:** useful RAW detail, not measured Adobe equivalence

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

Existing photos remain on V1. Update to Current Process changes selected photos in one
undoable step. New imports and Reset use V2. Reset restores RAW capture sharpening, colour NR and
embedded lens corrections instead of clearing them to the rendered-image defaults. Presets continue to set their ordinary
controls and preserve the receiving photo's process. Process is part of the noise-reduction
stage cache key; Radius is part of the sharpening-plane cache key.

## Evidence and limits

Synthetic regressions check effective Radius and Detail, unchanged flat fields, reduced
broad edge halos, preserved legacy rendering, cache invalidation, and colour-speckle
reduction without altering luminance. GPU equivalence cases cover both processes, Radius,
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

Luminance NR remains a separate control and starts at zero. Its Contrast control is still
unimplemented. The Gaussian colour filter is not an edge-aware or learned denoiser, and
the camera colour/tone fit is unchanged. These remain quality limitations.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | major | Describe V2 detail processing, legacy preservation, validation method and unresolved quality gaps |
