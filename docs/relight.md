# Relight

> **Last updated:** 2026-10-12

Relight adds lights to a photo after it was taken: a lamp placed on the scene, a low sun from one
side, a spot on a face. It is a tool of its own, the lamp button right under Edit in the tool strip
(Window ▸ Relight), and it is non-destructive like every other edit.

It is not a Lightroom feature.

## Using it

- **Add Light** puts a light in the middle of what is shown.
- **Place a light** by dragging its marker on the photo; a click on a marker selects its light. Nothing else on the photo moves a light: a click or a drag elsewhere leaves them all where they are.
  A drag is one undo step. Delete removes the selected light.
- Each light is a **Point** (a lamp shining every way), a **Directional** (parallel light from far
  away, like the sun: it has a direction, not a place, and its marker sits on a ring around the
  middle of the photo, on the side it shines from) or a **Spot** (a cone from a place).
- Directional and Spot lights carry a small **figure** on the photo and in the panel, turned in
  space by their numbers: a disc with rays for a Directional light (a full disc head-on, a thin
  ellipse with long rays at a grazing angle), a cone for a Spot (wider with Cone, seen down its
  length when it points straight into the photo). Moving Angle, Elevation or Cone turns it.
- **Elevation** is a full turn, −180° to 180°: 0 grazes the photo, 90 is head-on, beyond 90 the light
  comes over from the other side, and below 0 it shines from behind the photo (only on what faces
  away: a rim light) or, for a Spot, points out of it.
- **Light Color** is one of two things, whichever was set last: a **Temperature** in Kelvin
  (2000–12000: 6500 is white, lower is warmer like a bulb or a candle, higher cooler like shade),
  or a **Color** of your own, picked from the swatch's colour picker or typed as a hex code
  (`#ff8800`, `#f80`). They don't mix: setting a colour ignores the temperature, moving the
  temperature ignores the colour, and the panel says which is in use. The other value is kept.
- Per light: Intensity, colour, Angle and Elevation (Directional, Spot), Depth
  (how far above the photo the light is) and Falloff (Point, Spot), Cone (Spot). Up to 8 lights.
- For the scene: **Ambient** (the light the photo already had: −100 leaves only the added lights),
  **Softness** (how smooth the shapes are), **Shine** (specular highlights) and **Cast Shadows**
  with their opacity (100 keeps all of a light out of its shadows, lower lets some through) and
  softness.

The lights and sliders are commands and develop controls (`relight.*`, `relightLight.<i>.*`), so
the CLI, the control channel and MCP drive them like the panel does.

## Where the shapes come from

A light needs to know which way every surface faces and how far it is. Relight gets that in one of
two ways:

- **Scene Analysis (recommended).** *Analyze Photo* looks at the photo with **MoGe-2** (Microsoft,
  2025), a model that predicts for every pixel the surface normal, the depth and whether there is a
  surface at all (not sky). The normals shade the photo; they are joined with the slopes of the
  depth, which adds the steps between things; and the depth places every surface in front of or
  behind the lights and casts the shadows. The analysis takes about 15 s on a CPU (once per photo,
  in the background, shown in the activity stack) and its result is kept **with the photo**: a
  256 px grid of normals and depth in its settings (30–80 KB). Renders and exports read that and
  never need the model, so an analyzed photo looks the same on a machine without it, and in the
  web build.
- **Without it**, the relief is estimated from the photo's brightness (brighter = nearer, widely
  smoothed). It gives a pool of light a plausible roll, but it knows nothing of real shapes: a dark
  object is not a hole. Good for a quick glow; analyze the photo for anything else.

The analysis sees the photo as it looks when it is run, whole and unflipped. Flips and crops made
later carry it along; rotating the photo by 90° afterwards does not, so analyze again after that.

### The model is optional

It is not part of LightCraft and nothing requires it. *Download Model* fetches the
`Ruicheng/moge-2-vits-normal` checkpoint (134 MB, **MIT License**) from its public repository into
`<config>/models/moge-2-vits-normal` (or `$LIGHTCRAFT_RELIEF_DIR`), checks its size and SHA-256
before using it, and can be cancelled and resumed. Other locations: one base URL per line in
`<config>/models/relief-mirrors.txt`, or `LIGHTCRAFT_RELIEF_MIRRORS`. Putting `model.pt` in that
folder by hand works too.

The model runs in pure Rust (`crates/relief`, on [candle](https://github.com/huggingface/candle)):
on the GPU through Metal on macOS, on the CPU elsewhere (for now). The CLI and web builds don't
include it; they render analyzed photos like any other.

Commands: `relight.analyze`, `relight.clearGeometry`, `relight.model` (status),
`relight.model.download`, `relight.model.cancel`.

## How it renders

Relight multiplies the photo by a gain in scene-linear light, before white balance: the photo's own
light scaled by Ambient, plus what each light adds (diffuse light on the surface normal, optional
Blinn–Phong shine, optional shadows marched through the depth).

The gain is always computed on a 512 px render of the **whole, uncropped** frame and upsampled to
the output with the photo's luminance as a guide, so light does not spill over edges the small
render is too coarse to hold. Because that render does not depend on the output's size, crop or
zoom window, a thumbnail, a zoomed view and a full-size export are lit alike, and lights keep their
place on the photo when it is cropped.

A relit photo's scene-linear stage runs on the CPU (the rest of the render stays on the GPU).

## Limits

- One model, one size (ViT-S): fine shapes (hair, foliage) are smooth, and mirrors, glass and
  flat pictures of things (a poster, a screen) are seen as what they show.
- No relighting of what the photo's own light did: existing shadows and highlights stay. Ambient
  lowers them all together; it cannot remove one.
- The geometry is not written to XMP `crs:` tags (it lives in LightCraft's settings JSON).
- Copy / paste and sync leave Relight out unless it is ticked: lights and geometry belong to one
  photo's picture.
