# Interface theme

Settings > Interface > Interface Theme and View > Appearance offer System, Light and Dark.
The menu model drives both the macOS menu bar and the in-window menus. Commands and the
control channel use `view.appearance {"mode":"system"}`, `"light"` or `"dark"`.

Dark is the default, including older settings files without `appearanceMode`, and retains
the original palette. Light changes interface colours, including library cells, filmstrip,
develop panels, histogram and toolbars. Photo pixels, the Detail/Compare/Survey/Reference
surround, soft-proof background, mask overlays and colour-label swatches remain independent.

The saved `ui.json` setting is `settings.appearanceMode`. System stays saved as `system` when
the desktop changes appearance; the displayed palette is derived each frame. Manual Light
and Dark ignore desktop changes. If the operating system reports no appearance, System
uses Dark, independent of the previously displayed palette.

## Platform behaviour

On macOS and Windows the native egui/eframe integration supplies startup appearance and
live theme events. The window always inherits the native system appearance, including in
manual interface modes. Both egui style branches receive the selected interface palette;
this lets manual choices stay fixed without pinning the native window or disabling events.
The native title bar therefore follows the desktop even when the interface is manually set.
The web host uses egui's browser appearance report. Other hosts without a report use Dark.

On Linux the desktop host subscribes to the standard XDG Settings portal's
`org.freedesktop.appearance/color-scheme` before reading its initial value. Numeric values
are 1 for Dark, 2 for Light and 0 for no preference. A worker listens to `SettingChanged`
and repaints on changes, including a delayed initial result. It waits up to 250 ms at launch;
a slower portal may briefly show the fallback before repainting. There is no polling and
no periodic repaint in Preferences. Without a portal answer, an absolute-path `gsettings`
query runs once at startup. That fallback cannot follow later changes without portal signals.
A disconnected/unavailable portal is not reconnected automatically.

## Contribution sources

The Light palette, Linux portal listener and appearance labels for seven locales are adapted from luniiya's
[LightCraft PR #683](https://github.com/storytold/lightcraft/pull/683), head
`c8bca35210ac1cbfe55c017904c92a78ea692555`, under MIT OR Apache-2.0.
This implementation keeps its subscribe-before-read and common publication/repaint design,
registers the repaint context before spawning, preserves the existing photo surround and
keeps native appearance inheritance active. It offers the three choices requested by
[#589](https://github.com/storytold/lightcraft/issues/589) without the additional palette slots.
