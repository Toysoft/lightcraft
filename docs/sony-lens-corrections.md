# Sony embedded distortion corrections

LightCraft reads the signed 16-sample distortion table in the raw image IFD (`0x7037`) of
Sony **ILCE-7RM4A (A7R IVA)** and **ILCE-7M3 (A7 III)** Bayer ARWs. Other camera models are deliberately
left uncorrected, even when they contain a table with the same layout.
It converts that table into `OpcodeList3` / `WarpRectilinear`, so the existing profile correction control,
CPU/GPU optics path, EXIF orientation handling, and DNG export use the same correction.

This adds **distortion only**. Sony vignetting and lateral chromatic-aberration tables are not decoded.
The enabled and independently validated models are the ILCE-7RM4A with the FE 24–105mm F4 G OSS and
FE 200–600mm F5.6–6.3 G OSS, and the ILCE-7M3 with the FE 16–35mm F4 ZA OSS, FE 28–70mm F3.5–5.6 OSS and
FE 50mm F1.8 ([ILCE-7M3 validation](#ilce-7m3-validation)). Linear YCbCr ARWs and non-3:2 camera crops are excluded pending validation. An aspect crop may
retain the full-frame radial normalization; the decoder does not guess that relationship. Older files that carry
only encrypted correction metadata, different table lengths, and rejected tables remain uncorrected.
The camera's distortion Off setting does not erase its table. Newly imported photos use LightCraft's existing
embedded-profile default (enabled); the profile correction control can disable or adjust it.

## Clean-room evidence

The [ExifTool EXIF tag-name documentation](https://exiftool.org/TagNames/EXIF.html) identifies
`0x7037` as 17 signed 16-bit integers. The first value is the count, 16, followed by radial samples.
No external photo decoder implementation or Adobe lens profile was consulted.
Sony Imaging Edge Edit 4.1 was used only as a black-box reference on local scratch copies.

The observed inverse mapping, with radius `r` normalized to the default crop's half diagonal, is:

```
f(r) = 1 + interpolate(table, r) / 16384
source = center + (output - center) * zoom * f(r)
zoom = 1 / max(f(r)) over radii on the rectangular output boundary
```

The border radii range from the shorter half edge / half diagonal to 1. Table knots span 0 through 1.
Linear interpolation approximates the observed map closely; we do not claim to reproduce Sony's
internal interpolator. A fixed-size weighted least-squares fit expresses source radius as
`r * (k0 + k1*r² + k2*r⁴ + k3*r⁶)`. The fit is rejected if radial error exceeds 0.0005 of the
half diagonal (about 0.43 px at a 1440 px 3:2 preview), or the mapping folds. Table values are bounded
and the input must have precisely the supported signed-short layout. Crop offset and normalization
are converted into DNG's active-area coordinate system before creating the opcode. The warp's centre
and normalization come from the standard `DefaultCropOrigin` / `DefaultCropSize` when the file has them,
not from Sony's crop tags: on the tested A7R IVA files the former start at raw x=32 and the latter at
x=0, and the standard origin is the one that aligns with Sony's exports. The image itself keeps the
decoder's crop precedence for every Sony body (Sony's crop tags, then the default crop, then the
recorded image size), so the framing is unchanged; the opcode's centre is in active-area coordinates,
so the warp stays on the optical centre either way. On the A7R IVA that framing still sits 32 raw
pixels left of Sony's exports (eight pixels in a 2376-pixel-wide export).

The main raw-IFD table is important: the similarly named MakerNote table has different values and
must not be substituted into this formula. Main raw-IFD and SR2SubIFD values agreed in the test files.

## Validation

At 2376×1584, independent SIFT correspondences between Sony distortion On and Off TIFF exports gave:

| Lens / focal length | Median residual | 95th percentile | Maximum matched radius |
|---|---:|---:|---:|
| FE 200–600 / 200 mm | 0.043 px | 0.191 px | 0.97 half diagonals |
| FE 200–600 / 600 mm | 0.055 px | 0.231 px | 0.95 half diagonals |

These use the fixed 16384 scale and automatic border framing, not fitted per-image scale or zoom.
A median translation below 0.004 px was removed from the residuals. Correspondences do not cover
all pixels; these are geometric feature residuals, not photometric equality or a worst-case guarantee.
Across 20 sampled RAWs (16 at 24–105 mm, four at 200–600 mm), the polynomial's maximum deviation
from the interpolated file table was below 0.12 px at 1440 px. That measures approximation error,
not agreement with an independent renderer. Camera JPEG and Apple Core Image comparisons support
the wide-lens interpretation but are weaker evidence than same-renderer On/Off pairs.

Historical validation before restoring main's image crop precedence: LightCraft exports with the
standard default crop framing were compared directly with Sony's corrected
TIFFs. At 2376 px wide, median / 95th-percentile feature residuals were 0.35 / 1.04 px at 24 mm,
0.15 / 0.40 px at 200 mm, and 0.13 / 0.56 px at 600 mm (median translation below 0.024 px).
The uncorrected medians at 200 and 600 mm were 8.63 and 10.82 px respectively. Different colour,
sharpening, and demosaicing affect feature localization; these are geometry checks, not colour parity.
An isolated desktop run on Apple M1 Metal detected the profile, rendered without GPU fallback or
warnings, and exposed the existing Optics controls. Existing library edits are not reset: once the
source has loaded, enable lens corrections if they were previously disabled.

After restoring main's crop precedence, fresh 2376×1584 exports on 2026-10-10
(main `b70ae174` plus the aspect-framing guard) measured:

| Focal length | Corrected median / p95 residual | Uncorrected median | Removed translation x / y | Matches / maximum radius |
|---|---|---|---|---|
| 24 mm | 0.36 / 1.12 px | 5.62 px | 7.98 / −0.01 px | 1,442 / 0.95 |
| 200 mm | 0.13 / 0.35 px | 8.65 px | 8.01 / 0.02 px | 11,606 / 0.98 |
| 600 mm | 0.10 / 0.37 px | 10.33 px | 8.02 / 0.00 px | 12,268 / 0.97 |

Residuals subtract only the median feature translation; no scale or lens coefficients are fitted to
these reference pairs. The roughly eight-pixel horizontal translation is the retained crop-origin
difference described above, not a claim of exact Sony framing. Maximum radius is in half diagonals.
The distortion geometry remains aligned while every body's image framing follows main's crop order.

Tests cover signedness/length rejection, zero and malformed tables, independent 600 mm reference
geometry, barrel framing, offset crop normalization, both TIFF byte orders, header/full consistency,
and preservation through DNG export. No private photographs or reference TIFFs are committed.

## ILCE-7M3 validation

Checked on 2026-10-12 against the camera's own JPEG of the same exposure, which is an independent renderer
that applied the correction (RAW+JPEG, Lens Comp. ▸ Distortion Comp.: Auto). 27 RAW+JPEG pairs from one
ILCE-7M3: a checkerboard filling the frame on a TV (handheld) and on a desktop monitor (tripod), compressed and
uncompressed raws, apertures from wide open to f/8, plus one textured wall. Tables covered strong barrel (16 mm,
−690 at the corner), strong pincushion (70 mm, +1161) and the sign change between them.

Method (`tools/sony_distortion_check.py`): LightCraft renders each ARW at full size with the profile correction
on and off; sub-pixel board corners (Hessian saddle points refined with `cornerSubPix`, kept only where the four
quadrants alternate, labelled into rows and columns) are matched with the JPEG's by grid position, with the
match fixed by the central corners before any fit, because a board is periodic. A similarity (zoom, rotation,
shift) is fitted, as the camera fills its frame with its own zoom; no lens coefficient is fitted. The textured wall
uses SIFT matches instead. The script was checked on synthetic boards with a known radial distortion.

| Lens / focal length | Shots | Corrected vs JPEG: median / p95 (px at 6000) | Uncorrected median | Corner (r > 0.9) | Camera / LightCraft correction |
|---|---:|---|---:|---:|---:|
| FE 16–35 F4 ZA / 16 mm | 10 | 0.33–0.38 / 1.01–1.15 | 16.7–17.0 | 1.40–1.56 px | 1.02 |
| FE 16–35 F4 ZA / 20 mm | 4 | 0.16–0.24 / 0.34–0.50 | 3.9–4.4 | 0.15–0.24 px | 0.98 |
| FE 16–35 F4 ZA / 35 mm | 2 | 0.18–0.21 / 0.39–0.44 | 11.0–11.1 | 0.18–0.26 px | 1.01–1.02 |
| FE 28–70 / 28 mm | 2 | 0.24–0.25 / 0.59–0.64 | 13.2–13.4 | 0.92 px | 1.02 |
| FE 28–70 / 33 mm | 1 | 0.07 / 0.16 | 0.8 | 0.07 px | 1.00 |
| FE 28–70 / 50 mm | 1 | 0.30 / 0.69 | 20.8 | 1.24 px | 1.01 |
| FE 28–70 / 70 mm | 1 | 0.44 / 0.93 | 26.4 | 1.49 px | 1.01 |
| FE 50 F1.8 / 50 mm | 4 | 0.06–0.12 / 0.15–0.31 | 2.8–3.0 | 0.08–0.14 px | 1.00–1.01 |
| Wall, 16 mm (SIFT) | 1 | 0.37 / 1.06 | 5.6 | 1.22 px | — |

Residuals are in pixels of the 6000×4000 JPEG; at the 2376 px exports used above for the ILCE-7RM4A, the
board medians are 0.02–0.17 px and the p95 at most 0.46 px. The last column fits JPEG ≈ uncorrected +
α·(corrected − uncorrected) radially: LightCraft applies the camera's amount of correction within 2 %, slightly
less for strong barrel and slightly more at 20 mm, which is what remains at the 16 mm corners (about 1.5 px of a
35 px correction). Rows and columns of corrected boards are as straight as in the JPEG in every shot (largest
bow 0.8–7.8 px in both, from the screens), against 7–55 px uncorrected. Compressed and uncompressed raws,
the TV and the monitor, and wide open and f/8 agree. Two 16 mm board shots whose board stopped short of the
corners (reach 0.8 of the half diagonal) gave 0.85 px medians and are left out of the table.

On this body Sony's crop tags and the default crop agree (origin 12, 12, 6000×4000 of 6048×4024), so the
warp sits on the image centre; fitted against the camera, the centre is within 0.3 px. Unit tests keep the warp from
three of these tables within 0.0008 of the half diagonal (2.9 px at 6000 px) of the camera's fitted geometry.
The photographs stay with their owner; none is committed.

## Camera coverage and regression boundary

The model check is exact: `ILCE-7RM4` (A7R IV without the A suffix), unknown/missing
model names, and every other model remain unsupported by this distortion decoder. Matching tag
numbers and table lengths do not establish the scale, interpolation or crop normalization on another
body. Support can expand after independent geometry validation; do not add model-name prefix matching.

The current `cargo xtask corpus --download` manifest contains 23 Sony CC0 samples across 17 bodies,
including samples 4822 (A7R IVA / 24–105 mm) and 3989 (A9 II / 200–600 mm). All 23 files have
pinned SHA-256 hashes and are checked with `sony_embedded_distortion_is_limited_to_validated_models`:

| Camera | Corpus samples | Raw-IFD table observed | Expected correction |
|---|---|---|---|
| ILCE-7RM4A | Compressed (4822) | 16 samples | One distortion warp |
| ILCE-9M2 | Compressed (3989) | 16 samples | None |
| ILCE-7M3 | Compressed, uncompressed | 16 samples | One distortion warp |
| ILCE-7M4 | 14-bit, lossless L/M/S | 11 samples in the 14-bit file; 16 in L/M/S | None |
| ILCE-7RM2 | 12-bit uncompressed | 16 samples | None |
| DSC-RX100 | One | No raw-IFD table | None |
| DSC-RX100M3 | One | 11 samples | None |
| ILCE-7CR | Compressed, lossless L/M | Not used; model excluded | None |
| ILCE-7RM4 | 14-bit compressed | Not used; model excluded | None |
| DSLR-A500, DSLR-A700, SLT-A33 | One each | Not used; model excluded | None |
| ILCE-3500, ILCE-7S | 5600 K, Shade respectively | Not used; model excluded | None |
| ILCE-7SM2, DSLR-A580 | 16:9 framing, one each | Not used; model excluded | None |
| DSLR-A900 | Packed 12-bit | Not used; model excluded | None |

These are **model-boundary regression checks, not new geometric validation of those models**. The A7 III tables
were refused at first because their corner warp was unverified; the [ILCE-7M3 validation](#ilce-7m3-validation)
measured it against the camera's own JPEGs and enabled the body. The lossless M/S
files are linear YCbCr and remain excluded independently of the model check. Header and full decode
must agree for every sample. Synthetic tests also verify both TIFF byte orders, exact model matching,
that the warp is centred on the standard default crop while the image keeps Sony's crop, and that DNG export preserves an accepted warp without adding one to rejected
models. A non-3:2 Sony image crop is also rejected when the standard default crop still describes
3:2 geometry; the regression fails without the image-framing guard. The existing independent Sony
geometry tests continue to cover the enabled A7R IVA path.
Before restoring main's crop precedence, re-exporting the 24, 200 and 600 mm reference files after
the model restriction produced byte-identical corrected PNGs to the earlier validated exports. Isolated headless app checks showed the A7R IVA profile
controls present and the A7 III profile controls absent, with no notices and completed RAW renders.

Before enabling another body, compare corrected and uncorrected renders against independent references
across multiple lenses/focal lengths and photographs with features near the edges and corners. Record
residuals and spatial coverage, and check crop/aspect and compression variants. Embedded JPEGs are
useful when the camera actually applied distortion correction; an Off JPEG cannot certify an On warp.
Prefer same-renderer Sony On/Off exports when the camera setting or geometry is ambiguous. Table-fit
error alone only checks our polynomial approximation, not whether the table's interpretation is correct.
