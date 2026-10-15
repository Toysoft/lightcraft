# HDR display

How LightCraft shows an HDR edit in HDR on an HDR monitor (tracker row LR-VIEW-HDR-DISPLAY).

## The window

The window asks egui-wgpu for an HDR surface (`SurfaceConfig::prefer_hdr`). When the surface
offers `Rgba16Float` in the extended linear sRGB color space, that becomes the target format.
That color space is scRGB: `1.0` is SDR white, and larger values are brighter.

wgpu 30 does the per-platform setup, so LightCraft has no platform code and no `unsafe` for this:

| Platform | What wgpu does |
|---|---|
| Linux / Vulkan | `VK_COLOR_SPACE_EXTENDED_SRGB_LINEAR_EXT` |
| macOS / Metal | extended linear sRGB or Display P3, and the EDR layer setup |
| Windows / DX12 | `DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709` through `SetColorSpace1` |

egui draws its UI at SDR white on that surface, so the UI looks the same as on an 8-bit window.

`prefer_hdr` is emilk/egui#8703. Until an egui release has it, the root `Cargo.toml` takes every
egui crate from one git branch: egui 0.36.2 plus that commit. Whether to keep that patch or wait for
the release is a maintainer decision.

On Linux only native Wayland windows get an HDR surface (X11 windows, XWayland included, don't). So
while HDR display is on, LightCraft keeps native Wayland, and file drag-and-drop from the file
manager isn't available (winit has none on Wayland). With HDR display off it runs through XWayland
for drag-and-drop. `LIGHTCRAFT_WAYLAND=1` / `0` forces one or the other.

On KDE Plasma (Wayland), the surface is offered even with the system's HDR mode off: values above
`1.0` are then clipped to SDR white. Switching the system's HDR mode while LightCraft runs takes
effect live, with nothing to recreate.

## The loupe

`RenderJob::with_hdr_display(true)` is set by the loupe when the window is HDR. For an HDR edit
(not for overlays, soft proofs, windowed renders or high-bit-depth requests), the job renders its
HDR rendition as well:

- the extra render is `OutputDepth::F32Hdr` in sRGB primaries: display-linear, `1.0` = SDR white;
- it goes into `Rendered::deep`;
- the usual 8-bit SDR rendition stays the image that overlays, previews, thumbnails and the
  histogram use.

ui-egui keeps the HDR values on the loupe texture (`hdr_view::HdrPixels`) and paints them through
the host's `hdr_view::HdrPresenter`. ui-egui stays renderer-agnostic, since the web build uses glow.

The desktop app's presenter is `lightcraft-hdr-display::HdrDisplay`:

- it uploads the values as an `Rgba16Float` texture, only when the render's key changes;
- it draws them with an egui-wgpu paint callback over the image's rectangle;
- when a display headroom is set (`HdrDisplay::set_headroom`, the highest value the display shows),
  it eases highlights above 80 % of it in towards it, keeping hue, instead of letting the panel clip.

HDR edits on an HDR window render the whole frame (no zoomed 8-bit windows). The HDR render runs
on the CPU.

## Settings and checks

- **Settings ▸ Performance ▸ Show HDR photos in HDR on HDR displays** (on by default; applies after
  a restart, since the surface is chosen when the window is created).
- **`LIGHTCRAFT_HDR_DISPLAY=0` / `1`** overrides the setting.
- **`ui.inspect` → `hdrDisplay`:** `active` (the window is HDR) and `loupeHdr` (the loupe paints an
  HDR rendition).
- Screenshots of an HDR window work: egui-wgpu reads the frame back as sRGB, with HDR clipped.

## Status

- **Linux:** confirmed on an HDR monitor (KDE Plasma 6.6 Wayland, Intel Xe, Mesa 26).
- **macOS:** planned on top of this. Choose extended linear sRGB or Display P3, check that the
  swapchain gets EDR, and feed wgpu's `DisplayHdrInfo::tone_map_headroom` to
  `HdrDisplay::set_headroom`, re-queried when the display, the window's screen or the brightness
  changes. The surface isn't reachable through eframe today, so this needs a way to query it.
- **Windows:** untested.
- **Not yet:** zoomed windows, compare, the second window, a GPU HDR render, and a "Preview for SDR
  display" toggle.
