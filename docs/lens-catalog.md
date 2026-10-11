# Native lens calibration catalog

LightCraft resolves profile corrections from two independent providers:

1. The RAW file's own lens opcodes / camera correction tables, when present.
2. A pinned, separately licensed Lensfun **database**, evaluated by original Rust code.

The Lensfun LGPL library is neither linked nor ported. The database adaptation is CC BY-SA
3.0, with the licence and source manifest under `assets/lenses/`; application code remains
MIT OR Apache-2.0. No Adobe profiles are used.

## Coverage and matching

The initial snapshot is revision `43f9e001ab66c4fcdd4f400463f13b72f94b2288`:
1,039 camera records and 1,540 rectilinear lens calibration sets. These are calibration
sets, not a promise that every camera, lens, focal length and aperture is covered.
Projection-changing/decentered lenses and ACM models are excluded from this provider.

Matching uses normalized complete model names (plus explicit EXIF aliases), camera mount
compatibility and measured crop factors. A missing camera, wrong mount, ambiguous optical
revision or duplicate equally suitable set leaves corrections unavailable. In particular,
a Canon EF 28–105 USM name without `II` never silently selects the `II USM` calibration.
Canon EF lenses can match Canon RF bodies through an adapter that preserves the optical
geometry. Focal-reducing / teleconverter combinations require their own calibrated model;
they must not be represented by the bare lens name.

Each capability is independent. Distortion can be available without lateral CA or
vignetting. Vignetting also needs aperture coverage. Current metadata has no reliable focus
distance: focus-dependent sample sets (or measurements at only one focus distance) are
withheld rather than guessed at infinity. At least two agreeing focus distances are required.
The Optics panel identifies the provider and disables unavailable profile amount controls.
`Remove Chromatic Aberration` uses the resolved TCA when available, otherwise the existing
image-based estimate; it never applies both automatically.

Existing camera-embedded data wins as a complete provider. Mixing an external profile
with an already warped embedded correction can double-correct or misregister the image.
JPEGs never acquire an external RAW profile. LinearRaw DNGs that may already be corrected
are excluded from external matching; their embedded opcodes remain supported.

## Coordinate and processing contract

The original native evaluator uses the public Lensfun model equations:

- poly3: `rd = ru (1-k1+k1 ru²)`;
- poly5: `rd = ru (1+k1 ru²+k2 ru⁴)`;
- PTLens: `rd = ru (1-a-b-c+c ru+b ru²+a ru³)`;
- linear / poly3 TCA: red and blue source radii relative to green;
- PA vignetting: gain is **the inverse transmission**, `1/(1+k1 r²+k2 r⁴+k3 r⁶)`.

Distortion/TCA use the calibration half short edge; vignetting uses its half diagonal.
Calibration crop factor and aspect ratio determine both physical radii on the target sensor.
Default RAW crop changes the optical centre relative to the displayed image, not the lens's
physical radius. EXIF and user orientation rotate/reflect that centre. Previews, zoom
windows and exports use normalized coordinates and the same resolved coefficients.

The inverse map evaluates distortion, then TCA at the **distorted** radius, then samples
the source through the existing single geometry resample. Piecewise linear interpolation
stays within the measured focal/aperture domain; no zoom-endpoint extrapolation. Vignetting
compensation is bounded to four stops outside the polynomial's measured domain.

Catalog geometry currently runs on the shared f64 CPU mapper, including GPU renders;
the remaining GPU stages stay accelerated. This avoids an unverified GPU interpretation
of another optical model. This is a performance limitation to measure, not a fidelity claim.

Resolved coefficients and a source identity are stored on the photo. Moving a catalog or
updating the database does not change existing corrections; **Reload from Disk** resolves
them again. Older catalogs deserialize without the new optional field and require explicit
reload to acquire an external correction; opening an old edit cannot silently enable it. Render cache version
51 invalidates previews generated before this integration and the DNG normalization fix.

## Reproducible data refresh

`tools/compile_lens_catalog.py` consumes a directory containing the pinned upstream XML
files and `manifest.json` in the format recorded in `assets/lenses/source-manifest.json`.
Every source XML's SHA-256 must match before compilation; unsupported schema/calibration
attributes fail explicitly. Run:

```sh
python3 tools/compile_lens_catalog.py /path/to/verified-source assets/lenses/catalog.json
```

Review the revision, licence, exclusions, database tests and visual results before updating
the snapshot and manifest together. No network request or silent database update happens
in the editor. New data retains CC BY-SA attribution. The snapshot is not complete: newer
or uncommon lenses still need open calibration measurements, embedded metadata support,
or an independently licensed supplementary provider.

## Validation

Synthetic tests cover known model/EXIF aliases, ambiguous revisions, unknown cameras,
wrong mounts, unsupported focal values, interpolation, APS-C/full-frame normalization,
crop/orientation, embedded-provider priority and catalog serialization. Optical tests
use numerical equation checks and preview/export scaling, with interval-coverage and
CPU/GPU equivalence checks. Test images are original procedural data; private photos and
camera JPEGs are not bundled or uploaded.

Design references (architecture and public models, no copied code):
[RawTherapee lens metadata/provider interfaces](https://github.com/RawTherapee/RawTherapee/blob/5f486d3678b34c74ba0c63571c17babe20935019/rtengine/lensmetadata.h),
[darktable lens correction capabilities](https://github.com/darktable-org/darktable/blob/8c64bf7ca9a8b72fba4e426900c924148c7a7309/src/iop/lens.cc),
[Lensfun public equations](https://lensfun.github.io/manual/latest/group__Lens.html),
[coordinate / crop declarations](https://lensfun.github.io/manual/latest/el_lens.html),
[inverse processing order](https://lensfun.github.io/manual/latest/corrections.html).
