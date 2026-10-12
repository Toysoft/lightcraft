# Mobile layout

The mobile layout is an opt-in arrangement that puts the photo first: the photo view fills the
window, and the Library, Tools and Presets panels become the rows of one accordion strip along the
bottom instead of permanent columns.

Switch to it with **Window ▸ Mobile Layout** (`cmd:ui.dock.mobile`). Switch back with
**Window ▸ Reset Panel Layout** (`cmd:ui.dock.reset`), the same command that restores the desktop
starting arrangement. Nothing about the desktop layout changes for anyone who never invokes it: the
app still starts with the desktop tree, and the mobile arrangement is installed only by that
command.

## What it does

- The photo view is alone in its tab group, across the top of the window, and does not float.
- Library, Tools and Presets share one bottom strip of fixed height (260 points) as an accordion:
  a row's header opens and closes it, and the strip's height is shared between the open rows.
- Tools starts open; Presets and Library start closed.
- The existing panel commands keep working. A panel that is in the strip — even as a closed row —
  counts as present, so `sync` does not re-open it into a column beside the photo and the per-frame
  round trip leaves the arrangement alone.

## What it deliberately does not do

- It is not a Lightroom Mobile clone: no touch-first gesture set, no cloud sync, no in-app camera,
  and no phone or tablet layout breakpoint. It is a desktop arrangement that puts the photo first.
- It is not a parity feature. No row of [ui-parity.md](ui-parity.md) counts it, because neither
  Lightroom Classic 15.5 nor Lightroom 9.6 has a one-command arrangement switch like it.
- It is not a mode with its own state: no panel is closed and no panel loses its saved place. A
  panel closed while the strip is active is remembered with the usual hidden-panel location, exactly
  as it is in the desktop arrangement.

## Reversible

`cmd:ui.dock.reset` rebuilds the desktop starting arrangement, with the Library and Presets columns
closed, as it always has. The mobile arrangement stores no state that the reset cannot rebuild, and
the arrangement itself is not persisted as a setting.
