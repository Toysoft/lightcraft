# RAW parity: cameras, formats, demosaic, colour and lens profiles

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (created; first measurement against the installed Lightroom's camera and lens lists) · **Target:** Adobe Lightroom Classic 15.5 / Lightroom 9.6 (Camera Raw engine)

Raw processing is the deepest single dimension of Lightroom parity: a photographer whose camera
does not decode, or decodes with the wrong colour, cannot switch at all. This document is the
per-maker, per-format and per-stage checklist. The summary rows feed
[target-app-parity.md](target-app-parity.md); individual shortfalls are ranked in [gaps.md](gaps.md).
Details per format: [cr3.md](cr3.md), [raf-compression.md](raf-compression.md),
[sony-lens-corrections.md](sony-lens-corrections.md), [camera-preview-colour.md](camera-preview-colour.md),
[denoise.md](denoise.md).

## Headline

| Part | Weight | LightCraft | Kind | Remaining |
|---|---:|---:|---|---:|
| Decoding from sensor data, weighted by what raw shooters use | 35% | **~78%** | estimated (per-maker table below × market weights) | 100–185 h |
| Camera models verified | 15% | **~35%** (≈ 350 of ≈ 1,000 dedicated-camera models) | estimated from measured corpus counts | 20–35 h + sample files |
| Camera colour (default rendering per model) | 25% | **~35%** | estimated; ΔE figures measured | 80–150 h incl. render-fidelity suite |
| Demosaic and raw detail | 10% | **~60%** | estimated | counted in Enhance ([gaps.md](gaps.md)) |
| Lens profiles and corrections | 15% | **~15%** | estimated against a measured profile count | 40–80 h + calibration shots |
| **RAW, ready for real work** | 100% | **~50%** | estimated | **240–450 h** |

## What the target supports (measured)

From the installed Lightroom 9.6 bundle on the owner's Mac, by **folder and file names only** (the
clean-room rule allows listings, never contents):

- **Raw extensions** in `Info.plist` `CFBundleDocumentTypes`: dng, cr2, cr3, crw, dxo, erf, x3f, raf,
  gpr, 3fr, fff, dcr, kdc, mos, rwl, mef, mfw, mrw, nef, nrw, orf, ori, rw2, raw, pef, iiq, srw, arw,
  sr2, srf, arq (plus jpg, tif, heic).
- **Camera profiles:** 1,500 "Adobe Standard" profiles (one per supported model), led by Apple 195,
  Sony 156, Samsung 154 (mostly phones), Canon 136, Google 105, Nikon 99, Fujifilm 95, Panasonic 87,
  Xiaomi 73, Olympus 70, Leica 59, Phase One 26, Pentax 41, Leaf 19, DJI 18, Ricoh 15; and 395
  "Camera Matching" model folders (Canon 116, Nikon 84, Sony 83, Panasonic 41, Olympus 41, Pentax 9,
  OM 8, Ricoh 6, Leica 4, Sigma 3). Roughly 1,000 of the 1,500 are dedicated cameras, the rest phones
  and drones.
- **Lens profiles:** 3,635 profile files from 56 makers (e.g. Sigma 596, Canon 263, Tamron 166, Nikon
  156, Sony 156).
- **AI models** in its model folder (37 names), among them denoise, depth estimation, dust
  detection and the subject / sky / portrait selection models used by masks (see
  [target-app-parity.md](target-app-parity.md#ai)).

Lightroom Classic 15.5 (August 2026) shares the same Camera Raw engine and support list.

## Formats and containers

Source: `crates/raw/src/lib.rs` (`probe`, `decode_with`), `crates/raw/src/vendor/*.rs`, the import
list `lightcraft_engine::import::EXTENSIONS` (`crates/engine/src/import.rs`). "Preview" means the
photo opens from its embedded JPEG (`crates/raw/src/preview.rs` scans every container) and is
flagged `preview_only`; edits then apply to an 8-bit preview, not to sensor data.

| Format | Maker | Target | LightCraft | Missing |
|---|---|---|---|---|
| DNG | Adobe, Leica, Pentax, Ricoh, phones, converters | ✅ | ✅ uncompressed, lossless JPEG, lossy JPEG / Smart Previews, Deflate incl. float predictors, DNG 1.7 JPEG XL tiles, CFA and LinearRaw, tiles and strips, opcodes, semantic mattes (ProRAW sky) | GoPro VC-5 (GPR) payloads, non-rectangular or >3-colour CFAs, ProfileGainTableMap on by default |
| CR2 | Canon (2004–2018) | ✅ | ✅ lossless JPEG slices; sRAW / mRAW (31 samples, 16 models); colour-filter phase from the file's own tag (#85) | — |
| CR3 | Canon (2018–) | ✅ | 🟡 CRX lossless and C-RAW of versions 0x100 / 0x200 (exact on M50, R100, R8 against an independent oracle, `crates/raw/tests/cr3_corpus.rs`); everything else falls back to the preview | **other CRX variants** (vertical tiles, partial subbands), **CR3 sRAW / mRAW**, **HEIF/HDR PQ CR3**, dual-pixel data; only 3 bodies verified for every CR3 body Canon sells |
| CRW | Canon (≤ 2004) | ✅ | preview only | decoder |
| NEF / NRW | Nikon | ✅ | ✅ uncompressed, lossless, lossy types 1/2, lossy after split (#633), 12/14-bit, CropArea | **High Efficiency / HE★ NEF** (JPEG XS based: Z 8, Z 9, Z 6III, Z f, Z50II…; named and preview only, #193) |
| ARW / SR2 / SRF / ARQ | Sony | ✅ | ✅ uncompressed, ARW2 (cRAW), packed 12-bit, lossless LJ92, downsized lossless YCbCr (M/S), SR2 white balance and black level through the decrypted SR2SubIFD | **ARQ pixel shift**, SRF, ARW v1 and a few YCbCr layouts; sr2/srf/arq are not in the import list |
| RAF | Fujifilm | ✅ | ✅ uncompressed, lossless and lossy compressed, Bayer and X-Trans, GFX 14/16-bit (24 files on 13 bodies exact against a reference) | old FinePix layouts without a raw IFD |
| ORF / ORI | Olympus, OM System | ✅ | 🟡 uncompressed 16-bit, 12-bit packed, two-field 12-bit of old compacts, 12.8 bpp blocks (#651: E-300, E-500, E-330, E-M5 II and PEN-F high-res) | **compressed ORF** (nearly every interchangeable-lens body since ~2008), ORI not in the import list |
| RW2 / RWL / RAW | Panasonic, Leica | ✅ | ✅ every raw format (compressed 4, 6, 8; packed 2, 5, 7; 16-bit words), checked on 178 files from 118 bodies; embedded distortion | — |
| PEF | Pentax | ✅ | ✅ uncompressed, Huffman, PackBits-tagged 12-bit | — |
| SRW | Samsung | ✅ | 🟡 uncompressed and 12-bit packed | compressed 32770 / 32772 / 32773 (NX1, NX500, NX30, NX300, NX2000, NX3000, NX mini) |
| 3FR / FFF | Hasselblad | ✅ | 🟡 decoded when the file is DNG-like (CFA IFD, uncompressed or LJ92); preview otherwise | Hasselblad compression |
| IIQ | Phase One | ✅ | preview only | IIQ L / S decoders |
| MOS | Leaf | ✅ | preview only | decoder |
| ERF, KDC, DCR | Epson, Kodak | ✅ | ERF / KDC preview only; DCR not imported | decoders (low use) |
| X3F | Sigma (Foveon) | ✅ (older bodies) | preview only | Foveon decode and processing |
| MRW | Konica Minolta | ✅ | preview only | decoder |
| MEF, MFW | Mamiya | ✅ | not imported | decoders |
| GPR | GoPro | ✅ | not imported (VC-5 inside DNG unsupported) | VC-5 decoder |
| DXO | DxO PureRAW output | ✅ | not imported | linear DNG-like reader |
| HEIC / HEIF | Apple, phones | ✅ | ✅ through `crates/heif` (heic-rs), enabled in every release build | AVIF import |

## By maker

Weights are a judgement of the share of Lightroom's raw users by camera brand (no measured source):
Canon 30%, Sony 25%, Nikon 20%, Fujifilm 10%, Panasonic 4%, OM / Olympus 4%, phone DNG 3%,
Pentax / Ricoh 1.5%, Leica 1%, medium format 1%, other 0.5%. "Decoded" is the estimated share of
that brand's current raw files that develop from sensor data.

| Maker | Weight | Target profiles (Adobe Standard / Camera Matching) | Verified here (measured) | Decoded | Biggest gap |
|---|---:|---|---|---:|---|
| Canon | 30% | 136 / 116 | 18 manifest samples; 35 CR2 bodies for CFA layout; CR3: 3 bodies exact; 16 sRAW models | ~60% | CR3 variants beyond M50 / R100 / R8 |
| Sony | 25% | 156 / 83 | 23 manifest samples; 100 ARWs from 87 bodies (metadata vs ExifTool); 126 SR2 ARWs from 97 bodies | ~95% | ARQ pixel shift; lens corrections |
| Nikon | 20% | 99 / 84 | 6 manifest samples; ~60 NEFs checked by hand | ~85% | HE / HE★ NEF on current Z bodies |
| Fujifilm | 10% | 95 / — | 21 manifest samples; 24 RAFs on 13 bodies exact | ~95% | colour (2 bundled profiles) |
| Panasonic / Leica RW2 | 4% | 87 / 41 | 178 files from 118 bodies | ~100% | colour |
| OM / Olympus | 4% | 70 + 7 / 49 | 5 manifest samples | ~15% | compressed ORF |
| Phones (DNG) | 3% | ~600 | iPhone 12 Pro ProRAW, Pixel 2 XL | ~95% | per-model checks |
| Pentax / Ricoh | 1.5% | 41 + 15 / 15 | 3 + 1 manifest samples | ~95% | — |
| Leica | 1% | 59 / 4 | 1 manifest sample | ~95% | — |
| Hasselblad / Phase One / Leaf / Mamiya | 1% | ~55 | none | ~20% | IIQ, Hasselblad compression |
| Others (Sigma, Samsung, Kodak, Minolta, Epson) | 0.5% | ~30 | 3 Samsung samples | ~20% | X3F, compressed SRW, legacy formats |
| **Weighted** | 100% | ≈ 1,500 / 395 | **≈ 78 models in `RAW_SAMPLES` (`xtask/src/main.rs`, 103 files); ≈ 350 models checked in all** | **~78%** | |

## Demosaic, highlights and raw detail

| Stage | Target | LightCraft | Status |
|---|---|---|---|
| Bayer demosaic | Adobe's own, plus AI "Raw Details" | AHD (default), PPG, bilinear (`crates/raw/src/demosaic`) | 🟡 no Raw Details equivalent, quality unmeasured against Lightroom |
| X-Trans demosaic | ✅ | in-house edge-weighted directional | 🟡 unmeasured |
| Foveon | ✅ (older bodies) | — | ⬜ |
| Monochrome / LinearRaw | ✅ | no demosaic needed, handled | ✅ |
| Pixel shift (Sony ARQ, Pentax, OM high-res raw) | ✅ (Sony, Pentax) | — | ⬜ |
| Highlight reconstruction | ✅ | neutral clip, chromaticity diffusion from unclipped neighbours (`highlight.rs`) | 🟡 tuned by eye |
| AI denoise | ✅ Bayer and X-Trans, model bundled | Bayer only, user-installed weights, pure-Rust CPU / GPU (`crates/denoise`) | 🟡 no X-Trans, no shippable weights |
| Super Resolution | ✅ | — | ⬜ |
| Dual-pixel / depth data | ✅ (depth masks from phones) | DNG semantic mattes only | 🟡 |
| HDR / panorama merge to raw DNG | ✅ | linear DNG (float16 for HDR) with the source's colour tags (`crates/merge`) | ✅ |
| Convert to DNG | ✅ | uncompressed or LJ92 tiles (`dngwrite.rs`) | ✅ (no lossy DNG) |

## Colour: camera profiles and default rendering

Lightroom ships a measured Adobe Standard profile for every supported model, Camera Matching
profiles for 395, and Adobe Color / Landscape / Portrait / Vivid / Monochrome looks. LightCraft
never uses Adobe matrices, DCPs or LCPs. It starts a raw's colour from, first available
(`docs/camera-preview-colour.md`):

1. the DNG's own matrices and profile (ColorMatrix / ForwardMatrix 1-2, CameraCalibration,
   AnalogBalance, dual-illuminant interpolation, ProfileHueSatMap, ProfileLookTable,
   ProfileToneCurve; ProfileGainTableMap read and written but off by default);
2. a pooled per-model profile: **4 bundled** (Sony ILCE-7CR, ILCE-7M4, Fujifilm X-H2S, X-T4);
3. a guarded per-file fit to the camera's own embedded JPEG (ARW, NEF, RW2, RAF, CR3, CR2, PEF, SRW,
   ORF);
4. matrices fitted to measured spectral sensitivities for **52 models** (rawtoaces-data,
   Apache-2.0: Canon 22, Nikon 11, Sony 11, Fujifilm 3, Hasselblad 2, others 3);
5. a neutral fallback, flagged `matrix_is_fallback`.

Measured results (all against the camera's JPEG unless noted): ARW 7 of 10 public samples accepted,
ΔE 17–26 → 3–10; ILCE-7M4 29 files 4.74 → 3.54; ILCE-7CR withheld 4.07 → 3.29; NEF 13–46 → 2.9–7.8;
RW2 140 of 174 accepted, median 5.0; RAF 16.70 → 4.13, pooled X-H2S 1.62 / X-T4 2.34; SRW 2.2–7.6;
against Lightroom: Sony DRO median ΔE76 7.5 → 3.3, Olympus E-1 ΔE00 11.25 → 8.30, ProRAW with gain
map ΔE00 19 (why the map is off).

What's missing for parity: **measured (chart-based) calibration** for the models people shoot,
absolute-Kelvin white balance on non-DNG raws, a looks set comparable to Adobe Color / Landscape /
Portrait, and a **render-fidelity suite** that scores our default rendering against Lightroom's on
the same CC0 raws (references kept local in `plan/`).

## Lens profiles and corrections

| Feature | Target | LightCraft | Status |
|---|---|---|---|
| Lens profile database | 3,635 profiles, 56 makers, auto-selected by lens metadata | none (Adobe LCPs are never used) | ⬜ |
| Embedded DNG corrections (WarpRectilinear, FixVignetteRadial, GainMap, bad pixels, MapTable / MapPolynomial, per-row / column) | ✅ | ✅ applied; TrimBounds and WarpFisheye recorded, not applied | 🟡 |
| In-file manufacturer corrections | ✅ (Panasonic, Olympus, Fujifilm, Sony, Canon) | Panasonic / Leica RW2 distortion (79 photos, 13 bodies, 11 lenses); Sony tag 0x7037 distortion for ILCE-7RM4A only (2 lenses) | 🟡 no vignetting / lateral CA from makernotes, no Fujifilm, Olympus or Canon data |
| Manual distortion, vignetting, lateral CA, defringe | ✅ | ✅ (`crates/pipeline/src/optics.rs`) | ✅ |
| Lens Blur (AI depth) | ✅ | field reserved, not rendered | ⬜ |

## Remaining work (Opus 5.5 agent hours)

Calibrated on this repo's raw history (PR open-to-merge times on GitHub): Panasonic RW2 in every
format 6.5 h (#215), Fujifilm compressed RAF 8.6 h (#282), CR3 CRX and C-RAW 10 h (#279), NEF lossy
after split 4.7 h (#633), ORF 12.8 bpp 1.7 h (#651), HEIC 7 h (#609). A new decoder family with
black-box analysis runs 5–15 h; a variant of a known family 2–5 h.

| Work | Hours | Needs |
|---|---:|---|
| Compressed ORF | 8–15 | CC0 samples (have some) |
| CR3: remaining CRX variants, sRAW / mRAW, HEIF / HDR PQ CR3 | 25–45 | samples from more bodies |
| HE / HE★ NEF (JPEG XS) | 20–40 | a pure-Rust JPEG XS decoder (none exists) |
| Compressed SRW, ARQ pixel shift, IIQ, Hasselblad, X3F, legacy (CRW, MRW, ERF, KDC, DCR, MEF), GPR, old FinePix | 45–85 | samples; low user share |
| Model verification: one CC0 sample per popular model, checked for plausible colour | 20–35 | public samples; owners' files |
| Measured colour calibration and render-fidelity suite | 80–150 | chart shots per camera (human with cameras) or licensable measured data |
| Lens profile database of our own (fitting tools, data format, top 100 lenses), makernote vignetting / CA | 40–80 | calibration shots (human with lenses) |
| **Total** | **240–450 h** | ~60% parallelizes (each format is independent) |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | major | Created: formats, makers, demosaic, colour and lens checklists; measured the target's camera, profile and lens counts from the installed Lightroom 9.6 bundle listing; hours calibrated on raw PRs |
