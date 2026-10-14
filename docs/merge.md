# Photo Merge (HDR, Panorama, HDR Panorama, Focus Stack)

`crates/merge` (layer L3, no UI dependencies) implements the algorithms; `crates/engine/src/merge.rs`
plans, runs and finishes merges in the library; the desktop UI shows a dialog with a live preview.
All algorithms are our own implementations from published papers (cited in the module docs).

## Using it

- **App:** select the photos, then *Photo Merge ▸ HDR…* (`Ctrl+H`), *Panorama…* (`Ctrl+M`),
  *HDR Panorama…* or *Focus Stack…* from the photo context menu (or the `dialog.merge*` commands). The dialog previews
  the merge at ≤ 1024 px and re-runs the preview when an option changes; *Merge* runs the full merge
  in the background (a row in the activity stack shows its progress, with ✕ to stop it). The result is written next to the first photo as
  `<name>-HDR.dng`, `-Pano.dng`, `-HDR-Pano.dng` or `-Focus.dng`, imported and selected.
- **Commands** (CLI, MCP, control channel):
  - `merge.hdr {ids?, align=true, deghost=none|low|medium|high, autoSettings=true, stack=false, preview=false, showOverlay=false, previewPath?}`
  - `merge.panorama {ids?, projection=auto|spherical|cylindrical|perspective, boundaryWarp=0..100, autoCrop=false, fillEdges=false, autoSettings=true, maxMegapixels=40, preview, previewPath?}`
  - `merge.hdrPanorama {ids?, bracket=0 (from EXIF), …both sets of options}`
  - `merge.focusStack {ids?, align=true, autoSettings=true, stack=false, preview=false, previewPath?}`
- **CLI:** `lightcraft-cli merge hdr|panorama|hdr-panorama|focus [options] FILES…`;
  `lightcraft-cli synth-merge hdr|panorama|focus -o DIR` writes synthetic inputs (procedural scene).

## What it does

**HDR** (`hdr.rs`): frames ordered by EXIF exposure (or median luminance); the middle one is the
reference. Auto Align: features on exposure-normalised luminance, RANSAC translation / similarity /
homography (the simplest model that explains ≥ 97 % of the matches). Exposure ratios are measured
(median log ratio of well-exposed pixels between brightness neighbours), EXIF is the fallback.
Radiance is a weighted mean in linear light, weight `k · (1 − smoothstep(0.8·clip, 0.96·clip, max
channel))`. Deghost: a per-pixel consistency test against the reference radiance (with a noise
floor), dilated and feathered masks; the union is the deghost overlay. Output: 16-bit float DNG.
Lens corrections: frames are merged uncorrected, and the DNG's `OpcodeList3` carries the reference frame's
(`WarpRectilinear`, `WarpRectilinear2`, `FixVignetteRadial`: a DNG's own, or a camera's correction a raw reader
converts, e.g. Sony, Panasonic), re-expressed for its default crop (`Opcode::cropped`). So Optics ▸ Enable Lens
Corrections works on the result as on the sources; a vertical bracket keeps its pixels unturned and its
corrections.

**Panorama** (`pano/`): MOPS-style features, pairwise RANSAC homographies with Brown & Lowe's
verification, focal from homographies (or EXIF 35 mm), rotations chained along a maximum spanning
tree, Levenberg–Marquardt bundle adjustment of rotations + focal lengths, straightening.
Projections: spherical, cylindrical, perspective; *Auto* picks perspective for compact fields of
view < 110°, cylindrical when the vertical field is < 90°, else spherical. Exposure: EXIF, then
gain compensation from the overlaps. Seams: each pixel goes to the image with the highest
centre-weighted feather weight; multi-band blending (Laplacian pyramids) in log space. Boundary
Warp: a separable stretch of the smoothed valid-area edges to the canvas edges. Auto Crop: the
largest rectangle of image data (set as the develop crop, so it stays adjustable). Fill Edges:
push–pull diffusion. Output: 16-bit integer DNG (16-bit float for HDR panoramas).

**Focus Stack** (`focus.rs`, beyond Lightroom): the photos are put in shooting order (capture time,
then file name), since each is aligned to its neighbour. Full-size frames are never all held at
once: each is read twice. First small (≤ 1200 px; raws binned straight from the mosaic) versions,
in parallel: features, then each frame aligned to its neighbour with a similarity (focusing changes
the magnification; a homography fitted to features crowded into the sharp slice of the scene
extrapolates badly) and the steps chained. A frame whose step finds no fit is aligned to one of
the three frames before it instead; a frame that aligns to none of them is left out and reported
(`skipped`), as is any frame not chained to the largest group. The frame that sees the least of
the scene (the most magnified) becomes the reference, so every other frame covers it. Brightness:
each frame against the one it is chained to, as the ratio of their luminance thumbnails' sums over
the pixels both see away from clipping (sums, because blur keeps them while per-pixel ratios of
differently blurred frames lean one way); steps under 0.05 EV count as none, so measuring noise
can't wander over a long stack; EXIF only when the pixels can't tell. Measured, not read from
EXIF: on a Sony A7R VI, auto ISO's 160 → 125 left the raw data as bright. Then full size, one frame
at a time: brought to the reference's brightness, resampled into its geometry straight onto the
fusion canvas, decomposed into a Laplacian pyramid in square-root space. Every band is the mean of
the frames' bands, weighted by their energy `e`: that of the band's luminance summed over a
Gaussian window (σ = 3 level pixels) wider than a defocused edge spreads (else the blurred frame
wins just beside every sharp edge: a halo), against the band's noise floor `ε` (the 10th
percentile of the first frame's energy). A frame weighs 1 while `e ≤ 2ε` and `(1 + e/ε − 2)¹⁶`
above (the sums kept relative to the largest weight so far, so the power can't overflow): where
frames carry detail the sharpest dominates (Burt & Kolczynski's max-activity rule, made steep and
soft); where none carries more than noise they average, so out-of-focus areas get quieter instead
of keeping every frame's noise peaks. The coarsest level is the frames' average. Output: 16-bit integer DNG with the reference frame's colour, metadata and lens
corrections (colour tags are fitted for the reference only).

**DNG output** (`output.rs`): LinearRaw, 3 samples, Deflate tiles; values scaled below 1.0 with the
scale in `BaselineExposure`; raw sources keep their DNG colour tags (camera RGB), other sources get
`ColorMatrix1`/`ForwardMatrix1` of their RGB space.

## Measured (synthetic tests, `cargo test -p lightcraft-merge`)

- HDR, 3 brackets ±2 EV with 14-bit noise and handheld shifts: alignment error < 0.3 px, exposure
  error < 0.02 EV, radiance median log error < 0.03 (noise-limited, bias < 0.01), recovered
  highlights < 0.05; moving object: ghost error 0.2–2.0 without deghosting, < 0.06 with High.
- Panorama, 5 views over 182° with a 1.3× exposure drift: relative rotations < 0.1°, focal < 1 %,
  BA RMS ≈ 0.35 px; median log error vs ground truth < 0.02; no seam steps.
- Focus stack, 6 frames 640 × 420 of a scene whose depth runs across the frame (blur up to σ = 10 px,
  0.6 % focus breathing per frame): corner alignment error 0.4–1.35 px in either order, < 2 px
  from half-size alignment versions; the worst tenth of the frame errs 0.057 (mean absolute,
  linear) against 0.103 for the best single frame; frames darkened ½ EV each are measured within
  0.04 EV, without EXIF or with wrong EXIF; a featureless frame is left out and bridged over;
  noise in equally blurred areas drops to 0.36× a single frame's (8 frames).
- Focus stack, real: every 2nd frame (88) of a Sony A7R VI macro bracket (FE 100 mm, f/5.6), a
  2400 × 1500 crop of petal edges and out-of-focus background (a throwaway harness):
  background noise (8-bit high-pass) 0.39 / 0.33 / 0.56 (R / G / B) against 1.7 / 1.3 / 2.6 for a
  single frame and 1.84 / 1.38 / 2.92 for a maximum selection; petal texture (band-pass, noise
  discounted) 95–100 % of the maximum selection's.
- Focus stack, real, full size: all 175 frames of that bracket (67 MP lossless ARW) in 6 min 58 s
  wall / 1388 s CPU on a 4-CPU quota, peak 5.0 GB, measured with two speed-ups not in main yet: a
  faster AHD (the rework of #1320) and DNG Deflate at level 1 (#1731); 15 min 38 s / 2468 s before
  those, the binned small loads and the faster pyramid. Every frame aligned (median 1352 inliers,
  RMS 2 px at full size), the auto-ISO change measured as none; no ghosting, and the cyan fringe
  a maximum selection left along petal edges against the background is gone.

## Limits (known gaps)

- No lens-distortion model in the panorama bundle adjustment (rectilinear lenses assumed); no
  360° wrap-around; flat (translating-camera) scans are fitted with the rotation model. Panoramas and HDR
  panoramas carry no lens corrections, nor does an HDR whose frames don't share an orientation (they are turned
  before merging).
- No camera response calibration for JPEG brackets (sRGB decoding is taken as linear light).
- Boundary Warp is a separable stretch, not a content-preserving mesh warp; Fill Edges is diffusion,
  not patch synthesis.
- *Create Stack*: the result goes on top of a collapsed stack of its sources (`Catalog::stack_with_ops`); its
  caption also lists them.
- Panorama output is limited to `maxMegapixels` (default 40) — blending memory is ~25 bytes/pixel.
- The browser build has no file access for merges yet.
- Focus stacks: chained alignment accumulates error over long stacks (features move as the blur
  differs between neighbours: up to 0.2 % of the frame on the strongly blurred synthetic bracket;
  none visible on a real 175-frame macro stack); a similarity only, so a camera moved between
  shots (rotation about the lens's side, tilt) is not followed; no retouching from single frames,
  no depth-map mode, no halo control; frames must share their size. A preview keeps its ≤ 1024 px
  frames between the two passes (≈ 8 MB each).
